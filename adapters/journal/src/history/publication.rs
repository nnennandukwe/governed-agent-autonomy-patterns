use gaap::contracts::{
    AgentRunStatus, ContractSupport, EffectEvidenceType, EffectExecutionStatus, EvidenceType,
    ProtectedEffectResult, ResourceUsage, RunEvent, TerminalRunReceiptBody, VerificationVerdict,
    validate_protected_effect_request, verify_protected_effect_result, verify_terminal_receipt,
};
use gaap::{Gate, Outcome};

use super::evidence::Index;
use super::facts::usage_at_least;
use super::require;
use super::types::*;
use crate::{Error, Result};

pub(super) fn validate(
    p: &Publication,
    a: &Attempt,
    support: &ContractSupport,
    archive: &Index,
    dispatch: Option<&Dispatch>,
    observation: Option<&Observation>,
    verification: Option<&Verification>,
) -> Result<()> {
    let receipt = &p.receipt.body;
    verify_terminal_receipt(&a.run_request, support, &p.receipt)
        .map_err(|_| Error::invalid("original terminal receipt failed frozen validation"))?;
    let effect_hash = validate_protected_effect_request(&a.run_request, support, &a.effect_request)
        .map_err(|_| Error::invalid("invalid original effect request"))?;
    let events = &receipt.events;
    let mut executions = vec![];
    let mut mutations = vec![];
    let mut verified = vec![];
    let has_effect_decision = validate_decision_targets(receipt, &effect_hash)?;
    for event in events {
        match event {
            RunEvent::ToolExecution { .. } => executions.push(event),
            RunEvent::Mutation { .. } => mutations.push(event),
            RunEvent::Verification { .. } => verified.push(event),
            RunEvent::PlanRecorded { plan_digest, .. } => require(
                plan_digest == &a.proposal_digest,
                "receipt plan differs from the submission",
            )?,
            _ => {}
        }
    }
    require(
        executions.len() <= 1
            && mutations.len() <= 1
            && verified.into_iter().eq(verification.map(|v| &v.event)),
        "receipt contains extra execution, mutation or verification evidence",
    )?;
    if let Some(d) = dispatch {
        for decision in &d.decisions {
            require(
                events.get((decision.sequence() - 1) as usize) == Some(decision),
                "dispatch evidence differs from the original receipt",
            )?;
        }
        require(
            p.result.is_some(),
            "published dispatch lacks a sealed result",
        )?;
        if let Some(first) = executions.first() {
            require(
                first.sequence() > d.decisions.last().expect("validated decisions").sequence(),
                "receipt execution precedes dispatch gates",
            )?;
        }
    } else {
        require(
            receipt.terminal_status != AgentRunStatus::Completed
                && executions.is_empty()
                && mutations.is_empty(),
            "pre-dispatch receipt claims execution or completion",
        )?;
    }
    let floor = verification
        .map(|v| &v.usage_floor)
        .or_else(|| observation.map(|o| &o.usage_floor));
    if let Some(floor) = floor {
        usage_at_least(&receipt.usage, floor)?;
    }
    let Some(result) = &p.result else {
        return require(
            !has_effect_decision && executions.is_empty() && mutations.is_empty(),
            "receipt records an effect without its original result",
        );
    };
    verify_protected_effect_result(&a.run_request, support, &a.effect_request, result)
        .map_err(|_| Error::invalid("original protected effect result failed frozen validation"))?;
    let b = &result.body;
    require(events.iter().any(|e| matches!(e,
        RunEvent::ProtectedEffectDecision { decision_id, gate, protected_effect_digest, subject_digest, decision, .. }
        if decision_id == &b.decision.decision_id && gate == &b.decision.gate && protected_effect_digest == &effect_hash
           && subject_digest == &b.decision.subject_digest && decision == &b.decision.decision)),
        "receipt lacks the sealed result decision")?;
    let refused = matches!(
        b.execution_status,
        EffectExecutionStatus::AwaitingAuthority | EffectExecutionStatus::Denied
    );
    require(
        dispatch.is_some() || refused,
        "pre-dispatch result claims an attempted executor",
    )?;
    if refused {
        require(
            executions.is_empty()
                && mutations.is_empty()
                && observation.is_none_or(|o| o.disposition != Disposition::Replaced),
            "unexecuted refusal accompanies an observed write or tool execution",
        )?;
    } else {
        let d = dispatch.ok_or_else(|| Error::invalid("missing dispatch"))?;
        require(
            b.executor.as_ref() == Some(&d.executor)
                && b.observed_pre_effect_subject == d.before.subject
                && b.observed_capability == a.effect_request.capability
                && b.observed_tool_schema_digest == a.effect_request.tool_schema_digest,
            "result invocation differs from the recorded dispatch",
        )?;
        require(
            executions.len() == 1 && bound_execution(executions[0], result, a),
            "tool execution lacks the exact result binding",
        )?;
        for reference in &b.evidence {
            archive.bytes(
                &d.executor.name,
                reference.locator.as_deref(),
                &reference.digest,
            )?;
        }
        let used = ResourceUsage {
            cost_micros: b.usage.cost_micros,
            elapsed_ms: b.usage.elapsed_ms,
            model_tokens: b.usage.model_tokens,
            tool_calls: b.usage.tool_calls,
        };
        usage_at_least(&receipt.usage, &used)?;
        match &b.observed_post_effect_subject {
            Some(post) => require(
                mutations.len() == 1
                    && matches!(mutations[0],
                RunEvent::Mutation { decision_id, protected_effect_digest, before_subject_digest, after_subject_digest, evidence, .. }
                if matches!(d.decisions.last(), Some(RunEvent::ProtectedEffectDecision { decision_id: id, .. }) if id == decision_id)
                && protected_effect_digest == &effect_hash && before_subject_digest == &d.before.subject.digest
                && after_subject_digest == &post.digest && evidence.iter().any(|reference|
                    reference.evidence_type == EvidenceType::Artifact && b.evidence.iter().any(|original|
                      original.evidence_type == EffectEvidenceType::Artifact && original.digest == reference.digest
                      && original.locator == reference.locator))),
                "receipt mutation differs from the sealed effect result",
            )?,
            None => require(
                mutations.is_empty(),
                "receipt invents an unobserved mutation",
            )?,
        }
    }
    if let Some(o) = observation {
        match o.disposition {
            Disposition::NotStarted | Disposition::StoppedBeforeRename => require(
                b.execution_status != EffectExecutionStatus::Executed,
                "executed result contradicts observed non-execution",
            )?,
            Disposition::Replaced => {
                let after = o.after.as_ref().expect("validated replacement");
                require(
                    b.observed_post_effect_subject
                        .as_ref()
                        .is_none_or(|s| s == &after.subject),
                    "result post-effect subject contradicts observed replacement",
                )?;
            }
            Disposition::Unresolved => {}
        }
    }
    if receipt.terminal_status == AgentRunStatus::Completed {
        require(
            observation.is_some_and(|o| o.disposition == Disposition::Replaced)
                && matches!(
                    verification.map(|v| &v.event),
                    Some(RunEvent::Verification {
                        verdict: VerificationVerdict::Pass,
                        ..
                    })
                )
                && b.execution_status == EffectExecutionStatus::Executed,
            "completion lacks an observed replacement, passing verifier or executed result",
        )?;
    }
    Ok(())
}

/// During execution a workflow decision targets the effect. During verification it
/// targets the final subject, including when the coordinator refuses completion.
/// The frozen receipt validator separately checks passing verification before an allow.
pub(super) fn validate_decision_targets(
    body: &TerminalRunReceiptBody,
    effect_hash: &str,
) -> Result<bool> {
    let mut phase = AgentRunStatus::Accepted;
    let mut has_effect_decision = false;
    for event in &body.events {
        match event {
            RunEvent::StatusTransition { to, .. } => phase = *to,
            RunEvent::ProtectedEffectDecision {
                gate,
                protected_effect_digest,
                decision,
                subject_digest,
                ..
            } => {
                if *gate == Gate::Workflow && phase == AgentRunStatus::Verifying {
                    require(
                        subject_digest == &body.resulting_subject_digest
                            && protected_effect_digest == subject_digest
                            && (decision.outcome != Outcome::Allow
                                || decision.code == "workflow.completion_authorized"),
                        "completion decision differs from the final subject or authorization",
                    )?;
                } else {
                    require(
                        protected_effect_digest == effect_hash
                            && decision.code != "workflow.completion_authorized",
                        "receipt describes another effect or an out-of-phase completion",
                    )?;
                    has_effect_decision = true;
                }
            }
            _ => {}
        }
    }
    Ok(has_effect_decision)
}

fn bound_execution(event: &RunEvent, result: &ProtectedEffectResult, a: &Attempt) -> bool {
    let b = &result.body;
    let locator = format!("gaap:protected-effect-result/{}", b.effect_id);
    matches!(event, RunEvent::ToolExecution { decision_id, protected_effect_digest, action_digest, capability_digest, evidence, .. }
        if decision_id == &b.decision.decision_id && protected_effect_digest == &b.effect_request_digest
        && action_digest == &b.effect_request_digest && capability_digest == &a.effect_request.capability.digest
        && evidence.iter().any(|e| e.evidence_type == EvidenceType::ToolExecution && e.digest == result.result_digest
            && e.locator.as_deref() == Some(locator.as_str())))
}
