//! Canonical contracts for one proposed or observed protected effect.

mod model;
mod result_validation;
mod validation;

pub use model::{
    EffectClass, EffectEvidenceReference, EffectEvidenceType, EffectExecutionStatus, EffectExit,
    EffectUsage, ExecutorIdentity, FilesystemAccess, InputMetadataEntry, NetworkProtocol,
    OperationFamily, PROTECTED_EFFECT_REQUEST_SCHEMA, PROTECTED_EFFECT_RESULT_SCHEMA,
    ProtectedEffectDecision, ProtectedEffectRequest, ProtectedEffectResult,
    ProtectedEffectResultBody, Repeatability, RequestedScope, SandboxProfileIdentity,
};
pub use result_validation::{
    parse_protected_effect_result_json, seal_protected_effect_result,
    validate_protected_effect_result_body, verify_protected_effect_result,
};
pub use validation::{parse_protected_effect_request_json, validate_protected_effect_request};
