//! Pure Ed25519 adapters; storage, rotation and signer approval belong to the host.
use crate::{
    AttestationError,
    dsse::{SignatureVerifier, Signer, VerifiedKey, validate_key_id},
    error,
};
use ed25519_dalek::{Signature, Signer as _, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Secret material isn't serializable or printable through this adapter.
pub struct Ed25519Signer {
    key_id: String,
    key: SigningKey,
}
impl Ed25519Signer {
    pub fn from_seed(key_id: String, seed: [u8; 32]) -> Result<Self, AttestationError> {
        validate_key_id(&key_id)?;
        Ok(Self {
            key_id,
            key: SigningKey::from_bytes(&seed),
        })
    }
    pub fn public_key(&self) -> [u8; 32] {
        self.key.verifying_key().to_bytes()
    }
}
impl Signer for Ed25519Signer {
    fn key_id(&self) -> &str {
        &self.key_id
    }
    fn sign(&mut self, message: &[u8]) -> Result<Vec<u8>, AttestationError> {
        Ok(self.key.sign(message).to_bytes().to_vec())
    }
}

pub struct Ed25519Verifier {
    keys: BTreeMap<String, VerifyingKey>,
}
impl Ed25519Verifier {
    /// Trust is an explicit local allowlist, never a key supplied by an envelope.
    pub fn new(
        keys: impl IntoIterator<Item = (String, [u8; 32])>,
    ) -> Result<Self, AttestationError> {
        let mut trusted = BTreeMap::new();
        for (key_id, bytes) in keys {
            validate_key_id(&key_id)?;
            let key = VerifyingKey::from_bytes(&bytes).map_err(|_| error("ed25519.invalid_key"))?;
            if key.is_weak() || trusted.insert(key_id, key).is_some() || trusted.len() > 32 {
                return Err(error("ed25519.invalid_trust_store"));
            }
        }
        if trusted.is_empty() {
            return Err(error("ed25519.empty_trust_store"));
        }
        Ok(Self { keys: trusted })
    }
}
impl SignatureVerifier for Ed25519Verifier {
    fn verify(
        &mut self,
        key_hint: &str,
        message: &[u8],
        signature: &[u8],
    ) -> Result<VerifiedKey, AttestationError> {
        let key = self
            .keys
            .get(key_hint)
            .ok_or_else(|| error("ed25519.untrusted_key"))?;
        let signature =
            Signature::try_from(signature).map_err(|_| error("ed25519.invalid_signature"))?;
        key.verify_strict(message, &signature)
            .map_err(|_| error("ed25519.signature_mismatch"))?;
        let mut fingerprint = Sha256::new();
        fingerprint.update(b"gaap.attestation.ed25519/1\0");
        fingerprint.update(key.to_bytes());
        Ok(VerifiedKey {
            key_id: key_hint.into(),
            fingerprint: format!("sha256:{:x}", fingerprint.finalize()),
        })
    }
}
