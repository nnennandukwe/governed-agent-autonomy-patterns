//! Pinned Goose 1.54.0 conversation lifecycle with one proposal-only MCP extension.
//!
//! The host's AgentAdapter owns the whole run and admits external proposals. This
//! driver never executes those proposals or treats peer progress as execution proof.
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use gaap::contracts::{AgentRunRequest, ContractSupport, ProtectedEffectRequest};
use gaap::runtime::{Cancellation, RuntimePortError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::{
    AdapterPhase, AgentConfig, CancelHandle, Event, EvidenceHandle, Limits, SessionEvidence,
    SessionOutcome, Shared, ToolProgress, digest, error, worker,
};

pub(crate) const INVENTORY_METHOD: &str = "_goose/unstable/session/extensions/list";
pub const GOOSE_VERSION: &str = "1.54.0";

/// Host-approved proposal ingress process. It must only forward proposals to the
/// owning host; executable approval alone doesn't attest to that behavior.
#[derive(Clone, Serialize, Deserialize)]
pub struct GooseProfile {
    pub proposal_server: AgentConfig,
    pub server_name: String,
}

impl GooseProfile {
    /// Identity of this exact session profile and approved ingress launch.
    /// It isn't the separate MCP capability that the run requests for effects.
    pub fn capability(
        &self,
        agent: &AgentConfig,
    ) -> Result<gaap::contracts::CapabilityIdentity, RuntimePortError> {
        Ok(gaap::contracts::CapabilityIdentity {
            name: "acp.goose-proposals".into(),
            version: "1".into(),
            digest: digest(&json!({
                "sdk":crate::ACP_SDK_VERSION,"protocol":1,"goose":GOOSE_VERSION,"launch":agent,"profile":self,
                "directory_identity":worker::directory_identity(&agent.working_directory)?,"filesystem_delegation":false,"terminal_delegation":false,"ambient_permissions":false
            }))?,
        })
    }

    pub(crate) fn validate(&self) -> Result<(), RuntimePortError> {
        worker::validate_launch(&self.proposal_server)?;
        if self.server_name.is_empty()
            || self.server_name.len() > 64
            || !self
                .server_name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(error("acp.invalid_extension_name"));
        }
        Ok(())
    }

    pub(crate) fn extension(&self) -> Value {
        json!({"type":"mcp","server":{
            "name":self.server_name,"command":self.proposal_server.executable,
            "args":self.proposal_server.arguments,
            "env":self.proposal_server.environment.iter().map(|(name,value)|json!({"name":name,"value":value})).collect::<Vec<_>>()
        }})
    }

    pub(crate) fn expected_inventory(&self) -> Value {
        json!({"extensions":[{"extension":self.extension(),"extensionKey":self.server_name}]})
    }
}

/// Lifecycle events only. Raw operations arrive at the host's separate ingress.
pub enum ConversationEvent {
    Progress(ToolProgress),
    Finished,
}

/// Single-use ACP conversation, driven by a host-owned AgentAdapter.
pub struct GooseConversation {
    config: AgentConfig,
    profile: GooseProfile,
    limits: Limits,
    evidence: EvidenceHandle,
    shared: Shared,
    receiver: Option<mpsc::Receiver<Event>>,
    start: Option<oneshot::Sender<()>>,
    stopped: Option<mpsc::Receiver<()>>,
    phase: AdapterPhase,
}

impl GooseConversation {
    pub fn new(
        config: AgentConfig,
        profile: GooseProfile,
        limits: Limits,
    ) -> Result<Self, RuntimePortError> {
        worker::validate_launch(&config)?;
        profile.validate()?;
        if config.expected_name != "goose"
            || config.expected_version != GOOSE_VERSION
            || config.working_directory != profile.proposal_server.working_directory
        {
            return Err(error("acp.goose_profile_mismatch"));
        }
        crate::validate_limits(limits)?;
        let capability = profile.capability(&config)?;
        Ok(Self {
            config,
            profile,
            limits,
            evidence: EvidenceHandle(Arc::new(Mutex::new(SessionEvidence {
                sdk_version: crate::ACP_SDK_VERSION.into(),
                protocol_version: 1,
                capability_digest: capability.digest,
                ..SessionEvidence::default()
            }))),
            shared: Shared {
                pending: None,
                cancellation: CancellationToken::new(),
                outcome: Arc::new(std::sync::atomic::AtomicU8::new(
                    SessionOutcome::Active as u8,
                )),
            },
            receiver: None,
            start: None,
            stopped: None,
            phase: AdapterPhase::New,
        })
    }

    pub fn evidence(&self) -> EvidenceHandle {
        self.evidence.clone()
    }
    pub fn cancellation_handle(&self) -> CancelHandle {
        CancelHandle(
            self.shared.cancellation.clone(),
            self.shared.outcome.clone(),
        )
    }
    pub fn cancellation(&self) -> Option<Cancellation> {
        self.shared.explicit_cancellation()
    }

    /// Complete negotiation and exact extension inventory before returning a plan.
    /// The prompt stays suspended until the engine admits that plan.
    pub fn plan(
        &mut self,
        request: &AgentRunRequest,
        support: &ContractSupport,
    ) -> Result<String, RuntimePortError> {
        if self.phase != AdapterPhase::New {
            return Err(error("acp.run_already_bound"));
        }
        self.phase = AdapterPhase::Starting;
        let result = (|| {
            let run_digest = gaap::contracts::validate_request(request, support)
                .map_err(|_| error("acp.invalid_run"))?;
            self.evidence
                .0
                .lock()
                .expect("evidence lock")
                .run_request_digest = Some(run_digest);
            let (receiver, start, stopped) = worker::start(
                self.config.clone(),
                self.limits,
                request.clone(),
                self.evidence.clone(),
                self.shared.cancellation.clone(),
                Some(self.profile.clone()),
            );
            self.receiver = Some(receiver);
            self.start = Some(start);
            self.stopped = Some(stopped);
            match self.receive(self.limits.operation_timeout)? {
                Some(Event::Ready) => digest(
                    &json!({"profile":self.profile,"agent":self.config,"evidence":self.evidence.snapshot()}),
                ),
                _ => Err(error("acp.setup_order")),
            }
        })();
        match result {
            Ok(plan) => {
                self.phase = AdapterPhase::Ready;
                Ok(plan)
            }
            Err(failure) => {
                self.close();
                Err(failure)
            }
        }
    }

    pub fn start_prompt(&mut self) -> Result<(), RuntimePortError> {
        self.check()?;
        if !matches!(self.phase, AdapterPhase::Ready | AdapterPhase::Active) {
            return Err(error("acp.plan_not_ready"));
        }
        if let Some(start) = self.start.take() {
            start.send(()).map_err(|_| error("acp.agent_exit"))?;
            self.phase = AdapterPhase::Active;
        } else if self.phase != AdapterPhase::Active {
            return Err(error("acp.plan_not_ready"));
        }
        Ok(())
    }

    /// Poll alongside the host's bounded proposal channel. A timeout isn't a fault.
    pub fn poll(
        &mut self,
        timeout: Duration,
    ) -> Result<Option<ConversationEvent>, RuntimePortError> {
        if self.phase != AdapterPhase::Active {
            return Err(error("acp.prompt_not_active"));
        }
        match self.receive(timeout.min(self.limits.operation_timeout))? {
            None => Ok(None),
            Some(Event::ToolProgress(progress)) => Ok(Some(ConversationEvent::Progress(progress))),
            Some(Event::Finished(reason)) if reason == "end_turn" => {
                self.phase = AdapterPhase::Closed;
                Ok(Some(ConversationEvent::Finished))
            }
            Some(Event::Finished(_)) => Err(error("acp.unsuccessful_stop")),
            _ => Err(error("acp.unexpected_event")),
        }
    }

    /// Claim an admitted effect under the protocol-fault lock. The caller still
    /// repeats its permission, binding and scope checks through its executor.
    /// This admission claim isn't proof of dispatch. A downstream executor can
    /// still refuse before dispatch; only its observation/call evidence can prove
    /// an attempt. Claims never create tool-execution events or trusted usage.
    pub fn claim_dispatch(
        &mut self,
        request: &ProtectedEffectRequest,
    ) -> Result<(), RuntimePortError> {
        let mut evidence = self.evidence.0.lock().expect("evidence lock");
        if let Some(code) = &evidence.failure_code {
            return Err(RuntimePortError::before_dispatch(code));
        }
        if self.phase != AdapterPhase::Active || self.shared.cancellation.is_cancelled() {
            return Err(RuntimePortError::before_dispatch("acp.session_not_active"));
        }
        if evidence.run_request_digest.as_ref() != Some(&request.agent_run_request_digest) {
            return Err(RuntimePortError::before_dispatch(
                "acp.run_binding_mismatch",
            ));
        }
        evidence.dispatch_claims.push(digest(request)?);
        Ok(())
    }

    /// Retain prefix coordinates for the session metadata bound to this effect.
    pub fn effect_evidence(
        &self,
    ) -> Result<gaap::contracts::EffectEvidenceReference, RuntimePortError> {
        crate::session_effect_evidence(&self.evidence)
    }

    pub fn claim_completion(&mut self) -> Result<Option<Cancellation>, RuntimePortError> {
        self.check()?;
        if self.phase != AdapterPhase::Closed || self.evidence.snapshot().dispatch_claims.is_empty()
        {
            return Err(error("acp.no_completed_host_effect"));
        }
        Ok(self.shared.claim_completion())
    }

    fn check(&self) -> Result<(), RuntimePortError> {
        if let Some(code) = self.evidence.snapshot().failure_code {
            return Err(error(&code));
        }
        if self.shared.cancellation.is_cancelled() {
            return Err(error("acp.cancelled"));
        }
        Ok(())
    }

    fn receive(&self, timeout: Duration) -> Result<Option<Event>, RuntimePortError> {
        self.check()?;
        let receiver = self
            .receiver
            .as_ref()
            .ok_or_else(|| error("acp.not_started"))?;
        let event = match receiver.recv_timeout(timeout) {
            Ok(Event::Fault(code)) => return Err(error(&code)),
            Ok(event) => Some(event),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(_) => return Err(error("acp.agent_exit")),
        };
        self.check()?;
        Ok(event)
    }

    /// Reap Goose. Its proposal child must also obey the host channel's EOF/deadline.
    pub fn close(&mut self) {
        self.phase = AdapterPhase::Closed;
        self.start.take();
        self.shared.cancellation.cancel();
        if let Some(stopped) = self.stopped.take()
            && stopped.recv_timeout(Duration::from_secs(1)).is_err()
        {
            self.evidence.cleanup_timeout();
        }
    }
}

impl Drop for GooseConversation {
    fn drop(&mut self) {
        self.close();
    }
}
