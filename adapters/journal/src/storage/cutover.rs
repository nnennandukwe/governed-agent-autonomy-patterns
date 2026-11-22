//! The version fence precedes replacement of the journal and survives every authorized retry.
use super::publication::point;
use super::v2::Binding;
use super::{
    ANCHOR, JOURNAL, JournalV2, Origin, Pair, Phase, SlotOperation, Snapshot, validate_anchor,
    with_slot,
};
use crate::{Error, Host, Journal, Request, Result, canonical, digest, recovery};
use serde_json::{Value, json};
use std::fs::File;

pub(crate) fn convert(host: &Host, health: &File) -> Result<Value> {
    let _stopped = recovery::Quiescence::acquire(host)?;
    with_slot(host, health, true, |slot| {
        let _authority = recovery::OperatorFence::acquire(host, recovery::Role::Convert)?;
        slot.control_entries(&["lock", ANCHOR])?;
        slot.entries(&[JOURNAL])?;
        let saved = slot.read_anchor()?;
        let state = validate_anchor(host, &saved)?;
        let _custody = recovery::CandidateIndex::open(host, &slot.volume)?;
        let pair = match state {
            Phase::Active {
                generation,
                digest: hash,
                binding,
            } => {
                let (pair, _) =
                    slot.confirmed(saved, generation, &hash, binding, &Request::Enroll)?;
                if let Some(binding) = pair.binding.as_deref() {
                    authorized(host, &binding.origin)?;
                    pair.reconfirm(&slot)?;
                    pair
                } else {
                    start(&slot, pair)?
                }
            }
            Phase::Cutover(binding) => {
                authorized(host, &binding.origin)?;
                binding.reserve_anchor_room(host)?;
                let journal = slot
                    .journal()?
                    .ok_or_else(|| Error::invalid("cutover_journal_missing"))?;
                let destination = if digest(&journal.bytes) == binding.origin.source_journal_digest
                {
                    successor(&journal, &binding.profile_digest)?
                } else {
                    journal.bytes.clone()
                };
                if digest(&destination) != binding.origin.initial_journal_digest {
                    return Err(Error::invalid("cutover_journal_lineage"));
                }
                let candidate = JournalV2::decode(&destination)?;
                binding.lookup(&slot, &candidate, &Request::Enroll)?;
                let generation = binding.origin.source_generation;
                let pair = Pair {
                    anchor: saved,
                    journal,
                    generation,
                    binding: Some(binding),
                };
                // A prior process may have died immediately after a rename. Sync the exact
                // independently identified files and directories before advancing the fence.
                pair.reconfirm(&slot)?;
                activate(&slot, pair, &destination)?
            }
            Phase::Unused => return Err(Error::new("not_enrolled", "slot_unused")),
            Phase::Reserved => {
                return Err(Error::new("incomplete_enrollment", "stopped_reservation"));
            }
        };
        point(&slot.volume, "convert", "before_response")?;
        let profile = &pair
            .binding
            .as_ref()
            .expect("conversion produced a v0.2 binding")
            .profile_digest;
        Ok(
            json!({"status":"converted","workspace_id":host.slot.workspace_id,"store_id":host.slot.store_id,
            "generation":pair.generation,"reconciliation_profile_digest":profile}),
        )
    })
}

fn authorized(host: &Host, origin: &Origin) -> Result<()> {
    recovery::check_conversion(
        host,
        origin.source_generation,
        &origin.source_anchor_digest,
        &origin.source_journal_digest,
    )
}

fn successor(journal: &Snapshot, profile: &str) -> Result<Vec<u8>> {
    let source: Journal = serde_json::from_slice(&journal.bytes)
        .map_err(|_| Error::invalid("cutover_source_journal"))?;
    if source.format != "gaap.journal/0.1" {
        return Err(Error::invalid("cutover_source_format"));
    }
    let result = canonical(&JournalV2::from_source(source, profile.to_owned()))?;
    if result.len() > crate::JOURNAL_LIMIT {
        return Err(Error::new("capacity", "cutover_journal_capacity"));
    }
    Ok(result)
}

fn start(slot: &SlotOperation<'_>, mut pair: Pair) -> Result<Pair> {
    let profile = recovery::profile_digest(slot.host)?;
    let destination = successor(&pair.journal, &profile)?;
    let candidate = JournalV2::decode(&destination)?;
    let origin = Origin {
        source_generation: pair.generation,
        source_attempt_count: candidate.attempts.len(),
        source_anchor_digest: digest(&pair.anchor.bytes),
        source_journal_digest: digest(&pair.journal.bytes),
        initial_journal_digest: digest(&destination),
    };
    authorized(slot.host, &origin)?;
    let binding = Binding {
        profile_digest: profile,
        origin,
        pending: None,
    };
    binding.reserve_anchor_room(slot.host)?;
    binding.lookup(slot, &candidate, &Request::Enroll)?;
    let fence = canonical(&binding.render(
        slot.host,
        "cutover",
        pair.generation,
        &binding.origin.source_journal_digest,
    ))?;
    pair.anchor = slot.replace_anchor("cutover_fence", &fence, &pair.anchor, &pair.journal)?;
    pair.binding = Some(Box::new(binding));
    activate(slot, pair, &destination)
}

fn activate(slot: &SlotOperation<'_>, mut pair: Pair, destination: &[u8]) -> Result<Pair> {
    if pair.journal.bytes != destination {
        pair.journal =
            slot.replace_journal("cutover_journal", destination, &pair.anchor, &pair.journal)?;
    }
    let binding = pair
        .binding
        .as_deref()
        .ok_or_else(|| Error::invalid("missing_cutover_binding"))?;
    let active = canonical(&binding.render(
        slot.host,
        "active",
        pair.generation,
        &binding.origin.initial_journal_digest,
    ))?;
    pair.anchor = slot.replace_anchor("cutover_active", &active, &pair.anchor, &pair.journal)?;
    pair.check(slot)?;
    Ok(pair)
}
