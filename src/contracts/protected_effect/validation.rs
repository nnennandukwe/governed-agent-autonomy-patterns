use std::collections::BTreeSet;

use super::super::canonical::{
    canonical_protected_effect_request_bytes, canonical_resource_budget_digest, sha256_digest,
};
use super::super::model::{AgentRunRequest, EvidenceType};
use super::super::validation::{
    ContractError, ContractErrorCode, ContractSupport, validate_digest, validate_evidence,
    validate_non_empty, validate_request, validate_safe_integer,
};
use super::model::{PROTECTED_EFFECT_REQUEST_SCHEMA, ProtectedEffectRequest, RequestedScope};

const MAX_METADATA_ENTRIES: usize = 32;
const MAX_METADATA_NAME_CHARS: usize = 64;
const MAX_METADATA_VALUE_CHARS: usize = 256;

/// Strictly parse a Protected Effect Request from UTF-8 JSON bytes.
pub fn parse_protected_effect_request_json(
    bytes: &[u8],
) -> Result<ProtectedEffectRequest, ContractError> {
    serde_json::from_slice(bytes).map_err(|error| {
        ContractError::new(
            ContractErrorCode::InvalidJson,
            "$",
            format!("invalid Protected Effect Request JSON: {error}"),
        )
    })
}

/// Validate one proposed effect against its exact parent Agent Run Request.
pub fn validate_protected_effect_request(
    agent_run_request: &AgentRunRequest,
    support: &ContractSupport,
    request: &ProtectedEffectRequest,
) -> Result<String, ContractError> {
    let expected_agent_run_digest = validate_request(agent_run_request, support)?;
    if request.schema_version != PROTECTED_EFFECT_REQUEST_SCHEMA {
        return Err(ContractError::new(
            ContractErrorCode::UnsupportedSchema,
            "schema_version",
            format!(
                "unsupported Protected Effect Request schema: {}",
                request.schema_version
            ),
        ));
    }
    validate_non_empty(&request.effect_id, "effect_id")?;
    if request.effect_sequence == 0 {
        return Err(invalid_contract(
            "effect_sequence",
            "effect sequence must start at 1",
        ));
    }
    validate_safe_integer(request.effect_sequence, "effect_sequence")?;
    validate_non_empty(&request.run_id, "run_id")?;
    if request.run_id != agent_run_request.run_id {
        return Err(parent_mismatch("run_id"));
    }
    validate_digest(
        &request.agent_run_request_digest,
        "agent_run_request_digest",
    )?;
    if request.agent_run_request_digest != expected_agent_run_digest {
        return Err(parent_mismatch("agent_run_request_digest"));
    }

    validate_subject(request)?;
    validate_operation(request)?;
    validate_capability(request)?;
    if request.capability != agent_run_request.requested_capability {
        return Err(parent_mismatch("capability"));
    }
    if let Some(tool_schema_digest) = &request.tool_schema_digest {
        validate_digest(tool_schema_digest, "tool_schema_digest")?;
    }
    validate_digest(&request.input_digest, "input_digest")?;
    validate_metadata(request)?;
    validate_scopes(request)?;

    if request.policies != agent_run_request.policies {
        return Err(parent_mismatch("policies"));
    }
    validate_approvals(request)?;

    validate_digest(&request.resource_budget_digest, "resource_budget_digest")?;
    let expected_budget_digest =
        canonical_resource_budget_digest(&agent_run_request.resource_budget)?;
    if request.resource_budget_digest != expected_budget_digest {
        return Err(parent_mismatch("resource_budget_digest"));
    }

    validate_non_empty(&request.sandbox_profile.name, "sandbox_profile.name")?;
    validate_non_empty(&request.sandbox_profile.version, "sandbox_profile.version")?;
    validate_digest(&request.sandbox_profile.digest, "sandbox_profile.digest")?;
    validate_non_empty(&request.idempotency_key, "idempotency_key")?;

    canonical_protected_effect_request_bytes(request).map(|bytes| sha256_digest(&bytes))
}

fn validate_subject(request: &ProtectedEffectRequest) -> Result<(), ContractError> {
    validate_non_empty(&request.subject.locator, "subject.locator")?;
    validate_digest(&request.subject.digest, "subject.digest")
}

fn validate_operation(request: &ProtectedEffectRequest) -> Result<(), ContractError> {
    validate_non_empty(&request.normalized_operation, "normalized_operation")?;
    let Some((family, _)) = request.normalized_operation.split_once('.') else {
        return Err(invalid_contract(
            "normalized_operation",
            "normalized operation must contain a family prefix and operation name",
        ));
    };
    if family != request.operation_family.as_str()
        || !request
            .normalized_operation
            .split('.')
            .all(valid_normalized_segment)
    {
        return Err(invalid_contract(
            "normalized_operation",
            "normalized operation must use lower-snake segments prefixed by its operation family",
        ));
    }
    Ok(())
}

fn validate_capability(request: &ProtectedEffectRequest) -> Result<(), ContractError> {
    validate_non_empty(&request.capability.name, "capability.name")?;
    validate_non_empty(&request.capability.version, "capability.version")?;
    validate_digest(&request.capability.digest, "capability.digest")
}

fn validate_metadata(request: &ProtectedEffectRequest) -> Result<(), ContractError> {
    if request.input_metadata.len() > MAX_METADATA_ENTRIES {
        return Err(invalid_contract(
            "input_metadata",
            format!("input metadata is limited to {MAX_METADATA_ENTRIES} entries"),
        ));
    }
    let mut names = BTreeSet::new();
    for (index, entry) in request.input_metadata.iter().enumerate() {
        let path = format!("input_metadata[{index}]");
        validate_bounded_text(
            &entry.name,
            &format!("{path}.name"),
            MAX_METADATA_NAME_CHARS,
        )?;
        if !names.insert(entry.name.as_str()) {
            return Err(invalid_contract(
                format!("{path}.name"),
                "metadata names must be unique",
            ));
        }
        validate_bounded_text(
            &entry.value,
            &format!("{path}.value"),
            MAX_METADATA_VALUE_CHARS,
        )?;
    }
    Ok(())
}

fn validate_scopes(request: &ProtectedEffectRequest) -> Result<(), ContractError> {
    if request.requested_scopes.is_empty() {
        return Err(invalid_contract(
            "requested_scopes",
            "at least one requested scope is required",
        ));
    }
    for (index, scope) in request.requested_scopes.iter().enumerate() {
        let path = format!("requested_scopes[{index}]");
        match scope {
            RequestedScope::Filesystem { root, access, .. } => {
                validate_non_empty(root, &format!("{path}.root"))?;
                if access.is_empty() {
                    return Err(invalid_contract(
                        format!("{path}.access"),
                        "filesystem scope requires at least one access class",
                    ));
                }
                let mut classes = BTreeSet::new();
                for class in access {
                    if !classes.insert(class) {
                        return Err(invalid_contract(
                            format!("{path}.access"),
                            "filesystem access classes must be unique",
                        ));
                    }
                }
            }
            RequestedScope::Process {
                executable,
                working_directory,
            } => {
                validate_non_empty(executable, &format!("{path}.executable"))?;
                validate_non_empty(working_directory, &format!("{path}.working_directory"))?;
            }
            RequestedScope::Network { host, port, .. } => {
                validate_non_empty(host, &format!("{path}.host"))?;
                if host.contains("://") || host.chars().any(char::is_whitespace) {
                    return Err(invalid_contract(
                        format!("{path}.host"),
                        "network host must not contain a scheme or whitespace",
                    ));
                }
                if *port == 0 {
                    return Err(invalid_contract(
                        format!("{path}.port"),
                        "network port must be between 1 and 65535",
                    ));
                }
            }
            RequestedScope::ExternalService {
                service,
                operation,
                resource,
            } => {
                validate_non_empty(service, &format!("{path}.service"))?;
                validate_non_empty(operation, &format!("{path}.operation"))?;
                validate_non_empty(resource, &format!("{path}.resource"))?;
            }
        }
    }
    Ok(())
}

fn validate_approvals(request: &ProtectedEffectRequest) -> Result<(), ContractError> {
    let mut approval_ids = BTreeSet::new();
    for (index, approval) in request.approval_context.iter().enumerate() {
        let path = format!("approval_context[{index}]");
        validate_non_empty(&approval.approval_id, &format!("{path}.approval_id"))?;
        if !approval_ids.insert(approval.approval_id.as_str()) {
            return Err(invalid_contract(
                format!("{path}.approval_id"),
                "approval IDs must be unique",
            ));
        }
        validate_non_empty(&approval.actor_id, &format!("{path}.actor_id"))?;
        validate_non_empty(&approval.scope, &format!("{path}.scope"))?;
        validate_digest(&approval.subject_digest, &format!("{path}.subject_digest"))?;
        validate_evidence(&approval.evidence, &format!("{path}.evidence"))?;
        if approval.evidence.evidence_type != EvidenceType::Approval {
            return Err(invalid_contract(
                format!("{path}.evidence.evidence_type"),
                "approval context must reference approval evidence",
            ));
        }
    }
    Ok(())
}

fn validate_bounded_text(
    value: &str,
    path: &str,
    maximum_chars: usize,
) -> Result<(), ContractError> {
    validate_non_empty(value, path)?;
    if value.chars().count() > maximum_chars {
        return Err(invalid_contract(
            path,
            format!("value must not exceed {maximum_chars} characters"),
        ));
    }
    Ok(())
}

fn valid_normalized_segment(value: &str) -> bool {
    let mut characters = value.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_lowercase())
        && characters.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        })
}

fn parent_mismatch(path: &str) -> ContractError {
    ContractError::new(
        ContractErrorCode::RequestMismatch,
        path,
        "Protected Effect Request does not match the validated Agent Run Request",
    )
}

fn invalid_contract(path: impl Into<String>, message: impl Into<String>) -> ContractError {
    ContractError::new(ContractErrorCode::InvalidContract, path, message)
}
