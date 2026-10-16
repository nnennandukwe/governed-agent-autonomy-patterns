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
    fn blocked(&mut self, ask: bool, status: &'static str) {
        self.effect_statuses.push(status);
        if ask {
            self.transition(State::AwaitingAuthority);
        }
        self.transition(State::Blocked);
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
        completion_checked: false,
        effect_statuses: vec![],
    };
    result.transition(State::Planning);
    if result.charge(history.plan_usage, history.budget) {
        result.transition(State::Blocked);
        return result;
    }
    if !history.approved_plan {
        result.transition(State::Blocked);
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
            Operation::Cancel => result.transition(State::Interrupted),
            Operation::AdapterError { usage } => {
                result.charge(*usage, history.budget);
                result.transition(State::Failed);
            }
            Operation::Finish {
                verification,
                usage,
            } => {
                result.transition(State::Verifying);
                result.verified = true;
                let over = result.charge(*usage, history.budget);
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
                    result.transition(State::Failed);
                    continue;
                }
                if identities.len() >= history.max_effects
                    || identities.contains(identity)
                    || *defect == Defect::StaleSubject
                {
                    result.blocked(false, "denied");
                    continue;
                }
                // Trusted reservations must fit before dispatch. Cost's hard stop is inclusive.
                let projected: Meter =
                    std::array::from_fn(|axis| result.usage[axis] + history.estimate[axis]);
                if (1..4).any(|axis| projected[axis] > history.budget[axis]) {
                    result.blocked(false, "denied");
                    continue;
                }
                if history.permission != Permission::Allow {
                    let ask = history.permission == Permission::Ask;
                    result.blocked(ask, if ask { "awaiting_authority" } else { "denied" });
                    continue;
                }
                if *defect != Defect::None {
                    result.blocked(false, "denied");
                    continue;
                }
                if projected[0] >= history.budget[0] {
                    result.blocked(false, "denied");
                    continue;
                }
                result.dispatched.push(index);
                if *outcome == Outcome::BeforeDispatch {
                    result.charge(*usage, history.budget);
                    result.blocked(false, "denied");
                    continue;
                }
                if *outcome == Outcome::Denied {
                    result.charge(*usage, history.budget);
                    result.blocked(false, "denied");
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
                    Outcome::Success => result.transition(State::Blocked),
                    Outcome::Interrupted | Outcome::Timeout => {
                        result.transition(State::Interrupted)
                    }
                    Outcome::LostReply if over => result.transition(State::Blocked),
                    _ => result.transition(State::Failed),
                }
            }
        }
    }
    result
}
