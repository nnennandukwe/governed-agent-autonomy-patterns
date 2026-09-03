use std::fs;
use std::path::PathBuf;

use gaap::contracts::{
    AgentRunStatus, ContractSupport, EffectExecutionStatus, agent_run_request_schema,
    parse_agent_run_request_json, parse_protected_effect_request_json,
    parse_protected_effect_result_json, parse_terminal_run_receipt_json,
    protected_effect_request_schema, protected_effect_result_schema, terminal_run_receipt_schema,
    validate_protected_effect_request, validate_request, verify_protected_effect_result,
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
        (
            "schemas/protected-effect/v0.1.0/protected-effect-request.schema.json",
            protected_effect_request_schema(),
        ),
        (
            "schemas/protected-effect/v0.1.0/protected-effect-result.schema.json",
            protected_effect_result_schema(),
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

#[test]
fn committed_protected_effect_examples_match_schemas_and_semantic_contracts() {
    let root = repository_root();
    let agent_request_bytes =
        fs::read(root.join("examples/contracts/v0.1.0/agent-run-request.json"))
            .expect("Agent Run Request example should be readable");
    let agent_request = parse_agent_run_request_json(&agent_request_bytes)
        .expect("Agent Run Request example should parse");
    let support = ContractSupport::new(agent_request.policies.clone());

    let directory = root.join("examples/contracts/protected-effect/v0.1.0");
    let request_bytes = fs::read(directory.join("protected-effect-request.json"))
        .expect("Protected Effect Request example should be readable");
    let request_value: serde_json::Value = serde_json::from_slice(&request_bytes)
        .expect("Protected Effect Request example should be JSON");
    let request_validator = jsonschema::validator_for(&protected_effect_request_schema())
        .expect("Protected Effect Request schema should compile");
    request_validator
        .validate(&request_value)
        .unwrap_or_else(|error| panic!("Protected Effect Request example is invalid: {error}"));
    let request = parse_protected_effect_request_json(&request_bytes)
        .expect("Protected Effect Request example should parse");
    validate_protected_effect_request(&agent_request, &support, &request)
        .expect("Protected Effect Request example should validate");

    let result_validator = jsonschema::validator_for(&protected_effect_result_schema())
        .expect("Protected Effect Result schema should compile");
    let scenarios = [
        ("completed.json", EffectExecutionStatus::Executed),
        ("denied.json", EffectExecutionStatus::Denied),
        (
            "awaiting-authority.json",
            EffectExecutionStatus::AwaitingAuthority,
        ),
        ("failed.json", EffectExecutionStatus::Failed),
        ("interrupted.json", EffectExecutionStatus::Interrupted),
        ("stale-subject.json", EffectExecutionStatus::Denied),
        ("schema-drift.json", EffectExecutionStatus::Denied),
        (
            "unknown-outcome.json",
            EffectExecutionStatus::UnknownOutcome,
        ),
    ];
    for (file_name, expected_status) in scenarios {
        let path = directory.join(file_name);
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("could not read {}: {error}", path.display()));
        let value: serde_json::Value = serde_json::from_slice(&bytes)
            .unwrap_or_else(|error| panic!("{file_name} should be JSON: {error}"));
        result_validator
            .validate(&value)
            .unwrap_or_else(|error| panic!("{file_name} does not match schema: {error}"));
        let result = parse_protected_effect_result_json(&bytes)
            .unwrap_or_else(|error| panic!("{file_name} should parse: {error}"));
        verify_protected_effect_result(&agent_request, &support, &request, &result)
            .unwrap_or_else(|error| panic!("{file_name} should verify: {error}"));
        assert_eq!(result.body.execution_status, expected_status, "{file_name}");
    }
}
