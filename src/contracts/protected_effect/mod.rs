//! Canonical contracts for one proposed or observed protected effect.

mod model;
mod validation;

pub use model::{
    EffectClass, FilesystemAccess, InputMetadataEntry, NetworkProtocol, OperationFamily,
    PROTECTED_EFFECT_REQUEST_SCHEMA, ProtectedEffectRequest, Repeatability, RequestedScope,
    SandboxProfileIdentity,
};
pub use validation::{parse_protected_effect_request_json, validate_protected_effect_request};
