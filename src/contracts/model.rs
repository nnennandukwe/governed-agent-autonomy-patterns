use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};

use crate::{Decision, Gate};

pub const AGENT_RUN_REQUEST_SCHEMA: &str = "gaap.agent-run-request/0.1.0";
pub const TERMINAL_RUN_RECEIPT_SCHEMA: &str = "gaap.terminal-run-receipt/0.1.0";

/// Immutable input that identifies one governed Agent Run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentRunRequest {
    #[schemars(schema_with = "agent_run_request_schema_version")]
    pub schema_version: String,
    #[schemars(length(min = 1))]
    pub request_id: String,
    #[schemars(length(min = 1))]
    pub run_id: String,
    pub subject: Subject,
    pub requested_capability: CapabilityIdentity,
    pub task: TaskSpec,
    #[schemars(length(min = 1))]
    pub policies: Vec<PolicyIdentity>,
    pub resource_budget: ResourceBudget,
    pub approval_context: Vec<ApprovalReference>,
    pub required_verification: VerificationRequirement,
}

/// Exact repository or artifact that the Agent Run may inspect or change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Subject {
    pub kind: SubjectKind,
    #[schemars(length(min = 1))]
    pub locator: String,
    #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
    pub digest: String,
}

/// Closed subject categories supported by contract version 0.1.0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubjectKind {
    Repository,
    Artifact,
}

/// Requested engineering capability bound to its versioned content digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CapabilityIdentity {
    #[schemars(length(min = 1))]
    pub name: String,
    #[schemars(length(min = 1))]
    pub version: String,
    #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
    pub digest: String,
}

/// Self-contained task instructions and ordered textual constraints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskSpec {
    #[schemars(length(min = 1))]
    pub instructions: String,
    #[schemars(inner(length(min = 1)))]
    pub constraints: Vec<String>,
}

/// Exact policy identity accepted by a caller's [`super::ContractSupport`].
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct PolicyIdentity {
    #[schemars(length(min = 1))]
    pub name: String,
    #[schemars(length(min = 1))]
    pub version: String,
    #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
    pub digest: String,
}

/// Upper bounds declared for one Agent Run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResourceBudget {
    #[schemars(range(max = 9007199254740991_u64))]
    pub max_cost_micros: u64,
    #[schemars(range(max = 9007199254740991_u64))]
    pub max_elapsed_ms: u64,
    #[schemars(range(max = 9007199254740991_u64))]
    pub max_model_tokens: u64,
    #[schemars(range(max = 9007199254740991_u64))]
    pub max_tool_calls: u64,
}

/// Existing approval evidence bound to an actor, scope, and exact subject.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApprovalReference {
    #[schemars(length(min = 1))]
    pub approval_id: String,
    #[schemars(length(min = 1))]
    pub actor_id: String,
    #[schemars(length(min = 1))]
    pub scope: String,
    #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
    pub subject_digest: String,
    pub evidence: EvidenceReference,
}

/// Content-addressed evidence metadata; raw evidence remains external.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EvidenceReference {
    pub evidence_type: EvidenceType,
    #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
    pub digest: String,
    #[schemars(inner(length(min = 1)))]
    pub locator: Option<String>,
}

/// Closed evidence categories supported by contract version 0.1.0.
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

/// Independent evidence required before an Agent Run may complete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VerificationRequirement {
    pub independence: VerificationIndependence,
    #[schemars(length(min = 1))]
    pub evidence_types: Vec<EvidenceType>,
}

/// Actor-separation rules supported by contract version 0.1.0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VerificationIndependence {
    DifferentActor,
}

/// Lifecycle states for one Agent Run, including four distinct terminal states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentRunStatus {
    Accepted,
    Planning,
    AwaitingAuthority,
    Executing,
    Verifying,
    Completed,
    Blocked,
    Failed,
    Interrupted,
}

impl AgentRunStatus {
    /// Return whether no later lifecycle transition is permitted.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Blocked | Self::Failed | Self::Interrupted
        )
    }
}

/// Cumulative resource usage observed during one Agent Run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResourceUsage {
    #[schemars(range(max = 9007199254740991_u64))]
    pub cost_micros: u64,
    #[schemars(range(max = 9007199254740991_u64))]
    pub elapsed_ms: u64,
    #[schemars(range(max = 9007199254740991_u64))]
    pub model_tokens: u64,
    #[schemars(range(max = 9007199254740991_u64))]
    pub tool_calls: u64,
}

/// Independent verification result recorded in the receipt ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "UPPERCASE")]
pub enum VerificationVerdict {
    Pass,
    Fail,
}

/// One entry in the immutable, sequence-ordered Terminal Run Receipt ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "event_type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RunEvent {
    StatusTransition {
        #[schemars(range(min = 1, max = 9007199254740991_u64))]
        sequence: u64,
        from: AgentRunStatus,
        to: AgentRunStatus,
        reason: Option<String>,
    },
    PlanRecorded {
        #[schemars(range(min = 1, max = 9007199254740991_u64))]
        sequence: u64,
        #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
        plan_digest: String,
    },
    ApprovalRecorded {
        #[schemars(range(min = 1, max = 9007199254740991_u64))]
        sequence: u64,
        approval: ApprovalReference,
    },
    ProtectedEffectDecision {
        #[schemars(range(min = 1, max = 9007199254740991_u64))]
        sequence: u64,
        #[schemars(length(min = 1))]
        decision_id: String,
        gate: Gate,
        #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
        protected_effect_digest: String,
        #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
        subject_digest: String,
        decision: Decision,
    },
    ToolExecution {
        #[schemars(range(min = 1, max = 9007199254740991_u64))]
        sequence: u64,
        #[schemars(length(min = 1))]
        decision_id: String,
        #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
        protected_effect_digest: String,
        #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
        action_digest: String,
        #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
        capability_digest: String,
        #[schemars(length(min = 1))]
        evidence: Vec<EvidenceReference>,
    },
    Mutation {
        #[schemars(range(min = 1, max = 9007199254740991_u64))]
        sequence: u64,
        #[schemars(length(min = 1))]
        decision_id: String,
        #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
        protected_effect_digest: String,
        #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
        before_subject_digest: String,
        #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
        after_subject_digest: String,
        #[schemars(length(min = 1))]
        evidence: Vec<EvidenceReference>,
    },
    Verification {
        #[schemars(range(min = 1, max = 9007199254740991_u64))]
        sequence: u64,
        #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
        subject_digest: String,
        #[schemars(length(min = 1))]
        implementer_id: String,
        #[schemars(length(min = 1))]
        verifier_id: String,
        verdict: VerificationVerdict,
        #[schemars(length(min = 1))]
        evidence: Vec<EvidenceReference>,
    },
    Usage {
        #[schemars(range(min = 1, max = 9007199254740991_u64))]
        sequence: u64,
        usage: ResourceUsage,
    },
    Interruption {
        #[schemars(range(min = 1, max = 9007199254740991_u64))]
        sequence: u64,
        #[schemars(inner(length(min = 1)))]
        actor_id: Option<String>,
        #[schemars(length(min = 1))]
        reason: String,
        evidence: EvidenceReference,
    },
}

impl RunEvent {
    /// Return the one-based position declared by this event.
    pub fn sequence(&self) -> u64 {
        match self {
            Self::StatusTransition { sequence, .. }
            | Self::PlanRecorded { sequence, .. }
            | Self::ApprovalRecorded { sequence, .. }
            | Self::ProtectedEffectDecision { sequence, .. }
            | Self::ToolExecution { sequence, .. }
            | Self::Mutation { sequence, .. }
            | Self::Verification { sequence, .. }
            | Self::Usage { sequence, .. }
            | Self::Interruption { sequence, .. } => *sequence,
        }
    }
}

/// Canonical digest preimage containing the terminal Agent Run evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TerminalRunReceiptBody {
    #[schemars(schema_with = "terminal_run_receipt_schema_version")]
    pub schema_version: String,
    #[schemars(length(min = 1))]
    pub request_id: String,
    #[schemars(length(min = 1))]
    pub run_id: String,
    #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
    pub request_digest: String,
    #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
    pub initial_subject_digest: String,
    #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
    pub resulting_subject_digest: String,
    pub terminal_status: AgentRunStatus,
    #[schemars(length(min = 1))]
    pub terminal_reason: String,
    pub usage: ResourceUsage,
    #[schemars(length(min = 1))]
    pub events: Vec<RunEvent>,
}

/// Content-addressed envelope around a canonical Terminal Run Receipt body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TerminalRunReceipt {
    #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
    pub receipt_digest: String,
    pub body: TerminalRunReceiptBody,
}

fn agent_run_request_schema_version(_: &mut SchemaGenerator) -> Schema {
    schemars::json_schema!({
        "type": "string",
        "const": AGENT_RUN_REQUEST_SCHEMA
    })
}

fn terminal_run_receipt_schema_version(_: &mut SchemaGenerator) -> Schema {
    schemars::json_schema!({
        "type": "string",
        "const": TERMINAL_RUN_RECEIPT_SCHEMA
    })
}
