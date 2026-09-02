use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use gaap::contracts::{
    AgentRunRequest, ApprovalReference, ContractSupport, EffectClass, EffectEvidenceReference,
    EffectEvidenceType, EffectExecutionStatus, EffectExit, EffectUsage, EvidenceReference,
    EvidenceType, ExecutorIdentity, FilesystemAccess, InputMetadataEntry, NetworkProtocol,
    OperationFamily, PROTECTED_EFFECT_REQUEST_SCHEMA, PROTECTED_EFFECT_RESULT_SCHEMA,
    ProtectedEffectDecision, ProtectedEffectRequest, ProtectedEffectResult,
    ProtectedEffectResultBody, Repeatability, RequestedScope, SandboxProfileIdentity,
    canonical_resource_budget_digest, parse_agent_run_request_json, seal_protected_effect_result,
    validate_protected_effect_request, verify_protected_effect_result,
};
use gaap::{Decision, Gate, Outcome};
use serde::Serialize;
use sha2::{Digest, Sha256};

const AGENT_RUN_REQUEST_PATH: &str = "examples/contracts/v0.1.0/agent-run-request.json";
const DIRECTORY: &str = "examples/contracts/protected-effect/v0.1.0";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("protected effect example generation failed: {error}");
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
                "usage: cargo run --locked --example generate-protected-effect-examples -- [--check]"
                    .to_owned(),
            );
        }
    };

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let agent_run_request = read_agent_run_request(&root)?;
    let support = ContractSupport::new(agent_run_request.policies.clone());
    let request = protected_effect_request(&agent_run_request)?;
    let request_digest = validate_protected_effect_request(&agent_run_request, &support, &request)
        .map_err(|error| error.to_string())?;
    let results = [
        (
            "completed.json",
            completed(&agent_run_request, &support, &request, &request_digest)?,
        ),
        (
            "denied.json",
            denied(&agent_run_request, &support, &request, &request_digest)?,
        ),
        (
            "awaiting-authority.json",
            awaiting_authority(&agent_run_request, &support, &request, &request_digest)?,
        ),
        (
            "failed.json",
            failed(&agent_run_request, &support, &request, &request_digest)?,
        ),
        (
            "interrupted.json",
            interrupted(&agent_run_request, &support, &request, &request_digest)?,
        ),
        (
            "stale-subject.json",
            stale_subject(&agent_run_request, &support, &request, &request_digest)?,
        ),
        (
            "schema-drift.json",
            schema_drift(&agent_run_request, &support, &request, &request_digest)?,
        ),
        (
            "unknown-outcome.json",
            unknown_outcome(&agent_run_request, &support, &request, &request_digest)?,
        ),
    ];

    materialize(&root, "protected-effect-request.json", &request, check)?;
    for (file_name, result) in results {
        verify_protected_effect_result(&agent_run_request, &support, &request, &result)
            .map_err(|error| format!("{file_name} does not verify: {error}"))?;
        materialize(&root, file_name, &result, check)?;
    }
    Ok(())
}

fn read_agent_run_request(root: &Path) -> Result<AgentRunRequest, String> {
    let bytes = fs::read(root.join(AGENT_RUN_REQUEST_PATH))
        .map_err(|error| format!("could not read {AGENT_RUN_REQUEST_PATH}: {error}"))?;
    parse_agent_run_request_json(&bytes)
        .map_err(|error| format!("{AGENT_RUN_REQUEST_PATH} is invalid: {error}"))
}

fn protected_effect_request(
    agent_run_request: &AgentRunRequest,
) -> Result<ProtectedEffectRequest, String> {
    let agent_run_request_digest = gaap::contracts::validate_request(
        agent_run_request,
        &ContractSupport::new(agent_run_request.policies.clone()),
    )
    .map_err(|error| error.to_string())?;
    let resource_budget_digest =
        canonical_resource_budget_digest(&agent_run_request.resource_budget)
            .map_err(|error| error.to_string())?;
    let normalized_input = serde_json::json!({
        "arguments": ["test", "--locked"],
        "executable": "/usr/bin/cargo"
    });
    let input_bytes = serde_jcs::to_vec(&normalized_input).map_err(|error| error.to_string())?;

    Ok(ProtectedEffectRequest {
        schema_version: PROTECTED_EFFECT_REQUEST_SCHEMA.to_owned(),
        effect_id: "effect-001".to_owned(),
        effect_sequence: 1,
        run_id: agent_run_request.run_id.clone(),
        agent_run_request_digest,
        subject: agent_run_request.subject.clone(),
        operation_family: OperationFamily::Process,
        normalized_operation: "process.spawn".to_owned(),
        capability: agent_run_request.requested_capability.clone(),
        tool_schema_digest: Some(digest('d')),
        input_digest: format!("sha256:{:x}", Sha256::digest(input_bytes)),
        input_metadata: vec![
            InputMetadataEntry {
                name: "argument_count".to_owned(),
                value: "2".to_owned(),
            },
            InputMetadataEntry {
                name: "content_type".to_owned(),
                value: "application/json".to_owned(),
            },
        ],
        requested_scopes: vec![
            RequestedScope::Process {
                executable: "/usr/bin/cargo".to_owned(),
                working_directory: "/workspace".to_owned(),
            },
            RequestedScope::Filesystem {
                root: "/workspace".to_owned(),
                access: vec![FilesystemAccess::Read, FilesystemAccess::Modify],
                recursive: true,
            },
            RequestedScope::Network {
                protocol: NetworkProtocol::Https,
                host: "crates.io".to_owned(),
                port: 443,
            },
            RequestedScope::ExternalService {
                service: "github".to_owned(),
                operation: "pull_request.comment".to_owned(),
                resource: "nnennandukwe/governed-agent-autonomy-patterns#22".to_owned(),
            },
        ],
        policies: agent_run_request.policies.clone(),
        approval_context: vec![ApprovalReference {
            approval_id: "approval-effect-001".to_owned(),
            actor_id: "maintainer-001".to_owned(),
            scope: "protected_effect".to_owned(),
            subject_digest: agent_run_request.subject.digest.clone(),
            evidence: EvidenceReference {
                evidence_type: EvidenceType::Approval,
                digest: digest('e'),
                locator: None,
            },
        }],
        resource_budget_digest,
        sandbox_profile: SandboxProfileIdentity {
            name: "local-process".to_owned(),
            version: "0.1.0".to_owned(),
            digest: digest('f'),
        },
        idempotency_key: "run-001/effect-001".to_owned(),
        repeatability: Repeatability::NonRepeatable,
        expected_effect_class: EffectClass::Mutation,
    })
}

fn completed(
    agent_run_request: &AgentRunRequest,
    support: &ContractSupport,
    request: &ProtectedEffectRequest,
    request_digest: &str,
) -> Result<ProtectedEffectResult, String> {
    let mut body = attempted_body(
        request,
        request_digest,
        EffectExecutionStatus::Executed,
        None,
    );
    body.exit = Some(EffectExit::Code { code: 0 });
    body.evidence.extend([
        evidence(EffectEvidenceType::Exit, '0'),
        evidence(EffectEvidenceType::Output, '1'),
        evidence(EffectEvidenceType::Artifact, '2'),
        evidence(EffectEvidenceType::Mutation, '3'),
        evidence(EffectEvidenceType::SubjectObservation, '7'),
    ]);
    seal(agent_run_request, support, request, body)
}

fn denied(
    agent_run_request: &AgentRunRequest,
    support: &ContractSupport,
    request: &ProtectedEffectRequest,
    request_digest: &str,
) -> Result<ProtectedEffectResult, String> {
    seal(
        agent_run_request,
        support,
        request,
        non_execution_body(
            request,
            request_digest,
            EffectExecutionStatus::Denied,
            Outcome::Block,
            "permission.denied",
        ),
    )
}

fn awaiting_authority(
    agent_run_request: &AgentRunRequest,
    support: &ContractSupport,
    request: &ProtectedEffectRequest,
    request_digest: &str,
) -> Result<ProtectedEffectResult, String> {
    seal(
        agent_run_request,
        support,
        request,
        non_execution_body(
            request,
            request_digest,
            EffectExecutionStatus::AwaitingAuthority,
            Outcome::Ask,
            "permission.policy_requires_approval",
        ),
    )
}

fn failed(
    agent_run_request: &AgentRunRequest,
    support: &ContractSupport,
    request: &ProtectedEffectRequest,
    request_digest: &str,
) -> Result<ProtectedEffectResult, String> {
    let mut body = attempted_body(
        request,
        request_digest,
        EffectExecutionStatus::Failed,
        Some("process exited unsuccessfully"),
    );
    body.exit = Some(EffectExit::Code { code: 1 });
    body.evidence.extend([
        evidence(EffectEvidenceType::Exit, '0'),
        evidence(EffectEvidenceType::Output, '1'),
        evidence(EffectEvidenceType::Artifact, '2'),
        evidence(EffectEvidenceType::Mutation, '3'),
        evidence(EffectEvidenceType::Failure, '8'),
        evidence(EffectEvidenceType::SubjectObservation, '7'),
    ]);
    seal(agent_run_request, support, request, body)
}

fn interrupted(
    agent_run_request: &AgentRunRequest,
    support: &ContractSupport,
    request: &ProtectedEffectRequest,
    request_digest: &str,
) -> Result<ProtectedEffectResult, String> {
    let mut body = attempted_body(
        request,
        request_digest,
        EffectExecutionStatus::Interrupted,
        Some("operator interrupted the process"),
    );
    body.exit = Some(EffectExit::Signal {
        signal: "SIGTERM".to_owned(),
    });
    body.evidence.extend([
        evidence(EffectEvidenceType::Exit, '0'),
        evidence(EffectEvidenceType::Artifact, '2'),
        evidence(EffectEvidenceType::Mutation, '3'),
        evidence(EffectEvidenceType::Interruption, '8'),
        evidence(EffectEvidenceType::SubjectObservation, '7'),
    ]);
    seal(agent_run_request, support, request, body)
}

fn stale_subject(
    agent_run_request: &AgentRunRequest,
    support: &ContractSupport,
    request: &ProtectedEffectRequest,
    request_digest: &str,
) -> Result<ProtectedEffectResult, String> {
    let mut body = non_execution_body(
        request,
        request_digest,
        EffectExecutionStatus::Denied,
        Outcome::Block,
        "protected_effect.stale_subject",
    );
    body.observed_pre_effect_subject.digest = digest('8');
    body.decision.subject_digest = digest('8');
    body.evidence = vec![evidence(EffectEvidenceType::SubjectObservation, '8')];
    seal(agent_run_request, support, request, body)
}

fn schema_drift(
    agent_run_request: &AgentRunRequest,
    support: &ContractSupport,
    request: &ProtectedEffectRequest,
    request_digest: &str,
) -> Result<ProtectedEffectResult, String> {
    let mut body = non_execution_body(
        request,
        request_digest,
        EffectExecutionStatus::Denied,
        Outcome::Block,
        "protected_effect.capability_schema_drift",
    );
    body.observed_tool_schema_digest = Some(digest('8'));
    body.evidence = vec![evidence(EffectEvidenceType::CapabilitySchema, '8')];
    seal(agent_run_request, support, request, body)
}

fn unknown_outcome(
    agent_run_request: &AgentRunRequest,
    support: &ContractSupport,
    request: &ProtectedEffectRequest,
    request_digest: &str,
) -> Result<ProtectedEffectResult, String> {
    let body = attempted_body(
        request,
        request_digest,
        EffectExecutionStatus::UnknownOutcome,
        Some("executor disconnected before reconciliation"),
    );
    seal(agent_run_request, support, request, body)
}

fn attempted_body(
    request: &ProtectedEffectRequest,
    request_digest: &str,
    status: EffectExecutionStatus,
    reason: Option<&str>,
) -> ProtectedEffectResultBody {
    let mut post_subject = request.subject.clone();
    post_subject.digest = digest('9');
    let unknown = status == EffectExecutionStatus::UnknownOutcome;
    let mut evidence_refs = vec![
        evidence(EffectEvidenceType::Usage, '4'),
        evidence(EffectEvidenceType::Executor, '5'),
        evidence(EffectEvidenceType::Sandbox, '6'),
    ];
    if unknown {
        evidence_refs.push(evidence(EffectEvidenceType::UnknownOutcome, '8'));
    }

    ProtectedEffectResultBody {
        schema_version: PROTECTED_EFFECT_RESULT_SCHEMA.to_owned(),
        effect_id: request.effect_id.clone(),
        effect_sequence: request.effect_sequence,
        run_id: request.run_id.clone(),
        agent_run_request_digest: request.agent_run_request_digest.clone(),
        effect_request_digest: request_digest.to_owned(),
        observed_pre_effect_subject: request.subject.clone(),
        observed_capability: request.capability.clone(),
        observed_tool_schema_digest: request.tool_schema_digest.clone(),
        decision: decision(Outcome::Allow, request_digest, &request.subject.digest),
        execution_status: status,
        observed_post_effect_subject: (!unknown).then_some(post_subject),
        exit: None,
        usage: EffectUsage {
            cost_micros: 200_000,
            elapsed_ms: 2_000,
            model_tokens: 0,
            tool_calls: 1,
        },
        executor: Some(ExecutorIdentity {
            name: "local-runner".to_owned(),
            version: "0.1.0".to_owned(),
            digest: digest('6'),
        }),
        sandbox_profile: Some(request.sandbox_profile.clone()),
        reason: reason.map(str::to_owned),
        evidence: evidence_refs,
    }
}

fn non_execution_body(
    request: &ProtectedEffectRequest,
    request_digest: &str,
    status: EffectExecutionStatus,
    outcome: Outcome,
    reason: &str,
) -> ProtectedEffectResultBody {
    ProtectedEffectResultBody {
        schema_version: PROTECTED_EFFECT_RESULT_SCHEMA.to_owned(),
        effect_id: request.effect_id.clone(),
        effect_sequence: request.effect_sequence,
        run_id: request.run_id.clone(),
        agent_run_request_digest: request.agent_run_request_digest.clone(),
        effect_request_digest: request_digest.to_owned(),
        observed_pre_effect_subject: request.subject.clone(),
        observed_capability: request.capability.clone(),
        observed_tool_schema_digest: request.tool_schema_digest.clone(),
        decision: decision(outcome, request_digest, &request.subject.digest),
        execution_status: status,
        observed_post_effect_subject: None,
        exit: None,
        usage: EffectUsage {
            cost_micros: 0,
            elapsed_ms: 0,
            model_tokens: 0,
            tool_calls: 0,
        },
        executor: None,
        sandbox_profile: None,
        reason: Some(reason.to_owned()),
        evidence: vec![],
    }
}

fn decision(
    outcome: Outcome,
    request_digest: &str,
    subject_digest: &str,
) -> ProtectedEffectDecision {
    let code = match outcome {
        Outcome::Allow => "permission.policy_allowed",
        Outcome::Ask => "permission.policy_requires_approval",
        Outcome::Block => "permission.denied",
    };
    ProtectedEffectDecision {
        decision_id: "decision-001".to_owned(),
        gate: Gate::Permission,
        effect_request_digest: request_digest.to_owned(),
        subject_digest: subject_digest.to_owned(),
        decision: Decision {
            outcome,
            code: code.to_owned(),
            effects: vec![],
        },
    }
}

fn evidence(evidence_type: EffectEvidenceType, byte: char) -> EffectEvidenceReference {
    EffectEvidenceReference {
        evidence_type,
        digest: digest(byte),
        locator: None,
    }
}

fn seal(
    agent_run_request: &AgentRunRequest,
    support: &ContractSupport,
    request: &ProtectedEffectRequest,
    body: ProtectedEffectResultBody,
) -> Result<ProtectedEffectResult, String> {
    seal_protected_effect_result(agent_run_request, support, request, body)
        .map_err(|error| error.to_string())
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
                "{relative_path} is stale; run `cargo run --locked --example generate-protected-effect-examples`"
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
