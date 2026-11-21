use std::slice;

use serde_json::{Value, json};

use super::*;

const WORKSPACE_ID: &str = "11111111111111111111111111111111";
const STORE_ID: &str = "22222222222222222222222222222222";
const ATTEMPT: &str = "33333333333333333333333333333333";

fn hash(value: &Value) -> String {
    digest(&canonical(value).unwrap())
}

fn usage(calls: u64) -> Value {
    json!({"cost_micros":0,"elapsed_ms":0,"model_tokens":0,"tool_calls":calls})
}

struct Sample {
    workspace: Identity,
    profile: HistoricalProfile,
    archive: EvidenceArchive,
    row: Value,
}

impl Sample {
    fn new() -> Self {
        let mut run: Value = serde_json::from_str(include_str!(
            "../../../../examples/contracts/v0.1.0/agent-run-request.json"
        ))
        .unwrap();
        let mut effect: Value = serde_json::from_str(include_str!(
            "../../../../examples/contracts/protected-effect/v0.1.0/protected-effect-request.json"
        ))
        .unwrap();
        let proposal = json!({"path":"example.txt","content":"hello\n","plan":run["task"]["instructions"],"base_digest":run["subject"]["digest"]});
        run["subject"]["locator"] = json!("/workspace");
        run["approval_context"] = json!([]);
        effect["agent_run_request_digest"] = json!(hash(&run));
        effect["subject"] = run["subject"].clone();
        effect["input_digest"] = json!(hash(&proposal));
        effect["normalized_operation"] = json!("filesystem.write");
        effect["operation_family"] = json!("filesystem");
        effect["approval_context"] = json!([]);
        effect["input_metadata"] = json!([{"name":"path","value":"example.txt"}]);
        effect["expected_effect_class"] = json!("mutation");
        effect["requested_scopes"] = json!([{"scope_type":"filesystem","root":"/workspace","access":["modify"],"recursive":false}]);
        let profile: HistoricalProfile = serde_json::from_value(json!({
            "format":"gaap.local-file-history/0.1","policies":run["policies"],
            "capability":effect["capability"],"tool_schema_digest":effect["tool_schema_digest"],
            "sandbox":effect["sandbox_profile"],"executor":{"name":"writer","version":"1","digest":digest(b"writer")},
            "implementer_id":"implementer","verifier_id":"verifier","grant_observer":"grant-host",
            "effect_observer":"file-host","require_grant":false,
        })).unwrap();
        let binding = hash(&json!([
            WORKSPACE_ID,
            STORE_ID,
            digest(&canonical(&profile).unwrap()),
            "alice",
            "k",
            ATTEMPT,
            hash(&proposal),
            hash(&run),
            hash(&effect)
        ]));
        let intent = json!({"tag":"intent","binding":binding,"payload":{"run_request_digest":hash(&run),
            "effect_request_digest":hash(&effect),"proposed_artifact_digest":digest(b"hello\n")}});
        let row = json!({"principal":"alice","key":"k","attempt_id":ATTEMPT,"proposal":proposal,
            "proposal_digest":hash(&proposal),"run_request":run,"effect_request":effect,"facts":[intent]});
        Self {
            workspace: Identity {
                path: "/workspace".into(),
                dev: "1".into(),
                ino: "2".into(),
                uid: 42,
                mode: 0o700,
            },
            profile,
            archive: EvidenceArchive { entries: vec![] },
            row,
        }
    }

    fn lookup(
        &self,
        records: &[Value],
        principal: &str,
        proposal: Option<&Value>,
    ) -> Result<Lookup> {
        let profile_digest = digest(&canonical(&self.profile)?);
        let context = HistoryContext {
            workspace_id: WORKSPACE_ID,
            store_id: STORE_ID,
            profile_digest: &profile_digest,
            workspace: &self.workspace,
            profile: &self.profile,
            archive: &self.archive,
        };
        lookup(records, &context, principal, "k", proposal)
    }

    fn own(&self) -> Result<Lookup> {
        self.lookup(slice::from_ref(&self.row), "alice", None)
    }

    fn append(&mut self, tag: &str, payload: Value) {
        let binding = self.row["facts"][0]["binding"].clone();
        self.row["facts"]
            .as_array_mut()
            .unwrap()
            .push(json!({"tag":tag,"binding":binding,"payload":payload}));
    }

    fn dispatch(&mut self) {
        let run = &self.row["run_request"];
        let effect_digest = hash(&self.row["effect_request"]);
        let mut decisions = vec![];
        for (ix, gate) in ["permission", "tool_trust", "runtime", "workflow"]
            .iter()
            .enumerate()
        {
            decisions.push(json!({"event_type":"protected_effect_decision","sequence":ix+3,
                "decision_id":format!("gate-{ix}"),"gate":gate,"protected_effect_digest":effect_digest,
                "subject_digest":run["subject"]["digest"],"decision":{"outcome":"allow",
                "code":if *gate == "workflow" {"workflow.mutation_authorized"} else {"allowed"},"effects":[]}}));
        }
        let before = json!({"subject":run["subject"],"file":{"path":"/workspace/example.txt","dev":"1",
            "ino":"4","uid":42,"mode":384},"artifact_digest":digest(b"previous")});
        self.append(
            "dispatch",
            json!({"before":before,"executor":self.profile.executor,"decisions":decisions,
            "checked_at_ms":1,"checks":[],"reserved":usage(1),"usage_floor":usage(0)}),
        );
    }

    fn evidence(&mut self, locator: &str, blob: &Value) -> Value {
        let bytes = canonical(blob).unwrap();
        let bytes_hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        self.archive.entries.push(ArchiveEntry {
            actor_id: self.profile.effect_observer.clone(),
            locator: locator.into(),
            bytes_hex,
        });
        json!({"actor_id":self.profile.effect_observer,"evidence":{"evidence_type":"artifact",
            "locator":locator,"digest":digest(&bytes)}})
    }

    fn observe_negative(&mut self, entire_interval: bool) {
        let witness = json!({"kind":"observation","binding":self.row["facts"][0]["binding"],"actor_id":"file-host",
            "before":self.row["facts"][1]["payload"]["before"],"after":null,"disposition":"stopped_before_rename",
            "covers_through_stop":entire_interval});
        let source = self.evidence("opaque:one", &witness);
        self.append("observed",json!({"after":null,"source":source,"disposition":"stopped_before_rename","usage_floor":usage(1)}));
    }
}

#[test]
fn scoped_lookup_returns_a_prefix_conflict_or_absence() {
    let s = Sample::new();
    assert_eq!(s.own().unwrap().status, "partial");
    let original = &s.row["proposal"];
    assert_eq!(
        s.lookup(slice::from_ref(&s.row), "alice", Some(original))
            .unwrap()
            .attempt,
        Some(s.row.clone())
    );
    let mut changed = original.clone();
    changed["content"] = json!("hello");
    assert_eq!(
        s.lookup(slice::from_ref(&s.row), "alice", Some(&changed))
            .unwrap()
            .status,
        "conflict"
    );
    let other = s.lookup(slice::from_ref(&s.row), "bob", None).unwrap();
    assert_eq!(other.status, "absent");
    assert!(other.attempt.is_none());
}

#[test]
fn validates_other_principals_and_rejects_reused_identities() {
    let s = Sample::new();
    let mut edited = s.row.clone();
    edited["principal"] = json!("bob");
    edited["facts"][0]["payload"]["effect_request_digest"] = json!(digest(b"different"));
    assert!(s.lookup(slice::from_ref(&edited), "alice", None).is_err());
    assert!(
        s.lookup(&[s.row.clone(), s.row.clone()], "bob", None)
            .is_err()
    );
}

#[test]
fn dispatch_and_unresolved_observation_hold_every_namespace() {
    let mut s = Sample::new();
    s.dispatch();
    assert!(s.own().unwrap().workspace_hold);
    s.append(
        "observed",
        json!({"after":null,"source":null,"disposition":"unresolved","usage_floor":usage(0)}),
    );
    let result = s.lookup(slice::from_ref(&s.row), "bob", None).unwrap();
    assert!(result.workspace_hold && result.attempt.is_none());
    assert!(
        s.lookup(&[s.row.clone(), Sample::new().row], "bob", None)
            .is_err()
    );
}

#[test]
fn negative_observation_requires_every_possible_commit_through_stop() {
    let mut partial = Sample::new();
    partial.dispatch();
    partial.observe_negative(false);
    assert!(partial.own().is_err());
    let mut complete = Sample::new();
    complete.dispatch();
    complete.observe_negative(true);
    assert!(!complete.own().unwrap().workspace_hold);
    complete.archive.entries[0].bytes_hex.push_str("00");
    assert!(complete.own().is_err());
}

#[test]
fn rejects_an_unknown_tag_and_an_omitted_nullable_observation_field() {
    let mut s = Sample::new();
    s.dispatch();
    s.append(
        "observed",
        json!({"source":null,"disposition":"unresolved","usage_floor":usage(0)}),
    );
    assert!(s.own().is_err());
    s.row["facts"][2]["tag"] = json!("reconciled");
    assert!(s.own().is_err());
}

#[test]
fn rejects_underfunded_gates_and_wrong_operation_order() {
    let mut s = Sample::new();
    s.dispatch();
    let valid = s.row.clone();
    s.row["facts"][1]["payload"]["reserved"] = usage(101);
    assert!(s.own().is_err());
    s.row = valid;
    s.row["facts"][1]["payload"]["decisions"]
        .as_array_mut()
        .unwrap()
        .swap(0, 1);
    assert!(s.own().is_err());
}

#[test]
fn lookup_rejects_valid_individual_frozen_objects_from_another_admission() {
    let mut s = Sample::new();
    let receipt: Value = serde_json::from_str(include_str!(
        "../../../../examples/contracts/v0.1.0/completed.json"
    ))
    .unwrap();
    let result: Value = serde_json::from_str(include_str!(
        "../../../../examples/contracts/protected-effect/v0.1.0/completed.json"
    ))
    .unwrap();
    s.append("published", json!({"receipt":receipt,"result":result}));
    assert!(s.own().is_err());
}

#[test]
fn workflow_decision_ids_do_not_select_completion_or_change_its_digest_domain() {
    use gaap::contracts::{
        AgentRunRequest, AgentRunStatus, RunEvent, TerminalRunReceipt, seal_terminal_receipt,
        verify_terminal_receipt,
    };
    use gaap::{Gate, Outcome};

    let original: TerminalRunReceipt = serde_json::from_str(include_str!(
        "../../../../examples/contracts/v0.1.0/completed.json"
    ))
    .unwrap();
    let request: AgentRunRequest = serde_json::from_str(include_str!(
        "../../../../examples/contracts/v0.1.0/agent-run-request.json"
    ))
    .unwrap();
    let support = ContractSupport::new(request.policies.clone());
    let mut body = original.body;
    let mut effect_hash = String::new();
    for event in &mut body.events {
        match event {
            RunEvent::ProtectedEffectDecision {
                gate: Gate::Workflow,
                protected_effect_digest,
                decision_id,
                decision,
                ..
            } if decision.code == "workflow.mutation_authorized" => {
                effect_hash.clone_from(protected_effect_digest);
                *decision_id = "decision-completion".into();
            }
            RunEvent::Mutation { decision_id, .. } => *decision_id = "decision-completion".into(),
            RunEvent::ProtectedEffectDecision {
                gate: Gate::Workflow,
                decision_id,
                decision,
                ..
            } if decision.code == "workflow.completion_authorized" => {
                *decision_id = "opaque-final-id".into()
            }
            _ => {}
        }
    }
    let completed = seal_terminal_receipt(&request, &support, body.clone()).unwrap();
    verify_terminal_receipt(&request, &support, &completed).unwrap();
    assert!(super::publication::validate_decision_targets(&completed.body, &effect_hash).unwrap());

    let denial = "workflow.completion_refused";
    body.terminal_status = AgentRunStatus::Blocked;
    body.terminal_reason = denial.into();
    for event in &mut body.events {
        match event {
            RunEvent::ProtectedEffectDecision { decision, .. }
                if decision.code == "workflow.completion_authorized" =>
            {
                decision.outcome = Outcome::Block;
                decision.code = denial.into();
                decision.effects.clear();
            }
            RunEvent::StatusTransition { to, reason, .. } if *to == AgentRunStatus::Completed => {
                *to = AgentRunStatus::Blocked;
                *reason = Some(denial.into());
            }
            _ => {}
        }
    }
    let refused = seal_terminal_receipt(&request, &support, body.clone()).unwrap();
    verify_terminal_receipt(&request, &support, &refused).unwrap();
    assert!(super::publication::validate_decision_targets(&refused.body, &effect_hash).unwrap());
    let event = body
        .events
        .iter_mut()
        .find(|event| {
            matches!(event,
        RunEvent::ProtectedEffectDecision { decision, .. } if decision.code == denial)
        })
        .unwrap();
    if let RunEvent::ProtectedEffectDecision {
        protected_effect_digest,
        ..
    } = event
    {
        protected_effect_digest.clone_from(&effect_hash);
    }
    let wrong_target = seal_terminal_receipt(&request, &support, body).unwrap();
    assert!(
        super::publication::validate_decision_targets(&wrong_target.body, &effect_hash).is_err()
    );
}

#[test]
fn proven_noncommit_rejects_a_changed_subject_even_when_the_result_reports_a_failure() {
    use super::publication::validate_effect_observation;
    use super::types::{Disposition, Observation};
    use gaap::contracts::{EffectExecutionStatus, ProtectedEffectResult};

    let result: ProtectedEffectResult = serde_json::from_str(include_str!(
        "../../../../examples/contracts/protected-effect/v0.1.0/failed.json"
    ))
    .unwrap();
    let mut body = result.body;
    let changed = body.observed_post_effect_subject.clone();
    assert_ne!(changed.as_ref().unwrap(), &body.observed_pre_effect_subject);
    for disposition in [Disposition::NotStarted, Disposition::StoppedBeforeRename] {
        let observation = Observation {
            disposition,
            after: None,
            source: None,
            usage_floor: serde_json::from_value(usage(1)).unwrap(),
        };
        for status in [
            EffectExecutionStatus::Failed,
            EffectExecutionStatus::Interrupted,
            EffectExecutionStatus::UnknownOutcome,
        ] {
            body.execution_status = status;
            body.observed_post_effect_subject = changed.clone();
            assert!(
                validate_effect_observation(&body, &observation).is_err(),
                "{disposition:?} {status:?} changed"
            );
            body.observed_post_effect_subject = Some(body.observed_pre_effect_subject.clone());
            assert!(
                validate_effect_observation(&body, &observation).is_ok(),
                "{disposition:?} {status:?} unchanged"
            );
        }
        body.execution_status = EffectExecutionStatus::Executed;
        assert!(validate_effect_observation(&body, &observation).is_err());
        body.execution_status = EffectExecutionStatus::UnknownOutcome;
        body.observed_post_effect_subject = None;
        assert!(validate_effect_observation(&body, &observation).is_ok());
    }
}
