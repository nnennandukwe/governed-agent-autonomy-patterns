//! Attestation adapters stay outside the frozen GAAP domain contracts.
#![forbid(unsafe_code)]
pub mod dsse;
pub mod ed25519;
pub mod json;
pub mod snapshot;

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttestationError {
    pub code: &'static str,
}
impl fmt::Display for AttestationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}: don't treat this evidence as verified",
            self.code
        )
    }
}
impl std::error::Error for AttestationError {}
pub(crate) fn error(code: &'static str) -> AttestationError {
    AttestationError { code }
}
