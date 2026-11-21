use gaap::contracts::{
    AgentRunRequest, ApprovalReference, CapabilityIdentity, EffectUsage, EvidenceReference,
    ExecutorIdentity, PolicyIdentity, ProtectedEffectRequest, ProtectedEffectResult, ResourceUsage,
    RunEvent, SandboxProfileIdentity, Subject, TerminalRunReceipt,
};
use serde::{Deserialize, Serialize};

use crate::Identity;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoricalProfile {
    pub format: String,
    pub policies: Vec<PolicyIdentity>,
    pub capability: CapabilityIdentity,
    pub tool_schema_digest: String,
    pub sandbox: SandboxProfileIdentity,
    pub executor: ExecutorIdentity,
    pub implementer_id: String,
    pub verifier_id: String,
    pub grant_observer: String,
    pub effect_observer: String,
    pub require_grant: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceArchive {
    pub entries: Vec<ArchiveEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveEntry {
    pub locator: String,
    pub actor_id: String,
    pub bytes_hex: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Attempt {
    pub principal: String,
    pub key: String,
    pub attempt_id: String,
    pub proposal: Change,
    pub proposal_digest: String,
    pub run_request: AgentRunRequest,
    pub effect_request: ProtectedEffectRequest,
    pub facts: Vec<Fact>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Change {
    pub path: String,
    pub content: String,
    pub plan: String,
    pub base_digest: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Fact {
    pub binding: String,
    pub tag: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Intent {
    pub run_request_digest: String,
    pub effect_request_digest: String,
    pub proposed_artifact_digest: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FileObservation {
    pub subject: Subject,
    pub file: Identity,
    pub artifact_digest: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ObserverRef {
    pub actor_id: String,
    pub evidence: EvidenceReference,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GrantCheck {
    pub approval: ApprovalReference,
    pub source: ObserverRef,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Dispatch {
    pub before: FileObservation,
    pub executor: ExecutorIdentity,
    pub decisions: Vec<RunEvent>,
    pub checked_at_ms: u64,
    pub checks: Vec<GrantCheck>,
    pub reserved: EffectUsage,
    pub usage_floor: ResourceUsage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Disposition {
    Replaced,
    NotStarted,
    StoppedBeforeRename,
    Unresolved,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Observation {
    pub disposition: Disposition,
    pub after: Option<FileObservation>,
    pub source: Option<ObserverRef>,
    pub usage_floor: ResourceUsage,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Verification {
    pub event: RunEvent,
    pub usage_floor: ResourceUsage,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Publication {
    pub receipt: TerminalRunReceipt,
    pub result: Option<ProtectedEffectResult>,
}
