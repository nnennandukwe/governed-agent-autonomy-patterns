//! Closed v0.2 envelopes and the immutable origin of a converted slot.
use super::{SlotOperation, anchor};
use crate::{
    ANCHOR_LIMIT, ATTEMPT_LIMIT, Error, Host, JOURNAL_LIMIT, Journal, Lookup, MAX_SAFE, Request,
    Result, canonical, digest, is_digest, json_document, recovery,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub(super) const ANCHOR_FORMAT: &str = "gaap.journal-anchor/0.2";
const JOURNAL_FORMAT: &str = "gaap.journal/0.2";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Origin {
    pub(crate) source_generation: u64,
    pub(crate) source_attempt_count: usize,
    pub(crate) source_anchor_digest: String,
    pub(crate) source_journal_digest: String,
    pub(crate) initial_journal_digest: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Pending {
    pub(crate) attempt_id: String,
    pub(crate) evidence_digest: String,
    pub(crate) intake_by: String,
}

impl Pending {
    pub(super) fn validate(&self) -> Result<()> {
        if !crate::hex(&self.attempt_id, 32)
            || !is_digest(&self.evidence_digest)
            || !crate::token(&self.intake_by)
        {
            return Err(Error::invalid("invalid_pending_selector"));
        }
        Ok(())
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct JournalV2 {
    pub(crate) format: String,
    pub(crate) workspace_id: String,
    pub(crate) store_id: String,
    pub(crate) profile_digest: String,
    pub(crate) reconciliation_profile_digest: String,
    pub(crate) generation: u64,
    pub(crate) attempts: Vec<Value>,
    pub(crate) rulings: Vec<Value>,
}

impl JournalV2 {
    pub(super) fn from_source(source: Journal, reconciliation_profile_digest: String) -> Self {
        Self {
            format: JOURNAL_FORMAT.into(),
            workspace_id: source.workspace_id,
            store_id: source.store_id,
            profile_digest: source.profile_digest,
            reconciliation_profile_digest,
            generation: source.generation,
            attempts: source.attempts,
            rulings: Vec::new(),
        }
    }

    pub(super) fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > JOURNAL_LIMIT {
            return Err(Error::invalid("journal_exceeds_capacity"));
        }
        let raw = json_document(bytes, true)?;
        let journal: Self =
            serde_json::from_value(raw).map_err(|_| Error::invalid("v2_journal_shape"))?;
        if journal.format != JOURNAL_FORMAT
            || journal.generation > MAX_SAFE
            || journal.attempts.len() > ATTEMPT_LIMIT
            || (journal.generation == 0) != journal.attempts.is_empty()
        {
            return Err(Error::invalid("v2_journal_format_or_bounds"));
        }
        Ok(journal)
    }
}

#[derive(Clone)]
pub(super) struct Binding {
    pub(super) profile_digest: String,
    pub(super) origin: Origin,
    pub(super) pending: Option<Pending>,
}

impl Binding {
    pub(super) fn render(
        &self,
        host: &Host,
        phase: &str,
        generation: u64,
        journal_digest: &str,
    ) -> Value {
        let mut out = anchor(host, phase, json!(generation), json!(journal_digest));
        out["format"] = json!(ANCHOR_FORMAT);
        out["reconciliation_profile_digest"] = json!(self.profile_digest);
        out["origin"] = json!(self.origin);
        out["pending"] = json!(self.pending);
        out
    }

    pub(super) fn decode(host: &Host, raw: &Value) -> Result<(Self, u64, String)> {
        let profile_digest = recovery::profile_digest(host)?;
        let origin: Origin = serde_json::from_value(raw["origin"].clone())
            .map_err(|_| Error::invalid("v2_origin_shape"))?;
        let pending: Option<Pending> = serde_json::from_value(raw["pending"].clone())
            .map_err(|_| Error::invalid("v2_pending_shape"))?;
        if let Some(pair) = &pending {
            pair.validate()?;
        }
        let phase = raw["phase"]
            .as_str()
            .ok_or_else(|| Error::invalid("v2_anchor_phase"))?;
        let generation = raw["generation"]
            .as_u64()
            .filter(|&g| g <= MAX_SAFE)
            .ok_or_else(|| Error::invalid("v2_anchor_generation"))?;
        let hash = raw["journal_digest"]
            .as_str()
            .filter(|s| is_digest(s))
            .ok_or_else(|| Error::invalid("v2_anchor_digest"))?;
        if !matches!(phase, "active" | "cutover")
            || origin.source_generation > generation
            || origin.source_attempt_count > ATTEMPT_LIMIT
            || ![
                &origin.source_anchor_digest,
                &origin.source_journal_digest,
                &origin.initial_journal_digest,
            ]
            .iter()
            .all(|s| is_digest(s))
            || (phase == "cutover"
                && (pending.is_some()
                    || generation != origin.source_generation
                    || hash != origin.source_journal_digest))
        {
            return Err(Error::invalid("v2_origin_or_phase"));
        }
        let source_anchor = anchor(
            host,
            "active",
            json!(origin.source_generation),
            json!(&origin.source_journal_digest),
        );
        if digest(&canonical(&source_anchor)?) != origin.source_anchor_digest {
            return Err(Error::invalid("source_anchor_lineage"));
        }
        let binding = Self {
            profile_digest,
            origin,
            pending,
        };
        if binding.render(host, phase, generation, hash) != *raw {
            return Err(Error::invalid("v2_anchor_and_provisioning_disagree"));
        }
        Ok((binding, generation, hash.to_owned()))
    }

    pub(super) fn reserve_anchor_room(&self, host: &Host) -> Result<()> {
        let hash = format!("sha256:{}", "f".repeat(64));
        let mut largest = self.render(host, "active", MAX_SAFE, &hash);
        largest["pending"] =
            json!({"attempt_id":"f".repeat(32),"evidence_digest":hash,"intake_by":"x".repeat(128)});
        if canonical(&largest)?.len() > ANCHOR_LIMIT {
            return Err(Error::new("capacity", "pending_anchor_capacity"));
        }
        Ok(())
    }

    pub(super) fn lookup(
        &self,
        slot: &SlotOperation<'_>,
        journal: &JournalV2,
        request: &Request,
    ) -> Result<Lookup> {
        let host = slot.host;
        if journal.workspace_id != host.slot.workspace_id
            || journal.store_id != host.slot.store_id
            || journal.profile_digest != host.slot.profile_digest
            || journal.reconciliation_profile_digest != self.profile_digest
            || journal.generation < self.origin.source_generation
        {
            return Err(Error::invalid("v2_journal_slot_or_generation"));
        }
        let prefix = journal
            .attempts
            .get(..self.origin.source_attempt_count)
            .ok_or_else(|| Error::invalid("missing_original_attempts"))?;
        let source = Journal {
            format: "gaap.journal/0.1".into(),
            workspace_id: journal.workspace_id.clone(),
            store_id: journal.store_id.clone(),
            profile_digest: journal.profile_digest.clone(),
            generation: self.origin.source_generation,
            attempts: prefix.to_vec(),
        };
        let bytes = canonical(&source)?;
        let initial = JournalV2::from_source(source, self.profile_digest.clone());
        if digest(&bytes) != self.origin.source_journal_digest
            || digest(&canonical(&initial)?) != self.origin.initial_journal_digest
        {
            return Err(Error::invalid("source_history_lineage"));
        }
        let retained;
        let archive = if host.live.is_some() {
            retained = crate::live::refresh_archive(host, &slot.volume)?;
            &retained
        } else {
            &host.archive
        };
        crate::interpret_with_archive(host, archive, &bytes, &Request::Enroll)?;
        crate::reconciliation::lookup(
            host,
            &slot.volume,
            archive,
            journal,
            &self.origin,
            self.pending.as_ref(),
            request,
        )
    }
}
