use super::{binding, read_limit};
use crate::history::{ArchiveEntry, EvidenceArchive};
use crate::storage::linux::{Volume, checked_close, mount_id};
use crate::storage::publication::point;
use crate::writer::Submission;
use crate::{ATTEMPT_LIMIT, Error, Host, Result, canonical, digest, is_digest, json_document};
use gaap::contracts::RunEvent;
use rustix::fs::{self, Dir, Mode, OFlags, RenameFlags};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

const FORMAT: &str = "gaap.shipping-evidence/0.1";
const ITEM_LIMIT: usize = 2 * 1024 * 1024;
const ARCHIVE_LIMIT: usize = 32 * 1024 * 1024;
const MAX_ITEMS: usize = 4 * ATTEMPT_LIMIT;
const FAMILIES: [&str; 4] = ["grant", "effect", "executor", "verifier"];

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Capsule {
    format: String,
    binding: String,
    family: String,
    entries: Vec<ArchiveEntry>,
    source: Value,
}

fn invalid() -> Error {
    Error::invalid("invalid_independent_evidence_archive")
}
fn full() -> Error {
    Error::new("capacity", "live_evidence_archive_full")
}
fn tail(digest: &str) -> &str {
    &digest["sha256:".len()..]
}
fn bytes_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        write!(s, "{b:02x}").expect("String writes are infallible");
    }
    s
}
pub(super) fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    fn digit(x: u8) -> Option<u8> {
        match x {
            b'0'..=b'9' => Some(x - b'0'),
            b'a'..=b'f' => Some(x - b'a' + 10),
            _ => None,
        }
    }
    s.as_bytes()
        .chunks_exact(2)
        .map(|p| Some(digit(p[0])? * 16 + digit(p[1])?))
        .collect()
}
pub(super) fn entry(actor: &str, locator: &str, bytes: &[u8]) -> ArchiveEntry {
    ArchiveEntry {
        actor_id: actor.into(),
        locator: locator.into(),
        bytes_hex: bytes_hex(bytes),
    }
}

fn archive_dir(host: &Host, vol: &Volume<'_>) -> Result<File> {
    let pin = &binding(host)?.archive;
    let path = Path::new(&pin.path);
    let slot = &host.slot;
    if pin.dev != slot.workspace.dev
        || pin.uid != slot.workspace.uid
        || pin.mode != 0o700
        || [&slot.workspace, &slot.control, &slot.store]
            .iter()
            .any(|p| {
                p.ino == pin.ino
                    || path.starts_with(&p.path)
                    || Path::new(&p.path).starts_with(path)
            })
    {
        return Err(invalid());
    }
    let dir = vol.directory(path)?;
    vol.check_pins(&dir, pin, true)?;
    if mount_id(&dir)? != slot.mount_id {
        return Err(invalid());
    }
    Ok(dir)
}

struct Inventory {
    archive: EvidenceArchive,
    bytes: usize,
    capsules: BTreeSet<(String, String)>,
}

fn inventory(host: &Host, vol: &Volume<'_>, dir: &File) -> Result<Inventory> {
    let mut archive = host.archive.clone();
    let mut indexed = BTreeSet::new();
    for e in &archive.entries {
        let bytes = decode_hex(&e.bytes_hex).ok_or_else(invalid)?;
        indexed.insert((e.actor_id.clone(), e.locator.clone(), digest(&bytes)));
    }
    let mut capsules = BTreeSet::new();
    let mut total = 0usize;
    let mut count = 0;
    for entry in vol.io(Dir::read_from(dir), false, "read_archive_dir")? {
        let e = vol.io(entry, false, "read_archive_entry")?;
        let name = e.file_name().to_str().map_err(|_| invalid())?;
        if name == "." || name == ".." {
            continue;
        }
        count += 1;
        if count > MAX_ITEMS * 2 {
            return Err(invalid());
        }
        let parts = name.split('.').collect::<Vec<_>>();
        if let ["stage", hex, family] = parts.as_slice()
            && crate::hex(hex, 64)
            && FAMILIES.contains(family)
        {
            // An unpublished stage can survive a stopped producer. It's never evidence.
            continue;
        }
        let [binding, family, hash, "json"] = parts.as_slice() else {
            return Err(invalid());
        };
        if !crate::hex(binding, 64)
            || !FAMILIES.contains(family)
            || !crate::hex(hash, 64)
            || !capsules.insert(((*binding).into(), (*family).into()))
            || capsules.len() > MAX_ITEMS
        {
            return Err(invalid());
        }
        let file = vol
            .existing(dir, name, host.slot.workspace.uid)?
            .ok_or_else(invalid)?;
        let length = vol.io(file.metadata(), false, "archive_file_stat")?.len();
        if length > ITEM_LIMIT as u64 || length as usize > ARCHIVE_LIMIT - total {
            return Err(invalid());
        }
        let bytes = vol.io(read_limit(&file, length as usize), false, "archive_read")?;
        if bytes.len() != length as usize || tail(&digest(&bytes)) != *hash {
            return Err(invalid());
        }
        total += bytes.len();
        let capsule: Capsule =
            serde_json::from_value(json_document(&bytes, true)?).map_err(|_| invalid())?;
        if capsule.format != FORMAT
            || capsule.binding != format!("sha256:{binding}")
            || capsule.family != *family
        {
            return Err(invalid());
        }
        let actor = match *family {
            "grant" => &host.profile.grant_observer,
            "effect" => &host.profile.effect_observer,
            "executor" => &host.profile.executor.name,
            "verifier" => &host.profile.verifier_id,
            _ => unreachable!(),
        };
        for entry in capsule.entries {
            if &entry.actor_id != actor || entry.locator.trim().is_empty() {
                return Err(invalid());
            }
            let source = decode_hex(&entry.bytes_hex).ok_or_else(invalid)?;
            if indexed.insert((
                entry.actor_id.clone(),
                entry.locator.clone(),
                digest(&source),
            )) {
                archive.entries.push(entry);
            }
        }
    }
    Ok(Inventory {
        archive,
        bytes: total,
        capsules,
    })
}

pub(crate) fn refresh_archive(host: &Host, volume: &Volume<'_>) -> Result<EvidenceArchive> {
    if host.live.is_none() {
        return Ok(host.archive.clone());
    }
    let dir = archive_dir(host, volume)?;
    Ok(inventory(host, volume, &dir)?.archive)
}

pub(super) fn retain(
    tx: &Submission<'_>,
    binding: &str,
    family: &str,
    entries: Vec<ArchiveEntry>,
    source: Value,
) -> Result<()> {
    let host = tx.host();
    let volume = tx.volume();
    if !is_digest(binding) || !FAMILIES.contains(&family) {
        return Err(invalid());
    }
    let dir = archive_dir(host, volume)?;
    let listing = inventory(host, volume, &dir)?;
    let bytes = canonical(&Capsule {
        format: FORMAT.into(),
        binding: binding.into(),
        family: family.into(),
        entries,
        source,
    })?;
    let hex = tail(binding);
    let name = format!("{hex}.{family}.{}.json", tail(&digest(&bytes)));
    if listing
        .capsules
        .contains(&(hex.to_owned(), family.to_owned()))
    {
        let file = volume
            .existing(&dir, &name, host.slot.workspace.uid)?
            .ok_or_else(invalid)?;
        if volume.io(read_limit(&file, ITEM_LIMIT), false, "archive_repeat_read")? != bytes {
            return Err(invalid());
        }
        volume.writeback("archive_repeat_file_sync", || file.sync_all())?;
        return volume.writeback("archive_repeat_dir_sync", || dir.sync_all());
    }
    if bytes.len() > ITEM_LIMIT
        || listing.capsules.len() == MAX_ITEMS
        || bytes.len() > ARCHIVE_LIMIT - listing.bytes
    {
        return Err(full());
    }
    let cut = format!("archive_{family}");
    let stage = format!("stage.{hex}.{family}");
    point(volume, &cut, "create")?;
    let fd = volume.mutation("archive_create", || {
        fs::openat(
            &dir,
            stage.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map_err(io::Error::from)
    })?;
    let mut file = File::from(fd);
    // Preserve an unconfirmed stage for diagnosis. It never selects the active evidence.
    let prepared = (|| {
        volume.mutation("archive_mode", || {
            fs::fchmod(&file, Mode::RUSR | Mode::WUSR).map_err(Into::into)
        })?;
        let stat = volume.io(file.metadata(), false, "archive_stage_stat")?;
        if stat.nlink() != 1
            || stat.uid() != host.slot.workspace.uid
            || stat.mode() & 0o7777 != 0o600
        {
            return Err(invalid());
        }
        volume.same_entry(&dir, &stage, &stat)?;
        point(volume, &cut, "write")?;
        #[cfg(feature = "qualification")]
        if std::env::var("GAAP_JOURNAL_POINT").as_deref() == Ok(&format!("{cut}.write"))
            && std::env::var("GAAP_JOURNAL_ACTION").as_deref() == Ok("short")
        {
            volume.mutation("archive_short_write", || {
                file.write_all(&bytes[..bytes.len() / 2])
            })?;
            return Err(Error::unavailable("archive_incomplete_stage"));
        }
        volume.mutation("archive_write", || file.write_all(&bytes))?;
        point(volume, &cut, "file_sync")?;
        volume.writeback("archive_file_sync", || file.sync_all())?;
        Ok(stat)
    })();
    let closed = volume.writeback("archive_close", || {
        checked_close(file.into()).map_err(Into::into)
    });
    let stat = match (prepared, closed) {
        (Err(primary), Err(health)) => return Err(health.with_warning(primary.reason)),
        (result, closed) => {
            closed?;
            result?
        }
    };
    point(volume, &cut, "close")?;
    point(volume, &cut, "before_rename")?;
    let _ = archive_dir(host, volume)?;
    volume.same_entry(&dir, &stage, &stat)?;
    let saved = volume
        .existing(&dir, &stage, host.slot.workspace.uid)?
        .ok_or_else(invalid)?;
    if volume.io(
        read_limit(&saved, ITEM_LIMIT),
        false,
        "archive_stage_readback",
    )? != bytes
    {
        return Err(invalid());
    }
    volume.mutation("archive_rename", || {
        fs::renameat_with(
            &dir,
            stage.as_str(),
            &dir,
            name.as_str(),
            RenameFlags::NOREPLACE,
        )
        .map_err(Into::into)
    })?;
    point(volume, &cut, "after_rename")?;
    point(volume, &cut, "dir_sync")?;
    volume.writeback("archive_dir_sync", || dir.sync_all())?;
    volume.same_entry(&dir, &name, &stat)?;
    let _ = archive_dir(host, volume)?;
    Ok(())
}

pub fn retain_executor(tx: &Submission<'_>, binding: &str, record: &Value) -> Result<()> {
    let bytes = canonical(record)?;
    retain(
        tx,
        binding,
        "executor",
        vec![entry(
            &tx.host().profile.executor.name,
            "gaap:record/execution_evidence",
            &bytes,
        )],
        json!({"digest":digest(&bytes)}),
    )
}

pub fn retain_verifier(
    tx: &Submission<'_>,
    binding: &str,
    attempt_id: &str,
    event: &RunEvent,
    report: &Value,
    after_snapshot: &Value,
) -> Result<()> {
    let actor = &tx.host().profile.verifier_id;
    let RunEvent::Verification {
        verifier_id,
        evidence,
        subject_digest,
        ..
    } = event
    else {
        return Err(invalid());
    };
    let output = canonical(report)?;
    let after = canonical(after_snapshot)?;
    if verifier_id != actor || subject_digest != &digest(&after) {
        return Err(invalid());
    }
    let mut entries = Vec::new();
    for e in evidence {
        let bytes = match e.locator.as_deref() {
            Some("gaap:record/verification") => &output,
            Some("gaap:record/after") => &after,
            _ => return Err(invalid()),
        };
        if digest(bytes) != e.digest {
            return Err(invalid());
        }
        entries.push(entry(
            actor,
            e.locator.as_deref().ok_or_else(invalid)?,
            bytes,
        ));
    }
    let source = json!({"kind":"verification","actor_id":actor,"binding":binding,"event":event});
    entries.push(entry(
        actor,
        &format!("gaap:journal/verification/{attempt_id}"),
        &canonical(&source)?,
    ));
    retain(tx, binding, "verifier", entries, source)
}
