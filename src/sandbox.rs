//! Vendor-neutral host-owned execution profiles for protected effects.
//!
//! An admitted effect can select a profile; a profile never grants effect authority.
//! Runtime adapters supply observed identities and cleanup evidence after execution.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::contracts::Subject;

/// Immutable registry identity selected by the trusted host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxImage {
    pub reference: String,
    pub digest: String,
}

/// Explicit access to one host-owned input or output path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxMount {
    pub source: String,
    pub destination: String,
    pub writable: bool,
}

/// Network authority remains separate from a workload's requested command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum SandboxNetwork {
    Disabled,
    EndpointAllowlist { endpoints: Vec<String> },
}

/// Host bounds apply to the entire execution and retained output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxLimits {
    pub cpu_millis_per_second: u32,
    pub memory_bytes: u64,
    pub processes: u32,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub wall_clock_ms: u64,
    pub cleanup_ms: u64,
}

/// Collect only explicitly selected paths from a declared writable mount.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxOutputs {
    pub paths: Vec<String>,
    pub max_total_bytes: u64,
}

/// An execution binds either an admitted effect or an independent verification request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxInvocationKind {
    ProtectedEffect,
    Verification,
}

/// Serializable host adoption; Docker, Podman and runsc types stay in adapters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxRequest {
    pub schema_version: String,
    pub profile_id: String,
    pub invocation_kind: SandboxInvocationKind,
    pub invocation_digest: String,
    pub image: SandboxImage,
    pub command: Vec<String>,
    pub working_directory: String,
    pub user: String,
    pub environment: BTreeMap<String, String>,
    pub mounts: Vec<SandboxMount>,
    pub network: SandboxNetwork,
    pub limits: SandboxLimits,
    pub input_subject: Subject,
    pub outputs: SandboxOutputs,
}

/// Actual observed engine and low-level runtime identity, never a fallback claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxRuntimeIdentity {
    pub engine: String,
    pub engine_version: String,
    pub runtime: String,
    pub runtime_version: String,
    pub executable_digest: String,
}

/// Execution outcome and cleanup are independent facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxStatus {
    SetupFailed,
    Exited,
    Cancelled,
    TimedOut,
    OutputOverflow,
    ResourceLimit,
    RuntimeCrash,
    UnknownOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxCleanup {
    NotCreated,
    Removed,
    Unconfirmed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxArtifact {
    pub path: String,
    pub digest: String,
    pub bytes: Vec<u8>,
}

/// Raw output remains available for content-addressed Protected Effect evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxObservation {
    pub schema_version: String,
    pub request_digest: String,
    pub runtime: Option<SandboxRuntimeIdentity>,
    pub image_digest: Option<String>,
    pub resolved_image_id: Option<String>,
    pub owned_resource: Option<String>,
    pub intent_path: Option<String>,
    pub status: SandboxStatus,
    pub exit_code: Option<i64>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub stdout_digest: String,
    pub stderr_digest: String,
    pub artifacts: Vec<SandboxArtifact>,
    pub elapsed_ms: u64,
    pub cleanup: SandboxCleanup,
    pub reason: Option<String>,
    pub cleanup_reason: Option<String>,
    pub publication_error: Option<String>,
}

/// Only the outer engine decides whether an effect may reach this port.
pub trait SandboxAdapter {
    fn execute(
        &mut self,
        request: &SandboxRequest,
        cancelled: &dyn Fn() -> bool,
    ) -> SandboxObservation;
}

impl SandboxRequest {
    /// Host approval covers the environment and executable, independently of agent arguments.
    pub fn profile_digest(&self) -> Result<String, String> {
        use sha2::{Digest, Sha256};
        let profile = serde_json::json!({
            "schema_version":self.schema_version, "profile_id":self.profile_id,
            "image":self.image, "executable":self.command.first(),
            "working_directory":self.working_directory, "user":self.user,
            "environment":self.environment, "mounts":self.mounts, "network":self.network,
            "limits":self.limits, "outputs":self.outputs,
        });
        let bytes = serde_jcs::to_vec(&profile).map_err(|error| error.to_string())?;
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }
}
