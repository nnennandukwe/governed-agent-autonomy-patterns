//! Governed, bounded MCP stdio execution through GAAP's existing executor port.
#![forbid(unsafe_code)]

mod transport;
mod worker;

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use gaap::contracts::{
    AgentRunRequest, CapabilityIdentity, ContractSupport, EffectClass, EffectEvidenceReference,
    EffectEvidenceType, EffectExecutionStatus, EffectUsage, ExecutorIdentity, InputMetadataEntry,
    OperationFamily, PROTECTED_EFFECT_REQUEST_SCHEMA, ProtectedEffectRequest, Repeatability,
    RequestedScope, SandboxProfileIdentity, Subject, canonical_resource_budget_digest,
    validate_protected_effect_request, validate_request,
};
use gaap::runtime::{ExecutorObservation, ExecutorPort, PermissionPolicy, RuntimePortError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

pub const MCP_PROTOCOL_VERSION: &str = "2025-11-25";
pub const MCP_SDK_VERSION: &str = "3.5.1";

/// Host-approved bytes for a launch artifact, including the server and interpreter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchArtifact {
    pub path: PathBuf,
    pub digest: String,
}

/// Host-owned launch configuration. No ambient environment is inherited.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub executable: PathBuf,
    pub arguments: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub working_directory: PathBuf,
    pub expected_name: String,
    pub expected_version: String,
    /// Trusted host identity for the server package and its launch dependencies.
    pub artifact_digest: String,
    pub artifacts: Vec<LaunchArtifact>,
}

impl ServerConfig {
    pub fn new(executable: PathBuf, name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            executable,
            arguments: Vec::new(),
            environment: BTreeMap::new(),
            working_directory: PathBuf::new(),
            expected_name: name.into(),
            expected_version: version.into(),
            artifact_digest: String::new(),
            artifacts: Vec::new(),
        }
    }

    /// Pure manifest identity. The host approves expected digests independently.
    pub fn artifact_manifest_digest(&self) -> Result<String, McpError> {
        digest(&self.artifacts)
    }
}

/// Explicit bounds shared by discovery, invocation, transport and retained evidence.
#[derive(Debug, Clone, Copy)]
pub struct TransportLimits {
    pub timeout: Duration,
    pub max_frame_bytes: usize,
    pub max_stderr_bytes: usize,
    pub max_tools: usize,
    pub max_calls: usize,
    pub max_retained_bytes: usize,
}

impl Default for TransportLimits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            max_frame_bytes: 1024 * 1024,
            max_stderr_bytes: 64 * 1024,
            max_tools: 128,
            max_calls: 128,
            max_retained_bytes: 16 * 1024 * 1024,
        }
    }
}

/// A discovered descriptor, not a grant. Annotations never supply authority.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCapability {
    pub tool_name: String,
    pub capability: CapabilityIdentity,
    pub schema_digest: String,
    pub descriptor: Value,
}

/// Error codes describe the boundary crossed; recovery never implies replay safety.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpError {
    pub code: &'static str,
    pub detail: String,
}

impl McpError {
    pub(crate) fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for McpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {}; inspect retained evidence before retrying",
            self.code, self.detail
        )
    }
}
impl Error for McpError {}

/// Cancellation ends this session. It can't reopen an old terminal Agent Run.
#[derive(Clone)]
pub struct CancelHandle(CancellationToken);
impl CancelHandle {
    pub fn cancel(&self) {
        self.0.cancel();
    }
}

/// Independent host observations before and after dispatch, with measured usage.
#[derive(Debug, Clone)]
pub struct SubjectObservation {
    pub subject: Subject,
    /// Independent host evidence confirms the operation occurred; server success isn't proof.
    pub effect_confirmed: bool,
    /// Host-measured external cost. Unknown usage must return an error.
    pub cost_micros: u64,
    /// Host-measured tool model usage. Pure local fixture tools explicitly report zero.
    pub model_tokens: u64,
}

pub trait SubjectObserver: Send {
    fn observe(&mut self, effect: &ProtectedEffectRequest) -> Result<SubjectObservation, McpError>;
}

impl<F> SubjectObserver for F
where
    F: FnMut(&ProtectedEffectRequest) -> Result<SubjectObservation, McpError> + Send,
{
    fn observe(&mut self, effect: &ProtectedEffectRequest) -> Result<SubjectObservation, McpError> {
        self(effect)
    }
}

struct RunBinding {
    request: AgentRunRequest,
    support: ContractSupport,
    observer: Arc<Mutex<Box<dyn SubjectObserver>>>,
    deadline: Instant,
}

/// Exact invocation evidence retained in memory for independent host persistence.
#[derive(Debug, Clone, Serialize)]
pub struct CallEvidence {
    pub request_digest: String,
    pub capability_digest: String,
    pub protocol_version: String,
    pub sdk_version: String,
    pub status: EffectExecutionStatus,
    pub dispatched: bool,
    pub effect_confirmed: Option<bool>,
    pub observed_subject: Option<Subject>,
    pub host_usage: Option<EffectUsage>,
    pub output_digest: Option<String>,
    pub output: Option<Value>,
    pub output_omitted: bool,
    pub error: Option<String>,
}

pub(crate) enum WorkerCommand {
    Refresh(mpsc::Sender<Result<Vec<ToolCapability>, McpError>>),
    Invoke {
        effect: Box<ProtectedEffectRequest>,
        arguments: Value,
        approved: Vec<CapabilityIdentity>,
        policy: PermissionPolicy,
        observer: Arc<Mutex<Box<dyn SubjectObserver>>>,
        deadline: Instant,
        reply: mpsc::Sender<Result<worker::Invocation, McpError>>,
    },
    Stop,
}

/// A synchronous executor with its SDK session owned by a dedicated runtime thread.
/// It can be composed with AgentRunEngine, including from an existing Tokio host.
pub struct McpExecutor {
    sender: mpsc::SyncSender<WorkerCommand>,
    worker: Option<JoinHandle<()>>,
    tools: Vec<ToolCapability>,
    approved: Vec<CapabilityIdentity>,
    policy: PermissionPolicy,
    binding: Option<RunBinding>,
    prepared: BTreeMap<String, (String, Value)>,
    attempted: BTreeSet<String>,
    limits: TransportLimits,
    cancellation: CancellationToken,
    state: transport::SharedState,
    identity: ExecutorIdentity,
    server_digest: String,
    profile: SandboxProfileIdentity,
    calls: Vec<CallEvidence>,
    retained_bytes: usize,
}

impl McpExecutor {
    /// Start a host-configured stdio server, negotiate one exact version and discover tools.
    pub fn discover(config: ServerConfig, limits: TransportLimits) -> Result<Self, McpError> {
        worker::validate_config(&config, limits)?;
        let server_digest = digest(&json!({
            "domain": "gaap.mcp-server/0.1",
            "artifact_digest": config.artifact_digest,
            "executable": config.executable,
            "arguments": config.arguments,
            "environment_digest": digest(&config.environment)?,
            "working_directory": config.working_directory,
            "server_name": config.expected_name,
            "server_version": config.expected_version,
            "protocol": MCP_PROTOCOL_VERSION,
        }))?;
        let identity = ExecutorIdentity {
            name: "gaap-mcp-stdio".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            digest: digest(&json!({"server": server_digest, "sdk": MCP_SDK_VERSION}))?,
        };
        let profile = SandboxProfileIdentity {
            name: "local-stdio-unisolated".into(),
            version: "0.1".into(),
            digest: digest(
                &json!({"domain":"gaap.mcp-stdio-profile/0.1","server":server_digest,"environment":"explicit-allowlist","isolation":"none"}),
            )?,
        };
        let cancellation = CancellationToken::new();
        let (sender, receiver) = mpsc::sync_channel(1);
        let (init_sender, init_receiver) = mpsc::channel();
        let state = Arc::new(Mutex::new(transport::WireState::default()));
        let worker_state = state.clone();
        let worker_cancel = cancellation.clone();
        let worker_digest = server_digest.clone();
        let worker = std::thread::Builder::new()
            .name("gaap-mcp".into())
            .spawn(move || {
                worker::run(
                    config,
                    limits,
                    worker_digest,
                    worker_cancel,
                    worker_state,
                    receiver,
                    init_sender,
                );
            })
            .map_err(|e| McpError::new("mcp.worker_start", e.to_string()))?;
        let tools = match init_receiver.recv() {
            Ok(Ok(tools)) => tools,
            Ok(Err(error)) => {
                let _ = worker.join();
                return Err(error);
            }
            Err(error) => {
                let _ = worker.join();
                return Err(McpError::new("mcp.worker_start", error.to_string()));
            }
        };
        Ok(Self {
            sender,
            worker: Some(worker),
            tools,
            approved: Vec::new(),
            policy: PermissionPolicy {
                policy_decision: None,
                risk_tags: vec![],
                wrapper_chain: vec![],
                approvals: vec![],
            },
            binding: None,
            prepared: BTreeMap::new(),
            attempted: BTreeSet::new(),
            limits,
            cancellation,
            state,
            identity,
            server_digest,
            profile,
            calls: Vec::new(),
            retained_bytes: 0,
        })
    }

    pub fn tools(&self) -> &[ToolCapability] {
        &self.tools
    }
    pub fn identity(&self) -> &ExecutorIdentity {
        &self.identity
    }
    pub fn profile(&self) -> &SandboxProfileIdentity {
        &self.profile
    }
    pub fn cancellation_handle(&self) -> CancelHandle {
        CancelHandle(self.cancellation.clone())
    }
    pub fn call_evidence(&self) -> &[CallEvidence] {
        &self.calls
    }

    /// Bind one host-accepted run and independent observer. Rebinding is forbidden.
    pub fn bind_run(
        &mut self,
        request: AgentRunRequest,
        support: ContractSupport,
        policy: PermissionPolicy,
        observer: impl SubjectObserver + 'static,
    ) -> Result<(), McpError> {
        if self.binding.is_some() {
            return Err(McpError::new(
                "mcp.run_already_bound",
                "use a fresh executor for a new run",
            ));
        }
        validate_request(&request, &support)
            .map_err(|e| McpError::new("mcp.invalid_run", e.to_string()))?;
        self.policy = policy;
        let deadline = Instant::now()
            .checked_add(Duration::from_millis(
                request.resource_budget.max_elapsed_ms,
            ))
            .ok_or_else(|| {
                McpError::new(
                    "mcp.invalid_budget",
                    "run deadline exceeds the host clock range",
                )
            })?;
        self.binding = Some(RunBinding {
            request,
            support,
            deadline,
            observer: Arc::new(Mutex::new(Box::new(observer))),
        });
        Ok(())
    }

    /// Admit an exact discovered descriptor from trusted host configuration.
    pub fn approve_capability(&mut self, capability: &CapabilityIdentity) -> Result<(), McpError> {
        if !self.tools.iter().any(|tool| &tool.capability == capability) {
            return Err(McpError::new(
                "mcp.unknown_capability",
                "approval must match a discovered capability",
            ));
        }
        if !self.approved.contains(capability) {
            self.approved.push(capability.clone());
        }
        Ok(())
    }

    pub fn refresh_tools(&mut self) -> Result<&[ToolCapability], McpError> {
        transport::health(&self.state)?;
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(WorkerCommand::Refresh(reply))
            .map_err(|_| McpError::new("mcp.session_closed", "start a new session"))?;
        self.tools = receiver
            .recv()
            .map_err(|_| McpError::new("mcp.session_closed", "discovery worker exited"))??;
        self.approved
            .retain(|capability| self.tools.iter().any(|tool| &tool.capability == capability));
        Ok(&self.tools)
    }

    /// Prepare exact input bindings without dispatching work or approving a capability.
    pub fn prepare_effect(
        &mut self,
        tool_name: &str,
        arguments: Value,
        effect_id: String,
        sequence: u64,
        subject: Subject,
        effect_class: EffectClass,
    ) -> Result<ProtectedEffectRequest, McpError> {
        if self.prepared.len() >= self.limits.max_calls {
            return Err(McpError::new("mcp.call_limit", "start a fresh bounded run"));
        }
        let tool = self
            .tools
            .iter()
            .find(|tool| tool.tool_name == tool_name)
            .ok_or_else(|| {
                McpError::new("mcp.unknown_tool", "discover and approve the exact tool")
            })?;
        if serde_json::to_vec(&arguments)
            .map_err(|e| McpError::new("mcp.invalid_arguments", e.to_string()))?
            .len()
            > self.limits.max_frame_bytes / 2
        {
            return Err(McpError::new(
                "mcp.frame_limit",
                "arguments exceed the configured request bound",
            ));
        }
        let argument_bytes = serde_json::to_vec(&arguments)
            .map_err(|e| McpError::new("mcp.invalid_arguments", e.to_string()))?
            .len();
        if self.retained_bytes.saturating_add(argument_bytes) > self.limits.max_retained_bytes {
            return Err(McpError::new(
                "mcp.retention_limit",
                "retained inputs exceed the run's configured bound",
            ));
        }
        worker::validate_arguments(&tool.descriptor, &arguments)?;
        let binding = self.binding.as_ref().ok_or_else(|| {
            McpError::new(
                "mcp.run_required",
                "bind an accepted run before preparing effects",
            )
        })?;
        let run_digest = validate_request(&binding.request, &binding.support)
            .map_err(|e| McpError::new("mcp.invalid_run", e.to_string()))?;
        let input_digest = digest(
            &json!({"server": self.server_digest,"tool": tool_name,"arguments": arguments}),
        )?;
        let effect = ProtectedEffectRequest {
            schema_version: PROTECTED_EFFECT_REQUEST_SCHEMA.into(),
            effect_id,
            effect_sequence: sequence,
            run_id: binding.request.run_id.clone(),
            agent_run_request_digest: run_digest,
            subject: subject.clone(),
            operation_family: OperationFamily::ExternalService,
            normalized_operation: "external_service.mcp_call".into(),
            capability: tool.capability.clone(),
            tool_schema_digest: Some(tool.schema_digest.clone()),
            input_digest: input_digest.clone(),
            input_metadata: vec![InputMetadataEntry {
                name: "mcp_protocol".into(),
                value: MCP_PROTOCOL_VERSION.into(),
            }],
            requested_scopes: vec![RequestedScope::ExternalService {
                service: self.server_digest.clone(),
                operation: tool_name.into(),
                resource: subject.locator,
            }],
            policies: binding.request.policies.clone(),
            approval_context: binding.request.approval_context.clone(),
            resource_budget_digest: canonical_resource_budget_digest(
                &binding.request.resource_budget,
            )
            .map_err(|e| McpError::new("mcp.invalid_budget", e.to_string()))?,
            sandbox_profile: self.profile.clone(),
            idempotency_key: format!("mcp:{}:{sequence}:{input_digest}", binding.request.run_id),
            repeatability: Repeatability::NonRepeatable,
            expected_effect_class: effect_class,
        };
        validate_protected_effect_request(&binding.request, &binding.support, &effect)
            .map_err(|e| McpError::new("mcp.invalid_effect", e.to_string()))?;
        if self.prepared.contains_key(&effect.effect_id) {
            return Err(McpError::new(
                "mcp.identity_conflict",
                "effect ID is already prepared",
            ));
        }
        self.prepared
            .insert(effect.effect_id.clone(), (digest(&effect)?, arguments));
        self.retained_bytes += argument_bytes;
        Ok(effect)
    }

    fn execute_effect(
        &mut self,
        effect: &ProtectedEffectRequest,
    ) -> Result<ExecutorObservation, McpError> {
        let binding = self
            .binding
            .as_ref()
            .ok_or_else(|| McpError::new("mcp.run_required", "bind the accepted run"))?;
        let request_digest =
            validate_protected_effect_request(&binding.request, &binding.support, effect)
                .map_err(|e| McpError::new("mcp.invalid_effect", e.to_string()))?;
        let (prepared_digest, arguments) = self
            .prepared
            .get(&effect.effect_id)
            .ok_or_else(|| {
                McpError::new(
                    "mcp.input_binding",
                    "prepare the exact arguments before execution",
                )
            })?
            .clone();
        if prepared_digest != request_digest || effect.sandbox_profile != self.profile {
            return Err(McpError::new(
                "mcp.effect_binding",
                "the immutable prepared effect or execution profile changed",
            ));
        }
        let tool = self
            .tools
            .iter()
            .find(|tool| tool.capability == effect.capability)
            .map(|tool| tool.tool_name.clone())
            .ok_or_else(|| {
                McpError::new(
                    "mcp.capability_drift",
                    "prepare and approve the current descriptor",
                )
            })?;
        if effect.operation_family != OperationFamily::ExternalService
            || effect.normalized_operation != "external_service.mcp_call"
            || effect.requested_scopes
                != vec![RequestedScope::ExternalService {
                    service: self.server_digest.clone(),
                    operation: tool,
                    resource: effect.subject.locator.clone(),
                }]
        {
            return Err(McpError::new(
                "mcp.scope_binding",
                "effect scope differs from the prepared invocation",
            ));
        }
        if self.attempted.contains(&effect.effect_id)
            || self.attempted.contains(&effect.idempotency_key)
        {
            return Err(McpError::new(
                "mcp.replay_refused",
                "an attempted invocation can't be silently replayed",
            ));
        }
        let refusal = transport::health(&self.state).err().or_else(|| {
            self.cancellation
                .is_cancelled()
                .then(|| McpError::new("mcp.cancelled_before_dispatch", "session is terminal"))
        });
        if let Some(reason) = refusal {
            return Ok(ExecutorObservation {
                execution_status: EffectExecutionStatus::Denied,
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
                reason: Some(reason.to_string()),
                evidence: vec![],
            });
        }
        let preflight = self
            .binding
            .as_mut()
            .expect("validated binding")
            .observer
            .lock()
            .map_err(|_| {
                McpError::new(
                    "mcp.observer_unavailable",
                    "host observation lock is poisoned",
                )
            })?
            .observe(effect)?;
        if preflight.subject != effect.subject {
            return Err(McpError::new(
                "mcp.stale_subject",
                "fresh host observation differs from the requested subject",
            ));
        }
        const RECONCILIATION_RESERVE: usize = 2048;
        if self.retained_bytes.saturating_add(RECONCILIATION_RESERVE)
            > self.limits.max_retained_bytes
        {
            return Err(McpError::new(
                "mcp.retention_limit",
                "reserve reconciliation evidence before dispatch",
            ));
        }
        let start = Instant::now();
        let record_index = self.calls.len();
        self.calls.push(CallEvidence {
            request_digest: request_digest.clone(),
            capability_digest: effect.capability.digest.clone(),
            protocol_version: MCP_PROTOCOL_VERSION.into(),
            sdk_version: MCP_SDK_VERSION.into(),
            status: EffectExecutionStatus::UnknownOutcome,
            dispatched: false,
            effect_confirmed: None,
            observed_subject: None,
            host_usage: None,
            output_digest: None,
            output: None,
            output_omitted: false,
            error: Some(
                "In-memory invocation intent; no terminal response has been observed".into(),
            ),
        });
        self.retained_bytes += RECONCILIATION_RESERVE;
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(WorkerCommand::Invoke {
                effect: Box::new(effect.clone()),
                arguments,
                approved: self.approved.clone(),
                policy: self.policy.clone(),
                observer: self
                    .binding
                    .as_ref()
                    .expect("validated binding")
                    .observer
                    .clone(),
                deadline: self.binding.as_ref().expect("validated binding").deadline,
                reply,
            })
            .map_err(|_| McpError::new("mcp.session_closed", "inspect existing evidence"))?;
        // A lost reply can't establish whether dispatch occurred. Preserve the intent and refuse replay.
        self.calls[record_index].dispatched = true;
        let invocation = receiver.recv().unwrap_or_else(|_| {
            Ok(worker::Invocation {
                status: EffectExecutionStatus::UnknownOutcome,
                output: None,
                reason: Some("mcp.worker_exit: effect outcome is unknown".into()),
                dispatched: true,
            })
        });
        let (status, output, reason, dispatched) = match invocation {
            Ok(call) => (call.status, call.output, call.reason, call.dispatched),
            Err(error) => (
                EffectExecutionStatus::Denied,
                None,
                Some(error.to_string()),
                false,
            ),
        };
        if dispatched {
            self.attempted.insert(effect.effect_id.clone());
            self.attempted.insert(effect.idempotency_key.clone());
        }
        let reason = reason.map(|reason| {
            reason
                .chars()
                .map(|c| {
                    if c.is_ascii() && !c.is_ascii_control() {
                        c
                    } else {
                        '?'
                    }
                })
                .take(512)
                .collect::<String>()
        });
        let output_digest = output.as_ref().map(digest).transpose()?;
        let mut observed = None;
        let mut effect_confirmed = None;
        let mut cost_micros = 0;
        let mut model_tokens = 0;
        let mut final_status = status;
        let mut final_reason = reason;
        if dispatched {
            let observer = &self.binding.as_ref().expect("validated binding").observer;
            let observation = match observer.lock() {
                Ok(mut observer) => observer.observe(effect),
                Err(_) => Err(McpError::new(
                    "mcp.observer_unavailable",
                    "host observation lock is poisoned",
                )),
            };
            match observation {
                Ok(observation) => {
                    effect_confirmed = Some(observation.effect_confirmed);
                    if !observation.effect_confirmed {
                        final_status = EffectExecutionStatus::UnknownOutcome;
                        final_reason = Some(
                            "mcp.effect_unconfirmed: host evidence doesn't establish execution"
                                .into(),
                        );
                    }
                    if effect.expected_effect_class == EffectClass::Observation
                        && observation.subject.digest != effect.subject.digest
                    {
                        final_status = EffectExecutionStatus::UnknownOutcome;
                        final_reason = Some(
                            "mcp.unexpected_subject_change: observation changed the subject".into(),
                        );
                    }
                    observed = Some(observation.subject);
                    cost_micros = observation.cost_micros;
                    model_tokens = observation.model_tokens;
                }
                Err(error) => {
                    final_status = EffectExecutionStatus::UnknownOutcome;
                    final_reason = Some(error.to_string());
                }
            }
        }
        let usage = EffectUsage {
            cost_micros,
            elapsed_ms: if dispatched {
                start.elapsed().as_millis().min(9_007_199_254_740_991) as u64
            } else {
                0
            },
            model_tokens,
            tool_calls: u64::from(dispatched),
        };
        let mut record = CallEvidence {
            request_digest,
            capability_digest: effect.capability.digest.clone(),
            protocol_version: MCP_PROTOCOL_VERSION.into(),
            sdk_version: MCP_SDK_VERSION.into(),
            status: final_status,
            dispatched,
            effect_confirmed,
            observed_subject: observed.clone(),
            host_usage: observed.as_ref().map(|_| usage.clone()),
            output_digest: output_digest.clone(),
            output,
            output_omitted: false,
            error: final_reason.clone(),
        };
        let retained_before_call = self.retained_bytes - RECONCILIATION_RESERVE;
        let mut record_bytes = serde_json::to_vec(&record)
            .map_err(|e| McpError::new("mcp.evidence_encoding", e.to_string()))?
            .len();
        if retained_before_call.saturating_add(record_bytes) > self.limits.max_retained_bytes {
            record.output = None;
            record.output_omitted = true;
            record.observed_subject = None;
            record.error = record.error.map(|error| {
                error
                    .chars()
                    .map(|c| {
                        if c.is_ascii() && !c.is_ascii_control() {
                            c
                        } else {
                            '?'
                        }
                    })
                    .take(512)
                    .collect()
            });
            record_bytes = serde_json::to_vec(&record)
                .map_err(|e| McpError::new("mcp.evidence_encoding", e.to_string()))?
                .len();
        }
        // The reserved slot fits this bounded digest/usage/error projection even when raw output is omitted.
        debug_assert!(
            record_bytes <= RECONCILIATION_RESERVE
                || retained_before_call.saturating_add(record_bytes)
                    <= self.limits.max_retained_bytes
        );
        let record_digest = digest(&record)?;
        self.retained_bytes = retained_before_call + record_bytes;
        self.calls[record_index] = record;
        let mut evidence = Vec::new();
        evidence.push(EffectEvidenceReference {
            evidence_type: EffectEvidenceType::CapabilitySchema,
            digest: effect
                .tool_schema_digest
                .clone()
                .unwrap_or_else(|| effect.capability.digest.clone()),
            locator: None,
        });
        if dispatched {
            for (evidence_type, digest) in [
                (EffectEvidenceType::Executor, digest(&self.identity)?),
                (EffectEvidenceType::Sandbox, digest(&self.profile)?),
                (EffectEvidenceType::Usage, digest(&usage)?),
            ] {
                evidence.push(EffectEvidenceReference {
                    evidence_type,
                    digest,
                    locator: None,
                });
            }
            if let Some(output_digest) = output_digest {
                evidence.push(EffectEvidenceReference {
                    evidence_type: EffectEvidenceType::Output,
                    digest: output_digest,
                    locator: None,
                });
            }
            if let Some(subject) = &observed {
                evidence.push(EffectEvidenceReference {
                    evidence_type: EffectEvidenceType::SubjectObservation,
                    digest: digest(subject)?,
                    locator: None,
                });
                if effect.expected_effect_class == EffectClass::Mutation {
                    evidence.push(EffectEvidenceReference {
                        evidence_type: EffectEvidenceType::Mutation,
                        digest: record_digest.clone(),
                        locator: None,
                    });
                    evidence.push(EffectEvidenceReference {
                        evidence_type: EffectEvidenceType::Artifact,
                        digest: subject.digest.clone(),
                        locator: None,
                    });
                }
            }
        }
        if final_status == EffectExecutionStatus::UnknownOutcome {
            evidence.push(EffectEvidenceReference {
                evidence_type: EffectEvidenceType::UnknownOutcome,
                digest: record_digest,
                locator: None,
            });
        } else if final_status == EffectExecutionStatus::Failed {
            evidence.push(EffectEvidenceReference {
                evidence_type: EffectEvidenceType::Failure,
                digest: record_digest,
                locator: None,
            });
        } else if final_status == EffectExecutionStatus::Interrupted {
            evidence.push(EffectEvidenceReference {
                evidence_type: EffectEvidenceType::Interruption,
                digest: record_digest,
                locator: None,
            });
        }
        Ok(ExecutorObservation {
            execution_status: final_status,
            observed_post_effect_subject: observed,
            exit: None,
            usage,
            executor: if dispatched {
                Some(self.identity.clone())
            } else {
                None
            },
            sandbox_profile: if dispatched {
                Some(self.profile.clone())
            } else {
                None
            },
            reason: final_reason,
            evidence,
        })
    }
}

impl ExecutorPort for McpExecutor {
    fn execute(
        &mut self,
        effect: &ProtectedEffectRequest,
    ) -> Result<ExecutorObservation, RuntimePortError> {
        self.execute_effect(effect)
            .map_err(|e| RuntimePortError::new(e.to_string()))
    }
}

impl Drop for McpExecutor {
    fn drop(&mut self) {
        self.cancellation.cancel();
        let _ = self.sender.send(WorkerCommand::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub(crate) fn digest(value: &impl Serialize) -> Result<String, McpError> {
    let bytes =
        serde_jcs::to_vec(value).map_err(|e| McpError::new("mcp.canonical_json", e.to_string()))?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}
