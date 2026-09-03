use std::collections::BTreeSet;
use std::error::Error;
use std::fmt::{self, Display, Formatter};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::contracts::{
    AgentRunRequest, AgentRunStatus, ApprovalReference, CapabilityIdentity, ContractError,
    ContractSupport, EffectClass, EffectEvidenceReference, EffectEvidenceType,
    EffectExecutionStatus, EffectUsage, EvidenceReference, EvidenceType, ExecutorIdentity,
    PROTECTED_EFFECT_RESULT_SCHEMA, ProtectedEffectDecision, ProtectedEffectRequest,
    ProtectedEffectResult, ProtectedEffectResultBody, ResourceUsage, RunEvent,
    TERMINAL_RUN_RECEIPT_SCHEMA, TerminalRunReceipt, TerminalRunReceiptBody, VerificationVerdict,
    seal_protected_effect_result, seal_terminal_receipt, validate_protected_effect_request,
    validate_request,
};
use crate::{Decision, Gate, Outcome, RunCoordinator};

/// The sealed terminal output of one bounded Agent Run execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRunExecution {
    pub receipt: TerminalRunReceipt,
    pub protected_effect_results: Vec<ProtectedEffectResult>,
}

/// Runtime-only support metadata that does not alter the frozen v0.1 contracts.
#[derive(Debug, Clone)]
pub struct RuntimeSupport {
    contract_support: ContractSupport,
    trusted_capabilities: Vec<CapabilityIdentity>,
    permission_policy: PermissionPolicy,
    protected_effect_usage_estimate: EffectUsage,
    runtime_approvals: Vec<GateApproval>,
    trusted_verifier_ids: Vec<String>,
}

impl RuntimeSupport {
    /// Construct runtime support from trusted, runtime-owned gate inputs.
    pub fn new(
        contract_support: ContractSupport,
        trusted_capabilities: Vec<CapabilityIdentity>,
        permission_policy: PermissionPolicy,
        protected_effect_usage_estimate: EffectUsage,
        runtime_approvals: Vec<GateApproval>,
        trusted_verifier_ids: Vec<String>,
    ) -> Self {
        Self {
            contract_support,
            trusted_capabilities,
            permission_policy,
            protected_effect_usage_estimate,
            runtime_approvals,
            trusted_verifier_ids,
        }
    }

    /// Return the contract support used for request and receipt validation.
    pub fn contract_support(&self) -> &ContractSupport {
        &self.contract_support
    }

    fn trusts_capability(&self, capability: &CapabilityIdentity) -> bool {
        self.trusted_capabilities
            .iter()
            .any(|trusted| trusted == capability)
    }

    fn permission_policy(&self) -> &PermissionPolicy {
        &self.permission_policy
    }

    fn protected_effect_usage_estimate(&self) -> &EffectUsage {
        &self.protected_effect_usage_estimate
    }

    fn runtime_approval_for(&self, action_digest: &str) -> Option<&GateApproval> {
        self.runtime_approvals
            .iter()
            .find(|approval| approval.subject_digest == action_digest)
    }

    fn trusts_verifier(&self, verifier_id: &str) -> bool {
        self.trusted_verifier_ids
            .iter()
            .any(|trusted| trusted == verifier_id)
    }
}

/// Cost thresholds evaluated by the runtime gate before execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetThresholds {
    pub warn_micros: u64,
    pub approval_micros: u64,
    pub hard_stop_micros: u64,
}

/// Configuration for the deterministic in-memory Agent Run engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeConfig {
    pub implementer_id: String,
    pub executor_identity: ExecutorIdentity,
    pub max_effects: u64,
    pub budget_thresholds: BudgetThresholds,
}

/// A normalized approval for coordinator gates that are not contract approval references.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateApproval {
    pub subject_digest: String,
    pub max_cost_micros: Option<u64>,
}

/// The adapter's proposed plan digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanProposal {
    pub plan_digest: String,
}

/// A policy decision supplied by a deterministic policy adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyDecision {
    Allow,
    Ask,
    Deny,
}

impl PolicyDecision {
    fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Ask => "ask",
            Self::Deny => "deny",
        }
    }
}

/// Runtime-owned policy input for protected-effect permission decisions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionPolicy {
    pub policy_decision: Option<PolicyDecision>,
    pub risk_tags: Vec<String>,
    pub wrapper_chain: Vec<String>,
    pub approvals: Vec<GateApproval>,
}

impl PermissionPolicy {
    fn approval_for(&self, action_digest: &str) -> Option<&GateApproval> {
        self.approvals
            .iter()
            .find(|approval| approval.subject_digest == action_digest)
    }
}

/// One protected effect proposal from the agent adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtectedEffectProposal {
    pub request: ProtectedEffectRequest,
}

/// One adapter or verifier output plus the resources consumed to produce it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortObservation<T> {
    pub value: T,
    pub usage: ResourceUsage,
}

/// A bounded next step from the agent adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentStep {
    ProtectedEffect(Box<ProtectedEffectProposal>),
    Finish,
    Cancel(Cancellation),
}

/// Adapter-provided cancellation state for an interrupted run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cancellation {
    pub actor_id: Option<String>,
    pub reason: String,
    pub evidence: EvidenceReference,
}

/// Deterministic execution observation returned by the executor port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutorObservation {
    pub execution_status: EffectExecutionStatus,
    pub observed_post_effect_subject: Option<crate::contracts::Subject>,
    pub exit: Option<crate::contracts::EffectExit>,
    pub usage: EffectUsage,
    pub executor: Option<crate::contracts::ExecutorIdentity>,
    pub sandbox_profile: Option<crate::contracts::SandboxProfileIdentity>,
    pub reason: Option<String>,
    pub evidence: Vec<EffectEvidenceReference>,
}

/// Independent verification report returned by the verifier port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationReport {
    pub subject_digest: String,
    pub verifier_id: String,
    pub verdict: VerificationVerdict,
    pub evidence: Vec<EvidenceReference>,
}

/// Read-only context supplied when asking the agent adapter for another effect.
#[derive(Debug, Clone, Copy)]
pub struct AgentRunContext<'a> {
    pub current_subject: &'a crate::contracts::Subject,
    pub usage: &'a ResourceUsage,
    pub effects_seen: u64,
}

/// Read-only context supplied when requesting independent verification.
#[derive(Debug, Clone, Copy)]
pub struct VerificationContext<'a> {
    pub request: &'a AgentRunRequest,
    pub current_subject: &'a crate::contracts::Subject,
    pub usage: &'a ResourceUsage,
}

/// Narrow adapter that can propose a plan and the next bounded effect.
pub trait AgentAdapter {
    fn plan(
        &mut self,
        request: &AgentRunRequest,
    ) -> Result<PortObservation<PlanProposal>, RuntimePortError>;

    fn next_effect(
        &mut self,
        context: AgentRunContext<'_>,
    ) -> Result<PortObservation<AgentStep>, RuntimePortError>;
}

/// Narrow executor port used only after all mutation gates allow.
pub trait ExecutorPort {
    fn execute(
        &mut self,
        request: &ProtectedEffectRequest,
    ) -> Result<ExecutorObservation, RuntimePortError>;
}

/// Narrow verifier port used before the terminal completion decision.
pub trait VerifierPort {
    fn verify(
        &mut self,
        context: VerificationContext<'_>,
    ) -> Result<PortObservation<VerificationReport>, RuntimePortError>;
}

/// Failure returned by a runtime adapter port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePortError {
    message: String,
}

impl RuntimePortError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl Display for RuntimePortError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for RuntimePortError {}

/// Runtime-level failures before a terminal receipt can be sealed.
#[derive(Debug)]
pub enum RuntimeError {
    Contract(ContractError),
    InvalidConfig(String),
    Receipt(ContractError),
    ProtectedEffectResult(ContractError),
}

impl Display for RuntimeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Contract(error) => write!(formatter, "contract validation failed: {error}"),
            Self::InvalidConfig(message) => write!(formatter, "invalid runtime config: {message}"),
            Self::Receipt(error) => write!(formatter, "could not seal terminal receipt: {error}"),
            Self::ProtectedEffectResult(error) => {
                write!(formatter, "could not seal protected effect result: {error}")
            }
        }
    }
}

impl Error for RuntimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Contract(error) | Self::Receipt(error) | Self::ProtectedEffectResult(error) => {
                Some(error)
            }
            Self::InvalidConfig(_) => None,
        }
    }
}

enum EffectHandling {
    Continue(Box<Ledger>),
    Terminal(Box<AgentRunExecution>),
}

struct NonExecutionOutcome {
    decision: ProtectedEffectDecision,
    execution_status: EffectExecutionStatus,
    reason: String,
    evidence: Vec<EffectEvidenceReference>,
}

/// Deterministic in-memory engine for one bounded Agent Run.
pub struct AgentRunEngine<A, E, V> {
    support: RuntimeSupport,
    config: RuntimeConfig,
    coordinator: RunCoordinator,
    agent: A,
    executor: E,
    verifier: V,
}

impl<A, E, V> AgentRunEngine<A, E, V>
where
    A: AgentAdapter,
    E: ExecutorPort,
    V: VerifierPort,
{
    pub fn new(
        support: RuntimeSupport,
        config: RuntimeConfig,
        agent: A,
        executor: E,
        verifier: V,
    ) -> Self {
        Self {
            support,
            config,
            coordinator: RunCoordinator,
            agent,
            executor,
            verifier,
        }
    }

    pub fn run(&mut self, request: AgentRunRequest) -> Result<AgentRunExecution, RuntimeError> {
        validate_config(&self.config, &request)?;
        validate_support(&self.support)?;
        let request_digest = validate_request(&request, self.support.contract_support())
            .map_err(RuntimeError::Contract)?;
        let mut ledger = Ledger::new(request.clone(), request_digest);

        ledger.transition(AgentRunStatus::Planning, None);
        let plan_observation = match self.agent.plan(&request) {
            Ok(observation) => observation,
            Err(error) => {
                return self.finish(
                    ledger,
                    AgentRunStatus::Failed,
                    format!("adapter.plan_failed: {}", error.message()),
                );
            }
        };
        if let Some(reason) = ledger.record_usage(&plan_observation.usage) {
            return self.finish(ledger, AgentRunStatus::Blocked, reason);
        }

        let plan = plan_observation.value;
        if !is_digest_string(&plan.plan_digest) {
            return self.finish(ledger, AgentRunStatus::Failed, "plan.invalid_digest");
        }
        ledger.plan_recorded(plan.plan_digest.clone());
        let plan_approval = plan_approval_for(&request, &ledger.current_subject.digest);
        if let Some(approval) = plan_approval.cloned() {
            ledger.approval_recorded(approval);
        }
        let plan_decision = self.coordinator.evaluate(
            Gate::Plan,
            &plan_input(&ledger.current_subject, plan_approval),
        );
        if plan_decision.outcome != Outcome::Allow {
            return self.finish(ledger, AgentRunStatus::Blocked, plan_decision.code);
        }

        ledger.transition(AgentRunStatus::Executing, None);
        loop {
            let step_observation = match self.agent.next_effect(AgentRunContext {
                current_subject: &ledger.current_subject,
                usage: &ledger.usage,
                effects_seen: ledger.effects_seen,
            }) {
                Ok(observation) => observation,
                Err(error) => {
                    return self.finish(
                        ledger,
                        AgentRunStatus::Failed,
                        format!("adapter.next_effect_failed: {}", error.message()),
                    );
                }
            };
            if let Some(reason) = ledger.record_usage(&step_observation.usage) {
                return self.finish(ledger, AgentRunStatus::Blocked, reason);
            }

            match step_observation.value {
                AgentStep::ProtectedEffect(proposal) => {
                    match self.handle_protected_effect(ledger, *proposal)? {
                        EffectHandling::Continue(next) => {
                            ledger = *next;
                        }
                        EffectHandling::Terminal(execution) => {
                            return Ok(*execution);
                        }
                    }
                }
                AgentStep::Finish => {
                    return self.verify_and_finish(ledger);
                }
                AgentStep::Cancel(cancellation) => {
                    ledger.interruption(sanitize_cancellation(cancellation));
                    return self.finish(ledger, AgentRunStatus::Interrupted, "runtime.interrupted");
                }
            }
        }
    }

    fn handle_protected_effect(
        &mut self,
        mut ledger: Ledger,
        proposal: ProtectedEffectProposal,
    ) -> Result<EffectHandling, RuntimeError> {
        let effect_digest = match validate_protected_effect_request(
            &ledger.request,
            self.support.contract_support(),
            &proposal.request,
        ) {
            Ok(digest) => digest,
            Err(error) => {
                return self
                    .finish(
                        ledger,
                        AgentRunStatus::Failed,
                        format!("protected_effect.invalid_request: {}", error.message()),
                    )
                    .map(|execution| EffectHandling::Terminal(Box::new(execution)));
            }
        };

        if let Some(decision) =
            ledger.admission_decision(&proposal.request, &effect_digest, self.config.max_effects)
        {
            return self
                .decline_protected_effect(
                    ledger,
                    &proposal.request,
                    &effect_digest,
                    Gate::Runtime,
                    decision,
                )
                .map(|execution| EffectHandling::Terminal(Box::new(execution)));
        }

        if proposal.request.subject != ledger.current_subject {
            let decision = Decision {
                outcome: Outcome::Block,
                code: "protected_effect.stale_subject".to_string(),
                effects: vec!["stop_action".to_string()],
            };
            return self
                .decline_protected_effect(
                    ledger,
                    &proposal.request,
                    &effect_digest,
                    Gate::Permission,
                    decision,
                )
                .map(|execution| EffectHandling::Terminal(Box::new(execution)));
        }

        if let Some(decision) = budget_admission_decision(
            &ledger.usage,
            &ledger.request.resource_budget,
            self.support.protected_effect_usage_estimate(),
        ) {
            return self
                .decline_protected_effect(
                    ledger,
                    &proposal.request,
                    &effect_digest,
                    Gate::Runtime,
                    decision,
                )
                .map(|execution| EffectHandling::Terminal(Box::new(execution)));
        }

        let permission_decision = self.coordinator.evaluate(
            Gate::Permission,
            &permission_input(&effect_digest, self.support.permission_policy()),
        );
        let permission_decision_id =
            ledger.decision_id("permission", proposal.request.effect_sequence);
        ledger.protected_effect_decision(
            &permission_decision_id,
            Gate::Permission,
            &effect_digest,
            permission_decision.clone(),
        );
        if permission_decision.outcome != Outcome::Allow {
            let status = non_execution_status(&permission_decision);
            let result = self.non_execution_result(
                &ledger,
                &proposal.request,
                &effect_digest,
                NonExecutionOutcome {
                    decision: ProtectedEffectDecision {
                        decision_id: permission_decision_id,
                        gate: Gate::Permission,
                        effect_request_digest: effect_digest.clone(),
                        subject_digest: ledger.current_subject.digest.clone(),
                        decision: permission_decision.clone(),
                    },
                    execution_status: status,
                    reason: permission_decision.code.clone(),
                    evidence: vec![],
                },
            )?;
            ledger.effect_results.push(result);
            return self
                .blocked_from_decision(ledger, permission_decision)
                .map(|execution| EffectHandling::Terminal(Box::new(execution)));
        }

        let tool_trust_decision = self.coordinator.evaluate(
            Gate::ToolTrust,
            &tool_trust_input(&self.support, &proposal.request.capability),
        );
        let tool_trust_decision_id =
            ledger.decision_id("tool-trust", proposal.request.effect_sequence);
        ledger.protected_effect_decision(
            &tool_trust_decision_id,
            Gate::ToolTrust,
            &effect_digest,
            tool_trust_decision.clone(),
        );
        if tool_trust_decision.outcome != Outcome::Allow {
            let result = self.non_execution_result(
                &ledger,
                &proposal.request,
                &effect_digest,
                NonExecutionOutcome {
                    decision: ProtectedEffectDecision {
                        decision_id: tool_trust_decision_id,
                        gate: Gate::ToolTrust,
                        effect_request_digest: effect_digest.clone(),
                        subject_digest: ledger.current_subject.digest.clone(),
                        decision: tool_trust_decision.clone(),
                    },
                    execution_status: non_execution_status(&tool_trust_decision),
                    reason: tool_trust_decision.code.clone(),
                    evidence: vec![],
                },
            )?;
            ledger.effect_results.push(result);
            return self
                .blocked_from_decision(ledger, tool_trust_decision)
                .map(|execution| EffectHandling::Terminal(Box::new(execution)));
        }

        let runtime_decision = self.coordinator.evaluate(
            Gate::Runtime,
            &runtime_input(&self.support, &self.config, &ledger, &effect_digest),
        );
        let runtime_decision_id = ledger.decision_id("runtime", proposal.request.effect_sequence);
        ledger.protected_effect_decision(
            &runtime_decision_id,
            Gate::Runtime,
            &effect_digest,
            runtime_decision.clone(),
        );
        if runtime_decision.outcome != Outcome::Allow {
            let result = self.non_execution_result(
                &ledger,
                &proposal.request,
                &effect_digest,
                NonExecutionOutcome {
                    decision: ProtectedEffectDecision {
                        decision_id: runtime_decision_id,
                        gate: Gate::Runtime,
                        effect_request_digest: effect_digest.clone(),
                        subject_digest: ledger.current_subject.digest.clone(),
                        decision: runtime_decision.clone(),
                    },
                    execution_status: non_execution_status(&runtime_decision),
                    reason: runtime_decision.code.clone(),
                    evidence: vec![],
                },
            )?;
            ledger.effect_results.push(result);
            return self
                .blocked_from_decision(ledger, runtime_decision)
                .map(|execution| EffectHandling::Terminal(Box::new(execution)));
        }

        let workflow_decision_id =
            if proposal.request.expected_effect_class == EffectClass::Mutation {
                let workflow_decision = self.coordinator.evaluate(
                    Gate::Workflow,
                    &workflow_mutation_input(
                        &permission_decision,
                        &tool_trust_decision,
                        &runtime_decision,
                    ),
                );
                let workflow_decision_id =
                    ledger.decision_id("workflow", proposal.request.effect_sequence);
                ledger.protected_effect_decision(
                    &workflow_decision_id,
                    Gate::Workflow,
                    &effect_digest,
                    workflow_decision.clone(),
                );
                if workflow_decision.outcome != Outcome::Allow {
                    let result = self.non_execution_result(
                        &ledger,
                        &proposal.request,
                        &effect_digest,
                        NonExecutionOutcome {
                            decision: ProtectedEffectDecision {
                                decision_id: workflow_decision_id,
                                gate: Gate::Workflow,
                                effect_request_digest: effect_digest.clone(),
                                subject_digest: ledger.current_subject.digest.clone(),
                                decision: workflow_decision.clone(),
                            },
                            execution_status: non_execution_status(&workflow_decision),
                            reason: workflow_decision.code.clone(),
                            evidence: vec![],
                        },
                    )?;
                    ledger.effect_results.push(result);
                    return self
                        .blocked_from_decision(ledger, workflow_decision)
                        .map(|execution| EffectHandling::Terminal(Box::new(execution)));
                }
                Some(workflow_decision_id)
            } else {
                None
            };

        let observation = match self.executor.execute(&proposal.request) {
            Ok(observation) => observation,
            Err(error) => {
                let result = self.executor_error_result(
                    &ledger,
                    &proposal.request,
                    &effect_digest,
                    ProtectedEffectDecision {
                        decision_id: permission_decision_id.clone(),
                        gate: Gate::Permission,
                        effect_request_digest: effect_digest.clone(),
                        subject_digest: ledger.current_subject.digest.clone(),
                        decision: permission_decision.clone(),
                    },
                    error,
                )?;
                ledger.tool_execution(
                    &permission_decision_id,
                    &effect_digest,
                    &proposal.request.capability.digest,
                    &result,
                );
                if let Some(reason) = ledger.record_effect_usage(&result.body.usage) {
                    ledger.effect_results.push(result);
                    return self
                        .finish(ledger, AgentRunStatus::Blocked, reason)
                        .map(|execution| EffectHandling::Terminal(Box::new(execution)));
                }
                ledger.effect_results.push(result);
                return self
                    .finish(
                        ledger,
                        AgentRunStatus::Failed,
                        "protected_effect.unknown_outcome",
                    )
                    .map(|execution| EffectHandling::Terminal(Box::new(execution)));
            }
        };
        if matches!(
            observation.execution_status,
            EffectExecutionStatus::AwaitingAuthority | EffectExecutionStatus::Denied
        ) {
            let decision = executor_non_execution_decision(&observation);
            let _usage_reason = ledger.record_effect_usage(&observation.usage);
            return self
                .decline_protected_effect(
                    ledger,
                    &proposal.request,
                    &effect_digest,
                    Gate::Permission,
                    decision,
                )
                .map(|execution| EffectHandling::Terminal(Box::new(execution)));
        }
        if observation.sandbox_profile.as_ref() != Some(&proposal.request.sandbox_profile) {
            let _usage_reason = ledger.record_effect_usage(&observation.usage);
            return self
                .finish(
                    ledger,
                    AgentRunStatus::Failed,
                    "protected_effect.untrusted_sandbox",
                )
                .map(|execution| EffectHandling::Terminal(Box::new(execution)));
        }
        let effect_result = self.attempt_result(
            &ledger,
            &proposal.request,
            &effect_digest,
            ProtectedEffectDecision {
                decision_id: permission_decision_id.clone(),
                gate: Gate::Permission,
                effect_request_digest: effect_digest.clone(),
                subject_digest: ledger.current_subject.digest.clone(),
                decision: permission_decision,
            },
            observation,
        )?;
        ledger.accept_effect_identity(&proposal.request, &effect_digest);
        ledger.tool_execution(
            &permission_decision_id,
            &effect_digest,
            &proposal.request.capability.digest,
            &effect_result,
        );
        if let Some(post_subject) = effect_result.body.observed_post_effect_subject.clone() {
            if proposal.request.expected_effect_class == EffectClass::Mutation {
                let workflow_decision_id = workflow_decision_id
                    .as_deref()
                    .expect("mutation effects require a workflow decision");
                ledger.mutation(
                    workflow_decision_id,
                    &effect_digest,
                    &post_subject.digest,
                    &effect_result.body.evidence,
                );
                ledger.current_subject = post_subject;
            }
        }
        let usage_reason = ledger.record_effect_usage(&effect_result.body.usage);

        let mut terminal = match effect_result.body.execution_status {
            EffectExecutionStatus::Executed => None,
            EffectExecutionStatus::AwaitingAuthority => Some((
                AgentRunStatus::Blocked,
                "protected_effect.awaiting_authority",
            )),
            EffectExecutionStatus::Denied => {
                Some((AgentRunStatus::Blocked, "protected_effect.denied"))
            }
            EffectExecutionStatus::Failed => {
                Some((AgentRunStatus::Failed, "protected_effect.failed"))
            }
            EffectExecutionStatus::Interrupted => {
                ledger.interruption_from_effect(&effect_result);
                Some((AgentRunStatus::Interrupted, "protected_effect.interrupted"))
            }
            EffectExecutionStatus::UnknownOutcome => {
                Some((AgentRunStatus::Failed, "protected_effect.unknown_outcome"))
            }
        };
        if let Some(reason) = usage_reason {
            if terminal.is_none() {
                terminal = Some((AgentRunStatus::Blocked, reason));
            }
        }
        ledger.effect_results.push(effect_result);

        if let Some((status, reason)) = terminal {
            return self
                .finish(ledger, status, reason)
                .map(|execution| EffectHandling::Terminal(Box::new(execution)));
        }
        Ok(EffectHandling::Continue(Box::new(ledger)))
    }

    fn decline_protected_effect(
        &self,
        mut ledger: Ledger,
        request: &ProtectedEffectRequest,
        effect_digest: &str,
        gate: Gate,
        decision: Decision,
    ) -> Result<AgentRunExecution, RuntimeError> {
        let decision_id = ledger.decision_id("decline", request.effect_sequence);
        ledger.protected_effect_decision(&decision_id, gate, effect_digest, decision.clone());
        let result = self.non_execution_result(
            &ledger,
            request,
            effect_digest,
            NonExecutionOutcome {
                decision: ProtectedEffectDecision {
                    decision_id,
                    gate,
                    effect_request_digest: effect_digest.to_string(),
                    subject_digest: ledger.current_subject.digest.clone(),
                    decision: decision.clone(),
                },
                execution_status: non_execution_status(&decision),
                reason: decision.code.clone(),
                evidence: non_execution_evidence(&decision),
            },
        )?;
        ledger.effect_results.push(result);
        self.blocked_from_decision(ledger, decision)
    }

    fn executor_error_result(
        &self,
        ledger: &Ledger,
        request: &ProtectedEffectRequest,
        effect_digest: &str,
        decision: ProtectedEffectDecision,
        error: RuntimePortError,
    ) -> Result<ProtectedEffectResult, RuntimeError> {
        let body = ProtectedEffectResultBody {
            schema_version: PROTECTED_EFFECT_RESULT_SCHEMA.to_owned(),
            effect_id: request.effect_id.clone(),
            effect_sequence: request.effect_sequence,
            run_id: request.run_id.clone(),
            agent_run_request_digest: request.agent_run_request_digest.clone(),
            effect_request_digest: effect_digest.to_owned(),
            observed_pre_effect_subject: ledger.current_subject.clone(),
            observed_capability: request.capability.clone(),
            observed_tool_schema_digest: request.tool_schema_digest.clone(),
            decision,
            execution_status: EffectExecutionStatus::UnknownOutcome,
            observed_post_effect_subject: None,
            exit: None,
            usage: zero_effect_usage(),
            executor: Some(self.config.executor_identity.clone()),
            sandbox_profile: Some(request.sandbox_profile.clone()),
            reason: Some(format!("executor.failed: {}", error.message())),
            evidence: vec![
                runtime_effect_evidence(EffectEvidenceType::Executor, "executor"),
                runtime_effect_evidence(EffectEvidenceType::Sandbox, "sandbox"),
                runtime_effect_evidence(EffectEvidenceType::Usage, "usage"),
                runtime_effect_evidence(EffectEvidenceType::UnknownOutcome, "unknown-outcome"),
            ],
        };
        seal_protected_effect_result(
            &ledger.request,
            self.support.contract_support(),
            request,
            body,
        )
        .map_err(RuntimeError::ProtectedEffectResult)
    }

    fn verify_and_finish(&mut self, mut ledger: Ledger) -> Result<AgentRunExecution, RuntimeError> {
        ledger.transition(AgentRunStatus::Verifying, None);
        let report_observation = match self.verifier.verify(VerificationContext {
            request: &ledger.request,
            current_subject: &ledger.current_subject,
            usage: &ledger.usage,
        }) {
            Ok(observation) => observation,
            Err(error) => {
                return self.finish(
                    ledger,
                    AgentRunStatus::Failed,
                    format!("verifier.failed: {}", error.message()),
                );
            }
        };
        if let Some(reason) = ledger.record_usage(&report_observation.usage) {
            return self.finish(ledger, AgentRunStatus::Blocked, reason);
        }

        let report = report_observation.value;
        let verification_decision = self.coordinator.evaluate(
            Gate::Verification,
            &verification_input(&self.config, &ledger.current_subject.digest, &report),
        );
        if !self.support.trusts_verifier(&report.verifier_id) {
            return self.finish(
                ledger,
                AgentRunStatus::Blocked,
                "verification.untrusted_verifier",
            );
        }
        let report_is_receipt_safe = verification_report_is_receipt_safe(
            &ledger.request,
            &self.config.implementer_id,
            &report,
        );
        if verification_decision.outcome == Outcome::Allow && !report_is_receipt_safe {
            return self.finish(
                ledger,
                AgentRunStatus::Blocked,
                "verification.invalid_report",
            );
        }
        if report_is_receipt_safe {
            ledger.verification(&self.config.implementer_id, &report);
        }
        if verification_decision.outcome != Outcome::Allow {
            return self.finish(ledger, AgentRunStatus::Blocked, verification_decision.code);
        }
        let completion_decision = self.coordinator.evaluate(
            Gate::Workflow,
            &workflow_completion_input(
                &verification_decision,
                !ledger.mutation_required || ledger.mutation_authorized,
            ),
        );
        let completion_decision_id = "decision-completion".to_string();
        ledger.protected_effect_decision(
            &completion_decision_id,
            Gate::Workflow,
            &ledger.current_subject.digest.clone(),
            completion_decision.clone(),
        );
        if completion_decision.outcome != Outcome::Allow {
            return self.finish(ledger, AgentRunStatus::Blocked, completion_decision.code);
        }
        self.finish(ledger, AgentRunStatus::Completed, completion_decision.code)
    }

    fn non_execution_result(
        &self,
        ledger: &Ledger,
        request: &ProtectedEffectRequest,
        effect_digest: &str,
        outcome: NonExecutionOutcome,
    ) -> Result<ProtectedEffectResult, RuntimeError> {
        let body = ProtectedEffectResultBody {
            schema_version: PROTECTED_EFFECT_RESULT_SCHEMA.to_owned(),
            effect_id: request.effect_id.clone(),
            effect_sequence: request.effect_sequence,
            run_id: request.run_id.clone(),
            agent_run_request_digest: request.agent_run_request_digest.clone(),
            effect_request_digest: effect_digest.to_owned(),
            observed_pre_effect_subject: ledger.current_subject.clone(),
            observed_capability: request.capability.clone(),
            observed_tool_schema_digest: request.tool_schema_digest.clone(),
            decision: outcome.decision,
            execution_status: outcome.execution_status,
            observed_post_effect_subject: None,
            exit: None,
            usage: zero_effect_usage(),
            executor: None,
            sandbox_profile: None,
            reason: Some(outcome.reason),
            evidence: outcome.evidence,
        };
        seal_protected_effect_result(
            &ledger.request,
            self.support.contract_support(),
            request,
            body,
        )
        .map_err(RuntimeError::ProtectedEffectResult)
    }

    fn attempt_result(
        &self,
        ledger: &Ledger,
        request: &ProtectedEffectRequest,
        effect_digest: &str,
        decision: ProtectedEffectDecision,
        observation: ExecutorObservation,
    ) -> Result<ProtectedEffectResult, RuntimeError> {
        let body = ProtectedEffectResultBody {
            schema_version: PROTECTED_EFFECT_RESULT_SCHEMA.to_owned(),
            effect_id: request.effect_id.clone(),
            effect_sequence: request.effect_sequence,
            run_id: request.run_id.clone(),
            agent_run_request_digest: request.agent_run_request_digest.clone(),
            effect_request_digest: effect_digest.to_owned(),
            observed_pre_effect_subject: ledger.current_subject.clone(),
            observed_capability: request.capability.clone(),
            observed_tool_schema_digest: request.tool_schema_digest.clone(),
            decision,
            execution_status: observation.execution_status,
            observed_post_effect_subject: observation.observed_post_effect_subject,
            exit: observation.exit,
            usage: observation.usage,
            executor: Some(self.config.executor_identity.clone()),
            sandbox_profile: observation.sandbox_profile,
            reason: observation.reason,
            evidence: observation.evidence,
        };
        seal_protected_effect_result(
            &ledger.request,
            self.support.contract_support(),
            request,
            body,
        )
        .map_err(RuntimeError::ProtectedEffectResult)
    }

    fn blocked_from_decision(
        &self,
        mut ledger: Ledger,
        decision: Decision,
    ) -> Result<AgentRunExecution, RuntimeError> {
        if decision.outcome == Outcome::Ask {
            ledger.transition(
                AgentRunStatus::AwaitingAuthority,
                Some(decision.code.clone()),
            );
        }
        self.finish(ledger, AgentRunStatus::Blocked, decision.code)
    }

    fn finish(
        &self,
        mut ledger: Ledger,
        terminal_status: AgentRunStatus,
        reason: impl Into<String>,
    ) -> Result<AgentRunExecution, RuntimeError> {
        let reason = reason.into();
        ledger.usage_event();
        ledger.transition(terminal_status, Some(reason.clone()));
        let request = ledger.request.clone();
        let protected_effect_results = std::mem::take(&mut ledger.effect_results);
        let body = ledger.into_receipt_body(terminal_status, reason);
        let receipt = seal_terminal_receipt(&request, self.support.contract_support(), body)
            .map_err(RuntimeError::Receipt)?;
        Ok(AgentRunExecution {
            receipt,
            protected_effect_results,
        })
    }
}

struct Ledger {
    request: AgentRunRequest,
    request_digest: String,
    current_subject: crate::contracts::Subject,
    status: AgentRunStatus,
    usage: ResourceUsage,
    events: Vec<RunEvent>,
    effect_results: Vec<ProtectedEffectResult>,
    effects_seen: u64,
    effect_ids: BTreeSet<String>,
    effect_request_digests: BTreeSet<String>,
    idempotency_keys: BTreeSet<String>,
    mutation_required: bool,
    mutation_authorized: bool,
}

impl Ledger {
    fn new(request: AgentRunRequest, request_digest: String) -> Self {
        Self {
            current_subject: request.subject.clone(),
            request,
            request_digest,
            status: AgentRunStatus::Accepted,
            usage: zero_resource_usage(),
            events: Vec::new(),
            effect_results: Vec::new(),
            effects_seen: 0,
            effect_ids: BTreeSet::new(),
            effect_request_digests: BTreeSet::new(),
            idempotency_keys: BTreeSet::new(),
            mutation_required: false,
            mutation_authorized: false,
        }
    }

    fn next_sequence(&self) -> u64 {
        self.events.len() as u64 + 1
    }

    fn decision_id(&self, prefix: &str, effect_sequence: u64) -> String {
        format!(
            "decision-{effect_sequence}-{prefix}-event-{}",
            self.next_sequence()
        )
    }

    fn admission_decision(
        &self,
        request: &ProtectedEffectRequest,
        effect_digest: &str,
        max_effects: u64,
    ) -> Option<Decision> {
        let expected_sequence = self.effects_seen + 1;
        if request.effect_sequence != expected_sequence {
            return Some(Decision::block(
                "runtime.effect_sequence_invalid",
                &["stop_execution", "record_declined_effect"],
            ));
        }
        if self.effects_seen >= max_effects {
            return Some(Decision::block(
                "runtime.max_effects_exhausted",
                &["stop_execution", "record_declined_effect"],
            ));
        }
        if self.effect_ids.contains(&request.effect_id)
            || self.effect_request_digests.contains(effect_digest)
            || self.idempotency_keys.contains(&request.idempotency_key)
        {
            return Some(Decision::block(
                "runtime.duplicate_effect",
                &["stop_execution", "record_declined_effect"],
            ));
        }
        None
    }

    fn accept_effect_identity(&mut self, request: &ProtectedEffectRequest, effect_digest: &str) {
        self.effects_seen += 1;
        self.effect_ids.insert(request.effect_id.clone());
        self.effect_request_digests
            .insert(effect_digest.to_string());
        self.idempotency_keys
            .insert(request.idempotency_key.clone());
        if request.expected_effect_class == EffectClass::Mutation {
            self.mutation_required = true;
        }
    }

    fn transition(&mut self, to: AgentRunStatus, reason: Option<String>) {
        let from = self.status;
        self.events.push(RunEvent::StatusTransition {
            sequence: self.next_sequence(),
            from,
            to,
            reason,
        });
        self.status = to;
    }

    fn plan_recorded(&mut self, plan_digest: String) {
        self.events.push(RunEvent::PlanRecorded {
            sequence: self.next_sequence(),
            plan_digest,
        });
    }

    fn approval_recorded(&mut self, approval: ApprovalReference) {
        self.events.push(RunEvent::ApprovalRecorded {
            sequence: self.next_sequence(),
            approval,
        });
    }

    fn protected_effect_decision(
        &mut self,
        decision_id: &str,
        gate: Gate,
        protected_effect_digest: &str,
        decision: Decision,
    ) {
        let mutation_authorized = gate == Gate::Workflow
            && decision.outcome == Outcome::Allow
            && decision.code == "workflow.mutation_authorized";
        self.events.push(RunEvent::ProtectedEffectDecision {
            sequence: self.next_sequence(),
            decision_id: decision_id.to_string(),
            gate,
            protected_effect_digest: protected_effect_digest.to_string(),
            subject_digest: self.current_subject.digest.clone(),
            decision,
        });
        if mutation_authorized {
            self.mutation_authorized = true;
        }
    }

    fn tool_execution(
        &mut self,
        decision_id: &str,
        protected_effect_digest: &str,
        capability_digest: &str,
        result: &ProtectedEffectResult,
    ) {
        self.events.push(RunEvent::ToolExecution {
            sequence: self.next_sequence(),
            decision_id: decision_id.to_string(),
            protected_effect_digest: protected_effect_digest.to_string(),
            action_digest: protected_effect_digest.to_string(),
            capability_digest: capability_digest.to_string(),
            evidence: vec![EvidenceReference {
                evidence_type: EvidenceType::ToolExecution,
                digest: result.result_digest.clone(),
                locator: Some(format!(
                    "gaap:protected-effect-result/{}",
                    result.body.effect_id
                )),
            }],
        });
    }

    fn mutation(
        &mut self,
        decision_id: &str,
        protected_effect_digest: &str,
        after_subject_digest: &str,
        effect_evidence: &[EffectEvidenceReference],
    ) {
        self.events.push(RunEvent::Mutation {
            sequence: self.next_sequence(),
            decision_id: decision_id.to_string(),
            protected_effect_digest: protected_effect_digest.to_string(),
            before_subject_digest: self.current_subject.digest.clone(),
            after_subject_digest: after_subject_digest.to_string(),
            evidence: vec![artifact_evidence(effect_evidence)],
        });
    }

    fn record_usage(&mut self, usage: &ResourceUsage) -> Option<&'static str> {
        let Some(next_usage) = project_resource_usage(&self.usage, usage) else {
            self.usage = capped_resource_usage(&self.usage, usage);
            self.usage_event();
            return Some("runtime.usage_overflow");
        };
        self.usage = next_usage;
        self.usage_event();
        if usage_exceeds_budget(&self.usage, &self.request.resource_budget) {
            Some("runtime.budget_exceeded")
        } else {
            None
        }
    }

    fn record_effect_usage(&mut self, usage: &EffectUsage) -> Option<&'static str> {
        self.record_usage(&resource_usage_from_effect(usage))
    }

    fn usage_event(&mut self) {
        if latest_usage(&self.events).as_ref() == Some(&self.usage) {
            return;
        }
        self.events.push(RunEvent::Usage {
            sequence: self.next_sequence(),
            usage: self.usage.clone(),
        });
    }

    fn verification(&mut self, implementer_id: &str, report: &VerificationReport) {
        self.events.push(RunEvent::Verification {
            sequence: self.next_sequence(),
            subject_digest: report.subject_digest.clone(),
            implementer_id: implementer_id.to_string(),
            verifier_id: report.verifier_id.clone(),
            verdict: report.verdict,
            evidence: report.evidence.clone(),
        });
    }

    fn interruption(&mut self, cancellation: Cancellation) {
        self.events.push(RunEvent::Interruption {
            sequence: self.next_sequence(),
            actor_id: cancellation.actor_id,
            reason: cancellation.reason,
            evidence: cancellation.evidence,
        });
    }

    fn interruption_from_effect(&mut self, result: &ProtectedEffectResult) {
        self.events.push(RunEvent::Interruption {
            sequence: self.next_sequence(),
            actor_id: result
                .body
                .executor
                .as_ref()
                .map(|executor| executor.name.clone()),
            reason: result
                .body
                .reason
                .clone()
                .unwrap_or_else(|| "protected_effect.interrupted".to_string()),
            evidence: interruption_evidence(&result.body.evidence),
        });
    }

    fn into_receipt_body(
        self,
        terminal_status: AgentRunStatus,
        terminal_reason: String,
    ) -> TerminalRunReceiptBody {
        let AgentRunRequest {
            request_id,
            run_id,
            subject,
            ..
        } = self.request;
        TerminalRunReceiptBody {
            schema_version: TERMINAL_RUN_RECEIPT_SCHEMA.to_owned(),
            request_id,
            run_id,
            request_digest: self.request_digest,
            initial_subject_digest: subject.digest,
            resulting_subject_digest: self.current_subject.digest,
            terminal_status,
            terminal_reason,
            usage: self.usage,
            events: self.events,
        }
    }
}

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

fn validate_config(config: &RuntimeConfig, request: &AgentRunRequest) -> Result<(), RuntimeError> {
    if config.implementer_id.trim().is_empty() {
        return Err(RuntimeError::InvalidConfig(
            "implementer_id must not be empty".to_string(),
        ));
    }
    if config.executor_identity.name.trim().is_empty()
        || config.executor_identity.version.trim().is_empty()
        || !is_digest_string(&config.executor_identity.digest)
    {
        return Err(RuntimeError::InvalidConfig(
            "executor_identity must be a valid executor identity".to_string(),
        ));
    }
    if config.max_effects == 0 || config.max_effects > MAX_SAFE_INTEGER {
        return Err(RuntimeError::InvalidConfig(
            "max_effects must be between 1 and the JCS safe integer limit".to_string(),
        ));
    }
    let thresholds = &config.budget_thresholds;
    if thresholds.warn_micros > MAX_SAFE_INTEGER
        || thresholds.approval_micros > MAX_SAFE_INTEGER
        || thresholds.hard_stop_micros > MAX_SAFE_INTEGER
    {
        return Err(RuntimeError::InvalidConfig(
            "budget thresholds must not exceed the JCS safe integer limit".to_string(),
        ));
    }
    if thresholds.warn_micros > thresholds.approval_micros
        || thresholds.approval_micros > thresholds.hard_stop_micros
    {
        return Err(RuntimeError::InvalidConfig(
            "budget thresholds must be ordered warn <= approval <= hard_stop".to_string(),
        ));
    }
    if thresholds.hard_stop_micros > request.resource_budget.max_cost_micros {
        return Err(RuntimeError::InvalidConfig(
            "hard_stop_micros must not exceed the request cost budget".to_string(),
        ));
    }
    Ok(())
}

fn validate_support(support: &RuntimeSupport) -> Result<(), RuntimeError> {
    for (index, approval) in support.runtime_approvals.iter().enumerate() {
        if !is_digest_string(&approval.subject_digest)
            || approval
                .max_cost_micros
                .is_some_and(|maximum| maximum > MAX_SAFE_INTEGER)
        {
            return Err(RuntimeError::InvalidConfig(format!(
                "runtime_approvals[{index}] must bind a valid digest and safe maximum cost"
            )));
        }
    }
    for (index, approval) in support.permission_policy.approvals.iter().enumerate() {
        if !is_digest_string(&approval.subject_digest) {
            return Err(RuntimeError::InvalidConfig(format!(
                "permission_policy.approvals[{index}] must bind a valid digest"
            )));
        }
    }
    if support
        .trusted_verifier_ids
        .iter()
        .any(|verifier| verifier.trim().is_empty())
    {
        return Err(RuntimeError::InvalidConfig(
            "trusted_verifier_ids must not contain empty verifier IDs".to_string(),
        ));
    }
    if !effect_usage_is_safe(&support.protected_effect_usage_estimate) {
        return Err(RuntimeError::InvalidConfig(
            "protected_effect_usage_estimate must stay within JCS safe integer bounds".to_string(),
        ));
    }
    Ok(())
}

fn plan_input(subject: &crate::contracts::Subject, approval: Option<&ApprovalReference>) -> Value {
    json!({
        "subject_digest": subject.digest,
        "approval": approval
            .map(approval_reference_value)
            .unwrap_or(Value::Null),
    })
}

fn permission_input(action_digest: &str, permission: &PermissionPolicy) -> Value {
    let policy_decision = permission
        .policy_decision
        .map(|decision| Value::String(decision.as_str().to_string()))
        .unwrap_or(Value::Null);

    json!({
        "action_digest": action_digest,
        "policy_decision": policy_decision,
        "risk_tags": permission.risk_tags,
        "wrapper_chain": permission.wrapper_chain,
        "approval": permission
            .approval_for(action_digest)
            .map(gate_approval_value)
            .unwrap_or(Value::Null),
    })
}

fn tool_trust_input(support: &RuntimeSupport, capability: &CapabilityIdentity) -> Value {
    let approval = if support.trusts_capability(capability) {
        json!({
            "status": "approved",
            "subject_digest": capability.digest,
        })
    } else {
        Value::Null
    };

    json!({
        "capability_name": capability.name,
        "capability_digest": capability.digest,
        "approval": approval,
    })
}

fn runtime_input(
    support: &RuntimeSupport,
    config: &RuntimeConfig,
    ledger: &Ledger,
    effect_digest: &str,
) -> Value {
    let estimate = support.protected_effect_usage_estimate();
    json!({
        "action_digest": effect_digest,
        "usage_status": "known",
        "current_cost_micros": ledger.usage.cost_micros,
        "estimated_next_cost_micros": estimate.cost_micros,
        "thresholds": {
            "warn_micros": config.budget_thresholds.warn_micros,
            "approval_micros": config.budget_thresholds.approval_micros,
            "hard_stop_micros": config.budget_thresholds.hard_stop_micros,
        },
        "approval": support
            .runtime_approval_for(effect_digest)
            .map(gate_approval_value)
            .unwrap_or(Value::Null),
    })
}

fn workflow_mutation_input(
    permission: &Decision,
    tool_trust: &Decision,
    runtime: &Decision,
) -> Value {
    json!({
        "boundary": "mutation",
        "gate_results": {
            "plan": "allow",
            "permission": outcome_name(&permission.outcome),
            "tool_trust": outcome_name(&tool_trust.outcome),
            "runtime": outcome_name(&runtime.outcome),
        }
    })
}

fn workflow_completion_input(verification: &Decision, mutation_authorized: bool) -> Value {
    json!({
        "boundary": "completion",
        "mutation_authorized": mutation_authorized,
        "gate_results": {
            "verification": outcome_name(&verification.outcome),
        }
    })
}

fn verification_input(
    config: &RuntimeConfig,
    subject_digest: &str,
    report: &VerificationReport,
) -> Value {
    let verdict = match report.verdict {
        VerificationVerdict::Pass => "PASS",
        VerificationVerdict::Fail => "FAIL",
    };
    let evidence = report
        .evidence
        .iter()
        .enumerate()
        .map(|(index, item)| {
            json!({
                "command": format!("verification-evidence-{index}"),
                "output": item.digest,
                "result": "PASS",
            })
        })
        .collect::<Vec<_>>();

    json!({
        "subject_digest": subject_digest,
        "implementer_id": config.implementer_id,
        "report": {
            "verifier_id": report.verifier_id,
            "subject_digest": report.subject_digest,
            "verdict": verdict,
            "evidence": evidence,
        }
    })
}

fn approval_reference_value(approval: &ApprovalReference) -> Value {
    json!({
        "status": "approved",
        "subject_digest": approval.subject_digest,
    })
}

fn gate_approval_value(approval: &GateApproval) -> Value {
    let mut value = json!({
        "status": "approved",
        "subject_digest": approval.subject_digest,
    });
    if let Some(maximum) = approval.max_cost_micros {
        value["max_cost_micros"] = json!(maximum);
    }
    value
}

fn outcome_name(outcome: &Outcome) -> &'static str {
    match outcome {
        Outcome::Allow => "allow",
        Outcome::Ask => "ask",
        Outcome::Block => "block",
    }
}

fn non_execution_status(decision: &Decision) -> EffectExecutionStatus {
    match decision.outcome {
        Outcome::Ask => EffectExecutionStatus::AwaitingAuthority,
        Outcome::Allow | Outcome::Block => EffectExecutionStatus::Denied,
    }
}

fn plan_approval_for<'a>(
    request: &'a AgentRunRequest,
    subject_digest: &str,
) -> Option<&'a ApprovalReference> {
    request
        .approval_context
        .iter()
        .find(|approval| approval.subject_digest == subject_digest)
}

fn sanitize_cancellation(cancellation: Cancellation) -> Cancellation {
    let reason = if cancellation.reason.trim().is_empty() {
        "runtime.interrupted".to_string()
    } else {
        cancellation.reason
    };
    let actor_id = cancellation
        .actor_id
        .filter(|actor| !actor.trim().is_empty());
    let evidence = if evidence_reference_is_valid(&cancellation.evidence)
        && cancellation.evidence.evidence_type == EvidenceType::Interruption
    {
        cancellation.evidence
    } else {
        EvidenceReference {
            evidence_type: EvidenceType::Interruption,
            digest: digest_bytes(format!("gaap:runtime/interruption:{reason}").as_bytes()),
            locator: Some("gaap:runtime/interruption".to_string()),
        }
    };

    Cancellation {
        actor_id,
        reason,
        evidence,
    }
}

fn budget_admission_decision(
    current: &ResourceUsage,
    budget: &crate::contracts::ResourceBudget,
    estimate: &EffectUsage,
) -> Option<Decision> {
    let estimate = resource_usage_from_effect(estimate);
    let Some(projected) = project_resource_usage(current, &estimate) else {
        return Some(Decision::block(
            "runtime.invalid_budget",
            &["stop_execution", "request_usage_reconciliation"],
        ));
    };
    if projected.elapsed_ms > budget.max_elapsed_ms
        || projected.model_tokens > budget.max_model_tokens
        || projected.tool_calls > budget.max_tool_calls
    {
        return Some(Decision::block(
            "runtime.budget_exceeded",
            &["stop_execution", "request_usage_reconciliation"],
        ));
    }
    None
}

fn executor_non_execution_decision(observation: &ExecutorObservation) -> Decision {
    let code = match observation.execution_status {
        EffectExecutionStatus::AwaitingAuthority => "executor.awaiting_authority",
        EffectExecutionStatus::Denied => "executor.denied",
        _ => "executor.non_execution",
    };
    let mut decision = match observation.execution_status {
        EffectExecutionStatus::AwaitingAuthority => {
            Decision::ask(code, &["request_executor_authority"])
        }
        _ => Decision::block(code, &["stop_execution"]),
    };
    if let Some(reason) = observation
        .reason
        .as_ref()
        .filter(|reason| !reason.trim().is_empty())
    {
        decision.effects.push(reason.clone());
    }
    decision
}

fn non_execution_evidence(decision: &Decision) -> Vec<EffectEvidenceReference> {
    if decision.code == "protected_effect.stale_subject" {
        vec![runtime_effect_evidence(
            EffectEvidenceType::SubjectObservation,
            "current-subject",
        )]
    } else {
        vec![]
    }
}

fn runtime_effect_evidence(
    evidence_type: EffectEvidenceType,
    label: &str,
) -> EffectEvidenceReference {
    EffectEvidenceReference {
        evidence_type,
        digest: digest_bytes(format!("gaap:runtime/effect-evidence:{label}").as_bytes()),
        locator: Some(format!("gaap:runtime/{label}")),
    }
}

fn verification_report_is_receipt_safe(
    request: &AgentRunRequest,
    implementer_id: &str,
    report: &VerificationReport,
) -> bool {
    is_digest_string(&report.subject_digest)
        && !implementer_id.trim().is_empty()
        && !report.verifier_id.trim().is_empty()
        && evidence_set_is_valid(&report.evidence)
        && (report.verdict != VerificationVerdict::Pass
            || (report.verifier_id != implementer_id
                && request
                    .required_verification
                    .evidence_types
                    .iter()
                    .all(|required| {
                        report
                            .evidence
                            .iter()
                            .any(|evidence| evidence.evidence_type == *required)
                    })))
}

fn evidence_set_is_valid(evidence: &[EvidenceReference]) -> bool {
    !evidence.is_empty() && evidence.iter().all(evidence_reference_is_valid)
}

fn evidence_reference_is_valid(evidence: &EvidenceReference) -> bool {
    is_digest_string(&evidence.digest)
        && evidence
            .locator
            .as_ref()
            .is_none_or(|locator| !locator.trim().is_empty())
}

fn resource_usage_from_effect(usage: &EffectUsage) -> ResourceUsage {
    ResourceUsage {
        cost_micros: usage.cost_micros,
        elapsed_ms: usage.elapsed_ms,
        model_tokens: usage.model_tokens,
        tool_calls: usage.tool_calls,
    }
}

fn effect_usage_is_safe(usage: &EffectUsage) -> bool {
    usage.cost_micros <= MAX_SAFE_INTEGER
        && usage.elapsed_ms <= MAX_SAFE_INTEGER
        && usage.model_tokens <= MAX_SAFE_INTEGER
        && usage.tool_calls <= MAX_SAFE_INTEGER
}

fn project_resource_usage(current: &ResourceUsage, usage: &ResourceUsage) -> Option<ResourceUsage> {
    Some(ResourceUsage {
        cost_micros: checked_usage_sum(current.cost_micros, usage.cost_micros)?,
        elapsed_ms: checked_usage_sum(current.elapsed_ms, usage.elapsed_ms)?,
        model_tokens: checked_usage_sum(current.model_tokens, usage.model_tokens)?,
        tool_calls: checked_usage_sum(current.tool_calls, usage.tool_calls)?,
    })
}

fn capped_resource_usage(current: &ResourceUsage, usage: &ResourceUsage) -> ResourceUsage {
    ResourceUsage {
        cost_micros: capped_usage_sum(current.cost_micros, usage.cost_micros),
        elapsed_ms: capped_usage_sum(current.elapsed_ms, usage.elapsed_ms),
        model_tokens: capped_usage_sum(current.model_tokens, usage.model_tokens),
        tool_calls: capped_usage_sum(current.tool_calls, usage.tool_calls),
    }
}

fn checked_usage_sum(current: u64, usage: u64) -> Option<u64> {
    current
        .checked_add(usage)
        .filter(|total| *total <= MAX_SAFE_INTEGER)
}

fn capped_usage_sum(current: u64, usage: u64) -> u64 {
    current.saturating_add(usage).min(MAX_SAFE_INTEGER)
}

fn is_digest_string(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn zero_effect_usage() -> EffectUsage {
    EffectUsage {
        cost_micros: 0,
        elapsed_ms: 0,
        model_tokens: 0,
        tool_calls: 0,
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

fn latest_usage(events: &[RunEvent]) -> Option<ResourceUsage> {
    events.iter().rev().find_map(|event| match event {
        RunEvent::Usage { usage, .. } => Some(usage.clone()),
        _ => None,
    })
}

fn usage_exceeds_budget(usage: &ResourceUsage, budget: &crate::contracts::ResourceBudget) -> bool {
    usage.cost_micros > budget.max_cost_micros
        || usage.elapsed_ms > budget.max_elapsed_ms
        || usage.model_tokens > budget.max_model_tokens
        || usage.tool_calls > budget.max_tool_calls
}

fn artifact_evidence(effect_evidence: &[EffectEvidenceReference]) -> EvidenceReference {
    effect_evidence
        .iter()
        .find(|reference| reference.evidence_type == EffectEvidenceType::Artifact)
        .map(|reference| EvidenceReference {
            evidence_type: EvidenceType::Artifact,
            digest: reference.digest.clone(),
            locator: reference.locator.clone(),
        })
        .unwrap_or_else(|| EvidenceReference {
            evidence_type: EvidenceType::Artifact,
            digest: digest_bytes(b"gaap:runtime/missing-artifact-evidence"),
            locator: Some("gaap:runtime/missing-artifact-evidence".to_string()),
        })
}

fn interruption_evidence(effect_evidence: &[EffectEvidenceReference]) -> EvidenceReference {
    effect_evidence
        .iter()
        .find(|reference| reference.evidence_type == EffectEvidenceType::Interruption)
        .map(|reference| EvidenceReference {
            evidence_type: EvidenceType::Interruption,
            digest: reference.digest.clone(),
            locator: reference.locator.clone(),
        })
        .unwrap_or_else(|| EvidenceReference {
            evidence_type: EvidenceType::Interruption,
            digest: digest_bytes(b"gaap:runtime/interrupted-effect"),
            locator: Some("gaap:runtime/interrupted-effect".to_string()),
        })
}

fn digest_bytes(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
