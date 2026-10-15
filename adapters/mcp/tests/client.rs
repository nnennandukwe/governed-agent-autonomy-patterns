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
    config.working_directory = dir.path().canonicalize().unwrap();
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
    let confirmation = dir.path().join("dispatch");
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
                    effect_confirmed: confirmation.exists(),
                    subject: observed.clone(),
                    cost_micros: Some(0),
                    model_tokens: Some(0),
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
        "unsafe_integer",
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
        (
            "reported_only",
            true,
            gaap::contracts::AgentRunStatus::Failed,
        ),
        ("timeout", true, gaap::contracts::AgentRunStatus::Failed),
        ("tool_error", true, gaap::contracts::AgentRunStatus::Failed),
        (
            "unknown_content",
            true,
            gaap::contracts::AgentRunStatus::Failed,
        ),
        (
            "fresh_probe_failure",
            true,
            gaap::contracts::AgentRunStatus::Blocked,
        ),
    ] {
        let (dir, executor, effect) = if mode == "host_subject_change" {
            observed_fixture(60000, |count, effect| {
                let mut observation = same_subject(effect);
                if count == 3 {
                    observation.subject.digest = hash(b"independent changed subject");
                }
                Ok(observation)
            })
        } else if mode == "fresh_probe_failure" {
            observed_fixture(60000, |count, effect| {
                if count == 2 {
                    return Err(gaap_mcp::McpError {
                        code: "host.read_failed",
                        detail: "fresh observation unavailable".into(),
                    });
                }
                Ok(same_subject(effect))
            })
        } else {
            fixture(mode, true)
        };
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
        assert_eq!(
            dir.path().join("dispatch").exists(),
            !matches!(mode, "reported_only" | "fresh_probe_failure"),
            "{mode}"
        );
        if mode == "fresh_probe_failure" {
            assert_eq!(execution.receipt.body.usage.tool_calls, 0);
            assert_eq!(
                execution.protected_effect_results[0].body.execution_status,
                EffectExecutionStatus::Denied
            );
            assert!(
                execution.protected_effect_results[0]
                    .body
                    .executor
                    .is_none()
            );
            assert!(
                !execution
                    .receipt
                    .body
                    .events
                    .iter()
                    .any(|event| matches!(event, gaap::contracts::RunEvent::ToolExecution { .. }))
            );
        }
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
                    effect_confirmed: true,
                    subject: observed.clone(),
                    cost_micros: Some(0),
                    model_tokens: Some(0),
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
                    effect_confirmed: true,
                    subject: observed.clone(),
                    cost_micros: Some(0),
                    model_tokens: Some(0),
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
fn actual_mutation_survives_failed_verification_or_rejected_output() {
    for mode in ["mutation", "mutation_unsafe_integer", "mutation_unmeasured"] {
        let dir = TempDir::new().unwrap();
        let subject_path = dir.path().join("subject.txt");
        std::fs::write(&subject_path, "initial subject").unwrap();
        let mut executor = McpExecutor::discover(server(mode, &dir), limits()).unwrap();
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
                    let bytes = std::fs::read(&subject_path).unwrap();
                    Ok(SubjectObservation {
                        effect_confirmed: bytes == b"changed subject",
                        subject: gaap::contracts::Subject {
                            kind: gaap::contracts::SubjectKind::Artifact,
                            locator: locator.clone(),
                            digest: hash(&bytes),
                        },
                        cost_micros: if mode == "mutation_unmeasured" && bytes == b"changed subject"
                        {
                            None
                        } else {
                            Some(0)
                        },
                        model_tokens: Some(0),
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
            if mode == "mutation" {
                gaap::contracts::AgentRunStatus::Blocked
            } else {
                gaap::contracts::AgentRunStatus::Failed
            }
        );
        assert_eq!(
            execution.protected_effect_results[0].body.execution_status,
            if mode == "mutation" {
                EffectExecutionStatus::Executed
            } else {
                EffectExecutionStatus::UnknownOutcome
            }
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
    let copy = copy.canonicalize().unwrap();
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
                    effect_confirmed: true,
                    subject: observed.clone(),
                    cost_micros: Some(0),
                    model_tokens: Some(0),
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
    tight.max_retained_bytes = 16384;
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
                    effect_confirmed: true,
                    subject: observed.clone(),
                    cost_micros: Some(0),
                    model_tokens: Some(0),
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
    assert!(record.observed_subject.is_some());
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
                    effect_confirmed: true,
                    subject: observed.clone(),
                    cost_micros: Some(0),
                    model_tokens: Some(0),
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
        effect_confirmed: true,
        subject: effect.subject.clone(),
        cost_micros: Some(0),
        model_tokens: Some(0),
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
    assert!(result.observed_post_effect_subject.is_none());
    assert_ne!(
        executor.call_evidence()[0]
            .observed_subject
            .as_ref()
            .unwrap()
            .digest,
        effect.subject.digest
    );
    assert!(executor.call_evidence()[0].dispatched);
    assert!(executor.execute(&effect).is_err());
}

#[test]
fn server_ping_gets_an_empty_sdk_response_without_granting_effect_authority() {
    let (dir, mut executor, effect) = fixture("ping", true);
    let result = executor.execute(&effect).unwrap();
    assert_eq!(result.execution_status, EffectExecutionStatus::Executed);
    assert_eq!(result.usage.tool_calls, 1);
    assert!(dir.path().join("dispatch").exists());
}

#[test]
fn undeclared_server_operations_and_ping_input_fail_at_the_message_guard() {
    for (mode, code) in [
        ("server_request", "mcp.unsupported_server_message"),
        ("invalid_ping", "mcp.invalid_message"),
    ] {
        let (dir, mut executor, effect) = fixture(mode, true);
        let result = executor.execute(&effect).unwrap();
        assert_eq!(
            result.execution_status,
            EffectExecutionStatus::UnknownOutcome
        );
        assert!(result.reason.unwrap().starts_with(code));
        assert!(dir.path().join("dispatch").exists());
        assert!(executor.execute(&effect).is_err());
    }
}

#[test]
fn stderr_overflow_terminates_and_reaps_an_idle_server() {
    let dir = TempDir::new().unwrap();
    let executor = McpExecutor::discover(server("stderr_idle", &dir), limits()).unwrap();
    let pid = std::fs::read_to_string(dir.path().join("dispatch-pid")).unwrap();
    std::fs::write(dir.path().join("dispatch-trigger"), "overflow").unwrap();
    let mut reaped = false;
    for _ in 0..100 {
        let alive = Command::new("kill")
            .args(["-0", &pid])
            .output()
            .unwrap()
            .status
            .success();
        if !alive {
            reaped = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        reaped,
        "idle child wasn't terminated and reaped after its diagnostics overflowed"
    );
    assert!(!dir.path().join("dispatch").exists());
    drop(executor);
}

#[test]
fn cancellation_during_the_fresh_host_probe_is_a_non_execution_result() {
    let handle = std::sync::Arc::new(std::sync::Mutex::new(None::<gaap_mcp::CancelHandle>));
    let observed_handle = handle.clone();
    let (dir, mut executor, effect) = observed_fixture(10000, move |count, effect| {
        if count == 2 {
            observed_handle.lock().unwrap().as_ref().unwrap().cancel();
        }
        Ok(same_subject(effect))
    });
    *handle.lock().unwrap() = Some(executor.cancellation_handle());
    let error = executor.execute(&effect).unwrap_err();
    assert!(error.message().starts_with("mcp.observer_failed"));
    assert!(error.message().contains("mcp.observer_cancelled"));
    assert!(!dir.path().join("dispatch").exists());
}

#[test]
fn implicit_interpreters_and_unlisted_file_arguments_fail_before_launch() {
    for mode in ["script", "unlisted", "alias"] {
        let dir = TempDir::new().unwrap();
        let mut config = server("normal", &dir);
        if mode == "script" {
            let script = dir.path().join("implicit-server");
            std::fs::write(
                &script,
                format!(
                    "#!{}\n{}",
                    config.executable.display(),
                    include_str!("fixtures/server.py")
                ),
            )
            .unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            config.arguments.remove(0);
            config.executable = script.canonicalize().unwrap();
            config.artifacts = vec![LaunchArtifact {
                path: config.executable.clone(),
                digest: hash(&std::fs::read(&config.executable).unwrap()),
            }];
        } else if mode == "unlisted" {
            config.artifacts.truncate(1);
        } else {
            #[cfg(unix)]
            {
                let alias = dir.path().join("mutable-interpreter");
                std::os::unix::fs::symlink(&config.executable, &alias).unwrap();
                config.executable = alias;
                config.artifacts[0].path = config.executable.clone();
            }
            #[cfg(not(unix))]
            {
                continue;
            }
        }
        config.artifact_digest = config.artifact_manifest_digest().unwrap();
        let error = McpExecutor::discover(config, limits()).err().expect(mode);
        assert!(
            matches!(error.code, "mcp.launch_profile" | "mcp.artifact_identity"),
            "{mode}: {error}"
        );
        assert!(!dir.path().join("dispatch").exists());
    }
}

#[test]
fn measurement_failure_retains_last_known_usage_and_stops_further_calls() {
    let (dir, mut executor, effect) = observed_fixture(10000, |count, effect| {
        if count == 3 {
            return Err(gaap_mcp::McpError {
                code: "host.usage_unknown",
                detail: "measurement unavailable".into(),
            });
        }
        let mut observation = same_subject(effect);
        observation.cost_micros = Some(123);
        observation.model_tokens = Some(7);
        Ok(observation)
    });
    let result = executor.execute(&effect).unwrap();
    assert_eq!(
        result.execution_status,
        EffectExecutionStatus::UnknownOutcome
    );
    assert_eq!(result.usage.cost_micros, 123);
    assert_eq!(result.usage.model_tokens, 7);
    let record = &executor.call_evidence()[0];
    assert!(record.host_usage.is_none());
    assert_eq!(record.last_known_usage.cost_micros, 123);
    assert!(result.evidence.iter().any(|e| e.evidence_type
        == gaap::contracts::EffectEvidenceType::Usage
        && e.digest == hash(&serde_jcs::to_vec(record).unwrap())));
    let next = executor
        .prepare_effect(
            "echo",
            json!({"message":"next"}),
            "next".into(),
            2,
            effect.subject,
            EffectClass::Observation,
        )
        .unwrap();
    assert_eq!(
        executor.execute(&next).unwrap().execution_status,
        EffectExecutionStatus::Denied
    );
    assert!(dir.path().join("dispatch").exists());
}

#[test]
fn unsafe_integer_arguments_are_rejected_before_their_approval_digest_can_alias() {
    let dir = TempDir::new().unwrap();
    let mut executor = McpExecutor::discover(server("integer_input", &dir), limits()).unwrap();
    let capability = executor.tools()[0].capability.clone();
    let (run, support) = request(capability);
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
                    effect_confirmed: false,
                    cost_micros: Some(0),
                    model_tokens: Some(0),
                })
            },
        )
        .unwrap();
    executor
        .prepare_effect(
            "echo",
            json!({"message":2}),
            "safe".into(),
            1,
            run.subject.clone(),
            EffectClass::Observation,
        )
        .unwrap();
    for integer in [18446744073709551615u64, 18446744073709551614u64] {
        let error = executor
            .prepare_effect(
                "echo",
                json!({"message":integer}),
                "unsafe".into(),
                2,
                run.subject.clone(),
                EffectClass::Observation,
            )
            .unwrap_err();
        assert_eq!(error.code, "mcp.number_range");
    }
    assert!(!dir.path().join("dispatch").exists());
}

#[test]
fn output_compaction_keeps_an_unexpected_host_subject_for_reconciliation() {
    let dir = TempDir::new().unwrap();
    let mut tight = limits();
    tight.max_retained_bytes = 16384;
    let mut executor = McpExecutor::discover(server("large_result", &dir), tight).unwrap();
    let capability = executor.tools()[0].capability.clone();
    let (run, support) = request(capability.clone());
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
                let mut observed = same_subject(effect);
                if observations == 3 {
                    observed.subject.digest = hash(b"unexpected changed subject");
                }
                Ok(observed)
            },
        )
        .unwrap();
    executor.approve_capability(&capability).unwrap();
    let effect = executor
        .prepare_effect(
            "echo",
            json!({"message":"hello"}),
            "compact".into(),
            1,
            run.subject,
            EffectClass::Observation,
        )
        .unwrap();
    let result = executor.execute(&effect).unwrap();
    assert_eq!(
        result.execution_status,
        EffectExecutionStatus::UnknownOutcome
    );
    assert!(result.observed_post_effect_subject.is_none());
    let record = &executor.call_evidence()[0];
    assert!(record.output_omitted);
    assert_eq!(
        record.observed_subject.as_ref().unwrap().digest,
        hash(b"unexpected changed subject")
    );
    assert!(dir.path().join("dispatch").exists());
}

#[test]
fn stalled_host_probes_return_within_their_bound_and_cant_replay() {
    for stalled_probe in [1, 3] {
        let (dir, mut executor, effect) = observed_fixture(10000, move |count, effect| {
            if count == stalled_probe {
                std::thread::sleep(Duration::from_secs(2));
            }
            Ok(same_subject(effect))
        });
        let start = std::time::Instant::now();
        let result = executor.execute(&effect);
        if stalled_probe == 1 {
            assert!(
                result
                    .unwrap_err()
                    .message()
                    .starts_with("mcp.observer_timeout")
            );
            assert!(!dir.path().join("dispatch").exists());
        } else {
            let result = result.unwrap();
            assert_eq!(
                result.execution_status,
                EffectExecutionStatus::UnknownOutcome
            );
            assert!(result.reason.unwrap().starts_with("mcp.observer_timeout"));
            assert!(dir.path().join("dispatch").exists());
            assert!(executor.call_evidence()[0].dispatched);
        }
        assert!(start.elapsed() < Duration::from_millis(1500));
        drop(executor);
        assert!(start.elapsed() < Duration::from_millis(1800));
    }
}

#[test]
fn working_directory_replacement_revokes_the_old_capability_identity() {
    let dir = TempDir::new().unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir(&work).unwrap();
    let mut config = server("normal", &dir);
    config.working_directory = work.canonicalize().unwrap();
    let first = McpExecutor::discover(config.clone(), limits()).unwrap();
    let old = first.tools()[0].capability.clone();
    drop(first);
    std::fs::rename(&work, dir.path().join("moved")).unwrap();
    std::fs::create_dir(&work).unwrap();
    let mut second = McpExecutor::discover(config, limits()).unwrap();
    assert_ne!(second.tools()[0].capability.digest, old.digest);
    assert_eq!(
        second.approve_capability(&old).unwrap_err().code,
        "mcp.unknown_capability"
    );
    assert!(!dir.path().join("dispatch").exists());
}

#[test]
fn cancellation_interrupts_a_stalled_host_probe_without_waiting_for_its_return() {
    let entered = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed = entered.clone();
    let (dir, mut executor, effect) = observed_fixture(10000, move |_, effect| {
        observed.store(true, std::sync::atomic::Ordering::SeqCst);
        std::thread::sleep(Duration::from_secs(2));
        Ok(same_subject(effect))
    });
    let cancellation = executor.cancellation_handle();
    let canceller = std::thread::spawn(move || {
        for _ in 0..100 {
            if entered.load(std::sync::atomic::Ordering::SeqCst) {
                cancellation.cancel();
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("host probe didn't start");
    });
    let start = std::time::Instant::now();
    assert!(
        executor
            .execute(&effect)
            .unwrap_err()
            .message()
            .starts_with("mcp.observer_cancelled")
    );
    canceller.join().unwrap();
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(!dir.path().join("dispatch").exists());
    drop(executor);
}

#[test]
fn cancellation_during_reconciliation_keeps_timely_host_evidence() {
    let entered = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed = entered.clone();
    let (dir, mut executor, effect) = observed_fixture(10000, move |count, effect| {
        if count == 3 {
            observed.store(true, std::sync::atomic::Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(80));
        }
        Ok(same_subject(effect))
    });
    let cancellation = executor.cancellation_handle();
    let canceller = std::thread::spawn(move || {
        for _ in 0..200 {
            if entered.load(std::sync::atomic::Ordering::SeqCst) {
                cancellation.cancel();
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("post-dispatch probe didn't start");
    });
    let result = executor.execute(&effect).unwrap();
    canceller.join().unwrap();
    assert_eq!(result.execution_status, EffectExecutionStatus::Interrupted);
    assert!(result.observed_post_effect_subject.is_some());
    let record = &executor.call_evidence()[0];
    assert_eq!(record.effect_confirmed, Some(true));
    assert!(record.observed_subject.is_some());
    assert!(record.host_usage.is_some());
    assert!(dir.path().join("dispatch").exists());
    assert!(executor.execute(&effect).is_err());
}

#[test]
fn changed_subject_doesnt_inherit_initial_subject_approval() {
    let dir = TempDir::new().unwrap();
    let mut executor = McpExecutor::discover(server("normal", &dir), limits()).unwrap();
    let (mut run, support) = request(executor.tools()[0].capability.clone());
    // Keep the fixture's real subject approval, then propose against a newer subject.
    run.approval_context[0].subject_digest = run.subject.digest.clone();
    let current = gaap::contracts::Subject {
        digest: hash(b"changed subject"),
        ..run.subject.clone()
    };
    let observed = current.clone();
    executor
        .bind_run(
            run,
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
                    effect_confirmed: false,
                    cost_micros: Some(0),
                    model_tokens: Some(0),
                })
            },
        )
        .unwrap();
    let effect = executor
        .prepare_effect(
            "echo",
            json!({"message":"hello"}),
            "second-subject".into(),
            1,
            current,
            EffectClass::Observation,
        )
        .unwrap();
    assert!(effect.approval_context.is_empty());
}
