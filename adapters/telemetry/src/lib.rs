//! Optional, lossy telemetry from already-sealed Agent Run executions.
//! This crate has no engine, effect, verification or canonical-state writer.
#![doc = include_str!("../README.md")]

use std::net::SocketAddr;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gaap::contracts::{RunEvent, VerificationVerdict};
use gaap::runtime::AgentRunExecution;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const OTEL_PROTOCOL_VERSION: &str = "1.9.0";
pub const SEMCONV_VERSION: &str = "1.42.0";
pub const SCHEMA_URL: &str = "https://opentelemetry.io/schemas/1.42.0";
pub const MAX_EVENTS: usize = 1024;
pub const MAX_EFFECTS: usize = 128;
pub const MAX_PAYLOAD_BYTES: usize = 1_048_576;
const MAX_ID_BYTES: usize = 4096;

/// Host-supplied identities. Absent identities produce no attributes or parents.
/// Raw values are domain-separated SHA-256 hashes in the projection.
#[derive(Debug, Clone, Default)]
pub struct Correlation {
    pub workflow_run: Option<String>,
    pub action_request: Option<String>,
    pub execution_claim: Option<String>,
    pub attempt: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionError {
    TooLarge,
    InvalidDigest,
}

impl std::fmt::Display for ProjectionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::TooLarge => "telemetry projection exceeds a size limit; retain native evidence and reduce the observed batch",
            Self::InvalidDigest => "telemetry source contains a malformed digest; inspect the native sealed execution",
        })
    }
}
impl std::error::Error for ProjectionError {}

fn identity(namespace: &str, value: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(namespace.as_bytes());
    hash.update([0]);
    hash.update(value.as_bytes());
    format!("{:x}", hash.finalize())
}

fn attribute(key: &str, value: impl Into<String>) -> Value {
    json!({"key":key,"value":{"stringValue":value.into()}})
}

fn number(key: &str, value: u64) -> Value {
    json!({"key":key,"value":{"intValue":value.to_string()}})
}

fn is_digest(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value.as_bytes()[7..]
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

fn digest_attribute(attrs: &mut Vec<Value>, key: &str, value: &str) -> Result<(), ProjectionError> {
    if !is_digest(value) {
        return Err(ProjectionError::InvalidDigest);
    }
    attrs.push(attribute(key, value));
    Ok(())
}

fn event_projection(event: &RunEvent, timestamp: &str) -> Result<Value, ProjectionError> {
    let mut attrs = vec![number("gaap.event.sequence", event.sequence())];
    let name = match event {
        RunEvent::StatusTransition { from, to, .. } => {
            attrs.push(attribute("gaap.status.from", json!(from).as_str().unwrap()));
            attrs.push(attribute("gaap.status.to", json!(to).as_str().unwrap()));
            "gaap.status_transition"
        }
        RunEvent::PlanRecorded { plan_digest, .. } => {
            digest_attribute(&mut attrs, "gaap.plan.digest", plan_digest)?;
            "gaap.plan_recorded"
        }
        RunEvent::ApprovalRecorded { approval, .. } => {
            digest_attribute(
                &mut attrs,
                "gaap.approval.subject_digest",
                &approval.subject_digest,
            )?;
            "gaap.approval_recorded"
        }
        RunEvent::ProtectedEffectDecision {
            gate,
            protected_effect_digest,
            subject_digest,
            decision,
            ..
        } => {
            attrs.push(attribute("gaap.gate", json!(gate).as_str().unwrap()));
            attrs.push(attribute(
                "gaap.decision",
                json!(decision.outcome).as_str().unwrap(),
            ));
            digest_attribute(
                &mut attrs,
                "gaap.effect.request_digest",
                protected_effect_digest,
            )?;
            digest_attribute(&mut attrs, "gaap.subject.digest", subject_digest)?;
            "gaap.protected_effect_decision"
        }
        RunEvent::ToolExecution {
            protected_effect_digest,
            action_digest,
            capability_digest,
            ..
        } => {
            digest_attribute(
                &mut attrs,
                "gaap.effect.request_digest",
                protected_effect_digest,
            )?;
            digest_attribute(&mut attrs, "gaap.action.digest", action_digest)?;
            digest_attribute(&mut attrs, "gaap.capability.digest", capability_digest)?;
            "gaap.tool_execution"
        }
        RunEvent::Mutation {
            protected_effect_digest,
            before_subject_digest,
            after_subject_digest,
            ..
        } => {
            digest_attribute(
                &mut attrs,
                "gaap.effect.request_digest",
                protected_effect_digest,
            )?;
            digest_attribute(
                &mut attrs,
                "gaap.subject.before_digest",
                before_subject_digest,
            )?;
            digest_attribute(
                &mut attrs,
                "gaap.subject.after_digest",
                after_subject_digest,
            )?;
            "gaap.mutation"
        }
        RunEvent::Verification {
            subject_digest,
            verdict,
            ..
        } => {
            digest_attribute(
                &mut attrs,
                "gaap.verification.subject_digest",
                subject_digest,
            )?;
            attrs.push(attribute(
                "gaap.verification.verdict",
                match verdict {
                    VerificationVerdict::Pass => "pass",
                    VerificationVerdict::Fail => "fail",
                },
            ));
            "gaap.verification"
        }
        RunEvent::Usage { usage, .. } => {
            attrs.push(number("gaap.usage.elapsed_ms", usage.elapsed_ms));
            attrs.push(number("gaap.usage.tool_calls", usage.tool_calls));
            "gaap.usage"
        }
        RunEvent::Interruption { .. } => "gaap.interruption",
    };
    Ok(json!({"name":name,"timeUnixNano":timestamp,"attributes":attrs}))
}

/// Build OTLP/HTTP JSON from a sealed execution without modifying its source.
/// Timestamps describe projection time, never reconstructed runtime duration.
pub fn project(
    execution: &AgentRunExecution,
    correlation: &Correlation,
    projected_at_unix_nanos: u64,
) -> Result<Vec<u8>, ProjectionError> {
    let receipt = &execution.receipt;
    let body = &receipt.body;
    if body.events.len() > MAX_EVENTS
        || execution.protected_effect_results.len() > MAX_EFFECTS
        || body.run_id.len() > MAX_ID_BYTES
        || body.request_id.len() > MAX_ID_BYTES
        || execution
            .protected_effect_results
            .iter()
            .any(|effect| effect.body.effect_id.len() > MAX_ID_BYTES)
    {
        return Err(ProjectionError::TooLarge);
    }
    if !is_digest(&receipt.receipt_digest) {
        return Err(ProjectionError::InvalidDigest);
    }
    let trace_id = identity("gaap.trace.receipt", &receipt.receipt_digest)[..32].to_owned();
    let span_id = identity("gaap.span.receipt", &receipt.receipt_digest)[..16].to_owned();
    let mut attrs = vec![
        attribute("gaap.run.id_sha256", identity("gaap.run", &body.run_id)),
        attribute(
            "gaap.request.id_sha256",
            identity("gaap.request", &body.request_id),
        ),
        attribute(
            "gaap.run.terminal_status",
            json!(body.terminal_status).as_str().unwrap(),
        ),
        attribute("gaap.telemetry.timing", "projection_time_only"),
        attribute("gaap.telemetry.authority", "none"),
        attribute("gaap.usage.model_scope", "unavailable"),
        attribute(
            "gaap.run.reason",
            match body.terminal_reason.as_str() {
                "verification.stale_subject" => "verification.stale_subject",
                "workflow.completion_authorized" => "workflow.completion_authorized",
                "permission.policy_requires_approval" => "permission.policy_requires_approval",
                "permission.policy_denied" => "permission.policy_denied",
                "permission.denied" => "permission.denied",
                "runtime.interrupted" => "runtime.interrupted",
                _ => "other_omitted",
            },
        ),
    ];
    digest_attribute(&mut attrs, "gaap.receipt.digest", &receipt.receipt_digest)?;
    digest_attribute(&mut attrs, "gaap.request.digest", &body.request_digest)?;
    digest_attribute(
        &mut attrs,
        "gaap.subject.initial_digest",
        &body.initial_subject_digest,
    )?;
    digest_attribute(
        &mut attrs,
        "gaap.subject.resulting_digest",
        &body.resulting_subject_digest,
    )?;
    // Relative, token-free navigation hint. Viewer routing is evaluated separately.
    attrs.push(attribute(
        "gaap.evidence.ref",
        format!("receipts/{}", &receipt.receipt_digest[7..]),
    ));
    for (key, value) in [
        (
            "threadloop.workflow_run.id_sha256",
            &correlation.workflow_run,
        ),
        (
            "threadloop.action_request.id_sha256",
            &correlation.action_request,
        ),
        (
            "threadloop.execution_claim.id_sha256",
            &correlation.execution_claim,
        ),
        ("threadloop.attempt.id_sha256", &correlation.attempt),
    ] {
        if let Some(value) = value {
            if value.len() > MAX_ID_BYTES {
                return Err(ProjectionError::TooLarge);
            }
            attrs.push(attribute(key, identity(key, value)));
        }
    }
    let timestamp = projected_at_unix_nanos.to_string();
    let events = body
        .events
        .iter()
        .map(|event| event_projection(event, &timestamp))
        .collect::<Result<Vec<_>, _>>()?;
    let mut spans = vec![json!({
        "traceId":trace_id,"spanId":span_id,"name":"gaap.agent_run","kind":1,
        "startTimeUnixNano":timestamp,"endTimeUnixNano":timestamp,
        "attributes":attrs,"events":events,"status":{"code":0}
    })];
    for effect in &execution.protected_effect_results {
        let mut attrs = vec![
            attribute(
                "gaap.effect.id_sha256",
                identity("gaap.effect", &effect.body.effect_id),
            ),
            number("gaap.effect.sequence", effect.body.effect_sequence),
            attribute(
                "gaap.effect.decision",
                json!(effect.body.decision.decision.outcome)
                    .as_str()
                    .unwrap(),
            ),
            attribute(
                "gaap.effect.execution_status",
                json!(effect.body.execution_status).as_str().unwrap(),
            ),
            attribute("gaap.telemetry.timing", "projection_time_only"),
        ];
        digest_attribute(
            &mut attrs,
            "gaap.effect.result_digest",
            &effect.result_digest,
        )?;
        digest_attribute(
            &mut attrs,
            "gaap.effect.request_digest",
            &effect.body.effect_request_digest,
        )?;
        digest_attribute(
            &mut attrs,
            "gaap.capability.digest",
            &effect.body.observed_capability.digest,
        )?;
        if let Some(schema) = &effect.body.observed_tool_schema_digest {
            digest_attribute(&mut attrs, "gaap.tool.schema_digest", schema)?;
        }
        if let Some(sandbox) = &effect.body.sandbox_profile {
            digest_attribute(&mut attrs, "gaap.sandbox.profile_digest", &sandbox.digest)?;
        }
        spans.push(json!({
            "traceId":trace_id,"spanId":identity("gaap.span.effect", &effect.result_digest)[..16],
            "parentSpanId":span_id,"name":"gaap.protected_effect","kind":1,
            "startTimeUnixNano":timestamp,"endTimeUnixNano":timestamp,
            "attributes":attrs,"status":{"code":0}
        }));
    }
    let bytes = serde_json::to_vec(&json!({"resourceSpans":[{
        "resource":{"attributes":[
            attribute("service.name", "gaap"),
            attribute("service.version", env!("CARGO_PKG_VERSION")),
            attribute("gaap.otel.protocol.version", OTEL_PROTOCOL_VERSION),
            attribute("gaap.otel.semconv.version", SEMCONV_VERSION),
            attribute("gaap.telemetry.projection.version", "0.1.0")
        ]},"schemaUrl":SCHEMA_URL,"scopeSpans":[{
            "scope":{"name":"gaap.telemetry","version":env!("CARGO_PKG_VERSION")},
            "schemaUrl":SCHEMA_URL,"spans":spans
        }]
    }]}))
    .expect("allowlisted OTLP values serialize");
    if bytes.len() > MAX_PAYLOAD_BYTES {
        return Err(ProjectionError::TooLarge);
    }
    Ok(bytes)
}

#[derive(Debug, Clone, Copy)]
pub struct QueueConfig {
    pub capacity: usize,
    /// Zero samples nothing, one includes all, N includes a stable 1/N subset.
    pub sample_every: u32,
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            capacity: 16,
            sample_every: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    Delivered,
    Partial,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Submission {
    Disabled,
    SampledOut,
    Queued,
    Full,
    Disconnected,
    Invalid(ProjectionError),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Statistics {
    pub queued: u64,
    pub dropped: u64,
    pub sampled_out: u64,
    pub delivered: u64,
    pub partial: u64,
    pub failed: u64,
}

#[derive(Default)]
struct Counters {
    queued: AtomicU64,
    dropped: AtomicU64,
    sampled_out: AtomicU64,
    delivered: AtomicU64,
    partial: AtomicU64,
    failed: AtomicU64,
}

// Keep each reservation through construction, queueing and sink execution.
struct AdmissionSlot(Arc<AtomicUsize>);

impl Drop for AdmissionSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Release);
    }
}

struct Batch {
    // Release the allocation before releasing its admission slot.
    bytes: Vec<u8>,
    _slot: AdmissionSlot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shutdown {
    Complete,
    TimedOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigError {
    Capacity,
    Endpoint,
    Timeout,
    Worker,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Capacity => "telemetry queue capacity must be between 1 and 256; choose a bounded capacity",
            Self::Endpoint => "telemetry endpoint must be a loopback socket with a nonzero port; use 127.0.0.1:4318 or [::1]:4318",
            Self::Timeout => "telemetry HTTP timeout must be greater than zero and at most five seconds; choose a bounded timeout",
            Self::Worker => "telemetry worker couldn't start; continue with disabled export and inspect host resource limits",
        })
    }
}
impl std::error::Error for ConfigError {}

/// An opt-in worker. Default construction starts no worker and opens no socket.
pub struct Telemetry {
    sender: Option<SyncSender<Batch>>,
    worker: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    counters: Arc<Counters>,
    in_flight: Arc<AtomicUsize>,
    max_in_flight: usize,
    sample_every: u32,
    enabled: bool,
}

impl Default for Telemetry {
    fn default() -> Self {
        Self::disabled()
    }
}

impl Telemetry {
    pub fn disabled() -> Self {
        Self {
            sender: None,
            worker: None,
            stop: Arc::default(),
            counters: Arc::default(),
            in_flight: Arc::default(),
            max_in_flight: 0,
            sample_every: 1,
            enabled: false,
        }
    }

    /// Inject a sink for deterministic tests or another read-only transport.
    /// A blocked custom sink can retain one worker until that sink returns.
    pub fn with_sink(
        config: QueueConfig,
        mut sink: impl FnMut(&[u8]) -> Delivery + Send + 'static,
    ) -> Result<Self, ConfigError> {
        if !(1..=256).contains(&config.capacity) {
            return Err(ConfigError::Capacity);
        }
        let (sender, receiver) = mpsc::sync_channel::<Batch>(config.capacity);
        let stop = Arc::new(AtomicBool::new(false));
        let counters = Arc::new(Counters::default());
        let worker_stop = stop.clone();
        let worker_counters = counters.clone();
        let worker = thread::Builder::new()
            .name("gaap-telemetry".into())
            .spawn(move || {
                while !worker_stop.load(Ordering::Relaxed) {
                    match receiver.recv_timeout(Duration::from_millis(20)) {
                        Ok(batch) => {
                            let delivery = catch_unwind(AssertUnwindSafe(|| sink(&batch.bytes)))
                                .unwrap_or(Delivery::Failed);
                            let counter = match delivery {
                                Delivery::Delivered => &worker_counters.delivered,
                                Delivery::Partial => &worker_counters.partial,
                                Delivery::Failed => &worker_counters.failed,
                            };
                            counter.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                }
            })
            .map_err(|_| ConfigError::Worker)?;
        Ok(Self {
            sender: Some(sender),
            worker: Some(worker),
            stop,
            counters,
            in_flight: Arc::default(),
            max_in_flight: config.capacity + 1,
            sample_every: config.sample_every,
            enabled: true,
        })
    }

    /// Local OTLP JSON only: no DNS, redirects, proxy, credentials or environment overrides.
    pub fn otlp(
        endpoint: SocketAddr,
        timeout: Duration,
        config: QueueConfig,
    ) -> Result<Self, ConfigError> {
        if !endpoint.ip().is_loopback() || endpoint.port() == 0 {
            return Err(ConfigError::Endpoint);
        }
        if timeout.is_zero() || timeout > Duration::from_secs(5) {
            return Err(ConfigError::Timeout);
        }
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .max_redirects(0)
            .proxy(None)
            .build()
            .into();
        let url = format!("http://{endpoint}/v1/traces");
        Self::with_sink(config, move |bytes| {
            let Ok(mut response) = agent
                .post(&url)
                .header("Content-Type", "application/json")
                .send(bytes)
            else {
                return Delivery::Failed;
            };
            if response.status().as_u16() != 200 {
                return Delivery::Failed;
            }
            let Ok(body) = response.body_mut().with_config().limit(8192).read_to_vec() else {
                return Delivery::Failed;
            };
            let Ok(value) = serde_json::from_slice::<Value>(&body) else {
                return Delivery::Failed;
            };
            if !value.is_object() {
                return Delivery::Failed;
            }
            match value.get("partialSuccess") {
                None | Some(Value::Null) => Delivery::Delivered,
                Some(Value::Object(partial)) => {
                    let count = match partial.get("rejectedSpans") {
                        None | Some(Value::Null) => Some(0),
                        Some(Value::String(count)) => count.parse::<i64>().ok(),
                        Some(Value::Number(count)) => count.as_i64(),
                        Some(_) => None,
                    };
                    let message = match partial.get("errorMessage") {
                        None | Some(Value::Null) => Some(""),
                        Some(Value::String(message)) => Some(message.as_str()),
                        Some(_) => None,
                    };
                    match (count, message) {
                        (Some(0), Some("")) => Delivery::Delivered,
                        (Some(count), Some(_)) if count >= 0 => Delivery::Partial,
                        _ => Delivery::Failed,
                    }
                }
                Some(_) => Delivery::Failed,
            }
        })
    }

    /// Call after the engine returns. No exporter result is a runtime decision.
    pub fn observe(&self, execution: &AgentRunExecution, correlation: &Correlation) -> Submission {
        if !self.enabled {
            return Submission::Disabled;
        }
        let Some(sender) = &self.sender else {
            return Submission::Disconnected;
        };
        if !is_digest(&execution.receipt.receipt_digest) {
            self.counters.dropped.fetch_add(1, Ordering::Relaxed);
            return Submission::Invalid(ProjectionError::InvalidDigest);
        }
        let sample = Sha256::digest(execution.receipt.receipt_digest.as_bytes());
        let number = u32::from_be_bytes(sample[..4].try_into().unwrap());
        if self.sample_every == 0 || number % self.sample_every != 0 {
            self.counters.sampled_out.fetch_add(1, Ordering::Relaxed);
            return Submission::SampledOut;
        }
        if self
            .in_flight
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                (used < self.max_in_flight).then_some(used + 1)
            })
            .is_err()
        {
            self.counters.dropped.fetch_add(1, Ordering::Relaxed);
            return Submission::Full;
        }
        let slot = AdmissionSlot(self.in_flight.clone());
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .min(u64::MAX as u128) as u64;
        let bytes = match project(execution, correlation, timestamp) {
            Ok(bytes) => bytes,
            Err(error) => {
                self.counters.dropped.fetch_add(1, Ordering::Relaxed);
                return Submission::Invalid(error);
            }
        };
        match sender.try_send(Batch { bytes, _slot: slot }) {
            Ok(()) => {
                self.counters.queued.fetch_add(1, Ordering::Relaxed);
                Submission::Queued
            }
            Err(TrySendError::Full(_)) => {
                self.counters.dropped.fetch_add(1, Ordering::Relaxed);
                Submission::Full
            }
            Err(TrySendError::Disconnected(_)) => {
                self.counters.dropped.fetch_add(1, Ordering::Relaxed);
                Submission::Disconnected
            }
        }
    }

    pub fn statistics(&self) -> Statistics {
        let counters = &self.counters;
        Statistics {
            queued: counters.queued.load(Ordering::Relaxed),
            dropped: counters.dropped.load(Ordering::Relaxed),
            sampled_out: counters.sampled_out.load(Ordering::Relaxed),
            delivered: counters.delivered.load(Ordering::Relaxed),
            partial: counters.partial.load(Ordering::Relaxed),
            failed: counters.failed.load(Ordering::Relaxed),
        }
    }

    /// Stop admission and drain for at most this deadline, outside runtime execution.
    pub fn shutdown(&mut self, timeout: Duration) -> Shutdown {
        self.sender.take();
        let started = Instant::now();
        while self
            .worker
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
        {
            if started.elapsed() >= timeout {
                self.stop.store(true, Ordering::Relaxed);
                return Shutdown::TimedOut;
            }
            thread::sleep(Duration::from_millis(1));
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        Shutdown::Complete
    }
}

impl Drop for Telemetry {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.sender.take();
        // Dropping a JoinHandle detaches; a blocked sink can't hold runtime exit.
    }
}
