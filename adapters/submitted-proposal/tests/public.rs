//! External consumers exercise the public adapter through real engine ports.
use gaap::{contracts::*, runtime::*};
use gaap_submitted_proposal::{RunControl, SubmittedProposal, SubmittedProposalAdapter};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf, time::Duration};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    plan: String,
    base_digest: String,
    replacement: String,
}
fn hash(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
fn usage(calls: u64) -> ResourceUsage {
    ResourceUsage {
        cost_micros: 0,
        model_tokens: 0,
        elapsed_ms: 1,
        tool_calls: calls,
    }
}
fn profile() -> SandboxProfileIdentity {
    SandboxProfileIdentity {
        name: "trusted-test-process".into(),
        version: "1".into(),
        digest: hash(b"fixture only"),
    }
}
fn executor() -> ExecutorIdentity {
    ExecutorIdentity {
        name: "text-fixture-writer".into(),
        version: "1".into(),
        digest: hash(b"write and observe"),
    }
}
struct Fixture {
    _root: tempfile::TempDir,
    path: PathBuf,
    input: Input,
    request: AgentRunRequest,
    effect: ProtectedEffectRequest,
    support: ContractSupport,
    expected: String,
}
impl Fixture {
    fn new(filename: &str, expected: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join(filename);
        fs::write(&path, "before").unwrap();
        let subject = Subject {
            kind: SubjectKind::Repository,
            locator: root.path().display().to_string(),
            digest: hash(b"before"),
        };
        let policy = PolicyIdentity {
            name: format!("{filename}-policy"),
            version: "1".into(),
            digest: hash(filename.as_bytes()),
        };
        let support = ContractSupport::new([policy.clone()]);
        let input = Input {
            plan: format!("Replace {filename} and verify its exact text"),
            base_digest: subject.digest.clone(),
            replacement: expected.into(),
        };
        let proposal = SubmittedProposal::new(&input, &input.plan, &input.base_digest).unwrap();
        let request = AgentRunRequest {
            schema_version: AGENT_RUN_REQUEST_SCHEMA.into(),
            request_id: format!("host-request-{filename}"),
            run_id: format!("host-run-{filename}"),
            subject: subject.clone(),
            requested_capability: CapabilityIdentity {
                name: "text.replace".into(),
                version: "1".into(),
                digest: hash(b"text descriptor"),
            },
            task: TaskSpec {
                instructions: input.plan.clone(),
                constraints: vec!["one host-owned text target".into()],
            },
            policies: vec![policy.clone()],
            resource_budget: ResourceBudget {
                max_cost_micros: 1000,
                max_model_tokens: 0,
                max_elapsed_ms: 1000,
                max_tool_calls: 2,
            },
            approval_context: vec![ApprovalReference {
                approval_id: "host-approval".into(),
                actor_id: "host-policy".into(),
                scope: "exact fixture subject".into(),
                subject_digest: subject.digest,
                evidence: EvidenceReference {
                    evidence_type: EvidenceType::Approval,
                    digest: policy.digest,
                    locator: None,
                },
            }],
            required_verification: VerificationRequirement {
                independence: VerificationIndependence::DifferentActor,
                evidence_types: vec![EvidenceType::Artifact, EvidenceType::CommandOutput],
            },
        };
        let effect = ProtectedEffectRequest {
            schema_version: PROTECTED_EFFECT_REQUEST_SCHEMA.into(),
            effect_id: format!("{}/replace", request.run_id),
            effect_sequence: 1,
            run_id: request.run_id.clone(),
            agent_run_request_digest: validate_request(&request, &support).unwrap(),
            subject: request.subject.clone(),
            operation_family: OperationFamily::Filesystem,
            normalized_operation: "filesystem.write".into(),
            capability: request.requested_capability.clone(),
            tool_schema_digest: Some(hash(b"schema")),
            input_digest: proposal.input_digest().into(),
            input_metadata: vec![InputMetadataEntry {
                name: "path".into(),
                value: filename.into(),
            }],
            requested_scopes: vec![RequestedScope::Filesystem {
                root: request.subject.locator.clone(),
                access: vec![FilesystemAccess::Read, FilesystemAccess::Modify],
                recursive: false,
            }],
            policies: request.policies.clone(),
            approval_context: vec![],
            resource_budget_digest: canonical_resource_budget_digest(&request.resource_budget)
                .unwrap(),
            sandbox_profile: profile(),
            idempotency_key: format!("{}/replace", request.run_id),
            repeatability: Repeatability::NonRepeatable,
            expected_effect_class: EffectClass::Mutation,
        };
        Self {
            _root: root,
            path,
            input,
            request,
            effect,
            support,
            expected: expected.into(),
        }
    }
    fn adapter(&self, control: RunControl) -> Result<SubmittedProposalAdapter, RuntimePortError> {
        SubmittedProposalAdapter::new(
            SubmittedProposal::new(&self.input, &self.input.plan, &self.input.base_digest)?,
            &self.request,
            self.effect.clone(),
            &self.support,
            control,
        )
    }
    fn engine(
        &self,
        control: RunControl,
        unknown: bool,
        verifier_id: &str,
        cancel_in_verify: bool,
        changed_descriptor: bool,
    ) -> AgentRunEngine<SubmittedProposalAdapter, Writer, Oracle> {
        let mut trusted = self.request.requested_capability.clone();
        if changed_descriptor {
            trusted.digest = hash(b"old descriptor");
        }
        let support = RuntimeSupport::new(
            self.support.clone(),
            vec![trusted],
            PermissionPolicy {
                policy_decision: Some(PolicyDecision::Allow),
                risk_tags: vec![],
                wrapper_chain: vec![],
                approvals: vec![],
            },
            EffectUsage {
                cost_micros: 0,
                model_tokens: 0,
                elapsed_ms: 1,
                tool_calls: 1,
            },
            vec![],
            vec!["host-text-oracle".into()],
        );
        AgentRunEngine::new(
            support,
            RuntimeConfig {
                implementer_id: "submitted-test-host".into(),
                executor_identity: executor(),
                max_effects: 1,
                budget_thresholds: BudgetThresholds {
                    warn_micros: 800,
                    approval_micros: 900,
                    hard_stop_micros: 1000,
                },
            },
            self.adapter(control.clone()).unwrap(),
            Writer {
                path: self.path.clone(),
                input: self.input.clone(),
                unknown,
                control: control.clone(),
            },
            Oracle {
                path: self.path.clone(),
                expected: self.expected.clone(),
                verifier_id: verifier_id.into(),
                cancel: cancel_in_verify.then_some(control),
            },
        )
    }
    fn verify(&self, output: &AgentRunExecution) {
        verify_terminal_receipt(&self.request, &self.support, &output.receipt).unwrap();
        for result in &output.protected_effect_results {
            verify_protected_effect_result(&self.request, &self.support, &self.effect, result)
                .unwrap();
        }
    }
}
struct Writer {
    path: PathBuf,
    input: Input,
    unknown: bool,
    control: RunControl,
}
impl ExecutorPort for Writer {
    fn cancellation(&self) -> Option<Cancellation> {
        self.control.cancellation()
    }
    fn execute(
        &mut self,
        request: &ProtectedEffectRequest,
    ) -> Result<ExecutorObservation, RuntimePortError> {
        if self.control.cancellation().is_some()
            || hash(&fs::read(&self.path).unwrap()) != request.subject.digest
        {
            return Err(RuntimePortError::before_dispatch(
                "fixture stopped before dispatch",
            ));
        }
        fs::write(&self.path, &self.input.replacement).unwrap();
        if self.unknown {
            return Err(RuntimePortError::new("fixture lost write acknowledgement"));
        }
        let observed = hash(&fs::read(&self.path).unwrap());
        let mut subject = request.subject.clone();
        subject.digest = observed.clone();
        Ok(ExecutorObservation {
            execution_status: EffectExecutionStatus::Executed,
            observed_post_effect_subject: Some(subject),
            exit: None,
            usage: EffectUsage {
                cost_micros: 0,
                model_tokens: 0,
                elapsed_ms: 1,
                tool_calls: 1,
            },
            executor: Some(executor()),
            sandbox_profile: Some(profile()),
            reason: None,
            evidence: [
                EffectEvidenceType::Executor,
                EffectEvidenceType::Sandbox,
                EffectEvidenceType::SubjectObservation,
                EffectEvidenceType::Mutation,
                EffectEvidenceType::Artifact,
                EffectEvidenceType::Usage,
            ]
            .into_iter()
            .map(|evidence_type| EffectEvidenceReference {
                evidence_type,
                digest: observed.clone(),
                locator: None,
            })
            .collect(),
        })
    }
}
struct Oracle {
    path: PathBuf,
    expected: String,
    verifier_id: String,
    cancel: Option<RunControl>,
}
impl VerifierPort for Oracle {
    fn verify(
        &mut self,
        context: VerificationContext<'_>,
    ) -> Result<PortObservation<VerificationReport>, RuntimePortError> {
        let actual = fs::read_to_string(&self.path).unwrap();
        if let Some(control) = &self.cancel {
            control.cancel();
        }
        Ok(PortObservation {
            value: VerificationReport {
                subject_digest: context.current_subject.digest.clone(),
                verifier_id: self.verifier_id.clone(),
                verdict: if actual == self.expected {
                    VerificationVerdict::Pass
                } else {
                    VerificationVerdict::Fail
                },
                evidence: [EvidenceType::Artifact, EvidenceType::CommandOutput]
                    .into_iter()
                    .map(|evidence_type| EvidenceReference {
                        evidence_type,
                        digest: hash(actual.as_bytes()),
                        locator: None,
                    })
                    .collect(),
            },
            usage: usage(1),
        })
    }
}
fn control() -> RunControl {
    RunControl::new(Duration::from_secs(1))
}

#[test]
fn two_task_configurations_use_the_same_public_adapter_and_engine() {
    for (filename, expected) in [
        ("greeting.txt", "Hello, Ada!\n"),
        ("inventory.csv", "sku,count\nA17,3\n"),
    ] {
        let fixture = Fixture::new(filename, expected);
        let output = fixture
            .engine(control(), false, "host-text-oracle", false, false)
            .run(fixture.request.clone())
            .unwrap();
        fixture.verify(&output);
        assert_eq!(
            output.receipt.body.terminal_status,
            AgentRunStatus::Completed
        );
        assert_eq!(fs::read_to_string(&fixture.path).unwrap(), expected);
        assert!(output.receipt.body.events.iter().any(|event| matches!(event, RunEvent::PlanRecorded { plan_digest, .. } if plan_digest == &fixture.effect.input_digest)));
    }
}
#[test]
fn changed_plan_subject_payload_policy_and_capability_bindings_are_rejected() {
    for change in 0..6 {
        let mut fixture = Fixture::new("report.txt", "accepted\n");
        match change {
            0 => fixture.input.plan.push_str(" altered"),
            1 => fixture.input.replacement.push_str(" altered"),
            2 => fixture.input.base_digest = hash(b"stale"),
            3 => fixture.effect.capability.digest = hash(b"changed"),
            4 => fixture.effect.policies[0].digest = hash(b"changed"),
            _ => fixture.effect.input_digest = hash(b"different proposal"),
        }
        assert!(fixture.adapter(control()).is_err(), "mutation {change}");
        assert_eq!(fs::read_to_string(fixture.path).unwrap(), "before");
    }
}
#[test]
fn strict_host_parser_rejects_extra_fields_instead_of_promoting_authority() {
    let fixture = Fixture::new("report.txt", "accepted\n");
    for field in [
        "approved",
        "verifier_id",
        "policy_decision",
        "terminal_status",
        "threadloop_claim",
        "request_id",
        "run_id",
    ] {
        let mut value = serde_json::to_value(&fixture.input).unwrap();
        value[field] = serde_json::json!(true);
        assert!(serde_json::from_value::<Input>(value).is_err(), "{field}");
    }
}
#[test]
fn cancellation_and_deadline_before_dispatch_have_no_effect() {
    for deadline in [false, true] {
        let fixture = Fixture::new("report.txt", "accepted\n");
        let control = if deadline {
            RunControl::new(Duration::ZERO)
        } else {
            let control = control();
            control.cancel();
            control
        };
        let output = fixture
            .engine(control, false, "host-text-oracle", false, false)
            .run(fixture.request.clone())
            .unwrap();
        fixture.verify(&output);
        assert_eq!(
            output.receipt.body.terminal_status,
            AgentRunStatus::Interrupted
        );
        assert!(output.protected_effect_results.is_empty());
        assert_eq!(fs::read_to_string(fixture.path).unwrap(), "before");
    }
}
#[test]
fn late_cancel_or_untrusted_verifier_retains_the_executed_effect() {
    for (verifier_id, cancel) in [
        ("caller-forged-verifier", false),
        ("host-text-oracle", true),
    ] {
        let fixture = Fixture::new("report.txt", "accepted\n");
        let output = fixture
            .engine(control(), false, verifier_id, cancel, false)
            .run(fixture.request.clone())
            .unwrap();
        fixture.verify(&output);
        assert_ne!(
            output.receipt.body.terminal_status,
            AgentRunStatus::Completed
        );
        assert_eq!(
            output.protected_effect_results[0].body.execution_status,
            EffectExecutionStatus::Executed
        );
        assert_eq!(fs::read_to_string(fixture.path).unwrap(), "accepted\n");
    }
}
#[test]
fn changed_descriptor_revokes_trust_and_unknown_outcome_cant_replay_adapter() {
    let fixture = Fixture::new("report.txt", "accepted\n");
    let output = fixture
        .engine(control(), false, "host-text-oracle", false, true)
        .run(fixture.request.clone())
        .unwrap();
    fixture.verify(&output);
    assert_eq!(output.receipt.body.terminal_status, AgentRunStatus::Blocked);
    assert_eq!(fs::read_to_string(&fixture.path).unwrap(), "before");
    let mut engine = fixture.engine(control(), true, "host-text-oracle", false, false);
    let output = engine.run(fixture.request.clone()).unwrap();
    fixture.verify(&output);
    assert_eq!(
        output.protected_effect_results[0].body.execution_status,
        EffectExecutionStatus::UnknownOutcome
    );
    let replay = engine.run(fixture.request.clone()).unwrap();
    fixture.verify(&replay);
    assert!(replay.protected_effect_results.is_empty());
    assert_eq!(replay.receipt.body.terminal_status, AgentRunStatus::Failed);
    assert_eq!(fs::read_to_string(fixture.path).unwrap(), "accepted\n");
}
