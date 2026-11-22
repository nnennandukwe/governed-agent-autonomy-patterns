//! One authenticated foreground submission holds the permanent slot and volume locks.
//! The frozen reader checks each proposed generation before it can replace the prior one.
use super::linux::Volume;
use super::publication::point;
use super::{JournalV2, Pair, Phase, SlotOperation, validate_anchor, with_slot};
use crate::{ATTEMPT_LIMIT, Error, Host, Journal, MAX_SAFE, Request, Result, canonical, digest};
use gaap::contracts::{
    AgentRunRequest, ProtectedEffectRequest, ProtectedEffectResult, TerminalRunReceipt,
};
use serde_json::{Value, json};
use std::fs::File;
use std::io;

#[derive(Clone, Copy, PartialEq)]
enum Fact {
    Intent,
    Dispatch,
    Observed,
    Verified,
    Published,
}

impl Fact {
    fn labels(self) -> (&'static str, &'static str) {
        match self {
            Self::Intent => ("intent", "confirm_intent"),
            Self::Dispatch => ("dispatch", "confirm_dispatch"),
            Self::Observed => ("observed", "confirm_observed"),
            Self::Verified => ("verified", "confirm_verified"),
            Self::Published => ("published", "confirm_published"),
        }
    }
}

/// Scoped to a validated fresh key and the lifetime of one exclusive writer.
pub struct Submission<'a> {
    slot: SlotOperation<'a>,
    request: Request,
    pair: Pair,
    binding: Option<String>,
    last: Option<Fact>,
    failed: bool,
}

/// Return prior history before entering the fresh-work callback or issuing new identities.
pub fn with_submission(
    host: &Host,
    health: &File,
    key: &str,
    proposal: &Value,
    run: impl FnOnce(&mut Submission<'_>) -> Result<()>,
) -> Result<Value> {
    if !host.can_read {
        return Err(Error::new("forbidden", "current host permission denied"));
    }
    host.validate()?;
    crate::validate_key_and_proposal(key, Some(proposal))?;
    let request = Request::Lookup {
        key: key.to_owned(),
        proposal: Some(proposal.clone()),
    };
    let _writers = crate::recovery::writer_lease(host)?;
    with_slot(host, health, true, |slot| {
        let read = slot.read_fence()?;
        let saved = slot.read_anchor()?;
        let Phase::Active {
            generation,
            digest,
            binding,
        } = validate_anchor(host, &saved)?
        else {
            return slot.process(&request);
        };
        let (pair, lookup) = slot.confirmed(saved, generation, &digest, binding, &request)?;
        if lookup.status != "absent" {
            return lookup.at_generation(generation);
        }
        if !host.live.as_ref().is_some_and(|live| live.can_submit) {
            return Err(Error::new("forbidden", "current submit permission denied"));
        }
        if lookup.workspace_hold {
            return Err(Error::new("workspace_held", "unresolved_effect"));
        }
        if host.reconciliation.is_some() && pair.binding.is_none() {
            return Err(Error::unsupported("conversion_required"));
        }
        let retained: Value = serde_json::from_slice(&pair.journal.bytes)
            .map_err(|_| Error::invalid("invalid journal shape"))?;
        let attempts = retained["attempts"]
            .as_array()
            .ok_or_else(|| Error::invalid("invalid journal shape"))?;
        // A fresh successful invocation needs intent, dispatch, observed, verified and published.
        if attempts.len() >= ATTEMPT_LIMIT || generation > MAX_SAFE - 5 {
            return Err(Error::new("capacity", "journal admission limit reached"));
        }
        drop(read);
        let mut tx = Submission {
            slot,
            request,
            pair,
            binding: None,
            last: None,
            failed: false,
        };
        run(&mut tx)?;
        if tx.failed || tx.last != Some(Fact::Published) {
            return Err(Error::unavailable("worker_stopped_before_publication"));
        }
        tx.check_workspace()?;
        point(tx.volume(), "submit", "before_response")?;
        let _read = tx.slot.read_fence()?;
        let (generation, lookup) = tx.slot.lookup_bytes(
            &tx.pair.journal.bytes,
            tx.pair.binding.as_deref(),
            &tx.request,
        )?;
        lookup.at_generation(generation)
    })
}

impl Submission<'_> {
    /// Return the supervisor configuration used to authenticate this submission.
    pub fn host(&self) -> &Host {
        self.slot.host
    }

    pub(crate) fn volume(&self) -> &Volume<'_> {
        &self.slot.volume
    }

    /// Borrow the pinned directory for descriptor-relative target operations.
    pub fn workspace_dir(&self) -> &File {
        &self.slot.handles.workspace
    }

    /// Recheck the healthy volume and the exact files held by this transaction.
    pub fn check_workspace(&self) -> Result<()> {
        self.pair.check(&self.slot)
    }

    /// Reads can also occur inside the final target guard while its pending marker is set.
    pub fn target_io<T>(
        &self,
        reason: &'static str,
        operation: impl FnOnce() -> io::Result<T>,
    ) -> Result<T> {
        self.volume().io(operation(), false, reason)
    }

    fn require_dispatch(&self) -> Result<()> {
        if self.failed || self.last != Some(Fact::Dispatch) {
            return Err(Error::unavailable("target_requires_confirmed_dispatch"));
        }
        self.check_workspace()
    }

    /// Mark a target syscall pending before execution and retain any storage hold.
    pub fn target_mutation<T>(
        &self,
        reason: &'static str,
        operation: impl FnOnce() -> io::Result<T>,
    ) -> Result<T> {
        self.require_dispatch()?;
        self.volume().mutation(reason, operation)
    }

    /// Quarantine on any failed sync or checked writer close.
    pub fn target_writeback<T>(
        &self,
        reason: &'static str,
        operation: impl FnOnce() -> io::Result<T>,
    ) -> Result<T> {
        self.require_dispatch()?;
        self.volume().writeback(reason, operation)
    }

    /// Confirm the original proposal and requests before running the engine.
    pub fn begin(
        &mut self,
        attempt_id: &str,
        run: &AgentRunRequest,
        effect: &ProtectedEffectRequest,
    ) -> Result<String> {
        if self.last.is_some() || self.failed {
            return Err(Error::invalid("attempt_already_started"));
        }
        let Request::Lookup {
            key,
            proposal: Some(proposal),
        } = &self.request
        else {
            return Err(Error::invalid("missing_submission"));
        };
        let host = self.host();
        let proposal_hash = digest(&canonical(proposal)?);
        let run_hash = digest(&canonical(run)?);
        let effect_hash = digest(&canonical(effect)?);
        let binding = digest(&canonical(&[
            &host.slot.workspace_id,
            &host.slot.store_id,
            &host.slot.profile_digest,
            &host.principal,
            key,
            attempt_id,
            &proposal_hash,
            &run_hash,
            &effect_hash,
        ])?);
        let content = proposal["content"]
            .as_str()
            .ok_or_else(|| Error::invalid("missing_proposal_content"))?;
        let attempt = json!({
            "principal":host.principal, "key":key, "attempt_id":attempt_id,
            "proposal":proposal, "proposal_digest":proposal_hash, "run_request":run, "effect_request":effect,
            "facts":[{"tag":"intent","binding":binding,"payload":{"run_request_digest":run_hash,
                "effect_request_digest":effect_hash,"proposed_artifact_digest":digest(content.as_bytes())}}]
        });
        self.advance(Fact::Intent, |attempts| {
            attempts.push(attempt);
            Ok(())
        })?;
        self.binding = Some(binding.clone());
        Ok(binding)
    }

    /// Confirm the actual allowing gates and grant checks before entering the executor.
    pub fn dispatch(&mut self, payload: Value) -> Result<()> {
        self.append(Fact::Dispatch, payload)
    }

    /// Record what the independent witness established about this invocation.
    pub fn observed(&mut self, payload: Value) -> Result<()> {
        self.append(Fact::Observed, payload)
    }

    /// Retain the original engine verification event and measured usage.
    pub fn verified(&mut self, payload: Value) -> Result<()> {
        self.append(Fact::Verified, payload)
    }

    /// Publish the original sealed envelopes and confirm their journal generation.
    pub fn publish(
        &mut self,
        receipt: &TerminalRunReceipt,
        result: Option<&ProtectedEffectResult>,
    ) -> Result<()> {
        self.append(Fact::Published, json!({"receipt":receipt, "result":result}))
    }

    fn append(&mut self, fact: Fact, payload: Value) -> Result<()> {
        let binding = self
            .binding
            .clone()
            .ok_or_else(|| Error::invalid("intent_required"))?;
        self.advance(fact, |attempts| {
            let facts = attempts
                .last_mut()
                .and_then(|attempt| attempt["facts"].as_array_mut())
                .ok_or_else(|| Error::invalid("missing_current_attempt"))?;
            facts.push(json!({"tag":fact.labels().0,"binding":binding,"payload":payload}));
            Ok(())
        })
    }

    fn advance(
        &mut self,
        fact: Fact,
        append: impl FnOnce(&mut Vec<Value>) -> Result<()>,
    ) -> Result<()> {
        if self.failed || self.last == Some(Fact::Published) {
            return Err(Error::invalid("attempt_closed"));
        }
        // A failure anywhere below permanently disables later writes from this invocation.
        self.failed = true;
        self.check_workspace()?;
        if self.pair.generation == MAX_SAFE {
            return Err(Error::new("capacity", "journal_generation_exhausted"));
        }
        let bytes = if self.pair.binding.is_some() {
            let mut candidate = JournalV2::decode(&self.pair.journal.bytes)?;
            candidate.generation += 1;
            append(&mut candidate.attempts)?;
            canonical(&candidate)?
        } else {
            let mut candidate: Journal = serde_json::from_slice(&self.pair.journal.bytes)
                .map_err(|_| Error::invalid("invalid journal shape"))?;
            candidate.generation += 1;
            append(&mut candidate.attempts)?;
            canonical(&candidate)?
        };
        let (tag, confirm) = fact.labels();
        self.pair
            .advance(&self.slot, &self.request, &bytes, tag, confirm)?;
        self.last = Some(fact);
        self.failed = false;
        Ok(())
    }
}
