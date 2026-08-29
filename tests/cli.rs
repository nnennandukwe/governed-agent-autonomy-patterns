use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const PROTOCOL_SHA256: &str =
    "sha256:2ee636c7f634bb403603a1140f12c47634a7c19313fcc39272d13422f355b256";
const FIXTURE_SHA256: &str =
    "sha256:b03bd707aa7e6e94c78dae5e5e2339a3e556c39272eaeaf9075e2dff112ff7cf";

fn gaap() -> Command {
    Command::new(env!("CARGO_BIN_EXE_gaap"))
}

#[test]
fn help_names_the_available_subject_command() {
    let output = gaap().arg("--help").output().expect("gaap should start");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("help should be UTF-8");
    assert!(stdout.contains("gaap run-invariant"));
}

#[test]
fn subject_command_rejects_unrecognized_arguments() {
    let output = gaap()
        .args(["run-invariant", "unexpected"])
        .output()
        .expect("gaap should start");

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("does not accept arguments"));
}

#[test]
fn subject_adapter_returns_a_request_bound_decision() {
    let digest = format!("sha256:{}", "a".repeat(64));
    let request = json!({
        "schema_version": "run-invariant.subject-request/0.1.0",
        "protocol": {
            "version": "0.1.0",
            "sha256": PROTOCOL_SHA256,
        },
        "fixtures": { "sha256": FIXTURE_SHA256 },
        "cases": [{
            "id": "PLAN-INTEGRATION",
            "gate": "plan",
            "input": {
                "subject_digest": digest,
                "approval": {
                    "status": "approved",
                    "subject_digest": digest,
                }
            }
        }]
    });
    let request_bytes = serde_json::to_vec(&request).expect("request should serialize");
    let mut child = gaap()
        .arg("run-invariant")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("gaap should start");
    child
        .stdin
        .take()
        .expect("stdin should be piped")
        .write_all(&request_bytes)
        .expect("request should be written");
    let output = child.wait_with_output().expect("gaap should finish");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value =
        serde_json::from_slice(&output.stdout).expect("response should be one JSON document");
    assert_eq!(
        response["request_sha256"],
        format!("sha256:{:x}", Sha256::digest(&request_bytes))
    );
    assert_eq!(response["results"][0]["decision"]["outcome"], "allow");
    assert_eq!(
        response["results"][0]["decision"]["code"],
        "plan.approved_exact"
    );
}

#[test]
fn subject_adapter_rejects_an_unknown_protocol_digest() {
    let request = json!({
        "schema_version": "run-invariant.subject-request/0.1.0",
        "protocol": {
            "version": "0.1.0",
            "sha256": format!("sha256:{}", "0".repeat(64)),
        },
        "fixtures": { "sha256": FIXTURE_SHA256 },
        "cases": [{ "id": "PLAN-001", "gate": "plan", "input": {} }]
    });
    let mut child = gaap()
        .arg("run-invariant")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("gaap should start");
    child
        .stdin
        .take()
        .expect("stdin should be piped")
        .write_all(&serde_json::to_vec(&request).expect("request should serialize"))
        .expect("request should be written");
    let output = child.wait_with_output().expect("gaap should finish");

    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unsupported decision protocol"));
}
