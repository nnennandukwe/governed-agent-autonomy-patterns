use gaap::contracts::{
    AGENT_RUN_REQUEST_SCHEMA, AgentRunRequest, ApprovalReference, CapabilityIdentity,
    ContractErrorCode, ContractSupport, EffectClass, EffectEvidenceReference, EffectEvidenceType,
    EffectExecutionStatus, EffectExit, EffectUsage, EvidenceReference, EvidenceType,
    ExecutorIdentity, FilesystemAccess, InputMetadataEntry, NetworkProtocol, OperationFamily,
    PROTECTED_EFFECT_REQUEST_SCHEMA, PROTECTED_EFFECT_RESULT_SCHEMA, PolicyIdentity,
    ProtectedEffectDecision, ProtectedEffectRequest, ProtectedEffectResultBody, Repeatability,
    RequestedScope, ResourceBudget, SandboxProfileIdentity, Subject, SubjectKind, TaskSpec,
    VerificationIndependence, VerificationRequirement, canonical_protected_effect_request_bytes,
    parse_protected_effect_request_json, parse_protected_effect_result_json,
    seal_protected_effect_result, validate_protected_effect_request,
    validate_protected_effect_result_body, verify_protected_effect_result,
};
use gaap::{Decision, Gate, Outcome};
use sha2::{Digest, Sha256};

type RequestMutation = Box<dyn Fn(&mut ProtectedEffectRequest)>;

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

fn evidence(evidence_type: EvidenceType, byte: char) -> EvidenceReference {
    EvidenceReference {
        evidence_type,
        digest: digest(byte),
        locator: None,
    }
}

fn approval() -> ApprovalReference {
    ApprovalReference {
        approval_id: "approval-effect-001".to_owned(),
        actor_id: "maintainer-001".to_owned(),
        scope: "protected_effect".to_owned(),
        subject_digest: digest('a'),
        evidence: evidence(EvidenceType::Approval, 'e'),
    }
}

fn agent_run_request(run_id: &str) -> AgentRunRequest {
    AgentRunRequest {
        schema_version: AGENT_RUN_REQUEST_SCHEMA.to_owned(),
        request_id: format!("request-{run_id}"),
        run_id: run_id.to_owned(),
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
        approval_context: vec![],
        required_verification: VerificationRequirement {
            independence: VerificationIndependence::DifferentActor,
            evidence_types: vec![EvidenceType::CommandOutput, EvidenceType::Artifact],
        },
    }
}

fn effect_request(
    agent_request: &AgentRunRequest,
    support: &ContractSupport,
) -> ProtectedEffectRequest {
    let agent_run_request_digest = gaap::contracts::validate_request(agent_request, support)
        .expect("Agent Run Request should validate");
    let resource_budget_digest =
        gaap::contracts::canonical_resource_budget_digest(&agent_request.resource_budget)
            .expect("resource budget should canonicalize");

    ProtectedEffectRequest {
        schema_version: PROTECTED_EFFECT_REQUEST_SCHEMA.to_owned(),
        effect_id: "effect-001".to_owned(),
        effect_sequence: 1,
        run_id: agent_request.run_id.clone(),
        agent_run_request_digest,
        subject: agent_request.subject.clone(),
        operation_family: OperationFamily::Process,
        normalized_operation: "process.spawn".to_owned(),
        capability: agent_request.requested_capability.clone(),
        tool_schema_digest: Some(digest('d')),
        input_digest: digest('1'),
        input_metadata: vec![
            InputMetadataEntry {
                name: "argument_count".to_owned(),
                value: "2".to_owned(),
            },
            InputMetadataEntry {
                name: "content_type".to_owned(),
                value: "application/json".to_owned(),
            },
        ],
        requested_scopes: vec![
            RequestedScope::Process {
                executable: "/usr/bin/cargo".to_owned(),
                working_directory: "/workspace".to_owned(),
            },
            RequestedScope::Filesystem {
                root: "/workspace".to_owned(),
                access: vec![FilesystemAccess::Read, FilesystemAccess::Modify],
                recursive: true,
            },
            RequestedScope::Network {
                protocol: NetworkProtocol::Https,
                host: "crates.io".to_owned(),
                port: 443,
            },
            RequestedScope::ExternalService {
                service: "github".to_owned(),
                operation: "pull_request.comment".to_owned(),
                resource: "nnennandukwe/repository#22".to_owned(),
            },
        ],
        policies: agent_request.policies.clone(),
        approval_context: vec![approval()],
        resource_budget_digest,
        sandbox_profile: SandboxProfileIdentity {
            name: "local-process".to_owned(),
            version: "0.1.0".to_owned(),
            digest: digest('f'),
        },
        idempotency_key: "run-001/effect-001".to_owned(),
        repeatability: Repeatability::Idempotent,
        expected_effect_class: EffectClass::Mutation,
    }
}

#[test]
fn supported_effect_request_returns_a_stable_canonical_digest() {
    let agent_request = agent_run_request("run-001");
    let support = ContractSupport::new([policy()]);
    let request = effect_request(&agent_request, &support);

    let first = validate_protected_effect_request(&agent_request, &support, &request)
        .expect("effect request should validate");
    let second = validate_protected_effect_request(&agent_request, &support, &request)
        .expect("effect request should validate deterministically");

    assert_eq!(first, second);
    assert_eq!(
        first,
        "sha256:f34fe9ac5723efa50583aed72d23bfb4b44ab339e484383bea7d86fbf1c4cf62"
    );
}

#[test]
fn every_request_enum_member_is_accepted() {
    let agent_request = agent_run_request("run-001");
    let support = ContractSupport::new([policy()]);

    let cases = vec![
        (
            OperationFamily::Filesystem,
            "filesystem.write",
            RequestedScope::Filesystem {
                root: "/workspace".to_owned(),
                access: vec![
                    FilesystemAccess::Read,
                    FilesystemAccess::Create,
                    FilesystemAccess::Modify,
                    FilesystemAccess::Delete,
                ],
                recursive: true,
            },
        ),
        (
            OperationFamily::Process,
            "process.spawn",
            RequestedScope::Process {
                executable: "/usr/bin/cargo".to_owned(),
                working_directory: "/workspace".to_owned(),
            },
        ),
        (
            OperationFamily::ExternalService,
            "external_service.invoke",
            RequestedScope::ExternalService {
                service: "github".to_owned(),
                operation: "pull_request.comment".to_owned(),
                resource: "nnennandukwe/repository#22".to_owned(),
            },
        ),
    ];

    for (operation_family, normalized_operation, scope) in cases {
        let mut request = effect_request(&agent_request, &support);
        request.operation_family = operation_family;
        request.normalized_operation = normalized_operation.to_owned();
        request.requested_scopes = vec![scope];
        validate_protected_effect_request(&agent_request, &support, &request)
            .expect("each operation family and scope must validate");
    }

    for protocol in [
        NetworkProtocol::Tcp,
        NetworkProtocol::Udp,
        NetworkProtocol::Http,
        NetworkProtocol::Https,
    ] {
        let mut request = effect_request(&agent_request, &support);
        request.operation_family = OperationFamily::Network;
        request.normalized_operation = "network.connect".to_owned();
        request.requested_scopes = vec![RequestedScope::Network {
            protocol,
            host: "example.invalid".to_owned(),
            port: 443,
        }];
        validate_protected_effect_request(&agent_request, &support, &request)
            .expect("each network protocol must validate");
    }

    for repeatability in [
        Repeatability::Repeatable,
        Repeatability::Idempotent,
        Repeatability::NonRepeatable,
    ] {
        let mut request = effect_request(&agent_request, &support);
        request.repeatability = repeatability;
        validate_protected_effect_request(&agent_request, &support, &request)
            .expect("each repeatability value must validate");
    }

    for effect_class in [EffectClass::Observation, EffectClass::Mutation] {
        let mut request = effect_request(&agent_request, &support);
        request.expected_effect_class = effect_class;
        validate_protected_effect_request(&agent_request, &support, &request)
            .expect("each effect class must validate");
    }
}

#[test]
fn effect_request_rejects_unsupported_versions_and_unsafe_sequences() {
    let agent_request = agent_run_request("run-001");
    let support = ContractSupport::new([policy()]);

    for (request, expected_code, expected_path) in [
        (
            {
                let mut request = effect_request(&agent_request, &support);
                request.schema_version = "gaap.protected-effect-request/0.2.0".to_owned();
                request
            },
            ContractErrorCode::UnsupportedSchema,
            "schema_version",
        ),
        (
            {
                let mut request = effect_request(&agent_request, &support);
                request.effect_sequence = 0;
                request
            },
            ContractErrorCode::InvalidContract,
            "effect_sequence",
        ),
        (
            {
                let mut request = effect_request(&agent_request, &support);
                request.effect_sequence = 9_007_199_254_740_992;
                request
            },
            ContractErrorCode::InvalidContract,
            "effect_sequence",
        ),
    ] {
        let error = validate_protected_effect_request(&agent_request, &support, &request)
            .expect_err("unsupported version or unsafe sequence must fail");
        assert_eq!(error.code(), expected_code);
        assert_eq!(error.path(), expected_path);
    }
}

#[test]
fn effect_request_digest_changes_for_every_authority_bearing_input() {
    let agent_request = agent_run_request("run-001");
    let support = ContractSupport::new([policy()]);
    let request = effect_request(&agent_request, &support);
    let original = validate_protected_effect_request(&agent_request, &support, &request)
        .expect("effect request should validate");

    let mutations: Vec<RequestMutation> = vec![
        Box::new(|value| value.input_digest = digest('2')),
        Box::new(|value| value.tool_schema_digest = Some(digest('3'))),
        Box::new(|value| value.subject.digest = digest('4')),
        Box::new(|value| value.policies[0].digest = digest('5')),
        Box::new(|value| value.approval_context[0].approval_id = "approval-002".to_owned()),
        Box::new(|value| value.resource_budget_digest = digest('6')),
        Box::new(|value| value.sandbox_profile.digest = digest('7')),
    ];

    for mutate in mutations {
        let mut changed = request.clone();
        mutate(&mut changed);
        let changed_digest = canonical_protected_effect_request_bytes(&changed)
            .map(|bytes| format!("sha256:{:x}", Sha256::digest(bytes)))
            .expect("changed request should canonicalize");
        assert_ne!(changed_digest, original);
    }
}

#[test]
fn effect_request_must_match_the_parent_run_capability_policy_and_budget() {
    let agent_request = agent_run_request("run-001");
    let support = ContractSupport::new([policy()]);

    for (request, expected_path) in [
        (
            {
                let mut request = effect_request(&agent_request, &support);
                request.run_id = "run-other".to_owned();
                request
            },
            "run_id",
        ),
        (
            {
                let mut request = effect_request(&agent_request, &support);
                request.capability.digest = digest('8');
                request
            },
            "capability",
        ),
        (
            {
                let mut request = effect_request(&agent_request, &support);
                request.policies[0].digest = digest('8');
                request
            },
            "policies",
        ),
        (
            {
                let mut request = effect_request(&agent_request, &support);
                request.resource_budget_digest = digest('8');
                request
            },
            "resource_budget_digest",
        ),
    ] {
        let error = validate_protected_effect_request(&agent_request, &support, &request)
            .expect_err("parent mismatch must fail closed");
        assert_eq!(error.code(), ContractErrorCode::RequestMismatch);
        assert_eq!(error.path(), expected_path);
    }
}

#[test]
fn malformed_effect_request_digests_fail_closed() {
    let agent_request = agent_run_request("run-001");
    let support = ContractSupport::new([policy()]);

    for (request, expected_path) in [
        (
            {
                let mut request = effect_request(&agent_request, &support);
                request.agent_run_request_digest = "sha256:not-a-digest".to_owned();
                request
            },
            "agent_run_request_digest",
        ),
        (
            {
                let mut request = effect_request(&agent_request, &support);
                request.tool_schema_digest = Some("SHA256:ABC".to_owned());
                request
            },
            "tool_schema_digest",
        ),
        (
            {
                let mut request = effect_request(&agent_request, &support);
                request.input_digest = "sha256:1234".to_owned();
                request
            },
            "input_digest",
        ),
        (
            {
                let mut request = effect_request(&agent_request, &support);
                request.sandbox_profile.digest = "sha256:1234".to_owned();
                request
            },
            "sandbox_profile.digest",
        ),
    ] {
        let error = validate_protected_effect_request(&agent_request, &support, &request)
            .expect_err("malformed request digest must fail");
        assert_eq!(error.code(), ContractErrorCode::InvalidDigest);
        assert_eq!(error.path(), expected_path);
    }
}

#[test]
fn metadata_is_bounded_and_names_are_unique() {
    let agent_request = agent_run_request("run-001");
    let support = ContractSupport::new([policy()]);
    let mut request = effect_request(&agent_request, &support);
    request.input_metadata.push(InputMetadataEntry {
        name: "content_type".to_owned(),
        value: "text/plain".to_owned(),
    });

    let duplicate = validate_protected_effect_request(&agent_request, &support, &request)
        .expect_err("duplicate metadata names must fail");
    assert_eq!(duplicate.code(), ContractErrorCode::InvalidContract);
    assert_eq!(duplicate.path(), "input_metadata[2].name");

    request = effect_request(&agent_request, &support);
    request.input_metadata = (0..33)
        .map(|index| InputMetadataEntry {
            name: format!("key_{index}"),
            value: "value".to_owned(),
        })
        .collect();
    let too_many = validate_protected_effect_request(&agent_request, &support, &request)
        .expect_err("too many metadata entries must fail");
    assert_eq!(too_many.path(), "input_metadata");

    request = effect_request(&agent_request, &support);
    request.input_metadata[0].name = "n".repeat(65);
    let long_name = validate_protected_effect_request(&agent_request, &support, &request)
        .expect_err("long metadata name must fail");
    assert_eq!(long_name.path(), "input_metadata[0].name");

    request = effect_request(&agent_request, &support);
    request.input_metadata[0].value = "v".repeat(257);
    let long_value = validate_protected_effect_request(&agent_request, &support, &request)
        .expect_err("long metadata value must fail");
    assert_eq!(long_value.path(), "input_metadata[0].value");

    request = effect_request(&agent_request, &support);
    request.input_metadata[0].name = "Provider Display Name".to_owned();
    validate_protected_effect_request(&agent_request, &support, &request)
        .expect("metadata names are bounded strings, not normalized identifiers");
}

#[test]
fn typed_scopes_reject_invalid_family_specific_values() {
    let agent_request = agent_run_request("run-001");
    let support = ContractSupport::new([policy()]);
    let mut request = effect_request(&agent_request, &support);
    request.requested_scopes[0] = RequestedScope::Process {
        executable: String::new(),
        working_directory: "/workspace".to_owned(),
    };

    let empty_executable = validate_protected_effect_request(&agent_request, &support, &request)
        .expect_err("empty executable must fail");
    assert_eq!(empty_executable.path(), "requested_scopes[0].executable");

    request = effect_request(&agent_request, &support);
    request.requested_scopes[2] = RequestedScope::Network {
        protocol: NetworkProtocol::Https,
        host: "crates.io".to_owned(),
        port: 0,
    };
    let zero_port = validate_protected_effect_request(&agent_request, &support, &request)
        .expect_err("zero network port must fail");
    assert_eq!(zero_port.path(), "requested_scopes[2].port");

    request = effect_request(&agent_request, &support);
    request.requested_scopes[1] = RequestedScope::Filesystem {
        root: "/workspace".to_owned(),
        access: vec![FilesystemAccess::Read, FilesystemAccess::Read],
        recursive: true,
    };
    let duplicate_access = validate_protected_effect_request(&agent_request, &support, &request)
        .expect_err("duplicate filesystem access classes must fail");
    assert_eq!(duplicate_access.path(), "requested_scopes[1].access");

    request = effect_request(&agent_request, &support);
    request.requested_scopes[0] = RequestedScope::Process {
        executable: r"C:\Program Files\Cargo\cargo.exe".to_owned(),
        working_directory: r"C:\workspace".to_owned(),
    };
    validate_protected_effect_request(&agent_request, &support, &request)
        .expect("scope paths remain platform-neutral contract strings");
}

#[test]
fn normalized_operations_and_approval_references_fail_closed() {
    let agent_request = agent_run_request("run-001");
    let support = ContractSupport::new([policy()]);

    for (request, expected_path) in [
        (
            {
                let mut request = effect_request(&agent_request, &support);
                request.normalized_operation = "filesystem.modify".to_owned();
                request
            },
            "normalized_operation",
        ),
        (
            {
                let mut request = effect_request(&agent_request, &support);
                request.normalized_operation = "process.Spawn".to_owned();
                request
            },
            "normalized_operation",
        ),
        (
            {
                let mut request = effect_request(&agent_request, &support);
                request.approval_context[0].evidence.evidence_type = EvidenceType::Artifact;
                request
            },
            "approval_context[0].evidence.evidence_type",
        ),
        (
            {
                let mut request = effect_request(&agent_request, &support);
                request
                    .approval_context
                    .push(request.approval_context[0].clone());
                request
            },
            "approval_context[1].approval_id",
        ),
    ] {
        let error = validate_protected_effect_request(&agent_request, &support, &request)
            .expect_err("invalid operation or approval reference must fail");
        assert_eq!(error.code(), ContractErrorCode::InvalidContract);
        assert_eq!(error.path(), expected_path);
    }
}

#[test]
fn raw_effect_request_rejects_unknown_fields_families_and_repeatability() {
    let agent_request = agent_run_request("run-001");
    let support = ContractSupport::new([policy()]);
    let request = effect_request(&agent_request, &support);
    let value = serde_json::to_value(request).expect("request should serialize");

    for changed in [
        {
            let mut changed = value.clone();
            changed["provider"] = serde_json::json!("openai");
            changed
        },
        {
            let mut changed = value.clone();
            changed["operation_family"] = serde_json::json!("provider_tool");
            changed
        },
        {
            let mut changed = value.clone();
            changed["repeatability"] = serde_json::json!("maybe_retry");
            changed
        },
        {
            let mut changed = value.clone();
            changed["requested_scopes"][0]
                .as_object_mut()
                .expect("process scope should be an object")
                .remove("executable");
            changed
        },
        {
            let mut changed = value.clone();
            changed["requested_scopes"][0]["host"] = serde_json::json!("example.invalid");
            changed
        },
        {
            let mut changed = value;
            changed["requested_scopes"][0]["scope_type"] = serde_json::json!("container");
            changed
        },
    ] {
        let error = parse_protected_effect_request_json(
            &serde_json::to_vec(&changed).expect("request should serialize"),
        )
        .expect_err("unknown contract value must fail");
        assert_eq!(error.code(), ContractErrorCode::InvalidJson);
    }
}

fn effect_evidence(evidence_type: EffectEvidenceType, byte: char) -> EffectEvidenceReference {
    EffectEvidenceReference {
        evidence_type,
        digest: digest(byte),
        locator: None,
    }
}

fn decision(
    outcome: Outcome,
    effect_request_digest: &str,
    subject_digest: &str,
) -> ProtectedEffectDecision {
    ProtectedEffectDecision {
        decision_id: "decision-001".to_owned(),
        gate: Gate::Permission,
        effect_request_digest: effect_request_digest.to_owned(),
        subject_digest: subject_digest.to_owned(),
        decision: Decision {
            outcome,
            code: "permission.policy_allowed".to_owned(),
            effects: vec![],
        },
    }
}

fn usage() -> EffectUsage {
    EffectUsage {
        cost_micros: 10,
        elapsed_ms: 25,
        model_tokens: 0,
        tool_calls: 1,
    }
}

fn zero_usage() -> EffectUsage {
    EffectUsage {
        cost_micros: 0,
        elapsed_ms: 0,
        model_tokens: 0,
        tool_calls: 0,
    }
}

fn executed_result_body(
    request: &ProtectedEffectRequest,
    effect_request_digest: &str,
) -> ProtectedEffectResultBody {
    let mut post_subject = request.subject.clone();
    post_subject.digest = digest('9');

    ProtectedEffectResultBody {
        schema_version: PROTECTED_EFFECT_RESULT_SCHEMA.to_owned(),
        effect_id: request.effect_id.clone(),
        effect_sequence: request.effect_sequence,
        run_id: request.run_id.clone(),
        agent_run_request_digest: request.agent_run_request_digest.clone(),
        effect_request_digest: effect_request_digest.to_owned(),
        observed_pre_effect_subject: request.subject.clone(),
        observed_capability: request.capability.clone(),
        observed_tool_schema_digest: request.tool_schema_digest.clone(),
        decision: decision(
            Outcome::Allow,
            effect_request_digest,
            &request.subject.digest,
        ),
        execution_status: EffectExecutionStatus::Executed,
        observed_post_effect_subject: Some(post_subject),
        exit: Some(EffectExit::Code { code: 0 }),
        usage: usage(),
        executor: Some(ExecutorIdentity {
            name: "local-runner".to_owned(),
            version: "0.1.0".to_owned(),
            digest: digest('6'),
        }),
        sandbox_profile: Some(request.sandbox_profile.clone()),
        reason: None,
        evidence: vec![
            effect_evidence(EffectEvidenceType::Exit, '0'),
            effect_evidence(EffectEvidenceType::Output, '1'),
            effect_evidence(EffectEvidenceType::Artifact, '2'),
            effect_evidence(EffectEvidenceType::Mutation, '3'),
            effect_evidence(EffectEvidenceType::Usage, '4'),
            effect_evidence(EffectEvidenceType::Executor, '5'),
            effect_evidence(EffectEvidenceType::Sandbox, '6'),
            effect_evidence(EffectEvidenceType::SubjectObservation, '7'),
        ],
    }
}

fn non_execution_result_body(
    request: &ProtectedEffectRequest,
    effect_request_digest: &str,
    status: EffectExecutionStatus,
    outcome: Outcome,
) -> ProtectedEffectResultBody {
    let mut body = executed_result_body(request, effect_request_digest);
    body.decision = decision(outcome, effect_request_digest, &request.subject.digest);
    body.execution_status = status;
    body.observed_post_effect_subject = None;
    body.exit = None;
    body.usage = zero_usage();
    body.executor = None;
    body.sandbox_profile = None;
    body.reason = Some("authority was not granted".to_owned());
    body.evidence.clear();
    body
}

#[test]
fn executed_result_is_sealed_and_verified_with_a_stable_digest() {
    let agent_request = agent_run_request("run-001");
    let support = ContractSupport::new([policy()]);
    let request = effect_request(&agent_request, &support);
    let request_digest = validate_protected_effect_request(&agent_request, &support, &request)
        .expect("effect request should validate");
    let body = executed_result_body(&request, &request_digest);

    let result = seal_protected_effect_result(&agent_request, &support, &request, body)
        .expect("executed result should seal");
    verify_protected_effect_result(&agent_request, &support, &request, &result)
        .expect("sealed result should verify");

    assert_eq!(
        result.result_digest,
        "sha256:f3952c57da5ce2ec5327dc20b625b1f8363f271ac0cedb618e736bccc871102a"
    );
}

#[test]
fn result_rejects_unsupported_versions_unsafe_sequences_and_malformed_digests() {
    let agent_request = agent_run_request("run-001");
    let support = ContractSupport::new([policy()]);
    let request = effect_request(&agent_request, &support);
    let request_digest = validate_protected_effect_request(&agent_request, &support, &request)
        .expect("effect request should validate");

    for (body, expected_code, expected_path) in [
        (
            {
                let mut body = executed_result_body(&request, &request_digest);
                body.schema_version = "gaap.protected-effect-result/0.2.0".to_owned();
                body
            },
            ContractErrorCode::UnsupportedSchema,
            "body.schema_version",
        ),
        (
            {
                let mut body = executed_result_body(&request, &request_digest);
                body.effect_sequence = 9_007_199_254_740_992;
                body
            },
            ContractErrorCode::InvalidContract,
            "body.effect_sequence",
        ),
        (
            {
                let mut body = executed_result_body(&request, &request_digest);
                body.observed_tool_schema_digest = Some("sha256:1234".to_owned());
                body
            },
            ContractErrorCode::InvalidDigest,
            "body.observed_tool_schema_digest",
        ),
    ] {
        let error =
            validate_protected_effect_result_body(&agent_request, &support, &request, &body)
                .expect_err("invalid result version, bound, or digest must fail");
        assert_eq!(error.code(), expected_code);
        assert_eq!(error.path(), expected_path);
    }
}

#[test]
fn result_identity_sequence_and_request_digest_mismatches_fail_closed() {
    let agent_request = agent_run_request("run-001");
    let support = ContractSupport::new([policy()]);
    let request = effect_request(&agent_request, &support);
    let request_digest = validate_protected_effect_request(&agent_request, &support, &request)
        .expect("effect request should validate");

    for (body, expected_path) in [
        (
            {
                let mut body = executed_result_body(&request, &request_digest);
                body.effect_id = "effect-other".to_owned();
                body
            },
            "body.effect_id",
        ),
        (
            {
                let mut body = executed_result_body(&request, &request_digest);
                body.effect_sequence = 2;
                body
            },
            "body.effect_sequence",
        ),
        (
            {
                let mut body = executed_result_body(&request, &request_digest);
                body.agent_run_request_digest = digest('8');
                body
            },
            "body.agent_run_request_digest",
        ),
        (
            {
                let mut body = executed_result_body(&request, &request_digest);
                body.effect_request_digest = digest('8');
                body
            },
            "body.effect_request_digest",
        ),
        (
            {
                let mut body = executed_result_body(&request, &request_digest);
                body.decision.effect_request_digest = digest('8');
                body
            },
            "body.decision.effect_request_digest",
        ),
    ] {
        let error =
            validate_protected_effect_result_body(&agent_request, &support, &request, &body)
                .expect_err("result identity mismatch must fail closed");
        assert_eq!(error.code(), ContractErrorCode::RequestMismatch);
        assert_eq!(error.path(), expected_path);
    }
}

#[test]
fn decision_outcomes_and_execution_statuses_follow_the_authority_matrix() {
    let agent_request = agent_run_request("run-001");
    let support = ContractSupport::new([policy()]);
    let request = effect_request(&agent_request, &support);
    let request_digest = validate_protected_effect_request(&agent_request, &support, &request)
        .expect("effect request should validate");

    for outcome in [Outcome::Ask, Outcome::Block] {
        let mut body = executed_result_body(&request, &request_digest);
        body.decision = decision(outcome, &request_digest, &request.subject.digest);
        let error =
            validate_protected_effect_result_body(&agent_request, &support, &request, &body)
                .expect_err("ask or block must never coexist with execution");
        assert_eq!(error.code(), ContractErrorCode::UnauthorizedEffect);
        assert_eq!(error.path(), "body.decision.decision.outcome");
    }

    for (status, outcome) in [
        (EffectExecutionStatus::AwaitingAuthority, Outcome::Ask),
        (EffectExecutionStatus::Denied, Outcome::Block),
    ] {
        let body = non_execution_result_body(&request, &request_digest, status, outcome);
        validate_protected_effect_result_body(&agent_request, &support, &request, &body)
            .expect("non-execution authority result should validate");
    }
}

#[test]
fn attempted_results_require_observed_execution_identities_and_evidence() {
    let agent_request = agent_run_request("run-001");
    let support = ContractSupport::new([policy()]);
    let request = effect_request(&agent_request, &support);
    let request_digest = validate_protected_effect_request(&agent_request, &support, &request)
        .expect("effect request should validate");

    for (body, expected_path, expected_code) in [
        (
            {
                let mut body = executed_result_body(&request, &request_digest);
                body.executor = None;
                body
            },
            "body.executor",
            ContractErrorCode::InvalidContract,
        ),
        (
            {
                let mut body = executed_result_body(&request, &request_digest);
                body.sandbox_profile = None;
                body
            },
            "body.sandbox_profile",
            ContractErrorCode::InvalidContract,
        ),
        (
            {
                let mut body = executed_result_body(&request, &request_digest);
                body.evidence
                    .retain(|item| item.evidence_type != EffectEvidenceType::Usage);
                body
            },
            "body.evidence",
            ContractErrorCode::InvalidContract,
        ),
        (
            {
                let mut body = executed_result_body(&request, &request_digest);
                body.evidence
                    .retain(|item| item.evidence_type != EffectEvidenceType::SubjectObservation);
                body
            },
            "body.evidence",
            ContractErrorCode::InvalidContract,
        ),
        (
            {
                let mut body = executed_result_body(&request, &request_digest);
                body.observed_post_effect_subject = None;
                body
            },
            "body.observed_post_effect_subject",
            ContractErrorCode::InvalidContract,
        ),
        (
            {
                let mut body = executed_result_body(&request, &request_digest);
                body.sandbox_profile
                    .as_mut()
                    .expect("sandbox exists")
                    .digest = digest('8');
                body
            },
            "body.sandbox_profile",
            ContractErrorCode::RequestMismatch,
        ),
    ] {
        let error =
            validate_protected_effect_result_body(&agent_request, &support, &request, &body)
                .expect_err("attempted result missing a required identity or evidence must fail");
        assert_eq!(error.code(), expected_code);
        assert_eq!(error.path(), expected_path);
    }
}

#[test]
fn result_reason_presence_matches_execution_status() {
    let agent_request = agent_run_request("run-001");
    let support = ContractSupport::new([policy()]);
    let request = effect_request(&agent_request, &support);
    let request_digest = validate_protected_effect_request(&agent_request, &support, &request)
        .expect("effect request should validate");

    let mut executed = executed_result_body(&request, &request_digest);
    executed.reason = Some("unexpected reason".to_owned());
    let error =
        validate_protected_effect_result_body(&agent_request, &support, &request, &executed)
            .expect_err("executed result must not contain a reason");
    assert_eq!(error.path(), "body.reason");

    for (status, outcome) in [
        (EffectExecutionStatus::AwaitingAuthority, Outcome::Ask),
        (EffectExecutionStatus::Denied, Outcome::Block),
    ] {
        let mut body = non_execution_result_body(&request, &request_digest, status, outcome);
        body.reason = None;
        let error =
            validate_protected_effect_result_body(&agent_request, &support, &request, &body)
                .expect_err("non-execution result requires a reason");
        assert_eq!(error.path(), "body.reason");
    }

    for status in [
        EffectExecutionStatus::Failed,
        EffectExecutionStatus::Interrupted,
        EffectExecutionStatus::UnknownOutcome,
    ] {
        let mut body = executed_result_body(&request, &request_digest);
        body.execution_status = status;
        let error =
            validate_protected_effect_result_body(&agent_request, &support, &request, &body)
                .expect_err("attempted non-success result requires a reason");
        assert_eq!(error.path(), "body.reason");
    }
}

#[test]
fn awaiting_and_denied_results_reject_execution_derived_fields() {
    let agent_request = agent_run_request("run-001");
    let support = ContractSupport::new([policy()]);
    let request = effect_request(&agent_request, &support);
    let request_digest = validate_protected_effect_request(&agent_request, &support, &request)
        .expect("effect request should validate");

    for (body, expected_path) in [
        (
            {
                let mut body = non_execution_result_body(
                    &request,
                    &request_digest,
                    EffectExecutionStatus::AwaitingAuthority,
                    Outcome::Ask,
                );
                body.usage.tool_calls = 1;
                body
            },
            "body.usage",
        ),
        (
            {
                let mut body = non_execution_result_body(
                    &request,
                    &request_digest,
                    EffectExecutionStatus::Denied,
                    Outcome::Block,
                );
                body.executor = Some(ExecutorIdentity {
                    name: "must-not-run".to_owned(),
                    version: "0.1.0".to_owned(),
                    digest: digest('8'),
                });
                body
            },
            "body.executor",
        ),
        (
            {
                let mut body = non_execution_result_body(
                    &request,
                    &request_digest,
                    EffectExecutionStatus::Denied,
                    Outcome::Block,
                );
                body.evidence = vec![effect_evidence(EffectEvidenceType::Output, '8')];
                body
            },
            "body.evidence[0].evidence_type",
        ),
    ] {
        let error =
            validate_protected_effect_result_body(&agent_request, &support, &request, &body)
                .expect_err("non-execution result must not contain execution-derived state");
        assert_eq!(error.code(), ContractErrorCode::UnauthorizedEffect);
        assert_eq!(error.path(), expected_path);
    }
}

#[test]
fn attempted_failures_interruptions_and_unknown_outcomes_remain_distinct() {
    let agent_request = agent_run_request("run-001");
    let support = ContractSupport::new([policy()]);
    let request = effect_request(&agent_request, &support);
    let request_digest = validate_protected_effect_request(&agent_request, &support, &request)
        .expect("effect request should validate");

    for (status, required_evidence, conflicting_evidence) in [
        (
            EffectExecutionStatus::Failed,
            EffectEvidenceType::Failure,
            EffectEvidenceType::Interruption,
        ),
        (
            EffectExecutionStatus::Interrupted,
            EffectEvidenceType::Interruption,
            EffectEvidenceType::Failure,
        ),
        (
            EffectExecutionStatus::UnknownOutcome,
            EffectEvidenceType::UnknownOutcome,
            EffectEvidenceType::Failure,
        ),
    ] {
        let mut body = executed_result_body(&request, &request_digest);
        body.execution_status = status;
        body.reason = Some(format!("effect ended as {status:?}"));
        body.evidence.push(effect_evidence(required_evidence, '8'));
        if status == EffectExecutionStatus::Interrupted {
            body.exit = Some(EffectExit::Signal {
                signal: "SIGTERM".to_owned(),
            });
        }
        if status == EffectExecutionStatus::UnknownOutcome {
            body.observed_post_effect_subject = None;
            body.exit = None;
            body.evidence.retain(|reference| {
                matches!(
                    reference.evidence_type,
                    EffectEvidenceType::Usage
                        | EffectEvidenceType::Executor
                        | EffectEvidenceType::Sandbox
                        | EffectEvidenceType::UnknownOutcome
                )
            });
        }

        validate_protected_effect_result_body(&agent_request, &support, &request, &body)
            .expect("attempt status with its distinct evidence should validate");

        let mut contradictory = body.clone();
        contradictory
            .evidence
            .push(effect_evidence(conflicting_evidence, '7'));
        let error = validate_protected_effect_result_body(
            &agent_request,
            &support,
            &request,
            &contradictory,
        )
        .expect_err("attempt status with contradictory evidence must fail");
        assert!(error.path().ends_with("evidence_type"));

        body.evidence
            .retain(|reference| reference.evidence_type != required_evidence);
        let error =
            validate_protected_effect_result_body(&agent_request, &support, &request, &body)
                .expect_err("attempt status without its distinguishing evidence must fail");
        assert_eq!(error.path(), "body.evidence");
    }
}

#[test]
fn non_repeatable_request_can_record_an_unknown_outcome_without_becoming_failure() {
    let agent_request = agent_run_request("run-001");
    let support = ContractSupport::new([policy()]);
    let mut request = effect_request(&agent_request, &support);
    request.repeatability = Repeatability::NonRepeatable;
    let request_digest = validate_protected_effect_request(&agent_request, &support, &request)
        .expect("non-repeatable effect request should validate");
    let mut body = executed_result_body(&request, &request_digest);
    body.execution_status = EffectExecutionStatus::UnknownOutcome;
    body.observed_post_effect_subject = None;
    body.exit = None;
    body.reason = Some("executor disconnected before reconciliation".to_owned());
    body.evidence.retain(|reference| {
        matches!(
            reference.evidence_type,
            EffectEvidenceType::Usage | EffectEvidenceType::Executor | EffectEvidenceType::Sandbox
        )
    });
    body.evidence
        .push(effect_evidence(EffectEvidenceType::UnknownOutcome, '8'));

    validate_protected_effect_result_body(&agent_request, &support, &request, &body)
        .expect("unknown outcome remains representable for a non-repeatable request");
    assert_eq!(body.execution_status, EffectExecutionStatus::UnknownOutcome);
}

#[test]
fn stale_subject_and_tool_schema_drift_are_valid_denials() {
    let agent_request = agent_run_request("run-001");
    let support = ContractSupport::new([policy()]);
    let request = effect_request(&agent_request, &support);
    let request_digest = validate_protected_effect_request(&agent_request, &support, &request)
        .expect("effect request should validate");

    let mut stale = non_execution_result_body(
        &request,
        &request_digest,
        EffectExecutionStatus::Denied,
        Outcome::Block,
    );
    stale.observed_pre_effect_subject.digest = digest('8');
    stale.decision.subject_digest = digest('8');
    stale.reason = Some("protected_effect.stale_subject".to_owned());
    stale.evidence = vec![effect_evidence(EffectEvidenceType::SubjectObservation, '8')];
    validate_protected_effect_result_body(&agent_request, &support, &request, &stale)
        .expect("stale subject should be preserved as a denied result");

    let mut drift = non_execution_result_body(
        &request,
        &request_digest,
        EffectExecutionStatus::Denied,
        Outcome::Block,
    );
    drift.observed_tool_schema_digest = Some(digest('8'));
    drift.reason = Some("protected_effect.capability_schema_drift".to_owned());
    drift.evidence = vec![effect_evidence(EffectEvidenceType::CapabilitySchema, '8')];
    validate_protected_effect_result_body(&agent_request, &support, &request, &drift)
        .expect("tool-schema drift should be preserved as a denied result");
}

#[test]
fn drift_mismatches_cannot_be_disguised_as_executed_or_generic_denials() {
    let agent_request = agent_run_request("run-001");
    let support = ContractSupport::new([policy()]);
    let request = effect_request(&agent_request, &support);
    let request_digest = validate_protected_effect_request(&agent_request, &support, &request)
        .expect("effect request should validate");

    let mut executed = executed_result_body(&request, &request_digest);
    executed.observed_pre_effect_subject.digest = digest('8');
    executed.decision.subject_digest = digest('8');
    let error =
        validate_protected_effect_result_body(&agent_request, &support, &request, &executed)
            .expect_err("executed result with a stale subject must fail closed");
    assert_eq!(error.code(), ContractErrorCode::RequestMismatch);
    assert_eq!(error.path(), "body.observed_pre_effect_subject");

    let mut denied = non_execution_result_body(
        &request,
        &request_digest,
        EffectExecutionStatus::Denied,
        Outcome::Block,
    );
    denied.observed_tool_schema_digest = Some(digest('8'));
    let error = validate_protected_effect_result_body(&agent_request, &support, &request, &denied)
        .expect_err("schema drift must carry a precise denial reason");
    assert_eq!(error.path(), "body.reason");

    let mut invented_drift = non_execution_result_body(
        &request,
        &request_digest,
        EffectExecutionStatus::Denied,
        Outcome::Block,
    );
    invented_drift.reason = Some("protected_effect.stale_subject".to_owned());
    invented_drift.evidence = vec![effect_evidence(EffectEvidenceType::SubjectObservation, '8')];
    let error =
        validate_protected_effect_result_body(&agent_request, &support, &request, &invented_drift)
            .expect_err("reserved drift reason requires an observed identity difference");
    assert_eq!(error.code(), ContractErrorCode::InvalidContract);
    assert_eq!(error.path(), "body.reason");

    let mut capability_mismatch = non_execution_result_body(
        &request,
        &request_digest,
        EffectExecutionStatus::Denied,
        Outcome::Block,
    );
    capability_mismatch.observed_capability.digest = digest('8');
    capability_mismatch.reason = Some("protected_effect.capability_schema_drift".to_owned());
    capability_mismatch.evidence = vec![effect_evidence(EffectEvidenceType::CapabilitySchema, '8')];
    let error = validate_protected_effect_result_body(
        &agent_request,
        &support,
        &request,
        &capability_mismatch,
    )
    .expect_err("a different capability identity is not tool-schema drift");
    assert_eq!(error.code(), ContractErrorCode::RequestMismatch);
    assert_eq!(error.path(), "body.observed_capability");
}

#[test]
fn observation_and_mutation_results_enforce_subject_and_evidence_rules() {
    let agent_request = agent_run_request("run-001");
    let support = ContractSupport::new([policy()]);

    let mut observation_request = effect_request(&agent_request, &support);
    observation_request.expected_effect_class = EffectClass::Observation;
    let observation_digest =
        validate_protected_effect_request(&agent_request, &support, &observation_request)
            .expect("observation request should validate");
    let mut observation = executed_result_body(&observation_request, &observation_digest);
    observation.observed_post_effect_subject = Some(observation_request.subject.clone());
    observation.evidence.retain(|reference| {
        !matches!(
            reference.evidence_type,
            EffectEvidenceType::Mutation | EffectEvidenceType::Artifact
        )
    });
    validate_protected_effect_result_body(
        &agent_request,
        &support,
        &observation_request,
        &observation,
    )
    .expect("observation preserving its subject should validate");

    observation
        .observed_post_effect_subject
        .as_mut()
        .expect("post subject exists")
        .locator = "https://mirror.example.invalid/repository".to_owned();
    validate_protected_effect_result_body(
        &agent_request,
        &support,
        &observation_request,
        &observation,
    )
    .expect("observation preservation is defined by the subject digest");

    observation
        .observed_post_effect_subject
        .as_mut()
        .expect("post subject exists")
        .digest = digest('8');
    let error = validate_protected_effect_result_body(
        &agent_request,
        &support,
        &observation_request,
        &observation,
    )
    .expect_err("observation must preserve its subject");
    assert_eq!(error.path(), "body.observed_post_effect_subject");

    let mutation_request = effect_request(&agent_request, &support);
    let mutation_digest =
        validate_protected_effect_request(&agent_request, &support, &mutation_request)
            .expect("mutation request should validate");
    let mut mutation = executed_result_body(&mutation_request, &mutation_digest);
    mutation.evidence.retain(|reference| {
        !matches!(
            reference.evidence_type,
            EffectEvidenceType::Mutation | EffectEvidenceType::Artifact
        )
    });
    let error = validate_protected_effect_result_body(
        &agent_request,
        &support,
        &mutation_request,
        &mutation,
    )
    .expect_err("mutation must carry mutation and artifact evidence");
    assert_eq!(error.path(), "body.evidence");
}

#[test]
fn known_process_results_require_typed_exit_status_and_exit_evidence() {
    let agent_request = agent_run_request("run-001");
    let support = ContractSupport::new([policy()]);
    let request = effect_request(&agent_request, &support);
    let request_digest = validate_protected_effect_request(&agent_request, &support, &request)
        .expect("effect request should validate");

    let mut body = executed_result_body(&request, &request_digest);
    body.exit = None;
    let error = validate_protected_effect_result_body(&agent_request, &support, &request, &body)
        .expect_err("known process execution requires exit status");
    assert_eq!(error.path(), "body.exit");

    body = executed_result_body(&request, &request_digest);
    body.evidence
        .retain(|reference| reference.evidence_type != EffectEvidenceType::Exit);
    let error = validate_protected_effect_result_body(&agent_request, &support, &request, &body)
        .expect_err("known process execution requires exit evidence");
    assert_eq!(error.path(), "body.evidence");
}

#[test]
fn result_tampering_and_cross_run_replay_fail_closed() {
    let agent_request = agent_run_request("run-001");
    let support = ContractSupport::new([policy()]);
    let request = effect_request(&agent_request, &support);
    let request_digest = validate_protected_effect_request(&agent_request, &support, &request)
        .expect("effect request should validate");
    let result = seal_protected_effect_result(
        &agent_request,
        &support,
        &request,
        executed_result_body(&request, &request_digest),
    )
    .expect("executed result should seal");

    let mut tampered = result.clone();
    tampered.body.reason = Some("rewritten after sealing".to_owned());
    let error = verify_protected_effect_result(&agent_request, &support, &request, &tampered)
        .expect_err("body tampering must fail");
    assert_eq!(error.code(), ContractErrorCode::ResultTampering);
    assert_eq!(error.path(), "result_digest");

    let mut malformed = result.clone();
    malformed.result_digest = "sha256:not-a-digest".to_owned();
    let error = verify_protected_effect_result(&agent_request, &support, &request, &malformed)
        .expect_err("malformed result digest must fail as result tampering");
    assert_eq!(error.code(), ContractErrorCode::ResultTampering);
    assert_eq!(error.path(), "result_digest");

    let other_agent_request = agent_run_request("run-002");
    let other_request = effect_request(&other_agent_request, &support);
    let error =
        verify_protected_effect_result(&other_agent_request, &support, &other_request, &result)
            .expect_err("cross-run replay must fail");
    assert_eq!(error.code(), ContractErrorCode::RequestMismatch);

    let mut other_request = request.clone();
    other_request.effect_id = "effect-002".to_owned();
    other_request.input_digest = digest('8');
    other_request.idempotency_key = "run-001/effect-002".to_owned();
    let error = verify_protected_effect_result(&agent_request, &support, &other_request, &result)
        .expect_err("same-run cross-request replay must fail");
    assert_eq!(error.code(), ContractErrorCode::RequestMismatch);
}

#[test]
fn raw_effect_result_rejects_unknown_statuses_evidence_and_fields() {
    let agent_request = agent_run_request("run-001");
    let support = ContractSupport::new([policy()]);
    let request = effect_request(&agent_request, &support);
    let request_digest = validate_protected_effect_request(&agent_request, &support, &request)
        .expect("effect request should validate");
    let result = seal_protected_effect_result(
        &agent_request,
        &support,
        &request,
        executed_result_body(&request, &request_digest),
    )
    .expect("executed result should seal");
    let value = serde_json::to_value(result).expect("result should serialize");

    for changed in [
        {
            let mut changed = value.clone();
            changed["body"]["execution_status"] = serde_json::json!("retried");
            changed
        },
        {
            let mut changed = value.clone();
            changed["body"]["evidence"][0]["evidence_type"] = serde_json::json!("provider_trace");
            changed
        },
        {
            let mut changed = value.clone();
            changed["body"]["exit"] = serde_json::json!({
                "exit_type": "provider_status",
                "code": 0
            });
            changed
        },
        {
            let mut changed = value;
            changed["body"]["raw_output"] = serde_json::json!("secret");
            changed
        },
    ] {
        let error = parse_protected_effect_result_json(
            &serde_json::to_vec(&changed).expect("result should serialize"),
        )
        .expect_err("unknown result values and fields must fail");
        assert_eq!(error.code(), ContractErrorCode::InvalidJson);
    }
}
