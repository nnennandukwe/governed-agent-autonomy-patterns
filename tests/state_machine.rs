#[path = "state_machine/model.rs"]
mod model;
#[path = "state_machine/sut.rs"]
mod sut;

use gaap::contracts::{AgentRunStatus, RunEvent};
use model::*;
use proptest::{
    prelude::*,
    test_runner::{Config, RngSeed, TestError, TestRunner},
};

fn state(status: AgentRunStatus) -> State {
    match status {
        AgentRunStatus::Accepted => State::Accepted,
        AgentRunStatus::Planning => State::Planning,
        AgentRunStatus::AwaitingAuthority => State::AwaitingAuthority,
        AgentRunStatus::Executing => State::Executing,
        AgentRunStatus::Verifying => State::Verifying,
        AgentRunStatus::Completed => State::Completed,
        AgentRunStatus::Blocked => State::Blocked,
        AgentRunStatus::Failed => State::Failed,
        AgentRunStatus::Interrupted => State::Interrupted,
    }
}
fn check(history: &History) -> Result<(), String> {
    let expected = predict(history);
    let (actual, trace) = sut::execute(history)?;
    macro_rules! require { ($condition:expr, $label:expr) => { if !$condition { return Err(format!("{}: expected {expected:?}; actual status {:?}, usage {:?}, subject {}, executor calls {:?}", $label, actual.receipt.body.terminal_status, actual.receipt.body.usage, actual.receipt.body.resulting_subject_digest, trace.executor_calls)); } }; }
    let body = &actual.receipt.body;
    require!(
        body.request_digest == sut::hash(&sut::request(history)),
        "canonical run identity"
    );
    require!(
        state(body.terminal_status) == expected.state,
        "terminal status"
    );
    require!(sut::meter(&body.usage) == expected.usage, "retained usage");
    require!(
        body.resulting_subject_digest == sut::subject(expected.subject).digest,
        "latest subject"
    );
    require!(
        trace.executor_calls == expected.dispatched,
        "dispatch authority and no silent retry"
    );
    let external: Vec<_> = expected
        .dispatched
        .iter()
        .copied()
        .filter(|index| {
            !matches!(
                history.operations.get(*index),
                Some(Operation::Effect {
                    outcome: Outcome::BeforeDispatch | Outcome::Denied,
                    ..
                })
            )
        })
        .collect();
    require!(
        trace.external_dispatches == external,
        "external dispatch after pre-dispatch failure"
    );
    require!(
        trace.next_calls == expected.proposals && trace.closes == 1,
        "no post-terminal callbacks"
    );
    require!(
        trace.verifier_calls == usize::from(expected.verified),
        "independent verification invocation"
    );
    require!(
        trace.completion_checks == usize::from(expected.completion_checked),
        "current verification check"
    );
    let transitions: Vec<_> = body
        .events
        .iter()
        .filter_map(|event| {
            if let RunEvent::StatusTransition { from, to, .. } = event {
                Some((state(*from), state(*to)))
            } else {
                None
            }
        })
        .collect();
    require!(
        transitions.iter().map(|(_, to)| *to).collect::<Vec<_>>() == expected.states,
        "lifecycle transitions"
    );
    let mut previous = State::Accepted;
    let mut terminals = 0;
    for (from, to) in transitions {
        require!(
            !previous.terminal() && previous == from,
            "terminal transition or broken continuity"
        );
        if to.terminal() {
            terminals += 1;
        }
        previous = to;
    }
    require!(terminals == 1, "one terminal receipt event");
    require!(
        matches!(body.events.last(), Some(RunEvent::StatusTransition { to, .. }) if state(*to).terminal()),
        "no post-terminal events"
    );
    let mut retained = [0; 4];
    for event in &body.events {
        if let RunEvent::Usage { usage, .. } = event {
            let current = sut::meter(usage);
            require!(
                (0..4).all(|axis| current[axis] >= retained[axis]),
                "monotone usage"
            );
            retained = current;
        }
    }
    require!(retained == expected.usage, "final retained usage event");
    let statuses: Vec<_> = actual
        .protected_effect_results
        .iter()
        .map(|result| {
            serde_json::to_value(result.body.execution_status)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    require!(
        statuses == expected.effect_statuses,
        "effect classification"
    );
    for (result, proposal) in actual.protected_effect_results.iter().zip(&trace.requests) {
        require!(
            result.body.effect_request_digest == sut::hash(proposal),
            "canonical effect identity"
        );
        require!(
            result.result_digest == sut::hash(&result.body),
            "canonical result identity"
        );
    }
    require!(
        actual.receipt.receipt_digest == sut::hash(body),
        "canonical receipt identity"
    );
    Ok(())
}
fn meters() -> BoxedStrategy<Meter> {
    prop_oneof![8 => Just([0;4]), 4 => Just([1,1,1,0]), 1 => (0..130u64, 0..130u64, 0..130u64, 0..4u64).prop_map(|(cost,time,tokens,calls)| [cost,time,tokens,calls])].boxed()
}
fn operations() -> BoxedStrategy<Operation> {
    let outcomes = prop_oneof![12 => Just(Outcome::Success), 1 => proptest::sample::select(vec![Outcome::Denied, Outcome::BeforeDispatch, Outcome::FailedPreserved, Outcome::FailedChanged, Outcome::Interrupted, Outcome::Timeout, Outcome::UnknownPreserved, Outcome::UnknownChanged, Outcome::LostReply])];
    prop_oneof![
        12 => (0..16u8, any::<u8>(), prop_oneof![16 => Just(Defect::None), 1 => proptest::sample::select(vec![Defect::Schema, Defect::Descriptor, Defect::StaleSubject])], outcomes, meters(), any::<bool>()).prop_map(|(identity,content,defect,outcome,usage,repeatable)| Operation::Effect {identity,content,defect,outcome,usage,repeatable}),
        1 => (proptest::sample::select(vec![Verification::Pass, Verification::Fail, Verification::Stale, Verification::SameActor, Verification::Untrusted, Verification::Error, Verification::ChangedDuring, Verification::ChangedAfter]), meters()).prop_map(|(verification,usage)| Operation::Finish {verification,usage}),
        1 => Just(Operation::Cancel),
        1 => meters().prop_map(|usage| Operation::AdapterError {usage}),
    ].boxed()
}
fn histories(max_length: usize) -> impl Strategy<Value = History> {
    (prop_oneof![12 => Just(Permission::Allow), 1 => Just(Permission::Ask), 1 => Just(Permission::Block)],
     prop_oneof![20 => Just(true), 1 => Just(false)], meters(),
     prop_oneof![8 => Just([100;4]), 1 => (1..130u64, 0..130u64, 0..130u64, 0..10u64).prop_map(|(cost,time,tokens,calls)| [cost,time,tokens,calls])],
     prop_oneof![8 => Just([1,1,0,1]), 1 => meters()], 1..17usize,
     proptest::collection::vec(operations(), 0..=max_length))
        .prop_map(|(permission,approved_plan,plan_usage,budget,estimate,max_effects,operations)| History { permission,approved_plan,plan_usage,budget,estimate,max_effects,operations })
}
/// Proptest minimizes values; this final deletion pass also establishes one-operation minimality.
fn deletion_minimize(mut history: History) -> History {
    let mut index = 0;
    while index < history.operations.len() {
        let mut candidate = history.clone();
        candidate.operations.remove(index);
        if check(&candidate).is_err() {
            history = candidate;
            index = 0;
        } else {
            index += 1;
        }
    }
    history
}
#[test]
fn generated_agent_run_histories() {
    if let Ok(path) = std::env::var("GAAP_PROPTEST_REPLAY") {
        let history: History =
            serde_json::from_slice(&std::fs::read(path).expect("read replay JSON"))
                .expect("parse replay history");
        check(&history).unwrap_or_else(|reason| panic!("replay failed: {reason}"));
        return;
    }
    let (cases, max_length) = match std::env::var("GAAP_PROPTEST_PROFILE").as_deref() {
        Err(_) | Ok("ci") => (256, 32),
        Ok("deep") => (10_000, 256),
        Ok(profile) => panic!("unsupported GAAP_PROPTEST_PROFILE {profile}; use ci or deep"),
    };
    let seed = std::env::var("GAAP_PROPTEST_SEED")
        .map(|seed| seed.parse().expect("GAAP_PROPTEST_SEED must be u64"))
        .unwrap_or(18_020_261_015);
    eprintln!("GAAP_PROPTEST_SEED={seed} cases={cases} max_operations={max_length}");
    let mut runner = TestRunner::new(Config {
        cases,
        rng_seed: RngSeed::Fixed(seed),
        failure_persistence: None,
        max_shrink_iters: 100_000,
        max_shrink_time: 0,
        ..Config::default()
    });
    match runner.run(&histories(max_length), |history| {
        check(&history).map_err(TestCaseError::fail)
    }) {
        Ok(()) => (),
        Err(TestError::Fail(reason, history)) => {
            let minimized = deletion_minimize(history);
            let reason = check(&minimized).err().unwrap_or(reason.to_string());
            eprintln!(
                "GAAP_PROPTEST_SEED={seed}\nGAAP_MINIMIZED_HISTORY={}",
                serde_json::to_string(&minimized).unwrap()
            );
            panic!(
                "{reason}\nReplay: save GAAP_MINIMIZED_HISTORY as history.json; GAAP_PROPTEST_REPLAY=history.json cargo test --locked --test state_machine generated_agent_run_histories -- --nocapture"
            );
        }
        Err(error) => panic!("property run aborted: {error}"),
    }
}
fn success(identity: u8) -> Operation {
    Operation::Effect {
        identity,
        content: identity,
        defect: Defect::None,
        outcome: Outcome::Success,
        usage: [1, 1, 1, 0],
        repeatable: false,
    }
}
fn fixture(outcome: Outcome, verification: Verification) -> History {
    let mut operation = success(1);
    if let Operation::Effect {
        outcome: actual, ..
    } = &mut operation
    {
        *actual = outcome;
    }
    History {
        operations: vec![
            operation,
            Operation::Finish {
                verification,
                usage: [0; 4],
            },
            success(2),
        ],
        ..History::default()
    }
}
macro_rules! regression {
    ($name:ident, $outcome:ident, $verification:ident) => {
        #[test]
        fn $name() {
            check(&fixture(Outcome::$outcome, Verification::$verification)).unwrap();
        }
    };
}
regression!(
    failed_intent_persistence_prevents_dispatch,
    BeforeDispatch,
    Pass
);
regression!(
    failure_before_replacement_preserves_original,
    FailedPreserved,
    Pass
);
regression!(
    failure_after_replacement_retains_unknown_effect,
    UnknownChanged,
    Pass
);
regression!(
    failed_verification_retains_mutation_and_blocks_completion,
    Success,
    Fail
);
regression!(
    mutation_during_verification_invalidates_freshness,
    Success,
    ChangedDuring
);
regression!(
    mutation_after_verification_invalidates_freshness,
    Success,
    ChangedAfter
);
#[test]
fn lost_reply_refuses_repeatable_retry() {
    let mut history = fixture(Outcome::LostReply, Verification::Pass);
    if let Operation::Effect { repeatable, .. } = &mut history.operations[0] {
        *repeatable = true;
    }
    history.operations.insert(1, success(1));
    check(&history).unwrap();
}
#[test]
fn duplicate_identity_with_changed_content_cant_execute_again() {
    check(&History {
        operations: vec![
            success(1),
            Operation::Effect {
                identity: 1,
                content: 2,
                defect: Defect::None,
                outcome: Outcome::Success,
                usage: [0; 4],
                repeatable: true,
            },
            success(2),
        ],
        ..History::default()
    })
    .unwrap();
}
#[test]
fn schema_descriptor_and_stale_subject_fail_closed() {
    for defect in [Defect::Schema, Defect::Descriptor, Defect::StaleSubject] {
        let mut history = fixture(Outcome::Success, Verification::Pass);
        if let Operation::Effect { defect: actual, .. } = &mut history.operations[0] {
            *actual = defect;
        }
        check(&history).unwrap();
    }
}
#[test]
fn every_outcome_and_verification_has_a_named_input() {
    for outcome in [
        Outcome::Success,
        Outcome::Denied,
        Outcome::BeforeDispatch,
        Outcome::FailedPreserved,
        Outcome::FailedChanged,
        Outcome::Interrupted,
        Outcome::Timeout,
        Outcome::UnknownPreserved,
        Outcome::UnknownChanged,
        Outcome::LostReply,
    ] {
        for verification in [
            Verification::Pass,
            Verification::Fail,
            Verification::Stale,
            Verification::SameActor,
            Verification::Untrusted,
            Verification::Error,
            Verification::ChangedDuring,
            Verification::ChangedAfter,
        ] {
            check(&fixture(outcome, verification)).unwrap();
        }
    }
}

#[test]
fn reservation_overage_and_exhaustion_cover_each_budget_dimension() {
    for axis in 0..4 {
        let mut history = History {
            budget: [10; 4],
            ..History::default()
        };
        history.estimate = [0; 4];
        history.estimate[axis] = 11;
        history.operations = vec![success(1)];
        check(&history).unwrap();
        history.estimate = [0; 4];
        if let Operation::Effect { usage, .. } = &mut history.operations[0] {
            usage[axis] = 11;
        }
        check(&history).unwrap();
        history.operations = vec![success(1), success(2)];
        history.budget[axis] = 1;
        history.estimate[axis] = 1;
        check(&history).unwrap();
    }
}
#[test]
fn ask_and_block_never_dispatch_or_resume_the_same_run() {
    for permission in [Permission::Ask, Permission::Block] {
        check(&History {
            permission,
            operations: vec![success(1), success(2)],
            ..History::default()
        })
        .unwrap();
    }
}
#[test]
fn receipt_publication_uncertainty_refuses_same_run_replay() {
    // A host reports the uncertain effect through its existing executor port.
    let mut history = fixture(Outcome::UnknownChanged, Verification::Pass);
    if let Operation::Effect { repeatable, .. } = &mut history.operations[0] {
        *repeatable = true;
    }
    history.operations.insert(1, success(1));
    check(&history).unwrap();
}
#[test]
fn host_link_or_relocated_session_refusal_never_dispatches() {
    // The host owns filesystem/session checks; the core consumes its proven refusal.
    check(&fixture(Outcome::BeforeDispatch, Verification::Pass)).unwrap();
}
#[test]
fn known_publication_with_directory_sync_warning_retains_success() {
    // A durability warning doesn't change the host's known effect observation.
    check(&fixture(Outcome::Success, Verification::Pass)).unwrap();
}

#[test]
fn duplicate_identity_with_stale_subject_returns_a_sealed_refusal() {
    let history: History = serde_json::from_str(include_str!(
        "state_machine/fixtures/duplicate-stale-subject.json"
    ))
    .unwrap();
    check(&history).unwrap();
}
