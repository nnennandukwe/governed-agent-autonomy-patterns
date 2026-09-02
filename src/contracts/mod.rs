//! Versioned, provider-neutral contracts for one governed Agent Run.

mod canonical;
mod model;
mod schema;
mod validation;

pub use canonical::canonical_request_bytes;
pub use model::{
    AGENT_RUN_REQUEST_SCHEMA, AgentRunRequest, AgentRunStatus, ApprovalReference,
    CapabilityIdentity, EvidenceReference, EvidenceType, PolicyIdentity, ResourceBudget,
    ResourceUsage, RunEvent, Subject, SubjectKind, TERMINAL_RUN_RECEIPT_SCHEMA, TaskSpec,
    TerminalRunReceipt, TerminalRunReceiptBody, VerificationIndependence, VerificationRequirement,
    VerificationVerdict,
};
pub use schema::{agent_run_request_schema, terminal_run_receipt_schema};
pub use validation::{
    ContractError, ContractErrorCode, ContractSupport, parse_agent_run_request_json,
    parse_terminal_run_receipt_json, seal_terminal_receipt, validate_request,
    verify_terminal_receipt,
};
