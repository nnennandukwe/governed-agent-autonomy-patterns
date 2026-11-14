use std::collections::{BTreeMap, VecDeque};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use gaap::contracts::*;
use gaap::runtime::*;
use gaap::static_policy::StaticPolicyAdapter;
use gaap_cedar::{ApprovedAction, CedarAdapter, PreparedPack, content_digest, exact_permit};
use serde_json::{Value, json};

fn fixture_digest(hex_digit: char) -> String {
    format!("sha256:{}", hex_digit.to_string().repeat(64))
}
fn cap() -> CapabilityIdentity {
    CapabilityIdentity {
        name: "local.effect".into(),
        version: "1".into(),
        digest: fixture_digest('a'),
    }
}
fn scope() -> RequestedScope {
    RequestedScope::Filesystem {
        root: "/workspace/report.txt".into(),
        access: vec![FilesystemAccess::Modify],
        recursive: false,
    }
}
fn action() -> ApprovedAction {
    ApprovedAction::native(
        OperationFamily::Filesystem,
        "filesystem.write".into(),
        cap(),
    )
}
fn input() -> Value {
    json!({"path":"/workspace/report.txt","content":"approved"})
}
fn prepare(
    action: &ApprovedAction,
    scope: &RequestedScope,
    input: &Value,
    suffix: &str,
) -> PreparedPack {
    let source =
        exact_permit("local-run", action, scope, &content_digest(input).unwrap()).unwrap() + suffix;
    PreparedPack::prepare(
        "local-test",
        "1",
        "local-run",
        &source,
        vec![action.clone()],
    )
    .unwrap()
}
fn activate(pack: PreparedPack, inputs: Vec<Value>) -> CedarAdapter {
    let ids = pack.identities().clone();
    pack.activate(
        &ids,
        inputs
            .into_iter()
            .map(|v| (content_digest(&v).unwrap(), v))
            .collect(),
    )
    .unwrap()
}
fn run_request(capability: CapabilityIdentity, pins: &[PolicyIdentity]) -> AgentRunRequest {
    let subject = Subject {
        kind: SubjectKind::Repository,
        locator: "/workspace".into(),
        digest: fixture_digest('b'),
    };
    AgentRunRequest {
        schema_version: AGENT_RUN_REQUEST_SCHEMA.into(),
        request_id: "local-request".into(),
        run_id: "local-run".into(),
        subject: subject.clone(),
        requested_capability: capability,
        task: TaskSpec {
            instructions: "Write the approved local fixture".into(),
            constraints: vec![],
        },
        policies: pins.to_vec(),
        resource_budget: ResourceBudget {
            max_cost_micros: 100,
            max_elapsed_ms: 1000,
            max_model_tokens: 10,
            max_tool_calls: 2,
        },
        approval_context: vec![ApprovalReference {
            approval_id: "plan".into(),
            actor_id: "owner".into(),
            scope: "subject".into(),
            subject_digest: subject.digest,
            evidence: EvidenceReference {
                evidence_type: EvidenceType::Approval,
                digest: fixture_digest('c'),
                locator: None,
            },
        }],
        required_verification: VerificationRequirement {
            independence: VerificationIndependence::DifferentActor,
            evidence_types: vec![EvidenceType::Artifact],
        },
    }
}
fn effect(
    action: &ApprovedAction,
    scope: RequestedScope,
    input: &Value,
    pins: &[PolicyIdentity],
) -> (AgentRunRequest, ProtectedEffectRequest) {
    let run = run_request(action.capability.clone(), pins);
    let support = ContractSupport::new(pins.iter().cloned());
    let request = ProtectedEffectRequest {
        schema_version: PROTECTED_EFFECT_REQUEST_SCHEMA.into(),
        effect_id: "effect-1".into(),
        effect_sequence: 1,
        run_id: run.run_id.clone(),
        agent_run_request_digest: validate_request(&run, &support).unwrap(),
        subject: run.subject.clone(),
        operation_family: action.operation_family,
        normalized_operation: action.normalized_operation.clone(),
        capability: action.capability.clone(),
        tool_schema_digest: action.tool_schema_digest.clone(),
        input_digest: content_digest(input).unwrap(),
        input_metadata: vec![],
        requested_scopes: vec![scope],
        policies: pins.to_vec(),
        approval_context: vec![],
        resource_budget_digest: canonical_resource_budget_digest(&run.resource_budget).unwrap(),
        sandbox_profile: SandboxProfileIdentity {
            name: "local-fixture".into(),
            version: "1".into(),
            digest: fixture_digest('d'),
        },
        idempotency_key: "local-run/1".into(),
        repeatability: Repeatability::Idempotent,
        expected_effect_class: EffectClass::Mutation,
    };
    (run, request)
}

#[test]
fn exact_filesystem_process_network_and_service_boundaries() {
    let cases = [
        (action(), scope(), input()),
        (
            ApprovedAction::native(OperationFamily::Process, "process.exec".into(), cap()),
            RequestedScope::Process {
                executable: "/usr/bin/git".into(),
                working_directory: "/workspace".into(),
            },
            json!({"argv":["status","--short"]}),
        ),
        (
            ApprovedAction::native(OperationFamily::Network, "network.connect".into(), cap()),
            RequestedScope::Network {
                protocol: NetworkProtocol::Https,
                host: "example.invalid".into(),
                port: 443,
            },
            json!({"method":"GET","path":"/health"}),
        ),
        (
            ApprovedAction::native(
                OperationFamily::ExternalService,
                "external_service.read".into(),
                cap(),
            ),
            RequestedScope::ExternalService {
                service: "local-service".into(),
                operation: "read".into(),
                resource: "record-1".into(),
            },
            json!({"record":"1"}),
        ),
    ];
    for (action, scope, input) in cases {
        let pack = prepare(&action, &scope, &input, "");
        let (_, request) = effect(&action, scope.clone(), &input, pack.identities());
        let adapter = activate(
            pack,
            vec![input.clone(), json!({"argv":["--short","status"]})],
        );
        assert_eq!(adapter.evaluate(&request).decision, PolicyDecision::Allow);
        let mut changed = request.clone();
        changed.requested_scopes[0] = match scope {
            RequestedScope::Filesystem { .. } => RequestedScope::Filesystem {
                root: "/workspace/report.txt/child".into(),
                access: vec![FilesystemAccess::Modify],
                recursive: false,
            },
            RequestedScope::Process { .. } => RequestedScope::Process {
                executable: "/usr/bin/sh".into(),
                working_directory: "/workspace".into(),
            },
            RequestedScope::Network { .. } => RequestedScope::Network {
                protocol: NetworkProtocol::Https,
                host: "example.invalid.evil".into(),
                port: 443,
            },
            RequestedScope::ExternalService { .. } => RequestedScope::ExternalService {
                service: "local-service".into(),
                operation: "delete".into(),
                resource: "record-1".into(),
            },
        };
        assert_eq!(adapter.evaluate(&changed).decision, PolicyDecision::Deny);
        changed = request.clone();
        changed.input_digest = content_digest(&json!({"argv":["--short","status"]})).unwrap();
        assert_eq!(
            adapter.evaluate(&changed).decision,
            PolicyDecision::Deny,
            "ordered inputs can't retain resource authority"
        );
        changed = request.clone();
        changed.requested_scopes.push(RequestedScope::Network {
            protocol: NetworkProtocol::Https,
            host: "other.invalid".into(),
            port: 443,
        });
        assert_eq!(
            adapter.evaluate(&changed).decision,
            PolicyDecision::Deny,
            "every scope needs authority"
        );
    }
}

#[test]
fn explicit_forbid_overrides_an_exact_permit() {
    let pack = prepare(
        &action(),
        &scope(),
        &input(),
        "\nforbid(principal, action, resource);",
    );
    let (_, request) = effect(&action(), scope(), &input(), pack.identities());
    assert_eq!(
        activate(pack, vec![input()]).evaluate(&request).decision,
        PolicyDecision::Deny
    );
}

#[test]
fn activation_rejects_missing_invalid_incompatible_and_wildcard_policies() {
    for source in [
        "",
        "not a policy",
        "permit(principal, action, resource);",
        "permit(principal, action, resource) when { context.missing == true };",
    ] {
        let err =
            PreparedPack::prepare("pack", "1", "local-run", source, vec![action()]).unwrap_err();
        assert!(err.to_string().contains("validate and pin"));
    }
    let source = exact_permit(
        "local-run",
        &action(),
        &scope(),
        &content_digest(&input()).unwrap(),
    )
    .unwrap();
    for source in [
        source.replace("principal == Gaap::AgentRun::\"local-run\"", "principal"),
        source.replace("resource == ", "resource in "),
    ] {
        assert!(PreparedPack::prepare("pack", "1", "local-run", &source, vec![action()]).is_err());
    }
    let pack = prepare(&action(), &scope(), &input(), "");
    let mut pins = pack.identities().clone();
    pins[1].digest = fixture_digest('0');
    assert!(
        pack.activate(&pins, BTreeMap::new())
            .unwrap_err()
            .to_string()
            .contains("pin mismatch")
    );
}

#[test]
fn missing_context_unknown_actions_and_changed_pins_fail_closed() {
    let pack = prepare(&action(), &scope(), &input(), "");
    let (_, request) = effect(&action(), scope(), &input(), pack.identities());
    let adapter = activate(pack, vec![]);
    let evidence = adapter.evaluate(&request);
    assert_eq!(evidence.decision, PolicyDecision::Deny);
    assert!(evidence.errors[0].contains("missing bound input"));
    let adapter = activate(prepare(&action(), &scope(), &input(), ""), vec![input()]);
    for change in 0..6 {
        let mut changed = request.clone();
        match change {
            0 => changed.normalized_operation = "filesystem.delete".into(),
            1 => changed.capability.digest = fixture_digest('e'),
            2 => changed.policies[0].digest = fixture_digest('0'),
            3 => changed.subject.locator.clear(),
            4 => changed.run_id = "unknown-run".into(),
            _ => changed.tool_schema_digest = Some(fixture_digest('f')),
        };
        assert_eq!(adapter.evaluate(&changed).decision, PolicyDecision::Deny);
    }
    let mut changed = request;
    changed.requested_scopes[0] = RequestedScope::Filesystem {
        root: "/workspace/*".into(),
        access: vec![FilesystemAccess::Modify],
        recursive: true,
    };
    assert_eq!(adapter.evaluate(&changed).decision, PolicyDecision::Deny);
}

fn mcp_action() -> ApprovedAction {
    ApprovedAction::mcp(fixture_digest('1'),"2025-11-25".into(),json!({"name":"lookup","inputSchema":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"],"additionalProperties":false}})).unwrap()
}
fn mcp_scope() -> RequestedScope {
    RequestedScope::ExternalService {
        service: fixture_digest('1'),
        operation: "lookup".into(),
        resource: "/workspace".into(),
    }
}
fn mcp_input() -> Value {
    json!({"server":fixture_digest('1'),"tool":"lookup","arguments":{"query":"approved"}})
}

#[test]
fn mcp_descriptor_schema_identity_and_required_arguments_are_enforced() {
    let action = mcp_action();
    let input = mcp_input();
    let pack = prepare(&action, &mcp_scope(), &input, "");
    let (_, request) = effect(&action, mcp_scope(), &input, pack.identities());
    let original_ids = pack.identities().clone();
    let wrong = json!({"server":fixture_digest('1'),"tool":"lookup","arguments":{}});
    let wrong_type =
        json!({"server":fixture_digest('1'),"tool":"lookup","arguments":{"query":123}});
    let extra = json!({"server":fixture_digest('1'),"tool":"lookup","arguments":{"query":"approved","extra":true}});
    let adapter = activate(
        pack,
        vec![input, wrong.clone(), wrong_type.clone(), extra.clone()],
    );
    assert_eq!(adapter.evaluate(&request).decision, PolicyDecision::Allow);
    for input in [wrong, wrong_type, extra] {
        let mut changed = request.clone();
        changed.input_digest = content_digest(&input).unwrap();
        assert_eq!(adapter.evaluate(&changed).decision, PolicyDecision::Deny);
        assert!(!adapter.evaluate(&changed).errors.is_empty());
    }
    let mut descriptor = action.mcp.as_ref().unwrap().descriptor.clone();
    descriptor["inputSchema"]["properties"]["query"]["type"] = json!("integer");
    let changed =
        ApprovedAction::mcp(fixture_digest('1'), "2025-11-25".into(), descriptor).unwrap();
    assert_eq!(action.capability.name, changed.capability.name);
    assert_ne!(action.capability.digest, changed.capability.digest);
    assert_ne!(action.tool_schema_digest, changed.tool_schema_digest);
    let pack = prepare(&changed, &mcp_scope(), &mcp_input(), "");
    assert_ne!(original_ids[1], pack.identities()[1]);
    let mut description = action.mcp.as_ref().unwrap().descriptor.clone();
    description["description"] = json!("Changed advertised behavior");
    let described =
        ApprovedAction::mcp(fixture_digest('1'), "2025-11-25".into(), description).unwrap();
    assert_ne!(action.capability.digest, described.capability.digest);
    assert_eq!(action.tool_schema_digest, described.tool_schema_digest);
    let described_pack = prepare(&described, &mcp_scope(), &mcp_input(), "");
    assert_ne!(original_ids[1], described_pack.identities()[1]);
    let changed_adapter = activate(pack, vec![mcp_input()]);
    assert_eq!(
        changed_adapter.evaluate(&request).decision,
        PolicyDecision::Deny
    );
    let mut unknown = request;
    unknown.requested_scopes = vec![RequestedScope::ExternalService {
        service: fixture_digest('2'),
        operation: "lookup".into(),
        resource: "/workspace".into(),
    }];
    assert_eq!(adapter.evaluate(&unknown).decision, PolicyDecision::Deny);
}

#[test]
fn unsupported_descriptor_semantics_and_forged_identity_are_rejected() {
    let mut unsafe_descriptor = mcp_action().mcp.unwrap().descriptor;
    unsafe_descriptor["unknownNumericMetadata"] = json!(9_007_199_254_740_992_u64);
    assert!(
        ApprovedAction::mcp(fixture_digest('1'), "2025-11-25".into(), unsafe_descriptor).is_err()
    );
    let mut action = mcp_action();
    action.capability.digest = fixture_digest('0');
    assert!(
        PreparedPack::prepare(
            "pack",
            "1",
            "local-run",
            "forbid(principal, action, resource);",
            vec![action]
        )
        .is_err()
    );
    for schema in [
        json!({"type":"object","properties":{},"additionalProperties":true}),
        json!({"type":"string","pattern":"^safe$"}),
        json!({"type":"number"}),
        json!({"type":"string"}),
    ] {
        assert!(
            ApprovedAction::mcp(
                fixture_digest('1'),
                "2025-11-25".into(),
                json!({"name":"lookup","inputSchema":schema})
            )
            .is_err()
        );
    }
}

#[test]
fn evaluation_errors_override_otherwise_allowed_decisions() {
    let source = exact_permit(
        "local-run",
        &action(),
        &scope(),
        &content_digest(&input()).unwrap(),
    )
    .unwrap();
    let source = format!(
        "{source}\n{} when {{ 9223372036854775807 + 1 > 0 }};",
        source.trim_end_matches(';')
    );
    let pack = PreparedPack::prepare("pack", "1", "local-run", &source, vec![action()]).unwrap();
    let (_, request) = effect(&action(), scope(), &input(), pack.identities());
    let evidence = activate(pack, vec![input()]).evaluate(&request);
    assert_eq!(evidence.decision, PolicyDecision::Deny);
    assert!(!evidence.errors.is_empty());
}

#[test]
fn parallel_readonly_evaluations_are_deterministic_and_bound() {
    let pack = prepare(&action(), &scope(), &input(), "");
    let (_, request) = effect(&action(), scope(), &input(), pack.identities());
    let adapter = Arc::new(activate(pack, vec![input()]));
    let expected = adapter.evaluate(&request);
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let adapter = adapter.clone();
            let request = request.clone();
            std::thread::spawn(move || adapter.evaluate(&request))
        })
        .collect();
    for thread in threads {
        assert_eq!(thread.join().unwrap(), expected);
    }
    let mut changed = expected.clone();
    changed.effect_request_digest = fixture_digest('0');
    assert_ne!(expected.evidence_digest(), changed.evidence_digest());
}

struct Agent {
    plan: String,
    steps: VecDeque<AgentStep>,
}
fn usage() -> ResourceUsage {
    ResourceUsage {
        cost_micros: 0,
        elapsed_ms: 0,
        model_tokens: 0,
        tool_calls: 0,
    }
}
impl AgentAdapter for Agent {
    fn plan(
        &mut self,
        _: &AgentRunRequest,
    ) -> Result<PortObservation<PlanProposal>, RuntimePortError> {
        Ok(PortObservation {
            value: PlanProposal {
                plan_digest: self.plan.clone(),
            },
            usage: usage(),
        })
    }
    fn next_effect(
        &mut self,
        _: AgentRunContext<'_>,
    ) -> Result<PortObservation<AgentStep>, RuntimePortError> {
        Ok(PortObservation {
            value: self.steps.pop_front().unwrap(),
            usage: usage(),
        })
    }
}
struct Executor {
    path: std::path::PathBuf,
    calls: Arc<AtomicUsize>,
}
fn reference(kind: EffectEvidenceType) -> EffectEvidenceReference {
    EffectEvidenceReference {
        evidence_type: kind,
        digest: fixture_digest('e'),
        locator: None,
    }
}
fn executor_id() -> ExecutorIdentity {
    ExecutorIdentity {
        name: "fixture".into(),
        version: "1".into(),
        digest: fixture_digest('f'),
    }
}
impl ExecutorPort for Executor {
    fn execute(
        &mut self,
        effect: &ProtectedEffectRequest,
    ) -> Result<ExecutorObservation, RuntimePortError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::fs::write(&self.path, b"approved").unwrap();
        let mut subject = effect.subject.clone();
        subject.digest = fixture_digest('2');
        Ok(ExecutorObservation {
            execution_status: EffectExecutionStatus::Executed,
            observed_post_effect_subject: Some(subject),
            exit: None,
            usage: EffectUsage {
                cost_micros: 0,
                elapsed_ms: 0,
                model_tokens: 0,
                tool_calls: 1,
            },
            executor: Some(executor_id()),
            sandbox_profile: Some(effect.sandbox_profile.clone()),
            reason: None,
            evidence: vec![
                reference(EffectEvidenceType::Executor),
                reference(EffectEvidenceType::Sandbox),
                reference(EffectEvidenceType::Usage),
                reference(EffectEvidenceType::Mutation),
                reference(EffectEvidenceType::Artifact),
                reference(EffectEvidenceType::SubjectObservation),
            ],
        })
    }
}
struct Verifier {
    pass: bool,
}
impl VerifierPort for Verifier {
    fn verify(
        &mut self,
        context: VerificationContext<'_>,
    ) -> Result<PortObservation<VerificationReport>, RuntimePortError> {
        Ok(PortObservation {
            value: VerificationReport {
                subject_digest: context.current_subject.digest.clone(),
                verifier_id: "independent".into(),
                verdict: if self.pass {
                    VerificationVerdict::Pass
                } else {
                    VerificationVerdict::Fail
                },
                evidence: vec![EvidenceReference {
                    evidence_type: EvidenceType::Artifact,
                    digest: fixture_digest('e'),
                    locator: None,
                }],
            },
            usage: usage(),
        })
    }
}

#[test]
fn real_engine_preserves_static_and_runtime_authority_and_evidence() {
    for mode in [
        "allow",
        "deny",
        "context",
        "tool_trust",
        "risk",
        "budget",
        "verification",
        "legacy_deny",
        "host_unset",
        "stale_subject",
        "schema_drift",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.txt");
        let scope = RequestedScope::Filesystem {
            root: path.to_str().unwrap().into(),
            access: vec![FilesystemAccess::Modify],
            recursive: false,
        };
        let pack = prepare(
            &action(),
            &scope,
            &input(),
            if mode == "deny" {
                "\nforbid(principal, action, resource);"
            } else {
                ""
            },
        );
        let (mut run, mut effect) = effect(&action(), scope, &input(), pack.identities());
        if mode == "stale_subject" {
            effect.subject.digest = fixture_digest('3');
        }
        if mode == "schema_drift" {
            effect.tool_schema_digest = Some(fixture_digest('4'));
        }
        if mode == "budget" {
            run.resource_budget.max_tool_calls = 0;
            effect.agent_run_request_digest =
                validate_request(&run, &ContractSupport::new(run.policies.clone())).unwrap();
            effect.resource_budget_digest =
                canonical_resource_budget_digest(&run.resource_budget).unwrap();
        }
        let adapter = activate(
            pack,
            if mode == "context" {
                vec![]
            } else {
                vec![input()]
            },
        );
        let support = ContractSupport::new(run.policies.clone());
        let runtime = RuntimeSupport::new(
            support.clone(),
            if mode == "tool_trust" {
                vec![]
            } else {
                vec![cap()]
            },
            PermissionPolicy {
                policy_decision: if mode == "legacy_deny" {
                    Some(PolicyDecision::Deny)
                } else if mode == "host_unset" {
                    None
                } else {
                    Some(PolicyDecision::Allow)
                },
                risk_tags: if mode == "risk" {
                    vec!["destructive_path".into()]
                } else {
                    vec![]
                },
                wrapper_chain: vec![],
                approvals: vec![],
            },
            EffectUsage {
                cost_micros: 0,
                elapsed_ms: 0,
                model_tokens: 0,
                tool_calls: 1,
            },
            vec![],
            vec!["independent".into()],
        )
        .with_static_policy_adapter(Arc::new(adapter));
        let config = RuntimeConfig {
            implementer_id: "implementer".into(),
            executor_identity: executor_id(),
            max_effects: 2,
            budget_thresholds: BudgetThresholds {
                warn_micros: 80,
                approval_micros: 90,
                hard_stop_micros: 100,
            },
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let mut engine = AgentRunEngine::new(
            runtime,
            config,
            Agent {
                plan: fixture_digest('b'),
                steps: VecDeque::from([
                    AgentStep::ProtectedEffect(Box::new(ProtectedEffectProposal {
                        request: effect.clone(),
                    })),
                    AgentStep::Finish,
                ]),
            },
            Executor {
                path: path.clone(),
                calls: calls.clone(),
            },
            Verifier {
                pass: mode != "verification",
            },
        );
        let execution = engine.run(run.clone()).unwrap();
        let executed = mode == "allow" || mode == "verification";
        assert_eq!(
            calls.load(Ordering::SeqCst),
            usize::from(executed),
            "{mode}"
        );
        assert_eq!(path.exists(), executed, "{mode}");
        assert_eq!(
            execution.receipt.body.terminal_status,
            if mode == "allow" {
                AgentRunStatus::Completed
            } else {
                AgentRunStatus::Blocked
            },
            "{mode}"
        );
        verify_terminal_receipt(&run, &support, &execution.receipt).unwrap();
        verify_protected_effect_result(
            &run,
            &support,
            &effect,
            &execution.protected_effect_results[0],
        )
        .unwrap();
        if mode == "host_unset" {
            assert_eq!(
                execution.receipt.body.terminal_reason,
                "permission.unknown_action"
            );
            assert_eq!(
                execution.protected_effect_results[0].body.execution_status,
                EffectExecutionStatus::AwaitingAuthority
            );
            assert_eq!(
                execution.static_policy_evaluations[0].decision,
                PolicyDecision::Allow
            );
        }
        if !["budget", "stale_subject"].contains(&mode) {
            let evaluation = &execution.static_policy_evaluations[0];
            let marker = format!("static_policy.evidence:{}", evaluation.evidence_digest());
            assert!(execution.receipt.body.events.iter().any(|event|matches!(event,RunEvent::ProtectedEffectDecision{decision,..} if decision.effects.contains(&marker))));
            assert!(effect.policies.contains(&evaluation.policy));
            assert!(effect.policies.contains(&evaluation.schema));
            assert_eq!(
                evaluation.effect_request_digest,
                content_digest(&effect).unwrap()
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn real_stdio_mcp_descriptors_and_effects_pass_through_cedar_and_engine() {
    use gaap_mcp::{
        LaunchArtifact, MCP_PROTOCOL_VERSION, McpExecutor, ServerConfig, SubjectObservation,
        TransportLimits,
    };
    use sha2::{Digest, Sha256};
    use std::process::Command;
    fn hash(bytes: &[u8]) -> String {
        format!("sha256:{:x}", Sha256::digest(bytes))
    }
    for deny in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("dispatch");
        let python = Command::new("python3")
            .args(["-c", "import sys; print(sys.executable)"])
            .output()
            .unwrap();
        assert!(python.status.success());
        let executable = std::path::PathBuf::from(String::from_utf8(python.stdout).unwrap().trim())
            .canonicalize()
            .unwrap();
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../mcp/tests/fixtures/server.py")
            .canonicalize()
            .unwrap();
        let mut config = ServerConfig::new(executable.clone(), "fixture", "1");
        config.arguments = vec![
            fixture.display().to_string(),
            "normal".into(),
            marker.display().to_string(),
        ];
        config.working_directory = dir.path().canonicalize().unwrap();
        config.artifacts = vec![
            LaunchArtifact {
                path: executable.clone(),
                digest: hash(&std::fs::read(&executable).unwrap()),
            },
            LaunchArtifact {
                path: fixture.clone(),
                digest: hash(&std::fs::read(&fixture).unwrap()),
            },
        ];
        config.artifact_digest = config.artifact_manifest_digest().unwrap();
        let permission = PermissionPolicy {
            policy_decision: Some(PolicyDecision::Allow),
            risk_tags: vec![],
            wrapper_chain: vec![],
            approvals: vec![],
        };
        // Preparation inspects host-owned identities; the probe never invokes tools/call.
        let mut probe = McpExecutor::discover(config.clone(), TransportLimits::default()).unwrap();
        let tool = probe.tools()[0].clone();
        let provisional = [PolicyIdentity {
            name: "probe".into(),
            version: "1".into(),
            digest: fixture_digest('1'),
        }];
        let run = run_request(tool.capability.clone(), &provisional);
        let observed = run.subject.clone();
        probe
            .bind_run(
                run.clone(),
                ContractSupport::new(provisional),
                permission.clone(),
                move |_: &ProtectedEffectRequest| {
                    Ok(SubjectObservation {
                        subject: observed.clone(),
                        effect_confirmed: false,
                        cost_micros: Some(0),
                        model_tokens: Some(0),
                    })
                },
            )
            .unwrap();
        let preliminary = probe
            .prepare_effect(
                "echo",
                json!({"message":"hello"}),
                "probe".into(),
                1,
                run.subject,
                EffectClass::Observation,
            )
            .unwrap();
        let server = match &preliminary.requested_scopes[0] {
            RequestedScope::ExternalService { service, .. } => service.clone(),
            _ => unreachable!(),
        };
        drop(probe);
        assert!(!marker.exists());
        let action =
            ApprovedAction::mcp(server.clone(), MCP_PROTOCOL_VERSION.into(), tool.descriptor)
                .unwrap();
        assert_eq!(action.capability, tool.capability);
        assert_eq!(
            action.tool_schema_digest.as_ref(),
            Some(&tool.schema_digest)
        );
        let input = json!({"server":server,"tool":"echo","arguments":{"message":"hello"}});
        assert_eq!(content_digest(&input).unwrap(), preliminary.input_digest);
        let pack = prepare(
            &action,
            &preliminary.requested_scopes[0],
            &input,
            if deny {
                "\nforbid(principal, action, resource);"
            } else {
                ""
            },
        );
        let run = run_request(action.capability.clone(), pack.identities());
        let support = ContractSupport::new(run.policies.clone());
        let adapter = activate(pack, vec![input]);
        let mut executor = McpExecutor::discover(config, TransportLimits::default()).unwrap();
        let identity = executor.identity().clone();
        let observed = run.subject.clone();
        let confirmation = marker.clone();
        executor
            .bind_run(
                run.clone(),
                support.clone(),
                permission.clone(),
                move |_: &ProtectedEffectRequest| {
                    Ok(SubjectObservation {
                        subject: observed.clone(),
                        effect_confirmed: confirmation.exists(),
                        cost_micros: Some(0),
                        model_tokens: Some(0),
                    })
                },
            )
            .unwrap();
        executor.approve_capability(&action.capability).unwrap();
        let effect = executor
            .prepare_effect(
                "echo",
                json!({"message":"hello"}),
                "effect-1".into(),
                1,
                run.subject.clone(),
                EffectClass::Observation,
            )
            .unwrap();
        assert_eq!(
            adapter.evaluate(&effect).decision,
            if deny {
                PolicyDecision::Deny
            } else {
                PolicyDecision::Allow
            }
        );
        let runtime = RuntimeSupport::new(
            support.clone(),
            vec![action.capability.clone()],
            permission,
            EffectUsage {
                cost_micros: 0,
                elapsed_ms: 0,
                model_tokens: 0,
                tool_calls: 1,
            },
            vec![],
            vec!["independent".into()],
        )
        .with_trusted_tool_schemas(vec![TrustedToolSchema {
            capability: action.capability,
            tool_schema_digest: tool.schema_digest,
        }])
        .with_static_policy_adapter(Arc::new(adapter));
        let config = RuntimeConfig {
            implementer_id: "implementer".into(),
            executor_identity: identity,
            max_effects: 1,
            budget_thresholds: BudgetThresholds {
                warn_micros: 80,
                approval_micros: 90,
                hard_stop_micros: 100,
            },
        };
        let mut engine = AgentRunEngine::new(
            runtime,
            config,
            Agent {
                plan: fixture_digest('b'),
                steps: VecDeque::from([
                    AgentStep::ProtectedEffect(Box::new(ProtectedEffectProposal {
                        request: effect.clone(),
                    })),
                    AgentStep::Finish,
                ]),
            },
            executor,
            Verifier { pass: true },
        );
        let execution = engine.run(run.clone()).unwrap();
        assert_eq!(marker.exists(), !deny);
        assert_eq!(
            execution.receipt.body.terminal_status,
            if deny {
                AgentRunStatus::Blocked
            } else {
                AgentRunStatus::Completed
            }
        );
        verify_terminal_receipt(&run, &support, &execution.receipt).unwrap();
        verify_protected_effect_result(
            &run,
            &support,
            &effect,
            &execution.protected_effect_results[0],
        )
        .unwrap();
    }
}
