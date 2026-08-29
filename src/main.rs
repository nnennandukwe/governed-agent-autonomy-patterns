use std::io::{self, Read, Write};
use std::process::ExitCode;

use gaap::{Decision, Gate, RunCoordinator};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

const REQUEST_SCHEMA: &str = "run-invariant.subject-request/0.1.0";
const RESPONSE_SCHEMA: &str = "run-invariant.subject-response/0.1.0";
const PROTOCOL_VERSION: &str = "0.1.0";
const PROTOCOL_SHA256: &str =
    "sha256:2ee636c7f634bb403603a1140f12c47634a7c19313fcc39272d13422f355b256";
const FIXTURE_SHA256: &str =
    "sha256:b03bd707aa7e6e94c78dae5e5e2339a3e556c39272eaeaf9075e2dff112ff7cf";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SubjectRequest {
    schema_version: String,
    protocol: ProtocolIdentity,
    fixtures: FixtureIdentity,
    cases: Vec<CaseRequest>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProtocolIdentity {
    version: String,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureIdentity {
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CaseRequest {
    id: String,
    gate: String,
    input: Value,
}

#[derive(Debug, Serialize)]
struct SubjectResponse {
    schema_version: &'static str,
    request_sha256: String,
    subject: SubjectIdentity,
    results: Vec<CaseResult>,
}

#[derive(Debug, Serialize)]
struct SubjectIdentity {
    name: &'static str,
    version: &'static str,
    implementation: &'static str,
}

#[derive(Debug, Serialize)]
struct CaseResult {
    id: String,
    decision: Decision,
}

fn help() -> &'static str {
    "GAAP agent-run integrity coordinator\n\nUsage:\n  gaap run-invariant\n  gaap --help\n\nrun-invariant reads one subject request from stdin and writes one response to stdout.\n"
}

fn request_digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn run_invariant() -> Result<(), String> {
    let mut request_bytes = Vec::new();
    io::stdin()
        .read_to_end(&mut request_bytes)
        .map_err(|error| format!("could not read subject request: {error}"))?;
    let request: SubjectRequest = serde_json::from_slice(&request_bytes)
        .map_err(|error| format!("invalid subject request JSON: {error}"))?;

    if request.schema_version != REQUEST_SCHEMA {
        return Err(format!(
            "unsupported subject request schema: {}",
            request.schema_version
        ));
    }
    if request.protocol.version != PROTOCOL_VERSION || request.protocol.sha256 != PROTOCOL_SHA256 {
        return Err(format!(
            "unsupported decision protocol: expected {PROTOCOL_VERSION} at {PROTOCOL_SHA256}"
        ));
    }
    if request.fixtures.sha256 != FIXTURE_SHA256 {
        return Err(format!(
            "unsupported fixture corpus: expected {FIXTURE_SHA256}"
        ));
    }
    if request.cases.is_empty() {
        return Err("subject request must contain at least one case".to_owned());
    }

    let coordinator = RunCoordinator;
    let mut results = Vec::with_capacity(request.cases.len());
    for case in request.cases {
        let gate: Gate = case
            .gate
            .parse()
            .map_err(|error| format!("case {}: {error}", case.id))?;
        let decision = coordinator.evaluate(gate, &case.input);
        results.push(CaseResult {
            id: case.id,
            decision,
        });
    }

    let response = SubjectResponse {
        schema_version: RESPONSE_SCHEMA,
        request_sha256: request_digest(&request_bytes),
        subject: SubjectIdentity {
            name: "gaap",
            version: env!("CARGO_PKG_VERSION"),
            implementation: "rust",
        },
        results,
    };
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, &response)
        .map_err(|error| format!("could not write subject response: {error}"))?;
    writeln!(stdout).map_err(|error| format!("could not finish subject response: {error}"))?;
    Ok(())
}

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    let command = arguments.next().unwrap_or_else(|| "--help".to_owned());
    let has_extra_arguments = arguments.next().is_some();
    match command.as_str() {
        "--help" | "help" if !has_extra_arguments => {
            print!("{}", help());
            ExitCode::SUCCESS
        }
        "run-invariant" if !has_extra_arguments => match run_invariant() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("gaap error: {error}");
                ExitCode::FAILURE
            }
        },
        _ if has_extra_arguments => {
            eprintln!(
                "gaap error: {command} does not accept arguments\n\n{}",
                help()
            );
            ExitCode::from(2)
        }
        _ => {
            eprintln!("gaap error: unknown command: {command}\n\n{}", help());
            ExitCode::from(2)
        }
    }
}
