use std::process::Command;
use std::time::Duration;

use gaap::contracts::{
    AgentRunRequest, CapabilityIdentity, ContractSupport, EffectClass, EffectExecutionStatus,
    ProtectedEffectRequest,
};
use gaap::runtime::{ExecutorPort, PermissionPolicy, PolicyDecision};
use gaap_mcp::{LaunchArtifact, McpExecutor, ServerConfig, SubjectObservation, TransportLimits};
use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

fn hash(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
fn server(mode: &str, dir: &TempDir) -> ServerConfig {
    let python = Command::new("python3")
        .args(["-c", "import sys; print(sys.executable)"])
        .output()
        .unwrap();
    assert!(python.status.success());
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/server.py");
    let mut config = ServerConfig::new(
        std::path::PathBuf::from(String::from_utf8(python.stdout).unwrap().trim()),
        "fixture",
        "1",
    );
    config.arguments = vec![
        fixture.display().to_string(),
        mode.into(),
        dir.path().join("dispatch").display().to_string(),
    ];
    config.working_directory = dir.path().into();
    config.executable = config.executable.canonicalize().unwrap();
    config.artifacts = vec![
        LaunchArtifact {
            path: config.executable.clone(),
            digest: hash(&std::fs::read(&config.executable).unwrap()),
        },
        LaunchArtifact {
            path: fixture,
            digest: hash(include_bytes!("fixtures/server.py")),
        },
    ];
    config.artifact_digest = config.artifact_manifest_digest().unwrap();
    config
}
fn limits() -> TransportLimits {
    TransportLimits {
        timeout: Duration::from_millis(500),
        ..TransportLimits::default()
    }
}
fn request(capability: CapabilityIdentity) -> (AgentRunRequest, ContractSupport) {
    let mut request: AgentRunRequest = serde_json::from_str(include_str!(
        "../../../examples/contracts/v0.1.0/agent-run-request.json"
    ))
    .unwrap();
    request.requested_capability = capability;
    let support = ContractSupport::new(request.policies.clone());
    (request, support)
}
fn fixture(mode: &str, allowed: bool) -> (TempDir, McpExecutor, ProtectedEffectRequest) {
    let dir = TempDir::new().unwrap();
    let mut executor = McpExecutor::discover(server(mode, &dir), limits()).unwrap();
    let capability = executor.tools()[0].capability.clone();
    let (request, support) = request(capability.clone());
    let observed = request.subject.clone();
    executor
        .bind_run(
            request.clone(),
            support,
            PermissionPolicy {
                policy_decision: Some(PolicyDecision::Allow),
                risk_tags: vec![],
                wrapper_chain: vec![],
                approvals: vec![],
            },
            move |_: &ProtectedEffectRequest| {
                Ok(SubjectObservation {
                    subject: observed.clone(),
                    cost_micros: 0,
                    model_tokens: 0,
                })
            },
        )
        .unwrap();
    if allowed {
        executor.approve_capability(&capability).unwrap();
    }
    let effect = executor
        .prepare_effect(
            "echo",
            json!({"message":"hello"}),
            "effect-1".into(),
            1,
            request.subject,
            EffectClass::Observation,
        )
        .unwrap();
    (dir, executor, effect)
}

#[test]
fn invalid_server_configuration_fails_before_spawning() {
    let config = ServerConfig::new("relative-program".into(), "fixture", "1");
    assert!(McpExecutor::discover(config, TransportLimits::default()).is_err());
}

#[test]
fn discovered_tool_needs_exact_host_approval_before_dispatch() {
    let (dir, mut executor, effect) = fixture("normal", false);
    let blocked = executor.execute(&effect).unwrap();
    assert_eq!(
        blocked.execution_status,
        EffectExecutionStatus::AwaitingAuthority
    );
    assert!(!dir.path().join("dispatch").exists());
    let capability = effect.capability.clone();
    executor.approve_capability(&capability).unwrap();
    let allowed = executor.execute(&effect).unwrap();
    assert_eq!(allowed.execution_status, EffectExecutionStatus::Executed);
    assert_eq!(allowed.usage.tool_calls, 1);
    assert!(dir.path().join("dispatch").exists());
    assert!(executor.execute(&effect).is_err());
}

#[test]
fn changed_schema_description_or_annotations_revoke_prior_capability_authority() {
    for mode in ["schema_drift", "description_drift", "annotation_drift"] {
        let (dir, mut executor, effect) = fixture(mode, true);
        let result = executor.execute(&effect).unwrap();
        assert_eq!(
            result.execution_status,
            EffectExecutionStatus::AwaitingAuthority,
            "{mode}"
        );
        assert!(!dir.path().join("dispatch").exists(), "{mode}");
    }
}

#[test]
fn protocol_identity_and_inventory_fail_before_any_effect() {
    for mode in [
        "version",
        "identity",
        "duplicate_tool",
        "invalid_schema",
        "remote_schema",
    ] {
        let dir = TempDir::new().unwrap();
        assert!(
            McpExecutor::discover(server(mode, &dir), limits()).is_err(),
            "{mode}"
        );
        assert!(!dir.path().join("dispatch").exists());
    }
}

#[test]
fn uncertain_transport_and_malicious_outputs_preserve_attempted_effects() {
    for mode in [
        "timeout",
        "exit",
        "malformed",
        "duplicate_key",
        "partial",
        "oversized",
        "stderr",
        "wrong_id",
        "unknown_content",
        "bad_structured",
        "scalar_structured",
        "notify_drift",
    ] {
        let (dir, mut executor, effect) = fixture(mode, true);
        let result = executor.execute(&effect).unwrap();
        assert_eq!(
            result.execution_status,
            EffectExecutionStatus::UnknownOutcome,
            "{mode}: {:?}",
            result.reason
        );
        assert_eq!(result.usage.tool_calls, 1, "{mode}");
        assert!(dir.path().join("dispatch").exists(), "{mode}");
        assert!(executor.execute(&effect).is_err(), "{mode}");
        assert!(!executor.call_evidence().is_empty());
    }
}

#[test]
fn cancellation_after_dispatch_is_unknown_and_doesnt_replay() {
    let (dir, mut executor, effect) = fixture("cancel", true);
    let cancellation = executor.cancellation_handle();
    let marker = dir.path().join("dispatch");
    let canceller = std::thread::spawn(move || {
        for _ in 0..100 {
            if marker.exists() {
                cancellation.cancel();
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("fixture didn't dispatch");
    });
    let result = executor.execute(&effect).unwrap();
    canceller.join().unwrap();
    assert_eq!(
        result.execution_status,
        EffectExecutionStatus::UnknownOutcome
    );
    assert!(result.reason.unwrap().contains("mcp.cancelled"));
    assert!(executor.execute(&effect).is_err());
}

#[test]
fn unknown_tool_and_invalid_arguments_are_rejected_before_dispatch() {
    let (dir, mut executor, effect) = fixture("normal", true);
    for (name, args) in [
        ("missing", json!({"message":"hello"})),
        ("echo", json!({"message":1})),
        ("echo", json!({"message":"hello","approval":true})),
    ] {
        assert!(
            executor
                .prepare_effect(
                    name,
                    args,
                    "other".into(),
                    2,
                    effect.subject.clone(),
                    EffectClass::Observation
                )
                .is_err()
        );
    }
    assert!(!dir.path().join("dispatch").exists());
}

struct PlannedAgent {
    effect: Option<ProtectedEffectRequest>,
}
impl gaap::runtime::AgentAdapter for PlannedAgent {
    fn plan(
        &mut self,
        _request: &AgentRunRequest,
    ) -> Result<
        gaap::runtime::PortObservation<gaap::runtime::PlanProposal>,
        gaap::runtime::RuntimePortError,
    > {
        Ok(gaap::runtime::PortObservation {
            value: gaap::runtime::PlanProposal {
                plan_digest: hash(b"fixture plan"),
            },
            usage: zero_usage(),
        })
    }
    fn next_effect(
        &mut self,
        _context: gaap::runtime::AgentRunContext<'_>,
    ) -> Result<
        gaap::runtime::PortObservation<gaap::runtime::AgentStep>,
        gaap::runtime::RuntimePortError,
    > {
        Ok(gaap::runtime::PortObservation {
            value: self
                .effect
                .take()
                .map(|request| {
                    gaap::runtime::AgentStep::ProtectedEffect(Box::new(
                        gaap::runtime::ProtectedEffectProposal { request },
                    ))
                })
                .unwrap_or(gaap::runtime::AgentStep::Finish),
            usage: zero_usage(),
        })
    }
}
fn zero_usage() -> gaap::contracts::ResourceUsage {
    gaap::contracts::ResourceUsage {
        cost_micros: 0,
        elapsed_ms: 0,
        model_tokens: 0,
        tool_calls: 0,
    }
}
struct IndependentVerifier {
    passes: bool,
}
impl gaap::runtime::VerifierPort for IndependentVerifier {
    fn verify(
        &mut self,
        context: gaap::runtime::VerificationContext<'_>,
    ) -> Result<
        gaap::runtime::PortObservation<gaap::runtime::VerificationReport>,
        gaap::runtime::RuntimePortError,
    > {
        Ok(gaap::runtime::PortObservation {
            value: gaap::runtime::VerificationReport {
                subject_digest: context.current_subject.digest.clone(),
                verifier_id: "independent-test-verifier".into(),
                verdict: if self.passes {
                    gaap::contracts::VerificationVerdict::Pass
                } else {
                    gaap::contracts::VerificationVerdict::Fail
                },
                evidence: vec![
                    gaap::contracts::EvidenceReference {
                        evidence_type: gaap::contracts::EvidenceType::CommandOutput,
                        digest: hash(b"independent verification output"),
                        locator: None,
                    },
                    gaap::contracts::EvidenceReference {
                        evidence_type: gaap::contracts::EvidenceType::Artifact,
                        digest: context.current_subject.digest.clone(),
                        locator: None,
                    },
                ],
            },
            usage: zero_usage(),
        })
    }
}

#[test]
fn real_mcp_executor_runs_through_engine_and_seals_independently_valid_receipts() {
    for (mode, passes, expected) in [
        ("normal", true, gaap::contracts::AgentRunStatus::Completed),
        ("normal", false, gaap::contracts::AgentRunStatus::Blocked),
        ("timeout", true, gaap::contracts::AgentRunStatus::Failed),
        ("tool_error", true, gaap::contracts::AgentRunStatus::Failed),
        (
            "unknown_content",
            true,
            gaap::contracts::AgentRunStatus::Failed,
        ),
    ] {
        let (dir, executor, effect) = fixture(mode, true);
        let (request, support) = request(effect.capability.clone());
        let runtime_support = gaap::runtime::RuntimeSupport::new(
            support.clone(),
            vec![effect.capability.clone()],
            PermissionPolicy {
                policy_decision: Some(PolicyDecision::Allow),
                risk_tags: vec![],
                wrapper_chain: vec![],
                approvals: vec![],
            },
            gaap::contracts::EffectUsage {
                cost_micros: 0,
                elapsed_ms: 0,
                model_tokens: 0,
                tool_calls: 1,
            },
            vec![],
            vec!["independent-test-verifier".into()],
        );
        let config = gaap::runtime::RuntimeConfig {
            implementer_id: "test-agent".into(),
            executor_identity: executor.identity().clone(),
            max_effects: 1,
            budget_thresholds: gaap::runtime::BudgetThresholds {
                warn_micros: 100,
                approval_micros: 1000,
                hard_stop_micros: 1000000,
            },
        };
        let mut engine = gaap::runtime::AgentRunEngine::new(
            runtime_support,
            config,
            PlannedAgent {
                effect: Some(effect.clone()),
            },
            executor,
            IndependentVerifier { passes },
        );
        let execution = engine.run(request.clone()).unwrap();
        assert_eq!(execution.receipt.body.terminal_status, expected, "{mode}");
        gaap::contracts::verify_terminal_receipt(&request, &support, &execution.receipt).unwrap();
        assert_eq!(execution.protected_effect_results.len(), 1, "{mode}");
        gaap::contracts::verify_protected_effect_result(
            &request,
            &support,
            &effect,
            &execution.protected_effect_results[0],
        )
        .unwrap();
        assert!(dir.path().join("dispatch").exists(), "{mode}");
        if !passes {
            assert_eq!(
                execution.protected_effect_results[0].body.execution_status,
                EffectExecutionStatus::Executed
            );
        }
    }
}

#[test]
fn prepared_effect_bindings_cant_be_changed_after_host_preparation() {
    let (dir, mut executor, effect) = fixture("normal", true);
    for field in [
        "subject", "profile", "scope", "input", "run", "policy", "effect",
    ] {
        let mut altered = effect.clone();
        match field {
            "subject" => altered.subject.digest = hash(b"different subject"),
            "profile" => altered.sandbox_profile.digest = hash(b"fake isolation"),
            "scope" => altered.requested_scopes.clear(),
            "input" => altered.input_digest = hash(b"different arguments"),
            "run" => altered.run_id = "other-run".into(),
            "policy" => altered.policies[0].digest = hash(b"other policy"),
            "effect" => altered.effect_id = "other-effect".into(),
            _ => unreachable!(),
        }
        assert!(executor.execute(&altered).is_err(), "{field}");
        assert!(!dir.path().join("dispatch").exists(), "{field}");
    }
    assert_eq!(
        executor.execute(&effect).unwrap().execution_status,
        EffectExecutionStatus::Executed
    );
}

#[test]
fn explicit_environment_allowlist_is_the_only_child_environment() {
    if std::env::var("GAAP_ENVIRONMENT_TEST_CHILD").as_deref() != Ok("1") {
        let result = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "explicit_environment_allowlist_is_the_only_child_environment",
                "--nocapture",
            ])
            .env("GAAP_ENVIRONMENT_TEST_CHILD", "1")
            .env("GAAP_AMBIENT", "must-not-leak")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
        return;
    }
    assert_eq!(std::env::var("GAAP_AMBIENT").unwrap(), "must-not-leak");
    let dir = TempDir::new().unwrap();
    let mut config = server("environment", &dir);
    config
        .environment
        .insert("GAAP_ALLOWED".into(), "explicit".into());
    let mut executor = McpExecutor::discover(config, limits()).unwrap();
    let capability = executor.tools()[0].capability.clone();
    let (run, support) = request(capability.clone());
    let observed = run.subject.clone();
    executor
        .bind_run(
            run.clone(),
            support,
            PermissionPolicy {
                policy_decision: Some(PolicyDecision::Allow),
                risk_tags: vec![],
                wrapper_chain: vec![],
                approvals: vec![],
            },
            move |_: &ProtectedEffectRequest| {
                Ok(SubjectObservation {
                    subject: observed.clone(),
                    cost_micros: 0,
                    model_tokens: 0,
                })
            },
        )
        .unwrap();
    executor.approve_capability(&capability).unwrap();
    let effect = executor
        .prepare_effect(
            "echo",
            json!({"message":"hello"}),
            "environment".into(),
            1,
            run.subject,
            EffectClass::Observation,
        )
        .unwrap();
    let result = executor.execute(&effect).unwrap();
    assert_eq!(result.execution_status, EffectExecutionStatus::Executed);
    assert_eq!(
        executor.call_evidence()[0].output.as_ref().unwrap()["structuredContent"]["echo"],
        "explicit:absent"
    );
}

#[test]
fn host_permission_denial_blocks_an_approved_tool_before_dispatch() {
    let dir = TempDir::new().unwrap();
    let mut executor = McpExecutor::discover(server("normal", &dir), limits()).unwrap();
    let capability = executor.tools()[0].capability.clone();
    let (run, support) = request(capability.clone());
    let observed = run.subject.clone();
    executor
        .bind_run(
            run.clone(),
            support,
            PermissionPolicy {
                policy_decision: Some(PolicyDecision::Deny),
                risk_tags: vec![],
                wrapper_chain: vec![],
                approvals: vec![],
            },
            move |_: &ProtectedEffectRequest| {
                Ok(SubjectObservation {
                    subject: observed.clone(),
                    cost_micros: 0,
                    model_tokens: 0,
                })
            },
        )
        .unwrap();
    executor.approve_capability(&capability).unwrap();
    let effect = executor
        .prepare_effect(
            "echo",
            json!({"message":"hello"}),
            "denied".into(),
            1,
            run.subject,
            EffectClass::Observation,
        )
        .unwrap();
    let result = executor.execute(&effect).unwrap();
    assert_eq!(result.execution_status, EffectExecutionStatus::Denied);
    assert_eq!(result.reason.as_deref(), Some("permission.denied"));
    assert_eq!(result.usage.tool_calls, 0);
    assert!(!dir.path().join("dispatch").exists());
}

#[test]
fn actual_mutation_survives_failed_independent_verification() {
    let dir = TempDir::new().unwrap();
    let subject_path = dir.path().join("subject.txt");
    std::fs::write(&subject_path, "initial subject").unwrap();
    let mut executor = McpExecutor::discover(server("mutation", &dir), limits()).unwrap();
    let capability = executor.tools()[0].capability.clone();
    let (mut run, support) = request(capability.clone());
    run.subject.kind = gaap::contracts::SubjectKind::Artifact;
    run.subject.locator = subject_path.display().to_string();
    run.subject.digest = hash(b"initial subject");
    for approval in &mut run.approval_context {
        approval.subject_digest = run.subject.digest.clone();
    }
    let locator = run.subject.locator.clone();
    executor
        .bind_run(
            run.clone(),
            support.clone(),
            PermissionPolicy {
                policy_decision: Some(PolicyDecision::Allow),
                risk_tags: vec![],
                wrapper_chain: vec![],
                approvals: vec![],
            },
            move |_: &ProtectedEffectRequest| {
                Ok(SubjectObservation {
                    subject: gaap::contracts::Subject {
                        kind: gaap::contracts::SubjectKind::Artifact,
                        locator: locator.clone(),
                        digest: hash(&std::fs::read(&subject_path).unwrap()),
                    },
                    cost_micros: 0,
                    model_tokens: 0,
                })
            },
        )
        .unwrap();
    executor.approve_capability(&capability).unwrap();
    let effect = executor
        .prepare_effect(
            "echo",
            json!({"message":"hello"}),
            "mutation".into(),
            1,
            run.subject.clone(),
            EffectClass::Mutation,
        )
        .unwrap();
    let runtime_support = gaap::runtime::RuntimeSupport::new(
        support.clone(),
        vec![capability],
        PermissionPolicy {
            policy_decision: Some(PolicyDecision::Allow),
            risk_tags: vec![],
            wrapper_chain: vec![],
            approvals: vec![],
        },
        gaap::contracts::EffectUsage {
            cost_micros: 0,
            elapsed_ms: 0,
            model_tokens: 0,
            tool_calls: 1,
        },
        vec![],
        vec!["independent-test-verifier".into()],
    );
    let config = gaap::runtime::RuntimeConfig {
        implementer_id: "test-agent".into(),
        executor_identity: executor.identity().clone(),
        max_effects: 1,
        budget_thresholds: gaap::runtime::BudgetThresholds {
            warn_micros: 100,
            approval_micros: 1000,
            hard_stop_micros: 1000000,
        },
    };
    let mut engine = gaap::runtime::AgentRunEngine::new(
        runtime_support,
        config,
        PlannedAgent {
            effect: Some(effect.clone()),
        },
        executor,
        IndependentVerifier { passes: false },
    );
    let execution = engine.run(run.clone()).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join("subject.txt")).unwrap(),
        "changed subject"
    );
    assert_eq!(
        execution.receipt.body.terminal_status,
        gaap::contracts::AgentRunStatus::Blocked
    );
    assert_eq!(
        execution.protected_effect_results[0].body.execution_status,
        EffectExecutionStatus::Executed
    );
    assert_eq!(
        execution.receipt.body.resulting_subject_digest,
        hash(b"changed subject")
    );
    gaap::contracts::verify_terminal_receipt(&run, &support, &execution.receipt).unwrap();
    gaap::contracts::verify_protected_effect_result(
        &run,
        &support,
        &effect,
        &execution.protected_effect_results[0],
    )
    .unwrap();
}

#[test]
fn late_duplicate_response_poisoning_blocks_future_dispatch_without_erasing_the_first_effect() {
    let (dir, mut executor, effect) = fixture("duplicate_response", true);
    let first = executor.execute(&effect).unwrap();
    assert!(matches!(
        first.execution_status,
        EffectExecutionStatus::Executed | EffectExecutionStatus::UnknownOutcome
    ));
    assert!(dir.path().join("dispatch").exists());
    // Observe the duplicate through the next actual protocol operation, not a guessed delay.
    let error = executor.refresh_tools().unwrap_err();
    assert!(
        error.code == "mcp.response_identity" || error.detail.contains("duplicate"),
        "{error}"
    );
    assert_eq!(executor.call_evidence().len(), 1);
    assert!(executor.execute(&effect).is_err());
}

#[test]
fn altered_launch_artifacts_fail_before_spawn() {
    let dir = TempDir::new().unwrap();
    let mut config = server("normal", &dir);
    let copy = dir.path().join("changed-server.py");
    std::fs::write(&copy, "raise RuntimeError('must never launch')").unwrap();
    config.arguments[0] = copy.display().to_string();
    config.artifacts[1].path = copy;
    config.artifact_digest = config.artifact_manifest_digest().unwrap();
    let error = McpExecutor::discover(config, limits()).err().unwrap();
    assert_eq!(error.code, "mcp.artifact_identity");
    assert!(!dir.path().join("dispatch").exists());
}

#[test]
fn endless_empty_pages_reject_within_the_total_discovery_bound() {
    let dir = TempDir::new().unwrap();
    let begin = std::time::Instant::now();
    let error = McpExecutor::discover(server("empty_pages", &dir), limits())
        .err()
        .unwrap();
    assert!(
        matches!(error.code, "mcp.pagination" | "mcp.discovery_failed"),
        "{error}"
    );
    assert!(begin.elapsed() < Duration::from_secs(2));
    assert!(!dir.path().join("dispatch").exists());
}

#[test]
fn stale_host_subject_blocks_dispatch_at_the_observation_guard() {
    let dir = TempDir::new().unwrap();
    let mut executor = McpExecutor::discover(server("normal", &dir), limits()).unwrap();
    let capability = executor.tools()[0].capability.clone();
    let (run, support) = request(capability.clone());
    let mut observed = run.subject.clone();
    observed.digest = hash(b"new subject");
    executor
        .bind_run(
            run.clone(),
            support,
            PermissionPolicy {
                policy_decision: Some(PolicyDecision::Allow),
                risk_tags: vec![],
                wrapper_chain: vec![],
                approvals: vec![],
            },
            move |_: &ProtectedEffectRequest| {
                Ok(SubjectObservation {
                    subject: observed.clone(),
                    cost_micros: 0,
                    model_tokens: 0,
                })
            },
        )
        .unwrap();
    executor.approve_capability(&capability).unwrap();
    let effect = executor
        .prepare_effect(
            "echo",
            json!({"message":"hello"}),
            "stale".into(),
            1,
            run.subject,
            EffectClass::Observation,
        )
        .unwrap();
    let error = executor.execute(&effect).unwrap_err();
    assert!(error.message().starts_with("mcp.stale_subject"));
    assert!(!dir.path().join("dispatch").exists());
}

#[test]
fn full_output_budget_keeps_a_minimal_attempt_record_and_the_output_digest() {
    let dir = TempDir::new().unwrap();
    let mut tight = limits();
    tight.max_retained_bytes = 4096;
    let mut executor = McpExecutor::discover(server("large_result", &dir), tight).unwrap();
    let capability = executor.tools()[0].capability.clone();
    let (run, support) = request(capability.clone());
    let observed = run.subject.clone();
    executor
        .bind_run(
            run.clone(),
            support,
            PermissionPolicy {
                policy_decision: Some(PolicyDecision::Allow),
                risk_tags: vec![],
                wrapper_chain: vec![],
                approvals: vec![],
            },
            move |_: &ProtectedEffectRequest| {
                Ok(SubjectObservation {
                    subject: observed.clone(),
                    cost_micros: 0,
                    model_tokens: 0,
                })
            },
        )
        .unwrap();
    executor.approve_capability(&capability).unwrap();
    let effect = executor
        .prepare_effect(
            "echo",
            json!({"message":"hello"}),
            "retention".into(),
            1,
            run.subject,
            EffectClass::Observation,
        )
        .unwrap();
    assert_eq!(
        executor.execute(&effect).unwrap().execution_status,
        EffectExecutionStatus::Executed
    );
    let record = &executor.call_evidence()[0];
    assert!(record.output_omitted);
    assert!(record.output.is_none());
    assert!(record.output_digest.is_some());
    assert!(executor.execute(&effect).is_err());
}

#[test]
fn reconciliation_capacity_is_reserved_before_dispatch() {
    let dir = TempDir::new().unwrap();
    let mut tight = limits();
    tight.max_retained_bytes = 1024;
    let mut executor = McpExecutor::discover(server("normal", &dir), tight).unwrap();
    let capability = executor.tools()[0].capability.clone();
    let (run, support) = request(capability.clone());
    let observed = run.subject.clone();
    executor
        .bind_run(
            run.clone(),
            support,
            PermissionPolicy {
                policy_decision: Some(PolicyDecision::Allow),
                risk_tags: vec![],
                wrapper_chain: vec![],
                approvals: vec![],
            },
            move |_: &ProtectedEffectRequest| {
                Ok(SubjectObservation {
                    subject: observed.clone(),
                    cost_micros: 0,
                    model_tokens: 0,
                })
            },
        )
        .unwrap();
    executor.approve_capability(&capability).unwrap();
    let effect = executor
        .prepare_effect(
            "echo",
            json!({"message":"hello"}),
            "reserve".into(),
            1,
            run.subject,
            EffectClass::Observation,
        )
        .unwrap();
    let error = executor.execute(&effect).unwrap_err();
    assert!(error.message().starts_with("mcp.retention_limit"));
    assert!(!dir.path().join("dispatch").exists());
}

fn observed_fixture(
    budget_ms: u64,
    mut observer: impl FnMut(
        usize,
        &ProtectedEffectRequest,
    ) -> Result<SubjectObservation, gaap_mcp::McpError>
    + Send
    + 'static,
) -> (TempDir, McpExecutor, ProtectedEffectRequest) {
    let dir = TempDir::new().unwrap();
    let mut executor = McpExecutor::discover(server("normal", &dir), limits()).unwrap();
    let capability = executor.tools()[0].capability.clone();
    let (mut run, support) = request(capability.clone());
    run.resource_budget.max_elapsed_ms = budget_ms;
    let mut observations = 0;
    executor
        .bind_run(
            run.clone(),
            support,
            PermissionPolicy {
                policy_decision: Some(PolicyDecision::Allow),
                risk_tags: vec![],
                wrapper_chain: vec![],
                approvals: vec![],
            },
            move |effect: &ProtectedEffectRequest| {
                observations += 1;
                observer(observations, effect)
            },
        )
        .unwrap();
    executor.approve_capability(&capability).unwrap();
    let effect = executor
        .prepare_effect(
            "echo",
            json!({"message":"hello"}),
            "observed".into(),
            1,
            run.subject,
            EffectClass::Observation,
        )
        .unwrap();
    (dir, executor, effect)
}
fn same_subject(effect: &ProtectedEffectRequest) -> SubjectObservation {
    SubjectObservation {
        subject: effect.subject.clone(),
        cost_micros: 0,
        model_tokens: 0,
    }
}

#[test]
fn subject_changes_during_discovery_block_the_actual_dispatch_guard() {
    let (dir, mut executor, effect) = observed_fixture(10000, |count, effect| {
        let mut observation = same_subject(effect);
        if count == 2 {
            observation.subject.digest = hash(b"changed during discovery");
        }
        Ok(observation)
    });
    let result = executor.execute(&effect).unwrap();
    assert_eq!(result.execution_status, EffectExecutionStatus::Denied);
    assert!(result.reason.unwrap().starts_with("mcp.stale_subject"));
    assert_eq!(result.usage.tool_calls, 0);
    assert!(!dir.path().join("dispatch").exists());
    assert!(!executor.call_evidence()[0].dispatched);
}

#[test]
fn expired_run_budget_refuses_dispatch() {
    let (dir, mut executor, effect) = observed_fixture(1, |_, effect| Ok(same_subject(effect)));
    std::thread::sleep(Duration::from_millis(20));
    let result = executor.execute(&effect).unwrap();
    assert_eq!(result.execution_status, EffectExecutionStatus::Denied);
    assert!(result.reason.unwrap().starts_with("mcp.budget_elapsed"));
    assert!(!dir.path().join("dispatch").exists());
    assert_eq!(result.usage.tool_calls, 0);
}

#[test]
fn unavailable_host_measurements_keep_the_dispatched_record_and_refuse_replay() {
    let (dir, mut executor, effect) = observed_fixture(10000, |count, effect| {
        if count == 3 {
            return Err(gaap_mcp::McpError {
                code: "host.usage_unknown",
                detail: "usage measurement unavailable".into(),
            });
        }
        Ok(same_subject(effect))
    });
    let result = executor.execute(&effect).unwrap();
    assert_eq!(
        result.execution_status,
        EffectExecutionStatus::UnknownOutcome
    );
    assert!(result.reason.unwrap().starts_with("host.usage_unknown"));
    assert!(dir.path().join("dispatch").exists());
    let record = &executor.call_evidence()[0];
    assert!(record.dispatched);
    assert!(record.host_usage.is_none());
    assert!(record.output_digest.is_some());
    assert!(executor.execute(&effect).is_err());
}

#[test]
fn cancellation_before_execution_cant_dispatch() {
    let (dir, mut executor, effect) = fixture("normal", true);
    executor.cancellation_handle().cancel();
    let result = executor.execute(&effect).unwrap();
    assert_eq!(result.execution_status, EffectExecutionStatus::Denied);
    assert!(result.reason.unwrap().starts_with("mcp.cancelled"));
    assert_eq!(result.usage.tool_calls, 0);
    assert!(!dir.path().join("dispatch").exists());
}

#[test]
fn unexpected_subject_changes_after_observation_keep_an_unknown_effect() {
    let (dir, mut executor, effect) = observed_fixture(10000, |count, effect| {
        let mut observation = same_subject(effect);
        if count == 3 {
            observation.subject.digest = hash(b"independently observed change");
        }
        Ok(observation)
    });
    let result = executor.execute(&effect).unwrap();
    assert_eq!(
        result.execution_status,
        EffectExecutionStatus::UnknownOutcome
    );
    assert!(
        result
            .reason
            .unwrap()
            .starts_with("mcp.unexpected_subject_change")
    );
    assert!(dir.path().join("dispatch").exists());
    assert_ne!(
        result.observed_post_effect_subject.unwrap().digest,
        effect.subject.digest
    );
    assert!(executor.call_evidence()[0].dispatched);
    assert!(executor.execute(&effect).is_err());
}
