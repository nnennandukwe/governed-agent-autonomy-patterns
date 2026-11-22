//! Read-side validation of an entire retained generation, before selecting a submission.
mod evidence;
mod facts;
mod publication;
mod types;

use std::collections::BTreeSet;
use std::path::{Component, Path};

use gaap::contracts::{
    ContractSupport, EffectClass, FilesystemAccess, OperationFamily, RequestedScope, SubjectKind,
    validate_protected_effect_request, validate_request,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::{Error, Identity, Lookup, Result, canonical, digest, is_digest, token};
use evidence::Index;
pub(crate) use types::Attempt;
use types::{Change, Intent};

pub use types::{ArchiveEntry, EvidenceArchive, HistoricalProfile};

use crate::{ATTEMPT_LIMIT, MAX_SAFE as SAFE_INTEGER, PROPOSAL_LIMIT};

pub struct HistoryContext<'a> {
    pub workspace_id: &'a str,
    pub store_id: &'a str,
    pub profile_digest: &'a str,
    pub workspace: &'a Identity,
    pub profile: &'a HistoricalProfile,
    pub archive: &'a EvidenceArchive,
}

fn require(condition: bool, reason: &'static str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(Error::invalid(reason))
    }
}

// Serde accepts missing Option fields as null. The closed journal grammar requires both
// a typed parse and an exact field set, including every explicitly nullable member.
pub(crate) fn closed<T: DeserializeOwned + Serialize>(value: &Value) -> Result<T> {
    let parsed: T = serde_json::from_value(value.clone())
        .map_err(|_| Error::invalid("invalid history object shape"))?;
    let normalized = serde_json::to_value(&parsed)
        .map_err(|_| Error::invalid("unrepresentable history object"))?;
    require(
        normalized == *value,
        "missing or noncanonical history member",
    )?;
    Ok(parsed)
}

impl HistoricalProfile {
    pub fn validate(&self) -> Result<()> {
        if self.format != "gaap.local-file-history/0.1" {
            return Err(Error::unsupported("unsupported historical decoder"));
        }
        let valid = |name: &str, version: &str, hash: &str| {
            !name.trim().is_empty() && !version.trim().is_empty() && is_digest(hash)
        };
        require(
            !self.policies.is_empty()
                && self
                    .policies
                    .iter()
                    .all(|p| valid(&p.name, &p.version, &p.digest))
                && self.policies.iter().collect::<BTreeSet<_>>().len() == self.policies.len()
                && valid(
                    &self.capability.name,
                    &self.capability.version,
                    &self.capability.digest,
                )
                && valid(
                    &self.executor.name,
                    &self.executor.version,
                    &self.executor.digest,
                )
                && valid(
                    &self.sandbox.name,
                    &self.sandbox.version,
                    &self.sandbox.digest,
                )
                && is_digest(&self.tool_schema_digest)
                && [
                    &self.implementer_id,
                    &self.verifier_id,
                    &self.grant_observer,
                    &self.effect_observer,
                ]
                .iter()
                .all(|v| !v.trim().is_empty())
                && self.verifier_id != self.implementer_id
                && self.effect_observer != self.implementer_id
                && self.effect_observer != self.executor.name,
            "invalid retained history profile or independent actor roles",
        )
    }
}

fn validate_change(change: &Change) -> Result<()> {
    let path = Path::new(&change.path);
    let normal = !change.path.is_empty()
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        && change
            .path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
        && !change.path.contains('\0');
    require(
        normal
            && change.path.len() <= 200
            && change.content.len() <= 32768
            && !change.plan.trim().is_empty()
            && change.plan.len() <= 4096
            && is_digest(&change.base_digest)
            && canonical(change)?.len() <= PROPOSAL_LIMIT,
        "invalid local file proposal",
    )
}

pub fn validate_proposal(proposal: &Value) -> Result<()> {
    validate_change(&closed(proposal)?)
}

pub fn lookup(
    attempts: &[Value],
    ctx: &HistoryContext<'_>,
    principal: &str,
    key: &str,
    proposal: Option<&Value>,
) -> Result<Lookup> {
    require(
        token(principal) && token(key),
        "invalid principal or submission key",
    )?;
    if let Some(input) = proposal {
        validate_proposal(input)?;
    }
    let mut lookup = Lookup::absent();
    visit(attempts, ctx, |raw, attempt, status, hold| {
        require(
            !lookup.workspace_hold || !attempt.facts.iter().any(|fact| fact.tag == "dispatch"),
            "later dispatch crosses a workspace hold",
        )?;
        lookup.workspace_hold |= hold;
        if attempt.principal == principal && attempt.key == key {
            select(&mut lookup, raw, status, proposal);
        }
        Ok(())
    })?;
    Ok(lookup)
}

// Both formats validate every original attempt before releasing any selected data.
pub(crate) fn visit(
    attempts: &[Value],
    ctx: &HistoryContext<'_>,
    mut checked: impl FnMut(&Value, &Attempt, &'static str, bool) -> Result<()>,
) -> Result<()> {
    require(
        attempts.len() <= ATTEMPT_LIMIT,
        "history exceeds the retained-attempt limit",
    )?;
    ctx.profile.validate()?;
    let archive = ctx.archive.index()?;
    let support = ContractSupport::new(ctx.profile.policies.iter().cloned());
    let mut keys = BTreeSet::new();
    let mut identities = BTreeSet::new();
    for raw in attempts {
        let attempt: Attempt = closed(raw)?;
        require(
            keys.insert((attempt.principal.clone(), attempt.key.clone()))
                && [
                    &attempt.attempt_id,
                    &attempt.run_request.request_id,
                    &attempt.run_request.run_id,
                    &attempt.effect_request.effect_id,
                    &attempt.effect_request.idempotency_key,
                ]
                .into_iter()
                .enumerate()
                .all(|(kind, id)| identities.insert((kind, id.clone()))),
            "duplicate historical admission identity",
        )?;
        let (status, hold) = inspect(&attempt, ctx, &support, &archive)?;
        checked(raw, &attempt, status, hold)?;
    }
    Ok(())
}

pub(crate) fn select(
    result: &mut Lookup,
    raw: &Value,
    status: &'static str,
    proposal: Option<&Value>,
) {
    // The closed local proposal contains strings only. JSON value equality compares exactly
    // the object members and their values, independent of their serialized key order.
    if proposal.is_some_and(|candidate| raw.get("proposal") != Some(candidate)) {
        result.status = "conflict";
    } else {
        result.status = status;
        result.attempt = Some(raw.clone());
    }
}

fn inspect(
    attempt: &Attempt,
    ctx: &HistoryContext<'_>,
    support: &ContractSupport,
    archive: &Index,
) -> Result<(&'static str, bool)> {
    let run = &attempt.run_request;
    let effect = &attempt.effect_request;
    require(
        token(&attempt.principal)
            && token(&attempt.key)
            && attempt.attempt_id.len() == 32
            && attempt
                .attempt_id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid admission identity",
    )?;
    validate_change(&attempt.proposal)?;
    let run_hash = validate_request(run, support)
        .map_err(|_| Error::invalid("invalid original run request"))?;
    let effect_hash = validate_protected_effect_request(run, support, effect)
        .map_err(|_| Error::invalid("invalid original effect request"))?;
    let target = Path::new(&ctx.workspace.path).join(&attempt.proposal.path);
    let subject_path = match run.subject.kind {
        SubjectKind::Repository => Path::new(&ctx.workspace.path),
        SubjectKind::Artifact => target.as_path(),
    };
    require(
        run.subject.digest == attempt.proposal.base_digest
            && run.subject == effect.subject
            && subject_path.to_str() == Some(run.subject.locator.as_str())
            && run.task.instructions == attempt.proposal.plan
            && run.requested_capability == ctx.profile.capability
            && effect.tool_schema_digest.as_ref() == Some(&ctx.profile.tool_schema_digest)
            && effect.sandbox_profile == ctx.profile.sandbox
            && effect.operation_family == OperationFamily::Filesystem
            && effect.normalized_operation == "filesystem.write"
            && effect.expected_effect_class == EffectClass::Mutation
            && effect.effect_sequence == 1
            && effect
                .input_metadata
                .iter()
                .any(|m| m.name == "path" && m.value == attempt.proposal.path)
            && matches!(effect.requested_scopes.as_slice(),
                [RequestedScope::Filesystem { root, access, recursive: false }] if root == &ctx.workspace.path
                   && access.contains(&FilesystemAccess::Modify)
                   && access.iter().all(|a| matches!(a, FilesystemAccess::Read | FilesystemAccess::Modify)))
            && digest(&canonical(&attempt.proposal)?) == attempt.proposal_digest
            && effect.input_digest == attempt.proposal_digest,
        "proposal, enrolled file profile and original requests disagree",
    )?;
    let binding = digest(&canonical(&[
        ctx.workspace_id,
        ctx.store_id,
        ctx.profile_digest,
        &attempt.principal,
        &attempt.key,
        &attempt.attempt_id,
        &attempt.proposal_digest,
        &run_hash,
        &effect_hash,
    ])?);
    let (first, rest) = attempt
        .facts
        .split_first()
        .ok_or_else(|| Error::invalid("historical attempt lacks an intent"))?;
    require(
        first.tag == "intent"
            && attempt.facts.len() <= 5
            && attempt.facts.iter().all(|fact| fact.binding == binding),
        "invalid fact count, binding or initial tag",
    )?;
    let intent: Intent = closed(&first.payload)?;
    require(
        intent.run_request_digest == run_hash
            && intent.effect_request_digest == effect_hash
            && intent.proposed_artifact_digest == digest(attempt.proposal.content.as_bytes()),
        "intent digests disagree with retained input",
    )?;
    facts::inspect(rest, attempt, ctx, support, archive, &binding, &intent)
}

struct CanonicalObservation<'a> {
    binding: &'a str,
    before: &'a types::FileObservation,
    observation: &'a types::Observation,
}

#[cfg(test)]
mod tests;
