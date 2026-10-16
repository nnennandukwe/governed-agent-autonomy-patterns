//! Adapter to the real engine. This module supplies observations, never expected decisions.
use std::{cell::RefCell, rc::Rc};

use gaap::{contracts::*, runtime::*};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::model::{Defect, History, Meter, Operation, Outcome, Permission, Verification};

pub fn hash(value: &impl Serialize) -> String {
    format!(
        "sha256:{:x}",
        Sha256::digest(serde_jcs::to_vec(value).unwrap())
    )
}
pub fn subject(revision: u64) -> Subject {
    Subject {
        kind: SubjectKind::Repository,
        locator: "fixture://workspace".into(),
        digest: hash(&revision),
    }
}
fn resource(meter: Meter) -> ResourceUsage {
    ResourceUsage {
        cost_micros: meter[0],
        elapsed_ms: meter[1],
        model_tokens: meter[2],
        tool_calls: meter[3],
    }
}
fn effect_usage(meter: Meter) -> EffectUsage {
    EffectUsage {
        cost_micros: meter[0],
        elapsed_ms: meter[1],
        model_tokens: meter[2],
        tool_calls: meter[3],
    }
}
pub fn meter(usage: &ResourceUsage) -> Meter {
    [
        usage.cost_micros,
        usage.elapsed_ms,
        usage.model_tokens,
        usage.tool_calls,
    ]
}
fn evidence(kind: EvidenceType) -> EvidenceReference {
    EvidenceReference {
        evidence_type: kind,
        digest: hash(&format!("{kind:?}")),
        locator: None,
    }
}
fn executor() -> ExecutorIdentity {
    ExecutorIdentity {
        name: "property-executor".into(),
        version: "1".into(),
        digest: hash(&"executor"),
    }
}
pub fn request(history: &History) -> AgentRunRequest {
    let mut request: AgentRunRequest = serde_json::from_str(include_str!(
        "../../examples/contracts/v0.1.0/agent-run-request.json"
    ))
    .unwrap();
    request.subject = subject(0);
    request.approval_context[0].subject_digest = request.subject.digest.clone();
    if !history.approved_plan {
        request.approval_context.clear();
    }
    let [
        max_cost_micros,
        max_elapsed_ms,
        max_model_tokens,
        max_tool_calls,
    ] = history.budget;
    request.resource_budget = ResourceBudget {
        max_cost_micros,
        max_elapsed_ms,
        max_model_tokens,
        max_tool_calls,
    };
    request
}
pub fn effect(
    request: &AgentRunRequest,
    current: &Subject,
    sequence: u64,
    operation: &Operation,
) -> ProtectedEffectRequest {
    let Operation::Effect {
        identity,
        content,
        defect,
        repeatable,
        ..
    } = operation
    else {
        unreachable!()
    };
    let mut proposal: ProtectedEffectRequest = serde_json::from_str(include_str!(
        "../../examples/contracts/protected-effect/v0.1.0/protected-effect-request.json"
    ))
    .unwrap();
    proposal.operation_family = OperationFamily::Filesystem;
    proposal.normalized_operation = "filesystem.write".into();
    proposal.requested_scopes = vec![RequestedScope::Filesystem {
        root: "/workspace".into(),
        access: vec![FilesystemAccess::Modify],
        recursive: false,
    }];
    proposal.agent_run_request_digest = hash(request);
    proposal.resource_budget_digest = hash(&request.resource_budget);
    proposal.run_id = request.run_id.clone();
    proposal.effect_sequence = sequence;
    proposal.effect_id = format!("effect-{identity}");
    proposal.idempotency_key = format!("key-{identity}");
    proposal.input_digest = hash(content);
    proposal.subject = if *defect == Defect::StaleSubject {
        subject(u64::MAX)
    } else {
        current.clone()
    };
    proposal.approval_context.clear();
    proposal.capability = request.requested_capability.clone();
    proposal.policies = request.policies.clone();
    proposal.tool_schema_digest = Some(hash(&"schema"));
    if *defect == Defect::Schema {
        proposal.tool_schema_digest = Some(hash(&"changed-schema"));
    }
    if *defect == Defect::Descriptor {
        proposal.capability.digest = hash(&"changed-descriptor");
    }
    proposal.repeatability = if *repeatable {
        Repeatability::Repeatable
    } else {
        Repeatability::NonRepeatable
    };
    proposal
}
#[derive(Default, Debug)]
pub struct Trace {
    pub executor_calls: Vec<usize>,
    pub external_dispatches: Vec<usize>,
    pub contexts: Vec<(String, Meter)>,
    pub requests: Vec<ProtectedEffectRequest>,
    pub next_calls: usize,
    pub verifier_calls: usize,
    pub completion_checks: usize,
    pub closes: usize,
    active: Option<Operation>,
    active_index: usize,
    finish: Option<(Verification, Meter)>,
    revision: u64,
}
type Shared = Rc<RefCell<Trace>>;
struct Agent {
    history: History,
    request: AgentRunRequest,
    trace: Shared,
}
impl AgentAdapter for Agent {
    fn close(&mut self) {
        self.trace.borrow_mut().closes += 1;
    }
    fn plan(
        &mut self,
        _: &AgentRunRequest,
    ) -> Result<PortObservation<PlanProposal>, RuntimePortError> {
        Ok(PortObservation {
            value: PlanProposal {
                plan_digest: hash(&"plan"),
            },
            usage: resource(self.history.plan_usage),
        })
    }
    fn next_effect(
        &mut self,
        context: AgentRunContext<'_>,
    ) -> Result<PortObservation<AgentStep>, RuntimePortError> {
        let mut trace = self.trace.borrow_mut();
        let index = trace.next_calls;
        trace.next_calls += 1;
        trace
            .contexts
            .push((context.current_subject.digest.clone(), meter(context.usage)));
        let operation = self.history.operations.get(index).cloned().ok_or_else(|| {
            RuntimePortError::new("history exhausted without an explicit terminal operation")
        })?;
        trace.active_index = index;
        trace.active = Some(operation.clone());
        let step = match &operation {
            Operation::Effect { .. } => {
                let request = effect(
                    &self.request,
                    context.current_subject,
                    context.effects_seen + 1,
                    &operation,
                );
                trace.requests.push(request.clone());
                AgentStep::ProtectedEffect(Box::new(ProtectedEffectProposal { request }))
            }
            Operation::Finish {
                verification,
                usage,
            } => {
                trace.finish = Some((*verification, *usage));
                AgentStep::Finish
            }
            Operation::Cancel => AgentStep::Cancel(Cancellation {
                actor_id: Some("host".into()),
                reason: "cancelled".into(),
                evidence: evidence(EvidenceType::Interruption),
            }),
            Operation::AdapterError { usage } => {
                return Err(RuntimePortError::new("agent failed").with_usage(resource(*usage)));
            }
        };
        Ok(PortObservation {
            value: step,
            usage: resource([0; 4]),
        })
    }
}
struct Executor(Shared);
impl ExecutorPort for Executor {
    fn execute(
        &mut self,
        request: &ProtectedEffectRequest,
    ) -> Result<ExecutorObservation, RuntimePortError> {
        let mut trace = self.0.borrow_mut();
        let index = trace.active_index;
        trace.executor_calls.push(index);
        let Some(Operation::Effect { outcome, usage, .. }) = trace.active.clone() else {
            unreachable!()
        };
        if outcome == Outcome::BeforeDispatch {
            return Err(
                RuntimePortError::before_dispatch("intent persistence failed")
                    .with_usage(resource(usage)),
            );
        }
        if outcome != Outcome::Denied {
            trace.external_dispatches.push(index);
        }
        if outcome == Outcome::LostReply {
            return Err(RuntimePortError::new("reply lost after dispatch"));
        }
        let status = match outcome {
            Outcome::Success => EffectExecutionStatus::Executed,
            Outcome::Denied => EffectExecutionStatus::Denied,
            Outcome::FailedPreserved | Outcome::FailedChanged => EffectExecutionStatus::Failed,
            Outcome::Interrupted | Outcome::Timeout => EffectExecutionStatus::Interrupted,
            Outcome::UnknownPreserved | Outcome::UnknownChanged => {
                EffectExecutionStatus::UnknownOutcome
            }
            Outcome::BeforeDispatch | Outcome::LostReply => unreachable!(),
        };
        let changed = matches!(
            outcome,
            Outcome::Success | Outcome::FailedChanged | Outcome::UnknownChanged
        );
        if changed {
            trace.revision += 1;
        }
        let post = if outcome == Outcome::Denied {
            None
        } else {
            Some(subject(trace.revision))
        };
        let mut kinds = vec![
            EffectEvidenceType::Executor,
            EffectEvidenceType::Sandbox,
            EffectEvidenceType::Usage,
        ];
        if post.is_some() {
            kinds.extend([
                EffectEvidenceType::SubjectObservation,
                EffectEvidenceType::Mutation,
                EffectEvidenceType::Artifact,
            ]);
        }
        match status {
            EffectExecutionStatus::Executed => kinds.push(EffectEvidenceType::Output),
            EffectExecutionStatus::Failed => kinds.push(EffectEvidenceType::Failure),
            EffectExecutionStatus::Interrupted => kinds.push(EffectEvidenceType::Interruption),
            EffectExecutionStatus::UnknownOutcome => kinds.push(EffectEvidenceType::UnknownOutcome),
            _ => (),
        }
        Ok(ExecutorObservation {
            execution_status: status,
            observed_post_effect_subject: post,
            exit: None,
            usage: effect_usage(usage),
            executor: Some(executor()),
            sandbox_profile: Some(request.sandbox_profile.clone()),
            reason: (status != EffectExecutionStatus::Executed).then(|| format!("{outcome:?}")),
            evidence: kinds
                .into_iter()
                .map(|kind| EffectEvidenceReference {
                    evidence_type: kind,
                    digest: hash(&format!("{kind:?}")),
                    locator: None,
                })
                .collect(),
        })
    }
}
struct Verifier(Shared);
impl VerifierPort for Verifier {
    fn verify(
        &mut self,
        context: VerificationContext<'_>,
    ) -> Result<PortObservation<VerificationReport>, RuntimePortError> {
        let mut trace = self.0.borrow_mut();
        trace.verifier_calls += 1;
        let (verification, usage) = trace.finish.unwrap();
        if verification == Verification::Error {
            return Err(RuntimePortError::new("verifier failed").with_usage(resource(usage)));
        }
        let verifier_id = match verification {
            Verification::SameActor => "implementer",
            Verification::Untrusted => "untrusted",
            _ => "independent",
        };
        Ok(PortObservation {
            value: VerificationReport {
                subject_digest: if verification == Verification::Stale {
                    subject(u64::MAX).digest
                } else {
                    context.current_subject.digest.clone()
                },
                verifier_id: verifier_id.into(),
                verdict: if verification == Verification::Fail {
                    VerificationVerdict::Fail
                } else {
                    VerificationVerdict::Pass
                },
                evidence: vec![
                    evidence(EvidenceType::CommandOutput),
                    evidence(EvidenceType::Artifact),
                ],
            },
            usage: resource(usage),
        })
    }
    fn completion_check(
        &mut self,
        _: VerificationContext<'_>,
    ) -> Result<PortObservation<()>, RuntimePortError> {
        let mut trace = self.0.borrow_mut();
        trace.completion_checks += 1;
        if matches!(
            trace.finish.unwrap().0,
            Verification::ChangedDuring | Verification::ChangedAfter
        ) {
            return Err(RuntimePortError::new("subject changed after verification"));
        }
        Ok(PortObservation {
            value: (),
            usage: resource([0; 4]),
        })
    }
}
pub fn execute(history: &History) -> Result<(AgentRunExecution, Trace), String> {
    let request = request(history);
    let trace = Shared::default();
    let support = RuntimeSupport::new(
        ContractSupport::new(request.policies.clone()),
        vec![request.requested_capability.clone()],
        PermissionPolicy {
            policy_decision: Some(match history.permission {
                Permission::Allow => PolicyDecision::Allow,
                Permission::Ask => PolicyDecision::Ask,
                Permission::Block => PolicyDecision::Deny,
            }),
            risk_tags: vec![],
            wrapper_chain: vec![],
            approvals: vec![],
        },
        effect_usage(history.estimate),
        vec![],
        vec!["independent".into(), "implementer".into()],
    )
    .with_trusted_tool_schemas(vec![TrustedToolSchema {
        capability: request.requested_capability.clone(),
        tool_schema_digest: hash(&"schema"),
    }]);
    let mut engine = AgentRunEngine::new(
        support,
        RuntimeConfig {
            implementer_id: "implementer".into(),
            executor_identity: executor(),
            max_effects: history.max_effects as u64,
            budget_thresholds: BudgetThresholds {
                warn_micros: history.budget[0],
                approval_micros: history.budget[0],
                hard_stop_micros: history.budget[0],
            },
        },
        Agent {
            history: history.clone(),
            request: request.clone(),
            trace: trace.clone(),
        },
        Executor(trace.clone()),
        Verifier(trace.clone()),
    );
    let execution = engine.run(request).map_err(|error| error.to_string())?;
    drop(engine);
    Ok((execution, Rc::try_unwrap(trace).unwrap().into_inner()))
}
