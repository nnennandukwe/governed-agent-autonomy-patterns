use gaap::contracts::{
    AGENT_RUN_REQUEST_SCHEMA, AgentRunRequest, AgentRunStatus, ApprovalReference,
    CapabilityIdentity, ContractErrorCode, ContractSupport, EvidenceReference, EvidenceType,
    PolicyIdentity, ResourceBudget, ResourceUsage, RunEvent, Subject, SubjectKind,
    TERMINAL_RUN_RECEIPT_SCHEMA, TaskSpec, TerminalRunReceiptBody, VerificationIndependence,
    VerificationRequirement, VerificationVerdict, canonical_request_bytes,
    parse_agent_run_request_json, parse_terminal_run_receipt_json, seal_terminal_receipt,
    validate_request, verify_terminal_receipt,
};
use gaap::{Decision, Gate, Outcome};

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

fn approval() -> ApprovalReference {
    ApprovalReference {
        approval_id: "approval-001".to_owned(),
        actor_id: "maintainer-001".to_owned(),
        scope: "plan".to_owned(),
        subject_digest: digest('d'),
        evidence: EvidenceReference {
            evidence_type: EvidenceType::Approval,
            digest: digest('e'),
            locator: None,
        },
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
        approval_context: vec![approval()],
        required_verification: VerificationRequirement {
            independence: VerificationIndependence::DifferentActor,
            evidence_types: vec![EvidenceType::CommandOutput, EvidenceType::Artifact],
        },
    }
}

fn evidence(evidence_type: EvidenceType, byte: char) -> EvidenceReference {
    EvidenceReference {
        evidence_type,
        digest: digest(byte),
        locator: None,
    }
}

fn usage() -> ResourceUsage {
    ResourceUsage {
        cost_micros: 200_000,
        elapsed_ms: 2_000,
        model_tokens: 2_500,
        tool_calls: 1,
    }
}

fn zero_usage() -> ResourceUsage {
    ResourceUsage {
        cost_micros: 0,
        elapsed_ms: 0,
        model_tokens: 0,
        tool_calls: 0,
    }
}

fn completed_body(request_digest: String) -> TerminalRunReceiptBody {
    let initial_subject = digest('a');
    let resulting_subject = digest('f');
    let protected_effect = digest('7');
    TerminalRunReceiptBody {
        schema_version: TERMINAL_RUN_RECEIPT_SCHEMA.to_owned(),
        request_id: "request-001".to_owned(),
        run_id: "run-001".to_owned(),
        request_digest,
        initial_subject_digest: initial_subject.clone(),
        resulting_subject_digest: resulting_subject.clone(),
        terminal_status: AgentRunStatus::Completed,
        terminal_reason: "workflow.completion_authorized".to_owned(),
        usage: usage(),
        events: vec![
            RunEvent::StatusTransition {
                sequence: 1,
                from: AgentRunStatus::Accepted,
                to: AgentRunStatus::Planning,
                reason: None,
            },
            RunEvent::PlanRecorded {
                sequence: 2,
                plan_digest: digest('d'),
            },
            RunEvent::ApprovalRecorded {
                sequence: 3,
                approval: approval(),
            },
            RunEvent::StatusTransition {
                sequence: 4,
                from: AgentRunStatus::Planning,
                to: AgentRunStatus::Executing,
                reason: None,
            },
            RunEvent::ProtectedEffectDecision {
                sequence: 5,
                decision_id: "decision-tool-001".to_owned(),
                gate: Gate::Permission,
                protected_effect_digest: protected_effect.clone(),
                subject_digest: initial_subject.clone(),
                decision: Decision {
                    outcome: Outcome::Allow,
                    code: "permission.policy_allowed".to_owned(),
                    effects: vec![],
                },
            },
            RunEvent::ToolExecution {
                sequence: 6,
                decision_id: "decision-tool-001".to_owned(),
                protected_effect_digest: protected_effect.clone(),
                action_digest: digest('8'),
                capability_digest: digest('c'),
                evidence: vec![evidence(EvidenceType::ToolExecution, '9')],
            },
            RunEvent::Mutation {
                sequence: 7,
                decision_id: "decision-tool-001".to_owned(),
                protected_effect_digest: protected_effect,
                before_subject_digest: initial_subject,
                after_subject_digest: resulting_subject.clone(),
                evidence: vec![evidence(EvidenceType::Artifact, '0')],
            },
            RunEvent::Usage {
                sequence: 8,
                usage: usage(),
            },
            RunEvent::StatusTransition {
                sequence: 9,
                from: AgentRunStatus::Executing,
                to: AgentRunStatus::Verifying,
                reason: None,
            },
            RunEvent::Verification {
                sequence: 10,
                subject_digest: resulting_subject.clone(),
                implementer_id: "executor-001".to_owned(),
                verifier_id: "verifier-001".to_owned(),
                verdict: VerificationVerdict::Pass,
                evidence: vec![
                    evidence(EvidenceType::CommandOutput, '1'),
                    evidence(EvidenceType::Artifact, '2'),
                ],
            },
            RunEvent::ProtectedEffectDecision {
                sequence: 11,
                decision_id: "decision-completion-001".to_owned(),
                gate: Gate::Workflow,
                protected_effect_digest: resulting_subject.clone(),
                subject_digest: resulting_subject,
                decision: Decision {
                    outcome: Outcome::Allow,
                    code: "workflow.completion_authorized".to_owned(),
                    effects: vec!["record_completion".to_owned()],
                },
            },
            RunEvent::StatusTransition {
                sequence: 12,
                from: AgentRunStatus::Verifying,
                to: AgentRunStatus::Completed,
                reason: Some("workflow.completion_authorized".to_owned()),
            },
        ],
    }
}

fn set_event_sequence(event: &mut RunEvent, value: u64) {
    match event {
        RunEvent::StatusTransition { sequence, .. }
        | RunEvent::PlanRecorded { sequence, .. }
        | RunEvent::ApprovalRecorded { sequence, .. }
        | RunEvent::ProtectedEffectDecision { sequence, .. }
        | RunEvent::ToolExecution { sequence, .. }
        | RunEvent::Mutation { sequence, .. }
        | RunEvent::Verification { sequence, .. }
        | RunEvent::Usage { sequence, .. }
        | RunEvent::Interruption { sequence, .. } => *sequence = value,
    }
}

fn terminal_body(
    request_digest: String,
    terminal_status: AgentRunStatus,
    terminal_reason: &str,
) -> TerminalRunReceiptBody {
    let mut events = vec![
        RunEvent::StatusTransition {
            sequence: 1,
            from: AgentRunStatus::Accepted,
            to: AgentRunStatus::Planning,
            reason: None,
        },
        RunEvent::Usage {
            sequence: 2,
            usage: zero_usage(),
        },
    ];
    if terminal_status == AgentRunStatus::Interrupted {
        events.push(RunEvent::Interruption {
            sequence: 3,
            actor_id: Some("operator-001".to_owned()),
            reason: terminal_reason.to_owned(),
            evidence: evidence(EvidenceType::Interruption, '4'),
        });
    }
    let terminal_sequence = (events.len() + 1) as u64;
    events.push(RunEvent::StatusTransition {
        sequence: terminal_sequence,
        from: AgentRunStatus::Planning,
        to: terminal_status,
        reason: Some(terminal_reason.to_owned()),
    });
    TerminalRunReceiptBody {
        schema_version: TERMINAL_RUN_RECEIPT_SCHEMA.to_owned(),
        request_id: "request-001".to_owned(),
        run_id: "run-001".to_owned(),
        request_digest,
        initial_subject_digest: digest('a'),
        resulting_subject_digest: digest('a'),
        terminal_status,
        terminal_reason: terminal_reason.to_owned(),
        usage: zero_usage(),
        events,
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
fn request_accepts_the_jcs_safe_integer_maximum() {
    let mut request = request();
    request.resource_budget.max_cost_micros = 9_007_199_254_740_991;
    request.resource_budget.max_elapsed_ms = 9_007_199_254_740_991;
    request.resource_budget.max_model_tokens = 9_007_199_254_740_991;
    request.resource_budget.max_tool_calls = 9_007_199_254_740_991;
    let support = ContractSupport::new([policy()]);

    validate_request(&request, &support).expect("safe integer maximum should be accepted");
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

#[test]
fn completed_receipt_is_sealed_and_verified_against_the_request() {
    let request = request();
    let support = ContractSupport::new([policy()]);
    let request_digest = validate_request(&request, &support).expect("request should be valid");
    let body = completed_body(request_digest);

    let receipt =
        seal_terminal_receipt(&request, &support, body).expect("completed receipt should seal");

    assert!(receipt.receipt_digest.starts_with("sha256:"));
    verify_terminal_receipt(&request, &support, &receipt).expect("sealed receipt should verify");
}

#[test]
fn ask_decision_cannot_authorize_a_tool_execution() {
    let request = request();
    let support = ContractSupport::new([policy()]);
    let request_digest = validate_request(&request, &support).expect("request should be valid");
    let mut body = completed_body(request_digest);
    let RunEvent::ProtectedEffectDecision { decision, .. } = &mut body.events[4] else {
        panic!("fifth event should be the tool decision");
    };
    decision.outcome = Outcome::Ask;

    let error = seal_terminal_receipt(&request, &support, body)
        .expect_err("ask must not authorize execution");

    assert_eq!(error.code(), ContractErrorCode::UnauthorizedEffect);
    assert_eq!(error.path(), "events[5].decision_id");
}

#[test]
fn tool_execution_must_use_the_requested_capability() {
    let request = request();
    let support = ContractSupport::new([policy()]);
    let request_digest = validate_request(&request, &support).expect("request should be valid");
    let mut body = completed_body(request_digest);
    let RunEvent::ToolExecution {
        capability_digest, ..
    } = &mut body.events[5]
    else {
        panic!("sixth event should be the tool execution");
    };
    *capability_digest = digest('6');

    let error = seal_terminal_receipt(&request, &support, body)
        .expect_err("tool execution must match the requested capability");

    assert_eq!(error.code(), ContractErrorCode::RequestMismatch);
    assert_eq!(error.path(), "events[5].capability_digest");
}

#[test]
fn mutation_after_verification_makes_completion_stale() {
    let request = request();
    let support = ContractSupport::new([policy()]);
    let request_digest = validate_request(&request, &support).expect("request should be valid");
    let mut body = completed_body(request_digest);
    let prior_subject = digest('f');
    let later_subject = digest('6');
    body.resulting_subject_digest = later_subject.clone();
    body.events.insert(
        10,
        RunEvent::ProtectedEffectDecision {
            sequence: 11,
            decision_id: "decision-later-mutation-001".to_owned(),
            gate: Gate::Workflow,
            protected_effect_digest: digest('4'),
            subject_digest: prior_subject.clone(),
            decision: Decision {
                outcome: Outcome::Allow,
                code: "workflow.mutation_authorized".to_owned(),
                effects: vec!["mutate_subject".to_owned()],
            },
        },
    );
    body.events.insert(
        11,
        RunEvent::Mutation {
            sequence: 12,
            decision_id: "decision-later-mutation-001".to_owned(),
            protected_effect_digest: digest('4'),
            before_subject_digest: prior_subject,
            after_subject_digest: later_subject.clone(),
            evidence: vec![evidence(EvidenceType::Artifact, '5')],
        },
    );
    for (index, event) in body.events.iter_mut().enumerate().skip(12) {
        set_event_sequence(event, (index + 1) as u64);
    }
    let RunEvent::ProtectedEffectDecision {
        protected_effect_digest,
        subject_digest,
        ..
    } = &mut body.events[12]
    else {
        panic!("completion decision should follow the later mutation");
    };
    *protected_effect_digest = later_subject.clone();
    *subject_digest = later_subject;

    let error = seal_terminal_receipt(&request, &support, body)
        .expect_err("later mutation must stale verification");

    assert_eq!(error.code(), ContractErrorCode::StaleVerification);
}

#[test]
fn illegal_lifecycle_transition_fails_closed() {
    let request = request();
    let support = ContractSupport::new([policy()]);
    let request_digest = validate_request(&request, &support).expect("request should be valid");
    let mut body = completed_body(request_digest);
    let RunEvent::StatusTransition { to, .. } = &mut body.events[0] else {
        panic!("first event should be a status transition");
    };
    *to = AgentRunStatus::Executing;

    let error =
        seal_terminal_receipt(&request, &support, body).expect_err("illegal transition must fail");

    assert_eq!(error.code(), ContractErrorCode::InvalidTransition);
    assert_eq!(error.path(), "events[0]");
}

#[test]
fn completed_is_only_reachable_from_verifying() {
    let request = request();
    let support = ContractSupport::new([policy()]);
    let request_digest = validate_request(&request, &support).expect("request should be valid");
    let mut body = completed_body(request_digest);
    body.events.retain(|event| {
        !matches!(
            event,
            RunEvent::StatusTransition {
                to: AgentRunStatus::Executing | AgentRunStatus::Verifying,
                ..
            }
        )
    });
    let Some(RunEvent::StatusTransition { from, .. }) = body.events.last_mut() else {
        panic!("final event should be a status transition");
    };
    *from = AgentRunStatus::Planning;
    for (index, event) in body.events.iter_mut().enumerate() {
        set_event_sequence(event, (index + 1) as u64);
    }

    let error = seal_terminal_receipt(&request, &support, body)
        .expect_err("planning must not transition directly to completed");

    assert_eq!(error.code(), ContractErrorCode::InvalidTransition);
}

#[test]
fn tampered_receipt_body_does_not_verify() {
    let request = request();
    let support = ContractSupport::new([policy()]);
    let request_digest = validate_request(&request, &support).expect("request should be valid");
    let mut receipt = seal_terminal_receipt(&request, &support, completed_body(request_digest))
        .expect("completed receipt should seal");
    receipt.body.terminal_reason = "tampered".to_owned();

    let error =
        verify_terminal_receipt(&request, &support, &receipt).expect_err("tampering must fail");

    assert_eq!(error.code(), ContractErrorCode::ReceiptTampering);
    assert_eq!(error.path(), "receipt_digest");
}

#[test]
fn blocked_failed_and_interrupted_are_distinct_valid_outcomes() {
    let request = request();
    let support = ContractSupport::new([policy()]);
    let request_digest = validate_request(&request, &support).expect("request should be valid");

    for (status, reason) in [
        (AgentRunStatus::Blocked, "authority.required"),
        (AgentRunStatus::Failed, "execution.failed"),
        (AgentRunStatus::Interrupted, "operator.interrupted"),
    ] {
        let receipt = seal_terminal_receipt(
            &request,
            &support,
            terminal_body(request_digest.clone(), status, reason),
        )
        .expect("terminal outcome should seal");

        assert_eq!(receipt.body.terminal_status, status);
        assert_eq!(receipt.body.terminal_reason, reason);
    }
}

#[test]
fn raw_contracts_reject_unknown_terminal_and_evidence_types() {
    let request = request();
    let support = ContractSupport::new([policy()]);
    let request_digest = validate_request(&request, &support).expect("request should be valid");
    let receipt = seal_terminal_receipt(
        &request,
        &support,
        terminal_body(
            request_digest,
            AgentRunStatus::Blocked,
            "authority.required",
        ),
    )
    .expect("blocked receipt should seal");
    let mut receipt_json = serde_json::to_value(receipt).expect("receipt should serialize");
    receipt_json["body"]["terminal_status"] = serde_json::json!("paused");

    let terminal_error = parse_terminal_run_receipt_json(
        &serde_json::to_vec(&receipt_json).expect("receipt should serialize"),
    )
    .expect_err("unknown terminal status must fail");

    let mut request_json = serde_json::to_value(request).expect("request should serialize");
    request_json["required_verification"]["evidence_types"][0] = serde_json::json!("provider_log");
    let evidence_error = parse_agent_run_request_json(
        &serde_json::to_vec(&request_json).expect("request should serialize"),
    )
    .expect_err("unknown evidence type must fail");

    assert_eq!(terminal_error.code(), ContractErrorCode::InvalidJson);
    assert_eq!(evidence_error.code(), ContractErrorCode::InvalidJson);
}

#[test]
fn receipt_rejects_a_gap_in_event_sequence() {
    let request = request();
    let support = ContractSupport::new([policy()]);
    let request_digest = validate_request(&request, &support).expect("request should be valid");
    let mut body = completed_body(request_digest);
    set_event_sequence(&mut body.events[5], 7);

    let error =
        seal_terminal_receipt(&request, &support, body).expect_err("sequence gap must fail");

    assert_eq!(error.code(), ContractErrorCode::InvalidContract);
    assert_eq!(error.path(), "events[5].sequence");
}

#[test]
fn receipt_rejects_a_different_request_identity() {
    let request = request();
    let support = ContractSupport::new([policy()]);
    let request_digest = validate_request(&request, &support).expect("request should be valid");
    let mut body = completed_body(request_digest);
    body.request_id = "request-other".to_owned();

    let error =
        seal_terminal_receipt(&request, &support, body).expect_err("mismatched request must fail");

    assert_eq!(error.code(), ContractErrorCode::RequestMismatch);
    assert_eq!(error.path(), "body.request_id");
}

#[test]
fn completed_receipt_rejects_self_verification() {
    let request = request();
    let support = ContractSupport::new([policy()]);
    let request_digest = validate_request(&request, &support).expect("request should be valid");
    let mut body = completed_body(request_digest);
    let RunEvent::Verification {
        implementer_id,
        verifier_id,
        ..
    } = &mut body.events[9]
    else {
        panic!("ninth event should be verification");
    };
    *verifier_id = implementer_id.clone();

    let error =
        seal_terminal_receipt(&request, &support, body).expect_err("self-verification must fail");

    assert_eq!(error.code(), ContractErrorCode::InvalidContract);
    assert_eq!(error.path(), "events[9].verifier_id");
}

#[test]
fn completed_receipt_rejects_usage_over_budget() {
    let request = request();
    let support = ContractSupport::new([policy()]);
    let request_digest = validate_request(&request, &support).expect("request should be valid");
    let mut body = completed_body(request_digest);
    body.usage.cost_micros = request.resource_budget.max_cost_micros + 1;
    let RunEvent::Usage { usage, .. } = &mut body.events[7] else {
        panic!("eighth event should record usage");
    };
    usage.cost_micros = body.usage.cost_micros;

    let error = seal_terminal_receipt(&request, &support, body)
        .expect_err("over-budget completion must fail");

    assert_eq!(error.code(), ContractErrorCode::InvalidContract);
    assert_eq!(error.path(), "body.usage");
}

#[test]
fn completed_receipt_allows_a_request_without_approval_context() {
    let mut request = request();
    request.approval_context.clear();
    let support = ContractSupport::new([policy()]);
    let request_digest = validate_request(&request, &support).expect("request should be valid");
    let mut body = completed_body(request_digest);
    body.events
        .retain(|event| !matches!(event, RunEvent::ApprovalRecorded { .. }));
    for (index, event) in body.events.iter_mut().enumerate() {
        set_event_sequence(event, (index + 1) as u64);
    }

    let receipt = seal_terminal_receipt(&request, &support, body)
        .expect("approval-free request should be able to complete");

    verify_terminal_receipt(&request, &support, &receipt)
        .expect("approval-free completed receipt should verify");
}

#[test]
fn an_allow_decision_cannot_authorize_an_effect_after_its_subject_changes() {
    let request = request();
    let support = ContractSupport::new([policy()]);
    let request_digest = validate_request(&request, &support).expect("request should be valid");
    let mut body = completed_body(request_digest);
    body.events.insert(
        7,
        RunEvent::ToolExecution {
            sequence: 8,
            decision_id: "decision-tool-001".to_owned(),
            protected_effect_digest: digest('7'),
            action_digest: digest('6'),
            capability_digest: digest('c'),
            evidence: vec![evidence(EvidenceType::ToolExecution, '5')],
        },
    );
    for (index, event) in body.events.iter_mut().enumerate().skip(7) {
        set_event_sequence(event, (index + 1) as u64);
    }

    let error = seal_terminal_receipt(&request, &support, body)
        .expect_err("authority bound to the prior subject must be stale");

    assert_eq!(error.code(), ContractErrorCode::UnauthorizedEffect);
    assert_eq!(error.path(), "events[7].decision_id");
}

#[test]
fn stale_subject_decision_cannot_authorize_an_effect() {
    let request = request();
    let support = ContractSupport::new([policy()]);
    let request_digest = validate_request(&request, &support).expect("request should be valid");
    let mut body = completed_body(request_digest);
    let RunEvent::ProtectedEffectDecision { subject_digest, .. } = &mut body.events[4] else {
        panic!("fifth event should be the tool decision");
    };
    *subject_digest = digest('6');

    let error = seal_terminal_receipt(&request, &support, body)
        .expect_err("stale decision subject must fail");

    assert_eq!(error.code(), ContractErrorCode::InvalidContract);
    assert_eq!(error.path(), "events[4].subject_digest");
}

#[test]
fn mutation_requires_artifact_evidence() {
    let request = request();
    let support = ContractSupport::new([policy()]);
    let request_digest = validate_request(&request, &support).expect("request should be valid");
    let mut body = completed_body(request_digest);
    let RunEvent::Mutation { evidence, .. } = &mut body.events[6] else {
        panic!("seventh event should be the mutation");
    };
    evidence[0].evidence_type = EvidenceType::CommandOutput;

    let error = seal_terminal_receipt(&request, &support, body)
        .expect_err("mutation without artifact evidence must fail");

    assert_eq!(error.code(), ContractErrorCode::InvalidContract);
    assert_eq!(error.path(), "events[6].evidence");
}

#[test]
fn canonical_request_is_independent_of_json_property_order() {
    let request = request();
    let canonical = canonical_request_bytes(&request).expect("request should canonicalize");
    let value: serde_json::Value =
        serde_json::from_slice(&canonical).expect("canonical request should parse");
    let object = value.as_object().expect("request should be an object");
    let reversed_fields = object
        .iter()
        .rev()
        .map(|(key, value)| {
            format!(
                "{}:{}",
                serde_json::to_string(key).expect("key should serialize"),
                serde_json::to_string(value).expect("value should serialize")
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let reordered = format!("{{{reversed_fields}}}");
    let parsed =
        parse_agent_run_request_json(reordered.as_bytes()).expect("reordered request should parse");

    assert_eq!(
        canonical_request_bytes(&parsed).expect("reordered request should canonicalize"),
        canonical
    );
}
