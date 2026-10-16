use base64::{Engine, engine::general_purpose::STANDARD};
use gaap::contracts::{
    self, AgentRunStatus, ContractSupport, EffectEvidenceReference, EffectEvidenceType,
    EffectExecutionStatus, EvidenceReference, EvidenceType, ResourceUsage, RunEvent,
};
use gaap_attestation::{
    dsse,
    ed25519::{Ed25519Signer, Ed25519Verifier},
    snapshot::content_digest,
    statement::{self, Statement},
};
use std::{collections::BTreeMap, fs, path::PathBuf};

fn fixture(case: &str) -> (dsse::Envelope, Statement, BTreeMap<String, Vec<u8>>) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/goose")
        .join(case);
    let envelope = dsse::parse_envelope(&fs::read(root.join("envelope.json")).unwrap()).unwrap();
    let statement = serde_json::from_slice(&STANDARD.decode(&envelope.payload).unwrap()).unwrap();
    let blobs = fs::read_dir(root.join("blobs"))
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            (
                format!("sha256:{}", path.file_name().unwrap().to_str().unwrap()),
                fs::read(path).unwrap(),
            )
        })
        .collect();
    (envelope, statement, blobs)
}
fn signer() -> Ed25519Signer {
    Ed25519Signer::from_seed("gaap-goose-public-fixture-only".into(), [7; 32]).unwrap()
}
fn verifier() -> Ed25519Verifier {
    Ed25519Verifier::new([(
        "gaap-goose-public-fixture-only".into(),
        signer().public_key(),
    )])
    .unwrap()
}
fn signed_unchecked(statement: &Statement) -> dsse::Envelope {
    dsse::sign(
        statement::PAYLOAD_TYPE,
        &serde_jcs::to_vec(statement).unwrap(),
        &mut signer(),
    )
    .unwrap()
}
#[test]
fn real_goose_run_verifies_offline_without_live_process_state() {
    let (envelope, original, mut blobs) = fixture("allow");
    let support = ContractSupport::new(original.predicate.request.policies.clone());
    let verified = statement::verify(&envelope, &support, &mut blobs, &mut verifier()).unwrap();
    assert_eq!(
        verified.statement().predicate.receipt.body.terminal_status,
        AgentRunStatus::Completed
    );
    assert_eq!(verified.statement().predicate.effects.len(), 3);
    assert!(!verified.unresolved_references().is_empty()); // Hash-only metadata stays disclosed.
    assert_eq!(
        verified.signed_payload(),
        STANDARD.decode(envelope.payload).unwrap()
    );
    assert!(!verified.keys().is_empty());
}
#[test]
fn tampered_authenticated_fields_fail_before_predicate_interpretation() {
    let (envelope, original, mut blobs) = fixture("allow");
    let support = ContractSupport::new(original.predicate.request.policies.clone());
    let original: serde_json::Value = serde_json::to_value(original).unwrap();
    for pointer in [
        "/subject/0/name",
        "/predicate/request/run_id",
        "/predicate/effects/0/request/input_digest",
        "/predicate/effects/0/result/body/evidence/0/digest",
        "/predicate/receipt/body/usage/tool_calls",
        "/predicateType",
    ] {
        let mut value = original.clone();
        *value.pointer_mut(pointer).unwrap() = serde_json::json!("tampered");
        let mut changed = envelope.clone();
        changed.payload = STANDARD.encode(serde_json::to_vec(&value).unwrap());
        assert_eq!(
            statement::verify(&changed, &support, &mut blobs, &mut verifier())
                .err()
                .unwrap()
                .code,
            "ed25519.signature_mismatch",
            "{pointer}"
        );
    }
}
#[test]
fn even_trusted_signatures_cant_hide_inconsistent_contract_bindings() {
    let (_, original, mut blobs) = fixture("allow");
    let support = ContractSupport::new(original.predicate.request.policies.clone());
    for case in [
        "subject",
        "request",
        "result",
        "order",
        "missing",
        "usage",
        "version",
        "observed",
        "snapshot",
        "implementation",
    ] {
        let mut changed = original.clone();
        match case {
            "subject" => changed.subject[0].name.push_str("-other"),
            "request" => changed
                .predicate
                .request
                .task
                .instructions
                .push_str(" altered"),
            "result" => changed.predicate.effects[0].result.body.usage.tool_calls += 1,
            "order" => changed.predicate.effects.swap(0, 1),
            "missing" => {
                changed.predicate.effects.pop();
            }
            "usage" => changed.predicate.receipt.body.usage.tool_calls = 0,
            "version" => changed.predicate_type.push_str("/future"),
            "observed" => {
                changed.predicate.observed_subject.digest =
                    changed.predicate.request.subject.digest.clone();
                changed = statement::new_statement(changed.predicate);
            }
            "snapshot" => changed.predicate.snapshot_digests.clear(),
            "implementation" => changed.predicate.implementation.digest = "unknown".into(),
            _ => unreachable!(),
        }
        assert!(
            statement::verify(
                &signed_unchecked(&changed),
                &support,
                &mut blobs,
                &mut verifier()
            )
            .is_err(),
            "{case}"
        );
    }
}
#[test]
fn content_bytes_missing_files_and_unsupported_policy_fail_closed() {
    let (envelope, original, blobs) = fixture("allow");
    let support = ContractSupport::new(original.predicate.request.policies.clone());
    let mut tampered = blobs.clone();
    tampered.values_mut().next().unwrap().push(0);
    assert_eq!(
        statement::verify(&envelope, &support, &mut tampered, &mut verifier())
            .err()
            .unwrap()
            .code,
        "evidence.digest_mismatch"
    );
    let mut missing = blobs.clone();
    missing.remove(&original.predicate.request.subject.digest);
    assert!(statement::verify(&envelope, &support, &mut missing, &mut verifier()).is_err());
    assert!(
        statement::verify(
            &envelope,
            &ContractSupport::new([]),
            &mut blobs.clone(),
            &mut verifier()
        )
        .is_err()
    );
}
#[test]
fn unsigned_and_untrusted_envelopes_cant_become_verified_attestations() {
    let (mut envelope, original, mut blobs) = fixture("allow");
    let support = ContractSupport::new(original.predicate.request.policies.clone());
    envelope.signatures.clear();
    assert_eq!(
        statement::verify(&envelope, &support, &mut blobs, &mut verifier())
            .err()
            .unwrap()
            .code,
        "dsse.unsigned"
    );
    let mut other = Ed25519Signer::from_seed("untrusted-fixture".into(), [8; 32]).unwrap();
    envelope = dsse::sign(
        statement::PAYLOAD_TYPE,
        &serde_jcs::to_vec(&original).unwrap(),
        &mut other,
    )
    .unwrap();
    assert_eq!(
        statement::verify(&envelope, &support, &mut blobs, &mut verifier())
            .err()
            .unwrap()
            .code,
        "ed25519.untrusted_key"
    );
}
#[test]
fn blocked_and_stale_runs_keep_their_status_after_signing() {
    for case in ["ask", "bad_patch", "unreported"] {
        let (envelope, original, mut blobs) = fixture(case);
        let support = ContractSupport::new(original.predicate.request.policies.clone());
        let verified = statement::verify(&envelope, &support, &mut blobs, &mut verifier()).unwrap();
        assert_eq!(
            verified.statement().predicate.receipt.body.terminal_status,
            AgentRunStatus::Blocked
        );
        if case == "unreported" {
            assert_ne!(
                verified.statement().predicate.observed_subject.digest,
                verified
                    .statement()
                    .predicate
                    .receipt
                    .body
                    .resulting_subject_digest
            );
        }
    }
}

/// Deliberately synthetic unknown/failure/interruption variants of a retained real run.
/// They test semantic preservation; they aren't claims of additional Goose executions.
fn synthetic_non_success(status: EffectExecutionStatus) -> (Statement, BTreeMap<String, Vec<u8>>) {
    let (_, mut statement, mut blobs) = fixture("allow");
    statement.predicate.effects.truncate(1);
    let request = statement.predicate.request.clone();
    let support = ContractSupport::new(request.policies.clone());
    let effect = &mut statement.predicate.effects[0];
    let old_result = effect.result.result_digest.clone();
    let mut body = effect.result.body.clone();
    body.execution_status = status;
    body.reason = Some("synthetic.non_success_fixture".into());
    body.observed_post_effect_subject = if status == EffectExecutionStatus::UnknownOutcome {
        None
    } else {
        Some(effect.request.subject.clone())
    };
    body.exit = None;
    let evidence_type = match status {
        EffectExecutionStatus::UnknownOutcome => EffectEvidenceType::UnknownOutcome,
        EffectExecutionStatus::Interrupted => EffectEvidenceType::Interruption,
        EffectExecutionStatus::Failed => EffectEvidenceType::Failure,
        _ => unreachable!(),
    };
    let bytes = b"synthetic non-success fixture".to_vec();
    let digest = content_digest(&bytes);
    blobs.insert(digest.clone(), bytes);
    body.evidence.push(EffectEvidenceReference {
        evidence_type,
        digest: digest.clone(),
        locator: None,
    });
    effect.result =
        contracts::seal_protected_effect_result(&request, &support, &effect.request, body).unwrap();
    let receipt = &mut statement.predicate.receipt.body;
    let execution = receipt
        .events
        .iter()
        .position(|event| matches!(event, RunEvent::ToolExecution { .. }))
        .unwrap();
    receipt.events.truncate(execution + 1);
    for event in &mut receipt.events {
        if let RunEvent::ToolExecution { evidence, .. } = event {
            for reference in evidence {
                if reference.digest == old_result {
                    reference.digest = effect.result.result_digest.clone();
                }
            }
        }
    }
    let mut sequence = receipt.events.len() as u64 + 1;
    if status != EffectExecutionStatus::UnknownOutcome {
        // Known mutation outcome can report an unchanged subject while retaining evidence.
        receipt.events.push(RunEvent::Mutation {
            sequence,
            decision_id: format!("decision-1-workflow-event-{}", execution),
            protected_effect_digest: effect.result.body.effect_request_digest.clone(),
            before_subject_digest: request.subject.digest.clone(),
            after_subject_digest: request.subject.digest.clone(),
            evidence: vec![EvidenceReference {
                evidence_type: EvidenceType::Artifact,
                digest: request.subject.digest.clone(),
                locator: None,
            }],
        });
        // The original workflow decision identity is retained, independent of fixture lengths.
        let id = receipt
            .events
            .iter()
            .find_map(|event| match event {
                RunEvent::ProtectedEffectDecision {
                    decision_id,
                    gate: gaap::Gate::Workflow,
                    ..
                } => Some(decision_id.clone()),
                _ => None,
            })
            .unwrap();
        if let Some(RunEvent::Mutation { decision_id, .. }) = receipt.events.last_mut() {
            *decision_id = id;
        }
        sequence += 1;
    }
    let elapsed = receipt
        .events
        .iter()
        .filter_map(|event| match event {
            RunEvent::Usage { usage, .. } => Some(usage.elapsed_ms),
            _ => None,
        })
        .next_back()
        .unwrap();
    let usage = ResourceUsage {
        cost_micros: 0,
        elapsed_ms: elapsed,
        model_tokens: 0,
        tool_calls: 1,
    };
    receipt.events.push(RunEvent::Usage {
        sequence,
        usage: usage.clone(),
    });
    sequence += 1;
    let terminal = if status == EffectExecutionStatus::Interrupted {
        AgentRunStatus::Interrupted
    } else {
        AgentRunStatus::Failed
    };
    if terminal == AgentRunStatus::Interrupted {
        receipt.events.push(RunEvent::Interruption {
            sequence,
            actor_id: Some("synthetic-fixture".into()),
            reason: "synthetic.interrupted".into(),
            evidence: EvidenceReference {
                evidence_type: EvidenceType::Interruption,
                digest,
                locator: None,
            },
        });
        sequence += 1;
    }
    receipt.events.push(RunEvent::StatusTransition {
        sequence,
        from: AgentRunStatus::Executing,
        to: terminal,
        reason: Some("synthetic.non_success".into()),
    });
    receipt.usage = usage;
    receipt.terminal_status = terminal;
    receipt.terminal_reason = "synthetic.non_success".into();
    receipt.resulting_subject_digest = request.subject.digest.clone();
    statement.predicate.receipt =
        contracts::seal_terminal_receipt(&request, &support, receipt.clone()).unwrap();
    statement.predicate.observed_subject = request.subject;
    statement.predicate.blob_digests = blobs.keys().cloned().collect();
    statement = statement::new_statement(statement.predicate);
    (statement, blobs)
}
#[test]
fn failed_interrupted_and_unknown_outcomes_never_become_successful_evidence() {
    for outcome in [
        EffectExecutionStatus::Failed,
        EffectExecutionStatus::Interrupted,
        EffectExecutionStatus::UnknownOutcome,
    ] {
        let (original, mut blobs) = synthetic_non_success(outcome);
        let support = ContractSupport::new(original.predicate.request.policies.clone());
        let envelope = statement::sign(&original, &support, &mut blobs, &mut signer()).unwrap();
        let verified = statement::verify(&envelope, &support, &mut blobs, &mut verifier()).unwrap();
        assert_ne!(
            verified.statement().predicate.receipt.body.terminal_status,
            AgentRunStatus::Completed
        );
        assert_eq!(
            verified.statement().predicate.effects[0]
                .result
                .body
                .execution_status,
            outcome
        );
    }
}

#[test]
fn resealed_action_identity_tampering_still_fails_the_cross_ledger_binding() {
    let (_, mut changed, mut blobs) = fixture("allow");
    let support = ContractSupport::new(changed.predicate.request.policies.clone());
    for event in &mut changed.predicate.receipt.body.events {
        if let RunEvent::ToolExecution { action_digest, .. } = event {
            *action_digest = format!("sha256:{}", "0".repeat(64));
            break;
        }
    }
    changed.predicate.receipt = contracts::seal_terminal_receipt(
        &changed.predicate.request,
        &support,
        changed.predicate.receipt.body.clone(),
    )
    .unwrap();
    let envelope = signed_unchecked(&changed);
    assert_eq!(
        statement::verify(&envelope, &support, &mut blobs, &mut verifier())
            .err()
            .unwrap()
            .code,
        "attestation.result_evidence_mismatch"
    );
}

#[test]
fn an_explicit_interruption_cant_be_resealed_as_completion() {
    let (_, mut changed, mut blobs) = fixture("allow");
    let support = ContractSupport::new(changed.predicate.request.policies.clone());
    let events = &mut changed.predicate.receipt.body.events;
    let index = events.len() - 1;
    events.insert(
        index,
        RunEvent::Interruption {
            sequence: index as u64 + 1,
            actor_id: Some("fixture-operator".into()),
            reason: "operator.cancelled".into(),
            evidence: EvidenceReference {
                evidence_type: EvidenceType::Interruption,
                digest: content_digest(b"operator cancelled"),
                locator: None,
            },
        },
    );
    if let RunEvent::StatusTransition { sequence, .. } = events.last_mut().unwrap() {
        *sequence += 1;
    }
    // The frozen core accepts this shape; the adapter must reject the contradiction.
    changed.predicate.receipt = contracts::seal_terminal_receipt(
        &changed.predicate.request,
        &support,
        changed.predicate.receipt.body.clone(),
    )
    .unwrap();
    assert_eq!(
        statement::verify(
            &signed_unchecked(&changed),
            &support,
            &mut blobs,
            &mut verifier()
        )
        .err()
        .unwrap()
        .code,
        "attestation.interruption_status_mismatch"
    );
}
#[test]
fn attempted_effects_cant_claim_zero_known_tool_attempts() {
    let (_, mut changed, mut blobs) = fixture("allow");
    let support = ContractSupport::new(changed.predicate.request.policies.clone());
    for effect in &mut changed.predicate.effects {
        let old = effect.result.result_digest.clone();
        let mut body = effect.result.body.clone();
        body.usage.tool_calls = 0;
        effect.result = contracts::seal_protected_effect_result(
            &changed.predicate.request,
            &support,
            &effect.request,
            body,
        )
        .unwrap();
        for event in &mut changed.predicate.receipt.body.events {
            if let RunEvent::ToolExecution { evidence, .. } = event {
                for reference in evidence {
                    if reference.digest == old {
                        reference.digest = effect.result.result_digest.clone();
                    }
                }
            }
        }
    }
    for event in &mut changed.predicate.receipt.body.events {
        if let RunEvent::Usage { usage, .. } = event {
            usage.tool_calls = 0;
        }
    }
    changed.predicate.receipt.body.usage.tool_calls = 0;
    changed.predicate.receipt = contracts::seal_terminal_receipt(
        &changed.predicate.request,
        &support,
        changed.predicate.receipt.body.clone(),
    )
    .unwrap();
    assert_eq!(
        statement::verify(
            &signed_unchecked(&changed),
            &support,
            &mut blobs,
            &mut verifier()
        )
        .err()
        .unwrap()
        .code,
        "attestation.missing_known_attempt_usage"
    );
}

#[test]
fn resealed_completed_effect_sequences_must_start_at_one_without_gaps() {
    let (_, mut changed, mut blobs) = fixture("allow");
    let support = ContractSupport::new(changed.predicate.request.policies.clone());
    for effect in &mut changed.predicate.effects {
        let old_request = effect.result.body.effect_request_digest.clone();
        let old_result = effect.result.result_digest.clone();
        effect.request.effect_sequence += 1;
        let digest = contracts::validate_protected_effect_request(
            &changed.predicate.request,
            &support,
            &effect.request,
        )
        .unwrap();
        let mut body = effect.result.body.clone();
        body.effect_sequence = effect.request.effect_sequence;
        body.effect_request_digest = digest.clone();
        body.decision.effect_request_digest = digest.clone();
        effect.result = contracts::seal_protected_effect_result(
            &changed.predicate.request,
            &support,
            &effect.request,
            body,
        )
        .unwrap();
        for event in &mut changed.predicate.receipt.body.events {
            match event {
                RunEvent::ProtectedEffectDecision {
                    protected_effect_digest,
                    ..
                }
                | RunEvent::Mutation {
                    protected_effect_digest,
                    ..
                } if *protected_effect_digest == old_request => {
                    *protected_effect_digest = digest.clone()
                }
                RunEvent::ToolExecution {
                    protected_effect_digest,
                    action_digest,
                    evidence,
                    ..
                } if *protected_effect_digest == old_request => {
                    *protected_effect_digest = digest.clone();
                    *action_digest = digest.clone();
                    for reference in evidence {
                        if reference.digest == old_result {
                            reference.digest = effect.result.result_digest.clone();
                        }
                    }
                }
                _ => (),
            }
        }
    }
    changed.predicate.receipt = contracts::seal_terminal_receipt(
        &changed.predicate.request,
        &support,
        changed.predicate.receipt.body.clone(),
    )
    .unwrap();
    assert_eq!(
        statement::verify(
            &signed_unchecked(&changed),
            &support,
            &mut blobs,
            &mut verifier()
        )
        .err()
        .unwrap()
        .code,
        "attestation.effect_order_or_subject"
    );
}
#[test]
fn real_failed_and_cancelled_goose_fixtures_keep_their_terminal_status() {
    for (case, status) in [
        ("report_only", AgentRunStatus::Failed),
        ("cancel_before_dispatch", AgentRunStatus::Interrupted),
    ] {
        let (envelope, original, mut blobs) = fixture(case);
        let support = ContractSupport::new(original.predicate.request.policies.clone());
        let verified = statement::verify(&envelope, &support, &mut blobs, &mut verifier()).unwrap();
        assert_eq!(
            verified.statement().predicate.receipt.body.terminal_status,
            status
        );
        assert_eq!(
            verified.statement().predicate.receipt.body.usage.tool_calls,
            0
        );
    }
}

#[test]
fn every_dispatch_gate_must_allow_before_the_execution() {
    let (_, original, mut blobs) = fixture("allow");
    let support = ContractSupport::new(original.predicate.request.policies.clone());
    for gate in [gaap::Gate::ToolTrust, gaap::Gate::Runtime] {
        for mode in ["block", "ask", "missing", "late"] {
            let mut changed = original.clone();
            let events = &mut changed.predicate.receipt.body.events;
            let index=events.iter().position(|event|matches!(event,RunEvent::ProtectedEffectDecision{gate:event_gate,..} if *event_gate==gate)).unwrap();
            if matches!(mode, "block" | "ask") {
                if let RunEvent::ProtectedEffectDecision { decision, .. } = &mut events[index] {
                    decision.outcome = if mode == "ask" {
                        gaap::Outcome::Ask
                    } else {
                        gaap::Outcome::Block
                    };
                    decision.code = "synthetic.gate_refusal".into();
                }
            } else {
                let decision = events.remove(index);
                if mode == "late" {
                    let execution = events
                        .iter()
                        .position(|event| matches!(event, RunEvent::ToolExecution { .. }))
                        .unwrap();
                    events.insert(execution + 1, decision);
                }
                let mut value = serde_json::to_value(&events).unwrap();
                for (index, event) in value.as_array_mut().unwrap().iter_mut().enumerate() {
                    event["sequence"] = serde_json::json!(index + 1);
                }
                *events = serde_json::from_value(value).unwrap();
            }
            changed.predicate.receipt = contracts::seal_terminal_receipt(
                &changed.predicate.request,
                &support,
                changed.predicate.receipt.body.clone(),
            )
            .unwrap();
            assert_eq!(
                statement::verify(
                    &signed_unchecked(&changed),
                    &support,
                    &mut blobs,
                    &mut verifier()
                )
                .err()
                .expect("missing, late or refusing gates must stop dispatch")
                .code,
                "attestation.gates_not_admitted"
            );
        }
    }
}

#[test]
fn admission_decisions_for_an_earlier_subject_cant_authorize_later_dispatch() {
    let (_, mut changed, mut blobs) = fixture("allow");
    let support = ContractSupport::new(changed.predicate.request.policies.clone());
    let second_digest = changed.predicate.effects[1]
        .result
        .body
        .effect_request_digest
        .clone();
    let initial_subject = changed.predicate.request.subject.digest.clone();
    let events = &mut changed.predicate.receipt.body.events;
    let mut stale = Vec::new();
    events.retain_mut(|event| {
        if let RunEvent::ProtectedEffectDecision {
            protected_effect_digest,
            subject_digest,
            gate,
            ..
        } = event
            && *protected_effect_digest == second_digest
            && matches!(*gate, gaap::Gate::ToolTrust | gaap::Gate::Runtime)
        {
            *subject_digest = initial_subject.clone();
            stale.push(event.clone());
            return false;
        }
        true
    });
    let first_execution = events
        .iter()
        .position(|event| matches!(event, RunEvent::ToolExecution { .. }))
        .unwrap();
    events.splice(first_execution..first_execution, stale);
    let mut value = serde_json::to_value(&events).unwrap();
    for (index, event) in value.as_array_mut().unwrap().iter_mut().enumerate() {
        event["sequence"] = serde_json::json!(index + 1);
    }
    *events = serde_json::from_value(value).unwrap();
    changed.predicate.receipt = contracts::seal_terminal_receipt(
        &changed.predicate.request,
        &support,
        changed.predicate.receipt.body.clone(),
    )
    .unwrap();
    assert_eq!(
        statement::verify(
            &signed_unchecked(&changed),
            &support,
            &mut blobs,
            &mut verifier()
        )
        .err()
        .expect("old subject decisions must not admit a later subject")
        .code,
        "attestation.gates_not_admitted"
    );
}
