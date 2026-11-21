//! Enrollment owns three publications. The reader takes the same lock and never publishes.
pub(crate) mod linux;
pub(crate) mod publication;
pub mod writer;

use crate::{ANCHOR_LIMIT, Error, Host, JOURNAL_LIMIT, MAX_SAFE, Request, Result};
use linux::{Handles, Volume};
use rustix::fs;
use serde_json::{Value, json};
use std::fs::{File, Metadata};
use std::io::Read;
use std::os::unix::fs::MetadataExt;

const ANCHOR: &str = "anchor.json";
const JOURNAL: &str = "journal.json";

struct Snapshot {
    file: File,
    stat: Metadata,
    bytes: Vec<u8>,
}

impl Snapshot {
    fn read(
        dir: &File,
        name: &str,
        max: usize,
        uid: u32,
        volume: &Volume<'_>,
    ) -> Result<Option<Self>> {
        let Some(file) = volume.existing(dir, name, uid)? else {
            return Ok(None);
        };
        let stat = volume.io(file.metadata(), false, "state_metadata")?;
        if stat.len() > max as u64 {
            return Err(Error::invalid("state_exceeds_capacity"));
        }
        let mut bytes = Vec::with_capacity(stat.len() as usize);
        volume.io(
            (&file).take(max as u64 + 1).read_to_end(&mut bytes),
            false,
            "read_state",
        )?;
        if bytes.len() != stat.len() as usize || bytes.len() > max {
            return Err(Error::invalid("state_changed_during_read"));
        }
        let snapshot = Self { file, stat, bytes };
        snapshot.recheck(dir, name, volume)?;
        Ok(Some(snapshot))
    }

    fn recheck(&self, dir: &File, name: &str, volume: &Volume<'_>) -> Result<()> {
        volume.same_entry(dir, name, &self.stat)?;
        let current = volume.io(self.file.metadata(), false, "recheck_snapshot")?;
        if current.len() != self.stat.len()
            || current.ctime() != self.stat.ctime()
            || current.ctime_nsec() != self.stat.ctime_nsec()
        {
            return Err(Error::invalid("snapshot_changed"));
        }
        Ok(())
    }
}

fn anchor(host: &Host, phase: &str, generation: Value, digest: Value) -> Value {
    let slot = &host.slot;
    json!({"format":"gaap.journal-anchor/0.1","workspace_id":slot.workspace_id,"store_id":slot.store_id,
        "workspace":slot.workspace,"control":slot.control,"store":slot.store,"lock":slot.lock,
        "profile_digest":slot.profile_digest,"limits":crate::limits(),"phase":phase,"generation":generation,"journal_digest":digest})
}

enum Phase {
    Unused,
    Reserved,
    Active { generation: u64, digest: String },
}

fn validate_anchor(host: &Host, saved: &Snapshot) -> Result<Phase> {
    let record = crate::json_document(&saved.bytes, true)?;
    if record
        .get("format")
        .and_then(Value::as_str)
        .is_some_and(|value| value != "gaap.journal-anchor/0.1")
    {
        return Err(Error::unsupported("unsupported_anchor_format"));
    }
    let phase = record["phase"]
        .as_str()
        .ok_or_else(|| Error::invalid("anchor_phase"))?;
    let (state, expected) = match phase {
        "unused" | "reserved" => (
            if phase == "unused" {
                Phase::Unused
            } else {
                Phase::Reserved
            },
            anchor(host, phase, Value::Null, Value::Null),
        ),
        "active" => {
            let generation = record["generation"]
                .as_u64()
                .filter(|&n| n <= MAX_SAFE)
                .ok_or_else(|| Error::invalid("anchor_generation"))?;
            let digest = record["journal_digest"]
                .as_str()
                .filter(|v| crate::is_digest(v))
                .ok_or_else(|| Error::invalid("anchor_digest"))?;
            (
                Phase::Active {
                    generation,
                    digest: digest.to_owned(),
                },
                anchor(host, phase, json!(generation), json!(digest)),
            )
        }
        _ => return Err(Error::invalid("anchor_phase")),
    };
    if record != expected {
        return Err(Error::invalid("anchor_and_provisioning_disagree"));
    }
    Ok(state)
}

struct SlotOperation<'a> {
    host: &'a Host,
    volume: Volume<'a>,
    handles: Handles,
}

impl SlotOperation<'_> {
    fn read_anchor(&self) -> Result<Snapshot> {
        Snapshot::read(
            &self.handles.control,
            ANCHOR,
            ANCHOR_LIMIT,
            self.host.slot.control.uid,
            &self.volume,
        )?
        .ok_or_else(|| Error::unavailable("missing_host_anchor"))
    }

    fn journal(&self) -> Result<Option<Snapshot>> {
        Snapshot::read(
            &self.handles.store,
            JOURNAL,
            JOURNAL_LIMIT,
            self.host.slot.store.uid,
            &self.volume,
        )
    }

    fn entries(&self, allowed: &[&str]) -> Result<()> {
        let stream = self.volume.io(
            fs::Dir::read_from(&self.handles.store),
            false,
            "read_journal_directory",
        )?;
        for entry in stream {
            let entry = self.volume.io(entry, false, "read_journal_entry")?;
            let name = entry.file_name().to_bytes();
            if name != b"." && name != b".." && !allowed.iter().any(|item| item.as_bytes() == name)
            {
                return Err(Error::invalid("unexpected_journal_directory_entry"));
            }
        }
        Ok(())
    }

    fn recheck(&self, saved: &Snapshot, journal: Option<&Snapshot>) -> Result<()> {
        self.handles.recheck(self.host, &self.volume)?;
        saved.recheck(&self.handles.control, ANCHOR, &self.volume)?;
        match journal {
            Some(file) => file.recheck(&self.handles.store, JOURNAL, &self.volume),
            None => match fs::statat(&self.handles.store, JOURNAL, fs::AtFlags::SYMLINK_NOFOLLOW) {
                Err(rustix::io::Errno::NOENT) => Ok(()),
                Ok(_) => Err(Error::invalid("unexpected_journal")),
                Err(error) => self.volume.io(Err(error), false, "journal_entry_check"),
            },
        }
    }

    fn interpret(&self, bytes: &[u8], request: &Request) -> Result<(u64, crate::Lookup)> {
        if self.host.live.is_none() {
            return crate::interpret(self.host, bytes, request);
        }
        let archive = crate::live::refresh_archive(self.host, &self.volume)?;
        crate::interpret_with_archive(self.host, &archive, bytes, request)
    }

    fn confirmed(
        &self,
        saved: &Snapshot,
        generation: u64,
        expected_digest: &str,
        request: &Request,
    ) -> Result<(Snapshot, crate::Lookup)> {
        self.entries(&[JOURNAL])?;
        let journal = self
            .journal()?
            .ok_or_else(|| Error::invalid("active_journal_missing"))?;
        let (observed, lookup) = self.interpret(&journal.bytes, request)?;
        self.recheck(saved, Some(&journal))?;
        if observed == generation + 1 {
            return Err(Error::new("unconfirmed", "journal_ahead_of_anchor"));
        }
        if observed != generation || crate::digest(&journal.bytes) != expected_digest {
            return Err(Error::invalid("journal_confirmation_mismatch"));
        }
        Ok((journal, lookup))
    }

    fn enrolled(&self, generation: u64) -> Result<Value> {
        self.volume.healthy()?;
        publication::point(&self.volume, "enroll", "before_response")?;
        Ok(
            json!({"status":"enrolled","workspace_id":self.host.slot.workspace_id,"store_id":self.host.slot.store_id,"generation":generation}),
        )
    }

    fn enroll_empty(&self, unused: Snapshot) -> Result<Value> {
        let empty = crate::empty_journal(self.host)?;
        // A provisioned live archive must be available before reserving a new slot.
        if self.host.live.is_some() {
            self.interpret(&empty, &Request::Enroll)?;
        }
        let reserved = crate::canonical(&anchor(self.host, "reserved", Value::Null, Value::Null))?;
        self.publish(publication::Stage::Reserve, &reserved, &unused, None)?;
        let confirmed = self.read_anchor()?;
        if confirmed.bytes != reserved {
            return Err(Error::invalid("reservation_changed"));
        }
        self.publish(publication::Stage::Journal, &empty, &confirmed, None)?;
        publication::point(&self.volume, "journal", "reread")?;
        let saved = self
            .journal()?
            .ok_or_else(|| Error::invalid("missing_published_journal"))?;
        if saved.bytes != empty {
            return Err(Error::invalid("published_journal_changed"));
        }
        let (generation, _) = crate::interpret(self.host, &saved.bytes, &Request::Enroll)?;
        let active = crate::canonical(&anchor(
            self.host,
            "active",
            json!(generation),
            json!(crate::digest(&saved.bytes)),
        ))?;
        self.publish(
            publication::Stage::Activate,
            &active,
            &confirmed,
            Some(&saved),
        )?;
        let visible = self.read_anchor()?;
        if visible.bytes != active {
            return Err(Error::invalid("activated_anchor_changed"));
        }
        self.recheck(&visible, Some(&saved))?;
        self.enrolled(generation)
    }

    fn process(&self, request: &Request) -> Result<Value> {
        let saved = self.read_anchor()?;
        let state = validate_anchor(self.host, &saved)?;
        match state {
            Phase::Unused => {
                self.entries(&[])?;
                self.recheck(&saved, None)?;
                match request {
                    Request::Enroll => self.enroll_empty(saved),
                    Request::Lookup { .. } => Err(Error::new("not_enrolled", "slot_unused")),
                }
            }
            Phase::Reserved => {
                self.entries(&[JOURNAL, ".gaap-stage-journal"])?;
                let journal = self.journal()?;
                if let Some(file) = &journal
                    && file.bytes != crate::empty_journal(self.host)?
                {
                    return Err(Error::invalid("reservation_journal_conflict"));
                }
                self.recheck(&saved, journal.as_ref())?;
                Err(Error::new("incomplete_enrollment", "stopped_reservation"))
            }
            Phase::Active { generation, digest } => {
                let (journal, lookup) = self.confirmed(&saved, generation, &digest, request)?;
                match request {
                    Request::Enroll => {
                        self.volume
                            .writeback("repeat_anchor_sync", || saved.file.sync_all())?;
                        self.volume
                            .writeback("repeat_control_sync", || self.handles.control.sync_all())?;
                        self.recheck(&saved, Some(&journal))?;
                        self.enrolled(generation)
                    }
                    Request::Lookup { .. } => lookup.at_generation(generation),
                }
            }
        }
    }
}

pub fn run(host: &Host, health: &File, request: &Request) -> Result<Value> {
    with_slot(host, health, matches!(request, Request::Enroll), |slot| {
        slot.process(request)
    })
}

fn with_slot<'a, T>(
    host: &'a Host,
    health: &'a File,
    writer: bool,
    operation: impl FnOnce(SlotOperation<'a>) -> Result<T>,
) -> Result<T> {
    let volume = Volume {
        health,
        slot: &host.slot,
    };
    let _health_lock = volume.lock_health(writer)?;
    let handles = Handles::open(host, &volume)?;
    publication::point(&volume, "slot", "before_lock")?;
    handles.acquire(writer, &volume)?;
    publication::point(&volume, "slot", "after_lock")?;
    handles.recheck(host, &volume)?;
    operation(SlotOperation {
        host,
        volume,
        handles,
    })
}
