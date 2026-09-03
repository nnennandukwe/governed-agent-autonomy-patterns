//! Deterministic integrity decisions for governed agent runs.

#![forbid(unsafe_code)]

use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod contracts;
pub mod runtime;

const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const DANGEROUS_RISK_TAGS: &[&str] = &["destructive_flag", "destructive_path"];

/// A deterministic decision point owned by the run coordinator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Gate {
    /// Approval for the exact current plan before mutation.
    Plan,
    /// Authority for a normalized action before execution.
    Permission,
    /// Approval for the exact capability before activation.
    ToolTrust,
    /// Independent evidence for the current subject before completion.
    Verification,
    /// Known usage and budget authority before budgeted execution.
    Runtime,
    /// Aggregate authorization at mutation and completion transitions.
    Workflow,
}

impl FromStr for Gate {
    type Err = CoordinatorError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "plan" => Ok(Self::Plan),
            "permission" => Ok(Self::Permission),
            "tool_trust" => Ok(Self::ToolTrust),
            "verification" => Ok(Self::Verification),
            "runtime" => Ok(Self::Runtime),
            "workflow" => Ok(Self::Workflow),
            _ => Err(CoordinatorError {
                gate: value.to_owned(),
            }),
        }
    }
}

/// The precedence-bearing result of an integrity decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    /// The protected effect may proceed.
    Allow,
    /// The protected effect requires new authority before it may proceed.
    Ask,
    /// The protected effect must not proceed.
    Block,
}

/// A normalized decision returned at a protected agent-run effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    pub outcome: Outcome,
    pub code: String,
    pub effects: Vec<String>,
}

impl Decision {
    fn new(outcome: Outcome, code: &str, effects: &[&str]) -> Self {
        Self {
            outcome,
            code: code.to_owned(),
            effects: effects.iter().map(|effect| (*effect).to_owned()).collect(),
        }
    }

    fn allow(code: &str, effects: &[&str]) -> Self {
        Self::new(Outcome::Allow, code, effects)
    }

    fn ask(code: &str, effects: &[&str]) -> Self {
        Self::new(Outcome::Ask, code, effects)
    }

    fn block(code: &str, effects: &[&str]) -> Self {
        Self::new(Outcome::Block, code, effects)
    }
}

/// Returned when a caller requests a gate that this coordinator does not own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoordinatorError {
    gate: String,
}

impl Display for CoordinatorError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        write!(formatter, "unsupported gate: {}", self.gate)
    }
}

impl Error for CoordinatorError {}

/// Evaluates every currently supported integrity gate through one interface.
#[derive(Debug, Default)]
pub struct RunCoordinator;

impl RunCoordinator {
    /// Evaluate a normalized gate input without performing the protected effect.
    pub fn evaluate(&self, gate: Gate, input: &Value) -> Decision {
        match gate {
            Gate::Plan => evaluate_plan(input),
            Gate::Permission => evaluate_permission(input),
            Gate::ToolTrust => evaluate_tool_trust(input),
            Gate::Verification => evaluate_verification(input),
            Gate::Runtime => evaluate_runtime(input),
            Gate::Workflow => evaluate_workflow(input),
        }
    }
}

fn field<'a>(input: &'a Value, name: &str) -> Option<&'a Value> {
    input.as_object()?.get(name)
}

fn string_field<'a>(input: &'a Value, name: &str) -> Option<&'a str> {
    field(input, name)?.as_str()
}

fn is_digest(value: Option<&Value>) -> bool {
    let Some(value) = value.and_then(Value::as_str) else {
        return false;
    };
    let Some(hex) = value.strip_prefix("sha256:") else {
        return false;
    };
    hex.len() == 64
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn has_exact_approval(input: &Value, subject_digest: &str) -> bool {
    let Some(approval) = field(input, "approval") else {
        return false;
    };
    string_field(approval, "status") == Some("approved")
        && string_field(approval, "subject_digest") == Some(subject_digest)
}

fn evaluate_plan(input: &Value) -> Decision {
    if !is_digest(field(input, "subject_digest")) {
        return Decision::block("plan.invalid_input", &["stop_mutation"]);
    }
    let Some(approval) = field(input, "approval").filter(|value| !value.is_null()) else {
        return Decision::block("plan.approval_required", &["stop_mutation"]);
    };
    if string_field(approval, "status") != Some("approved") {
        return Decision::block("plan.approval_required", &["stop_mutation"]);
    }
    if string_field(approval, "subject_digest") != string_field(input, "subject_digest") {
        return Decision::block(
            "plan.stale_approval",
            &["stop_mutation", "request_new_plan_approval"],
        );
    }
    Decision::allow("plan.approved_exact", &[])
}

fn evaluate_permission(input: &Value) -> Decision {
    let policy = field(input, "policy_decision");
    let policy_is_valid = match policy {
        Some(Value::Null) => true,
        Some(Value::String(value)) => matches!(value.as_str(), "allow" | "ask" | "deny"),
        _ => false,
    };
    if !is_digest(field(input, "action_digest"))
        || !policy_is_valid
        || !field(input, "risk_tags").is_some_and(Value::is_array)
        || !field(input, "wrapper_chain").is_some_and(Value::is_array)
    {
        return Decision::block("permission.invalid_input", &["stop_action"]);
    }

    let action_digest = string_field(input, "action_digest").unwrap_or_default();
    let exact_approval = has_exact_approval(input, action_digest);
    if policy.and_then(Value::as_str) == Some("deny") {
        return Decision::block("permission.denied", &["stop_action"]);
    }

    let has_dangerous_risk = field(input, "risk_tags")
        .and_then(Value::as_array)
        .is_some_and(|tags| {
            tags.iter().any(|tag| {
                tag.as_str()
                    .is_some_and(|value| DANGEROUS_RISK_TAGS.contains(&value))
            })
        });
    if has_dangerous_risk {
        return if exact_approval {
            Decision::allow("permission.approved_exception", &["record_action_approval"])
        } else {
            Decision::ask(
                "permission.dangerous_requires_approval",
                &["request_action_approval"],
            )
        };
    }

    let has_wrapper = field(input, "wrapper_chain")
        .and_then(Value::as_array)
        .is_some_and(|chain| !chain.is_empty());
    if has_wrapper {
        return if exact_approval {
            Decision::allow("permission.approved_exception", &["record_action_approval"])
        } else {
            Decision::ask(
                "permission.wrapper_requires_approval",
                &["request_action_approval"],
            )
        };
    }

    match policy.and_then(Value::as_str) {
        Some("ask") if exact_approval => {
            Decision::allow("permission.approved_exception", &["record_action_approval"])
        }
        Some("ask") => Decision::ask(
            "permission.policy_requires_approval",
            &["request_action_approval"],
        ),
        Some("allow") => Decision::allow("permission.policy_allowed", &[]),
        _ => Decision::ask("permission.unknown_action", &["request_action_approval"]),
    }
}

fn evaluate_tool_trust(input: &Value) -> Decision {
    let capability_name = string_field(input, "capability_name");
    if capability_name.is_none_or(|value| value.trim().is_empty())
        || !is_digest(field(input, "capability_digest"))
    {
        return Decision::block("tool_trust.invalid_input", &["stop_capability_activation"]);
    }
    let Some(approval) = field(input, "approval").filter(|value| !value.is_null()) else {
        return Decision::block(
            "tool_trust.approval_required",
            &["stop_capability_activation"],
        );
    };
    if string_field(approval, "status") != Some("approved") {
        return Decision::block(
            "tool_trust.approval_required",
            &["stop_capability_activation"],
        );
    }
    if string_field(approval, "subject_digest") != string_field(input, "capability_digest") {
        return Decision::block(
            "tool_trust.stale_approval",
            &[
                "stop_capability_activation",
                "request_new_capability_approval",
            ],
        );
    }
    Decision::allow("tool_trust.approved_exact", &[])
}

fn evidence_is_valid(value: Option<&Value>) -> bool {
    value.and_then(Value::as_array).is_some_and(|items| {
        !items.is_empty()
            && items.iter().all(|item| {
                string_field(item, "command").is_some_and(|value| !value.trim().is_empty())
                    && string_field(item, "output").is_some_and(|value| !value.trim().is_empty())
                    && string_field(item, "result") == Some("PASS")
            })
    })
}

fn evaluate_verification(input: &Value) -> Decision {
    if !is_digest(field(input, "subject_digest"))
        || string_field(input, "implementer_id").is_none_or(|value| value.trim().is_empty())
    {
        return Decision::block("verification.invalid_input", &["stop_completion"]);
    }
    let Some(report) = field(input, "report").filter(|value| !value.is_null()) else {
        return Decision::block("verification.report_required", &["stop_completion"]);
    };
    if string_field(report, "verifier_id").is_none_or(|value| value.trim().is_empty()) {
        return Decision::block("verification.invalid_input", &["stop_completion"]);
    }
    if string_field(report, "verifier_id") == string_field(input, "implementer_id") {
        return Decision::block("verification.independence_required", &["stop_completion"]);
    }
    if string_field(report, "subject_digest") != string_field(input, "subject_digest") {
        return Decision::block(
            "verification.stale_subject",
            &["stop_completion", "request_new_verification"],
        );
    }
    if !evidence_is_valid(field(report, "evidence")) {
        return Decision::block("verification.evidence_required", &["stop_completion"]);
    }
    if string_field(report, "verdict") != Some("PASS") {
        return Decision::block("verification.pass_required", &["stop_completion"]);
    }
    Decision::allow("verification.passed", &["record_verification_receipt"])
}

fn safe_non_negative_integer(value: Option<&Value>) -> Option<u64> {
    value
        .and_then(Value::as_u64)
        .filter(|number| *number <= MAX_SAFE_INTEGER)
}

fn valid_budget(input: &Value) -> Option<(u64, u64, u64, u64)> {
    let current = safe_non_negative_integer(field(input, "current_cost_micros"))?;
    let next = safe_non_negative_integer(field(input, "estimated_next_cost_micros"))?;
    let thresholds = field(input, "thresholds")?;
    let warning = safe_non_negative_integer(field(thresholds, "warn_micros"))?;
    let approval = safe_non_negative_integer(field(thresholds, "approval_micros"))?;
    let hard_stop = safe_non_negative_integer(field(thresholds, "hard_stop_micros"))?;
    if warning > approval || approval > hard_stop {
        return None;
    }
    let projected = current.checked_add(next)?;
    if projected > MAX_SAFE_INTEGER {
        return None;
    }
    Some((projected, warning, approval, hard_stop))
}

fn evaluate_runtime(input: &Value) -> Decision {
    let usage_status = string_field(input, "usage_status");
    if !is_digest(field(input, "action_digest"))
        || !matches!(usage_status, Some("known" | "missing" | "ambiguous"))
    {
        return Decision::block("runtime.invalid_input", &["stop_execution"]);
    }
    let Some((projected, warning, approval_threshold, hard_stop)) = valid_budget(input) else {
        return Decision::block("runtime.invalid_budget", &["stop_execution"]);
    };
    if usage_status != Some("known") {
        return Decision::block(
            "runtime.usage_unknown",
            &["stop_execution", "request_usage_reconciliation"],
        );
    }
    if projected >= hard_stop {
        return Decision::block("runtime.hard_stop", &["stop_execution", "notify_operator"]);
    }
    if projected >= approval_threshold {
        let action_digest = string_field(input, "action_digest").unwrap_or_default();
        let approved_max = field(input, "approval")
            .and_then(|approval| safe_non_negative_integer(field(approval, "max_cost_micros")));
        if has_exact_approval(input, action_digest)
            && approved_max.is_some_and(|maximum| maximum >= projected)
        {
            return Decision::allow("runtime.overage_approved", &["record_overage_approval"]);
        }
        return Decision::ask("runtime.approval_required", &["request_overage_approval"]);
    }
    if projected >= warning {
        return Decision::allow("runtime.warning", &["notify_operator"]);
    }
    Decision::allow("runtime.within_budget", &[])
}

fn outcome<'a>(input: &'a Value, name: &str) -> Option<&'a str> {
    let value = string_field(input, name)?;
    matches!(value, "allow" | "ask" | "block").then_some(value)
}

fn evaluate_workflow(input: &Value) -> Decision {
    let boundary = string_field(input, "boundary");
    let gate_results = field(input, "gate_results").filter(|value| value.is_object());
    if gate_results.is_none() || !matches!(boundary, Some("mutation" | "completion")) {
        let effect = if boundary == Some("mutation") {
            "stop_mutation"
        } else {
            "stop_completion"
        };
        return Decision::block("workflow.invalid_input", &[effect]);
    }
    let gate_results = gate_results.unwrap_or(&Value::Null);

    if boundary == Some("mutation") {
        let outcomes = [
            outcome(gate_results, "plan"),
            outcome(gate_results, "permission"),
            outcome(gate_results, "tool_trust"),
            outcome(gate_results, "runtime"),
        ];
        if outcomes.iter().any(Option::is_none) {
            return Decision::block("workflow.invalid_input", &["stop_mutation"]);
        }
        if outcomes.contains(&Some("block")) {
            return Decision::block("workflow.mutation_blocked", &["stop_mutation"]);
        }
        if outcomes.contains(&Some("ask")) {
            return Decision::ask("workflow.mutation_requires_approval", &["pause_mutation"]);
        }
        return Decision::allow(
            "workflow.mutation_authorized",
            &["record_mutation_authorization"],
        );
    }

    let mutation_authorized = field(input, "mutation_authorized").and_then(Value::as_bool);
    let Some(mutation_authorized) = mutation_authorized else {
        return Decision::block("workflow.invalid_input", &["stop_completion"]);
    };
    if !mutation_authorized {
        return Decision::block("workflow.mutation_not_authorized", &["stop_completion"]);
    }
    let Some(verification) = outcome(gate_results, "verification") else {
        return Decision::block("workflow.invalid_input", &["stop_completion"]);
    };
    if verification != "allow" {
        return Decision::block("workflow.verification_failed", &["stop_completion"]);
    }
    Decision::allow("workflow.completion_authorized", &["record_completion"])
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Gate, Outcome, RunCoordinator};

    #[test]
    fn exact_plan_approval_allows_mutation() {
        let digest = format!("sha256:{}", "a".repeat(64));
        let input = json!({
            "subject_digest": digest,
            "approval": {
                "status": "approved",
                "subject_digest": digest,
            }
        });

        let decision = RunCoordinator.evaluate(Gate::Plan, &input);

        assert_eq!(decision.outcome, Outcome::Allow);
        assert_eq!(decision.code, "plan.approved_exact");
    }

    #[test]
    fn missing_usage_fails_closed() {
        let input = json!({
            "action_digest": format!("sha256:{}", "b".repeat(64)),
            "usage_status": "missing",
            "current_cost_micros": 10,
            "estimated_next_cost_micros": 5,
            "thresholds": {
                "warn_micros": 20,
                "approval_micros": 30,
                "hard_stop_micros": 40,
            }
        });

        let decision = RunCoordinator.evaluate(Gate::Runtime, &input);

        assert_eq!(decision.outcome, Outcome::Block);
        assert_eq!(decision.code, "runtime.usage_unknown");
    }

    #[test]
    fn completion_requires_prior_mutation_authorization() {
        let input = json!({
            "boundary": "completion",
            "mutation_authorized": false,
            "gate_results": { "verification": "allow" }
        });

        let decision = RunCoordinator.evaluate(Gate::Workflow, &input);

        assert_eq!(decision.outcome, Outcome::Block);
        assert_eq!(decision.code, "workflow.mutation_not_authorized");
    }
}
