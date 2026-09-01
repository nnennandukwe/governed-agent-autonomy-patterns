//! Versioned, provider-neutral contracts for one governed Agent Run.

mod canonical;
mod model;
mod validation;

pub use canonical::canonical_request_bytes;
pub use model::{
    AGENT_RUN_REQUEST_SCHEMA, AgentRunRequest, ApprovalReference, CapabilityIdentity,
    EvidenceReference, EvidenceType, PolicyIdentity, ResourceBudget, Subject, SubjectKind,
    TaskSpec, VerificationIndependence, VerificationRequirement,
};
pub use validation::{
    ContractError, ContractErrorCode, ContractSupport, parse_agent_run_request_json,
    validate_request,
};
