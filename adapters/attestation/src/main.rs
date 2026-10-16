//! Offline composition root. Fixture signing uses a deliberately public test seed.
use base64::{Engine, engine::general_purpose::STANDARD};
use gaap::contracts::{
    AgentRunRequest, ContractSupport, PolicyIdentity, ProtectedEffectRequest,
    ProtectedEffectResult, Subject, TerminalRunReceipt,
};
use gaap_attestation::{
    AttestationError, dsse,
    ed25519::{Ed25519Signer, Ed25519Verifier},
    json, snapshot,
    statement::{self, BlobReader, EffectRecord, Implementation, Predicate},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[derive(Deserialize)]
struct RunBundle {
    request: AgentRunRequest,
    receipt: TerminalRunReceipt,
    protected_effect_requests: Vec<ProtectedEffectRequest>,
    protected_effect_results: Vec<ProtectedEffectResult>,
    after: Subject,
    evidence_blobs: BTreeMap<String, Vec<u8>>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TrustedKey {
    key_id: String,
    public_key_base64: String,
}
struct DirectoryBlobs(PathBuf);
impl BlobReader for DirectoryBlobs {
    fn read(
        &mut self,
        digest: &str,
        max_bytes: usize,
    ) -> std::result::Result<Vec<u8>, AttestationError> {
        // Fixed digest-only names never interpret untrusted locators or path segments.
        let name = digest
            .strip_prefix("sha256:")
            .filter(|name| {
                name.len() == 64
                    && name
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            })
            .ok_or(AttestationError {
                code: "evidence.invalid_digest",
            })?;
        bounded_read(&self.0.join(name), max_bytes).map_err(|_| AttestationError {
            code: "evidence.unreadable_blob",
        })
    }
}
fn bounded_read(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err("input exceeds its byte limit".into());
    }
    Ok(bytes)
}
fn fixture_sign(
    bundle_path: &Path,
    destination: &Path,
    implementation_digest: String,
) -> Result<()> {
    let value = json::parse(&bounded_read(bundle_path, dsse::MAX_PAYLOAD_BYTES)?)?;
    let mut bundle: RunBundle = serde_json::from_value(value)?;
    if bundle.protected_effect_requests.len() != bundle.protected_effect_results.len() {
        return Err("effect request/result count differs".into());
    }
    let support = ContractSupport::new(bundle.request.policies.clone());
    let policies = bundle.request.policies.clone();
    let mut snapshots = vec![
        bundle.request.subject.digest.clone(),
        bundle.receipt.body.resulting_subject_digest.clone(),
        bundle.after.digest.clone(),
    ];
    let effects: Vec<_> = bundle
        .protected_effect_requests
        .into_iter()
        .zip(bundle.protected_effect_results)
        .map(|(request, result)| {
            snapshots.push(request.subject.digest.clone());
            snapshots.push(result.body.observed_pre_effect_subject.digest.clone());
            if let Some(subject) = &result.body.observed_post_effect_subject {
                snapshots.push(subject.digest.clone());
            }
            EffectRecord { request, result }
        })
        .collect();
    snapshots.sort();
    snapshots.dedup();
    let statement = statement::new_statement(Predicate {
        implementation: Implementation {
            name: "gaap-governed-goose".into(),
            version: "0.1.0".into(),
            digest: implementation_digest,
        },
        contract_versions: statement::contract_versions(),
        request: bundle.request,
        receipt: bundle.receipt,
        effects,
        observed_subject: bundle.after,
        snapshot_profile: snapshot::SNAPSHOT_PROFILE.into(),
        snapshot_digests: snapshots,
        blob_digests: bundle.evidence_blobs.keys().cloned().collect(),
    });
    // Public fixture key only. Production trust must come from separately configured keys.
    let mut signer = Ed25519Signer::from_seed("gaap-goose-public-fixture-only".into(), [7; 32])?;
    let envelope = statement::sign(
        &statement,
        &support,
        &mut bundle.evidence_blobs,
        &mut signer,
    )?;
    fs::create_dir(destination)?;
    fs::create_dir(destination.join("blobs"))?;
    for (digest, bytes) in &bundle.evidence_blobs {
        fs::write(destination.join("blobs").join(&digest[7..]), bytes)?;
    }
    fs::write(
        destination.join("envelope.json"),
        serde_json::to_vec_pretty(&envelope)?,
    )?;
    fs::write(
        destination.join("fixture-trust.json"),
        serde_json::to_vec_pretty(&vec![TrustedKey {
            key_id: "gaap-goose-public-fixture-only".into(),
            public_key_base64: STANDARD.encode(signer.public_key()),
        }])?,
    )?;
    fs::write(
        destination.join("fixture-policies.json"),
        serde_json::to_vec_pretty(&policies)?,
    )?;
    fs::write(
        destination.join("FIXTURE_ONLY.txt"),
        "Public fixture key. Don't configure this key as production trust.\nA verified signature doesn't authorize a controller lifecycle transition.\n",
    )?;
    Ok(())
}
fn verify(
    envelope_path: &Path,
    trust_path: &Path,
    policies_path: &Path,
    blobs_path: &Path,
) -> Result<()> {
    let envelope = dsse::parse_envelope(&bounded_read(envelope_path, dsse::MAX_ENVELOPE_BYTES)?)?;
    let trust: Vec<TrustedKey> =
        serde_json::from_value(json::parse(&bounded_read(trust_path, 16384)?)?)?;
    let keys: Vec<_> = trust
        .into_iter()
        .map(|key| {
            let bytes: [u8; 32] = STANDARD
                .decode(key.public_key_base64)?
                .try_into()
                .map_err(|_| "public key must contain 32 bytes")?;
            Ok((key.key_id, bytes))
        })
        .collect::<Result<_>>()?;
    let policies: Vec<PolicyIdentity> =
        serde_json::from_value(json::parse(&bounded_read(policies_path, 65536)?)?)?;
    let verified = statement::verify(
        &envelope,
        &ContractSupport::new(policies),
        &mut DirectoryBlobs(blobs_path.into()),
        &mut Ed25519Verifier::new(keys)?,
    )?;
    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "signature_verified":true, "receipt_digest":verified.statement().predicate.receipt.receipt_digest,
            "run_id":verified.statement().predicate.request.run_id,
            "terminal_status":verified.statement().predicate.receipt.body.terminal_status,
            "effect_outcomes":verified.statement().predicate.effects.iter().map(|effect|effect.result.body.execution_status).collect::<Vec<_>>(),
            "observed_subject_digest":verified.statement().predicate.observed_subject.digest,
            "ledger_subject_digest":verified.statement().predicate.receipt.body.resulting_subject_digest,
            "unresolved_reference_digests":verified.unresolved_references(),
            "controller_authority":"none"
        }))?
    );
    Ok(())
}
fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [command] if command == "--help" || command == "help" => {
            println!("Offline GAAP receipt verification\n\nverify ENVELOPE TRUST_KEYS POLICIES BLOBS_DIRECTORY\n  Verify signatures against explicit host trust, contracts and retained content.\nfixture-sign BUNDLE NEW_DIRECTORY HOST_BINARY_DIGEST\n  Export a test envelope using a public fixture key. Don't use it for production trust.\nschema\n  Print the versioned in-toto statement schema.\n\nA verified signature doesn't authorize a controller transition."); Ok(())
        },
        [command,bundle,destination,digest] if command == "fixture-sign" => fixture_sign(Path::new(bundle),Path::new(destination),digest.clone()),
        [command,envelope,trust,policies,blobs] if command == "verify" => verify(Path::new(envelope),Path::new(trust),Path::new(policies),Path::new(blobs)),
        [command] if command == "schema" => { println!("{}",serde_json::to_string_pretty(&statement::schema())?); Ok(()) },
        _ => Err("usage: gaap-attestation fixture-sign BUNDLE NEW_DIRECTORY HOST_BINARY_DIGEST | verify ENVELOPE TRUST_KEYS POLICIES BLOBS_DIRECTORY | schema".into()),
    }
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!(
                "{error}. Inspect the envelope, explicit trust/policy configuration and retained blobs; use --help for usage."
            );
            std::process::ExitCode::FAILURE
        }
    }
}
