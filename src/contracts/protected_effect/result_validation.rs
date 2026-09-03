use std::collections::BTreeSet;

use super::super::canonical::{canonical_protected_effect_result_body_bytes, sha256_digest};
use super::super::model::{AgentRunRequest, CapabilityIdentity, Subject};
use super::super::validation::{
    ContractError, ContractErrorCode, ContractSupport, validate_digest, validate_non_empty,
    validate_safe_integer,
};
use super::model::{
    EffectClass, EffectEvidenceReference, EffectEvidenceType, EffectExecutionStatus, EffectExit,
    EffectUsage, ExecutorIdentity, OperationFamily, PROTECTED_EFFECT_RESULT_SCHEMA,
    ProtectedEffectRequest, ProtectedEffectResult, ProtectedEffectResultBody,
    SandboxProfileIdentity,
};
use super::validation::validate_protected_effect_request;
use crate::{Gate, Outcome};

const STALE_SUBJECT_REASON: &str = "protected_effect.stale_subject";
const SCHEMA_DRIFT_REASON: &str = "protected_effect.capability_schema_drift";
const COMBINED_DRIFT_REASON: &str = "protected_effect.subject_and_capability_schema_drift";

/// Strictly parse a Protected Effect Result from UTF-8 JSON bytes.
pub fn parse_protected_effect_result_json(
    bytes: &[u8],
) -> Result<ProtectedEffectResult, ContractError> {
    serde_json::from_slice(bytes).map_err(|error| {
        ContractError::new(
            ContractErrorCode::InvalidJson,
            "$",
            format!("invalid Protected Effect Result JSON: {error}"),
        )
    })
}

/// Validate a result body and its exact binding to a protected effect request.
pub fn validate_protected_effect_result_body(
    agent_run_request: &AgentRunRequest,
    support: &ContractSupport,
    request: &ProtectedEffectRequest,
    body: &ProtectedEffectResultBody,
) -> Result<(), ContractError> {
    let effect_request_digest =
        validate_protected_effect_request(agent_run_request, support, request)?;
    validate_result_identity(request, &effect_request_digest, body)?;
    validate_observed_identities(body)?;
    validate_decision(&effect_request_digest, body)?;
    validate_usage(&body.usage)?;
    validate_evidence(&body.evidence)?;
    validate_operation_specific_result(request, body)?;

    let subject_drift = body.observed_pre_effect_subject != request.subject;
    if body.observed_capability != request.capability {
        return Err(request_mismatch("body.observed_capability"));
    }
    let schema_drift = body.observed_tool_schema_digest != request.tool_schema_digest;
    validate_drift(body, subject_drift, schema_drift)?;
    validate_status_matrix(request, body)?;
    Ok(())
}

/// Validate and seal one immutable Protected Effect Result body.
pub fn seal_protected_effect_result(
    agent_run_request: &AgentRunRequest,
    support: &ContractSupport,
    request: &ProtectedEffectRequest,
    body: ProtectedEffectResultBody,
) -> Result<ProtectedEffectResult, ContractError> {
    validate_protected_effect_result_body(agent_run_request, support, request, &body)?;
    let result_digest = sha256_digest(&canonical_protected_effect_result_body_bytes(&body)?);
    Ok(ProtectedEffectResult {
        result_digest,
        body,
    })
}

/// Verify a result digest and its semantic binding to one exact effect request.
pub fn verify_protected_effect_result(
    agent_run_request: &AgentRunRequest,
    support: &ContractSupport,
    request: &ProtectedEffectRequest,
    result: &ProtectedEffectResult,
) -> Result<(), ContractError> {
    validate_digest(&result.result_digest, "result_digest").map_err(|_| {
        ContractError::new(
            ContractErrorCode::ResultTampering,
            "result_digest",
            "result digest is malformed",
        )
    })?;
    let actual_digest = sha256_digest(&canonical_protected_effect_result_body_bytes(&result.body)?);
    if actual_digest != result.result_digest {
        return Err(ContractError::new(
            ContractErrorCode::ResultTampering,
            "result_digest",
            format!(
                "result digest mismatch: expected {}, calculated {actual_digest}",
                result.result_digest
            ),
        ));
    }
    validate_protected_effect_result_body(agent_run_request, support, request, &result.body)
}

fn validate_result_identity(
    request: &ProtectedEffectRequest,
    effect_request_digest: &str,
    body: &ProtectedEffectResultBody,
) -> Result<(), ContractError> {
    if body.schema_version != PROTECTED_EFFECT_RESULT_SCHEMA {
        return Err(ContractError::new(
            ContractErrorCode::UnsupportedSchema,
            "body.schema_version",
            format!(
                "unsupported Protected Effect Result schema: {}",
                body.schema_version
            ),
        ));
    }
    validate_non_empty(&body.effect_id, "body.effect_id")?;
    if body.effect_id != request.effect_id {
        return Err(request_mismatch("body.effect_id"));
    }
    validate_safe_integer(body.effect_sequence, "body.effect_sequence")?;
    if body.effect_sequence == 0 || body.effect_sequence != request.effect_sequence {
        return Err(request_mismatch("body.effect_sequence"));
    }
    validate_non_empty(&body.run_id, "body.run_id")?;
    if body.run_id != request.run_id {
        return Err(request_mismatch("body.run_id"));
    }
    validate_digest(
        &body.agent_run_request_digest,
        "body.agent_run_request_digest",
    )?;
    if body.agent_run_request_digest != request.agent_run_request_digest {
        return Err(request_mismatch("body.agent_run_request_digest"));
    }
    validate_digest(&body.effect_request_digest, "body.effect_request_digest")?;
    if body.effect_request_digest != effect_request_digest {
        return Err(request_mismatch("body.effect_request_digest"));
    }
    Ok(())
}

fn validate_observed_identities(body: &ProtectedEffectResultBody) -> Result<(), ContractError> {
    validate_subject(
        &body.observed_pre_effect_subject,
        "body.observed_pre_effect_subject",
    )?;
    validate_capability(&body.observed_capability, "body.observed_capability")?;
    if let Some(digest) = &body.observed_tool_schema_digest {
        validate_digest(digest, "body.observed_tool_schema_digest")?;
    }
    if let Some(subject) = &body.observed_post_effect_subject {
        validate_subject(subject, "body.observed_post_effect_subject")?;
    }
    Ok(())
}

fn validate_decision(
    effect_request_digest: &str,
    body: &ProtectedEffectResultBody,
) -> Result<(), ContractError> {
    let decision = &body.decision;
    validate_non_empty(&decision.decision_id, "body.decision.decision_id")?;
    validate_digest(
        &decision.effect_request_digest,
        "body.decision.effect_request_digest",
    )?;
    if decision.effect_request_digest != effect_request_digest {
        return Err(request_mismatch("body.decision.effect_request_digest"));
    }
    validate_digest(&decision.subject_digest, "body.decision.subject_digest")?;
    if decision.subject_digest != body.observed_pre_effect_subject.digest {
        return Err(request_mismatch("body.decision.subject_digest"));
    }
    validate_non_empty(&decision.decision.code, "body.decision.decision.code")?;
    for (index, effect) in decision.decision.effects.iter().enumerate() {
        validate_non_empty(effect, &format!("body.decision.decision.effects[{index}]"))?;
    }
    if matches!(
        body.execution_status,
        EffectExecutionStatus::Executed
            | EffectExecutionStatus::Failed
            | EffectExecutionStatus::Interrupted
            | EffectExecutionStatus::UnknownOutcome
    ) && decision.gate != Gate::Permission
    {
        return Err(ContractError::new(
            ContractErrorCode::UnauthorizedEffect,
            "body.decision.gate",
            "an attempted effect requires an allow decision from the permission gate",
        ));
    }
    Ok(())
}

fn validate_status_matrix(
    request: &ProtectedEffectRequest,
    body: &ProtectedEffectResultBody,
) -> Result<(), ContractError> {
    let expected_outcome = match body.execution_status {
        EffectExecutionStatus::Executed
        | EffectExecutionStatus::Failed
        | EffectExecutionStatus::Interrupted
        | EffectExecutionStatus::UnknownOutcome => Outcome::Allow,
        EffectExecutionStatus::AwaitingAuthority => Outcome::Ask,
        EffectExecutionStatus::Denied => Outcome::Block,
    };
    if body.decision.decision.outcome != expected_outcome {
        let code = if matches!(
            body.execution_status,
            EffectExecutionStatus::Executed
                | EffectExecutionStatus::Failed
                | EffectExecutionStatus::Interrupted
                | EffectExecutionStatus::UnknownOutcome
        ) {
            ContractErrorCode::UnauthorizedEffect
        } else {
            ContractErrorCode::InvalidContract
        };
        return Err(ContractError::new(
            code,
            "body.decision.decision.outcome",
            format!(
                "{:?} requires a {expected_outcome:?} decision",
                body.execution_status
            ),
        ));
    }
    match body.execution_status {
        EffectExecutionStatus::Executed => validate_executed(request, body),
        EffectExecutionStatus::AwaitingAuthority | EffectExecutionStatus::Denied => {
            validate_non_execution(body)
        }
        EffectExecutionStatus::Failed => validate_known_attempt(
            request,
            body,
            EffectEvidenceType::Failure,
            &[
                EffectEvidenceType::Interruption,
                EffectEvidenceType::UnknownOutcome,
            ],
            "failed result requires failure evidence",
        ),
        EffectExecutionStatus::Interrupted => validate_known_attempt(
            request,
            body,
            EffectEvidenceType::Interruption,
            &[
                EffectEvidenceType::Failure,
                EffectEvidenceType::UnknownOutcome,
            ],
            "interrupted result requires interruption evidence",
        ),
        EffectExecutionStatus::UnknownOutcome => validate_unknown_attempt(request, body),
    }
}

fn validate_executed(
    request: &ProtectedEffectRequest,
    body: &ProtectedEffectResultBody,
) -> Result<(), ContractError> {
    if body.reason.is_some() {
        return Err(invalid_contract(
            "body.reason",
            "executed result must not contain a failure reason",
        ));
    }
    validate_attempt_common(request, body)?;
    validate_known_post_effect(request, body)?;
    reject_evidence_types(
        body,
        &[
            EffectEvidenceType::Failure,
            EffectEvidenceType::Interruption,
            EffectEvidenceType::UnknownOutcome,
        ],
    )
}

fn validate_non_execution(body: &ProtectedEffectResultBody) -> Result<(), ContractError> {
    validate_reason(body)?;
    if body.observed_post_effect_subject.is_some() {
        return Err(unauthorized("body.observed_post_effect_subject"));
    }
    if body.exit.is_some() {
        return Err(unauthorized("body.exit"));
    }
    if !usage_is_zero(&body.usage) {
        return Err(unauthorized("body.usage"));
    }
    if body.executor.is_some() {
        return Err(unauthorized("body.executor"));
    }
    if body.sandbox_profile.is_some() {
        return Err(unauthorized("body.sandbox_profile"));
    }
    for (index, reference) in body.evidence.iter().enumerate() {
        if evidence_is_execution_derived(reference.evidence_type) {
            return Err(unauthorized(format!(
                "body.evidence[{index}].evidence_type"
            )));
        }
    }
    Ok(())
}

fn validate_known_attempt(
    request: &ProtectedEffectRequest,
    body: &ProtectedEffectResultBody,
    required_evidence: EffectEvidenceType,
    rejected_evidence: &[EffectEvidenceType],
    message: &str,
) -> Result<(), ContractError> {
    validate_reason(body)?;
    validate_attempt_common(request, body)?;
    validate_known_post_effect(request, body)?;
    require_evidence(body, required_evidence, message)?;
    reject_evidence_types(body, rejected_evidence)
}

fn validate_unknown_attempt(
    request: &ProtectedEffectRequest,
    body: &ProtectedEffectResultBody,
) -> Result<(), ContractError> {
    validate_reason(body)?;
    validate_attempt_common(request, body)?;
    require_evidence(
        body,
        EffectEvidenceType::UnknownOutcome,
        "unknown outcome requires unknown_outcome evidence",
    )?;
    reject_evidence_types(
        body,
        &[
            EffectEvidenceType::Failure,
            EffectEvidenceType::Interruption,
        ],
    )?;
    if body.exit.is_some() {
        return Err(invalid_contract(
            "body.exit",
            "unknown outcome must not claim a known process exit",
        ));
    }
    if body.observed_post_effect_subject.is_some() {
        validate_known_post_effect(request, body)?;
    }
    Ok(())
}

fn validate_attempt_common(
    request: &ProtectedEffectRequest,
    body: &ProtectedEffectResultBody,
) -> Result<(), ContractError> {
    let executor = body
        .executor
        .as_ref()
        .ok_or_else(|| invalid_contract("body.executor", "attempt requires executor identity"))?;
    validate_executor(executor)?;
    require_evidence(
        body,
        EffectEvidenceType::Executor,
        "attempt requires executor evidence",
    )?;
    let sandbox = body.sandbox_profile.as_ref().ok_or_else(|| {
        invalid_contract("body.sandbox_profile", "attempt requires sandbox identity")
    })?;
    validate_sandbox(sandbox)?;
    if sandbox != &request.sandbox_profile {
        return Err(request_mismatch("body.sandbox_profile"));
    }
    require_evidence(
        body,
        EffectEvidenceType::Sandbox,
        "attempt requires sandbox evidence",
    )?;
    require_evidence(
        body,
        EffectEvidenceType::Usage,
        "attempt requires usage evidence",
    )?;
    Ok(())
}

fn validate_known_post_effect(
    request: &ProtectedEffectRequest,
    body: &ProtectedEffectResultBody,
) -> Result<(), ContractError> {
    let post_subject = body.observed_post_effect_subject.as_ref().ok_or_else(|| {
        invalid_contract(
            "body.observed_post_effect_subject",
            "known outcome requires a post-effect subject observation",
        )
    })?;
    require_evidence(
        body,
        EffectEvidenceType::SubjectObservation,
        "known post-effect subject requires subject_observation evidence",
    )?;
    match request.expected_effect_class {
        EffectClass::Observation => {
            if post_subject.digest != request.subject.digest {
                return Err(invalid_contract(
                    "body.observed_post_effect_subject",
                    "observation result must preserve the requested subject digest",
                ));
            }
        }
        EffectClass::Mutation => {
            require_evidence(
                body,
                EffectEvidenceType::Mutation,
                "mutation result requires mutation evidence",
            )?;
            require_evidence(
                body,
                EffectEvidenceType::Artifact,
                "mutation result requires artifact evidence",
            )?;
        }
    }

    if request.operation_family == OperationFamily::Process
        && matches!(
            body.execution_status,
            EffectExecutionStatus::Executed | EffectExecutionStatus::Failed
        )
    {
        let exit = body.exit.as_ref().ok_or_else(|| {
            invalid_contract("body.exit", "known process result requires an exit status")
        })?;
        validate_exit(exit)?;
        require_evidence(
            body,
            EffectEvidenceType::Exit,
            "known process result requires exit evidence",
        )?;
    } else if request.operation_family == OperationFamily::Process {
        if let Some(exit) = &body.exit {
            validate_exit(exit)?;
            require_evidence(
                body,
                EffectEvidenceType::Exit,
                "recorded exit status requires exit evidence",
            )?;
        }
    }
    Ok(())
}

fn validate_operation_specific_result(
    request: &ProtectedEffectRequest,
    body: &ProtectedEffectResultBody,
) -> Result<(), ContractError> {
    if request.operation_family == OperationFamily::Process {
        return Ok(());
    }
    if body.exit.is_some() {
        return Err(invalid_contract(
            "body.exit",
            "only process effects may record an exit status",
        ));
    }
    if let Some((index, _)) = body
        .evidence
        .iter()
        .enumerate()
        .find(|(_, reference)| reference.evidence_type == EffectEvidenceType::Exit)
    {
        return Err(invalid_contract(
            format!("body.evidence[{index}].evidence_type"),
            "only process effects may record exit evidence",
        ));
    }
    Ok(())
}

fn validate_drift(
    body: &ProtectedEffectResultBody,
    subject_drift: bool,
    schema_drift: bool,
) -> Result<(), ContractError> {
    if !subject_drift && !schema_drift {
        if matches!(
            body.reason.as_deref(),
            Some(STALE_SUBJECT_REASON | SCHEMA_DRIFT_REASON | COMBINED_DRIFT_REASON)
        ) {
            return Err(invalid_contract(
                "body.reason",
                "drift denial reason requires a requested-versus-observed identity difference",
            ));
        }
        return Ok(());
    }
    if body.execution_status != EffectExecutionStatus::Denied {
        let path = if subject_drift {
            "body.observed_pre_effect_subject"
        } else {
            "body.observed_tool_schema_digest"
        };
        return Err(request_mismatch(path));
    }

    let expected_reason = match (subject_drift, schema_drift) {
        (true, false) => STALE_SUBJECT_REASON,
        (false, true) => SCHEMA_DRIFT_REASON,
        (true, true) => COMBINED_DRIFT_REASON,
        (false, false) => unreachable!("drift validation is called only when drift exists"),
    };
    if body.reason.as_deref() != Some(expected_reason) {
        return Err(invalid_contract(
            "body.reason",
            format!("drift denial requires reason {expected_reason}"),
        ));
    }
    if subject_drift {
        require_evidence(
            body,
            EffectEvidenceType::SubjectObservation,
            "stale-subject denial requires subject_observation evidence",
        )?;
    }
    if schema_drift {
        require_evidence(
            body,
            EffectEvidenceType::CapabilitySchema,
            "schema-drift denial requires capability_schema evidence",
        )?;
    }
    Ok(())
}

fn validate_subject(subject: &Subject, path: &str) -> Result<(), ContractError> {
    validate_non_empty(&subject.locator, &format!("{path}.locator"))?;
    validate_digest(&subject.digest, &format!("{path}.digest"))
}

fn validate_capability(capability: &CapabilityIdentity, path: &str) -> Result<(), ContractError> {
    validate_non_empty(&capability.name, &format!("{path}.name"))?;
    validate_non_empty(&capability.version, &format!("{path}.version"))?;
    validate_digest(&capability.digest, &format!("{path}.digest"))
}

fn validate_executor(executor: &ExecutorIdentity) -> Result<(), ContractError> {
    validate_non_empty(&executor.name, "body.executor.name")?;
    validate_non_empty(&executor.version, "body.executor.version")?;
    validate_digest(&executor.digest, "body.executor.digest")
}

fn validate_sandbox(sandbox: &SandboxProfileIdentity) -> Result<(), ContractError> {
    validate_non_empty(&sandbox.name, "body.sandbox_profile.name")?;
    validate_non_empty(&sandbox.version, "body.sandbox_profile.version")?;
    validate_digest(&sandbox.digest, "body.sandbox_profile.digest")
}

fn validate_exit(exit: &EffectExit) -> Result<(), ContractError> {
    match exit {
        EffectExit::Code { .. } => Ok(()),
        EffectExit::Signal { signal } => validate_non_empty(signal, "body.exit.signal"),
    }
}

fn validate_usage(usage: &EffectUsage) -> Result<(), ContractError> {
    validate_safe_integer(usage.cost_micros, "body.usage.cost_micros")?;
    validate_safe_integer(usage.elapsed_ms, "body.usage.elapsed_ms")?;
    validate_safe_integer(usage.model_tokens, "body.usage.model_tokens")?;
    validate_safe_integer(usage.tool_calls, "body.usage.tool_calls")
}

fn validate_evidence(evidence: &[EffectEvidenceReference]) -> Result<(), ContractError> {
    let mut identities = BTreeSet::new();
    for (index, reference) in evidence.iter().enumerate() {
        let path = format!("body.evidence[{index}]");
        validate_digest(&reference.digest, &format!("{path}.digest"))?;
        if let Some(locator) = &reference.locator {
            validate_non_empty(locator, &format!("{path}.locator"))?;
        }
        if !identities.insert((
            reference.evidence_type,
            reference.digest.as_str(),
            reference.locator.as_deref(),
        )) {
            return Err(invalid_contract(
                path,
                "duplicate effect evidence reference",
            ));
        }
    }
    Ok(())
}

fn validate_reason(body: &ProtectedEffectResultBody) -> Result<(), ContractError> {
    let reason = body
        .reason
        .as_ref()
        .ok_or_else(|| invalid_contract("body.reason", "non-executed result requires a reason"))?;
    validate_non_empty(reason, "body.reason")
}

fn require_evidence(
    body: &ProtectedEffectResultBody,
    evidence_type: EffectEvidenceType,
    message: &str,
) -> Result<(), ContractError> {
    if body
        .evidence
        .iter()
        .any(|reference| reference.evidence_type == evidence_type)
    {
        Ok(())
    } else {
        Err(invalid_contract("body.evidence", message))
    }
}

fn reject_evidence_types(
    body: &ProtectedEffectResultBody,
    rejected: &[EffectEvidenceType],
) -> Result<(), ContractError> {
    if let Some((index, _)) = body
        .evidence
        .iter()
        .enumerate()
        .find(|(_, reference)| rejected.contains(&reference.evidence_type))
    {
        return Err(invalid_contract(
            format!("body.evidence[{index}].evidence_type"),
            "evidence type contradicts the execution status",
        ));
    }
    Ok(())
}

fn evidence_is_execution_derived(evidence_type: EffectEvidenceType) -> bool {
    match evidence_type {
        EffectEvidenceType::SubjectObservation | EffectEvidenceType::CapabilitySchema => false,
        EffectEvidenceType::Exit
        | EffectEvidenceType::Output
        | EffectEvidenceType::Artifact
        | EffectEvidenceType::Mutation
        | EffectEvidenceType::Usage
        | EffectEvidenceType::Executor
        | EffectEvidenceType::Sandbox
        | EffectEvidenceType::Failure
        | EffectEvidenceType::Interruption
        | EffectEvidenceType::UnknownOutcome => true,
    }
}

fn usage_is_zero(usage: &EffectUsage) -> bool {
    usage.cost_micros == 0
        && usage.elapsed_ms == 0
        && usage.model_tokens == 0
        && usage.tool_calls == 0
}

fn request_mismatch(path: &str) -> ContractError {
    ContractError::new(
        ContractErrorCode::RequestMismatch,
        path,
        "Protected Effect Result does not match the validated effect request",
    )
}

fn unauthorized(path: impl Into<String>) -> ContractError {
    ContractError::new(
        ContractErrorCode::UnauthorizedEffect,
        path,
        "non-execution result contains execution-derived state",
    )
}

fn invalid_contract(path: impl Into<String>, message: impl Into<String>) -> ContractError {
    ContractError::new(ContractErrorCode::InvalidContract, path, message)
}
