use gaap::contracts::{
    AGENT_RUN_REQUEST_SCHEMA, AgentRunRequest, ApprovalReference, CapabilityIdentity,
    ContractErrorCode, ContractSupport, EvidenceReference, EvidenceType, PolicyIdentity,
    ResourceBudget, Subject, SubjectKind, TaskSpec, VerificationIndependence,
    VerificationRequirement, canonical_request_bytes, parse_agent_run_request_json,
    validate_request,
};

fn digest(byte: char) -> String {
    format!("sha256:{}", byte.to_string().repeat(64))
}

fn policy() -> PolicyIdentity {
    PolicyIdentity {
        name: "gaap.run-coordinator".to_owned(),
        version: "0.1.0".to_owned(),
        digest: digest('b'),
    }
}

fn request() -> AgentRunRequest {
    AgentRunRequest {
        schema_version: AGENT_RUN_REQUEST_SCHEMA.to_owned(),
        request_id: "request-001".to_owned(),
        run_id: "run-001".to_owned(),
        subject: Subject {
            kind: SubjectKind::Repository,
            locator: "https://example.invalid/repository".to_owned(),
            digest: digest('a'),
        },
        requested_capability: CapabilityIdentity {
            name: "change-code".to_owned(),
            version: "1".to_owned(),
            digest: digest('c'),
        },
        task: TaskSpec {
            instructions: "Implement the approved change.".to_owned(),
            constraints: vec!["Do not publish a release.".to_owned()],
        },
        policies: vec![policy()],
        resource_budget: ResourceBudget {
            max_cost_micros: 1_000_000,
            max_elapsed_ms: 60_000,
            max_model_tokens: 100_000,
            max_tool_calls: 100,
        },
        approval_context: vec![ApprovalReference {
            approval_id: "approval-001".to_owned(),
            actor_id: "maintainer-001".to_owned(),
            scope: "plan".to_owned(),
            subject_digest: digest('d'),
            evidence: EvidenceReference {
                evidence_type: EvidenceType::Approval,
                digest: digest('e'),
                locator: None,
            },
        }],
        required_verification: VerificationRequirement {
            independence: VerificationIndependence::DifferentActor,
            evidence_types: vec![EvidenceType::CommandOutput, EvidenceType::Artifact],
        },
    }
}

#[test]
fn supported_request_returns_a_stable_canonical_digest() {
    let request = request();
    let support = ContractSupport::new([policy()]);

    let request_digest = validate_request(&request, &support).expect("request should be valid");
    let canonical = canonical_request_bytes(&request).expect("request should canonicalize");
    let reparsed: AgentRunRequest =
        serde_json::from_slice(&canonical).expect("canonical request should parse");

    assert_eq!(reparsed, request);
    assert_eq!(
        request_digest,
        "sha256:3d029feed0e1c4d89e58a4920401f2179f6af0fa24f59afeef33393808ed6632"
    );
}

#[test]
fn request_with_an_unknown_policy_fails_closed() {
    let request = request();
    let support = ContractSupport::default();

    let error = validate_request(&request, &support).expect_err("unknown policy must fail");

    assert_eq!(error.code(), ContractErrorCode::UnknownPolicy);
    assert_eq!(error.path(), "policies[0]");
}

#[test]
fn request_rejects_an_unknown_schema_version() {
    let mut request = request();
    request.schema_version = "gaap.agent-run-request/9.9.9".to_owned();
    let support = ContractSupport::new([policy()]);

    let error = validate_request(&request, &support).expect_err("unknown schema must fail");

    assert_eq!(error.code(), ContractErrorCode::UnsupportedSchema);
    assert_eq!(error.path(), "schema_version");
}

#[test]
fn request_rejects_an_unsafe_integer() {
    let mut request = request();
    request.resource_budget.max_model_tokens = 9_007_199_254_740_992;
    let support = ContractSupport::new([policy()]);

    let error = validate_request(&request, &support).expect_err("unsafe integer must fail");

    assert_eq!(error.code(), ContractErrorCode::InvalidContract);
    assert_eq!(error.path(), "resource_budget.max_model_tokens");
}

#[test]
fn raw_request_rejects_unknown_fields() {
    let mut value = serde_json::to_value(request()).expect("request should serialize");
    value
        .as_object_mut()
        .expect("request should be an object")
        .insert("provider".to_owned(), serde_json::json!("openai"));

    let error = parse_agent_run_request_json(
        &serde_json::to_vec(&value).expect("request should serialize"),
    )
    .expect_err("unknown fields must fail");

    assert_eq!(error.code(), ContractErrorCode::InvalidJson);
}

#[test]
fn raw_request_rejects_duplicate_fields() {
    let json = serde_json::to_string(&request()).expect("request should serialize");
    let duplicate = json.replacen(
        "\"request_id\":",
        "\"request_id\":\"shadow-request\",\"request_id\":",
        1,
    );

    let error =
        parse_agent_run_request_json(duplicate.as_bytes()).expect_err("duplicate fields must fail");

    assert_eq!(error.code(), ContractErrorCode::InvalidJson);
}
