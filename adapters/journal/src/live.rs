//! Trusted host inputs for a single foreground shipping invocation.
use crate::Identity;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LiveBinding {
    pub can_submit: bool,
    pub authority: Identity,
    pub authority_lock: Identity,
    pub archive: Identity,
}

#[cfg(target_os = "linux")]
mod archive;
#[cfg(target_os = "linux")]
mod authority;
#[cfg(target_os = "linux")]
mod observation;
#[cfg(target_os = "linux")]
mod witness;
#[cfg(target_os = "linux")]
pub(crate) use archive::refresh_archive;
#[cfg(target_os = "linux")]
pub use archive::{retain_executor, retain_verifier};
#[cfg(target_os = "linux")]
pub use authority::{Admission, GrantEvidence, admit};
#[cfg(target_os = "linux")]
pub use witness::{EffectEvidence, Witness, witness_main};

#[cfg(target_os = "linux")]
fn binding(host: &crate::Host) -> crate::Result<&LiveBinding> {
    host.live
        .as_ref()
        .ok_or_else(|| crate::Error::new("forbidden", "live_host_not_provisioned"))
}

#[cfg(target_os = "linux")]
fn read_limit(file: &std::fs::File, limit: usize) -> std::io::Result<Vec<u8>> {
    use std::io::{Error, ErrorKind, Read};
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(Error::new(
            ErrorKind::InvalidData,
            "input exceeds its bound",
        ));
    }
    Ok(bytes)
}
