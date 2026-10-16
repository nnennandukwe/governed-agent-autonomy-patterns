use gaap::contracts::{
    AgentRunRequest, ContractSupport, TerminalRunReceipt, verify_terminal_receipt,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
fn frame(value: &Value) -> Vec<u8> {
    let mut bytes = serde_jcs::to_vec(value).unwrap();
    bytes.push(b'\n');
    bytes
}

#[test]
fn runtime_subject_binds_real_port_calls_and_refuses_post_terminal_input() {
    for probe in [false, true] {
        let request: Value = serde_json::from_str(include_str!(
            "../examples/contracts/v0.1.0/agent-run-request.json"
        ))
        .unwrap();
        let start = json!({"kind":"start","protocol":"run-invariant.runtime/0.2.0","request":request,"support":{"policies":request["policies"],"capabilities":[request["requested_capability"]],"tool_schemas":[],"effect_usage_estimate":{"cost_micros":0,"elapsed_ms":0,"model_tokens":0,"tool_calls":1},"permission":"allow","executor":{"name":"fixture-executor","version":"1","digest":format!("sha256:{}","9".repeat(64))},"verifier_ids":["fixture-verifier"],"max_effects":4}});
        let start_bytes = frame(&start);
        let mut child = Command::new(env!("CARGO_BIN_EXE_gaap"))
            .arg("runtime-subject")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        input.write_all(&start_bytes).unwrap();
        input.flush().unwrap();
        let mut transcript = vec![];
        let mut ports = vec![];
        loop {
            let mut line = vec![];
            assert!(output.read_until(b'\n', &mut line).unwrap() > 0);
            let message: Value = serde_json::from_slice(&line).unwrap();
            assert_eq!(frame(&message), line);
            if message["kind"] == "terminal" {
                assert_eq!(message["start_digest"], digest(&start_bytes));
                assert_eq!(message["transcript_digest"], digest(&transcript));
                let receipt: TerminalRunReceipt =
                    serde_json::from_value(message["receipt"].clone()).unwrap();
                let typed: AgentRunRequest = serde_json::from_value(request.clone()).unwrap();
                verify_terminal_receipt(
                    &typed,
                    &ContractSupport::new(typed.policies.clone()),
                    &receipt,
                )
                .unwrap();
                assert_eq!(message["receipt"]["body"]["terminal_status"], "completed");
                assert_eq!(message["effects"], json!([]));
                if probe {
                    input.write_all(&frame(&start)).unwrap();
                }
                break;
            }
            transcript.extend(line);
            let port = message["port"].as_str().unwrap();
            ports.push(port.to_owned());
            let reply = match port {
                "clock.adapter_elapsed_ms" => json!(0),
                "agent.plan" => {
                    assert_eq!(message["input"], request);
                    json!(format!("sha256:{}", "6".repeat(64)))
                }
                "agent.next" => json!({"step":"finish"}),
                "verifier.verify" => {
                    json!({"subject_digest":request["subject"]["digest"],"verifier_id":"fixture-verifier","verdict":"PASS","evidence":request["required_verification"]["evidence_types"].as_array().unwrap().iter().map(|kind|json!({"evidence_type":kind,"digest":format!("sha256:{}","c".repeat(64)),"locator":null})).collect::<Vec<_>>()})
                }
                "subject.observe" => request["subject"].clone(),
                "signer.sign" => {
                    json!({"keyid":"test-only","signature":"host-verifies-signature-separately"})
                }
                unexpected => panic!("unexpected port {unexpected}"),
            };
            let reply_bytes =
                frame(&json!({"kind":"reply","sequence":message["sequence"],"value":reply}));
            transcript.extend(&reply_bytes);
            input.write_all(&reply_bytes).unwrap();
            input.flush().unwrap();
        }
        drop(input);
        let mut extra = String::new();
        assert_eq!(output.read_line(&mut extra).unwrap(), 0);
        let status = child.wait().unwrap();
        assert_eq!(status.code(), Some(if probe { 1 } else { 0 }));
        assert_eq!(
            ports,
            [
                "agent.plan",
                "clock.adapter_elapsed_ms",
                "agent.next",
                "clock.adapter_elapsed_ms",
                "verifier.verify",
                "clock.adapter_elapsed_ms",
                "subject.observe",
                "clock.adapter_elapsed_ms",
                "signer.sign"
            ]
        );
    }
}
