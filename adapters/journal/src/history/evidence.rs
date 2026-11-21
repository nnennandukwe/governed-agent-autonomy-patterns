use std::collections::BTreeMap;

use gaap::contracts::{EvidenceReference, EvidenceType};
use serde_json::json;

use super::types::{EvidenceArchive, GrantCheck, ObserverRef};
use super::{CanonicalObservation, require};
use crate::{Error, Result, canonical, digest, is_digest};

/// Look up exact bytes in the independently supplied host archive. Locators never resolve
/// through the filesystem, a network client or values taken from the journal itself.
pub(super) struct Index {
    entries: BTreeMap<(String, String), BTreeMap<String, Vec<u8>>>,
}

impl EvidenceArchive {
    pub fn validate(&self) -> Result<()> {
        self.index().map(|_| ())
    }

    pub(super) fn index(&self) -> Result<Index> {
        let mut index = Index {
            entries: BTreeMap::new(),
        };
        for entry in &self.entries {
            require(
                !entry.locator.trim().is_empty()
                    && !entry.actor_id.trim().is_empty()
                    && entry.bytes_hex.len().is_multiple_of(2),
                "invalid trusted archive entry",
            )?;
            let bytes = entry
                .bytes_hex
                .as_bytes()
                .chunks_exact(2)
                .map(|pair| {
                    let high = nibble(pair[0])?;
                    let low = nibble(pair[1])?;
                    Ok((high << 4) | low)
                })
                .collect::<Result<Vec<_>>>()?;
            let records = index
                .entries
                .entry((entry.locator.clone(), entry.actor_id.clone()))
                .or_default();
            require(
                records.insert(digest(&bytes), bytes).is_none(),
                "duplicate trusted evidence entry",
            )?;
        }
        Ok(index)
    }
}

fn nibble(byte: u8) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err(Error::invalid(
            "archive bytes require lowercase hexadecimal encoding",
        )),
    }
}

impl Index {
    pub(super) fn bytes(&self, actor: &str, locator: Option<&str>, hash: &str) -> Result<&[u8]> {
        let locator = locator
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| Error::invalid("missing trusted evidence locator"))?;
        require(is_digest(hash), "invalid archived evidence digest")?;
        self.entries
            .get(&(locator.to_owned(), actor.to_owned()))
            .ok_or_else(|| {
                Error::unavailable("required evidence unavailable for the trusted actor")
            })?
            .get(hash)
            .map(Vec::as_slice)
            .ok_or_else(|| {
                Error::invalid("trusted evidence bytes disagree with the retained digest")
            })
    }

    pub(super) fn reference(&self, actor: &str, source: &EvidenceReference) -> Result<&[u8]> {
        self.bytes(actor, source.locator.as_deref(), &source.digest)
    }

    fn observed_bytes(
        &self,
        actor: &str,
        source: &ObserverRef,
        kind: EvidenceType,
    ) -> Result<&[u8]> {
        require(
            source.actor_id == actor && source.evidence.evidence_type == kind,
            "observer role or evidence type mismatch",
        )?;
        self.reference(actor, &source.evidence)
    }

    pub(super) fn grant(
        &self,
        actor: &str,
        check: &GrantCheck,
        binding: &str,
        time: u64,
    ) -> Result<()> {
        let expected = canonical(&json!({
            "kind":"grant", "actor_id":actor, "binding":binding, "checked_at_ms":time,
            "approval":check.approval, "valid":true,
        }))?;
        require(
            self.observed_bytes(actor, &check.source, EvidenceType::Approval)? == expected,
            "valid grant evidence disagrees with the original check or attempt",
        )
    }

    pub(super) fn observation(
        &self,
        actor: &str,
        source: &ObserverRef,
        facts: CanonicalObservation<'_>,
    ) -> Result<()> {
        let expected = |covers_through_stop| {
            json!({
                "kind":"observation", "actor_id":actor, "binding":facts.binding,
                "before":facts.before, "after":facts.observation.after,
                "disposition":facts.observation.disposition, "covers_through_stop":covers_through_stop,
            })
        };
        let covers_stop = matches!(
            facts.observation.disposition,
            super::types::Disposition::NotStarted | super::types::Disposition::StoppedBeforeRename
        );
        let bytes = self.observed_bytes(actor, source, EvidenceType::Artifact)?;
        require(
            bytes == canonical(&expected(true))?
                || (!covers_stop && bytes == canonical(&expected(false))?),
            "original evidence differs from the claimed file observation or lacks final coverage",
        )
    }
}
