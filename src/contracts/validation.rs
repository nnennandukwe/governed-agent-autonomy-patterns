use std::collections::BTreeSet;
use std::error::Error;
use std::fmt::{self, Display, Formatter};

use serde::{Deserialize, Serialize};

use super::canonical::{canonical_request_bytes, sha256_digest};
use super::model::{
    AGENT_RUN_REQUEST_SCHEMA, AgentRunRequest, EvidenceReference, EvidenceType, PolicyIdentity,
};

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

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

    pub fn code(&self) -> ContractErrorCode {
        self.code
    }

    pub fn path(&self) -> &str {
        &self.path
    }
}

impl Display for ContractError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.path, self.message)
    }
}

impl Error for ContractError {}

#[derive(Debug, Clone, Default)]
pub struct ContractSupport {
    policies: BTreeSet<PolicyIdentity>,
}

impl ContractSupport {
    pub fn new(policies: impl IntoIterator<Item = PolicyIdentity>) -> Self {
        Self {
            policies: policies.into_iter().collect(),
        }
    }

    pub fn supports(&self, policy: &PolicyIdentity) -> bool {
        self.policies.contains(policy)
    }
}

pub fn parse_agent_run_request_json(bytes: &[u8]) -> Result<AgentRunRequest, ContractError> {
    serde_json::from_slice(bytes).map_err(|error| {
        ContractError::new(
            ContractErrorCode::InvalidJson,
            "$",
            format!("invalid Agent Run Request JSON: {error}"),
        )
    })
}

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
