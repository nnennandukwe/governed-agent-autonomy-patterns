//! Provider-neutral, runtime-only static authorization evidence.

use std::fmt::Debug;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::contracts::{PolicyIdentity, ProtectedEffectRequest};
use crate::runtime::PolicyDecision;

/// One immutable evaluation, retained outside the frozen receipt envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaticPolicyEvaluation {
    pub effect_request_digest: String,
    pub policy: PolicyIdentity,
    pub schema: PolicyIdentity,
    pub decision: PolicyDecision,
    pub reason: String,
    pub determining_policies: Vec<String>,
    pub errors: Vec<String>,
}

impl StaticPolicyEvaluation {
    /// Content identity of the complete provider-neutral evidence record.
    pub fn evidence_digest(&self) -> String {
        let bytes = serde_jcs::to_vec(self).expect("static policy evidence contains only strings");
        format!("sha256:{:x}", Sha256::digest(bytes))
    }
}

/// Host-installed static policy evaluator. RunCoordinator still decides each gate.
pub trait StaticPolicyAdapter: Debug + Send + Sync {
    fn evaluate(&self, request: &ProtectedEffectRequest) -> StaticPolicyEvaluation;
}
