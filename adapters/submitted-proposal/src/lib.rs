//! One supplied proposal through the existing Agent Run engine.
//!
//! The host parses task input, allocates identities, builds canonical requests,
//! and supplies policy, executor and verifier ports. This adapter holds no
//! provider, transport, task answers, persistence or lifecycle authority.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gaap::contracts::{
    AgentRunRequest, ContractSupport, EvidenceReference, EvidenceType, ProtectedEffectRequest,
    ResourceUsage, validate_protected_effect_request, validate_request,
};
use gaap::runtime::{
    AgentAdapter, AgentRunContext, AgentStep, Cancellation, PlanProposal, PortObservation,
    ProtectedEffectProposal, RuntimePortError,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

/// The supplied input's canonical bytes and its host-mapped plan/subject.
///
/// Construct this only after strict task deserialization. Keep the full supplied
/// input here so changing a plan or any task field changes the effect binding.
#[derive(Debug, Clone)]
pub struct SubmittedProposal {
    plan: String,
    base_digest: String,
    input_digest: String,
}

impl SubmittedProposal {
    pub fn new(
        input: &impl Serialize,
        plan: &str,
        base_digest: &str,
    ) -> Result<Self, RuntimePortError> {
        if plan.trim().is_empty() || plan.len() > 4096 {
            return Err(RuntimePortError::new("supply a plan of 1 to 4096 bytes"));
        }
        let bytes = serde_jcs::to_vec(input).map_err(|e| RuntimePortError::new(e.to_string()))?;
        Ok(Self {
            plan: plan.into(),
            base_digest: base_digest.into(),
            input_digest: digest(&bytes),
        })
    }

    pub fn input_digest(&self) -> &str {
        &self.input_digest
    }
}

/// Host-owned cancellation and deadline arbitration for one admitted run.
/// Task executors must check it immediately before their commit operation.
#[derive(Clone, Debug)]
pub struct RunControl(Arc<Mutex<ControlState>>);

#[derive(Debug)]
struct ControlState {
    deadline: Instant,
    cancellation: Option<Cancellation>,
    completed: bool,
}

impl RunControl {
    /// An unrepresentable deadline interrupts immediately.
    pub fn new(timeout: Duration) -> Self {
        let now = Instant::now();
        let deadline = now.checked_add(timeout);
        Self(Arc::new(Mutex::new(ControlState {
            deadline: deadline.unwrap_or(now),
            cancellation: deadline
                .is_none()
                .then(|| cancellation("host proposal timeout exceeds the monotonic clock range")),
            completed: false,
        })))
    }

    /// Record a caller cancellation; the caller can't choose its authority or evidence.
    pub fn cancel(&self) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if !state.completed && state.cancellation.is_none() {
            state.cancellation = Some(cancellation("caller cancelled the proposal"));
        }
    }

    pub fn cancellation(&self) -> Option<Cancellation> {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        Self::observe(&mut state)
    }

    fn observe(state: &mut ControlState) -> Option<Cancellation> {
        if !state.completed && state.cancellation.is_none() && Instant::now() >= state.deadline {
            state.cancellation = Some(cancellation("host proposal deadline exceeded"));
        }
        state.cancellation.clone()
    }

    fn claim_completion(&self) -> Option<Cancellation> {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let cancelled = Self::observe(&mut state);
        if cancelled.is_none() {
            state.completed = true;
        }
        cancelled
    }
}

/// A one-shot AgentAdapter bound to host-issued canonical requests.
pub struct SubmittedProposalAdapter {
    request_digest: String,
    support: ContractSupport,
    input_digest: String,
    effect: Option<ProtectedEffectRequest>,
    control: RunControl,
    started: bool,
}

impl SubmittedProposalAdapter {
    pub fn new(
        proposal: SubmittedProposal,
        request: &AgentRunRequest,
        effect: ProtectedEffectRequest,
        support: &ContractSupport,
        control: RunControl,
    ) -> Result<Self, RuntimePortError> {
        let error = |message| RuntimePortError::new(message);
        let request_digest =
            validate_request(request, support).map_err(|e| error(e.to_string()))?;
        validate_protected_effect_request(request, support, &effect)
            .map_err(|e| error(e.to_string()))?;
        if request.task.instructions != proposal.plan
            || request.subject.digest != proposal.base_digest
            || effect.subject != request.subject
            || effect.capability != request.requested_capability
            || effect.policies != request.policies
            || effect.input_digest != proposal.input_digest
            || effect.effect_sequence != 1
        {
            return Err(error(
                "proposal binding differs from the host request; inspect the current subject before submitting again".into(),
            ));
        }
        Ok(Self {
            request_digest,
            support: support.clone(),
            input_digest: proposal.input_digest,
            effect: Some(effect),
            control,
            started: false,
        })
    }
}

impl AgentAdapter for SubmittedProposalAdapter {
    fn cancellation(&self) -> Option<Cancellation> {
        self.control.cancellation()
    }

    fn claim_completion(&mut self) -> Result<Option<Cancellation>, RuntimePortError> {
        Ok(self.control.claim_completion())
    }

    fn plan(
        &mut self,
        request: &AgentRunRequest,
    ) -> Result<PortObservation<PlanProposal>, RuntimePortError> {
        let actual = validate_request(request, &self.support)
            .map_err(|e| RuntimePortError::new(e.to_string()))?;
        if self.started || actual != self.request_digest {
            return Err(RuntimePortError::new(
                "adapter is bound to one host request and can't be replayed",
            ));
        }
        self.started = true;
        Ok(PortObservation {
            value: PlanProposal {
                plan_digest: self.input_digest.clone(),
            },
            usage: usage(),
        })
    }

    fn next_effect(
        &mut self,
        _: AgentRunContext<'_>,
    ) -> Result<PortObservation<AgentStep>, RuntimePortError> {
        if !self.started {
            return Err(RuntimePortError::new("plan the bound host request first"));
        }
        let value = if let Some(cancelled) = self.control.cancellation() {
            AgentStep::Cancel(cancelled)
        } else {
            self.effect.take().map_or(AgentStep::Finish, |request| {
                AgentStep::ProtectedEffect(Box::new(ProtectedEffectProposal { request }))
            })
        };
        Ok(PortObservation {
            value,
            usage: usage(),
        })
    }
}

fn usage() -> ResourceUsage {
    // This adapter performs no model inference or tool execution. These zeros
    // account only for its work; external conversation usage stays unavailable.
    ResourceUsage {
        cost_micros: 0,
        model_tokens: 0,
        elapsed_ms: 0,
        tool_calls: 0,
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn cancellation(reason: &str) -> Cancellation {
    Cancellation {
        actor_id: None,
        reason: reason.into(),
        evidence: EvidenceReference {
            evidence_type: EvidenceType::Interruption,
            digest: digest(reason.as_bytes()),
            locator: Some("gaap:host/proposal-control".into()),
        },
    }
}
