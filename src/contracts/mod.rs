//! Versioned, provider-neutral contracts for one governed Agent Run.

mod canonical;
mod model;
mod protected_effect;
mod schema;
mod validation;

pub use canonical::{
    canonical_protected_effect_request_bytes, canonical_protected_effect_result_body_bytes,
    canonical_request_bytes, canonical_resource_budget_digest,
};
pub use model::{
    AGENT_RUN_REQUEST_SCHEMA, AgentRunRequest, AgentRunStatus, ApprovalReference,
    CapabilityIdentity, EvidenceReference, EvidenceType, PolicyIdentity, ResourceBudget,
    ResourceUsage, RunEvent, Subject, SubjectKind, TERMINAL_RUN_RECEIPT_SCHEMA, TaskSpec,
    TerminalRunReceipt, TerminalRunReceiptBody, VerificationIndependence, VerificationRequirement,
    VerificationVerdict,
};
pub use protected_effect::{
    EffectClass, EffectEvidenceReference, EffectEvidenceType, EffectExecutionStatus, EffectExit,
    EffectUsage, ExecutorIdentity, FilesystemAccess, InputMetadataEntry, NetworkProtocol,
    OperationFamily, PROTECTED_EFFECT_REQUEST_SCHEMA, PROTECTED_EFFECT_RESULT_SCHEMA,
    ProtectedEffectDecision, ProtectedEffectRequest, ProtectedEffectResult,
    ProtectedEffectResultBody, Repeatability, RequestedScope, SandboxProfileIdentity,
    parse_protected_effect_request_json, parse_protected_effect_result_json,
    seal_protected_effect_result, validate_protected_effect_request,
    validate_protected_effect_result_body, verify_protected_effect_result,
};
pub use schema::{agent_run_request_schema, terminal_run_receipt_schema};
pub use validation::{
    ContractError, ContractErrorCode, ContractSupport, parse_agent_run_request_json,
    parse_terminal_run_receipt_json, seal_terminal_receipt, validate_request,
    verify_terminal_receipt,
};
