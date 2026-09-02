use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};

use super::canonical::{canonical_receipt_body_bytes, canonical_request_bytes, sha256_digest};
use super::model::{
    AGENT_RUN_REQUEST_SCHEMA, AgentRunRequest, AgentRunStatus, ApprovalReference,
    EvidenceReference, EvidenceType, PolicyIdentity, ResourceBudget, ResourceUsage, RunEvent,
    TERMINAL_RUN_RECEIPT_SCHEMA, TerminalRunReceipt, TerminalRunReceiptBody, VerificationVerdict,
};
use crate::{Gate, Outcome};

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// Stable machine-readable categories returned by contract validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContractErrorCode {
    InvalidJson,
    UnsupportedSchema,
    UnknownPolicy,
    InvalidDigest,
    InvalidContract,
    InvalidTransition,
    UnauthorizedEffect,
    StaleVerification,
    RequestMismatch,
    ReceiptTampering,
}

/// Contract validation failure with a stable code and document path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractError {
    code: ContractErrorCode,
    path: String,
    message: String,
}

impl ContractError {
    pub(crate) fn new(
        code: ContractErrorCode,
        path: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            path: path.into(),
            message: message.into(),
        }
    }

    /// Return the stable machine-readable error category.
    pub fn code(&self) -> ContractErrorCode {
        self.code
    }

    /// Return the contract path that failed validation.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Return the human-readable explanation of the failure.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl Display for ContractError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.path, self.message)
    }
}

impl Error for ContractError {}

/// Exact policy identities that a caller is prepared to evaluate.
#[derive(Debug, Clone, Default)]
pub struct ContractSupport {
    policies: BTreeSet<PolicyIdentity>,
}

impl ContractSupport {
    /// Construct support from exact name, version, and digest tuples.
    pub fn new(policies: impl IntoIterator<Item = PolicyIdentity>) -> Self {
        Self {
            policies: policies.into_iter().collect(),
        }
    }

    /// Return whether an exact policy identity is supported.
    pub fn supports(&self, policy: &PolicyIdentity) -> bool {
        self.policies.contains(policy)
    }
}

/// Strictly parse an Agent Run Request from UTF-8 JSON bytes.
pub fn parse_agent_run_request_json(bytes: &[u8]) -> Result<AgentRunRequest, ContractError> {
    serde_json::from_slice(bytes).map_err(|error| {
        ContractError::new(
            ContractErrorCode::InvalidJson,
            "$",
            format!("invalid Agent Run Request JSON: {error}"),
        )
    })
}

/// Strictly parse a Terminal Run Receipt from UTF-8 JSON bytes.
pub fn parse_terminal_run_receipt_json(bytes: &[u8]) -> Result<TerminalRunReceipt, ContractError> {
    serde_json::from_slice(bytes).map_err(|error| {
        ContractError::new(
            ContractErrorCode::InvalidJson,
            "$",
            format!("invalid Terminal Run Receipt JSON: {error}"),
        )
    })
}

/// Validate request semantics and return its canonical SHA-256 digest.
pub fn validate_request(
    request: &AgentRunRequest,
    support: &ContractSupport,
) -> Result<String, ContractError> {
    if request.schema_version != AGENT_RUN_REQUEST_SCHEMA {
        return Err(ContractError::new(
            ContractErrorCode::UnsupportedSchema,
            "schema_version",
            format!(
                "unsupported Agent Run Request schema: {}",
                request.schema_version
            ),
        ));
    }
    validate_non_empty(&request.request_id, "request_id")?;
    validate_non_empty(&request.run_id, "run_id")?;
    validate_non_empty(&request.subject.locator, "subject.locator")?;
    validate_digest(&request.subject.digest, "subject.digest")?;
    validate_non_empty(
        &request.requested_capability.name,
        "requested_capability.name",
    )?;
    validate_non_empty(
        &request.requested_capability.version,
        "requested_capability.version",
    )?;
    validate_digest(
        &request.requested_capability.digest,
        "requested_capability.digest",
    )?;
    validate_non_empty(&request.task.instructions, "task.instructions")?;
    for (index, constraint) in request.task.constraints.iter().enumerate() {
        validate_non_empty(constraint, &format!("task.constraints[{index}]"))?;
    }

    if request.policies.is_empty() {
        return Err(ContractError::new(
            ContractErrorCode::InvalidContract,
            "policies",
            "at least one policy identity is required",
        ));
    }
    let mut identities = BTreeSet::new();
    for (index, policy) in request.policies.iter().enumerate() {
        let path = format!("policies[{index}]");
        validate_non_empty(&policy.name, &format!("{path}.name"))?;
        validate_non_empty(&policy.version, &format!("{path}.version"))?;
        validate_digest(&policy.digest, &format!("{path}.digest"))?;
        if !identities.insert(policy) {
            return Err(ContractError::new(
                ContractErrorCode::InvalidContract,
                path,
                "duplicate policy identity",
            ));
        }
        if !support.supports(policy) {
            return Err(ContractError::new(
                ContractErrorCode::UnknownPolicy,
                path,
                format!(
                    "unsupported policy identity: {}@{} ({})",
                    policy.name, policy.version, policy.digest
                ),
            ));
        }
    }

    validate_safe_integer(
        request.resource_budget.max_cost_micros,
        "resource_budget.max_cost_micros",
    )?;
    validate_safe_integer(
        request.resource_budget.max_elapsed_ms,
        "resource_budget.max_elapsed_ms",
    )?;
    validate_safe_integer(
        request.resource_budget.max_model_tokens,
        "resource_budget.max_model_tokens",
    )?;
    validate_safe_integer(
        request.resource_budget.max_tool_calls,
        "resource_budget.max_tool_calls",
    )?;

    for (index, approval) in request.approval_context.iter().enumerate() {
        let path = format!("approval_context[{index}]");
        validate_non_empty(&approval.approval_id, &format!("{path}.approval_id"))?;
        validate_non_empty(&approval.actor_id, &format!("{path}.actor_id"))?;
        validate_non_empty(&approval.scope, &format!("{path}.scope"))?;
        validate_digest(&approval.subject_digest, &format!("{path}.subject_digest"))?;
      if approval.subject_digest != request.subject.digest {
          return Err(ContractError::new(
              ContractErrorCode::RequestMismatch,
              format!("{path}.subject_digest"),
              "approval context subject does not match the request subject",
          ));
      }
        if approval.evidence.evidence_type != EvidenceType::Approval {
            return Err(ContractError::new(
                ContractErrorCode::InvalidContract,
                format!("{path}.evidence.evidence_type"),
                "approval context must reference approval evidence",
            ));
        }
        validate_evidence(&approval.evidence, &format!("{path}.evidence"))?;
    }

    if request.required_verification.evidence_types.is_empty() {
        return Err(ContractError::new(
            ContractErrorCode::InvalidContract,
            "required_verification.evidence_types",
            "at least one verification evidence type is required",
        ));
    }
    for (index, evidence_type) in request
        .required_verification
        .evidence_types
        .iter()
        .enumerate()
    {
        if !matches!(
            evidence_type,
            EvidenceType::CommandOutput
                | EvidenceType::Artifact
                | EvidenceType::VerificationAttestation
        ) {
            return Err(ContractError::new(
                ContractErrorCode::InvalidContract,
                format!("required_verification.evidence_types[{index}]"),
                "evidence type cannot satisfy independent verification",
            ));
        }
    }

    canonical_request_bytes(request).map(|bytes| sha256_digest(&bytes))
}

/// Validate a terminal body and return its content-addressed receipt envelope.
pub fn seal_terminal_receipt(
    request: &AgentRunRequest,
    support: &ContractSupport,
    body: TerminalRunReceiptBody,
) -> Result<TerminalRunReceipt, ContractError> {
    validate_receipt_body(request, support, &body)?;
    let receipt_digest = sha256_digest(&canonical_receipt_body_bytes(&body)?);
    Ok(TerminalRunReceipt {
        receipt_digest,
        body,
    })
}

/// Verify a receipt's digest and semantic binding to an Agent Run Request.
pub fn verify_terminal_receipt(
    request: &AgentRunRequest,
    support: &ContractSupport,
    receipt: &TerminalRunReceipt,
) -> Result<(), ContractError> {
    validate_digest(&receipt.receipt_digest, "receipt_digest").map_err(|_| {
        ContractError::new(
            ContractErrorCode::ReceiptTampering,
            "receipt_digest",
            "receipt digest is malformed",
        )
    })?;
    let actual_digest = sha256_digest(&canonical_receipt_body_bytes(&receipt.body)?);
    if actual_digest != receipt.receipt_digest {
        return Err(ContractError::new(
            ContractErrorCode::ReceiptTampering,
            "receipt_digest",
            format!(
                "receipt digest mismatch: expected {}, calculated {actual_digest}",
                receipt.receipt_digest
            ),
        ));
    }
    validate_receipt_body(request, support, &receipt.body)
}

#[derive(Debug, Clone)]
struct RecordedDecision {
    sequence: u64,
    gate: Gate,
    protected_effect_digest: String,
    subject_digest: String,
    outcome: Outcome,
    code: String,
}

fn validate_receipt_body(
    request: &AgentRunRequest,
    support: &ContractSupport,
    body: &TerminalRunReceiptBody,
) -> Result<(), ContractError> {
    let expected_request_digest = validate_request(request, support)?;
    if body.schema_version != TERMINAL_RUN_RECEIPT_SCHEMA {
        return Err(ContractError::new(
            ContractErrorCode::UnsupportedSchema,
            "body.schema_version",
            format!(
                "unsupported Terminal Run Receipt schema: {}",
                body.schema_version
            ),
        ));
    }
    if body.request_id != request.request_id {
        return Err(request_mismatch("body.request_id"));
    }
    if body.run_id != request.run_id {
        return Err(request_mismatch("body.run_id"));
    }
    if body.request_digest != expected_request_digest {
        return Err(request_mismatch("body.request_digest"));
    }
    if body.initial_subject_digest != request.subject.digest {
        return Err(request_mismatch("body.initial_subject_digest"));
    }
    validate_digest(
        &body.resulting_subject_digest,
        "body.resulting_subject_digest",
    )?;
    if !body.terminal_status.is_terminal() {
        return Err(ContractError::new(
            ContractErrorCode::InvalidContract,
            "body.terminal_status",
            "receipt status must be terminal",
        ));
    }
    validate_non_empty(&body.terminal_reason, "body.terminal_reason")?;
    validate_usage(&body.usage, "body.usage")?;
    if body.events.is_empty() {
        return Err(ContractError::new(
            ContractErrorCode::InvalidContract,
            "events",
            "terminal receipt must contain at least one event",
        ));
    }

    let mut status = AgentRunStatus::Accepted;
    let mut current_subject = body.initial_subject_digest.clone();
    let mut last_mutation_sequence = 0;
    let mut latest_passing_verification: Option<(u64, String)> = None;
    let mut latest_usage: Option<ResourceUsage> = None;
    let mut completion_decision: Option<RecordedDecision> = None;
    let mut decisions = BTreeMap::<String, RecordedDecision>::new();
    let mut interruption_seen = false;

    for (index, event) in body.events.iter().enumerate() {
        let path = format!("events[{index}]");
        let expected_sequence = u64::try_from(index + 1).map_err(|_| {
            ContractError::new(
                ContractErrorCode::InvalidContract,
                &path,
                "event sequence exceeds the supported integer range",
            )
        })?;
        if event.sequence() != expected_sequence {
            return Err(ContractError::new(
                ContractErrorCode::InvalidContract,
                format!("{path}.sequence"),
                format!("event sequence must be contiguous from 1; expected {expected_sequence}"),
            ));
        }
        validate_safe_integer(event.sequence(), &format!("{path}.sequence"))?;

        match event {
            RunEvent::StatusTransition {
                from, to, reason, ..
            } => {
                if *from != status || !valid_transition(*from, *to) {
                    return Err(ContractError::new(
                        ContractErrorCode::InvalidTransition,
                        path,
                        format!("invalid Agent Run transition from {from:?} to {to:?}"),
                    ));
                }
                if let Some(reason) = reason {
                    validate_non_empty(reason, &format!("{path}.reason"))?;
                }
                status = *to;
            }
            RunEvent::PlanRecorded { plan_digest, .. } => {
                validate_digest(plan_digest, &format!("{path}.plan_digest"))?;
            }
            RunEvent::ApprovalRecorded { approval, .. } => {
                validate_approval(approval, &path)?;
            }
            RunEvent::ProtectedEffectDecision {
                sequence,
                decision_id,
                gate,
                protected_effect_digest,
                subject_digest,
                decision,
            } => {
                validate_non_empty(decision_id, &format!("{path}.decision_id"))?;
                validate_digest(
                    protected_effect_digest,
                    &format!("{path}.protected_effect_digest"),
                )?;
                validate_digest(subject_digest, &format!("{path}.subject_digest"))?;
                if subject_digest != &current_subject {
                    return Err(ContractError::new(
                        ContractErrorCode::InvalidContract,
                        format!("{path}.subject_digest"),
                        "decision is not bound to the current subject",
                    ));
                }
                validate_non_empty(&decision.code, &format!("{path}.decision.code"))?;
                for (effect_index, effect) in decision.effects.iter().enumerate() {
                    validate_non_empty(
                        effect,
                        &format!("{path}.decision.effects[{effect_index}]"),
                    )?;
                }
                let record = RecordedDecision {
                    sequence: *sequence,
                    gate: *gate,
                    protected_effect_digest: protected_effect_digest.clone(),
                    subject_digest: subject_digest.clone(),
                    outcome: decision.outcome.clone(),
                    code: decision.code.clone(),
                };
                if decisions
                    .insert(decision_id.clone(), record.clone())
                    .is_some()
                {
                    return Err(ContractError::new(
                        ContractErrorCode::InvalidContract,
                        format!("{path}.decision_id"),
                        "decision IDs must be unique",
                    ));
                }
                if *gate == Gate::Workflow
                    && decision.outcome == Outcome::Allow
                    && decision.code == "workflow.completion_authorized"
                {
                    completion_decision = Some(record);
                }
            }
            RunEvent::ToolExecution {
                decision_id,
                protected_effect_digest,
                action_digest,
                capability_digest,
                evidence,
                ..
            } => {
                validate_authorization(
                    &decisions,
                    decision_id,
                    protected_effect_digest,
                    event.sequence(),
                    &current_subject,
                    &path,
                )?;
                validate_digest(action_digest, &format!("{path}.action_digest"))?;
                validate_digest(capability_digest, &format!("{path}.capability_digest"))?;
                if capability_digest != &request.requested_capability.digest {
                    return Err(ContractError::new(
                        ContractErrorCode::RequestMismatch,
                        format!("{path}.capability_digest"),
                        "tool execution capability does not match the Agent Run Request",
                    ));
                }
                validate_evidence_set(evidence, &path)?;
                if !evidence
                    .iter()
                    .any(|item| item.evidence_type == EvidenceType::ToolExecution)
                {
                    return Err(ContractError::new(
                        ContractErrorCode::InvalidContract,
                        format!("{path}.evidence"),
                        "tool execution requires tool_execution evidence",
                    ));
                }
            }
            RunEvent::Mutation {
                sequence,
                decision_id,
                protected_effect_digest,
                before_subject_digest,
                after_subject_digest,
                evidence,
            } => {
                validate_authorization(
                    &decisions,
                    decision_id,
                    protected_effect_digest,
                    *sequence,
                    &current_subject,
                    &path,
                )?;
                validate_digest(
                    before_subject_digest,
                    &format!("{path}.before_subject_digest"),
                )?;
                validate_digest(
                    after_subject_digest,
                    &format!("{path}.after_subject_digest"),
                )?;
                if before_subject_digest != &current_subject {
                    return Err(ContractError::new(
                        ContractErrorCode::InvalidContract,
                        format!("{path}.before_subject_digest"),
                        "mutation does not begin at the current subject",
                    ));
                }
                validate_evidence_set(evidence, &path)?;
                if !evidence
                    .iter()
                    .any(|item| item.evidence_type == EvidenceType::Artifact)
                {
                    return Err(ContractError::new(
                        ContractErrorCode::InvalidContract,
                        format!("{path}.evidence"),
                        "mutation requires artifact evidence",
                    ));
                }
                current_subject.clone_from(after_subject_digest);
                last_mutation_sequence = *sequence;
            }
            RunEvent::Verification {
                sequence,
                subject_digest,
                implementer_id,
                verifier_id,
                verdict,
                evidence,
            } => {
                validate_digest(subject_digest, &format!("{path}.subject_digest"))?;
                validate_non_empty(implementer_id, &format!("{path}.implementer_id"))?;
                validate_non_empty(verifier_id, &format!("{path}.verifier_id"))?;
                validate_evidence_set(evidence, &path)?;
                if *verdict == VerificationVerdict::Pass {
                    if implementer_id == verifier_id {
                        return Err(ContractError::new(
                            ContractErrorCode::InvalidContract,
                            format!("{path}.verifier_id"),
                            "passing verification must use a different actor",
                        ));
                    }
                    for required in &request.required_verification.evidence_types {
                        if !evidence.iter().any(|item| item.evidence_type == *required) {
                            return Err(ContractError::new(
                                ContractErrorCode::InvalidContract,
                                format!("{path}.evidence"),
                                format!("missing required {required:?} evidence"),
                            ));
                        }
                    }
                    if subject_digest == &current_subject {
                        latest_passing_verification = Some((*sequence, subject_digest.clone()));
                    }
                }
            }
            RunEvent::Usage { usage, .. } => {
                validate_usage(usage, &format!("{path}.usage"))?;
                if latest_usage
                    .as_ref()
                    .is_some_and(|previous| !usage_is_monotonic(previous, usage))
                {
                    return Err(ContractError::new(
                        ContractErrorCode::InvalidContract,
                        format!("{path}.usage"),
                        "cumulative usage must not decrease",
                    ));
                }
                latest_usage = Some(usage.clone());
            }
            RunEvent::Interruption {
                actor_id,
                reason,
                evidence,
                ..
            } => {
                if let Some(actor_id) = actor_id {
                    validate_non_empty(actor_id, &format!("{path}.actor_id"))?;
                }
                validate_non_empty(reason, &format!("{path}.reason"))?;
                validate_evidence(evidence, &format!("{path}.evidence"))?;
                if evidence.evidence_type != EvidenceType::Interruption {
                    return Err(ContractError::new(
                        ContractErrorCode::InvalidContract,
                        format!("{path}.evidence.evidence_type"),
                        "interruption event requires interruption evidence",
                    ));
                }
                interruption_seen = true;
            }
        }
    }

    if status != body.terminal_status {
        return Err(ContractError::new(
            ContractErrorCode::InvalidTransition,
            "body.terminal_status",
            "terminal status does not match the final lifecycle transition",
        ));
    }
    if !matches!(
        body.events.last(),
        Some(RunEvent::StatusTransition { to, .. }) if *to == body.terminal_status
    ) {
        return Err(ContractError::new(
            ContractErrorCode::InvalidTransition,
            "events",
            "the final event must transition to the declared terminal status",
        ));
    }
    if current_subject != body.resulting_subject_digest {
        return Err(ContractError::new(
            ContractErrorCode::RequestMismatch,
            "body.resulting_subject_digest",
            "resulting subject does not match the latest observed mutation",
        ));
    }
    if latest_usage.as_ref() != Some(&body.usage) {
        return Err(ContractError::new(
            ContractErrorCode::InvalidContract,
            "body.usage",
            "receipt usage must equal the final cumulative usage event",
        ));
    }
    if body.terminal_status == AgentRunStatus::Interrupted && !interruption_seen {
        return Err(ContractError::new(
            ContractErrorCode::InvalidContract,
            "events",
            "interrupted receipt requires interruption evidence",
        ));
    }

    if body.terminal_status == AgentRunStatus::Completed {
        validate_completed_receipt(
            request,
            &body.usage,
            &current_subject,
            last_mutation_sequence,
            latest_passing_verification,
            completion_decision,
        )?;
    } else if completion_decision.is_some() {
        return Err(ContractError::new(
            ContractErrorCode::InvalidContract,
            "body.terminal_status",
            "a completion-authorized receipt must terminate as completed",
        ));
    }

    Ok(())
}

fn validate_completed_receipt(
    request: &AgentRunRequest,
    usage: &ResourceUsage,
    current_subject: &str,
    last_mutation_sequence: u64,
    latest_passing_verification: Option<(u64, String)>,
    completion_decision: Option<RecordedDecision>,
) -> Result<(), ContractError> {
    if !usage_within_budget(usage, &request.resource_budget) {
        return Err(ContractError::new(
            ContractErrorCode::InvalidContract,
            "body.usage",
            "completed run exceeds its resource budget",
        ));
    }
    let Some((verification_sequence, verification_subject)) = latest_passing_verification else {
        return Err(ContractError::new(
            ContractErrorCode::StaleVerification,
            "events",
            "completion requires a current passing independent verification",
        ));
    };
    if verification_sequence <= last_mutation_sequence || verification_subject != current_subject {
        return Err(ContractError::new(
            ContractErrorCode::StaleVerification,
            "events",
            "a mutation after verification makes that verification insufficient",
        ));
    }
    let Some(completion) = completion_decision else {
        return Err(ContractError::new(
            ContractErrorCode::UnauthorizedEffect,
            "events",
            "completion requires a workflow completion allow decision",
        ));
    };
    if completion.sequence <= verification_sequence
        || completion.subject_digest != current_subject
        || completion.protected_effect_digest != current_subject
        || completion.outcome != Outcome::Allow
        || completion.gate != Gate::Workflow
        || completion.code != "workflow.completion_authorized"
    {
        return Err(ContractError::new(
            ContractErrorCode::UnauthorizedEffect,
            "events",
            "completion decision is not bound to the current verified subject",
        ));
    }
    Ok(())
}

fn validate_authorization(
    decisions: &BTreeMap<String, RecordedDecision>,
    decision_id: &str,
    protected_effect_digest: &str,
    effect_sequence: u64,
    current_subject: &str,
    path: &str,
) -> Result<(), ContractError> {
    validate_non_empty(decision_id, &format!("{path}.decision_id"))?;
    validate_digest(
        protected_effect_digest,
        &format!("{path}.protected_effect_digest"),
    )?;
    let Some(decision) = decisions.get(decision_id) else {
        return Err(ContractError::new(
            ContractErrorCode::UnauthorizedEffect,
            format!("{path}.decision_id"),
            "effect does not reference an earlier decision",
        ));
    };
    if decision.sequence >= effect_sequence
        || decision.protected_effect_digest != protected_effect_digest
        || decision.subject_digest != current_subject
        || !matches!(
              decision.gate,
              Gate::Permission | Gate::ToolTrust | Gate::Runtime | Gate::Workflow
          )
          || decision.outcome != Outcome::Allow
    {
        return Err(ContractError::new(
            ContractErrorCode::UnauthorizedEffect,
            format!("{path}.decision_id"),
            "effect requires an earlier matching allow decision",
        ));
    }
    Ok(())
}

fn validate_approval(approval: &ApprovalReference, path: &str) -> Result<(), ContractError> {
    validate_non_empty(
        &approval.approval_id,
        &format!("{path}.approval.approval_id"),
    )?;
    validate_non_empty(&approval.actor_id, &format!("{path}.approval.actor_id"))?;
    validate_non_empty(&approval.scope, &format!("{path}.approval.scope"))?;
    validate_digest(
        &approval.subject_digest,
        &format!("{path}.approval.subject_digest"),
    )?;
    validate_evidence(&approval.evidence, &format!("{path}.approval.evidence"))?;
    if approval.evidence.evidence_type != EvidenceType::Approval {
        return Err(ContractError::new(
            ContractErrorCode::InvalidContract,
            format!("{path}.approval.evidence.evidence_type"),
            "approval event requires approval evidence",
        ));
    }
    Ok(())
}

fn validate_evidence_set(evidence: &[EvidenceReference], path: &str) -> Result<(), ContractError> {
    if evidence.is_empty() {
        return Err(ContractError::new(
            ContractErrorCode::InvalidContract,
            format!("{path}.evidence"),
            "event requires content-addressed evidence",
        ));
    }
    for (index, item) in evidence.iter().enumerate() {
        validate_evidence(item, &format!("{path}.evidence[{index}]"))?;
    }
    Ok(())
}

fn validate_usage(usage: &ResourceUsage, path: &str) -> Result<(), ContractError> {
    validate_safe_integer(usage.cost_micros, &format!("{path}.cost_micros"))?;
    validate_safe_integer(usage.elapsed_ms, &format!("{path}.elapsed_ms"))?;
    validate_safe_integer(usage.model_tokens, &format!("{path}.model_tokens"))?;
    validate_safe_integer(usage.tool_calls, &format!("{path}.tool_calls"))?;
    Ok(())
}

fn usage_is_monotonic(previous: &ResourceUsage, current: &ResourceUsage) -> bool {
    current.cost_micros >= previous.cost_micros
        && current.elapsed_ms >= previous.elapsed_ms
        && current.model_tokens >= previous.model_tokens
        && current.tool_calls >= previous.tool_calls
}

fn usage_within_budget(usage: &ResourceUsage, budget: &ResourceBudget) -> bool {
    usage.cost_micros <= budget.max_cost_micros
        && usage.elapsed_ms <= budget.max_elapsed_ms
        && usage.model_tokens <= budget.max_model_tokens
        && usage.tool_calls <= budget.max_tool_calls
}

fn valid_transition(from: AgentRunStatus, to: AgentRunStatus) -> bool {
    if from.is_terminal() || from == to {
        return false;
    }
    if matches!(
        to,
        AgentRunStatus::Blocked | AgentRunStatus::Failed | AgentRunStatus::Interrupted
    ) {
        return true;
    }
    matches!(
        (from, to),
        (AgentRunStatus::Accepted, AgentRunStatus::Planning)
            | (
                AgentRunStatus::Planning,
                AgentRunStatus::AwaitingAuthority | AgentRunStatus::Executing
            )
            | (
                AgentRunStatus::AwaitingAuthority,
                AgentRunStatus::Planning | AgentRunStatus::Executing
            )
            | (
                AgentRunStatus::Executing,
                AgentRunStatus::AwaitingAuthority | AgentRunStatus::Verifying
            )
            | (
                AgentRunStatus::Verifying,
                AgentRunStatus::Executing | AgentRunStatus::Completed
            )
    )
}

fn request_mismatch(path: &str) -> ContractError {
    ContractError::new(
        ContractErrorCode::RequestMismatch,
        path,
        "receipt does not match the validated Agent Run Request",
    )
}

pub(crate) fn validate_non_empty(value: &str, path: &str) -> Result<(), ContractError> {
    if value.trim().is_empty() {
        return Err(ContractError::new(
            ContractErrorCode::InvalidContract,
            path,
            "value must not be empty",
        ));
    }
    Ok(())
}

pub(crate) fn validate_digest(value: &str, path: &str) -> Result<(), ContractError> {
    let Some(hex) = value.strip_prefix("sha256:") else {
        return Err(invalid_digest(path));
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid_digest(path));
    }
    Ok(())
}

pub(crate) fn validate_safe_integer(value: u64, path: &str) -> Result<(), ContractError> {
    if value > MAX_SAFE_INTEGER {
        return Err(ContractError::new(
            ContractErrorCode::InvalidContract,
            path,
            format!("integer exceeds the JCS safe maximum {MAX_SAFE_INTEGER}"),
        ));
    }
    Ok(())
}

pub(crate) fn validate_evidence(
    evidence: &EvidenceReference,
    path: &str,
) -> Result<(), ContractError> {
    validate_digest(&evidence.digest, &format!("{path}.digest"))?;
    if let Some(locator) = &evidence.locator {
        validate_non_empty(locator, &format!("{path}.locator"))?;
    }
    Ok(())
}

fn invalid_digest(path: &str) -> ContractError {
    ContractError::new(
        ContractErrorCode::InvalidDigest,
        path,
        "expected sha256 followed by 64 lowercase hexadecimal characters",
    )
}
