use super::{binding, profile_digest, proof};
use crate::storage::linux::{Volume, mount_id};
use crate::{Error, Host, Identity, Result, digest, json_document};
use rustix::fs::Dir;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs::{File, Metadata};
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

const ITEM_LIMIT: u64 = 2 * 1024 * 1024;
const TOTAL_LIMIT: u64 = 32 * 1024 * 1024;
const COUNT_LIMIT: usize = 48;

fn invalid() -> Error {
    Error::invalid("invalid_observer_archive")
}

/// Inventory validates names and metadata. Candidate bytes are opened only for
/// an existing ruling or after the intake anchor has committed.
pub(crate) struct CandidateIndex {
    directory: File,
    pin: Identity,
    files: BTreeMap<String, Metadata>,
}
impl CandidateIndex {
    pub(crate) fn open(host: &Host, volume: &Volume<'_>) -> Result<Self> {
        let config = binding(host)?;
        profile_digest(host)?;
        let pin = &config.candidates;
        let path = Path::new(&pin.path);
        let slot = &host.slot;
        if pin.dev != slot.workspace.dev
            || pin.uid != slot.workspace.uid
            || [&slot.workspace, &slot.control, &slot.store]
                .into_iter()
                .chain(host.live.iter().map(|live| &live.archive))
                .any(|other| {
                    pin.ino == other.ino
                        || path.starts_with(&other.path)
                        || Path::new(&other.path).starts_with(path)
                })
        {
            return Err(invalid());
        }
        let directory = volume.directory(path)?;
        volume.check_pins(&directory, pin, true)?;
        if mount_id(&directory)? != slot.mount_id {
            return Err(invalid());
        }
        let mut files = BTreeMap::new();
        let mut total = 0u64;
        for entry in volume.io(Dir::read_from(&directory), false, "open_observer_archive")? {
            let entry = volume.io(entry, false, "list_observer_archive")?;
            let name = entry.file_name().to_str().map_err(|_| invalid())?;
            if matches!(name, "." | "..") {
                continue;
            }
            let hex = name
                .strip_suffix(".json")
                .filter(|id| crate::hex(id, 64))
                .ok_or_else(invalid)?;
            if files.len() >= COUNT_LIMIT {
                return Err(invalid());
            }
            let file = volume
                .existing(&directory, name, pin.uid)?
                .ok_or_else(invalid)?;
            let md = volume.io(file.metadata(), false, "stat_observer_record")?;
            if md.dev().to_string() != pin.dev || md.len() > ITEM_LIMIT {
                return Err(invalid());
            }
            total += md.len();
            if total > TOTAL_LIMIT || files.insert(format!("sha256:{hex}"), md).is_some() {
                return Err(invalid());
            }
        }
        Ok(Self {
            directory,
            pin: pin.clone(),
            files,
        })
    }
    pub(crate) fn contains(&self, hash: &str) -> bool {
        self.files.contains_key(hash)
    }

    pub(crate) fn assess(
        &self,
        host: &Host,
        volume: &Volume<'_>,
        attempt: &Value,
        hash: &str,
    ) -> Result<proof::Finding> {
        let md = self
            .files
            .get(hash)
            .ok_or_else(|| Error::unavailable("previously_selected_observer_record_missing"))?;
        let name = format!("{}.json", &hash[7..]);
        let path = volume.directory(Path::new(&self.pin.path))?;
        volume.check_pins(&path, &self.pin, true)?;
        volume.same_entry(&self.directory, &name, md)?;
        let file = volume
            .existing(&self.directory, &name, self.pin.uid)?
            .ok_or_else(invalid)?;
        let mut bytes = Vec::with_capacity(md.len() as usize);
        volume.io(
            (&file).take(ITEM_LIMIT + 1).read_to_end(&mut bytes),
            false,
            "read_observer_record",
        )?;
        volume.same_entry(&self.directory, &name, md)?;
        if bytes.len() as u64 != md.len() || digest(&bytes) != hash {
            return Err(invalid());
        }
        let raw = json_document(&bytes, true).map_err(|_| invalid())?;
        proof::assess(host, attempt, raw)
    }
}
