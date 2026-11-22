//! A single reconciliation operation holds the supervisor launch fence, volume and slot.
use super::linux::Volume;
use super::publication::point;
use super::{JournalV2, Pair, Pending, Phase, SlotOperation, validate_anchor, with_slot};
use crate::{Error, Host, Lookup, MAX_SAFE, Request, Result, canonical, digest, recovery};
use serde_json::Value;
use std::fs::File;

pub(crate) struct Reconciliation<'a> {
    slot: SlotOperation<'a>,
    pair: Pair,
    journal: JournalV2,
}

pub(crate) fn with_reconciliation<T>(
    host: &Host,
    health: &File,
    operation: impl FnOnce(&mut Reconciliation<'_>) -> Result<T>,
) -> Result<T> {
    let _stopped = recovery::Quiescence::acquire(host)?;
    with_slot(host, health, true, |slot| {
        // Check the current grant before touching the selected history. The caller acquires
        // a fresh authority fence when deciding the ruling, after the pending publication.
        let authority = recovery::OperatorFence::acquire(host, recovery::Role::Reconcile)?;
        let saved = slot.read_anchor()?;
        let (generation, hash, binding) = match validate_anchor(host, &saved)? {
            Phase::Active {
                generation,
                digest,
                binding: Some(binding),
            } => (generation, digest, binding),
            Phase::Cutover(_) => {
                return Err(Error::new("cutover_pending", "conversion_incomplete"));
            }
            _ => return Err(Error::unsupported("conversion_required")),
        };
        let (pair, _) =
            slot.confirmed(saved, generation, &hash, Some(binding), &Request::Enroll)?;
        let journal = JournalV2::decode(&pair.journal.bytes)?;
        drop(authority);
        operation(&mut Reconciliation {
            slot,
            pair,
            journal,
        })
    })
}

impl Reconciliation<'_> {
    pub(crate) fn host(&self) -> &Host {
        self.slot.host
    }
    pub(crate) fn volume(&self) -> &Volume<'_> {
        &self.slot.volume
    }
    pub(crate) fn journal(&self) -> &JournalV2 {
        &self.journal
    }
    fn binding(&self) -> &super::v2::Binding {
        self.pair
            .binding
            .as_deref()
            .expect("operator opened a v0.2 anchor")
    }
    pub(crate) fn pending(&self) -> Option<&Pending> {
        self.binding().pending.as_ref()
    }
    pub(crate) fn lookup(&self, request: &Request) -> Result<Lookup> {
        self.pair.check(&self.slot)?;
        self.binding().lookup(&self.slot, &self.journal, request)
    }
    pub(crate) fn reconfirm(&self) -> Result<()> {
        self.pair.reconfirm(&self.slot)
    }
    pub(crate) fn point(&self, stage: &'static str, operation: &'static str) -> Result<()> {
        point(self.volume(), stage, operation)
    }
    pub(crate) fn before_response(&self) -> Result<()> {
        self.point("reconcile", "before_response")
    }

    fn write_pointer(&mut self, stage: &'static str, pending: Option<Pending>) -> Result<()> {
        self.pair.check(&self.slot)?;
        let mut updated = self.binding().clone();
        updated.pending = pending;
        let bytes = canonical(&updated.render(
            self.host(),
            "active",
            self.pair.generation,
            &digest(&self.pair.journal.bytes),
        ))?;
        self.pair.anchor =
            self.slot
                .replace_anchor(stage, &bytes, &self.pair.anchor, &self.pair.journal)?;
        self.pair.binding = Some(Box::new(updated));
        Ok(())
    }

    pub(crate) fn set_pending(&mut self, pending: Pending) -> Result<()> {
        pending.validate()?;
        if self.pending().is_some() {
            return Err(Error::new("busy", "pending_intake_exists"));
        }
        self.write_pointer("intake", Some(pending))
    }

    pub(crate) fn append_ruling(&mut self, ruling: Value) -> Result<()> {
        let pending = self
            .pending()
            .ok_or_else(|| Error::invalid("pending_intake_required"))?;
        if ruling["attempt_id"] != pending.attempt_id
            || ruling["evidence_digest"] != pending.evidence_digest
        {
            return Err(Error::invalid("ruling_does_not_match_intake"));
        }
        if self.pair.generation == MAX_SAFE {
            return Err(Error::new("capacity", "journal_generation_exhausted"));
        }
        self.journal.generation += 1;
        self.journal.rulings.push(ruling);
        let bytes = canonical(&self.journal)?;
        self.pair.advance(
            &self.slot,
            &Request::Enroll,
            &bytes,
            "ruling",
            "confirm_ruling",
        )
    }

    pub(crate) fn clear_pending(&mut self) -> Result<()> {
        if self.pending().is_none() {
            return Err(Error::invalid("pending_intake_required"));
        }
        self.reconfirm()?;
        self.write_pointer("clear_intake", None)
    }
}
