use gaap::contracts::{
    AGENT_RUN_REQUEST_SCHEMA, AgentRunRequest, ApprovalReference, CapabilityIdentity,
    ContractErrorCode, ContractSupport, EffectClass, EvidenceReference, EvidenceType,
    FilesystemAccess, InputMetadataEntry, NetworkProtocol, OperationFamily,
    PROTECTED_EFFECT_REQUEST_SCHEMA, PolicyIdentity, ProtectedEffectRequest, Repeatability,
    RequestedScope, ResourceBudget, SandboxProfileIdentity, Subject, SubjectKind, TaskSpec,
    VerificationIndependence, VerificationRequirement, canonical_protected_effect_request_bytes,
    parse_protected_effect_request_json, validate_protected_effect_request,
};
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
