use serde::Serialize;
use sha2::{Digest, Sha256};

use super::model::AgentRunRequest;
use super::validation::{ContractError, ContractErrorCode};

pub fn canonical_request_bytes(request: &AgentRunRequest) -> Result<Vec<u8>, ContractError> {
    canonical_bytes(request)
}

pub(crate) fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, ContractError> {
    serde_jcs::to_vec(value).map_err(|error| {
        ContractError::new(
            ContractErrorCode::InvalidContract,
            "$",
            format!("could not canonicalize contract: {error}"),
        )
    })
}

pub(crate) fn sha256_digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
