use std::collections::BTreeSet;
use std::path::Path;

use gaap::contracts::{ContractSupport, ResourceUsage, RunEvent};
use gaap::{Gate, Outcome};
use serde_json::json;

use super::evidence::Index;
use super::publication;
use super::types::*;
use super::{CanonicalObservation, HistoryContext, SAFE_INTEGER, closed, require};
use crate::{Error, Result, canonical, digest, is_digest};

pub(super) fn inspect(
    rest: &[Fact],
    attempt: &Attempt,
    ctx: &HistoryContext<'_>,
    support: &ContractSupport,
    archive: &Index,
    binding: &str,
    intent: &Intent,
) -> Result<(&'static str, bool)> {
    let mut facts = rest.iter().peekable();
    let dispatch = facts
        .next_if(|f| f.tag == "dispatch")
        .map(|f| closed::<Dispatch>(&f.payload))
        .transpose()?;
    if let Some(d) = &dispatch {
        validate_dispatch(d, attempt, ctx, binding, intent, archive)?;
    }
    let observed = facts
        .next_if(|f| f.tag == "observed")
        .map(|f| closed::<Observation>(&f.payload))
        .transpose()?;
    if let Some(o) = &observed {
        let d = dispatch
            .as_ref()
            .ok_or_else(|| Error::invalid("observation precedes dispatch"))?;
        validate_observation(o, d, attempt, ctx, binding, intent, archive)?;
    }
    let verified = facts
        .next_if(|f| f.tag == "verified")
        .map(|f| closed::<Verification>(&f.payload))
        .transpose()?;
    if let Some(v) = &verified {
        let o = observed
            .as_ref()
            .ok_or_else(|| Error::invalid("verification precedes observation"))?;
        let last_gate = dispatch.as_ref().and_then(|d| d.decisions.last());
        require(
            last_gate.is_some_and(|gate| v.event.sequence() > gate.sequence()),
            "verification precedes the final dispatch gate",
        )?;
        validate_verification(v, o, ctx, attempt, binding, archive)?;
    }
    let published = facts
        .next_if(|f| f.tag == "published")
        .map(|f| closed::<Publication>(&f.payload))
        .transpose()?;
    require(
        facts.next().is_none(),
        "unknown, misplaced or repeated historical fact",
    )?;
    let hold = match &observed {
        None => dispatch.is_some(),
        Some(o) => o.disposition == Disposition::Unresolved,
    };
    if let Some(p) = &published {
        require(
            dispatch.is_none() || observed.is_some(),
            "publication skips a dispatch observation",
        )?;
        publication::validate(
            p,
            attempt,
            support,
            archive,
            dispatch.as_ref(),
            observed.as_ref(),
            verified.as_ref(),
        )?;
        return Ok(("published", hold));
    }
    Ok(("partial", hold))
}

pub(super) fn usage_at_least(later: &ResourceUsage, floor: &ResourceUsage) -> Result<()> {
    require(
        [
            (later.cost_micros, floor.cost_micros),
            (later.elapsed_ms, floor.elapsed_ms),
            (later.model_tokens, floor.model_tokens),
            (later.tool_calls, floor.tool_calls),
        ]
        .iter()
        .all(|&(actual, known)| actual <= SAFE_INTEGER && actual >= known),
        "historical resource usage fell below an earlier bound or exceeded the safe range",
    )
}

fn validate_file(obs: &FileObservation, attempt: &Attempt, ctx: &HistoryContext<'_>) -> Result<()> {
    let file = &obs.file;
    let expected = Path::new(&ctx.workspace.path).join(&attempt.proposal.path);
    require(
        expected.to_str() == Some(file.path.as_str())
            && file.dev == ctx.workspace.dev
            && file.uid == ctx.workspace.uid
            && file.mode <= 0o7777
            && file
                .ino
                .parse::<u64>()
                .is_ok_and(|n| n.to_string() == file.ino)
            && obs.subject.kind == attempt.run_request.subject.kind
            && obs.subject.locator == attempt.run_request.subject.locator
            && is_digest(&obs.subject.digest)
            && is_digest(&obs.artifact_digest),
        "historical file observation differs from the enrolled file binding",
    )
}

fn validate_dispatch(
    d: &Dispatch,
    attempt: &Attempt,
    ctx: &HistoryContext<'_>,
    binding: &str,
    intent: &Intent,
    archive: &Index,
) -> Result<()> {
    let effect = &attempt.effect_request;
    let profile = ctx.profile;
    validate_file(&d.before, attempt, ctx)?;
    require(
        d.before.subject == effect.subject
            && d.executor == profile.executor
            && d.checked_at_ms <= SAFE_INTEGER,
        "dispatch observation, executor or check time disagrees with the admission",
    )?;
    let budget = &attempt.run_request.resource_budget;
    let reservation = [
        (
            d.reserved.cost_micros,
            d.usage_floor.cost_micros,
            budget.max_cost_micros,
        ),
        (
            d.reserved.model_tokens,
            d.usage_floor.model_tokens,
            budget.max_model_tokens,
        ),
        (
            d.reserved.elapsed_ms,
            d.usage_floor.elapsed_ms,
            budget.max_elapsed_ms,
        ),
        (
            d.reserved.tool_calls,
            d.usage_floor.tool_calls,
            budget.max_tool_calls,
        ),
    ];
    require(
        reservation
            .iter()
            .all(|&(r, u, b)| u.checked_add(r).is_some_and(|v| v <= b)),
        "dispatch reservation exceeds available authority",
    )?;
    let gates = [
        Gate::Permission,
        Gate::ToolTrust,
        Gate::Runtime,
        Gate::Workflow,
    ];
    require(
        d.decisions.len() == gates.len(),
        "incomplete dispatch gate history",
    )?;
    let mut previous = 0;
    let mut identifiers = BTreeSet::new();
    for (gate, decision) in gates.into_iter().zip(&d.decisions) {
        let RunEvent::ProtectedEffectDecision {
            sequence,
            decision_id,
            gate: actual,
            protected_effect_digest,
            subject_digest,
            decision,
        } = decision
        else {
            return Err(Error::invalid("invalid dispatch decision event"));
        };
        require(
            *actual == gate
                && *sequence > previous
                && *sequence <= SAFE_INTEGER
                && !decision_id.trim().is_empty()
                && identifiers.insert(decision_id)
                && protected_effect_digest == &intent.effect_request_digest
                && subject_digest == &d.before.subject.digest
                && decision.outcome == Outcome::Allow
                && !decision.code.trim().is_empty()
                && decision.effects.iter().all(|s| !s.trim().is_empty())
                && (gate != Gate::Workflow || decision.code == "workflow.mutation_authorized"),
            "dispatch gate chronology or subject binding mismatch",
        )?;
        previous = *sequence;
    }

    let approvals = attempt
        .run_request
        .approval_context
        .iter()
        .chain(&effect.approval_context)
        .collect::<Vec<_>>();
    let mut checked = BTreeSet::new();
    for check in &d.checks {
        require(
            approvals.contains(&&check.approval) && checked.insert(&check.approval.approval_id),
            "unknown or repeated historical approval",
        )?;
        archive.grant(&profile.grant_observer, check, binding, d.checked_at_ms)?;
    }
    require(
        (!profile.require_grant || !checked.is_empty())
            && approvals
                .iter()
                .all(|a| d.checks.iter().any(|c| &c.approval == *a)),
        "dispatch lacks a required historical grant check",
    )
}

fn validate_observation(
    o: &Observation,
    d: &Dispatch,
    attempt: &Attempt,
    ctx: &HistoryContext<'_>,
    binding: &str,
    intent: &Intent,
    archive: &Index,
) -> Result<()> {
    usage_at_least(&o.usage_floor, &d.usage_floor)?;
    match (&o.disposition, &o.after) {
        (Disposition::Replaced, Some(after)) => {
            validate_file(after, attempt, ctx)?;
            require(
                after.artifact_digest == intent.proposed_artifact_digest
                    && after.file.ino != d.before.file.ino
                    && o.usage_floor.tool_calls > d.usage_floor.tool_calls,
                "replacement lacks a new file identity, bound bytes or attempted tool usage",
            )?;
        }
        (
            Disposition::NotStarted | Disposition::StoppedBeforeRename | Disposition::Unresolved,
            None,
        ) => {}
        _ => {
            return Err(Error::invalid(
                "observation disposition disagrees with the retained output",
            ));
        }
    }
    match &o.source {
        Some(source) => archive.observation(
            &ctx.profile.effect_observer,
            source,
            CanonicalObservation {
                binding,
                before: &d.before,
                observation: o,
            },
        ),
        None => require(
            o.disposition == Disposition::Unresolved,
            "known outcome lacks independent evidence",
        ),
    }
}

fn validate_verification(
    v: &Verification,
    o: &Observation,
    ctx: &HistoryContext<'_>,
    attempt: &Attempt,
    binding: &str,
    archive: &Index,
) -> Result<()> {
    let after = o
        .after
        .as_ref()
        .ok_or_else(|| Error::invalid("verification requires an observed replacement"))?;
    usage_at_least(&v.usage_floor, &o.usage_floor)?;
    let RunEvent::Verification {
        sequence,
        subject_digest,
        implementer_id,
        verifier_id,
        evidence,
        ..
    } = &v.event
    else {
        return Err(Error::invalid("invalid verification event"));
    };
    require(
        *sequence > 0
            && *sequence <= SAFE_INTEGER
            && subject_digest == &after.subject.digest
            && implementer_id == &ctx.profile.implementer_id
            && verifier_id == &ctx.profile.verifier_id
            && !evidence.is_empty(),
        "verification lacks a matching subject, trusted actor or evidence",
    )?;
    for reference in evidence {
        archive.reference(verifier_id, reference)?;
    }
    // The archive binds the actual verifier response to this invocation as well as to
    // its output. Raw verifier output or a byte-identical file alone can't do that.
    let bytes = canonical(
        &json!({"kind":"verification", "actor_id":verifier_id, "binding":binding, "event":v.event}),
    )?;
    let locator = format!("gaap:journal/verification/{}", attempt.attempt_id);
    archive.bytes(verifier_id, Some(&locator), &digest(&bytes))?;
    if matches!(
        &v.event,
        RunEvent::Verification {
            verdict: gaap::contracts::VerificationVerdict::Pass,
            ..
        }
    ) {
        require(
            attempt
                .run_request
                .required_verification
                .evidence_types
                .iter()
                .all(|kind| evidence.iter().any(|r| r.evidence_type == *kind)),
            "passing verifier omitted required evidence",
        )?;
    }
    Ok(())
}
