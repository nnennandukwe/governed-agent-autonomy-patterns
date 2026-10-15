//! One stable-v1 ACP session proposes effects; GAAP retains execution authority.
#![forbid(unsafe_code)]

mod wire;
mod worker;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use gaap::contracts::*;
use gaap::runtime::*;
use gaap::{Gate, Outcome, RunCoordinator};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub const ACP_SDK_VERSION: &str = "3.3.0";
pub const ACP_PROTOCOL_VERSION: u32 = 1;

/// Host-approved launch bytes. The host must serialize filesystem changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchArtifact {
    pub path: PathBuf,
    pub digest: String,
}

/// A trusted local agent that delegates filesystem access over ACP.
/// This isn't an operating-system sandbox or an attestation of agent behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub executable: PathBuf,
    pub arguments: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub working_directory: PathBuf,
    pub expected_name: String,
    pub expected_version: String,
    pub artifacts: Vec<LaunchArtifact>,
}

impl AgentConfig {
    /// Pure identity for host approval; launch verification doesn't grant authority.
    pub fn capability(&self) -> Result<CapabilityIdentity, RuntimePortError> {
        Ok(CapabilityIdentity {
            name: "acp.filesystem".into(),
            version: "1".into(),
            digest: digest(&json!({"sdk":ACP_SDK_VERSION,"protocol":1,"launch":self,
                "directory_identity": worker::directory_identity(&self.working_directory)?,
                "profile":"delegated-filesystem-v1","terminal":false,"ambient_permissions":false}))?,
        })
    }
}

/// Hard bounds for one session, including both directions of wire traffic.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub operation_timeout: Duration,
    pub max_frame_bytes: usize,
    pub max_messages: usize,
    pub max_total_bytes: usize,
    pub max_stderr_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            operation_timeout: Duration::from_secs(10),
            max_frame_bytes: 1024 * 1024,
            max_messages: 1024,
            max_total_bytes: 16 * 1024 * 1024,
            max_stderr_bytes: 64 * 1024,
        }
    }
}

/// Full raw input for the supported delegated filesystem operations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum FileOperation {
    Read {
        path: PathBuf,
        line: Option<u32>,
        limit: Option<u32>,
    },
    Write {
        path: PathBuf,
        content: String,
    },
}

/// A trusted host performs the effect and independently reports its observation.
/// Returned read text is sent to the agent; it isn't included in trace records.
pub struct HostEffect {
    pub observation: ExecutorObservation,
    pub read_text: Option<String>,
}

/// Host-owned effect execution and out-of-band model metering.
/// `usage` returns an incremental delta since its previous call. Unknown usage is
/// an error; zero is valid only when the host establishes that nothing was billed.
pub trait AcpHost: Send + 'static {
    fn usage(&mut self) -> Result<ResourceUsage, RuntimePortError>;
    fn execute(
        &mut self,
        operation: &FileOperation,
        request: &ProtectedEffectRequest,
    ) -> Result<HostEffect, RuntimePortError>;
}

/// A payload-free protocol trace. Never enable raw SDK tracing around secrets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceEntry {
    pub direction: String,
    pub method: String,
    pub frame_digest: String,
}

/// Host-reported metadata retained when it can't be claimed as a known result.
/// This includes subject locators and host evidence, never returned file text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconciliationRecord {
    pub effect_request_digest: String,
    pub report: serde_json::Value,
    pub digest: String,
}

/// Prefix coordinates make every effect digest reproducible from final evidence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectCheckpoint {
    pub progress_count: usize,
    pub dispatch_count: usize,
    pub digest: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionEvidence {
    pub sdk_version: String,
    pub protocol_version: u32,
    pub run_request_digest: Option<String>,
    pub session_digest: Option<String>,
    pub negotiation_digest: Option<String>,
    pub capability_digest: String,
    pub trace: Vec<TraceEntry>,
    pub failure_code: Option<String>,
    pub cleanup_failure: Option<String>,
    pub usage_failure: Option<String>,
    pub tool_progress: Vec<ToolProgress>,
    /// Ordered effect digests admitted under the same lock as protocol faults.
    pub dispatch_claims: Vec<String>,
    pub effect_checkpoints: Vec<EffectCheckpoint>,
    pub reconciliation: Vec<ReconciliationRecord>,
}

/// Read-only access to retained session evidence after shutdown.
#[derive(Clone)]
pub struct EvidenceHandle(Arc<Mutex<SessionEvidence>>);
impl EvidenceHandle {
    pub fn snapshot(&self) -> SessionEvidence {
        self.0.lock().expect("evidence lock").clone()
    }

    pub(crate) fn record_failure(&self, code: impl Into<String>) -> String {
        self.0
            .lock()
            .expect("evidence lock")
            .failure_code
            .get_or_insert_with(|| code.into())
            .clone()
    }

    fn cleanup_timeout(&self) {
        let mut retained = self.0.lock().expect("evidence lock");
        retained.cleanup_failure = Some("acp.cleanup_timeout".into());
        retained
            .failure_code
            .get_or_insert_with(|| "acp.cleanup_timeout".into());
    }
}

/// Agent-reported progress, never execution or billing authority.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolProgress {
    pub tool_id_digest: String,
    pub status: Option<String>,
    pub is_update: bool,
}

/// Host-owned cancellation for this single session.
#[derive(Clone)]
pub struct CancelHandle(CancellationToken, Arc<std::sync::atomic::AtomicU8>);
impl CancelHandle {
    pub fn cancel(&self) {
        use std::sync::atomic::Ordering::SeqCst;
        if self
            .1
            .compare_exchange(
                SessionOutcome::Active as u8,
                SessionOutcome::Cancelled as u8,
                SeqCst,
                SeqCst,
            )
            .is_ok()
        {
            self.0.cancel();
        }
    }
}

pub(crate) enum Event {
    Ready,
    ToolProgress(ToolProgress),
    Effect(
        FileOperation,
        oneshot::Sender<Result<Option<String>, String>>,
    ),
    Finished(String),
    Fault(String),
}

struct Pending {
    operation: FileOperation,
    request: ProtectedEffectRequest,
    reply: oneshot::Sender<Result<Option<String>, String>>,
}

struct Shared {
    pending: Option<Pending>,
    cancellation: CancellationToken,
    outcome: Arc<std::sync::atomic::AtomicU8>,
}

#[repr(u8)]
enum SessionOutcome {
    Active,
    Cancelled,
    CompletionClaimed,
}

impl Shared {
    fn claim_completion(&self) -> Option<Cancellation> {
        use std::sync::atomic::Ordering::SeqCst;
        let _ = self.outcome.compare_exchange(
            SessionOutcome::Active as u8,
            SessionOutcome::CompletionClaimed as u8,
            SeqCst,
            SeqCst,
        );
        self.explicit_cancellation()
    }

    fn explicit_cancellation(&self) -> Option<Cancellation> {
        (self.outcome.load(std::sync::atomic::Ordering::SeqCst) == SessionOutcome::Cancelled as u8)
            .then(|| Cancellation {
                actor_id: None,
                reason: "acp.cancelled".into(),
                evidence: EvidenceReference {
                    evidence_type: EvidenceType::Interruption,
                    digest: format!("sha256:{:x}", Sha256::digest(b"acp.cancelled")),
                    locator: None,
                },
            })
    }
}

#[derive(PartialEq, Eq)]
enum AdapterPhase {
    New,
    Starting,
    Ready,
    Active,
    Closed,
}

/// Single-use agent port; its paired executor releases suspended requests.
pub struct AcpAdapter {
    config: AgentConfig,
    limits: Limits,
    support: ContractSupport,
    profile: SandboxProfileIdentity,
    capability: CapabilityIdentity,
    shared: Arc<Mutex<Shared>>,
    host: HostPort,
    events: Option<mpsc::Receiver<Event>>,
    stopped: Option<mpsc::Receiver<()>>,
    start: Option<oneshot::Sender<()>>,
    binding: Option<AgentRunRequest>,
    deadline: Option<Instant>,
    evidence: EvidenceHandle,
    phase: AdapterPhase,
    effects: u64,
}

/// Exact-input host executor for requests proposed by its matching adapter.
pub struct AcpExecutor {
    shared: Arc<Mutex<Shared>>,
    host: HostPort,
    capability: CapabilityIdentity,
    policy: PermissionPolicy,
    evidence: EvidenceHandle,
}

impl AcpAdapter {
    /// Build a fresh, single-use session and its matching protected-effect port.
    pub fn new(
        config: AgentConfig,
        limits: Limits,
        support: ContractSupport,
        profile: SandboxProfileIdentity,
        policy: PermissionPolicy,
        host: impl AcpHost,
    ) -> Result<(Self, AcpExecutor), RuntimePortError> {
        worker::validate_launch(&config)?;
        if limits.operation_timeout.is_zero()
            || limits.operation_timeout > Duration::from_secs(60)
            || !(1024..=4 * 1024 * 1024).contains(&limits.max_frame_bytes)
            || !(8..=4096).contains(&limits.max_messages)
            || !(limits.max_frame_bytes..=64 * 1024 * 1024).contains(&limits.max_total_bytes)
            || limits.max_stderr_bytes > 1024 * 1024
        {
            return Err(error("acp.invalid_limits"));
        }
        let capability = config.capability()?;
        let shared = Arc::new(Mutex::new(Shared {
            pending: None,
            cancellation: CancellationToken::new(),
            outcome: Arc::new(std::sync::atomic::AtomicU8::new(
                SessionOutcome::Active as u8,
            )),
        }));
        let evidence = EvidenceHandle(Arc::new(Mutex::new(SessionEvidence {
            sdk_version: ACP_SDK_VERSION.into(),
            protocol_version: 1,
            capability_digest: capability.digest.clone(),
            ..SessionEvidence::default()
        })));
        let host = HostPort::new(
            host,
            limits.operation_timeout,
            shared.lock().expect("session lock").cancellation.clone(),
            evidence.clone(),
        );
        let executor = AcpExecutor {
            shared: shared.clone(),
            host: host.clone(),
            capability: capability.clone(),
            policy,
            evidence: evidence.clone(),
        };
        Ok((
            Self {
                config,
                limits,
                support,
                profile,
                capability,
                shared,
                host,
                events: None,
                stopped: None,
                start: None,
                binding: None,
                deadline: None,
                evidence,
                phase: AdapterPhase::New,
                effects: 0,
            },
            executor,
        ))
    }

    pub fn capability(&self) -> &CapabilityIdentity {
        &self.capability
    }
    pub fn evidence(&self) -> EvidenceHandle {
        self.evidence.clone()
    }
    pub fn cancellation_handle(&self) -> CancelHandle {
        let shared = self.shared.lock().expect("session lock");
        CancelHandle(shared.cancellation.clone(), shared.outcome.clone())
    }

    fn meter(&self) -> Result<ResourceUsage, RuntimePortError> {
        self.host.usage().inspect_err(|_| {
            self.evidence.0.lock().expect("evidence lock").usage_failure =
                Some("acp.usage_unknown".into());
        })
    }

    fn event(&self) -> Result<Event, RuntimePortError> {
        let deadline = self.deadline.ok_or_else(|| error("acp.run_not_bound"))?;
        let operation_deadline = Instant::now() + self.limits.operation_timeout;
        loop {
            if self.cancellation_handle().0.is_cancelled() {
                return Err(error("acp.cancelled"));
            }
            let remaining = deadline
                .min(operation_deadline)
                .saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                self.cancellation_handle().0.cancel();
                return Err(error("acp.timeout"));
            }
            match self
                .events
                .as_ref()
                .ok_or_else(|| error("acp.not_started"))?
                .recv_timeout(remaining.min(Duration::from_millis(10)))
            {
                Ok(event) => {
                    if self.cancellation_handle().0.is_cancelled() {
                        return Err(error("acp.cancelled"));
                    }
                    if let Event::ToolProgress(_progress) = event {
                        // Retained by the SDK callback even while a host effect waits.
                        continue;
                    }
                    return Ok(event);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => (),
                Err(_) => return Err(error("acp.agent_exit")),
            }
        }
    }

    fn prepare(
        &mut self,
        operation: FileOperation,
        reply: oneshot::Sender<Result<Option<String>, String>>,
        context: AgentRunContext<'_>,
    ) -> Result<AgentStep, RuntimePortError> {
        if let Some(code) = self.evidence.snapshot().failure_code {
            return Err(error(&code));
        }
        let run = self
            .binding
            .as_ref()
            .ok_or_else(|| error("acp.run_not_bound"))?;
        if self.shared.lock().expect("session lock").pending.is_some() {
            return Err(error("acp.concurrent_effect"));
        }
        let (path, access, class, name) = match &operation {
            FileOperation::Read { path, .. } => (
                path,
                vec![FilesystemAccess::Read],
                EffectClass::Observation,
                "filesystem.read",
            ),
            FileOperation::Write { path, .. } => (
                path,
                vec![FilesystemAccess::Create, FilesystemAccess::Modify],
                EffectClass::Mutation,
                "filesystem.write",
            ),
        };
        if !path.is_absolute()
            || path
                .components()
                .any(|p| matches!(p, std::path::Component::ParentDir))
            || !path.starts_with(&self.config.working_directory)
        {
            return Err(error("acp.path_outside_workspace"));
        }
        let parent = path
            .parent()
            .ok_or_else(|| error("acp.path_outside_workspace"))?;
        if parent.canonicalize().ok().as_ref() != Some(&parent.to_path_buf())
            || (std::fs::symlink_metadata(path).is_ok()
                && path.canonicalize().ok().as_ref() != Some(path))
        {
            return Err(error("acp.noncanonical_effect_path"));
        }
        let sequence = context
            .effects_seen
            .checked_add(1)
            .ok_or_else(|| error("acp.effect_limit"))?;
        let effect = ProtectedEffectRequest {
            schema_version: PROTECTED_EFFECT_REQUEST_SCHEMA.into(),
            effect_id: format!("acp-{sequence}"),
            effect_sequence: sequence,
            run_id: run.run_id.clone(),
            agent_run_request_digest: validate_request(run, &self.support)
                .map_err(|_| error("acp.invalid_run"))?,
            subject: context.current_subject.clone(),
            operation_family: OperationFamily::Filesystem,
            normalized_operation: name.into(),
            capability: self.capability.clone(),
            tool_schema_digest: None,
            input_digest: digest(&operation)?,
            input_metadata: vec![
                InputMetadataEntry {
                    name: "protocol".into(),
                    value: "acp-v1".into(),
                },
                InputMetadataEntry {
                    name: "acp_sdk".into(),
                    value: ACP_SDK_VERSION.into(),
                },
                InputMetadataEntry {
                    name: "acp_session".into(),
                    value: self
                        .evidence
                        .snapshot()
                        .session_digest
                        .ok_or_else(|| error("acp.session_unbound"))?,
                },
            ],
            requested_scopes: vec![RequestedScope::Filesystem {
                root: path.to_string_lossy().into_owned(),
                access,
                recursive: false,
            }],
            policies: run.policies.clone(),
            approval_context: run
                .approval_context
                .iter()
                .filter(|a| a.subject_digest == context.current_subject.digest)
                .cloned()
                .collect(),
            resource_budget_digest: canonical_resource_budget_digest(&run.resource_budget)
                .map_err(|_| error("acp.invalid_budget"))?,
            sandbox_profile: self.profile.clone(),
            idempotency_key: format!("acp:{}:{sequence}", run.run_id),
            repeatability: Repeatability::NonRepeatable,
            expected_effect_class: class,
        };
        validate_protected_effect_request(run, &self.support, &effect)
            .map_err(|_| error("acp.invalid_effect"))?;
        self.shared.lock().expect("session lock").pending = Some(Pending {
            operation,
            request: effect.clone(),
            reply,
        });
        self.effects += 1;
        Ok(AgentStep::ProtectedEffect(Box::new(
            ProtectedEffectProposal { request: effect },
        )))
    }
}

impl AgentAdapter for AcpAdapter {
    fn claim_completion(&mut self) -> Result<Option<Cancellation>, RuntimePortError> {
        Ok(self.shared.lock().expect("session lock").claim_completion())
    }

    fn cancellation(&self) -> Option<Cancellation> {
        self.shared
            .lock()
            .expect("session lock")
            .explicit_cancellation()
    }

    fn close(&mut self) {
        self.phase = AdapterPhase::Closed;
        self.start.take();
        self.cancellation_handle().0.cancel();
        self.shared.lock().expect("session lock").pending.take();
        if let Some(stopped) = self.stopped.take()
            && stopped.recv_timeout(Duration::from_secs(1)).is_err()
        {
            self.evidence.cleanup_timeout();
        }
    }

    fn plan(
        &mut self,
        request: &AgentRunRequest,
    ) -> Result<PortObservation<PlanProposal>, RuntimePortError> {
        if self.phase != AdapterPhase::New {
            return Err(error("acp.run_already_bound"));
        }
        self.phase = AdapterPhase::Starting;
        let run_digest =
            validate_request(request, &self.support).map_err(|_| error("acp.invalid_run"))?;
        if request.requested_capability != self.capability {
            return Err(error("acp.capability_mismatch"));
        }
        if self.config.capability()? != self.capability {
            return Err(error("acp.launch_identity_changed"));
        }
        self.deadline = Instant::now().checked_add(Duration::from_millis(
            request.resource_budget.max_elapsed_ms,
        ));
        *self.host.deadline.lock().expect("host clock") = self.deadline;
        self.binding = Some(request.clone());
        self.evidence
            .0
            .lock()
            .expect("evidence lock")
            .run_request_digest = Some(run_digest.clone());
        let (events, start, stopped) = worker::start(
            self.config.clone(),
            self.limits,
            request.clone(),
            self.evidence.clone(),
            self.cancellation_handle().0,
        );
        self.events = Some(events);
        self.stopped = Some(stopped);
        self.start = Some(start);
        match self.event()? {
            Event::Ready => (),
            Event::Fault(code) => return Err(error(&code)),
            _ => return Err(error("acp.setup_order")),
        }
        let usage = self.meter()?;
        self.phase = AdapterPhase::Ready;
        Ok(PortObservation {
            value: PlanProposal {
                plan_digest: digest(
                    &json!({"run":run_digest,"capability":self.capability,"session":self.evidence.snapshot().session_digest,"negotiation":self.evidence.snapshot().negotiation_digest}),
                )?,
            },
            usage,
        })
    }

    fn next_effect(
        &mut self,
        context: AgentRunContext<'_>,
    ) -> Result<PortObservation<AgentStep>, RuntimePortError> {
        if self.phase == AdapterPhase::Closed {
            return Err(error("acp.session_finished"));
        }
        if !matches!(self.phase, AdapterPhase::Ready | AdapterPhase::Active) {
            return Err(error("acp.plan_not_ready"));
        }
        if !self.cancellation_handle().0.is_cancelled()
            && let Some(start) = self.start.take()
        {
            let _ = start.send(());
            self.phase = AdapterPhase::Active;
        }
        let event = match self.event() {
            Ok(event) => event,
            Err(e) if e.message().starts_with("acp.cancelled") => {
                self.phase = AdapterPhase::Closed;
                return Ok(PortObservation {
                    value: AgentStep::Cancel(Cancellation {
                        actor_id: None,
                        reason: "acp.cancelled".into(),
                        evidence: EvidenceReference {
                            evidence_type: EvidenceType::CommandOutput,
                            digest: digest(&self.evidence.snapshot())?,
                            locator: None,
                        },
                    }),
                    usage: self.meter()?,
                });
            }
            Err(e) => return Err(e),
        };
        if let Some(code) = self.evidence.snapshot().failure_code {
            return Err(error(&code));
        }
        let step = match event {
            Event::Effect(operation, reply) => self.prepare(operation, reply, context)?,
            Event::Finished(reason) => {
                self.phase = AdapterPhase::Closed;
                if reason != "end_turn" {
                    return Err(error(&format!("acp.stop_{reason}")));
                }
                if self.effects == 0 {
                    return Err(error("acp.no_host_effect"));
                }
                AgentStep::Finish
            }
            Event::Fault(code) => {
                self.phase = AdapterPhase::Closed;
                return Err(error(&code));
            }
            Event::Ready => return Err(error("acp.duplicate_setup")),
            Event::ToolProgress(_) => unreachable!("progress is consumed by event()"),
        };
        Ok(PortObservation {
            value: step,
            usage: self.meter()?,
        })
    }
}

impl Drop for AcpAdapter {
    fn drop(&mut self) {
        self.close();
    }
}

impl ExecutorPort for AcpExecutor {
    fn claim_completion(&mut self) -> Result<Option<Cancellation>, RuntimePortError> {
        Ok(self.shared.lock().expect("session lock").claim_completion())
    }

    fn cancellation(&self) -> Option<Cancellation> {
        self.shared
            .lock()
            .expect("session lock")
            .explicit_cancellation()
    }

    fn execute(
        &mut self,
        request: &ProtectedEffectRequest,
    ) -> Result<ExecutorObservation, RuntimePortError> {
        let Some(pending) = self.shared.lock().expect("session lock").pending.take() else {
            return Ok(non_dispatch(
                EffectExecutionStatus::Denied,
                "acp.no_pending_effect".into(),
            ));
        };
        if pending.request != *request
            || request.capability != self.capability
            || self
                .shared
                .lock()
                .expect("session lock")
                .cancellation
                .is_cancelled()
        {
            let _ = pending.reply.send(Err("acp.binding_mismatch".into()));
            return Ok(non_dispatch(
                EffectExecutionStatus::Denied,
                "acp.binding_mismatch".into(),
            ));
        }
        // The port repeats its host-owned permission decision; ACP UI never grants authority.
        let permission=RunCoordinator.evaluate(Gate::Permission,&json!({
            "action_digest": digest(request)?,
            "approval":self.policy.approvals.iter().find(|a|a.subject_digest==digest(request).unwrap_or_default()).map(|a|json!({"status":"approved","subject_digest":a.subject_digest,"max_cost_micros":a.max_cost_micros})),
            "policy_decision":match self.policy.policy_decision { Some(PolicyDecision::Allow)=>"allow",Some(PolicyDecision::Ask)=>"ask",_=>"deny" },
            "risk_tags":self.policy.risk_tags,"wrapper_chain":self.policy.wrapper_chain
        }));
        if permission.outcome != Outcome::Allow {
            let _ = pending.reply.send(Err("acp.permission_denied".into()));
            return Ok(non_dispatch(
                if permission.outcome == Outcome::Ask {
                    EffectExecutionStatus::AwaitingAuthority
                } else {
                    EffectExecutionStatus::Denied
                },
                permission.code,
            ));
        }
        if let Some(code) = self.evidence.snapshot().failure_code {
            let _ = pending.reply.send(Err(code.clone()));
            return Ok(non_dispatch(EffectExecutionStatus::Denied, code));
        }
        let result = self
            .host
            .execute(pending.operation.clone(), request.clone());
        match result {
            HostCompletion::Attempted(Ok(mut effect)) => {
                effect
                    .observation
                    .evidence
                    .push(session_effect_evidence(&self.evidence)?);
                if matches!(pending.operation, FileOperation::Read { .. })
                    && effect
                        .observation
                        .observed_post_effect_subject
                        .as_ref()
                        .is_some_and(|subject| subject != &request.subject)
                {
                    let reported = &effect.observation;
                    let report = json!({"reported_status":reported.execution_status,"post_subject":reported.observed_post_effect_subject,"exit":reported.exit,"usage":reported.usage,
                        "executor":reported.executor,"sandbox_profile":reported.sandbox_profile,"reason":reported.reason,"evidence":reported.evidence});
                    let effect_request_digest = digest(request)?;
                    let reconciliation = digest(
                        &json!({"effect_request_digest":effect_request_digest,"report":report}),
                    )?;
                    self.evidence
                        .0
                        .lock()
                        .expect("evidence lock")
                        .reconciliation
                        .push(ReconciliationRecord {
                            effect_request_digest,
                            report,
                            digest: reconciliation.clone(),
                        });
                    effect.observation.evidence.push(EffectEvidenceReference {
                        evidence_type: EffectEvidenceType::UnknownOutcome,
                        digest: reconciliation,
                        locator: None,
                    });
                    effect.observation.execution_status = EffectExecutionStatus::UnknownOutcome;
                    effect.observation.observed_post_effect_subject = None;
                    effect.observation.exit = None;
                    effect.observation.reason = Some("acp.read_subject_changed".into());
                    self.evidence.record_failure("acp.read_subject_changed");
                }
                let successful =
                    effect.observation.execution_status == EffectExecutionStatus::Executed;
                if matches!(pending.operation, FileOperation::Read { .. })
                    && successful
                    && effect.read_text.is_none()
                {
                    self.evidence.record_failure("acp.read_evidence_missing");
                    let _ = pending.reply.send(Err("acp.read_evidence_missing".into()));
                    return Ok(effect.observation);
                }
                let reply = if successful {
                    Ok(effect.read_text)
                } else {
                    Err("acp.host_effect_failed".into())
                };
                let _ = pending.reply.send(reply);
                Ok(effect.observation)
            }
            HostCompletion::Attempted(Err(e)) => {
                let _ = pending.reply.send(Err("acp.host_effect_unknown".into()));
                Err(e.with_evidence(vec![session_effect_evidence(&self.evidence)?]))
            }
            HostCompletion::Declined(code) => {
                let _ = pending.reply.send(Err(code.clone()));
                Ok(non_dispatch(EffectExecutionStatus::Denied, code))
            }
        }
    }
}

fn session_effect_evidence(
    handle: &EvidenceHandle,
) -> Result<EffectEvidenceReference, RuntimePortError> {
    let mut identity = handle.0.lock().expect("evidence lock");
    let digest = digest(
        &json!({"sdk":identity.sdk_version,"protocol":identity.protocol_version,"session":identity.session_digest,
        "run":identity.run_request_digest,"capability":identity.capability_digest,"tool_progress":identity.tool_progress,"dispatch_claims":identity.dispatch_claims}),
    )?;
    let checkpoint = EffectCheckpoint {
        progress_count: identity.tool_progress.len(),
        dispatch_count: identity.dispatch_claims.len(),
        digest: digest.clone(),
    };
    identity.effect_checkpoints.push(checkpoint);
    Ok(EffectEvidenceReference {
        evidence_type: EffectEvidenceType::CapabilitySchema,
        digest,
        locator: None,
    })
}

fn non_dispatch(execution_status: EffectExecutionStatus, reason: String) -> ExecutorObservation {
    ExecutorObservation {
        execution_status,
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
        reason: Some(reason),
        evidence: vec![],
    }
}
enum HostCompletion {
    Declined(String),
    Attempted(Result<Box<HostEffect>, RuntimePortError>),
}

#[derive(Clone)]
struct HostPort {
    sender: mpsc::SyncSender<HostCommand>,
    timeout: Duration,
    deadline: Arc<Mutex<Option<Instant>>>,
    cancellation: CancellationToken,
    busy: Arc<std::sync::atomic::AtomicBool>,
    quarantined: Arc<std::sync::atomic::AtomicBool>,
}
enum HostCommand {
    Usage(
        Instant,
        mpsc::Sender<Result<ResourceUsage, RuntimePortError>>,
    ),
    Execute(
        Instant,
        FileOperation,
        Box<ProtectedEffectRequest>,
        mpsc::Sender<HostCompletion>,
    ),
}
impl HostPort {
    fn new(
        mut host: impl AcpHost,
        timeout: Duration,
        cancellation: CancellationToken,
        evidence: EvidenceHandle,
    ) -> Self {
        let (sender, receiver) = mpsc::sync_channel(1);
        let busy = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let quarantined = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_busy = busy.clone();
        let worker_quarantined = quarantined.clone();
        let worker_cancel = cancellation.clone();
        std::thread::spawn(move || {
            while let Ok(command) = receiver.recv() {
                match command {
                    HostCommand::Usage(deadline, reply) => {
                        let result = if Instant::now() >= deadline
                            || worker_quarantined.load(std::sync::atomic::Ordering::SeqCst)
                        {
                            Err(error("acp.host_expired"))
                        } else {
                            host.usage().map_err(|_| error("acp.usage_unknown"))
                        };
                        worker_busy.store(false, std::sync::atomic::Ordering::SeqCst);
                        let _ = reply.send(result);
                    }
                    HostCommand::Execute(deadline, operation, request, reply) => {
                        // The claim and wire-fault recording share one lock. A fault
                        // before the claim revokes queued work; after it, the operation
                        // is in flight and its actual outcome must be retained.
                        let claim = {
                            let mut retained = evidence.0.lock().expect("evidence lock");
                            if let Some(code) = retained.failure_code.clone() {
                                Err(code)
                            } else if worker_cancel.is_cancelled() {
                                Err("acp.cancelled".into())
                            } else if Instant::now() >= deadline
                                || worker_quarantined.load(std::sync::atomic::Ordering::SeqCst)
                            {
                                Err("acp.host_expired".into())
                            } else {
                                digest(&request)
                                    .map(|claim| retained.dispatch_claims.push(claim))
                                    .map_err(|_| "acp.invalid_identity".into())
                            }
                        };
                        let result = match claim {
                            Err(code) => HostCompletion::Declined(code),
                            Ok(()) => HostCompletion::Attempted(
                                host.execute(&operation, &request)
                                    .map(Box::new)
                                    .map_err(|_| error("acp.host_effect_unknown")),
                            ),
                        };
                        worker_busy.store(false, std::sync::atomic::Ordering::SeqCst);
                        let _ = reply.send(result);
                    }
                }
            }
        });
        Self {
            sender,
            timeout,
            deadline: Arc::new(Mutex::new(None)),
            cancellation,
            busy,
            quarantined,
        }
    }
    fn begin(&self, reconcile: bool) -> Result<Instant, RuntimePortError> {
        use std::sync::atomic::Ordering::SeqCst;
        if self.quarantined.load(SeqCst) || (!reconcile && self.cancellation.is_cancelled()) {
            return Err(error("acp.host_quarantined"));
        }
        let now = Instant::now();
        let run_deadline = self
            .deadline
            .lock()
            .expect("host clock")
            .ok_or_else(|| error("acp.run_not_bound"))?;
        let deadline = if reconcile && self.cancellation.is_cancelled() {
            now + Duration::from_millis(250).min(self.timeout)
        } else {
            run_deadline.min(now + self.timeout)
        };
        if now >= deadline {
            return Err(error("acp.timeout"));
        }
        if self
            .busy
            .compare_exchange(false, true, SeqCst, SeqCst)
            .is_err()
        {
            return Err(error("acp.host_busy"));
        }
        Ok(deadline)
    }
    fn wait<T>(
        &self,
        receiver: mpsc::Receiver<Result<T, RuntimePortError>>,
        deadline: Instant,
        code: &str,
    ) -> Result<T, RuntimePortError> {
        match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(result) => result,
            Err(_) => {
                self.quarantined
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                self.cancellation.cancel();
                Err(error(code))
            }
        }
    }
    fn usage(&self) -> Result<ResourceUsage, RuntimePortError> {
        let deadline = self.begin(true)?;
        let (reply, receiver) = mpsc::channel();
        self.sender
            .try_send(HostCommand::Usage(deadline, reply))
            .map_err(|_| error("acp.host_unavailable"))?;
        self.wait(receiver, deadline, "acp.usage_unknown")
    }
    fn execute(&self, operation: FileOperation, request: ProtectedEffectRequest) -> HostCompletion {
        let deadline = match self.begin(false) {
            Ok(deadline) => deadline,
            Err(e) => return HostCompletion::Declined(e.message().into()),
        };
        let (reply, receiver) = mpsc::channel();
        if self
            .sender
            .try_send(HostCommand::Execute(
                deadline,
                operation,
                Box::new(request),
                reply,
            ))
            .is_err()
        {
            return HostCompletion::Declined("acp.host_unavailable".into());
        }
        match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(result) => result,
            Err(_) => {
                self.quarantined
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                self.cancellation.cancel();
                HostCompletion::Attempted(Err(error("acp.effect_unknown")))
            }
        }
    }
}

pub(crate) fn error(code: &str) -> RuntimePortError {
    RuntimePortError::new(format!(
        "{code}: session can't resume; inspect host evidence before a fresh run"
    ))
}
pub(crate) fn digest(value: &impl Serialize) -> Result<String, RuntimePortError> {
    let bytes = serde_jcs::to_vec(value).map_err(|_| error("acp.invalid_identity"))?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoDispatch;
    impl AcpHost for NoDispatch {
        fn usage(&mut self) -> Result<ResourceUsage, RuntimePortError> {
            panic!("no usage")
        }
        fn execute(
            &mut self,
            _: &FileOperation,
            _: &ProtectedEffectRequest,
        ) -> Result<HostEffect, RuntimePortError> {
            panic!("mustn't dispatch")
        }
    }

    #[test]
    fn queued_host_effect_retains_the_protocol_fault() {
        let evidence = EvidenceHandle(Arc::new(Mutex::new(SessionEvidence {
            failure_code: Some("acp.duplicate_request".into()),
            ..SessionEvidence::default()
        })));
        let host = HostPort::new(
            NoDispatch,
            Duration::from_secs(5),
            CancellationToken::new(),
            evidence.clone(),
        );
        let (reply, receiver) = mpsc::channel();
        // The fault has already been observed when a queued command reaches the worker.
        // Use a valid request from the frozen public fixture; no executor can run here.
        let request: ProtectedEffectRequest = serde_json::from_str(include_str!(
            "../../../examples/contracts/protected-effect/v0.1.0/protected-effect-request.json"
        ))
        .unwrap();
        host.sender
            .try_send(HostCommand::Execute(
                Instant::now() + Duration::from_secs(5),
                FileOperation::Read {
                    path: PathBuf::from("/unused"),
                    line: None,
                    limit: None,
                },
                Box::new(request),
                reply,
            ))
            .unwrap();
        match receiver.recv_timeout(Duration::from_secs(5)).unwrap() {
            HostCompletion::Declined(code) => assert_eq!(code, "acp.duplicate_request"),
            HostCompletion::Attempted(_) => panic!("mustn't attempt after a protocol fault"),
        }
        assert!(evidence.snapshot().dispatch_claims.is_empty());
    }

    #[test]
    fn cleanup_retains_primary_fault_and_its_own_diagnostic() {
        for primary in [None, Some("acp.duplicate_request".to_owned())] {
            let evidence = EvidenceHandle(Arc::new(Mutex::new(SessionEvidence {
                failure_code: primary.clone(),
                ..SessionEvidence::default()
            })));
            evidence.cleanup_timeout();
            let retained = evidence.snapshot();
            assert_eq!(
                retained.failure_code.as_deref(),
                primary.as_deref().or(Some("acp.cleanup_timeout"))
            );
            assert_eq!(
                retained.cleanup_failure.as_deref(),
                Some("acp.cleanup_timeout")
            );
        }
    }

    #[test]
    fn cancellation_and_completion_share_one_atomic_winner() {
        for order in 0..10 {
            let shared = Arc::new(Shared {
                pending: None,
                cancellation: CancellationToken::new(),
                outcome: Arc::new(std::sync::atomic::AtomicU8::new(
                    SessionOutcome::Active as u8,
                )),
            });
            let cancel = CancelHandle(shared.cancellation.clone(), shared.outcome.clone());
            let cancelled = if order == 0 {
                cancel.cancel();
                shared.claim_completion()
            } else if order == 1 {
                let result = shared.claim_completion();
                cancel.cancel();
                result
            } else {
                let ready = Arc::new(std::sync::Barrier::new(2));
                let other = ready.clone();
                let racer = std::thread::spawn(move || {
                    other.wait();
                    cancel.cancel();
                });
                ready.wait();
                let result = shared.claim_completion();
                racer.join().unwrap();
                result
            };
            assert_eq!(
                cancelled.is_some(),
                shared.explicit_cancellation().is_some()
            );
            assert_eq!(cancelled.is_some(), shared.cancellation.is_cancelled());
            assert_eq!(shared.claim_completion().is_some(), cancelled.is_some());
            if order == 0 {
                assert!(cancelled.is_some());
            }
            if order == 1 {
                assert!(cancelled.is_none());
            }
        }
    }

    struct PausedHost {
        entered: mpsc::Sender<()>,
        release: mpsc::Receiver<()>,
    }
    impl AcpHost for PausedHost {
        fn usage(&mut self) -> Result<ResourceUsage, RuntimePortError> {
            panic!("no usage")
        }
        fn execute(
            &mut self,
            _: &FileOperation,
            _: &ProtectedEffectRequest,
        ) -> Result<HostEffect, RuntimePortError> {
            self.entered.send(()).unwrap();
            self.release.recv_timeout(Duration::from_secs(5)).unwrap();
            Err(error("fixture.host_response_lost"))
        }
    }

    #[test]
    fn a_fault_after_dispatch_claim_preserves_the_in_flight_attempt() {
        let evidence = EvidenceHandle(Arc::new(Mutex::new(SessionEvidence::default())));
        let (entered, started) = mpsc::channel();
        let (release, resume) = mpsc::channel();
        let host = HostPort::new(
            PausedHost {
                entered,
                release: resume,
            },
            Duration::from_secs(5),
            CancellationToken::new(),
            evidence.clone(),
        );
        let request: ProtectedEffectRequest = serde_json::from_str(include_str!(
            "../../../examples/contracts/protected-effect/v0.1.0/protected-effect-request.json"
        ))
        .unwrap();
        let expected = digest(&request).unwrap();
        let (reply, receiver) = mpsc::channel();
        host.sender
            .try_send(HostCommand::Execute(
                Instant::now() + Duration::from_secs(5),
                FileOperation::Read {
                    path: PathBuf::from("/unused"),
                    line: None,
                    limit: None,
                },
                Box::new(request),
                reply,
            ))
            .unwrap();
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(evidence.snapshot().dispatch_claims, vec![expected]);
        // The protocol guard can retain a later fault while the callback runs.
        evidence.0.lock().unwrap().failure_code = Some("acp.duplicate_request".into());
        release.send(()).unwrap();
        assert!(matches!(
            receiver.recv_timeout(Duration::from_secs(5)).unwrap(),
            HostCompletion::Attempted(Err(_))
        ));
        assert_eq!(
            evidence.snapshot().failure_code.as_deref(),
            Some("acp.duplicate_request")
        );
    }
}
