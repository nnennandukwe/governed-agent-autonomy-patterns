use super::{binding, profile_digest};
use crate::storage::linux::{bounded_flock, identity_matches, open_directory};
use crate::{Error, Host, Identity, Result, json_document};
use rustix::fs::{self, AtFlags, Mode, OFlags, ResolveFlags};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

#[derive(Clone, Copy)]
pub(crate) enum Role {
    Read,
    Convert,
    Reconcile,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rights {
    read: bool,
    convert: bool,
    reconcile: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Authority {
    format: String,
    workspace_id: String,
    store_id: String,
    reconciliation_profile_digest: String,
    principals: BTreeMap<String, Rights>,
}

fn unavailable() -> Error {
    Error::unavailable("operator_authority_unavailable")
}
fn forbidden() -> Error {
    Error::new("forbidden", "current_workspace_operator_access_denied")
}

fn directory(pin: &Identity) -> Result<File> {
    let dir = open_directory(Path::new(&pin.path)).map_err(|_| unavailable())?;
    let md = dir.metadata().map_err(|_| unavailable())?;
    if pin.mode != 0o700 || !md.is_dir() || !identity_matches(&md, pin) {
        return Err(unavailable());
    }
    Ok(dir)
}

fn regular(dir: &File, name: &str, uid: u32) -> Result<File> {
    let file = File::from(
        fs::openat2(
            dir,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_XDEV,
        )
        .map_err(|_| unavailable())?,
    );
    let md = file.metadata().map_err(|_| unavailable())?;
    if !md.is_file() || md.mode() & 0o7777 != 0o600 || md.nlink() != 1 || md.uid() != uid {
        return Err(unavailable());
    }
    Ok(file)
}

fn locked(host: &Host, name: &str, pin: &Identity, exclusive: bool) -> Result<(File, File)> {
    let parent = &binding(host)?.authority;
    if parent.dev == host.slot.workspace.dev
        || parent.uid != host.slot.workspace.uid
        || pin.uid != parent.uid
        || pin.dev != parent.dev
        || pin.mode != 0o600
        || Path::new(&parent.path).join(name) != Path::new(&pin.path)
    {
        return Err(unavailable());
    }
    let dir = directory(parent)?;
    let lock = regular(&dir, name, pin.uid)?;
    let md = lock.metadata().map_err(|_| unavailable())?;
    if !identity_matches(&md, pin) || md.len() != 0 {
        return Err(unavailable());
    }
    bounded_flock(&lock, exclusive).map_err(|e| match e {
        rustix::io::Errno::AGAIN => Error::new("busy", "supervisor_fence_busy"),
        _ => unavailable(),
    })?;
    let dir = directory(parent)?;
    let entry = fs::statat(&dir, name, AtFlags::SYMLINK_NOFOLLOW).map_err(|_| unavailable())?;
    if entry.st_dev != md.dev()
        || entry.st_ino != md.ino()
        || entry.st_nlink != 1
        || entry.st_uid != md.uid()
        || entry.st_mode != md.mode()
    {
        return Err(unavailable());
    }
    Ok((dir, lock))
}

/// Hold the supervisor's shared policy lock through the decision it authorized.
pub(crate) struct OperatorFence {
    _lock: File,
}
impl OperatorFence {
    pub(crate) fn acquire(host: &Host, role: Role) -> Result<Self> {
        if !host.can_read {
            return Err(forbidden());
        }
        let b = binding(host)?;
        let (dir, lock) = locked(host, "authority.lock", &b.authority_lock, false)?;
        let file = regular(&dir, "operator.json", b.authority.uid)?;
        const LIMIT: u64 = 64 * 1024;
        let mut bytes = Vec::new();
        file.take(LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| unavailable())?;
        if bytes.len() as u64 > LIMIT {
            return Err(unavailable());
        }
        let value = json_document(&bytes, true).map_err(|_| unavailable())?;
        let record: Authority = serde_json::from_value(value).map_err(|_| unavailable())?;
        if record.format != "gaap.reconciliation-authority/0.2"
            || record.workspace_id != host.slot.workspace_id
            || record.store_id != host.slot.store_id
            || record.reconciliation_profile_digest != profile_digest(host)?
            || record.principals.keys().any(|name| !crate::token(name))
        {
            return Err(unavailable());
        }
        let rights = record
            .principals
            .get(&host.principal)
            .ok_or_else(forbidden)?;
        if !rights.read
            || !match role {
                Role::Read => true,
                Role::Reconcile => rights.reconcile,
                Role::Convert => rights.convert,
            }
        {
            return Err(forbidden());
        }
        Ok(Self { _lock: lock })
    }
}

/// The supervisor puts every possible committer under this independent launch lease.
pub(crate) struct Quiescence {
    _lock: File,
}
impl Quiescence {
    pub(crate) fn acquire(host: &Host) -> Result<Self> {
        let (_, lock) = locked(host, "writers.lock", &binding(host)?.writers_lock, true)?;
        Ok(Self { _lock: lock })
    }
}
pub(crate) fn writer_lease(host: &Host) -> Result<Option<File>> {
    let Some(b) = &host.reconciliation else {
        return Ok(None);
    };
    Ok(Some(
        locked(host, "writers.lock", &b.writers_lock, false)?.1,
    ))
}

pub(crate) fn check_conversion(
    host: &Host,
    generation: u64,
    anchor: &str,
    journal: &str,
) -> Result<()> {
    let expected = binding(host)?.conversion.as_ref().ok_or_else(forbidden)?;
    if expected.source_generation != generation
        || expected.source_anchor_digest != anchor
        || expected.source_journal_digest != journal
    {
        return Err(Error::new("conflict", "conversion_source_changed"));
    }
    Ok(())
}
