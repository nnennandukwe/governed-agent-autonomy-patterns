//! Supervisor-provisioned authority and immutable host observer records for v0.2.
use crate::Identity;
#[cfg(target_os = "linux")]
use crate::{Error, Host, Result, canonical, digest};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub format: String,
    pub workspace_id: String,
    pub store_id: String,
    pub profile_digest: String,
    pub effect_observer: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversionSource {
    pub source_generation: u64,
    pub source_anchor_digest: String,
    pub source_journal_digest: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub profile: Profile,
    pub candidates: Identity,
    pub authority: Identity,
    pub authority_lock: Identity,
    pub writers_lock: Identity,
    pub conversion: Option<ConversionSource>,
    pub entry_socket_cookie: Option<String>,
}

#[cfg(target_os = "linux")]
pub(crate) fn binding(host: &Host) -> Result<&Binding> {
    host.reconciliation
        .as_ref()
        .ok_or_else(|| Error::unavailable("reconciliation_host_not_provisioned"))
}

#[cfg(target_os = "linux")]
pub(crate) fn profile_digest(host: &Host) -> Result<String> {
    let profile = &binding(host)?.profile;
    if profile.format != "gaap.shipping-reconciliation/0.2"
        || profile.workspace_id != host.slot.workspace_id
        || profile.store_id != host.slot.store_id
        || profile.profile_digest != host.slot.profile_digest
        || profile.effect_observer != host.profile.effect_observer
        || profile.effect_observer == host.profile.implementer_id
        || profile.effect_observer == host.profile.executor.name
    {
        return Err(Error::invalid("reconciliation_profile_binding"));
    }
    Ok(digest(&canonical(profile)?))
}

#[cfg(target_os = "linux")]
mod archive;
#[cfg(target_os = "linux")]
mod authority;
#[cfg(target_os = "linux")]
mod gate;
#[cfg(target_os = "linux")]
mod proof;
#[cfg(target_os = "linux")]
pub(crate) use archive::CandidateIndex;
#[cfg(target_os = "linux")]
pub(crate) use authority::{OperatorFence, Quiescence, Role, check_conversion, writer_lease};
#[cfg(target_os = "linux")]
pub use gate::{enter, seal_gate};
