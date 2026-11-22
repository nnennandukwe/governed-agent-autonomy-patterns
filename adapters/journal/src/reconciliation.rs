//! Authenticate an operator's evidence without resuming the original run.
mod timeline;

use std::fs::File;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::recovery::{CandidateIndex, OperatorFence, Role};
use crate::storage::{self, Pending};
use crate::{Error, Host, MAX_SAFE, Request, Result};

pub(crate) use timeline::lookup;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Ruling {
    attempt_id: String,
    binding: String,
    evidence_digest: String,
    observer_id: String,
    admitted_by: String,
    disposition: String,
    reason: String,
    after: Option<Value>,
    recorded_generation: u64,
    original_fact_count: u64,
}

pub(crate) fn reconcile(
    host: &Host,
    health: &File,
    attempt_id: &str,
    digest: &str,
) -> Result<Value> {
    let request = Request::Reconcile {
        attempt_id: attempt_id.to_owned(),
        evidence_digest: digest.to_owned(),
    };
    storage::with_reconciliation(host, health, |tx| {
        // Validate all prior attempts and admitted evidence before disclosing a selector.
        let selection_rights = OperatorFence::acquire(host, Role::Reconcile)?;
        let history = tx.lookup(&request)?;
        let pending = tx.pending();
        if pending.is_some_and(|intake| {
            intake.attempt_id != attempt_id || intake.evidence_digest != digest
        }) {
            return Err(Error::new("busy", "another evidence intake is pending"));
        }
        if let Some(stored) = history.rulings.as_ref().and_then(|rows| {
            rows.iter()
                .find(|row| row["evidence_digest"].as_str() == Some(digest))
        }) {
            if tx.pending().is_some() {
                tx.clear_pending()?;
            }
            return recorded(tx, &request, stored.clone());
        }

        let index = CandidateIndex::open(host, tx.volume())?;
        let Some(attempt) = tx
            .journal()
            .attempts
            .iter()
            .find(|row| row["attempt_id"].as_str() == Some(attempt_id))
        else {
            return Err(Error::new(
                "not_found",
                "selector unavailable in this workspace",
            ));
        };
        if !index.contains(digest) {
            return Err(if pending.is_some() {
                Error::invalid("pending evidence is missing")
            } else {
                Error::new("not_found", "selector unavailable in this workspace")
            });
        }
        let attempt = attempt.clone();
        let facts = attempt["facts"]
            .as_array()
            .ok_or_else(|| Error::invalid("missing attempt facts"))?;
        if !facts.iter().any(|fact| fact["tag"] == "dispatch") {
            return Err(Error::new(
                "ineligible_evidence",
                "attempt never dispatched",
            ));
        }
        if tx.pending().is_none() {
            tx.set_pending(Pending {
                attempt_id: attempt_id.to_owned(),
                evidence_digest: digest.to_owned(),
                intake_by: host.principal.clone(),
            })?;
        } else {
            // A previous publisher could have died after rename and before directory sync.
            tx.reconfirm()?;
        }
        drop(selection_rights);
        let finding = index.assess(host, tx.volume(), &attempt, digest);
        tx.point("reconcile", "before_authority")?;
        let _rights = OperatorFence::acquire(host, Role::Reconcile)?;
        let finding = match finding {
            Ok(found) => found,
            Err(error) if error.status == "ineligible_evidence" => {
                tx.clear_pending()?;
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        let journal = tx.journal();
        let generation = journal
            .generation
            .checked_add(1)
            .filter(|&next| next <= MAX_SAFE)
            .ok_or_else(|| Error::new("capacity", "journal generation exhausted"))?;
        let original_fact_count = journal.attempts.iter().try_fold(0u64, |total, row| {
            row["facts"]
                .as_array()
                .map(|facts| total + facts.len() as u64)
                .ok_or_else(|| Error::invalid("missing original fact frontier"))
        })?;
        let ruling = Ruling {
            attempt_id: attempt_id.to_owned(),
            binding: facts[0]["binding"]
                .as_str()
                .ok_or_else(|| Error::invalid("missing binding"))?
                .to_owned(),
            evidence_digest: digest.to_owned(),
            observer_id: finding.observer_id,
            disposition: finding.disposition,
            reason: finding.reason,
            after: finding.after,
            admitted_by: host.principal.clone(),
            recorded_generation: generation,
            original_fact_count,
        };
        let ruling =
            serde_json::to_value(ruling).map_err(|_| Error::invalid("ruling encoding failed"))?;
        tx.append_ruling(ruling.clone())?;
        // clear_pending reconfirms the visible ruling before publishing a clear pointer.
        tx.clear_pending()?;
        recorded(tx, &request, ruling)
    })
}

fn recorded(
    tx: &mut storage::Reconciliation<'_>,
    request: &Request,
    ruling: Value,
) -> Result<Value> {
    tx.reconfirm()?;
    let history = tx.lookup(request)?;
    tx.before_response()?;
    Ok(json!({
        "status": "recorded", "ruling": ruling, "generation": tx.journal().generation,
        "workspace_hold": history.workspace_hold, "effect_status": history.effect_status,
    }))
}
