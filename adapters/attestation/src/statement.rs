//! Versioned in-toto evidence, verified without controller lifecycle authority.
use crate::{AttestationError, dsse, error, json, snapshot};
use gaap::contracts::{
    AGENT_RUN_REQUEST_SCHEMA, AgentRunRequest, AgentRunStatus, ContractSupport, EffectClass,
    EffectExecutionStatus, EvidenceType, PROTECTED_EFFECT_REQUEST_SCHEMA,
    PROTECTED_EFFECT_RESULT_SCHEMA, ProtectedEffectRequest, ProtectedEffectResult, RunEvent,
    Subject, TERMINAL_RUN_RECEIPT_SCHEMA, TerminalRunReceipt, validate_protected_effect_request,
    verify_protected_effect_result, verify_terminal_receipt,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const PAYLOAD_TYPE: &str = "application/vnd.in-toto+json";
pub const STATEMENT_TYPE: &str = "https://in-toto.io/Statement/v1";
pub const PREDICATE_TYPE: &str =
    "https://github.com/nnennandukwe/governed-agent-autonomy-patterns/attestation/terminal-run/v1";
pub const MAX_EFFECTS: usize = 128;
pub const MAX_BLOBS: usize = 1024;
pub const MAX_BLOB_BYTES: usize = 1024 * 1024;
pub const MAX_TOTAL_BLOB_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Statement {
    #[serde(rename = "_type")]
    #[schemars(schema_with = "statement_type_schema")]
    pub statement_type: String,
    #[schemars(length(min = 1, max = 1))]
    pub subject: Vec<StatementSubject>,
    #[schemars(schema_with = "predicate_type_schema")]
    pub predicate_type: String,
    pub predicate: Predicate,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StatementSubject {
    pub name: String,
    pub digest: BTreeMap<String, String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Implementation {
    #[schemars(length(min = 1))]
    pub name: String,
    #[schemars(length(min = 1))]
    pub version: String,
    #[schemars(regex(pattern = r"^sha256:[0-9a-f]{64}$"))]
    pub digest: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EffectRecord {
    pub request: ProtectedEffectRequest,
    pub result: ProtectedEffectResult,
}
/// Full contracts retain approvals, plan decisions, usage, sandbox and uncertain outcomes.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Predicate {
    pub implementation: Implementation,
    #[schemars(schema_with = "contract_versions_schema")]
    pub contract_versions: Vec<String>,
    pub request: AgentRunRequest,
    pub receipt: TerminalRunReceipt,
    #[schemars(length(max = 128))]
    pub effects: Vec<EffectRecord>,
    /// Fresh physical observation; a non-success ledger can retain an earlier subject.
    pub observed_subject: Subject,
    #[schemars(schema_with = "snapshot_profile_schema")]
    pub snapshot_profile: String,
    /// Every initial, admitted and observed snapshot, plus its file bytes, is retained.
    #[schemars(
        length(min = 1, max = 131),
        inner(regex(pattern = r"^sha256:[0-9a-f]{64}$"))
    )]
    pub snapshot_digests: Vec<String>,
    /// All listed bytes are checked; other contract references remain metadata claims.
    #[schemars(
        length(min = 1, max = 1024),
        inner(regex(pattern = r"^sha256:[0-9a-f]{64}$"))
    )]
    pub blob_digests: Vec<String>,
}

/// Hosts supply bounded content bytes. The adapter never follows receipt locators.
pub trait BlobReader {
    fn read(&mut self, digest: &str, max_bytes: usize) -> Result<Vec<u8>, AttestationError>;
}
impl BlobReader for BTreeMap<String, Vec<u8>> {
    fn read(&mut self, digest: &str, max_bytes: usize) -> Result<Vec<u8>, AttestationError> {
        let bytes = self
            .get(digest)
            .ok_or_else(|| error("evidence.missing_blob"))?;
        if bytes.len() > max_bytes {
            return Err(error("evidence.blob_limit"));
        }
        Ok(bytes.clone())
    }
}

/// Private fields prevent an unsigned statement from posing as verified evidence.
/// A valid signature and receipt still aren't permission for a controller transition.
pub struct VerifiedAttestation {
    statement: Statement,
    envelope: dsse::VerifiedEnvelope,
    unresolved_references: Vec<String>,
}
impl VerifiedAttestation {
    pub fn statement(&self) -> &Statement {
        &self.statement
    }
    pub fn signed_payload(&self) -> &[u8] {
        self.envelope.payload()
    }
    pub fn keys(&self) -> &[dsse::VerifiedKey] {
        self.envelope.keys()
    }
    pub fn unresolved_references(&self) -> &[String] {
        &self.unresolved_references
    }
}

pub fn contract_versions() -> Vec<String> {
    [
        AGENT_RUN_REQUEST_SCHEMA,
        TERMINAL_RUN_RECEIPT_SCHEMA,
        PROTECTED_EFFECT_REQUEST_SCHEMA,
        PROTECTED_EFFECT_RESULT_SCHEMA,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}
pub fn new_statement(predicate: Predicate) -> Statement {
    let observed = &predicate.observed_subject;
    Statement {
        statement_type: STATEMENT_TYPE.into(),
        subject: vec![StatementSubject {
            name: observed.locator.clone(),
            digest: BTreeMap::from([(
                "sha256".into(),
                observed.digest.strip_prefix("sha256:").unwrap_or("").into(),
            )]),
        }],
        predicate_type: PREDICATE_TYPE.into(),
        predicate,
    }
}

/// Signing validates semantics/content but doesn't create externally trusted evidence.
pub fn sign(
    statement: &Statement,
    support: &ContractSupport,
    blobs: &mut impl BlobReader,
    signer: &mut impl dsse::Signer,
) -> Result<dsse::Envelope, AttestationError> {
    validate(statement, support, blobs)?;
    let payload =
        serde_jcs::to_vec(statement).map_err(|_| error("attestation.invalid_statement"))?;
    // Apply the same I-JSON profile to producer bytes as the external consumer.
    json::parse(&payload)?;
    dsse::sign(PAYLOAD_TYPE, &payload, signer)
}
pub fn verify(
    envelope: &dsse::Envelope,
    support: &ContractSupport,
    blobs: &mut impl BlobReader,
    verifier: &mut impl dsse::SignatureVerifier,
) -> Result<VerifiedAttestation, AttestationError> {
    // Authenticate raw bytes before interpreting any predicate-controlled fields.
    let envelope = dsse::verify(envelope, PAYLOAD_TYPE, verifier)?;
    let statement: Statement = serde_json::from_value(json::parse(envelope.payload())?)
        .map_err(|_| error("attestation.invalid_statement"))?;
    let unresolved_references = validate(&statement, support, blobs)?;
    Ok(VerifiedAttestation {
        statement,
        envelope,
        unresolved_references,
    })
}

fn validate(
    statement: &Statement,
    support: &ContractSupport,
    blobs: &mut impl BlobReader,
) -> Result<Vec<String>, AttestationError> {
    let predicate = &statement.predicate;
    if statement.statement_type != STATEMENT_TYPE
        || statement.predicate_type != PREDICATE_TYPE
        || predicate.contract_versions != contract_versions()
        || predicate.snapshot_profile != snapshot::SNAPSHOT_PROFILE
    {
        return Err(error("attestation.unsupported_version"));
    }
    let implementation = &predicate.implementation;
    snapshot::validate_digest(&implementation.digest)?;
    if implementation.name.trim().is_empty() || implementation.version.trim().is_empty() {
        return Err(error("attestation.invalid_implementation"));
    }
    snapshot::validate_digest(&predicate.observed_subject.digest)?;
    if predicate.observed_subject.kind != predicate.request.subject.kind
        || predicate.observed_subject.locator != predicate.request.subject.locator
        || statement.subject.len() != 1
        || statement.subject[0].name != predicate.observed_subject.locator
        || statement.subject[0].digest
            != BTreeMap::from([(
                "sha256".into(),
                predicate.observed_subject.digest[7..].into(),
            )])
    {
        return Err(error("attestation.subject_mismatch"));
    }
    verify_terminal_receipt(&predicate.request, support, &predicate.receipt)
        .map_err(|_| error("attestation.invalid_receipt"))?;
    if predicate.receipt.body.terminal_status != AgentRunStatus::Interrupted
        && predicate
            .receipt
            .body
            .events
            .iter()
            .any(|event| matches!(event, RunEvent::Interruption { .. }))
    {
        return Err(error("attestation.interruption_status_mismatch"));
    }
    if predicate.receipt.body.terminal_status == AgentRunStatus::Completed
        && predicate.observed_subject.digest != predicate.receipt.body.resulting_subject_digest
    {
        return Err(error("attestation.stale_completion"));
    }
    validate_effects(predicate, support)?;
    validate_content(predicate, blobs)
}

fn validate_effects(
    predicate: &Predicate,
    support: &ContractSupport,
) -> Result<(), AttestationError> {
    if predicate.effects.len() > MAX_EFFECTS {
        return Err(error("attestation.effect_limit"));
    }
    let events = &predicate.receipt.body.events;
    let mut seen = BTreeSet::new();
    let mut order = Vec::new();
    for event in events {
        if let RunEvent::ProtectedEffectDecision {
            protected_effect_digest,
            subject_digest,
            gate,
            decision,
            ..
        } = event
            && !(*gate == gaap::Gate::Workflow
                && protected_effect_digest == subject_digest
                && matches!(
                    decision.code.as_str(),
                    "workflow.completion_authorized"
                        | "workflow.mutation_not_authorized"
                        | "workflow.verification_failed"
                ))
            && seen.insert(protected_effect_digest)
        {
            order.push(protected_effect_digest);
        }
    }
    if order.len() != predicate.effects.len() {
        return Err(error("attestation.missing_effect"));
    }
    let mut ids = BTreeSet::new();
    let mut current_subject = &predicate.request.subject.digest;
    let mut sums = [0u64; 4];
    for (index, effect) in predicate.effects.iter().enumerate() {
        let request = &effect.request;
        let result = &effect.result;
        for subject in std::iter::once(&request.subject)
            .chain(std::iter::once(&result.body.observed_pre_effect_subject))
            .chain(result.body.observed_post_effect_subject.iter())
        {
            if subject.kind != predicate.request.subject.kind
                || subject.locator != predicate.request.subject.locator
            {
                return Err(error("attestation.effect_subject_scope"));
            }
        }
        let digest = validate_protected_effect_request(&predicate.request, support, request)
            .map_err(|_| error("attestation.invalid_effect_request"))?;
        verify_protected_effect_result(&predicate.request, support, request, result)
            .map_err(|_| error("attestation.invalid_effect_result"))?;
        if &digest != order[index]
            || !ids.insert(&request.effect_id)
            || request.effect_sequence != index as u64 + 1
            || &request.subject.digest != current_subject
        {
            return Err(error("attestation.effect_order_or_subject"));
        }
        let decision = &result.body.decision;
        if !events.iter().any(|event| matches!(event,
            RunEvent::ProtectedEffectDecision { decision_id, gate, protected_effect_digest, subject_digest, decision: value, .. }
            if decision_id == &decision.decision_id && gate == &decision.gate && protected_effect_digest == &digest
                && subject_digest == &decision.subject_digest && value == &decision.decision
        )) { return Err(error("attestation.effect_decision_mismatch")); }
        let executions: Vec<_> = events.iter().filter(|event| matches!(event,
            RunEvent::ToolExecution { protected_effect_digest, .. } if protected_effect_digest == &digest
        )).collect();
        let attempted = !matches!(
            result.body.execution_status,
            EffectExecutionStatus::Denied | EffectExecutionStatus::AwaitingAuthority
        );
        if attempted && result.body.usage.tool_calls == 0 {
            return Err(error("attestation.missing_known_attempt_usage"));
        }
        if executions.len() != usize::from(attempted) {
            return Err(error("attestation.execution_mismatch"));
        }
        if let Some(RunEvent::ToolExecution { sequence, .. }) = executions.first().copied() {
            validate_admission(events, &digest, request, *sequence)?;
        }
        if let Some(RunEvent::ToolExecution {
            action_digest,
            capability_digest,
            evidence,
            decision_id,
            ..
        }) = executions.first().copied()
            && (action_digest != &digest
                || capability_digest != &request.capability.digest
                || decision_id != &decision.decision_id
                || !evidence.iter().any(|reference| {
                    reference.evidence_type == EvidenceType::ToolExecution
                        && reference.digest == result.result_digest
                }))
        {
            return Err(error("attestation.result_evidence_mismatch"));
        }
        let mutations: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                RunEvent::Mutation {
                    protected_effect_digest,
                    before_subject_digest,
                    after_subject_digest,
                    ..
                } if protected_effect_digest == &digest => {
                    Some((before_subject_digest, after_subject_digest))
                }
                _ => None,
            })
            .collect();
        let expected_mutation = attempted
            && request.expected_effect_class == EffectClass::Mutation
            && result.body.observed_post_effect_subject.is_some();
        if mutations.len() != usize::from(expected_mutation) {
            return Err(error("attestation.mutation_mismatch"));
        }
        if let Some((before, after)) = mutations.first() {
            if *before != current_subject
                || Some(after.as_str())
                    != result
                        .body
                        .observed_post_effect_subject
                        .as_ref()
                        .map(|subject| subject.digest.as_str())
            {
                return Err(error("attestation.mutation_subject_mismatch"));
            }
            current_subject = after;
        }
        for (sum, value) in sums.iter_mut().zip([
            result.body.usage.cost_micros,
            result.body.usage.elapsed_ms,
            result.body.usage.model_tokens,
            result.body.usage.tool_calls,
        ]) {
            *sum = sum
                .checked_add(value)
                .ok_or_else(|| error("attestation.usage_overflow"))?;
        }
        if predicate.receipt.body.terminal_status == AgentRunStatus::Completed
            && result.body.execution_status != EffectExecutionStatus::Executed
        {
            return Err(error("attestation.non_success_effect"));
        }
    }
    let usage = &predicate.receipt.body.usage;
    if current_subject != &predicate.receipt.body.resulting_subject_digest
        || sums
            .iter()
            .zip([
                usage.cost_micros,
                usage.elapsed_ms,
                usage.model_tokens,
                usage.tool_calls,
            ])
            .any(|(sum, total)| *sum > total)
    {
        return Err(error("attestation.subject_or_usage_mismatch"));
    }
    Ok(())
}

fn validate_admission(
    events: &[RunEvent],
    digest: &str,
    request: &ProtectedEffectRequest,
    execution_sequence: u64,
) -> Result<(), AttestationError> {
    let mut required = vec![
        gaap::Gate::Permission,
        gaap::Gate::ToolTrust,
        gaap::Gate::Runtime,
    ];
    if request.expected_effect_class == EffectClass::Mutation {
        required.push(gaap::Gate::Workflow);
    }
    let decisions: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            RunEvent::ProtectedEffectDecision {
                sequence,
                protected_effect_digest,
                subject_digest,
                gate,
                decision,
                ..
            } if protected_effect_digest == digest => {
                Some((*sequence, *gate, subject_digest, decision))
            }
            _ => None,
        })
        .collect();
    if decisions.iter().any(|(_, _, subject, decision)| {
        *subject != &request.subject.digest || decision.outcome != gaap::Outcome::Allow
    }) || required.iter().any(|required_gate| {
        !decisions.iter().any(|(sequence, gate, _, decision)| {
            gate == required_gate
                && *sequence < execution_sequence
                && (*gate != gaap::Gate::Workflow
                    || decision.code == "workflow.mutation_authorized")
        })
    }) {
        return Err(error("attestation.gates_not_admitted"));
    }
    Ok(())
}

fn validate_content(
    predicate: &Predicate,
    reader: &mut impl BlobReader,
) -> Result<Vec<String>, AttestationError> {
    if predicate.blob_digests.len() > MAX_BLOBS
        || predicate.snapshot_digests.len() > MAX_EFFECTS + 3
    {
        return Err(error("evidence.blob_limit"));
    }
    let mut blobs = BTreeMap::new();
    let mut total = 0usize;
    for digest in &predicate.blob_digests {
        snapshot::validate_digest(digest)?;
        let bytes = reader.read(digest, MAX_BLOB_BYTES)?;
        total = total
            .checked_add(bytes.len())
            .ok_or_else(|| error("evidence.blob_limit"))?;
        if bytes.len() > MAX_BLOB_BYTES || total > MAX_TOTAL_BLOB_BYTES {
            return Err(error("evidence.blob_limit"));
        }
        if snapshot::content_digest(&bytes) != *digest || blobs.insert(digest, bytes).is_some() {
            return Err(error("evidence.digest_mismatch"));
        }
    }
    let snapshots: BTreeSet<_> = predicate.snapshot_digests.iter().collect();
    if snapshots.len() != predicate.snapshot_digests.len() {
        return Err(error("evidence.duplicate_snapshot"));
    }
    let mut required = BTreeSet::from([
        &predicate.request.subject.digest,
        &predicate.receipt.body.resulting_subject_digest,
        &predicate.observed_subject.digest,
    ]);
    for effect in &predicate.effects {
        for digest in std::iter::once(&effect.request.input_digest)
            .chain(effect.request.tool_schema_digest.iter())
            .chain(effect.result.body.observed_tool_schema_digest.iter())
        {
            if !blobs.contains_key(digest) {
                return Err(error("evidence.missing_input_or_schema"));
            }
        }
        required.insert(&effect.request.subject.digest);
        required.insert(&effect.result.body.observed_pre_effect_subject.digest);
        if let Some(subject) = &effect.result.body.observed_post_effect_subject {
            required.insert(&subject.digest);
        }
    }
    if !required.is_subset(&snapshots) {
        return Err(error("evidence.missing_snapshot"));
    }
    for digest in snapshots {
        let bytes = blobs
            .get(digest)
            .ok_or_else(|| error("evidence.missing_blob"))?;
        let manifest = snapshot::SnapshotManifest::parse(bytes)?;
        if manifest.digest()? != *digest {
            return Err(error("evidence.noncanonical_snapshot"));
        }
        for entry in manifest.entries().values() {
            if let snapshot::SnapshotEntry::File { digest, .. } = entry {
                let bytes = blobs
                    .get(digest)
                    .ok_or_else(|| error("evidence.missing_file"))?;
                if bytes.len() > snapshot::MAX_FILE_BYTES {
                    return Err(error("evidence.file_limit"));
                }
            }
        }
    }
    let mut unresolved = BTreeSet::new();
    for approval in predicate.request.approval_context.iter().chain(
        predicate
            .effects
            .iter()
            .flat_map(|effect| &effect.request.approval_context),
    ) {
        if !blobs.contains_key(&approval.evidence.digest) {
            unresolved.insert(approval.evidence.digest.clone());
        }
    }
    for event in &predicate.receipt.body.events {
        let references = match event {
            RunEvent::Verification { evidence, .. } => {
                for reference in evidence {
                    if !blobs.contains_key(&reference.digest) {
                        return Err(error("evidence.missing_verification"));
                    }
                }
                evidence.as_slice()
            }
            RunEvent::ApprovalRecorded { approval, .. } => std::slice::from_ref(&approval.evidence),
            RunEvent::ToolExecution { evidence, .. } | RunEvent::Mutation { evidence, .. } => {
                evidence.as_slice()
            }
            RunEvent::Interruption { evidence, .. } => std::slice::from_ref(evidence),
            _ => &[],
        };
        for reference in references {
            if !blobs.contains_key(&reference.digest)
                && !predicate
                    .effects
                    .iter()
                    .any(|effect| effect.result.result_digest == reference.digest)
            {
                unresolved.insert(reference.digest.clone());
            }
        }
    }
    for effect in &predicate.effects {
        for reference in &effect.result.body.evidence {
            if !blobs.contains_key(&reference.digest) {
                unresolved.insert(reference.digest.clone());
            }
        }
    }
    Ok(unresolved.into_iter().collect())
}

pub fn schema() -> schemars::Schema {
    let mut schema = schemars::schema_for!(Statement);
    schema.insert("$id".into(), serde_json::json!(PREDICATE_TYPE));
    schema
}

fn statement_type_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"type":"string","const":STATEMENT_TYPE})
}
fn predicate_type_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"type":"string","const":PREDICATE_TYPE})
}
fn snapshot_profile_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"type":"string","const":snapshot::SNAPSHOT_PROFILE})
}
fn contract_versions_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"type":"array","minItems":4,"maxItems":4,"prefixItems":contract_versions().into_iter().map(|version|serde_json::json!({"type":"string","const":version})).collect::<Vec<_>>()})
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD};
    fn content_fixture(case: &str) -> (Predicate, BTreeMap<String, Vec<u8>>) {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/goose")
            .join(case);
        let envelope =
            dsse::parse_envelope(&std::fs::read(root.join("envelope.json")).unwrap()).unwrap();
        let statement: Statement =
            serde_json::from_slice(&STANDARD.decode(envelope.payload).unwrap()).unwrap();
        let blobs = std::fs::read_dir(root.join("blobs"))
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                (
                    format!("sha256:{}", path.file_name().unwrap().to_str().unwrap()),
                    std::fs::read(path).unwrap(),
                )
            })
            .collect();
        (statement.predicate, blobs)
    }
    #[test]
    fn content_validation_requires_every_pre_effect_observation_snapshot() {
        let (mut predicate, mut blobs) = content_fixture("allow");
        validate_content(&predicate, &mut blobs).unwrap();
        // Isolate retention validation; full contract checks separately reject inconsistent ledgers.
        predicate.effects[0]
            .result
            .body
            .observed_pre_effect_subject
            .digest = format!("sha256:{}", "0".repeat(64));
        assert_eq!(
            validate_content(&predicate, &mut blobs).unwrap_err().code,
            "evidence.missing_snapshot"
        );
    }
    #[test]
    fn content_validation_requires_the_observed_schema_bytes() {
        let (mut predicate, mut blobs) = content_fixture("ask");
        validate_content(&predicate, &mut blobs).unwrap();
        predicate.effects[0].result.body.observed_tool_schema_digest =
            Some(format!("sha256:{}", "0".repeat(64)));
        assert_eq!(
            validate_content(&predicate, &mut blobs).unwrap_err().code,
            "evidence.missing_input_or_schema"
        );
    }
    #[test]
    fn request_only_approval_references_are_disclosed_when_bytes_arent_retained() {
        let (mut predicate, mut blobs) = content_fixture("allow");
        let run_digest = format!("sha256:{}", "0".repeat(64));
        let effect_digest = format!("sha256:{}", "1".repeat(64));
        predicate.request.approval_context[0].evidence.digest = run_digest.clone();
        predicate.effects[0].request.approval_context[0]
            .evidence
            .digest = effect_digest.clone();
        let unresolved = validate_content(&predicate, &mut blobs).unwrap();
        assert!(unresolved.contains(&run_digest));
        assert!(unresolved.contains(&effect_digest));
    }
}
