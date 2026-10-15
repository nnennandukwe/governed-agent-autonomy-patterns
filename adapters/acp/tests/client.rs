use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use gaap::contracts::*;
use gaap::runtime::*;
use gaap_acp::*;
use sha2::{Digest, Sha256};

fn hash(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
fn fixed(byte: u8) -> String {
    hash(&[byte])
}
fn subject(path: &std::path::Path) -> Subject {
    Subject {
        kind: SubjectKind::Artifact,
        locator: "fixture://workspace/subject.txt".into(),
        digest: hash(&std::fs::read(path).unwrap()),
    }
}
fn identity() -> ExecutorIdentity {
    ExecutorIdentity {
        name: "host-filesystem".into(),
        version: "1".into(),
        digest: fixed(8),
    }
}
fn profile() -> SandboxProfileIdentity {
    SandboxProfileIdentity {
        name: "trusted-local-test".into(),
        version: "1".into(),
        digest: fixed(9),
    }
}
fn policy() -> PolicyIdentity {
    PolicyIdentity {
        name: "test-policy".into(),
        version: "1".into(),
        digest: fixed(3),
    }
}
fn allow() -> PermissionPolicy {
    PermissionPolicy {
        policy_decision: Some(PolicyDecision::Allow),
        risk_tags: vec![],
        wrapper_chain: vec![],
        approvals: vec![],
    }
}
fn zero() -> ResourceUsage {
    ResourceUsage {
        cost_micros: 0,
        elapsed_ms: 0,
        model_tokens: 0,
        tool_calls: 0,
    }
}

struct Host {
    path: PathBuf,
    calls: Arc<AtomicUsize>,
    confirm: bool,
    unknown_usage: bool,
    omit_read: bool,
    stall_usage: bool,
    stall_effect: bool,
    zero_tool_count: bool,
    late_meter_stall: bool,
    usage_calls: usize,
}
impl AcpHost for Host {
    fn usage(&mut self) -> Result<ResourceUsage, RuntimePortError> {
        self.usage_calls += 1;
        if self.stall_usage || (self.late_meter_stall && self.usage_calls > 1) {
            std::thread::sleep(Duration::from_secs(10));
        }
        if self.unknown_usage {
            Err(RuntimePortError::new("fixture.usage_unknown"))
        } else {
            Ok(zero())
        }
    }
    fn execute(
        &mut self,
        operation: &FileOperation,
        _request: &ProtectedEffectRequest,
    ) -> Result<HostEffect, RuntimePortError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let text = match operation {
            FileOperation::Write { path, content } => {
                assert_eq!(path, &self.path);
                std::fs::write(path, content).unwrap();
                None
            }
            FileOperation::Read { path, .. } => {
                assert_eq!(path, &self.path);
                Some(std::fs::read_to_string(path).unwrap())
            }
        };
        if self.stall_effect {
            std::thread::sleep(Duration::from_secs(10));
        }
        Ok(HostEffect {
            read_text: if self.omit_read { None } else { text },
            observation: ExecutorObservation {
                execution_status: if self.confirm {
                    EffectExecutionStatus::Executed
                } else {
                    EffectExecutionStatus::UnknownOutcome
                },
                observed_post_effect_subject: Some(subject(&self.path)),
                exit: None,
                usage: EffectUsage {
                    cost_micros: 0,
                    elapsed_ms: 0,
                    model_tokens: 0,
                    tool_calls: u64::from(!self.zero_tool_count),
                },
                executor: Some(identity()),
                sandbox_profile: Some(profile()),
                reason: if self.confirm {
                    None
                } else {
                    Some("host.unconfirmed".into())
                },
                evidence: [
                    EffectEvidenceType::Executor,
                    EffectEvidenceType::Sandbox,
                    EffectEvidenceType::Usage,
                    EffectEvidenceType::SubjectObservation,
                    EffectEvidenceType::Mutation,
                    EffectEvidenceType::Artifact,
                    EffectEvidenceType::UnknownOutcome,
                ]
                .into_iter()
                .filter(|kind| !self.confirm || *kind != EffectEvidenceType::UnknownOutcome)
                .map(|evidence_type| EffectEvidenceReference {
                    evidence_type,
                    digest: subject(&self.path).digest,
                    locator: None,
                })
                .collect(),
            },
        })
    }
}
struct Verifier {
    path: PathBuf,
    pass: bool,
    cancel: Option<CancelHandle>,
}
impl VerifierPort for Verifier {
    fn verify(
        &mut self,
        context: VerificationContext<'_>,
    ) -> Result<PortObservation<VerificationReport>, RuntimePortError> {
        if let Some(cancel) = &self.cancel {
            cancel.cancel();
        }
        let actual = subject(&self.path);
        Ok(PortObservation {
            usage: zero(),
            value: VerificationReport {
                subject_digest: actual.digest.clone(),
                verifier_id: "independent-host".into(),
                verdict: if self.pass
                    && actual == *context.current_subject
                    && std::fs::read_to_string(&self.path).unwrap() == "after\n"
                {
                    VerificationVerdict::Pass
                } else {
                    VerificationVerdict::Fail
                },
                evidence: vec![EvidenceReference {
                    evidence_type: EvidenceType::Artifact,
                    digest: actual.digest,
                    locator: None,
                }],
            },
        })
    }
}

fn config(root: &std::path::Path, mode: &str) -> AgentConfig {
    let python = std::process::Command::new("python3")
        .args(["-c", "import sys; print(sys.executable)"])
        .output()
        .unwrap();
    let executable = PathBuf::from(String::from_utf8(python.stdout).unwrap().trim())
        .canonicalize()
        .unwrap();
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/agent.py")
        .canonicalize()
        .unwrap();
    AgentConfig {
        executable: executable.clone(),
        arguments: vec![script.to_string_lossy().into_owned(), mode.into()],
        environment: BTreeMap::new(),
        working_directory: root.canonicalize().unwrap(),
        expected_name: "fixture-agent".into(),
        expected_version: "1.0".into(),
        artifacts: vec![executable, script]
            .into_iter()
            .map(|path| LaunchArtifact {
                digest: hash(&std::fs::read(&path).unwrap()),
                path,
            })
            .collect(),
    }
}
fn request(capability: CapabilityIdentity, path: &std::path::Path) -> AgentRunRequest {
    let subject = subject(path);
    AgentRunRequest {
        schema_version: AGENT_RUN_REQUEST_SCHEMA.into(),
        request_id: "fixture-run".into(),
        run_id: "fixture-run".into(),
        subject: subject.clone(),
        requested_capability: capability,
        task: TaskSpec {
            instructions: "Write after to subject.txt, then read it.".into(),
            constraints: vec!["Don't execute terminal commands.".into()],
        },
        policies: vec![policy()],
        resource_budget: ResourceBudget {
            max_cost_micros: 1000000,
            max_elapsed_ms: 60000,
            max_model_tokens: 10000,
            max_tool_calls: 8,
        },
        approval_context: vec![ApprovalReference {
            approval_id: "host-approved".into(),
            actor_id: "owner".into(),
            scope: "subject".into(),
            subject_digest: subject.digest,
            evidence: EvidenceReference {
                evidence_type: EvidenceType::Approval,
                digest: fixed(4),
                locator: None,
            },
        }],
        required_verification: VerificationRequirement {
            independence: VerificationIndependence::DifferentActor,
            evidence_types: vec![EvidenceType::Artifact],
        },
    }
}

struct Fixture {
    dir: tempfile::TempDir,
    adapter: AcpAdapter,
    executor: AcpExecutor,
    request: AgentRunRequest,
    calls: Arc<AtomicUsize>,
    evidence: EvidenceHandle,
    wait_for_fault: bool,
    cancel_before_dispatch: Option<CancelHandle>,
    cancel_after_dispatch: Option<CancelHandle>,
    cancel_on_verify: Option<CancelHandle>,
}
fn fixture(mode: &str, permission: PermissionPolicy, confirm: bool) -> Fixture {
    fixture_with_usage(mode, permission, confirm, false)
}
fn fixture_with_usage(
    mode: &str,
    permission: PermissionPolicy,
    confirm: bool,
    unknown_usage: bool,
) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().canonicalize().unwrap().join("subject.txt");
    std::fs::write(&path, "before\n").unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let (adapter, executor) = AcpAdapter::new(
        config(
            dir.path(),
            if mode == "cancel_meter" {
                "cancel"
            } else {
                mode
            },
        ),
        Limits {
            // Leave headroom for SDK schema construction on shared CI runners.
            // Timeout fixtures deliberately stall longer than this bounded wait.
            operation_timeout: Duration::from_secs(5),
            max_frame_bytes: if matches!(mode, "byte_limit" | "bounded_normal") {
                4096
            } else {
                1024 * 1024
            },
            max_total_bytes: if matches!(mode, "byte_limit" | "bounded_normal") {
                8192
            } else {
                16 * 1024 * 1024
            },
            ..Limits::default()
        },
        ContractSupport::new([policy()]),
        profile(),
        permission,
        Host {
            path: path.clone(),
            calls: calls.clone(),
            confirm,
            unknown_usage,
            omit_read: mode == "missing_read",
            stall_usage: mode == "stall_usage",
            stall_effect: mode == "host_timeout",
            zero_tool_count: mode == "zero_tool_count",
            late_meter_stall: mode == "cancel_meter",
            usage_calls: 0,
        },
    )
    .unwrap();
    let request = request(adapter.capability().clone(), &path);
    let evidence = adapter.evidence();
    let cancel_on_verify = (mode == "cancel_verify").then(|| adapter.cancellation_handle());
    let cancel_after_dispatch = (mode == "cancel_after_ask").then(|| adapter.cancellation_handle());
    let cancel_before_dispatch =
        (mode == "cancel_at_dispatch").then(|| adapter.cancellation_handle());
    Fixture {
        dir,
        adapter,
        executor,
        request,
        calls,
        evidence,
        wait_for_fault: mode == "duplicate_request",
        cancel_before_dispatch,
        cancel_after_dispatch,
        cancel_on_verify,
    }
}
struct WaitingExecutor {
    inner: AcpExecutor,
    evidence: EvidenceHandle,
    wait_for_fault: bool,
    cancel_before_dispatch: Option<CancelHandle>,
    cancel_after_dispatch: Option<CancelHandle>,
}
impl ExecutorPort for WaitingExecutor {
    fn cancellation(&self) -> Option<Cancellation> {
        self.inner.cancellation()
    }
    fn execute(
        &mut self,
        request: &ProtectedEffectRequest,
    ) -> Result<ExecutorObservation, RuntimePortError> {
        if self.wait_for_fault {
            for _ in 0..1000 {
                if self.evidence.snapshot().failure_code.is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(self.evidence.snapshot().failure_code.is_some());
        }
        if let Some(cancel) = &self.cancel_before_dispatch {
            cancel.cancel();
        }
        let result = self.inner.execute(request);
        if let Some(cancel) = &self.cancel_after_dispatch {
            cancel.cancel();
        }
        result
    }
}

fn run(
    fixture: Fixture,
    permission: PermissionPolicy,
    pass: bool,
) -> (tempfile::TempDir, AgentRunExecution, usize, SessionEvidence) {
    let support = ContractSupport::new([policy()]);
    let runtime_support = RuntimeSupport::new(
        support.clone(),
        vec![fixture.request.requested_capability.clone()],
        permission,
        EffectUsage {
            cost_micros: 0,
            elapsed_ms: 0,
            model_tokens: 0,
            tool_calls: 1,
        },
        vec![],
        vec!["independent-host".into()],
    );
    let runtime_config = RuntimeConfig {
        implementer_id: "acp-test-engine".into(),
        executor_identity: identity(),
        max_effects: 8,
        budget_thresholds: BudgetThresholds {
            warn_micros: 800000,
            approval_micros: 900000,
            hard_stop_micros: 1000000,
        },
    };
    let mut engine = AgentRunEngine::new(
        runtime_support,
        runtime_config,
        fixture.adapter,
        WaitingExecutor {
            inner: fixture.executor,
            evidence: fixture.evidence.clone(),
            wait_for_fault: fixture.wait_for_fault,
            cancel_before_dispatch: fixture.cancel_before_dispatch,
            cancel_after_dispatch: fixture.cancel_after_dispatch,
        },
        Verifier {
            path: fixture.dir.path().join("subject.txt"),
            pass,
            cancel: fixture.cancel_on_verify,
        },
    );
    let result = engine.run(fixture.request.clone()).unwrap();
    if let Ok(pid) = std::fs::read_to_string(fixture.dir.path().join("agent-pid")) {
        assert!(
            !std::process::Command::new("kill")
                .args(["-0", pid.trim()])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap()
                .success(),
            "agent survived terminal engine return"
        );
    }
    verify_terminal_receipt(&fixture.request, &support, &result.receipt).unwrap();
    let count = fixture.calls.load(Ordering::SeqCst);
    let evidence = fixture.evidence.snapshot();
    drop(engine);
    (fixture.dir, result, count, evidence)
}

#[test]
fn stable_v1_delegates_real_filesystem_effects_and_seals_verified_receipt() {
    let (dir, result, count, evidence) = run(fixture("normal", allow(), true), allow(), true);
    assert_eq!(
        result.receipt.body.terminal_status,
        AgentRunStatus::Completed,
        "{:?}",
        result.receipt.body
    );
    assert_eq!(count, 2);
    assert_eq!(result.protected_effect_results.len(), 2);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("subject.txt")).unwrap(),
        "after\n"
    );
    assert_eq!(evidence.protocol_version, 1);
    assert!(evidence.session_digest.is_some());
    let trace = serde_json::to_string(&evidence).unwrap();
    assert!(!trace.contains("SECRET_AGENT_TEXT"));
    assert!(!trace.contains(&dir.path().to_string_lossy().to_string()));
}

#[test]
fn malformed_foreign_duplicate_and_unsupported_messages_cant_complete() {
    for (mode, expected) in [
        ("wrong_version", "acp.version_mismatch"),
        ("malformed_capabilities", "acp.malformed_result"),
        ("duplicate_key", "acp.invalid_json"),
        ("early_update", "acp.session_order"),
        ("partial", "acp.partial_frame"),
        ("partial_output", "acp.partial_output"),
        ("exit", "acp.agent_exit"),
        ("oversized", "acp.frame_limit"),
        ("stderr", "acp.stderr_limit"),
        ("duplicate_response", "acp.unexpected_response"),
        ("wrong_session", "acp.session_order"),
        ("tool_order", "acp.tool_order"),
        ("large_tool_id", "acp.invalid_tool_id"),
        ("byte_limit", "acp.total_bytes_limit"),
        ("terminal", "acp.unsupported_effect"),
        ("escape", "acp.path_outside_workspace"),
        ("report_only", "acp.no_host_effect"),
    ] {
        let (dir, result, count, evidence) = run(fixture(mode, allow(), true), allow(), true);
        assert_ne!(
            result.receipt.body.terminal_status,
            AgentRunStatus::Completed,
            "{mode}"
        );
        assert_eq!(count, 0, "{mode}: {evidence:?}");
        assert!(
            result.receipt.body.terminal_reason.contains(expected),
            "{mode}: expected {expected}, got {}",
            result.receipt.body.terminal_reason
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("subject.txt")).unwrap(),
            "before\n",
            "{mode}"
        );
        assert!(!dir.path().join("forbidden").exists(), "{mode}");
    }
}

#[test]
fn engine_permission_denial_prevents_host_dispatch() {
    let deny = PermissionPolicy {
        policy_decision: Some(PolicyDecision::Deny),
        ..allow()
    };
    let (dir, result, count, _) = run(fixture("normal", deny.clone(), true), deny, true);
    assert_eq!(result.receipt.body.terminal_status, AgentRunStatus::Blocked);
    assert_eq!(count, 0);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("subject.txt")).unwrap(),
        "before\n"
    );
}

#[test]
fn permission_ui_with_incomplete_input_never_approves_agent_execution() {
    let (dir, result, count, evidence) = run(fixture("permission", allow(), true), allow(), true);
    assert_eq!(
        evidence.failure_code.as_deref(),
        Some("acp.authority_required")
    );
    assert_ne!(
        result.receipt.body.terminal_status,
        AgentRunStatus::Completed
    );
    assert_eq!(count, 0);
    assert!(!dir.path().join("forbidden").exists());
}

#[test]
fn unconfirmed_host_effect_and_failed_independent_verification_cant_complete() {
    for (confirm, pass) in [(false, true), (true, false)] {
        let (dir, result, count, _) = run(fixture("normal", allow(), confirm), allow(), pass);
        assert_ne!(
            result.receipt.body.terminal_status,
            AgentRunStatus::Completed
        );
        assert!(count >= 1);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("subject.txt")).unwrap(),
            "after\n"
        );
        assert_eq!(
            result.protected_effect_results.len(),
            if confirm { 2 } else { 1 }
        );
    }
}

#[test]
fn transport_loss_after_mutation_preserves_effect_evidence_without_replay() {
    for mode in ["post_effect_exit", "post_effect_timeout"] {
        let (dir, result, count, _) = run(fixture(mode, allow(), true), allow(), true);
        assert_ne!(
            result.receipt.body.terminal_status,
            AgentRunStatus::Completed
        );
        assert_eq!(count, 1);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("subject.txt")).unwrap(),
            "after\n"
        );
        assert_eq!(result.protected_effect_results.len(), 1);
        assert_eq!(
            result.protected_effect_results[0].body.execution_status,
            EffectExecutionStatus::Executed
        );
    }
}

#[test]
fn timeout_and_exit_have_separate_diagnostics() {
    let (_, timeout, _, _) = run(fixture("missing_response", allow(), true), allow(), true);
    let (_, exit, _, _) = run(fixture("exit", allow(), true), allow(), true);
    let timeout = serde_json::to_string(&timeout.receipt).unwrap();
    let exit = serde_json::to_string(&exit.receipt).unwrap();
    assert!(timeout.contains("acp.timeout"), "{timeout}");
    assert!(exit.contains("acp.agent_exit"), "{exit}");
}

#[test]
fn cancellation_sends_protocol_cancel_and_interrupts_without_dispatch() {
    let f = fixture("cancel", allow(), true);
    let cancel = f.adapter.cancellation_handle();
    let marker = f.dir.path().join("prompt-started");
    let canceller = std::thread::spawn(move || {
        for _ in 0..1000 {
            if marker.exists() {
                cancel.cancel();
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("prompt didn't start");
    });
    let (_, result, count, _) = run(f, allow(), true);
    canceller.join().unwrap();
    assert_eq!(
        result.receipt.body.terminal_status,
        AgentRunStatus::Interrupted,
        "{:?}",
        result.receipt.body
    );
    assert_eq!(count, 0);
}

#[test]
fn unknown_model_usage_blocks_before_prompt_and_closes_child() {
    let (dir, result, count, _) = run(
        fixture_with_usage("normal", allow(), true, true),
        allow(),
        true,
    );
    assert_eq!(result.receipt.body.terminal_status, AgentRunStatus::Failed);
    assert!(
        result
            .receipt
            .body
            .terminal_reason
            .contains("acp.usage_unknown")
    );
    assert!(!dir.path().join("prompt-started").exists());
    assert_eq!(count, 0);
}

#[test]
fn blocked_plan_closes_agent_before_engine_returns() {
    let mut f = fixture("normal", allow(), true);
    f.request.approval_context.clear();
    let (dir, result, count, _) = run(f, allow(), true);
    assert_eq!(result.receipt.body.terminal_status, AgentRunStatus::Blocked);
    assert!(!dir.path().join("prompt-started").exists());
    assert_eq!(count, 0);
}

#[test]
fn paired_executor_refuses_changed_inputs_and_replay() {
    for tamper in [false, true] {
        let mut f = fixture("normal", allow(), true);
        f.adapter.plan(&f.request).unwrap();
        let step = f
            .adapter
            .next_effect(AgentRunContext {
                current_subject: &f.request.subject,
                usage: &zero(),
                effects_seen: 0,
            })
            .unwrap();
        let AgentStep::ProtectedEffect(proposal) = step.value else {
            panic!("expected effect");
        };
        let original = proposal.request;
        let mut changed = original.clone();
        if tamper {
            changed.input_digest = fixed(99);
            assert_eq!(
                f.executor.execute(&changed).unwrap().execution_status,
                EffectExecutionStatus::Denied
            );
        } else {
            assert_eq!(
                f.executor.execute(&original).unwrap().execution_status,
                EffectExecutionStatus::Executed
            );
        }
        assert_eq!(
            f.executor.execute(&original).unwrap().execution_status,
            EffectExecutionStatus::Denied
        );
        assert_eq!(f.calls.load(Ordering::SeqCst), usize::from(!tamper));
        f.adapter.close();
    }
}

#[cfg(unix)]
#[test]
fn symlink_escape_is_rejected_before_host_execution() {
    let f = fixture("symlink", allow(), true);
    let outside = tempfile::NamedTempFile::new().unwrap();
    std::os::unix::fs::symlink(outside.path(), f.dir.path().join("linked.txt")).unwrap();
    let (_, result, count, _) = run(f, allow(), true);
    assert_eq!(count, 0);
    assert_ne!(
        result.receipt.body.terminal_status,
        AgentRunStatus::Completed
    );
    assert!(std::fs::read(outside.path()).unwrap().is_empty());
}

#[test]
fn missing_read_payload_preserves_the_known_host_observation() {
    let (_, result, count, _) = run(fixture("missing_read", allow(), true), allow(), true);
    assert_eq!(result.receipt.body.terminal_status, AgentRunStatus::Failed);
    assert_eq!(count, 2);
    assert_eq!(result.protected_effect_results.len(), 2);
    let read = &result.protected_effect_results[1].body;
    assert_eq!(read.execution_status, EffectExecutionStatus::Executed);
    assert!(read.observed_post_effect_subject.is_some());
    assert_eq!(read.usage.tool_calls, 1);
    assert!(
        result
            .receipt
            .body
            .terminal_reason
            .contains("acp.read_evidence_missing")
    );
}

#[test]
fn duplicate_protocol_fault_revokes_a_previously_prepared_effect() {
    let mut f = fixture("duplicate_request", allow(), true);
    f.adapter.plan(&f.request).unwrap();
    let step = f
        .adapter
        .next_effect(AgentRunContext {
            current_subject: &f.request.subject,
            usage: &zero(),
            effects_seen: 0,
        })
        .unwrap();
    let AgentStep::ProtectedEffect(proposal) = step.value else {
        panic!("expected effect");
    };
    for _ in 0..1000 {
        if f.evidence.snapshot().failure_code.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        f.evidence.snapshot().failure_code.as_deref(),
        Some("acp.duplicate_request")
    );
    let observation = f.executor.execute(&proposal.request).unwrap();
    assert_eq!(observation.execution_status, EffectExecutionStatus::Denied);
    assert!(observation.executor.is_none());
    assert_eq!(observation.usage.tool_calls, 0);
    assert_eq!(f.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        std::fs::read_to_string(f.dir.path().join("subject.txt")).unwrap(),
        "before\n"
    );
    f.adapter.close();
}

#[test]
fn stricter_executor_policy_records_non_dispatch_instead_of_an_unknown_attempt() {
    for decision in [PolicyDecision::Deny, PolicyDecision::Ask] {
        let strict = PermissionPolicy {
            policy_decision: Some(decision),
            ..allow()
        };
        let (_, result, count, _) = run(fixture("normal", strict, true), allow(), true);
        assert_eq!(count, 0);
        assert_eq!(result.receipt.body.terminal_status, AgentRunStatus::Blocked);
        let effect = &result.protected_effect_results[0].body;
        assert_eq!(
            effect.execution_status,
            if decision == PolicyDecision::Ask {
                EffectExecutionStatus::AwaitingAuthority
            } else {
                EffectExecutionStatus::Denied
            }
        );
        assert_eq!(effect.usage.tool_calls, 0);
        assert!(effect.executor.is_none());
    }
}

#[test]
fn bounded_total_transport_has_a_successful_control() {
    let (_, result, count, _) = run(fixture("bounded_normal", allow(), true), allow(), true);
    assert_eq!(
        result.receipt.body.terminal_status,
        AgentRunStatus::Completed
    );
    assert_eq!(count, 2);
}

#[test]
fn failed_plan_cant_start_a_prompt_through_the_adapter_port() {
    let mut f = fixture_with_usage("normal", allow(), true, true);
    assert!(f.adapter.plan(&f.request).is_err());
    assert!(
        f.adapter
            .next_effect(AgentRunContext {
                current_subject: &f.request.subject,
                usage: &zero(),
                effects_seen: 0
            })
            .is_err()
    );
    assert!(!f.dir.path().join("prompt-started").exists());
    f.adapter.close();
}

#[test]
fn a_stalled_usage_callback_expires_without_starting_the_agent_prompt() {
    let (dir, result, count, _) = run(fixture("stall_usage", allow(), true), allow(), true);
    assert_eq!(result.receipt.body.terminal_status, AgentRunStatus::Failed);
    assert!(
        result
            .receipt
            .body
            .terminal_reason
            .contains("acp.usage_unknown")
    );
    assert!(!dir.path().join("prompt-started").exists());
    assert_eq!(count, 0);
}

#[test]
fn valid_tool_progress_preserves_the_governed_effect_path() {
    let (_, result, count, evidence) = run(fixture("tool_progress", allow(), true), allow(), true);
    assert_eq!(evidence.tool_progress.len(), 3);
    assert_eq!(
        evidence.tool_progress.last().unwrap().status.as_deref(),
        Some("completed")
    );
    assert_eq!(
        result.receipt.body.terminal_status,
        AgentRunStatus::Completed
    );
    assert_eq!(count, 2);
}

#[test]
fn protocol_refusal_receipt_records_no_dispatch_or_attempt_usage() {
    let (_, result, count, _) = run(fixture("duplicate_request", allow(), true), allow(), true);
    assert_eq!(count, 0);
    assert_eq!(result.receipt.body.terminal_status, AgentRunStatus::Blocked);
    let effect = &result.protected_effect_results[0].body;
    assert_eq!(effect.execution_status, EffectExecutionStatus::Denied);
    assert!(effect.executor.is_none());
    assert_eq!(effect.usage.tool_calls, 0);
    assert!(
        !result
            .receipt
            .body
            .events
            .iter()
            .any(|event| matches!(event, RunEvent::ToolExecution { .. }))
    );
}

#[test]
fn advertised_authentication_methods_dont_mean_authentication_is_required() {
    let (_, result, count, _) = run(fixture("advertised_auth", allow(), true), allow(), true);
    assert_eq!(
        result.receipt.body.terminal_status,
        AgentRunStatus::Completed
    );
    assert_eq!(count, 2);
}

#[test]
fn actual_authentication_requirement_stops_before_prompt_and_dispatch() {
    let (dir, result, count, _) = run(fixture("requires_auth", allow(), true), allow(), true);
    assert_eq!(result.receipt.body.terminal_status, AgentRunStatus::Failed);
    assert!(
        result
            .receipt
            .body
            .terminal_reason
            .contains("acp.authentication_required")
    );
    assert_eq!(count, 0);
    assert!(!dir.path().join("prompt-started").exists());
}

#[test]
fn cancellation_after_proposal_is_interrupted_without_host_dispatch() {
    let (_, result, count, _) = run(fixture("cancel_at_dispatch", allow(), true), allow(), true);
    assert_eq!(
        result.receipt.body.terminal_status,
        AgentRunStatus::Interrupted
    );
    assert_eq!(count, 0);
    let effect = &result.protected_effect_results[0].body;
    assert_eq!(effect.execution_status, EffectExecutionStatus::Denied);
    assert!(effect.executor.is_none());
    assert_eq!(effect.usage.tool_calls, 0);
    assert_eq!(result.receipt.body.usage.tool_calls, 0);
}

#[test]
fn timed_out_host_dispatch_preserves_unknown_outcome_and_counts_attempt() {
    let (dir, result, count, _) = run(fixture("host_timeout", allow(), true), allow(), true);
    assert_eq!(result.receipt.body.terminal_status, AgentRunStatus::Failed);
    assert_eq!(count, 1);
    let effect = &result.protected_effect_results[0].body;
    assert_eq!(
        effect.execution_status,
        EffectExecutionStatus::UnknownOutcome
    );
    assert_eq!(effect.usage.tool_calls, 1);
    assert_eq!(result.receipt.body.usage.tool_calls, 1);
    assert!(effect.observed_post_effect_subject.is_none());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("subject.txt")).unwrap(),
        "after\n"
    );
}

#[test]
fn explicit_cancellation_during_uncertain_dispatch_preserves_both_outcomes() {
    let f = fixture("host_timeout", allow(), true);
    let calls = f.calls.clone();
    let cancel = f.adapter.cancellation_handle();
    let canceller = std::thread::spawn(move || {
        for _ in 0..1000 {
            if calls.load(Ordering::SeqCst) > 0 {
                cancel.cancel();
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("host dispatch didn't start");
    });
    let (_, result, count, _) = run(f, allow(), true);
    canceller.join().unwrap();
    assert_eq!(
        result.receipt.body.terminal_status,
        AgentRunStatus::Interrupted
    );
    assert_eq!(count, 1);
    assert_eq!(
        result.protected_effect_results[0].body.execution_status,
        EffectExecutionStatus::UnknownOutcome
    );
    assert_eq!(result.receipt.body.usage.tool_calls, 1);
    assert!(
        result
            .receipt
            .body
            .events
            .iter()
            .any(|event| matches!(event, RunEvent::Interruption { .. }))
    );
}

#[test]
fn signed_json_rpc_ids_preserve_the_governed_effect_path() {
    let (_, result, count, _) = run(fixture("signed_ids", allow(), true), allow(), true);
    assert_eq!(
        result.receipt.body.terminal_status,
        AgentRunStatus::Completed
    );
    assert_eq!(count, 2);
}

#[test]
fn setup_cancellation_is_interrupted_without_prompt_or_dispatch() {
    let f = fixture("missing_response", allow(), true);
    let cancel = f.adapter.cancellation_handle();
    let marker = f.dir.path().join("agent-pid");
    let canceller = std::thread::spawn(move || {
        for _ in 0..1000 {
            if marker.exists() {
                cancel.cancel();
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("agent didn't start");
    });
    let (dir, result, count, _) = run(f, allow(), true);
    canceller.join().unwrap();
    assert_eq!(
        result.receipt.body.terminal_status,
        AgentRunStatus::Interrupted
    );
    assert_eq!(count, 0);
    assert!(!dir.path().join("prompt-started").exists());
}

#[test]
fn a_zero_host_counter_cant_erase_the_known_dispatch() {
    let (_, result, count, _) = run(fixture("zero_tool_count", allow(), true), allow(), true);
    assert_eq!(
        result.receipt.body.terminal_status,
        AgentRunStatus::Completed
    );
    assert_eq!(count, 2);
    assert_eq!(result.receipt.body.usage.tool_calls, 2);
    for effect in result.protected_effect_results {
        assert_eq!(effect.body.usage.tool_calls, 1);
    }
}

#[test]
fn authority_required_stays_blocked_when_cancellation_arrives_after_ask() {
    let ask = PermissionPolicy {
        policy_decision: Some(PolicyDecision::Ask),
        ..allow()
    };
    let (_, result, count, _) = run(fixture("cancel_after_ask", ask, true), allow(), true);
    assert_eq!(result.receipt.body.terminal_status, AgentRunStatus::Blocked);
    assert_eq!(count, 0);
    assert_eq!(
        result.protected_effect_results[0].body.execution_status,
        EffectExecutionStatus::AwaitingAuthority
    );
    assert!(
        !result
            .receipt
            .body
            .events
            .iter()
            .any(|event| matches!(event, RunEvent::Interruption { .. }))
    );
}

#[test]
fn cancellation_during_verification_cant_complete_the_run() {
    let (_, result, count, _) = run(fixture("cancel_verify", allow(), true), allow(), true);
    assert_eq!(count, 2);
    assert_eq!(
        result.receipt.body.terminal_status,
        AgentRunStatus::Interrupted
    );
    assert!(
        result
            .receipt
            .body
            .events
            .iter()
            .any(|event| matches!(event, RunEvent::Interruption { .. }))
    );
}

#[test]
fn a_stalled_meter_cant_hide_explicit_prompt_cancellation() {
    let f = fixture("cancel_meter", allow(), true);
    let marker = f.dir.path().join("prompt-started");
    let cancel = f.adapter.cancellation_handle();
    let canceller = std::thread::spawn(move || {
        for _ in 0..1000 {
            if marker.exists() {
                cancel.cancel();
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("prompt didn't start");
    });
    let (_, result, count, evidence) = run(f, allow(), true);
    canceller.join().unwrap();
    assert_eq!(count, 0);
    assert_eq!(
        result.receipt.body.terminal_status,
        AgentRunStatus::Interrupted
    );
    assert_eq!(evidence.usage_failure.as_deref(), Some("acp.usage_unknown"));
}

#[test]
fn relative_launch_files_need_approved_bytes_in_the_child_directory() {
    struct NeverHost;
    impl AcpHost for NeverHost {
        fn usage(&mut self) -> Result<ResourceUsage, RuntimePortError> {
            panic!("mustn't start")
        }
        fn execute(
            &mut self,
            _: &FileOperation,
            _: &ProtectedEffectRequest,
        ) -> Result<HostEffect, RuntimePortError> {
            panic!("mustn't dispatch")
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let relative = format!("{}.json", dir.path().file_name().unwrap().to_string_lossy());
    assert!(!std::path::Path::new(&relative).exists());
    let path = dir.path().join(&relative);
    std::fs::write(&path, "host configuration").unwrap();
    for approved in [false, true] {
        let mut launch = config(dir.path(), "normal");
        launch.arguments.push(relative.clone());
        if approved {
            launch.artifacts.push(LaunchArtifact {
                path: path.canonicalize().unwrap(),
                digest: hash(b"host configuration"),
            });
        }
        let result = AcpAdapter::new(
            launch,
            Limits::default(),
            ContractSupport::new([policy()]),
            profile(),
            allow(),
            NeverHost,
        );
        assert_eq!(result.is_ok(), approved);
    }
}

#[test]
fn approved_relative_python_script_resolves_in_the_child_directory() {
    struct NeverHost;
    impl AcpHost for NeverHost {
        fn usage(&mut self) -> Result<ResourceUsage, RuntimePortError> {
            Ok(zero())
        }
        fn execute(
            &mut self,
            _: &FileOperation,
            _: &ProtectedEffectRequest,
        ) -> Result<HostEffect, RuntimePortError> {
            panic!("mustn't dispatch")
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let mut launch = config(dir.path(), "normal");
    let copied = dir.path().join("agent.py");
    std::fs::copy(&launch.artifacts[1].path, &copied).unwrap();
    launch.artifacts[1].path = copied.canonicalize().unwrap();
    launch.arguments[0] = "agent.py".into();
    let capability = launch.capability().unwrap();
    let path = dir.path().join("subject.txt");
    std::fs::write(&path, "before\n").unwrap();
    let run_request = request(capability.clone(), &path);
    let (mut adapter, _) = AcpAdapter::new(
        launch,
        Limits::default(),
        ContractSupport::new([policy()]),
        profile(),
        allow(),
        NeverHost,
    )
    .unwrap();
    assert_eq!(adapter.capability(), &capability);
    adapter.plan(&run_request).unwrap();
    assert!(dir.path().join("agent-pid").exists());
    assert!(!dir.path().join("prompt-started").exists());
    adapter.close();
}
