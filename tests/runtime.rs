use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use gaap::contracts::{
    AgentRunRequest, AgentRunStatus, ApprovalReference, CapabilityIdentity, ContractSupport,
    EffectClass, EffectEvidenceReference, EffectEvidenceType, EffectExecutionStatus, EffectUsage,
    EvidenceReference, EvidenceType, ExecutorIdentity, FilesystemAccess, InputMetadataEntry,
    OperationFamily, PROTECTED_EFFECT_REQUEST_SCHEMA, PolicyIdentity, ProtectedEffectRequest,
    Repeatability, RequestedScope, ResourceBudget, RunEvent, SandboxProfileIdentity, Subject,
    SubjectKind, TaskSpec, VerificationIndependence, VerificationRequirement, VerificationVerdict,
    canonical_resource_budget_digest, validate_request, verify_protected_effect_result,
    verify_terminal_receipt,
};
use gaap::runtime::{
    AgentAdapter, AgentRunContext, AgentRunEngine, AgentStep, BudgetThresholds,
    ExecutorObservation, ExecutorPort, PermissionInput, PlanProposal, PolicyDecision,
    ProtectedEffectProposal, RuntimeConfig, RuntimePortError, RuntimeSupport, VerificationContext,
    VerificationReport, VerifierPort,
};

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

fn runtime_support(trusted_capabilities: Vec<CapabilityIdentity>) -> RuntimeSupport {
    RuntimeSupport::new(support(), trusted_capabilities)
}

fn config() -> RuntimeConfig {
    RuntimeConfig {
        implementer_id: "runtime-test-engine".to_string(),
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

fn allow_permission() -> PermissionInput {
    PermissionInput {
        policy_decision: Some(PolicyDecision::Allow),
        risk_tags: vec![],
        wrapper_chain: vec![],
        approval: None,
    }
}

fn ask_permission() -> PermissionInput {
    PermissionInput {
        policy_decision: Some(PolicyDecision::Ask),
        risk_tags: vec![],
        wrapper_chain: vec![],
        approval: None,
    }
}

fn deny_permission() -> PermissionInput {
    PermissionInput {
        policy_decision: Some(PolicyDecision::Deny),
        risk_tags: vec![],
        wrapper_chain: vec![],
        approval: None,
    }
}

fn plan_for(request: &AgentRunRequest) -> PlanProposal {
    PlanProposal {
        plan_digest: digest(b'6'),
        approval: Some(approval_for(&request.subject.digest)),
    }
}

fn proposal(
    effect: ProtectedEffectRequest,
    permission: PermissionInput,
) -> ProtectedEffectProposal {
    proposal_with_cost(effect, permission, 10)
}

fn proposal_with_cost(
    effect: ProtectedEffectRequest,
    permission: PermissionInput,
    estimated_cost_micros: u64,
) -> ProtectedEffectProposal {
    ProtectedEffectProposal {
        request: effect,
        permission,
        estimated_cost_micros,
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

fn executed_observation_with_cost(next_subject: Subject, cost_micros: u64) -> ExecutorObservation {
    let mut observation = executed_observation(next_subject);
    observation.usage = usage(cost_micros);
    observation
}

fn unknown_outcome_observation() -> ExecutorObservation {
    ExecutorObservation {
        execution_status: EffectExecutionStatus::UnknownOutcome,
        observed_post_effect_subject: None,
        exit: None,
        usage: usage(10),
        executor: Some(executor_identity()),
        sandbox_profile: Some(SandboxProfileIdentity {
            name: "sandbox-fixture".to_string(),
            version: "0.1.0".to_string(),
            digest: digest(b'8'),
        }),
        reason: Some("executor lost post-effect visibility".to_string()),
        evidence: vec![
            effect_evidence(EffectEvidenceType::Executor, b'a'),
            effect_evidence(EffectEvidenceType::Sandbox, b'0'),
            effect_evidence(EffectEvidenceType::Usage, b'3'),
            effect_evidence(EffectEvidenceType::UnknownOutcome, b'5'),
        ],
    }
}

fn interrupted_observation() -> ExecutorObservation {
    ExecutorObservation {
        execution_status: EffectExecutionStatus::Interrupted,
        observed_post_effect_subject: Some(subject(digest(b'1'))),
        exit: None,
        usage: usage(10),
        executor: Some(executor_identity()),
        sandbox_profile: Some(SandboxProfileIdentity {
            name: "sandbox-fixture".to_string(),
            version: "0.1.0".to_string(),
            digest: digest(b'8'),
        }),
        reason: Some("operator interrupted run".to_string()),
        evidence: vec![
            effect_evidence(EffectEvidenceType::Executor, b'a'),
            effect_evidence(EffectEvidenceType::Artifact, b'b'),
            effect_evidence(EffectEvidenceType::Sandbox, b'0'),
            effect_evidence(EffectEvidenceType::SubjectObservation, b'1'),
            effect_evidence(EffectEvidenceType::Mutation, b'2'),
            effect_evidence(EffectEvidenceType::Usage, b'3'),
            effect_evidence(EffectEvidenceType::Interruption, b'5'),
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

struct ScriptedAgent {
    plan: Option<Result<PlanProposal, RuntimePortError>>,
    steps: VecDeque<Result<AgentStep, RuntimePortError>>,
}

impl ScriptedAgent {
    fn new(
        plan: PlanProposal,
        steps: impl IntoIterator<Item = Result<AgentStep, RuntimePortError>>,
    ) -> Self {
        Self {
            plan: Some(Ok(plan)),
            steps: steps.into_iter().collect(),
        }
    }
}

impl AgentAdapter for ScriptedAgent {
    fn plan(&mut self, _request: &AgentRunRequest) -> Result<PlanProposal, RuntimePortError> {
        self.plan
            .take()
            .expect("agent plan should be called exactly once")
    }

    fn next_effect(
        &mut self,
        _context: AgentRunContext<'_>,
    ) -> Result<AgentStep, RuntimePortError> {
        self.steps
            .pop_front()
            .expect("agent should have another scripted step")
    }
}

struct RecordingExecutor {
    calls: Rc<RefCell<Vec<String>>>,
    observations: VecDeque<Result<ExecutorObservation, RuntimePortError>>,
}

impl RecordingExecutor {
    fn new(
        calls: Rc<RefCell<Vec<String>>>,
        observations: impl IntoIterator<Item = Result<ExecutorObservation, RuntimePortError>>,
    ) -> Self {
        Self {
            calls,
            observations: observations.into_iter().collect(),
        }
    }
}

impl ExecutorPort for RecordingExecutor {
    fn execute(
        &mut self,
        request: &ProtectedEffectRequest,
    ) -> Result<ExecutorObservation, RuntimePortError> {
        self.calls.borrow_mut().push(request.effect_id.clone());
        self.observations
            .pop_front()
            .expect("executor should have another scripted observation")
    }
}

struct ScriptedVerifier {
    report: Option<Result<VerificationReport, RuntimePortError>>,
}

impl ScriptedVerifier {
    fn new(report: VerificationReport) -> Self {
        Self {
            report: Some(Ok(report)),
        }
    }
}

impl VerifierPort for ScriptedVerifier {
    fn verify(
        &mut self,
        _context: VerificationContext<'_>,
    ) -> Result<VerificationReport, RuntimePortError> {
        self.report
            .take()
            .expect("verifier should be called exactly once")
    }
}

fn run_with(
    run_request: AgentRunRequest,
    runtime_support: RuntimeSupport,
    agent: ScriptedAgent,
    executor: RecordingExecutor,
    verifier: ScriptedVerifier,
) -> gaap::runtime::AgentRunExecution {
    let mut engine = AgentRunEngine::new(runtime_support, config(), agent, executor, verifier);
    engine
        .run(run_request)
        .expect("runtime should return receipt")
}

#[test]
fn completed_run_returns_a_sealed_terminal_receipt_and_effect_result() {
    let run_request = request();
    let support = support();
    let effect = effect_request(&run_request, &support, 1);
    let post_subject = subject(digest(b'e'));
    let calls = Rc::new(RefCell::new(Vec::new()));

    let execution = run_with(
        run_request.clone(),
        runtime_support(vec![run_request.requested_capability.clone()]),
        ScriptedAgent::new(
            plan_for(&run_request),
            [
                Ok(AgentStep::ProtectedEffect(Box::new(proposal(
                    effect.clone(),
                    allow_permission(),
                )))),
                Ok(AgentStep::Finish),
            ],
        ),
        RecordingExecutor::new(
            calls.clone(),
            [Ok(executed_observation(post_subject.clone()))],
        ),
        ScriptedVerifier::new(passing_verification(&post_subject.digest)),
    );

    assert_eq!(
        execution.receipt.body.terminal_status,
        AgentRunStatus::Completed
    );
    assert_eq!(
        execution.receipt.body.terminal_reason,
        "workflow.completion_authorized"
    );
    assert_eq!(calls.borrow().as_slice(), ["effect-1"]);
    assert_eq!(execution.receipt.body.usage.cost_micros, 10);
    assert_eq!(execution.protected_effect_results.len(), 1);
    assert_eq!(
        execution.protected_effect_results[0].body.execution_status,
        EffectExecutionStatus::Executed
    );

    verify_terminal_receipt(&run_request, &support, &execution.receipt).unwrap();
    verify_protected_effect_result(
        &run_request,
        &support,
        &effect,
        &execution.protected_effect_results[0],
    )
    .unwrap();
}

#[test]
fn ask_decision_terminalizes_without_invoking_executor() {
    let run_request = request();
    let support = support();
    let effect = effect_request(&run_request, &support, 1);
    let calls = Rc::new(RefCell::new(Vec::new()));

    let execution = run_with(
        run_request.clone(),
        runtime_support(vec![run_request.requested_capability.clone()]),
        ScriptedAgent::new(
            plan_for(&run_request),
            [Ok(AgentStep::ProtectedEffect(Box::new(proposal(
                effect.clone(),
                ask_permission(),
            ))))],
        ),
        RecordingExecutor::new(calls.clone(), []),
        ScriptedVerifier::new(passing_verification(&run_request.subject.digest)),
    );

    assert_eq!(
        execution.receipt.body.terminal_status,
        AgentRunStatus::Blocked
    );
    assert_eq!(
        execution.receipt.body.terminal_reason,
        "permission.policy_requires_approval"
    );
    assert!(calls.borrow().is_empty());
    assert_eq!(
        execution.protected_effect_results[0].body.execution_status,
        EffectExecutionStatus::AwaitingAuthority
    );

    verify_terminal_receipt(&run_request, &support, &execution.receipt).unwrap();
    verify_protected_effect_result(
        &run_request,
        &support,
        &effect,
        &execution.protected_effect_results[0],
    )
    .unwrap();
}

#[test]
fn denied_effect_terminalizes_without_invoking_executor() {
    let run_request = request();
    let support = support();
    let effect = effect_request(&run_request, &support, 1);
    let calls = Rc::new(RefCell::new(Vec::new()));

    let execution = run_with(
        run_request.clone(),
        runtime_support(vec![run_request.requested_capability.clone()]),
        ScriptedAgent::new(
            plan_for(&run_request),
            [Ok(AgentStep::ProtectedEffect(Box::new(proposal(
                effect.clone(),
                deny_permission(),
            ))))],
        ),
        RecordingExecutor::new(calls.clone(), []),
        ScriptedVerifier::new(passing_verification(&run_request.subject.digest)),
    );

    assert_eq!(
        execution.receipt.body.terminal_status,
        AgentRunStatus::Blocked
    );
    assert_eq!(execution.receipt.body.terminal_reason, "permission.denied");
    assert!(calls.borrow().is_empty());
    assert_eq!(
        execution.protected_effect_results[0].body.execution_status,
        EffectExecutionStatus::Denied
    );

    verify_terminal_receipt(&run_request, &support, &execution.receipt).unwrap();
    verify_protected_effect_result(
        &run_request,
        &support,
        &effect,
        &execution.protected_effect_results[0],
    )
    .unwrap();
}

#[test]
fn untrusted_capability_blocks_before_executor() {
    let run_request = request();
    let support = support();
    let effect = effect_request(&run_request, &support, 1);
    let calls = Rc::new(RefCell::new(Vec::new()));

    let execution = run_with(
        run_request.clone(),
        runtime_support(vec![]),
        ScriptedAgent::new(
            plan_for(&run_request),
            [Ok(AgentStep::ProtectedEffect(Box::new(proposal(
                effect.clone(),
                allow_permission(),
            ))))],
        ),
        RecordingExecutor::new(calls.clone(), []),
        ScriptedVerifier::new(passing_verification(&run_request.subject.digest)),
    );

    assert_eq!(
        execution.receipt.body.terminal_status,
        AgentRunStatus::Blocked
    );
    assert_eq!(
        execution.receipt.body.terminal_reason,
        "tool_trust.approval_required"
    );
    assert!(calls.borrow().is_empty());
    assert_eq!(
        execution.protected_effect_results[0].body.execution_status,
        EffectExecutionStatus::Denied
    );

    verify_terminal_receipt(&run_request, &support, &execution.receipt).unwrap();
    verify_protected_effect_result(
        &run_request,
        &support,
        &effect,
        &execution.protected_effect_results[0],
    )
    .unwrap();
}

#[test]
fn runtime_hard_stop_blocks_before_executor() {
    let run_request = request();
    let support = support();
    let effect = effect_request(&run_request, &support, 1);
    let calls = Rc::new(RefCell::new(Vec::new()));

    let execution = run_with(
        run_request.clone(),
        runtime_support(vec![run_request.requested_capability.clone()]),
        ScriptedAgent::new(
            plan_for(&run_request),
            [Ok(AgentStep::ProtectedEffect(Box::new(
                proposal_with_cost(effect.clone(), allow_permission(), 1_000_000),
            )))],
        ),
        RecordingExecutor::new(calls.clone(), []),
        ScriptedVerifier::new(passing_verification(&run_request.subject.digest)),
    );

    assert_eq!(
        execution.receipt.body.terminal_status,
        AgentRunStatus::Blocked
    );
    assert_eq!(execution.receipt.body.terminal_reason, "runtime.hard_stop");
    assert!(calls.borrow().is_empty());
    assert_eq!(
        execution.protected_effect_results[0].body.execution_status,
        EffectExecutionStatus::Denied
    );

    verify_terminal_receipt(&run_request, &support, &execution.receipt).unwrap();
    verify_protected_effect_result(
        &run_request,
        &support,
        &effect,
        &execution.protected_effect_results[0],
    )
    .unwrap();
}

#[test]
fn actual_usage_over_budget_blocks_before_verification() {
    let run_request = request();
    let support = support();
    let effect = effect_request(&run_request, &support, 1);
    let post_subject = subject(digest(b'e'));
    let calls = Rc::new(RefCell::new(Vec::new()));

    let execution = run_with(
        run_request.clone(),
        runtime_support(vec![run_request.requested_capability.clone()]),
        ScriptedAgent::new(
            plan_for(&run_request),
            [Ok(AgentStep::ProtectedEffect(Box::new(proposal(
                effect.clone(),
                allow_permission(),
            ))))],
        ),
        RecordingExecutor::new(
            calls.clone(),
            [Ok(executed_observation_with_cost(post_subject, 1_000_001))],
        ),
        ScriptedVerifier::new(passing_verification(&run_request.subject.digest)),
    );

    assert_eq!(
        execution.receipt.body.terminal_status,
        AgentRunStatus::Blocked
    );
    assert_eq!(
        execution.receipt.body.terminal_reason,
        "runtime.budget_exceeded"
    );
    assert_eq!(calls.borrow().as_slice(), ["effect-1"]);
    assert_eq!(execution.receipt.body.usage.cost_micros, 1_000_001);
    assert_eq!(
        execution.protected_effect_results[0].body.execution_status,
        EffectExecutionStatus::Executed
    );

    verify_terminal_receipt(&run_request, &support, &execution.receipt).unwrap();
    verify_protected_effect_result(
        &run_request,
        &support,
        &effect,
        &execution.protected_effect_results[0],
    )
    .unwrap();
}

#[test]
fn executor_unknown_outcome_is_distinct_from_failure() {
    let run_request = request();
    let support = support();
    let effect = effect_request(&run_request, &support, 1);
    let calls = Rc::new(RefCell::new(Vec::new()));

    let execution = run_with(
        run_request.clone(),
        runtime_support(vec![run_request.requested_capability.clone()]),
        ScriptedAgent::new(
            plan_for(&run_request),
            [Ok(AgentStep::ProtectedEffect(Box::new(proposal(
                effect.clone(),
                allow_permission(),
            ))))],
        ),
        RecordingExecutor::new(calls.clone(), [Ok(unknown_outcome_observation())]),
        ScriptedVerifier::new(passing_verification(&run_request.subject.digest)),
    );

    assert_eq!(
        execution.receipt.body.terminal_status,
        AgentRunStatus::Failed
    );
    assert_eq!(
        execution.receipt.body.terminal_reason,
        "protected_effect.unknown_outcome"
    );
    assert_eq!(calls.borrow().as_slice(), ["effect-1"]);
    assert_eq!(
        execution.protected_effect_results[0].body.execution_status,
        EffectExecutionStatus::UnknownOutcome
    );

    verify_terminal_receipt(&run_request, &support, &execution.receipt).unwrap();
    verify_protected_effect_result(
        &run_request,
        &support,
        &effect,
        &execution.protected_effect_results[0],
    )
    .unwrap();
}

#[test]
fn interrupted_effect_produces_interrupted_receipt() {
    let run_request = request();
    let support = support();
    let effect = effect_request(&run_request, &support, 1);
    let calls = Rc::new(RefCell::new(Vec::new()));

    let execution = run_with(
        run_request.clone(),
        runtime_support(vec![run_request.requested_capability.clone()]),
        ScriptedAgent::new(
            plan_for(&run_request),
            [Ok(AgentStep::ProtectedEffect(Box::new(proposal(
                effect.clone(),
                allow_permission(),
            ))))],
        ),
        RecordingExecutor::new(calls.clone(), [Ok(interrupted_observation())]),
        ScriptedVerifier::new(passing_verification(&run_request.subject.digest)),
    );

    assert_eq!(
        execution.receipt.body.terminal_status,
        AgentRunStatus::Interrupted
    );
    assert_eq!(
        execution.receipt.body.terminal_reason,
        "protected_effect.interrupted"
    );
    assert_eq!(calls.borrow().as_slice(), ["effect-1"]);
    assert!(
        execution
            .receipt
            .body
            .events
            .iter()
            .any(|event| matches!(event, RunEvent::Interruption { .. }))
    );

    verify_terminal_receipt(&run_request, &support, &execution.receipt).unwrap();
    verify_protected_effect_result(
        &run_request,
        &support,
        &effect,
        &execution.protected_effect_results[0],
    )
    .unwrap();
}

#[test]
fn stale_verification_blocks_completion() {
    let run_request = request();
    let support = support();
    let effect = effect_request(&run_request, &support, 1);
    let post_subject = subject(digest(b'e'));
    let calls = Rc::new(RefCell::new(Vec::new()));

    let execution = run_with(
        run_request.clone(),
        runtime_support(vec![run_request.requested_capability.clone()]),
        ScriptedAgent::new(
            plan_for(&run_request),
            [
                Ok(AgentStep::ProtectedEffect(Box::new(proposal(
                    effect.clone(),
                    allow_permission(),
                )))),
                Ok(AgentStep::Finish),
            ],
        ),
        RecordingExecutor::new(calls.clone(), [Ok(executed_observation(post_subject))]),
        ScriptedVerifier::new(passing_verification(&digest(b'f'))),
    );

    assert_eq!(
        execution.receipt.body.terminal_status,
        AgentRunStatus::Blocked
    );
    assert_eq!(
        execution.receipt.body.terminal_reason,
        "verification.stale_subject"
    );
    assert_eq!(calls.borrow().as_slice(), ["effect-1"]);

    verify_terminal_receipt(&run_request, &support, &execution.receipt).unwrap();
}

#[test]
fn same_inputs_produce_same_receipt_and_effect_digests() {
    fn successful_execution() -> gaap::runtime::AgentRunExecution {
        let run_request = request();
        let support = support();
        let effect = effect_request(&run_request, &support, 1);
        let post_subject = subject(digest(b'e'));
        let calls = Rc::new(RefCell::new(Vec::new()));

        run_with(
            run_request.clone(),
            runtime_support(vec![run_request.requested_capability.clone()]),
            ScriptedAgent::new(
                plan_for(&run_request),
                [
                    Ok(AgentStep::ProtectedEffect(Box::new(proposal(
                        effect.clone(),
                        allow_permission(),
                    )))),
                    Ok(AgentStep::Finish),
                ],
            ),
            RecordingExecutor::new(calls, [Ok(executed_observation(post_subject.clone()))]),
            ScriptedVerifier::new(passing_verification(&post_subject.digest)),
        )
    }

    let first = successful_execution();
    let second = successful_execution();

    assert_eq!(first.receipt.receipt_digest, second.receipt.receipt_digest);
    assert_eq!(
        first.protected_effect_results[0].result_digest,
        second.protected_effect_results[0].result_digest
    );
}
