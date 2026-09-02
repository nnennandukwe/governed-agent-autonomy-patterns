use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};

use super::super::model::{ApprovalReference, CapabilityIdentity, PolicyIdentity, Subject};

pub const PROTECTED_EFFECT_REQUEST_SCHEMA: &str = "gaap.protected-effect-request/0.1.0";

/// One protected effect proposed inside a governed Agent Run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProtectedEffectRequest {
    #[schemars(schema_with = "protected_effect_request_schema_version")]
    pub schema_version: String,
    #[schemars(length(min = 1))]
    pub effect_id: String,
    #[schemars(range(min = 1, max = 9007199254740991_u64))]
    pub effect_sequence: u64,
    #[schemars(length(min = 1))]
    pub run_id: String,
    #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
    pub agent_run_request_digest: String,
    pub subject: Subject,
    pub operation_family: OperationFamily,
    #[schemars(length(min = 1))]
    pub normalized_operation: String,
    pub capability: CapabilityIdentity,
    #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
    pub tool_schema_digest: Option<String>,
    #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
    pub input_digest: String,
    #[schemars(length(max = 32))]
    pub input_metadata: Vec<InputMetadataEntry>,
    #[schemars(length(min = 1))]
    pub requested_scopes: Vec<RequestedScope>,
    #[schemars(length(min = 1))]
    pub policies: Vec<PolicyIdentity>,
    pub approval_context: Vec<ApprovalReference>,
    #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
    pub resource_budget_digest: String,
    pub sandbox_profile: SandboxProfileIdentity,
    #[schemars(length(min = 1))]
    pub idempotency_key: String,
    pub repeatability: Repeatability,
    pub expected_effect_class: EffectClass,
}

/// Closed operation families supported by Protected Effect Request 0.1.0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OperationFamily {
    Filesystem,
    Process,
    Network,
    ExternalService,
}

impl OperationFamily {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Filesystem => "filesystem",
            Self::Process => "process",
            Self::Network => "network",
            Self::ExternalService => "external_service",
        }
    }
}

/// Bounded non-secret metadata describing external normalized input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InputMetadataEntry {
    #[schemars(length(min = 1, max = 64))]
    pub name: String,
    #[schemars(length(min = 1, max = 256))]
    pub value: String,
}

/// One exact resource boundary requested by a protected effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "scope_type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RequestedScope {
    Filesystem {
        #[schemars(length(min = 1))]
        root: String,
        #[schemars(length(min = 1))]
        access: Vec<FilesystemAccess>,
        recursive: bool,
    },
    Process {
        #[schemars(length(min = 1))]
        executable: String,
        #[schemars(length(min = 1))]
        working_directory: String,
    },
    Network {
        protocol: NetworkProtocol,
        #[schemars(length(min = 1))]
        host: String,
        #[schemars(range(min = 1))]
        port: u16,
    },
    ExternalService {
        #[schemars(length(min = 1))]
        service: String,
        #[schemars(length(min = 1))]
        operation: String,
        #[schemars(length(min = 1))]
        resource: String,
    },
}

/// Filesystem authority classes supported by the typed scope contract.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum FilesystemAccess {
    Read,
    Create,
    Modify,
    Delete,
}

/// Network protocols supported by the typed scope contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NetworkProtocol {
    Tcp,
    Udp,
    Http,
    Https,
}

/// How the future runtime may reason about repeating an identical request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Repeatability {
    Repeatable,
    Idempotent,
    NonRepeatable,
}

/// Whether the proposed effect is expected to observe or mutate its subject.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EffectClass {
    Observation,
    Mutation,
}

/// Exact sandbox profile requested for any eventual execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SandboxProfileIdentity {
    #[schemars(length(min = 1))]
    pub name: String,
    #[schemars(length(min = 1))]
    pub version: String,
    #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
    pub digest: String,
}

fn protected_effect_request_schema_version(_generator: &mut SchemaGenerator) -> Schema {
    serde_json::json!({
        "type": "string",
        "const": PROTECTED_EFFECT_REQUEST_SCHEMA,
    })
    .try_into()
    .expect("Protected Effect Request schema version must be valid JSON Schema")
}
