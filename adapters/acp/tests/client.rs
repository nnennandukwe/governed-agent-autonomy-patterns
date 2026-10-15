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
        kind: SubjectKind::Repository,
        locator: "fixture://workspace".into(),
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
}
impl AcpHost for Host {
    fn usage(&mut self) -> Result<ResourceUsage, RuntimePortError> {
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
        Ok(HostEffect {
            read_text: text,
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
                    tool_calls: 1,
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
}
impl VerifierPort for Verifier {
    fn verify(
        &mut self,
        context: VerificationContext<'_>,
    ) -> Result<PortObservation<VerificationReport>, RuntimePortError> {
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
            max_elapsed_ms: 4000,
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
        config(dir.path(), mode),
        Limits {
            operation_timeout: Duration::from_millis(500),
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
        },
    )
    .unwrap();
    let request = request(adapter.capability().clone(), &path);
    let evidence = adapter.evidence();
    Fixture {
        dir,
        adapter,
        executor,
        request,
        calls,
        evidence,
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
        fixture.executor,
        Verifier {
            path: fixture.dir.path().join("subject.txt"),
            pass,
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
        ("exit", "acp.agent_exit"),
        ("oversized", "acp.frame_limit"),
        ("stderr", "acp.stderr_limit"),
        ("duplicate_response", "acp.unexpected_response"),
        ("wrong_session", "acp.session_order"),
        ("tool_order", "acp.tool_order"),
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
    let (dir, result, count, _) = run(fixture("permission", allow(), true), allow(), true);
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
        for _ in 0..200 {
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
            assert!(f.executor.execute(&changed).is_err());
        } else {
            assert_eq!(
                f.executor.execute(&original).unwrap().execution_status,
                EffectExecutionStatus::Executed
            );
        }
        assert!(f.executor.execute(&original).is_err());
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
