use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::Ruling;
use crate::history::{self, Attempt, EvidenceArchive, HistoryContext};
use crate::recovery::CandidateIndex;
use crate::storage::linux::Volume;
use crate::storage::{JournalV2, Origin, Pending};
use crate::{Error, Host, Lookup, MAX_SAFE, Request, Result, is_digest, token};

/// A known observation contains the full after-image identity for a replacement.
/// A later difference in source digest alone doesn't create a dispute.
#[derive(PartialEq)]
struct Outcome {
    disposition: String,
    after: Option<Value>,
}

enum Effect {
    Undispatched,
    Unresolved,
    Known(Outcome),
    Disputed,
}

impl Effect {
    fn from_original(attempt: &Attempt) -> Self {
        if let Some(observed) = attempt.facts.iter().find(|fact| fact.tag == "observed") {
            // history::visit has checked the typed disposition, FileObservation and source.
            if let Some(disposition) = observed.payload["disposition"].as_str()
                && disposition != "unresolved"
            {
                return Self::Known(Outcome {
                    disposition: disposition.to_owned(),
                    after: observed
                        .payload
                        .get("after")
                        .filter(|output| !output.is_null())
                        .cloned(),
                });
            }
        }
        if attempt.facts.iter().any(|fact| fact.tag == "dispatch") {
            Self::Unresolved
        } else {
            Self::Undispatched
        }
    }

    fn admit(&mut self, observation: Outcome) -> Result<()> {
        match self {
            Self::Undispatched => return Err(Error::invalid("ruling precedes original dispatch")),
            Self::Unresolved => *self = Self::Known(observation),
            Self::Known(first) if *first != observation => *self = Self::Disputed,
            Self::Known(_) | Self::Disputed => {}
        }
        Ok(())
    }

    fn held(&self) -> bool {
        matches!(self, Self::Unresolved | Self::Disputed)
    }

    fn status(&self) -> &str {
        match self {
            Self::Undispatched => "not_dispatched",
            Self::Unresolved => "unresolved",
            Self::Known(outcome) => &outcome.disposition,
            Self::Disputed => "disputed",
        }
    }
}

struct Timeline<'a> {
    attempts: &'a [Value],
    effects: BTreeMap<String, Effect>,
    admitted: BTreeSet<(String, String)>,
    generation: u64,
    fact_count: u64,
}

impl Timeline<'_> {
    fn apply(
        &mut self,
        host: &Host,
        volume: &Volume<'_>,
        index: &CandidateIndex,
        ruling: &Ruling,
    ) -> Result<()> {
        let Some(effect) = self.effects.get_mut(&ruling.attempt_id) else {
            return Err(Error::invalid("ruling predates its attempt"));
        };
        let row = self
            .attempts
            .iter()
            .find(|row| row["attempt_id"].as_str() == Some(&ruling.attempt_id))
            .ok_or_else(|| Error::invalid("ruling has no original attempt"))?;
        self.generation += 1;
        if ruling.original_fact_count != self.fact_count
            || ruling.recorded_generation != self.generation
            || !token(&ruling.admitted_by)
            || !is_digest(&ruling.evidence_digest)
            || row["facts"][0]["binding"].as_str() != Some(&ruling.binding)
            || !self
                .admitted
                .insert((ruling.attempt_id.clone(), ruling.evidence_digest.clone()))
            || !index.contains(&ruling.evidence_digest)
        {
            return Err(Error::invalid(
                "invalid ruling identity, publication frontier or source",
            ));
        }
        let source = index
            .assess(host, volume, row, &ruling.evidence_digest)
            .map_err(|error| {
                if error.status == "ineligible_evidence" {
                    Error::invalid("retained ruling lacks admissible evidence")
                } else {
                    error
                }
            })?;
        if ruling.observer_id != host.profile.effect_observer
            || source.observer_id != ruling.observer_id
            || source.disposition != ruling.disposition
            || source.reason != ruling.reason
            || source.after != ruling.after
        {
            return Err(Error::invalid(
                "retained ruling disagrees with original observer evidence",
            ));
        }
        effect.admit(Outcome {
            disposition: source.disposition,
            after: source.after,
        })
    }
}

pub(crate) fn lookup(
    host: &Host,
    volume: &Volume<'_>,
    archive: &EvidenceArchive,
    journal: &JournalV2,
    origin: &Origin,
    pending: Option<&Pending>,
    request: &Request,
) -> Result<Lookup> {
    let index = CandidateIndex::open(host, volume)?;
    if origin.source_attempt_count > journal.attempts.len() || origin.source_generation > MAX_SAFE {
        return Err(Error::invalid("invalid source attempt frontier"));
    }
    let context = HistoryContext {
        profile: &host.profile,
        archive,
        workspace: &host.slot.workspace,
        workspace_id: &host.slot.workspace_id,
        store_id: &host.slot.store_id,
        profile_digest: &host.slot.profile_digest,
    };
    let mut timeline = Timeline {
        attempts: &journal.attempts,
        effects: BTreeMap::new(),
        admitted: BTreeSet::new(),
        generation: origin.source_generation,
        fact_count: 0,
    };
    let mut answer = Lookup::absent();
    let mut selection = None;
    let mut seen = 0;
    let mut rulings = journal.rulings.iter().peekable();
    history::visit(&journal.attempts, &context, |raw, attempt, status, _| {
        let imported = seen < origin.source_attempt_count;
        seen += 1;
        if !imported && timeline.effects.values().any(Effect::held) {
            return Err(Error::invalid(
                "new admission crosses an unresolved or disputed effect",
            ));
        }
        let count = attempt.facts.len() as u64;
        timeline.fact_count += count;
        if !imported {
            timeline.generation += count;
        }
        timeline
            .effects
            .insert(attempt.attempt_id.clone(), Effect::from_original(attempt));
        match request {
            Request::Lookup { key, proposal }
                if host.principal == attempt.principal && *key == attempt.key =>
            {
                history::select(&mut answer, raw, status, proposal.as_ref());
                if answer.attempt.is_some() {
                    selection = Some(attempt.attempt_id.clone());
                }
            }
            Request::Reconcile { attempt_id, .. } if *attempt_id == attempt.attempt_id => {
                // A scoped operator can select an effect without receiving the original proposal.
                selection = Some(attempt.attempt_id.clone());
            }
            _ => {}
        }
        // A source attempt can't grow after conversion. Later rulings can appear
        // only at whole-attempt boundaries, after their original dispatch.
        if seen >= origin.source_attempt_count {
            while let Some(raw) = rulings.peek() {
                let frontier = raw["original_fact_count"]
                    .as_u64()
                    .ok_or_else(|| Error::invalid("invalid ruling frontier"))?;
                if frontier > timeline.fact_count {
                    break;
                }
                let ruling = history::closed::<Ruling>(raw)?;
                timeline.apply(host, volume, &index, &ruling)?;
                rulings.next();
            }
        }
        Ok(())
    })?;
    if rulings.next().is_some()
        || timeline.generation != journal.generation
        || journal.generation > MAX_SAFE
    {
        return Err(Error::invalid(
            "ruling history disagrees with confirmed generation",
        ));
    }
    if let Some(intake) = pending
        && (!token(&intake.intake_by)
            || !is_digest(&intake.evidence_digest)
            || !index.contains(&intake.evidence_digest)
            || !timeline
                .effects
                .get(&intake.attempt_id)
                .is_some_and(|effect| !matches!(effect, Effect::Undispatched)))
    {
        return Err(Error::invalid(
            "pending intake lost its original attempt or pinned source",
        ));
    }
    answer.workspace_hold = pending.is_some() || timeline.effects.values().any(Effect::held);
    if let Some(id) = selection {
        answer.effect_status = timeline
            .effects
            .get(&id)
            .map(|effect| effect.status().to_owned());
        answer.intake_pending = Some(pending.is_some_and(|intake| intake.attempt_id == id));
        answer.rulings = Some(
            journal
                .rulings
                .iter()
                .filter(|ruling| ruling["attempt_id"].as_str() == Some(&id))
                .cloned()
                .collect(),
        );
    }
    Ok(answer)
}
