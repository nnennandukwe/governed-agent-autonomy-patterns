use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};

use super::super::model::{ApprovalReference, CapabilityIdentity, PolicyIdentity, Subject};
use crate::{Decision, Gate};

pub const PROTECTED_EFFECT_REQUEST_SCHEMA: &str = "gaap.protected-effect-request/0.1.0";
pub const PROTECTED_EFFECT_RESULT_SCHEMA: &str = "gaap.protected-effect-result/0.1.0";

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

/// The decisive RunCoordinator record bound to an exact effect request and subject.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProtectedEffectDecision {
    #[schemars(length(min = 1))]
    pub decision_id: String,
    pub gate: Gate,
    #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
    pub effect_request_digest: String,
    #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
    pub subject_digest: String,
    pub decision: Decision,
}

/// Closed execution outcomes supported by Protected Effect Result 0.1.0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EffectExecutionStatus {
    Executed,
    AwaitingAuthority,
    Denied,
    Failed,
    Interrupted,
    UnknownOutcome,
}

/// A known process exit, represented without provider-specific status types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "exit_type", rename_all = "snake_case", deny_unknown_fields)]
pub enum EffectExit {
    Code {
        code: i32,
    },
    Signal {
        #[schemars(length(min = 1))]
        signal: String,
    },
}

/// Exact executor identity observed for an attempted protected effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutorIdentity {
    #[schemars(length(min = 1))]
    pub name: String,
    #[schemars(length(min = 1))]
    pub version: String,
    #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
    pub digest: String,
}

/// Closed evidence categories supported by Protected Effect Result 0.1.0.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum EffectEvidenceType {
    Exit,
    Output,
    Artifact,
    Mutation,
    Usage,
    Executor,
    Sandbox,
    Failure,
    Interruption,
    SubjectObservation,
    CapabilitySchema,
    UnknownOutcome,
}

/// Content-addressed effect evidence metadata; raw evidence remains external.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EffectEvidenceReference {
    pub evidence_type: EffectEvidenceType,
    #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
    pub digest: String,
    #[schemars(inner(length(min = 1)))]
    pub locator: Option<String>,
}

/// Resource usage attributable only to this protected effect attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EffectUsage {
    #[schemars(range(max = 9007199254740991_u64))]
    pub cost_micros: u64,
    #[schemars(range(max = 9007199254740991_u64))]
    pub elapsed_ms: u64,
    #[schemars(range(max = 9007199254740991_u64))]
    pub model_tokens: u64,
    #[schemars(range(max = 9007199254740991_u64))]
    pub tool_calls: u64,
}

/// Canonical digest preimage containing one protected effect result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProtectedEffectResultBody {
    #[schemars(schema_with = "protected_effect_result_schema_version")]
    pub schema_version: String,
    #[schemars(length(min = 1))]
    pub effect_id: String,
    #[schemars(range(min = 1, max = 9007199254740991_u64))]
    pub effect_sequence: u64,
    #[schemars(length(min = 1))]
    pub run_id: String,
    #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
    pub agent_run_request_digest: String,
    #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
    pub effect_request_digest: String,
    pub observed_pre_effect_subject: Subject,
    pub observed_capability: CapabilityIdentity,
    #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
    pub observed_tool_schema_digest: Option<String>,
    pub decision: ProtectedEffectDecision,
    pub execution_status: EffectExecutionStatus,
    pub observed_post_effect_subject: Option<Subject>,
    pub exit: Option<EffectExit>,
    pub usage: EffectUsage,
    pub executor: Option<ExecutorIdentity>,
    pub sandbox_profile: Option<SandboxProfileIdentity>,
    #[schemars(inner(length(min = 1)))]
    pub reason: Option<String>,
    pub evidence: Vec<EffectEvidenceReference>,
}

/// Content-addressed envelope around a canonical Protected Effect Result body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProtectedEffectResult {
    #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
    pub result_digest: String,
    pub body: ProtectedEffectResultBody,
}

fn protected_effect_request_schema_version(_generator: &mut SchemaGenerator) -> Schema {
    serde_json::json!({
        "type": "string",
        "const": PROTECTED_EFFECT_REQUEST_SCHEMA,
    })
    .try_into()
    .expect("Protected Effect Request schema version must be valid JSON Schema")
}

fn protected_effect_result_schema_version(_generator: &mut SchemaGenerator) -> Schema {
    serde_json::json!({
        "type": "string",
        "const": PROTECTED_EFFECT_RESULT_SCHEMA,
    })
    .try_into()
    .expect("Protected Effect Result schema version must be valid JSON Schema")
}
