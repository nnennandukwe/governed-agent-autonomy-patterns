use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const AGENT_RUN_REQUEST_SCHEMA: &str = "gaap.agent-run-request/0.1.0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentRunRequest {
    pub schema_version: String,
    pub request_id: String,
    pub run_id: String,
    pub subject: Subject,
    pub requested_capability: CapabilityIdentity,
    pub task: TaskSpec,
    pub policies: Vec<PolicyIdentity>,
    pub resource_budget: ResourceBudget,
    pub approval_context: Vec<ApprovalReference>,
    pub required_verification: VerificationRequirement,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Subject {
    pub kind: SubjectKind,
    pub locator: String,
    pub digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubjectKind {
    Repository,
    Artifact,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapabilityIdentity {
    pub name: String,
    pub version: String,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskSpec {
    pub instructions: String,
    pub constraints: Vec<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct PolicyIdentity {
    pub name: String,
    pub version: String,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResourceBudget {
    pub max_cost_micros: u64,
    pub max_elapsed_ms: u64,
    pub max_model_tokens: u64,
    pub max_tool_calls: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApprovalReference {
    pub approval_id: String,
    pub actor_id: String,
    pub scope: String,
    pub subject_digest: String,
    pub evidence: EvidenceReference,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EvidenceReference {
    pub evidence_type: EvidenceType,
    pub digest: String,
    pub locator: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceType {
    Approval,
    CommandOutput,
    Artifact,
    ToolExecution,
    VerificationAttestation,
    ResourceUsage,
    Interruption,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VerificationRequirement {
    pub independence: VerificationIndependence,
    pub evidence_types: Vec<EvidenceType>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VerificationIndependence {
    DifferentActor,
}
