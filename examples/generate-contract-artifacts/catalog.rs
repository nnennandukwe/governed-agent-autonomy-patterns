use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use gaap::contracts::{
    AgentRunStatus, ContractSupport, EffectExecutionStatus, agent_run_request_schema,
    parse_agent_run_request_json, parse_protected_effect_request_json,
    parse_protected_effect_result_json, parse_terminal_run_receipt_json,
    protected_effect_request_schema, protected_effect_result_schema, terminal_run_receipt_schema,
    validate_protected_effect_request, validate_request, verify_protected_effect_result,
    verify_terminal_receipt,
};
use serde_json::Value;

use super::agent_run;
use super::protected_effect;
use super::reconcile::{ArtifactError, DurableState, Phase, RenderedArtifact};

pub(crate) const AGENT_RUN_REQUEST_SCHEMA_PATH: &str =
    "schemas/agent-run/v0.1.0/agent-run-request.schema.json";
pub(crate) const TERMINAL_RUN_RECEIPT_SCHEMA_PATH: &str =
    "schemas/agent-run/v0.1.0/terminal-run-receipt.schema.json";
pub(crate) const PROTECTED_EFFECT_REQUEST_SCHEMA_PATH: &str =
    "schemas/protected-effect/v0.1.0/protected-effect-request.schema.json";
pub(crate) const PROTECTED_EFFECT_RESULT_SCHEMA_PATH: &str =
    "schemas/protected-effect/v0.1.0/protected-effect-result.schema.json";
pub(crate) const AGENT_RUN_REQUEST_EXAMPLE_PATH: &str =
    "examples/contracts/v0.1.0/agent-run-request.json";
pub(crate) const PROTECTED_EFFECT_REQUEST_EXAMPLE_PATH: &str =
    "examples/contracts/protected-effect/v0.1.0/protected-effect-request.json";

pub(crate) type BuildArtifact = fn(&ArtifactStore) -> Result<BuiltArtifact, String>;

#[derive(Debug)]
pub(crate) struct BuiltArtifact {
    pub value: Value,
    pub contents: Vec<u8>,
}

impl BuiltArtifact {
    pub(crate) fn serialize(value: &impl serde::Serialize) -> Result<Self, String> {
        let value_tree = serde_json::to_value(value).map_err(|error| error.to_string())?;
        let mut contents = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
        contents.push(b'\n');
        Ok(Self {
            value: value_tree,
            contents,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArtifactKind {
    Schema,
    Example,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SemanticExpectation {
    Schema,
    AgentRunRequest,
    TerminalRunReceipt {
        request_path: &'static str,
        status: AgentRunStatus,
        reason: &'static str,
    },
    ProtectedEffectRequest {
        agent_request_path: &'static str,
    },
    ProtectedEffectResult {
        agent_request_path: &'static str,
        effect_request_path: &'static str,
        status: EffectExecutionStatus,
        reason: Option<&'static str>,
    },
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ArtifactSpec {
    pub name: &'static str,
    pub path: &'static str,
    pub kind: ArtifactKind,
    pub schema_path: Option<&'static str>,
    pub dependencies: &'static [&'static str],
    pub build: BuildArtifact,
    pub expectation: SemanticExpectation,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ContractVersionSpec<'a> {
    pub family: &'static str,
    pub version: &'static str,
    pub artifacts: &'a [ArtifactSpec],
}

#[derive(Debug, Default)]
pub(crate) struct ArtifactStore {
    values: BTreeMap<&'static str, Value>,
}

impl ArtifactStore {
    pub(crate) fn get(&self, path: &str) -> Result<&Value, String> {
        self.values
            .get(path)
            .ok_or_else(|| format!("artifact dependency is unavailable: {path}"))
    }

    fn insert(&mut self, path: &'static str, value: Value) {
        self.values.insert(path, value);
    }
}

const fn schema(name: &'static str, path: &'static str, build: BuildArtifact) -> ArtifactSpec {
    ArtifactSpec {
        name,
        path,
        kind: ArtifactKind::Schema,
        schema_path: None,
        dependencies: &[],
        build,
        expectation: SemanticExpectation::Schema,
    }
}

const fn example(
    name: &'static str,
    path: &'static str,
    schema_path: &'static str,
    dependencies: &'static [&'static str],
    build: BuildArtifact,
    expectation: SemanticExpectation,
) -> ArtifactSpec {
    ArtifactSpec {
        name,
        path,
        kind: ArtifactKind::Example,
        schema_path: Some(schema_path),
        dependencies,
        build,
        expectation,
    }
}

fn build_agent_run_request_schema(_: &ArtifactStore) -> Result<BuiltArtifact, String> {
    BuiltArtifact::serialize(&agent_run_request_schema())
}

fn build_terminal_run_receipt_schema(_: &ArtifactStore) -> Result<BuiltArtifact, String> {
    BuiltArtifact::serialize(&terminal_run_receipt_schema())
}

fn build_protected_effect_request_schema(_: &ArtifactStore) -> Result<BuiltArtifact, String> {
    BuiltArtifact::serialize(&protected_effect_request_schema())
}

fn build_protected_effect_result_schema(_: &ArtifactStore) -> Result<BuiltArtifact, String> {
    BuiltArtifact::serialize(&protected_effect_result_schema())
}

const AGENT_RUN_ARTIFACTS: &[ArtifactSpec] = &[
    schema(
        "agent-run-request-schema",
        AGENT_RUN_REQUEST_SCHEMA_PATH,
        build_agent_run_request_schema,
    ),
    schema(
        "terminal-run-receipt-schema",
        TERMINAL_RUN_RECEIPT_SCHEMA_PATH,
        build_terminal_run_receipt_schema,
    ),
    example(
        "agent-run-request",
        AGENT_RUN_REQUEST_EXAMPLE_PATH,
        AGENT_RUN_REQUEST_SCHEMA_PATH,
        &[AGENT_RUN_REQUEST_SCHEMA_PATH],
        agent_run::build_request,
        SemanticExpectation::AgentRunRequest,
    ),
    example(
        "completed",
        "examples/contracts/v0.1.0/completed.json",
        TERMINAL_RUN_RECEIPT_SCHEMA_PATH,
        &[
            TERMINAL_RUN_RECEIPT_SCHEMA_PATH,
            AGENT_RUN_REQUEST_EXAMPLE_PATH,
        ],
        agent_run::build_completed,
        SemanticExpectation::TerminalRunReceipt {
            request_path: AGENT_RUN_REQUEST_EXAMPLE_PATH,
            status: AgentRunStatus::Completed,
            reason: "workflow.completion_authorized",
        },
    ),
    example(
        "blocked",
        "examples/contracts/v0.1.0/blocked.json",
        TERMINAL_RUN_RECEIPT_SCHEMA_PATH,
        &[
            TERMINAL_RUN_RECEIPT_SCHEMA_PATH,
            AGENT_RUN_REQUEST_EXAMPLE_PATH,
        ],
        agent_run::build_blocked,
        SemanticExpectation::TerminalRunReceipt {
            request_path: AGENT_RUN_REQUEST_EXAMPLE_PATH,
            status: AgentRunStatus::Blocked,
            reason: "authority.required",
        },
    ),
    example(
        "failed",
        "examples/contracts/v0.1.0/failed.json",
        TERMINAL_RUN_RECEIPT_SCHEMA_PATH,
        &[
            TERMINAL_RUN_RECEIPT_SCHEMA_PATH,
            AGENT_RUN_REQUEST_EXAMPLE_PATH,
        ],
        agent_run::build_failed,
        SemanticExpectation::TerminalRunReceipt {
            request_path: AGENT_RUN_REQUEST_EXAMPLE_PATH,
            status: AgentRunStatus::Failed,
            reason: "execution.failed",
        },
    ),
    example(
        "interrupted",
        "examples/contracts/v0.1.0/interrupted.json",
        TERMINAL_RUN_RECEIPT_SCHEMA_PATH,
        &[
            TERMINAL_RUN_RECEIPT_SCHEMA_PATH,
            AGENT_RUN_REQUEST_EXAMPLE_PATH,
        ],
        agent_run::build_interrupted,
        SemanticExpectation::TerminalRunReceipt {
            request_path: AGENT_RUN_REQUEST_EXAMPLE_PATH,
            status: AgentRunStatus::Interrupted,
            reason: "operator.interrupted",
        },
    ),
    example(
        "budget-exhausted",
        "examples/contracts/v0.1.0/budget-exhausted.json",
        TERMINAL_RUN_RECEIPT_SCHEMA_PATH,
        &[
            TERMINAL_RUN_RECEIPT_SCHEMA_PATH,
            AGENT_RUN_REQUEST_EXAMPLE_PATH,
        ],
        agent_run::build_budget_exhausted,
        SemanticExpectation::TerminalRunReceipt {
            request_path: AGENT_RUN_REQUEST_EXAMPLE_PATH,
            status: AgentRunStatus::Blocked,
            reason: "runtime.hard_stop",
        },
    ),
    example(
        "denied-effect",
        "examples/contracts/v0.1.0/denied-effect.json",
        TERMINAL_RUN_RECEIPT_SCHEMA_PATH,
        &[
            TERMINAL_RUN_RECEIPT_SCHEMA_PATH,
            AGENT_RUN_REQUEST_EXAMPLE_PATH,
        ],
        agent_run::build_denied_effect,
        SemanticExpectation::TerminalRunReceipt {
            request_path: AGENT_RUN_REQUEST_EXAMPLE_PATH,
            status: AgentRunStatus::Blocked,
            reason: "permission.denied",
        },
    ),
    example(
        "stale-verification",
        "examples/contracts/v0.1.0/stale-verification.json",
        TERMINAL_RUN_RECEIPT_SCHEMA_PATH,
        &[
            TERMINAL_RUN_RECEIPT_SCHEMA_PATH,
            AGENT_RUN_REQUEST_EXAMPLE_PATH,
        ],
        agent_run::build_stale_verification,
        SemanticExpectation::TerminalRunReceipt {
            request_path: AGENT_RUN_REQUEST_EXAMPLE_PATH,
            status: AgentRunStatus::Blocked,
            reason: "verification.stale_subject",
        },
    ),
];

const PROTECTED_EFFECT_ARTIFACTS: &[ArtifactSpec] = &[
    schema(
        "protected-effect-request-schema",
        PROTECTED_EFFECT_REQUEST_SCHEMA_PATH,
        build_protected_effect_request_schema,
    ),
    schema(
        "protected-effect-result-schema",
        PROTECTED_EFFECT_RESULT_SCHEMA_PATH,
        build_protected_effect_result_schema,
    ),
    example(
        "protected-effect-request",
        PROTECTED_EFFECT_REQUEST_EXAMPLE_PATH,
        PROTECTED_EFFECT_REQUEST_SCHEMA_PATH,
        &[
            PROTECTED_EFFECT_REQUEST_SCHEMA_PATH,
            AGENT_RUN_REQUEST_EXAMPLE_PATH,
        ],
        protected_effect::build_request,
        SemanticExpectation::ProtectedEffectRequest {
            agent_request_path: AGENT_RUN_REQUEST_EXAMPLE_PATH,
        },
    ),
    example(
        "completed",
        "examples/contracts/protected-effect/v0.1.0/completed.json",
        PROTECTED_EFFECT_RESULT_SCHEMA_PATH,
        &[
            PROTECTED_EFFECT_RESULT_SCHEMA_PATH,
            AGENT_RUN_REQUEST_EXAMPLE_PATH,
            PROTECTED_EFFECT_REQUEST_EXAMPLE_PATH,
        ],
        protected_effect::build_completed,
        SemanticExpectation::ProtectedEffectResult {
            agent_request_path: AGENT_RUN_REQUEST_EXAMPLE_PATH,
            effect_request_path: PROTECTED_EFFECT_REQUEST_EXAMPLE_PATH,
            status: EffectExecutionStatus::Executed,
            reason: None,
        },
    ),
    example(
        "denied",
        "examples/contracts/protected-effect/v0.1.0/denied.json",
        PROTECTED_EFFECT_RESULT_SCHEMA_PATH,
        &[
            PROTECTED_EFFECT_RESULT_SCHEMA_PATH,
            AGENT_RUN_REQUEST_EXAMPLE_PATH,
            PROTECTED_EFFECT_REQUEST_EXAMPLE_PATH,
        ],
        protected_effect::build_denied,
        SemanticExpectation::ProtectedEffectResult {
            agent_request_path: AGENT_RUN_REQUEST_EXAMPLE_PATH,
            effect_request_path: PROTECTED_EFFECT_REQUEST_EXAMPLE_PATH,
            status: EffectExecutionStatus::Denied,
            reason: Some("permission.denied"),
        },
    ),
    example(
        "awaiting-authority",
        "examples/contracts/protected-effect/v0.1.0/awaiting-authority.json",
        PROTECTED_EFFECT_RESULT_SCHEMA_PATH,
        &[
            PROTECTED_EFFECT_RESULT_SCHEMA_PATH,
            AGENT_RUN_REQUEST_EXAMPLE_PATH,
            PROTECTED_EFFECT_REQUEST_EXAMPLE_PATH,
        ],
        protected_effect::build_awaiting_authority,
        SemanticExpectation::ProtectedEffectResult {
            agent_request_path: AGENT_RUN_REQUEST_EXAMPLE_PATH,
            effect_request_path: PROTECTED_EFFECT_REQUEST_EXAMPLE_PATH,
            status: EffectExecutionStatus::AwaitingAuthority,
            reason: Some("permission.policy_requires_approval"),
        },
    ),
    example(
        "failed",
        "examples/contracts/protected-effect/v0.1.0/failed.json",
        PROTECTED_EFFECT_RESULT_SCHEMA_PATH,
        &[
            PROTECTED_EFFECT_RESULT_SCHEMA_PATH,
            AGENT_RUN_REQUEST_EXAMPLE_PATH,
            PROTECTED_EFFECT_REQUEST_EXAMPLE_PATH,
        ],
        protected_effect::build_failed,
        SemanticExpectation::ProtectedEffectResult {
            agent_request_path: AGENT_RUN_REQUEST_EXAMPLE_PATH,
            effect_request_path: PROTECTED_EFFECT_REQUEST_EXAMPLE_PATH,
            status: EffectExecutionStatus::Failed,
            reason: Some("process exited unsuccessfully"),
        },
    ),
    example(
        "interrupted",
        "examples/contracts/protected-effect/v0.1.0/interrupted.json",
        PROTECTED_EFFECT_RESULT_SCHEMA_PATH,
        &[
            PROTECTED_EFFECT_RESULT_SCHEMA_PATH,
            AGENT_RUN_REQUEST_EXAMPLE_PATH,
            PROTECTED_EFFECT_REQUEST_EXAMPLE_PATH,
        ],
        protected_effect::build_interrupted,
        SemanticExpectation::ProtectedEffectResult {
            agent_request_path: AGENT_RUN_REQUEST_EXAMPLE_PATH,
            effect_request_path: PROTECTED_EFFECT_REQUEST_EXAMPLE_PATH,
            status: EffectExecutionStatus::Interrupted,
            reason: Some("operator interrupted the process"),
        },
    ),
    example(
        "stale-subject",
        "examples/contracts/protected-effect/v0.1.0/stale-subject.json",
        PROTECTED_EFFECT_RESULT_SCHEMA_PATH,
        &[
            PROTECTED_EFFECT_RESULT_SCHEMA_PATH,
            AGENT_RUN_REQUEST_EXAMPLE_PATH,
            PROTECTED_EFFECT_REQUEST_EXAMPLE_PATH,
        ],
        protected_effect::build_stale_subject,
        SemanticExpectation::ProtectedEffectResult {
            agent_request_path: AGENT_RUN_REQUEST_EXAMPLE_PATH,
            effect_request_path: PROTECTED_EFFECT_REQUEST_EXAMPLE_PATH,
            status: EffectExecutionStatus::Denied,
            reason: Some("protected_effect.stale_subject"),
        },
    ),
    example(
        "schema-drift",
        "examples/contracts/protected-effect/v0.1.0/schema-drift.json",
        PROTECTED_EFFECT_RESULT_SCHEMA_PATH,
        &[
            PROTECTED_EFFECT_RESULT_SCHEMA_PATH,
            AGENT_RUN_REQUEST_EXAMPLE_PATH,
            PROTECTED_EFFECT_REQUEST_EXAMPLE_PATH,
        ],
        protected_effect::build_schema_drift,
        SemanticExpectation::ProtectedEffectResult {
            agent_request_path: AGENT_RUN_REQUEST_EXAMPLE_PATH,
            effect_request_path: PROTECTED_EFFECT_REQUEST_EXAMPLE_PATH,
            status: EffectExecutionStatus::Denied,
            reason: Some("protected_effect.capability_schema_drift"),
        },
    ),
    example(
        "unknown-outcome",
        "examples/contracts/protected-effect/v0.1.0/unknown-outcome.json",
        PROTECTED_EFFECT_RESULT_SCHEMA_PATH,
        &[
            PROTECTED_EFFECT_RESULT_SCHEMA_PATH,
            AGENT_RUN_REQUEST_EXAMPLE_PATH,
            PROTECTED_EFFECT_REQUEST_EXAMPLE_PATH,
        ],
        protected_effect::build_unknown_outcome,
        SemanticExpectation::ProtectedEffectResult {
            agent_request_path: AGENT_RUN_REQUEST_EXAMPLE_PATH,
            effect_request_path: PROTECTED_EFFECT_REQUEST_EXAMPLE_PATH,
            status: EffectExecutionStatus::UnknownOutcome,
            reason: Some("executor disconnected before reconciliation"),
        },
    ),
];

pub(crate) const CONTRACTS: &[ContractVersionSpec<'static>] = &[
    ContractVersionSpec {
        family: "agent-run",
        version: "0.1.0",
        artifacts: AGENT_RUN_ARTIFACTS,
    },
    ContractVersionSpec {
        family: "protected-effect",
        version: "0.1.0",
        artifacts: PROTECTED_EFFECT_ARTIFACTS,
    },
];

pub(crate) fn render_catalog(
    contracts: &[ContractVersionSpec<'_>],
) -> Result<Vec<RenderedArtifact>, ArtifactError> {
    if let Err(problems) = validate_catalog(contracts) {
        return Err(precommit_error(Phase::Catalog, problems));
    }

    let mut remaining: BTreeMap<&str, &ArtifactSpec> = contracts
        .iter()
        .flat_map(|contract| contract.artifacts)
        .map(|artifact| (artifact.path, artifact))
        .collect();
    let mut store = ArtifactStore::default();
    let mut rendered = Vec::new();
    let mut problems = Vec::new();

    loop {
        let ready: Vec<&str> = remaining
            .iter()
            .filter(|(_, artifact)| {
                artifact
                    .dependencies
                    .iter()
                    .all(|dependency| store.values.contains_key(dependency))
            })
            .map(|(path, _)| *path)
            .collect();
        if ready.is_empty() {
            break;
        }

        for path in ready {
            let artifact = remaining
                .remove(path)
                .expect("ready artifacts must remain registered");
            match render_artifact(artifact, &store) {
                Ok((value, output)) => {
                    store.insert(artifact.path, value);
                    rendered.push(output);
                }
                Err(problem) => problems.push(problem),
            }
        }
    }

    for artifact in remaining.values() {
        let unavailable = artifact
            .dependencies
            .iter()
            .filter(|dependency| !store.values.contains_key(*dependency))
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
        problems.push(format!(
            "{} could not render because dependencies are unavailable: {unavailable}",
            artifact.path
        ));
    }

    if !problems.is_empty() {
        problems.sort();
        problems.dedup();
        return Err(precommit_error(Phase::Render, problems));
    }

    rendered.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(rendered)
}

fn render_artifact(
    artifact: &ArtifactSpec,
    store: &ArtifactStore,
) -> Result<(Value, RenderedArtifact), String> {
    let built = (artifact.build)(store)
        .map_err(|error| format!("{} could not render: {error}", artifact.path))?;
    let rendered_value: Value = serde_json::from_slice(&built.contents)
        .map_err(|error| format!("{} rendered invalid JSON: {error}", artifact.path))?;
    if rendered_value != built.value {
        return Err(format!(
            "{} rendered bytes disagree with its validation value",
            artifact.path
        ));
    }

    if let Some(schema_path) = artifact.schema_path {
        let schema = store.get(schema_path)?;
        let validator = jsonschema::validator_for(schema)
            .map_err(|error| format!("{schema_path} could not compile: {error}"))?;
        validator
            .validate(&built.value)
            .map_err(|error| format!("{} does not match {schema_path}: {error}", artifact.path))?;
    }
    validate_semantics(artifact, store, &built.value)?;

    Ok((
        built.value,
        RenderedArtifact {
            path: artifact.path.to_owned(),
            contents: built.contents,
        },
    ))
}

fn validate_semantics(
    artifact: &ArtifactSpec,
    store: &ArtifactStore,
    value: &Value,
) -> Result<(), String> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| format!("{} could not serialize: {error}", artifact.path))?;
    match artifact.expectation {
        SemanticExpectation::Schema => {
            jsonschema::validator_for(value)
                .map_err(|error| format!("{} is not a valid schema: {error}", artifact.path))?;
            let schema_id = value
                .get("$id")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("{} has no string $id", artifact.path))?;
            if !schema_id.ends_with(artifact.path) {
                return Err(format!(
                    "{} has an unexpected $id: {schema_id}",
                    artifact.path
                ));
            }
        }
        SemanticExpectation::AgentRunRequest => {
            let request = parse_agent_run_request_json(&bytes)
                .map_err(|error| format!("{} does not parse: {error}", artifact.path))?;
            let support = ContractSupport::new(request.policies.clone());
            validate_request(&request, &support)
                .map_err(|error| format!("{} does not validate: {error}", artifact.path))?;
        }
        SemanticExpectation::TerminalRunReceipt {
            request_path,
            status,
            reason,
        } => {
            let request_bytes = serde_json::to_vec(store.get(request_path)?)
                .map_err(|error| format!("{request_path} could not serialize: {error}"))?;
            let request = parse_agent_run_request_json(&request_bytes)
                .map_err(|error| format!("{request_path} does not parse: {error}"))?;
            let support = ContractSupport::new(request.policies.clone());
            let receipt = parse_terminal_run_receipt_json(&bytes)
                .map_err(|error| format!("{} does not parse: {error}", artifact.path))?;
            verify_terminal_receipt(&request, &support, &receipt)
                .map_err(|error| format!("{} does not verify: {error}", artifact.path))?;
            if receipt.body.terminal_status != status || receipt.body.terminal_reason != reason {
                return Err(format!(
                    "{} has unexpected terminal status or reason",
                    artifact.path
                ));
            }
        }
        SemanticExpectation::ProtectedEffectRequest { agent_request_path } => {
            let agent_bytes = serde_json::to_vec(store.get(agent_request_path)?)
                .map_err(|error| format!("{agent_request_path} could not serialize: {error}"))?;
            let agent_request = parse_agent_run_request_json(&agent_bytes)
                .map_err(|error| format!("{agent_request_path} does not parse: {error}"))?;
            let support = ContractSupport::new(agent_request.policies.clone());
            let request = parse_protected_effect_request_json(&bytes)
                .map_err(|error| format!("{} does not parse: {error}", artifact.path))?;
            validate_protected_effect_request(&agent_request, &support, &request)
                .map_err(|error| format!("{} does not validate: {error}", artifact.path))?;
        }
        SemanticExpectation::ProtectedEffectResult {
            agent_request_path,
            effect_request_path,
            status,
            reason,
        } => {
            let agent_bytes = serde_json::to_vec(store.get(agent_request_path)?)
                .map_err(|error| format!("{agent_request_path} could not serialize: {error}"))?;
            let effect_bytes = serde_json::to_vec(store.get(effect_request_path)?)
                .map_err(|error| format!("{effect_request_path} could not serialize: {error}"))?;
            let agent_request = parse_agent_run_request_json(&agent_bytes)
                .map_err(|error| format!("{agent_request_path} does not parse: {error}"))?;
            let effect_request = parse_protected_effect_request_json(&effect_bytes)
                .map_err(|error| format!("{effect_request_path} does not parse: {error}"))?;
            let support = ContractSupport::new(agent_request.policies.clone());
            let result = parse_protected_effect_result_json(&bytes)
                .map_err(|error| format!("{} does not parse: {error}", artifact.path))?;
            verify_protected_effect_result(&agent_request, &support, &effect_request, &result)
                .map_err(|error| format!("{} does not verify: {error}", artifact.path))?;
            if result.body.execution_status != status || result.body.reason.as_deref() != reason {
                return Err(format!(
                    "{} has unexpected execution status or reason",
                    artifact.path
                ));
            }
        }
    }
    Ok(())
}

fn precommit_error(phase: Phase, problems: Vec<String>) -> ArtifactError {
    ArtifactError {
        phase,
        state: DurableState::Unchanged,
        problems,
        recovery: "fix the catalog or renderer; no managed artifacts were changed",
    }
}

pub(crate) fn validate_catalog(contracts: &[ContractVersionSpec<'_>]) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    let mut contract_keys = BTreeSet::new();
    let mut artifact_paths = BTreeSet::new();
    let mut artifact_kinds = BTreeMap::new();
    let mut artifact_names = BTreeSet::new();

    for contract in contracts {
        let contract_key = format!("{}/{}", contract.family, contract.version);
        if contract.family.is_empty() || contract.version.is_empty() {
            errors.push(format!(
                "contract family and version must be non-empty: {contract_key}"
            ));
        }
        if !contract_keys.insert(contract_key.clone()) {
            errors.push(format!("duplicate contract version: {contract_key}"));
        }

        for artifact in contract.artifacts {
            if artifact.name.is_empty() {
                errors.push(format!(
                    "artifact name must be non-empty: {}",
                    artifact.path
                ));
            }
            if !artifact_paths.insert(artifact.path) {
                errors.push(format!("duplicate artifact path: {}", artifact.path));
            } else {
                artifact_kinds.insert(artifact.path, artifact.kind);
            }
            if !artifact_names.insert((contract_key.clone(), artifact.name)) {
                errors.push(format!(
                    "duplicate artifact name in {contract_key}: {}",
                    artifact.name
                ));
            }
            validate_artifact_path(contract, artifact, &mut errors);
        }
    }

    for contract in contracts {
        for artifact in contract.artifacts {
            for dependency in artifact.dependencies {
                if !artifact_paths.contains(dependency) {
                    errors.push(format!(
                        "artifact {} dependency is not registered: {dependency}",
                        artifact.path
                    ));
                }
                if dependency == &artifact.path {
                    errors.push(format!(
                        "artifact cannot depend on itself: {}",
                        artifact.path
                    ));
                }
            }

            match (artifact.kind, artifact.schema_path) {
                (ArtifactKind::Schema, Some(schema_path)) => errors.push(format!(
                    "schema artifact {} cannot declare schema association {schema_path}",
                    artifact.path
                )),
                (ArtifactKind::Example, None) => errors.push(format!(
                    "example artifact {} must declare a schema association",
                    artifact.path
                )),
                (ArtifactKind::Example, Some(schema_path)) => {
                    if !artifact_paths.contains(schema_path) {
                        errors.push(format!(
                            "artifact {} schema is not registered: {schema_path}",
                            artifact.path
                        ));
                    }
                    if artifact_kinds.get(schema_path) == Some(&ArtifactKind::Example) {
                        errors.push(format!(
                            "artifact {} schema association is not a schema: {schema_path}",
                            artifact.path
                        ));
                    }
                    if !artifact.dependencies.contains(&schema_path) {
                        errors.push(format!(
                            "artifact {} must declare schema dependency {schema_path}",
                            artifact.path
                        ));
                    }
                }
                (ArtifactKind::Schema, None) => {}
            }

            match (artifact.kind, artifact.expectation) {
                (ArtifactKind::Schema, SemanticExpectation::Schema) => {}
                (ArtifactKind::Schema, _) => errors.push(format!(
                    "schema artifact {} has example semantic metadata",
                    artifact.path
                )),
                (ArtifactKind::Example, SemanticExpectation::Schema) => errors.push(format!(
                    "example artifact {} has schema semantic metadata",
                    artifact.path
                )),
                (ArtifactKind::Example, expectation) => {
                    for dependency in semantic_dependencies(expectation) {
                        if !artifact.dependencies.contains(&dependency) {
                            errors.push(format!(
                                "artifact {} must declare semantic dependency {dependency}",
                                artifact.path
                            ));
                        }
                    }
                }
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn semantic_dependencies(expectation: SemanticExpectation) -> Vec<&'static str> {
    match expectation {
        SemanticExpectation::Schema | SemanticExpectation::AgentRunRequest => vec![],
        SemanticExpectation::TerminalRunReceipt { request_path, .. } => vec![request_path],
        SemanticExpectation::ProtectedEffectRequest { agent_request_path } => {
            vec![agent_request_path]
        }
        SemanticExpectation::ProtectedEffectResult {
            agent_request_path,
            effect_request_path,
            ..
        } => vec![agent_request_path, effect_request_path],
    }
}

fn validate_artifact_path(
    contract: &ContractVersionSpec<'_>,
    artifact: &ArtifactSpec,
    errors: &mut Vec<String>,
) {
    let path = Path::new(artifact.path);
    let managed =
        artifact.path.starts_with("schemas/") || artifact.path.starts_with("examples/contracts/");
    let safe_components = path
        .components()
        .all(|component| matches!(component, Component::Normal(_) | Component::CurDir));
    let expected_version_segment = format!("/v{}/", contract.version);
    let versioned = format!("/{}", artifact.path).contains(&expected_version_segment);
    let json = path
        .extension()
        .is_some_and(|extension| extension == "json");

    if path.is_absolute() || !managed || !safe_components || !versioned || !json {
        errors.push(format!(
            "artifact path is outside its managed version root: {}",
            artifact.path
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY: &[&str] = &[];

    fn empty_json(_: &ArtifactStore) -> Result<BuiltArtifact, String> {
        BuiltArtifact::serialize(&serde_json::json!({}))
    }

    const SCHEMA: ArtifactSpec = ArtifactSpec {
        name: "request-schema",
        path: "schemas/example/v0.1.0/request.schema.json",
        kind: ArtifactKind::Schema,
        schema_path: None,
        dependencies: EMPTY,
        build: empty_json,
        expectation: SemanticExpectation::Schema,
    };

    const EXAMPLE: ArtifactSpec = ArtifactSpec {
        name: "request-example",
        path: "examples/contracts/example/v0.1.0/request.json",
        kind: ArtifactKind::Example,
        schema_path: Some(SCHEMA.path),
        dependencies: &[SCHEMA.path],
        build: empty_json,
        expectation: SemanticExpectation::AgentRunRequest,
    };

    #[test]
    fn duplicate_contract_versions_are_rejected() {
        let artifacts = [SCHEMA];
        let contracts = [
            ContractVersionSpec {
                family: "example",
                version: "0.1.0",
                artifacts: &artifacts,
            },
            ContractVersionSpec {
                family: "example",
                version: "0.1.0",
                artifacts: &[],
            },
        ];

        let errors = validate_catalog(&contracts).expect_err("duplicate version must fail");

        assert!(errors.iter().any(|error| error.contains("example/0.1.0")));
    }

    #[test]
    fn duplicate_artifact_paths_are_rejected() {
        let duplicate = ArtifactSpec {
            name: "other-schema",
            ..SCHEMA
        };
        let artifacts = [SCHEMA, duplicate];
        let contracts = [ContractVersionSpec {
            family: "example",
            version: "0.1.0",
            artifacts: &artifacts,
        }];

        let errors = validate_catalog(&contracts).expect_err("duplicate path must fail");

        assert!(errors.iter().any(|error| error.contains(SCHEMA.path)));
    }

    #[test]
    fn duplicate_scenario_names_are_rejected_within_a_contract_version() {
        let duplicate = ArtifactSpec {
            path: "schemas/example/v0.1.0/other.schema.json",
            ..SCHEMA
        };
        let artifacts = [SCHEMA, duplicate];
        let contracts = [ContractVersionSpec {
            family: "example",
            version: "0.1.0",
            artifacts: &artifacts,
        }];

        let errors = validate_catalog(&contracts).expect_err("duplicate scenario must fail");

        assert!(
            errors
                .iter()
                .any(|error| error.contains("duplicate artifact name"))
        );
    }

    #[test]
    fn unknown_dependencies_and_schema_associations_are_rejected() {
        let example = ArtifactSpec {
            schema_path: Some("schemas/example/v0.1.0/missing.schema.json"),
            dependencies: &["examples/contracts/example/v0.1.0/missing.json"],
            ..EXAMPLE
        };
        let artifacts = [SCHEMA, example];
        let contracts = [ContractVersionSpec {
            family: "example",
            version: "0.1.0",
            artifacts: &artifacts,
        }];

        let errors = validate_catalog(&contracts).expect_err("unknown references must fail");

        assert_eq!(
            errors
                .iter()
                .filter(|error| error.contains("is not registered"))
                .count(),
            2
        );
    }

    #[test]
    fn paths_outside_managed_roots_are_rejected() {
        let artifacts = [
            ArtifactSpec {
                path: "/tmp/request.json",
                ..SCHEMA
            },
            ArtifactSpec {
                path: "schemas/../Cargo.toml",
                ..SCHEMA
            },
            ArtifactSpec {
                path: "docs/request.json",
                ..SCHEMA
            },
        ];
        let contracts = [ContractVersionSpec {
            family: "example",
            version: "0.1.0",
            artifacts: &artifacts,
        }];

        let errors = validate_catalog(&contracts).expect_err("unsafe paths must fail");

        assert_eq!(
            errors
                .iter()
                .filter(|error| error.contains("outside its managed version root"))
                .count(),
            3
        );
    }

    #[test]
    fn example_schema_must_be_a_declared_dependency() {
        let example = ArtifactSpec {
            dependencies: EMPTY,
            ..EXAMPLE
        };
        let artifacts = [SCHEMA, example];
        let contracts = [ContractVersionSpec {
            family: "example",
            version: "0.1.0",
            artifacts: &artifacts,
        }];

        let errors = validate_catalog(&contracts).expect_err("schema dependency must fail");

        assert!(
            errors
                .iter()
                .any(|error| error.contains("schema dependency"))
        );
    }

    #[test]
    fn scenario_semantics_must_name_declared_dependencies() {
        let example = ArtifactSpec {
            expectation: SemanticExpectation::TerminalRunReceipt {
                request_path: "examples/contracts/example/v0.1.0/parent.json",
                status: AgentRunStatus::Completed,
                reason: "complete",
            },
            ..EXAMPLE
        };
        let artifacts = [SCHEMA, example];
        let contracts = [ContractVersionSpec {
            family: "example",
            version: "0.1.0",
            artifacts: &artifacts,
        }];

        let errors = validate_catalog(&contracts).expect_err("metadata mismatch must fail");

        assert!(
            errors
                .iter()
                .any(|error| error.contains("semantic dependency"))
        );
    }

    #[test]
    fn rendering_reports_all_independent_builder_failures() {
        fn fail(_: &ArtifactStore) -> Result<BuiltArtifact, String> {
            Err("intentional render failure".to_owned())
        }
        let artifacts = [
            ArtifactSpec {
                build: fail,
                ..SCHEMA
            },
            ArtifactSpec {
                name: "other-schema",
                path: "schemas/example/v0.1.0/other.schema.json",
                build: fail,
                ..SCHEMA
            },
        ];
        let contracts = [ContractVersionSpec {
            family: "example",
            version: "0.1.0",
            artifacts: &artifacts,
        }];

        let error = render_catalog(&contracts).expect_err("builder failures must fail rendering");

        assert_eq!(error.phase, Phase::Render);
        assert_eq!(error.state, DurableState::Unchanged);
        assert_eq!(
            error
                .problems
                .iter()
                .filter(|problem| problem.contains("intentional render failure"))
                .count(),
            2
        );
    }

    #[test]
    fn rendered_bytes_must_match_the_value_that_was_validated() {
        fn disagree(_: &ArtifactStore) -> Result<BuiltArtifact, String> {
            Ok(BuiltArtifact {
                value: serde_json::json!({}),
                contents: b"[]\n".to_vec(),
            })
        }
        let artifacts = [ArtifactSpec {
            build: disagree,
            ..SCHEMA
        }];
        let contracts = [ContractVersionSpec {
            family: "example",
            version: "0.1.0",
            artifacts: &artifacts,
        }];

        let error = render_catalog(&contracts).expect_err("split representations must fail");

        assert!(error.problems[0].contains("rendered bytes disagree with its validation value"));
    }

    #[test]
    fn dependency_cycles_fail_before_reconciliation() {
        let artifacts = [
            ArtifactSpec {
                dependencies: &["schemas/example/v0.1.0/other.schema.json"],
                ..SCHEMA
            },
            ArtifactSpec {
                name: "other-schema",
                path: "schemas/example/v0.1.0/other.schema.json",
                dependencies: &[SCHEMA.path],
                ..SCHEMA
            },
        ];
        let contracts = [ContractVersionSpec {
            family: "example",
            version: "0.1.0",
            artifacts: &artifacts,
        }];

        let error = render_catalog(&contracts).expect_err("cycles must fail rendering");

        assert_eq!(error.phase, Phase::Render);
        assert_eq!(error.problems.len(), 2);
        assert!(
            error
                .problems
                .iter()
                .all(|problem| problem.contains("dependencies are unavailable"))
        );
    }

    #[test]
    fn protected_effect_request_consumes_the_in_memory_agent_request() {
        let mut store = ArtifactStore::default();
        let mut request = agent_run::build_request(&store).expect("request should build");
        request.value["run_id"] = Value::String("run-from-memory".to_owned());
        store.insert(AGENT_RUN_REQUEST_EXAMPLE_PATH, request.value);

        let effect = protected_effect::build_request(&store).expect("effect should build");

        assert_eq!(effect.value["run_id"], "run-from-memory");
    }

    #[test]
    fn canonical_catalog_renders_the_committed_artifact_set_byte_for_byte() {
        let rendered = render_catalog(CONTRACTS).expect("catalog should render");
        let schema_count = CONTRACTS
            .iter()
            .flat_map(|contract| contract.artifacts)
            .filter(|artifact| artifact.kind == ArtifactKind::Schema)
            .count();
        let example_count = CONTRACTS
            .iter()
            .flat_map(|contract| contract.artifacts)
            .filter(|artifact| artifact.kind == ArtifactKind::Example)
            .count();

        assert_eq!(schema_count, 4);
        assert_eq!(example_count, 17);
        assert_eq!(rendered.len(), 21);

        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        for artifact in rendered {
            let committed = std::fs::read(root.join(&artifact.path))
                .unwrap_or_else(|error| panic!("could not read {}: {error}", artifact.path));
            assert_eq!(
                artifact.contents, committed,
                "artifact drift: {}",
                artifact.path
            );
        }
    }
}
