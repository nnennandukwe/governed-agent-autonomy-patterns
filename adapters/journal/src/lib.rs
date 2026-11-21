#[cfg(target_os = "linux")]
pub mod handoff;
pub mod history;
pub mod live;
#[cfg(target_os = "linux")]
pub use storage::writer;
mod json;
#[cfg(target_os = "linux")]
mod storage;

pub use json::json_document;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const MAX_SAFE: u64 = 9_007_199_254_740_991;
pub const JOURNAL_LIMIT: usize = 8 * 1024 * 1024;
pub const PROPOSAL_LIMIT: usize = 256 * 1024;
pub const ANCHOR_LIMIT: usize = 16 * 1024;
pub const ATTEMPT_LIMIT: usize = 12;
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Serialize)]
pub struct Error {
    pub status: &'static str,
    pub reason: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<&'static str>,
}

impl Error {
    pub fn new(status: &'static str, reason: &'static str) -> Self {
        Self {
            status,
            reason,
            warning: None,
        }
    }
    pub fn with_warning(mut self, warning: &'static str) -> Self {
        self.warning = Some(warning);
        self
    }
    pub fn invalid(reason: &'static str) -> Self {
        Self::new("invalid_store", reason)
    }
    pub fn unavailable(reason: &'static str) -> Self {
        Self::new("unavailable", reason)
    }
    pub fn unsupported(reason: &'static str) -> Self {
        Self::new("unsupported_storage", reason)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub path: String,
    pub dev: String,
    pub ino: String,
    pub uid: u32,
    pub mode: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Slot {
    pub workspace_id: String,
    pub store_id: String,
    pub profile_digest: String,
    pub workspace: Identity,
    pub control: Identity,
    pub store: Identity,
    pub lock: Identity,
    pub health: Identity,
    pub mount_id: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Host {
    pub slot: Slot,
    pub principal: String,
    pub can_enroll: bool,
    pub can_read: bool,
    pub profile: history::HistoricalProfile,
    pub archive: history::EvidenceArchive,
    #[serde(default)]
    pub live: Option<live::LiveBinding>,
}

#[derive(Debug)]
pub enum Request {
    Enroll,
    Lookup {
        key: String,
        proposal: Option<Value>,
    },
}

#[derive(Debug, Serialize)]
pub struct Lookup {
    pub status: &'static str,
    pub workspace_hold: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt: Option<Value>,
}

impl Lookup {
    #[cfg(target_os = "linux")]
    fn at_generation(self, generation: u64) -> Result<Value> {
        let mut answer =
            serde_json::to_value(self).map_err(|_| Error::invalid("lookup_encoding"))?;
        answer["generation"] = json!(generation);
        Ok(answer)
    }
}

pub fn token(s: &str) -> bool {
    (1..=128).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

fn hex(s: &str, len: usize) -> bool {
    s.len() == len
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn is_digest(s: &str) -> bool {
    s.strip_prefix("sha256:").is_some_and(|raw| hex(raw, 64))
}

pub fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
pub fn canonical<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    serde_jcs::to_vec(value).map_err(|_| Error::invalid("canonical encoding failed"))
}

pub fn limits() -> Value {
    json!({"anchor_bytes":ANCHOR_LIMIT,"journal_bytes":JOURNAL_LIMIT,"attempts":ATTEMPT_LIMIT,
           "raw_proposal_bytes":PROPOSAL_LIMIT,"canonical_proposal_bytes":PROPOSAL_LIMIT,"depth":64})
}

impl Host {
    pub fn validate(&self) -> Result<()> {
        if !token(&self.principal)
            || !hex(&self.slot.store_id, 32)
            || !hex(&self.slot.workspace_id, 32)
            || digest(&canonical(&self.profile)?) != self.slot.profile_digest
        {
            return Err(Error::invalid("invalid trusted principal or slot binding"));
        }
        self.profile.validate()?;
        self.archive.validate()?;
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    format: String,
    store_id: String,
    workspace_id: String,
    profile_digest: String,
    generation: u64,
    attempts: Vec<Value>,
}

pub fn empty_journal(host: &Host) -> Result<Vec<u8>> {
    canonical(&Journal {
        format: "gaap.journal/0.1".into(),
        store_id: host.slot.store_id.clone(),
        workspace_id: host.slot.workspace_id.clone(),
        profile_digest: host.slot.profile_digest.clone(),
        generation: 0,
        attempts: vec![],
    })
}

pub fn interpret(host: &Host, bytes: &[u8], request: &Request) -> Result<(u64, Lookup)> {
    interpret_with_archive(host, &host.archive, bytes, request)
}

fn interpret_with_archive(
    host: &Host,
    archive: &history::EvidenceArchive,
    bytes: &[u8],
    request: &Request,
) -> Result<(u64, Lookup)> {
    if bytes.len() > JOURNAL_LIMIT {
        return Err(Error::invalid("journal exceeds enrolled capacity"));
    }
    let value = json_document(bytes, true)?;
    if value
        .get("format")
        .and_then(Value::as_str)
        .is_some_and(|f| f != "gaap.journal/0.1")
    {
        return Err(Error::unsupported("unsupported journal format"));
    }
    let journal: Journal =
        serde_json::from_value(value).map_err(|_| Error::invalid("invalid journal shape"))?;
    if journal.store_id != host.slot.store_id
        || journal.workspace_id != host.slot.workspace_id
        || journal.profile_digest != host.slot.profile_digest
        || journal.attempts.len() > ATTEMPT_LIMIT
        || journal.generation > MAX_SAFE
        || (journal.generation == 0) != journal.attempts.is_empty()
    {
        return Err(Error::invalid(
            "invalid journal identity, generation or capacity",
        ));
    }
    let ctx = history::HistoryContext {
        workspace_id: &host.slot.workspace_id,
        store_id: &host.slot.store_id,
        profile_digest: &host.slot.profile_digest,
        workspace: &host.slot.workspace,
        profile: &host.profile,
        archive,
    };
    let (key, proposal) = match request {
        Request::Enroll => ("enrollment-check", None),
        Request::Lookup { key, proposal } => (key.as_str(), proposal.as_ref()),
    };
    let lookup = history::lookup(&journal.attempts, &ctx, &host.principal, key, proposal)?;
    Ok((journal.generation, lookup))
}

/// The supervisor supplies the authenticated host configuration and independent health file.
/// User data can select only a lookup key and optional proposal.
#[cfg(target_os = "linux")]
pub fn execute(host: &Host, health: &std::fs::File, request: &Request) -> Result<Value> {
    let permitted = match request {
        Request::Enroll => host.can_enroll,
        Request::Lookup { .. } => host.can_read,
    };
    if !permitted {
        return Err(Error::new("forbidden", "current host permission denied"));
    }
    host.validate()?;
    if let Request::Lookup { key, proposal } = request {
        validate_key_and_proposal(key, proposal.as_ref())?;
    }
    storage::run(host, health, request)
}

#[cfg(target_os = "linux")]
fn validate_key_and_proposal(key: &str, proposal: Option<&Value>) -> Result<()> {
    if !token(key) {
        return Err(Error::new("invalid_request", "invalid submission key"));
    }
    if let Some(proposal) = proposal {
        history::validate_proposal(proposal)
            .map_err(|_| Error::new("invalid_request", "invalid local file proposal"))?;
        if canonical(proposal)?.len() > PROPOSAL_LIMIT {
            return Err(Error::new(
                "capacity",
                "proposal exceeds its enrolled byte limit",
            ));
        }
    }
    Ok(())
}
