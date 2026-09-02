use std::fs;
use std::path::PathBuf;

use gaap::contracts::{
    AgentRunStatus, ContractSupport, agent_run_request_schema, parse_agent_run_request_json,
    parse_terminal_run_receipt_json, terminal_run_receipt_schema, validate_request,
    verify_terminal_receipt,
};

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn generated_contract_schemas_match_the_committed_documents() {
    for (path, generated) in [
        (
            "schemas/agent-run/v0.1.0/agent-run-request.schema.json",
            agent_run_request_schema(),
        ),
        (
            "schemas/agent-run/v0.1.0/terminal-run-receipt.schema.json",
            terminal_run_receipt_schema(),
        ),
    ] {
        let expected = format!(
            "{}\n",
            serde_json::to_string_pretty(&generated).expect("schema should serialize")
        );
        let actual = fs::read_to_string(repository_root().join(path))
            .unwrap_or_else(|error| panic!("could not read {path}: {error}"));

        assert_eq!(actual, expected, "generated schema drifted: {path}");
    }
}

#[test]
fn committed_examples_match_schemas_and_semantic_contracts() {
    let root = repository_root();
    let request_path = root.join("examples/contracts/v0.1.0/agent-run-request.json");
    let request_bytes = fs::read(&request_path).expect("request example should be readable");
    let request_value: serde_json::Value =
        serde_json::from_slice(&request_bytes).expect("request example should be JSON");
    let request_validator = jsonschema::validator_for(&agent_run_request_schema())
        .expect("request schema should compile");
    request_validator
        .validate(&request_value)
        .unwrap_or_else(|error| panic!("request example does not match schema: {error}"));
    let request =
        parse_agent_run_request_json(&request_bytes).expect("request example should parse");
    let support = ContractSupport::new(request.policies.clone());
    validate_request(&request, &support).expect("request example should validate");

    let receipt_validator = jsonschema::validator_for(&terminal_run_receipt_schema())
        .expect("receipt schema should compile");
    let scenarios = [
        (
            "completed.json",
            AgentRunStatus::Completed,
            "workflow.completion_authorized",
        ),
        (
            "blocked.json",
            AgentRunStatus::Blocked,
            "authority.required",
        ),
        ("failed.json", AgentRunStatus::Failed, "execution.failed"),
        (
            "interrupted.json",
            AgentRunStatus::Interrupted,
            "operator.interrupted",
        ),
        (
            "budget-exhausted.json",
            AgentRunStatus::Blocked,
            "runtime.hard_stop",
        ),
        (
            "denied-effect.json",
            AgentRunStatus::Blocked,
            "permission.denied",
        ),
        (
            "stale-verification.json",
            AgentRunStatus::Blocked,
            "verification.stale_subject",
        ),
    ];
    for (file_name, expected_status, expected_reason) in scenarios {
        let path = root.join("examples/contracts/v0.1.0").join(file_name);
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("could not read {}: {error}", path.display()));
        let value: serde_json::Value = serde_json::from_slice(&bytes)
            .unwrap_or_else(|error| panic!("{file_name} should be JSON: {error}"));
        receipt_validator
            .validate(&value)
            .unwrap_or_else(|error| panic!("{file_name} does not match schema: {error}"));
        let receipt = parse_terminal_run_receipt_json(&bytes)
            .unwrap_or_else(|error| panic!("{file_name} should parse: {error}"));
        verify_terminal_receipt(&request, &support, &receipt)
            .unwrap_or_else(|error| panic!("{file_name} should verify: {error}"));

        assert_eq!(receipt.body.terminal_status, expected_status, "{file_name}");
        assert_eq!(receipt.body.terminal_reason, expected_reason, "{file_name}");
    }
}
