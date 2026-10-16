//! Independent oracle: no GAAP imports, validators, or coordinator decisions.
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// Cost, elapsed time, model tokens, tool calls; all observations are deltas.
pub type Meter = [u64; 4];

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum Permission {
    Allow,
    Ask,
    Block,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum Defect {
    None,
    Schema,
    Descriptor,
    StaleSubject,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum Outcome {
    Success,
    Denied,
    BeforeDispatch,
    FailedPreserved,
    FailedChanged,
    Interrupted,
    Timeout,
    UnknownPreserved,
    UnknownChanged,
    LostReply,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum Verification {
    Pass,
    Fail,
    Stale,
    SameActor,
    Untrusted,
    Error,
    ChangedDuring,
    ChangedAfter,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Operation {
    Effect {
        identity: u8,
        content: u8,
        defect: Defect,
        outcome: Outcome,
        usage: Meter,
        repeatable: bool,
    },
    Finish {
        verification: Verification,
        usage: Meter,
    },
    Cancel,
    AdapterError {
        usage: Meter,
    },
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct History {
    pub permission: Permission,
    pub approved_plan: bool,
    pub plan_usage: Meter,
    pub budget: Meter,
    pub estimate: Meter,
    pub max_effects: usize,
    pub operations: Vec<Operation>,
}
impl Default for History {
    fn default() -> Self {
        Self {
            permission: Permission::Allow,
            approved_plan: true,
            plan_usage: [0; 4],
            budget: [100; 4],
            estimate: [1, 1, 0, 1],
            max_effects: 8,
            operations: vec![],
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Accepted,
    Planning,
    AwaitingAuthority,
    Executing,
    Verifying,
    Completed,
    Blocked,
    Failed,
    Interrupted,
}
impl State {
    pub fn terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Blocked | Self::Failed | Self::Interrupted
        )
    }
}
#[derive(Debug)]
pub struct Expected {
    pub state: State,
    pub states: Vec<State>,
    pub usage: Meter,
    pub subject: u64,
    pub dispatched: Vec<usize>,
    pub proposals: usize,
    pub contexts: Vec<(u64, Meter)>,
    pub verified: bool,
    pub terminal_reason: Option<&'static str>,
    pub completion_checked: bool,
    pub effect_statuses: Vec<&'static str>,
}
impl Expected {
    fn transition(&mut self, state: State) {
        assert!(!self.state.terminal());
        self.state = state;
        self.states.push(state);
    }
    fn charge(&mut self, delta: Meter, budget: Meter) -> bool {
        for (total, value) in self.usage.iter_mut().zip(delta) {
            *total += value;
        }
        self.usage
            .iter()
            .zip(budget)
            .any(|(total, limit)| *total > limit)
    }
    fn finish(&mut self, state: State, reason: &'static str) {
        self.terminal_reason = Some(reason);
        self.transition(state);
    }
    fn blocked(&mut self, ask: bool, status: &'static str, reason: &'static str) {
        self.effect_statuses.push(status);
        if ask {
            self.transition(State::AwaitingAuthority);
        }
        self.finish(State::Blocked, reason);
    }
}
/// Predict a whole bounded run. Unconsumed operations are attempted work after termination.
pub fn predict(history: &History) -> Expected {
    let mut result = Expected {
        state: State::Accepted,
        states: vec![],
        usage: [0; 4],
        subject: 0,
        dispatched: vec![],
        proposals: 0,
        contexts: vec![],
        verified: false,
        terminal_reason: None,
        completion_checked: false,
        effect_statuses: vec![],
    };
    result.transition(State::Planning);
    if result.charge(history.plan_usage, history.budget) {
        result.finish(State::Blocked, "runtime.budget_exceeded");
        return result;
    }
    if !history.approved_plan {
        result.finish(State::Blocked, "plan.approval_required");
        return result;
    }
    result.transition(State::Executing);
    let mut identities = BTreeSet::new();
    for (index, operation) in history.operations.iter().enumerate() {
        if result.state.terminal() {
            break;
        }
        result.contexts.push((result.subject, result.usage));
        result.proposals += 1;
        match operation {
            Operation::Cancel => result.finish(State::Interrupted, "runtime.interrupted"),
            Operation::AdapterError { usage } => {
                result.charge(*usage, history.budget);
                result.finish(State::Failed, "adapter.next_effect_failed: agent failed");
            }
            Operation::Finish {
                verification,
                usage,
            } => {
                result.transition(State::Verifying);
                result.verified = true;
                let over = result.charge(*usage, history.budget);
                result.terminal_reason = Some(if *verification == Verification::Error {
                    "verifier.failed: verifier failed"
                } else if over {
                    "runtime.budget_exceeded"
                } else {
                    match verification {
                        Verification::Fail => "verification.pass_required",
                        Verification::Stale => "verification.stale_subject",
                        Verification::SameActor => "verification.independence_required",
                        Verification::Untrusted => "verification.untrusted_verifier",
                        Verification::ChangedDuring | Verification::ChangedAfter => {
                            "verifier.completion_check_failed: subject changed after verification"
                        }
                        Verification::Pass => "workflow.completion_authorized",
                        Verification::Error => unreachable!(),
                    }
                });
                let terminal = if *verification == Verification::Error {
                    State::Failed
                } else if over
                    || matches!(
                        verification,
                        Verification::Fail
                            | Verification::Stale
                            | Verification::SameActor
                            | Verification::Untrusted
                    )
                {
                    State::Blocked
                } else {
                    result.completion_checked = true;
                    if matches!(
                        verification,
                        Verification::ChangedDuring | Verification::ChangedAfter
                    ) {
                        State::Failed
                    } else {
                        State::Completed
                    }
                };
                result.transition(terminal);
            }
            Operation::Effect {
                identity,
                defect,
                outcome,
                usage,
                ..
            } => {
                if *defect == Defect::Descriptor {
                    result.finish(State::Failed, "protected_effect.invalid_request: Protected Effect Request does not match the validated Agent Run Request");
                    continue;
                }
                if *defect == Defect::StaleSubject {
                    result.blocked(false, "denied", "protected_effect.stale_subject");
                    continue;
                }
                if identities.len() >= history.max_effects {
                    result.blocked(false, "denied", "runtime.max_effects_exhausted");
                    continue;
                }
                if identities.contains(identity) {
                    result.blocked(false, "denied", "runtime.duplicate_effect");
                    continue;
                }
                // Trusted reservations must fit before dispatch. Cost's hard stop is inclusive.
                let projected: Meter =
                    std::array::from_fn(|axis| result.usage[axis] + history.estimate[axis]);
                if (1..4).any(|axis| projected[axis] > history.budget[axis]) {
                    result.blocked(false, "denied", "runtime.budget_exceeded");
                    continue;
                }
                if history.permission != Permission::Allow {
                    let ask = history.permission == Permission::Ask;
                    result.blocked(
                        ask,
                        if ask { "awaiting_authority" } else { "denied" },
                        if ask {
                            "permission.policy_requires_approval"
                        } else {
                            "permission.denied"
                        },
                    );
                    continue;
                }
                if *defect != Defect::None {
                    result.blocked(false, "denied", "tool_trust.approval_required");
                    continue;
                }
                if projected[0] >= history.budget[0] {
                    result.blocked(false, "denied", "runtime.hard_stop");
                    continue;
                }
                result.dispatched.push(index);
                if *outcome == Outcome::BeforeDispatch {
                    result.charge(*usage, history.budget);
                    result.blocked(false, "denied", "executor.not_dispatched");
                    continue;
                }
                if *outcome == Outcome::Denied {
                    result.charge(*usage, history.budget);
                    result.blocked(false, "denied", "executor.denied");
                    continue;
                }
                identities.insert(*identity);
                let mut retained = *usage;
                retained[3] = retained[3].max(1);
                // Lost responses expose only the known tool-attempt minimum in v0.1.
                if *outcome == Outcome::LostReply {
                    retained = [0, 0, 0, 1];
                }
                let over = result.charge(retained, history.budget);
                if matches!(
                    outcome,
                    Outcome::Success | Outcome::FailedChanged | Outcome::UnknownChanged
                ) {
                    result.subject += 1;
                }
                let status = match outcome {
                    Outcome::Success => "executed",
                    Outcome::FailedPreserved | Outcome::FailedChanged => "failed",
                    Outcome::Interrupted | Outcome::Timeout => "interrupted",
                    Outcome::UnknownPreserved | Outcome::UnknownChanged | Outcome::LostReply => {
                        "unknown_outcome"
                    }
                    Outcome::Denied | Outcome::BeforeDispatch => unreachable!(),
                };
                result.effect_statuses.push(status);
                match outcome {
                    Outcome::Success if !over => (),
                    Outcome::Success => result.finish(State::Blocked, "runtime.budget_exceeded"),
                    Outcome::Interrupted | Outcome::Timeout => {
                        result.finish(State::Interrupted, "protected_effect.interrupted")
                    }
                    Outcome::LostReply if over => {
                        result.finish(State::Blocked, "runtime.budget_exceeded")
                    }
                    Outcome::FailedPreserved | Outcome::FailedChanged => {
                        result.finish(State::Failed, "protected_effect.failed")
                    }
                    _ => result.finish(State::Failed, "protected_effect.unknown_outcome"),
                }
            }
        }
    }
    result
}
