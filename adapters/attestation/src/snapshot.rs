//! Portable working-tree metadata excludes host device/inode/path identities.
//! File-content digests need separately retained content-addressed bytes.
use crate::{AttestationError, error};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const SNAPSHOT_PROFILE: &str = "gaap.working-tree-snapshot/1";
pub const MAX_ENTRIES: usize = 65; // The root plus 64 entries beneath it.
pub const MAX_DEPTH: usize = 8;
pub const MAX_FILE_BYTES: usize = 8192;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SnapshotEntry {
    Directory {
        #[schemars(range(max = 4095))]
        mode: u32,
    },
    File {
        #[schemars(range(max = 4095))]
        mode: u32,
        #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
        digest: String,
    },
}

/// Only validated relative paths and metadata can construct this manifest.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct SnapshotManifest(BTreeMap<String, SnapshotEntry>);
impl SnapshotManifest {
    pub fn from_entries(
        entries: BTreeMap<String, SnapshotEntry>,
    ) -> Result<Self, AttestationError> {
        if entries.is_empty()
            || entries.len() > MAX_ENTRIES
            || !matches!(entries.get("."), Some(SnapshotEntry::Directory { .. }))
        {
            return Err(error("snapshot.invalid_root"));
        }
        for (path, entry) in &entries {
            if path != "."
                && (path.is_empty()
                    || path.len() > 1024
                    || path.starts_with('/')
                    || path == ".git"
                    || path.starts_with(".git/")
                    || path.contains('\\')
                    || path.contains('\0')
                    || path.split('/').count() > MAX_DEPTH
                    || path
                        .split('/')
                        .any(|part| part.is_empty() || matches!(part, "." | "..")))
            {
                return Err(error("snapshot.invalid_path"));
            }
            let mode = match entry {
                SnapshotEntry::Directory { mode } | SnapshotEntry::File { mode, .. } => *mode,
            };
            if mode > 0o7777 {
                return Err(error("snapshot.invalid_mode"));
            }
            if let SnapshotEntry::File { digest, .. } = entry {
                validate_digest(digest)?;
            }
            if path != "." {
                let parent = path.rsplit_once('/').map_or(".", |(parent, _)| parent);
                if !matches!(entries.get(parent), Some(SnapshotEntry::Directory { .. })) {
                    return Err(error("snapshot.missing_parent"));
                }
            }
        }
        Ok(Self(entries))
    }
    pub fn parse(bytes: &[u8]) -> Result<Self, AttestationError> {
        let entries = serde_json::from_value(crate::json::parse(bytes)?)
            .map_err(|_| error("snapshot.invalid_manifest"))?;
        Self::from_entries(entries)
    }
    pub fn entries(&self) -> &BTreeMap<String, SnapshotEntry> {
        &self.0
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, AttestationError> {
        serde_jcs::to_vec(&self.0).map_err(|_| error("snapshot.invalid_manifest"))
    }
    pub fn digest(&self) -> Result<String, AttestationError> {
        Ok(content_digest(&self.canonical_bytes()?))
    }
}
pub fn content_digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
pub(crate) fn validate_digest(digest: &str) -> Result<(), AttestationError> {
    if digest.len() != 71
        || !digest.starts_with("sha256:")
        || !digest[7..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(error("attestation.invalid_digest"));
    }
    Ok(())
}
