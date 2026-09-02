use serde::Serialize;
use sha2::{Digest, Sha256};

use super::model::{AgentRunRequest, TerminalRunReceiptBody};
use super::validation::{ContractError, ContractErrorCode};

/// Serialize an Agent Run Request using RFC 8785 JSON Canonicalization Scheme.
pub fn canonical_request_bytes(request: &AgentRunRequest) -> Result<Vec<u8>, ContractError> {
    canonical_bytes(request)
}

pub(crate) fn canonical_receipt_body_bytes(
    body: &TerminalRunReceiptBody,
) -> Result<Vec<u8>, ContractError> {
    canonical_bytes(body)
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

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::canonical_bytes;

    #[test]
    fn matches_the_rfc_8785_unicode_property_sorting_vector() {
        let input: Value = serde_json::from_str(
            r#"{
                "\u20ac": "Euro Sign",
                "\r": "Carriage Return",
                "\ufb33": "Hebrew Letter Dalet With Dagesh",
                "1": "One",
                "\ud83d\ude00": "Emoji: Grinning Face",
                "\u0080": "Control",
                "\u00f6": "Latin Small Letter O With Diaeresis"
            }"#,
        )
        .expect("RFC 8785 vector should parse");

        let canonical = canonical_bytes(&input).expect("RFC 8785 vector should canonicalize");

        assert_eq!(
            String::from_utf8(canonical).expect("canonical JSON should be UTF-8"),
            "{\"\\r\":\"Carriage Return\",\"1\":\"One\",\"\u{80}\":\"Control\",\"ö\":\"Latin Small Letter O With Diaeresis\",\"€\":\"Euro Sign\",\"😀\":\"Emoji: Grinning Face\",\"דּ\":\"Hebrew Letter Dalet With Dagesh\"}"
        );
    }
}
