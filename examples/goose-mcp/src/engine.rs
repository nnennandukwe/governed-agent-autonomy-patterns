//! Deterministic engine view, using an in-memory artifact and fixture evidence.
//! The goose MCP view has its own decision log and does not use these receipts.
use gaap::contracts::*;
use gaap::runtime::*;
use serde_json::{Value, json};
use std::{cell::RefCell, rc::Rc};
fn digest(byte: u8) -> String {
    format!("sha256:{}", char::from(byte).to_string().repeat(64))
}

fn subject(digest: String) -> Subject {
    Subject {
        kind: SubjectKind::Repository,
        locator: "https://example.invalid/repository".to_string(),
        digest,
    }
}

fn capability() -> CapabilityIdentity {
    CapabilityIdentity {
        name: "filesystem.write".to_string(),
        version: "1.0.0".to_string(),
        digest: digest(b'2'),
    }
}

fn policy() -> PolicyIdentity {
    PolicyIdentity {
        name: "fixture-policy".to_string(),
        version: "2026-09-03".to_string(),
        digest: digest(b'3'),
    }
}

fn approval_for(subject_digest: &str) -> ApprovalReference {
    ApprovalReference {
        approval_id: format!("approval-{subject_digest}"),
        actor_id: "owner@example.com".to_string(),
        scope: "subject".to_string(),
        subject_digest: subject_digest.to_string(),
        evidence: EvidenceReference {
            evidence_type: EvidenceType::Approval,
            digest: digest(b'4'),
            locator: Some("approval://fixture".to_string()),
        },
    }
}

fn evidence(evidence_type: EvidenceType, digest_byte: u8) -> EvidenceReference {
    EvidenceReference {
        evidence_type,
        digest: digest(digest_byte),
        locator: Some(format!("fixture://{}", char::from(digest_byte))),
    }
}

fn effect_evidence(evidence_type: EffectEvidenceType, digest_byte: u8) -> EffectEvidenceReference {
    EffectEvidenceReference {
        evidence_type,
        digest: digest(digest_byte),
        locator: Some(format!("fixture://effect/{}", char::from(digest_byte))),
    }
}

fn request() -> AgentRunRequest {
    AgentRunRequest {
        schema_version: gaap::contracts::AGENT_RUN_REQUEST_SCHEMA.to_owned(),
        request_id: "run-fixture".to_string(),
        run_id: "run-fixture".to_string(),
        subject: subject(digest(b'1')),
        requested_capability: capability(),
        task: TaskSpec {
            instructions: "Implement the approved change.".to_string(),
            constraints: vec!["Do not publish a release.".to_string()],
        },
        policies: vec![policy()],
        resource_budget: ResourceBudget {
            max_cost_micros: 1_000_000,
            max_elapsed_ms: 60_000,
            max_model_tokens: 100_000,
            max_tool_calls: 100,
        },
        approval_context: vec![approval_for(&digest(b'1'))],
        required_verification: VerificationRequirement {
            independence: VerificationIndependence::DifferentActor,
            evidence_types: vec![EvidenceType::CommandOutput, EvidenceType::Artifact],
        },
    }
}

fn support() -> ContractSupport {
    ContractSupport::new([policy()])
}

fn config() -> RuntimeConfig {
    RuntimeConfig {
        implementer_id: "runtime-test-engine".to_string(),
        executor_identity: executor_identity(),
        max_effects: 4,
        budget_thresholds: BudgetThresholds {
            warn_micros: 800_000,
            approval_micros: 900_000,
            hard_stop_micros: 1_000_000,
        },
    }
}

fn effect_request(
    request: &AgentRunRequest,
    support: &ContractSupport,
    effect_sequence: u64,
) -> ProtectedEffectRequest {
    let request_digest = validate_request(request, support)
        .expect("fixture request should validate")
        .to_owned();
    let budget_digest = canonical_resource_budget_digest(&request.resource_budget)
        .expect("fixture budget should canonicalize");

    ProtectedEffectRequest {
        schema_version: PROTECTED_EFFECT_REQUEST_SCHEMA.to_owned(),
        effect_id: format!("effect-{effect_sequence}"),
        effect_sequence,
        run_id: request.run_id.clone(),
        agent_run_request_digest: request_digest,
        subject: request.subject.clone(),
        operation_family: OperationFamily::Filesystem,
        normalized_operation: "filesystem.write".to_string(),
        capability: request.requested_capability.clone(),
        tool_schema_digest: Some(digest(b'6')),
        input_digest: digest(b'7'),
        input_metadata: vec![InputMetadataEntry {
            name: "path".to_string(),
            value: "artifact.txt".to_string(),
        }],
        requested_scopes: vec![RequestedScope::Filesystem {
            root: "/workspace".to_string(),
            access: vec![FilesystemAccess::Read, FilesystemAccess::Modify],
            recursive: true,
        }],
        policies: request.policies.clone(),
        approval_context: vec![approval_for(&request.subject.digest)],
        resource_budget_digest: budget_digest,
        sandbox_profile: SandboxProfileIdentity {
            name: "sandbox-fixture".to_string(),
            version: "0.1.0".to_string(),
            digest: digest(b'8'),
        },
        idempotency_key: format!("{}/effect-{effect_sequence}", request.run_id),
        repeatability: Repeatability::Idempotent,
        expected_effect_class: EffectClass::Mutation,
    }
}

fn executor_identity() -> ExecutorIdentity {
    ExecutorIdentity {
        name: "test-executor".to_string(),
        version: "0.1.0".to_string(),
        digest: digest(b'9'),
    }
}

fn usage(cost: u64) -> EffectUsage {
    EffectUsage {
        cost_micros: cost,
        elapsed_ms: 10,
        model_tokens: 0,
        tool_calls: 1,
    }
}

fn zero_resource_usage() -> ResourceUsage {
    ResourceUsage {
        cost_micros: 0,
        elapsed_ms: 0,
        model_tokens: 0,
        tool_calls: 0,
    }
}

fn executed_observation(next_subject: Subject) -> ExecutorObservation {
    ExecutorObservation {
        execution_status: EffectExecutionStatus::Executed,
        observed_post_effect_subject: Some(next_subject),
        exit: None,
        usage: usage(10),
        executor: Some(executor_identity()),
        sandbox_profile: Some(SandboxProfileIdentity {
            name: "sandbox-fixture".to_string(),
            version: "0.1.0".to_string(),
            digest: digest(b'8'),
        }),
        reason: None,
        evidence: vec![
            effect_evidence(EffectEvidenceType::Executor, b'a'),
            effect_evidence(EffectEvidenceType::Artifact, b'b'),
            effect_evidence(EffectEvidenceType::Sandbox, b'0'),
            effect_evidence(EffectEvidenceType::SubjectObservation, b'1'),
            effect_evidence(EffectEvidenceType::Mutation, b'2'),
            effect_evidence(EffectEvidenceType::Usage, b'3'),
            effect_evidence(EffectEvidenceType::Output, b'4'),
        ],
    }
}

fn passing_verification(subject_digest: &str) -> VerificationReport {
    VerificationReport {
        subject_digest: subject_digest.to_string(),
        verifier_id: "fixture-verifier".to_string(),
        verdict: VerificationVerdict::Pass,
        evidence: vec![
            evidence(EvidenceType::CommandOutput, b'c'),
            evidence(EvidenceType::Artifact, b'd'),
        ],
    }
}

struct PlannedAgent {
    effect: Option<ProtectedEffectRequest>,
}
fn observed<T>(value: T) -> PortObservation<T> {
    PortObservation {
        value,
        usage: zero_resource_usage(),
    }
}
impl AgentAdapter for PlannedAgent {
    fn plan(
        &mut self,
        _: &AgentRunRequest,
    ) -> Result<PortObservation<PlanProposal>, RuntimePortError> {
        Ok(observed(PlanProposal {
            plan_digest: crate::store::digest(b"update the in-memory artifact to version 1.1.0"),
        }))
    }
    fn next_effect(
        &mut self,
        _: AgentRunContext<'_>,
    ) -> Result<PortObservation<AgentStep>, RuntimePortError> {
        Ok(observed(
            self.effect.take().map_or(AgentStep::Finish, |request| {
                AgentStep::ProtectedEffect(Box::new(ProtectedEffectProposal { request }))
            }),
        ))
    }
}
struct MemoryExecutor(Rc<RefCell<String>>);
impl ExecutorPort for MemoryExecutor {
    fn execute(
        &mut self,
        _: &ProtectedEffectRequest,
    ) -> Result<ExecutorObservation, RuntimePortError> {
        *self.0.borrow_mut() = "1.1.0".into();
        Ok(executed_observation(subject(crate::store::digest(
            self.0.borrow().as_bytes(),
        ))))
    }
}
struct ArtifactVerifier {
    artifact: Rc<RefCell<String>>,
    stale: bool,
}
impl VerifierPort for ArtifactVerifier {
    fn verify(
        &mut self,
        _: VerificationContext<'_>,
    ) -> Result<PortObservation<VerificationReport>, RuntimePortError> {
        let content = self.artifact.borrow();
        if content.as_str() != "1.1.0" {
            return Err(RuntimePortError::new("artifact assertion failed"));
        }
        let digest = crate::store::digest(if self.stale {
            b"1.0.0"
        } else {
            content.as_bytes()
        });
        Ok(observed(passing_verification(&digest)))
    }
}

pub fn run(scenario: &str) -> Result<Value, String> {
    if !["completed", "approval-required", "stale-verification"].contains(&scenario) {
        return Err("engine scenario: completed | approval-required | stale-verification".into());
    }
    let artifact = Rc::new(RefCell::new("1.0.0".to_string()));
    let mut request = request();
    request.subject = subject(crate::store::digest(artifact.borrow().as_bytes()));
    request.approval_context = vec![approval_for(&request.subject.digest)];
    let support = support();
    let effect = effect_request(&request, &support, 1);
    let runtime_support = RuntimeSupport::new(
        support.clone(),
        vec![capability()],
        PermissionPolicy {
            policy_decision: Some(if scenario == "approval-required" {
                PolicyDecision::Ask
            } else {
                PolicyDecision::Allow
            }),
            risk_tags: vec![],
            wrapper_chain: vec![],
            approvals: vec![],
        },
        usage(10),
        vec![],
        vec!["fixture-verifier".into()],
    );
    let mut engine = AgentRunEngine::new(
        runtime_support,
        config(),
        PlannedAgent {
            effect: Some(effect.clone()),
        },
        MemoryExecutor(artifact.clone()),
        ArtifactVerifier {
            artifact: artifact.clone(),
            stale: scenario == "stale-verification",
        },
    );
    let execution = engine.run(request.clone()).map_err(|e| e.to_string())?;
    verify_terminal_receipt(&request, &support, &execution.receipt).map_err(|e| e.to_string())?;
    for result in &execution.protected_effect_results {
        verify_protected_effect_result(&request, &support, &effect, result)
            .map_err(|e| e.to_string())?;
    }
    Ok(json!({
        "view":"Engine view: deterministic AgentRunEngine with in-memory ports",
        "scenario":scenario,
        "usage_kind":"fixture values, not measured model usage",
        "evidence_kind":"deterministic fixture references; no external process or sandbox attestation",
        "artifact_version":artifact.borrow().as_str(),
        "terminal_receipt_verified":true,
        "receipt":execution.receipt,
        "protected_effect_results":execution.protected_effect_results,
    }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn engine_views_preserve_artifact_and_receipt_semantics() {
        let completed = super::run("completed").unwrap();
        assert_eq!(completed["artifact_version"], "1.1.0");
        assert_eq!(completed["receipt"]["body"]["terminal_status"], "completed");
        let denied = super::run("approval-required").unwrap();
        assert_eq!(denied["artifact_version"], "1.0.0");
        assert_eq!(denied["receipt"]["body"]["terminal_status"], "blocked");
        let stale = super::run("stale-verification").unwrap();
        assert_eq!(stale["artifact_version"], "1.1.0");
        assert_eq!(stale["receipt"]["body"]["terminal_status"], "blocked");
    }
}
