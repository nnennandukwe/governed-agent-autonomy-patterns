//! DSSE v1.0.2 framing authenticates raw payload bytes and their media type.
//! https://github.com/secure-systems-lab/dsse/blob/v1.0.2/protocol.md
use crate::{AttestationError, error};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const MAX_PAYLOAD_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_ENVELOPE_BYTES: usize = 6 * 1024 * 1024;
pub const MAX_SIGNATURES: usize = 16;

/// Envelope extensions are ignored, as DSSE requires. Known duplicate fields fail.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Envelope {
    pub payload_type: String,
    pub payload: String,
    pub signatures: Vec<EnvelopeSignature>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvelopeSignature {
    #[serde(default)]
    pub keyid: String,
    pub sig: String,
}

/// Signer/key access belongs to the host; keys never enter the domain contracts.
pub trait Signer {
    fn key_id(&self) -> &str;
    fn sign(&mut self, message: &[u8]) -> Result<Vec<u8>, AttestationError>;
}
/// An unauthenticated key hint only selects configured trust; it can't create it.
pub trait SignatureVerifier {
    fn verify(
        &mut self,
        key_hint: &str,
        message: &[u8],
        signature: &[u8],
    ) -> Result<VerifiedKey, AttestationError>;
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedKey {
    pub key_id: String,
    pub fingerprint: String,
}

/// Only successful signature verification constructs this type.
/// It isn't a GAAP outcome or permission to advance a controller lifecycle.
pub struct VerifiedEnvelope {
    payload: Vec<u8>,
    keys: Vec<VerifiedKey>,
}
impl VerifiedEnvelope {
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
    pub fn keys(&self) -> &[VerifiedKey] {
        &self.keys
    }
}

/// Published DSSE PAE uses byte lengths and raw bytes, without canonicalization.
pub fn pre_authentication_encoding(
    payload_type: &str,
    payload: &[u8],
) -> Result<Vec<u8>, AttestationError> {
    if payload_type.is_empty()
        || payload_type.len() > 256
        || !payload_type.is_ascii()
        || payload.len() > MAX_PAYLOAD_BYTES
    {
        return Err(error("dsse.invalid_payload"));
    }
    let prefix = format!(
        "DSSEv1 {} {} {} ",
        payload_type.len(),
        payload_type,
        payload.len()
    );
    let mut message = prefix.into_bytes();
    message.extend_from_slice(payload);
    Ok(message)
}

pub fn sign(
    payload_type: &str,
    payload: &[u8],
    signer: &mut impl Signer,
) -> Result<Envelope, AttestationError> {
    let message = pre_authentication_encoding(payload_type, payload)?;
    let keyid = signer.key_id().to_owned();
    validate_key_id(&keyid)?;
    let signature = signer.sign(&message)?;
    if signature.is_empty() || signature.len() > 1024 {
        return Err(error("dsse.invalid_signature"));
    }
    Ok(Envelope {
        payload_type: payload_type.into(),
        payload: STANDARD.encode(payload),
        signatures: vec![EnvelopeSignature {
            keyid,
            sig: STANDARD.encode(signature),
        }],
    })
}

/// This profile requires all listed signatures to verify under distinct trusted keys.
/// Unsigned envelopes and unknown hints can't be promoted to verified evidence.
pub fn verify(
    envelope: &Envelope,
    expected_payload_type: &str,
    verifier: &mut impl SignatureVerifier,
) -> Result<VerifiedEnvelope, AttestationError> {
    if envelope.payload_type != expected_payload_type {
        return Err(error("dsse.payload_type_mismatch"));
    }
    if envelope.signatures.is_empty() {
        return Err(error("dsse.unsigned"));
    }
    if envelope.signatures.len() > MAX_SIGNATURES
        || envelope.payload.len() > MAX_PAYLOAD_BYTES.div_ceil(3) * 4
    {
        return Err(error("dsse.envelope_limit"));
    }
    let payload = STANDARD
        .decode(&envelope.payload)
        .map_err(|_| error("dsse.invalid_base64"))?;
    let message = pre_authentication_encoding(&envelope.payload_type, &payload)?;
    let mut identities = BTreeSet::new();
    let mut keys = Vec::new();
    for signature in &envelope.signatures {
        validate_key_id(&signature.keyid)?;
        if signature.sig.len() > 1368 {
            return Err(error("dsse.signature_limit"));
        }
        let bytes = STANDARD
            .decode(&signature.sig)
            .map_err(|_| error("dsse.invalid_base64"))?;
        if bytes.is_empty() || bytes.len() > 1024 {
            return Err(error("dsse.invalid_signature"));
        }
        let identity = verifier.verify(&signature.keyid, &message, &bytes)?;
        if identity.key_id.is_empty()
            || identity.key_id.len() > 128
            || identity.fingerprint.len() != 71
            || !identity.fingerprint.starts_with("sha256:")
            || !identity.fingerprint[7..]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(error("dsse.invalid_key_identity"));
        }
        if !identities.insert(identity.fingerprint.clone()) {
            return Err(error("dsse.duplicate_signer"));
        }
        keys.push(identity);
    }
    Ok(VerifiedEnvelope { payload, keys })
}

pub fn parse_envelope(bytes: &[u8]) -> Result<Envelope, AttestationError> {
    if bytes.len() > MAX_ENVELOPE_BYTES {
        return Err(error("dsse.envelope_limit"));
    }
    serde_json::from_slice(bytes).map_err(|_| error("dsse.invalid_envelope"))
}
pub(crate) fn validate_key_id(key_id: &str) -> Result<(), AttestationError> {
    if key_id.is_empty()
        || key_id.len() > 128
        || !key_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
    {
        return Err(error("dsse.invalid_key_hint"));
    }
    Ok(())
}
