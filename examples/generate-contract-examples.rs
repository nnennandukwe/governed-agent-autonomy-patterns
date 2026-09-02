use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use gaap::contracts::{
    AGENT_RUN_REQUEST_SCHEMA, AgentRunRequest, AgentRunStatus, ApprovalReference,
    CapabilityIdentity, ContractSupport, EvidenceReference, EvidenceType, PolicyIdentity,
    ResourceBudget, ResourceUsage, RunEvent, Subject, SubjectKind, TERMINAL_RUN_RECEIPT_SCHEMA,
    TaskSpec, TerminalRunReceipt, TerminalRunReceiptBody, VerificationIndependence,
    VerificationRequirement, VerificationVerdict, seal_terminal_receipt, validate_request,
    verify_terminal_receipt,
};
use gaap::{Decision, Gate, Outcome};
use serde::Serialize;

const DIRECTORY: &str = "examples/contracts/v0.1.0";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("contract example generation failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let arguments: Vec<String> = env::args().skip(1).collect();
    let check = match arguments.as_slice() {
        [] => false,
        [argument] if argument == "--check" => true,
        _ => {
            return Err(
                "usage: cargo run --locked --example generate-contract-examples -- [--check]"
                    .to_owned(),
            );
        }
    };

    let request = request();
    let support = ContractSupport::new([policy()]);
    let request_digest = validate_request(&request, &support).map_err(|error| error.to_string())?;
    let receipts = [
        (
            "completed.json",
            completed(&request, &support, &request_digest)?,
        ),
        (
            "blocked.json",
            blocked(&request, &support, &request_digest)?,
        ),
        ("failed.json", failed(&request, &support, &request_digest)?),
        (
            "interrupted.json",
            interrupted(&request, &support, &request_digest)?,
        ),
        (
            "budget-exhausted.json",
            budget_exhausted(&request, &support, &request_digest)?,
        ),
        (
            "denied-effect.json",
            denied_effect(&request, &support, &request_digest)?,
        ),
        (
            "stale-verification.json",
            stale_verification(&request, &support, &request_digest)?,
        ),
    ];

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    materialize(&root, "agent-run-request.json", &request, check)?;
    for (file_name, receipt) in receipts {
        verify_terminal_receipt(&request, &support, &receipt)
            .map_err(|error| format!("{file_name} does not verify: {error}"))?;
        materialize(&root, file_name, &receipt, check)?;
    }
    Ok(())
}

fn materialize<T: Serialize>(
    root: &Path,
    file_name: &str,
    value: &T,
    check: bool,
) -> Result<(), String> {
    let relative_path = format!("{DIRECTORY}/{file_name}");
    let path = root.join(&relative_path);
    let contents = format!(
        "{}\n",
        serde_json::to_string_pretty(value)
            .map_err(|error| format!("could not serialize {relative_path}: {error}"))?
    );
    if check {
        let actual = fs::read_to_string(&path)
            .map_err(|error| format!("could not read {relative_path}: {error}"))?;
        if actual != contents {
            return Err(format!(
                "{relative_path} is stale; run `cargo run --locked --example generate-contract-examples`"
            ));
        }
        println!("example is current: {relative_path}");
    } else {
        let parent = path
            .parent()
            .ok_or_else(|| format!("example path has no parent: {relative_path}"))?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
        fs::write(&path, contents)
            .map_err(|error| format!("could not write {relative_path}: {error}"))?;
        println!("wrote example: {relative_path}");
    }
    Ok(())
}

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
        scope: "subject".to_owned(),
        subject_digest: digest('a'),
        evidence: evidence(EvidenceType::Approval, 'e'),
    }
}

fn evidence(evidence_type: EvidenceType, byte: char) -> EvidenceReference {
    EvidenceReference {
        evidence_type,
        digest: digest(byte),
        locator: None,
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

fn status(
    sequence: u64,
    from: AgentRunStatus,
    to: AgentRunStatus,
    reason: Option<&str>,
) -> RunEvent {
    RunEvent::StatusTransition {
        sequence,
        from,
        to,
        reason: reason.map(str::to_owned),
    }
}

fn decision(
    sequence: u64,
    decision_id: &str,
    gate: Gate,
    protected_effect_digest: String,
    subject_digest: String,
    outcome: Outcome,
    code: &str,
) -> RunEvent {
    RunEvent::ProtectedEffectDecision {
        sequence,
        decision_id: decision_id.to_owned(),
        gate,
        protected_effect_digest,
        subject_digest,
        decision: Decision {
            outcome,
            code: code.to_owned(),
            effects: vec![],
        },
    }
}

fn body(
    request_digest: &str,
    resulting_subject_digest: String,
    terminal_status: AgentRunStatus,
    terminal_reason: &str,
    usage: ResourceUsage,
    events: Vec<RunEvent>,
) -> TerminalRunReceiptBody {
    TerminalRunReceiptBody {
        schema_version: TERMINAL_RUN_RECEIPT_SCHEMA.to_owned(),
        request_id: "request-001".to_owned(),
        run_id: "run-001".to_owned(),
        request_digest: request_digest.to_owned(),
        initial_subject_digest: digest('a'),
        resulting_subject_digest,
        terminal_status,
        terminal_reason: terminal_reason.to_owned(),
        usage,
        events,
    }
}

fn seal(
    request: &AgentRunRequest,
    support: &ContractSupport,
    body: TerminalRunReceiptBody,
) -> Result<TerminalRunReceipt, String> {
    seal_terminal_receipt(request, support, body).map_err(|error| error.to_string())
}

fn completed(
    request: &AgentRunRequest,
    support: &ContractSupport,
    request_digest: &str,
) -> Result<TerminalRunReceipt, String> {
    let initial = digest('a');
    let resulting = digest('f');
    let effect = digest('7');
    let final_usage = usage();
    seal(
        request,
        support,
        body(
            request_digest,
            resulting.clone(),
            AgentRunStatus::Completed,
            "workflow.completion_authorized",
            final_usage.clone(),
            vec![
                status(1, AgentRunStatus::Accepted, AgentRunStatus::Planning, None),
                RunEvent::PlanRecorded {
                    sequence: 2,
                    plan_digest: digest('d'),
                },
                RunEvent::ApprovalRecorded {
                    sequence: 3,
                    approval: approval(),
                },
                status(4, AgentRunStatus::Planning, AgentRunStatus::Executing, None),
                decision(
                    5,
                    "decision-tool-001",
                    Gate::Permission,
                    effect.clone(),
                    initial.clone(),
                    Outcome::Allow,
                    "permission.policy_allowed",
                ),
                RunEvent::ToolExecution {
                    sequence: 6,
                    decision_id: "decision-tool-001".to_owned(),
                    protected_effect_digest: effect.clone(),
                    action_digest: digest('8'),
                    capability_digest: digest('c'),
                    evidence: vec![evidence(EvidenceType::ToolExecution, '9')],
                },
                decision(
                    7,
                    "decision-mutation-001",
                    Gate::Workflow,
                    effect.clone(),
                    initial.clone(),
                    Outcome::Allow,
                    "workflow.mutation_authorized",
                ),
                RunEvent::Mutation {
                    sequence: 8,
                    decision_id: "decision-mutation-001".to_owned(),
                    protected_effect_digest: effect,
                    before_subject_digest: initial,
                    after_subject_digest: resulting.clone(),
                    evidence: vec![evidence(EvidenceType::Artifact, '0')],
                },
                RunEvent::Usage {
                    sequence: 9,
                    usage: final_usage,
                },
                status(
                    10,
                    AgentRunStatus::Executing,
                    AgentRunStatus::Verifying,
                    None,
                ),
                RunEvent::Verification {
                    sequence: 11,
                    subject_digest: resulting.clone(),
                    implementer_id: "executor-001".to_owned(),
                    verifier_id: "verifier-001".to_owned(),
                    verdict: VerificationVerdict::Pass,
                    evidence: vec![
                        evidence(EvidenceType::CommandOutput, '1'),
                        evidence(EvidenceType::Artifact, '2'),
                    ],
                },
                RunEvent::ProtectedEffectDecision {
                    sequence: 12,
                    decision_id: "decision-completion-001".to_owned(),
                    gate: Gate::Workflow,
                    protected_effect_digest: resulting.clone(),
                    subject_digest: resulting,
                    decision: Decision {
                        outcome: Outcome::Allow,
                        code: "workflow.completion_authorized".to_owned(),
                        effects: vec!["record_completion".to_owned()],
                    },
                },
                status(
                    13,
                    AgentRunStatus::Verifying,
                    AgentRunStatus::Completed,
                    Some("workflow.completion_authorized"),
                ),
            ],
        ),
    )
}

fn blocked(
    request: &AgentRunRequest,
    support: &ContractSupport,
    request_digest: &str,
) -> Result<TerminalRunReceipt, String> {
    let initial = digest('a');
    let effect = digest('7');
    seal(
        request,
        support,
        body(
            request_digest,
            initial.clone(),
            AgentRunStatus::Blocked,
            "authority.required",
            zero_usage(),
            vec![
                status(1, AgentRunStatus::Accepted, AgentRunStatus::Planning, None),
                decision(
                    2,
                    "decision-authority-001",
                    Gate::Permission,
                    effect,
                    initial,
                    Outcome::Ask,
                    "permission.policy_requires_approval",
                ),
                status(
                    3,
                    AgentRunStatus::Planning,
                    AgentRunStatus::AwaitingAuthority,
                    Some("authority.required"),
                ),
                RunEvent::Usage {
                    sequence: 4,
                    usage: zero_usage(),
                },
                status(
                    5,
                    AgentRunStatus::AwaitingAuthority,
                    AgentRunStatus::Blocked,
                    Some("authority.required"),
                ),
            ],
        ),
    )
}

fn failed(
    request: &AgentRunRequest,
    support: &ContractSupport,
    request_digest: &str,
) -> Result<TerminalRunReceipt, String> {
    seal(
        request,
        support,
        body(
            request_digest,
            digest('a'),
            AgentRunStatus::Failed,
            "execution.failed",
            zero_usage(),
            vec![
                status(1, AgentRunStatus::Accepted, AgentRunStatus::Planning, None),
                RunEvent::Usage {
                    sequence: 2,
                    usage: zero_usage(),
                },
                status(
                    3,
                    AgentRunStatus::Planning,
                    AgentRunStatus::Failed,
                    Some("execution.failed"),
                ),
            ],
        ),
    )
}

fn interrupted(
    request: &AgentRunRequest,
    support: &ContractSupport,
    request_digest: &str,
) -> Result<TerminalRunReceipt, String> {
    seal(
        request,
        support,
        body(
            request_digest,
            digest('a'),
            AgentRunStatus::Interrupted,
            "operator.interrupted",
            zero_usage(),
            vec![
                status(1, AgentRunStatus::Accepted, AgentRunStatus::Planning, None),
                RunEvent::Interruption {
                    sequence: 2,
                    actor_id: Some("operator-001".to_owned()),
                    reason: "operator.interrupted".to_owned(),
                    evidence: evidence(EvidenceType::Interruption, '4'),
                },
                RunEvent::Usage {
                    sequence: 3,
                    usage: zero_usage(),
                },
                status(
                    4,
                    AgentRunStatus::Planning,
                    AgentRunStatus::Interrupted,
                    Some("operator.interrupted"),
                ),
            ],
        ),
    )
}

fn budget_exhausted(
    request: &AgentRunRequest,
    support: &ContractSupport,
    request_digest: &str,
) -> Result<TerminalRunReceipt, String> {
    let initial = digest('a');
    let final_usage = ResourceUsage {
        cost_micros: request.resource_budget.max_cost_micros,
        elapsed_ms: request.resource_budget.max_elapsed_ms,
        model_tokens: request.resource_budget.max_model_tokens,
        tool_calls: request.resource_budget.max_tool_calls,
    };
    seal(
        request,
        support,
        body(
            request_digest,
            initial.clone(),
            AgentRunStatus::Blocked,
            "runtime.hard_stop",
            final_usage.clone(),
            vec![
                status(1, AgentRunStatus::Accepted, AgentRunStatus::Planning, None),
                status(2, AgentRunStatus::Planning, AgentRunStatus::Executing, None),
                decision(
                    3,
                    "decision-budget-001",
                    Gate::Runtime,
                    digest('7'),
                    initial,
                    Outcome::Block,
                    "runtime.hard_stop",
                ),
                RunEvent::Usage {
                    sequence: 4,
                    usage: final_usage,
                },
                status(
                    5,
                    AgentRunStatus::Executing,
                    AgentRunStatus::Blocked,
                    Some("runtime.hard_stop"),
                ),
            ],
        ),
    )
}

fn denied_effect(
    request: &AgentRunRequest,
    support: &ContractSupport,
    request_digest: &str,
) -> Result<TerminalRunReceipt, String> {
    let initial = digest('a');
    seal(
        request,
        support,
        body(
            request_digest,
            initial.clone(),
            AgentRunStatus::Blocked,
            "permission.denied",
            zero_usage(),
            vec![
                status(1, AgentRunStatus::Accepted, AgentRunStatus::Planning, None),
                status(2, AgentRunStatus::Planning, AgentRunStatus::Executing, None),
                decision(
                    3,
                    "decision-denied-001",
                    Gate::Permission,
                    digest('7'),
                    initial,
                    Outcome::Block,
                    "permission.denied",
                ),
                RunEvent::Usage {
                    sequence: 4,
                    usage: zero_usage(),
                },
                status(
                    5,
                    AgentRunStatus::Executing,
                    AgentRunStatus::Blocked,
                    Some("permission.denied"),
                ),
            ],
        ),
    )
}

fn stale_verification(
    request: &AgentRunRequest,
    support: &ContractSupport,
    request_digest: &str,
) -> Result<TerminalRunReceipt, String> {
    let initial = digest('a');
    let verified = digest('f');
    let resulting = digest('6');
    let first_effect = digest('7');
    let second_effect = digest('5');
    let final_usage = usage();
    seal(
        request,
        support,
        body(
            request_digest,
            resulting.clone(),
            AgentRunStatus::Blocked,
            "verification.stale_subject",
            final_usage.clone(),
            vec![
                status(1, AgentRunStatus::Accepted, AgentRunStatus::Planning, None),
                RunEvent::PlanRecorded {
                    sequence: 2,
                    plan_digest: digest('d'),
                },
                RunEvent::ApprovalRecorded {
                    sequence: 3,
                    approval: approval(),
                },
                status(4, AgentRunStatus::Planning, AgentRunStatus::Executing, None),
                decision(
                    5,
                    "decision-mutation-001",
                    Gate::Workflow,
                    first_effect.clone(),
                    initial.clone(),
                    Outcome::Allow,
                    "workflow.mutation_authorized",
                ),
                RunEvent::Mutation {
                    sequence: 6,
                    decision_id: "decision-mutation-001".to_owned(),
                    protected_effect_digest: first_effect,
                    before_subject_digest: initial,
                    after_subject_digest: verified.clone(),
                    evidence: vec![evidence(EvidenceType::Artifact, '0')],
                },
                status(
                    7,
                    AgentRunStatus::Executing,
                    AgentRunStatus::Verifying,
                    None,
                ),
                RunEvent::Verification {
                    sequence: 8,
                    subject_digest: verified.clone(),
                    implementer_id: "executor-001".to_owned(),
                    verifier_id: "verifier-001".to_owned(),
                    verdict: VerificationVerdict::Pass,
                    evidence: vec![
                        evidence(EvidenceType::CommandOutput, '1'),
                        evidence(EvidenceType::Artifact, '2'),
                    ],
                },
                status(
                    9,
                    AgentRunStatus::Verifying,
                    AgentRunStatus::Executing,
                    None,
                ),
                decision(
                    10,
                    "decision-mutation-002",
                    Gate::Workflow,
                    second_effect.clone(),
                    verified.clone(),
                    Outcome::Allow,
                    "workflow.mutation_authorized",
                ),
                RunEvent::Mutation {
                    sequence: 11,
                    decision_id: "decision-mutation-002".to_owned(),
                    protected_effect_digest: second_effect,
                    before_subject_digest: verified,
                    after_subject_digest: resulting,
                    evidence: vec![evidence(EvidenceType::Artifact, '3')],
                },
                RunEvent::Usage {
                    sequence: 12,
                    usage: final_usage,
                },
                status(
                    13,
                    AgentRunStatus::Executing,
                    AgentRunStatus::Blocked,
                    Some("verification.stale_subject"),
                ),
            ],
        ),
    )
}
