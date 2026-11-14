use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::rc::Rc;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use gaap::contracts::*;
use gaap::runtime::*;
use gaap_telemetry::*;
use serde_json::{Value, json};

// Count allocations only on the probing thread; other tests keep using System.
struct TrackingAllocator;
thread_local! {
    static TRACK_ALLOCATIONS: Cell<bool> = const { Cell::new(false) };
    static ALLOCATED_BYTES: Cell<usize> = const { Cell::new(0) };
}
fn record_allocation(bytes: usize) {
    if TRACK_ALLOCATIONS.try_with(Cell::get).unwrap_or(false) {
        let _ = ALLOCATED_BYTES.try_with(|total| total.set(total.get().saturating_add(bytes)));
    }
}
// SAFETY: every allocation operation delegates unchanged to the system allocator.
unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record_allocation(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record_allocation(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record_allocation(size);
        unsafe { System.realloc(pointer, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator;
struct AllocationProbe;
impl Drop for AllocationProbe {
    fn drop(&mut self) {
        TRACK_ALLOCATIONS.with(|enabled| enabled.set(false));
    }
}
fn allocated_by(action: impl FnOnce() -> Submission) -> (Submission, usize) {
    ALLOCATED_BYTES.with(|total| total.set(0));
    TRACK_ALLOCATIONS.with(|enabled| enabled.set(true));
    let probe = AllocationProbe;
    let result = action();
    drop(probe);
    (result, ALLOCATED_BYTES.with(Cell::get))
}

#[derive(Clone, Copy, Debug)]
enum Scenario {
    Completed,
    Denied,
    Ask,
    Failed,
    Interrupted,
    Unknown,
    Stale,
    Unverified,
}
const SCENARIOS: [Scenario; 8] = [
    Scenario::Completed,
    Scenario::Denied,
    Scenario::Ask,
    Scenario::Failed,
    Scenario::Interrupted,
    Scenario::Unknown,
    Scenario::Stale,
    Scenario::Unverified,
];

struct Agent {
    steps: VecDeque<AgentStep>,
}
impl AgentAdapter for Agent {
    fn plan(
        &mut self,
        _: &AgentRunRequest,
    ) -> Result<PortObservation<PlanProposal>, RuntimePortError> {
        Ok(PortObservation {
            value: PlanProposal {
                plan_digest: hash('1'),
            },
            usage: zero(),
        })
    }
    fn next_effect(
        &mut self,
        _: AgentRunContext<'_>,
    ) -> Result<PortObservation<AgentStep>, RuntimePortError> {
        Ok(PortObservation {
            value: self.steps.pop_front().unwrap(),
            usage: zero(),
        })
    }
}
fn hash(character: char) -> String {
    format!("sha256:{}", character.to_string().repeat(64))
}
fn zero() -> ResourceUsage {
    ResourceUsage {
        cost_micros: 0,
        elapsed_ms: 0,
        model_tokens: 0,
        tool_calls: 0,
    }
}
fn evidence(kind: EffectEvidenceType) -> EffectEvidenceReference {
    EffectEvidenceReference {
        evidence_type: kind,
        digest: hash('2'),
        locator: Some("/secret/path?token=secret".into()),
    }
}
struct Executor {
    scenario: Scenario,
    calls: Rc<RefCell<Vec<String>>>,
}
impl ExecutorPort for Executor {
    fn execute(
        &mut self,
        effect: &ProtectedEffectRequest,
    ) -> Result<ExecutorObservation, RuntimePortError> {
        self.calls.borrow_mut().push(effect.effect_id.clone());
        if matches!(self.scenario, Scenario::Unknown) {
            return Err(RuntimePortError::new("secret tool output"));
        }
        let mut subject = effect.subject.clone();
        subject.digest = hash('3');
        let status = match self.scenario {
            Scenario::Failed => EffectExecutionStatus::Failed,
            Scenario::Interrupted => EffectExecutionStatus::Interrupted,
            _ => EffectExecutionStatus::Executed,
        };
        let mut evidence = vec![
            evidence(EffectEvidenceType::Executor),
            evidence(EffectEvidenceType::Sandbox),
            evidence(EffectEvidenceType::Mutation),
            evidence(EffectEvidenceType::Artifact),
            evidence(EffectEvidenceType::SubjectObservation),
            evidence(EffectEvidenceType::Usage),
            evidence(EffectEvidenceType::Output),
            evidence(EffectEvidenceType::Exit),
        ];
        if status == EffectExecutionStatus::Failed {
            evidence.push(self::evidence(EffectEvidenceType::Failure));
        }
        if status == EffectExecutionStatus::Interrupted {
            evidence.push(self::evidence(EffectEvidenceType::Interruption));
        }
        Ok(ExecutorObservation {
            execution_status: status,
            observed_post_effect_subject: Some(subject),
            exit: Some(EffectExit::Code { code: 0 }),
            usage: EffectUsage {
                cost_micros: 10,
                elapsed_ms: 1,
                model_tokens: 0,
                tool_calls: 1,
            },
            executor: Some(ExecutorIdentity {
                name: "fixture".into(),
                version: "1".into(),
                digest: hash('4'),
            }),
            sandbox_profile: Some(effect.sandbox_profile.clone()),
            reason: if status == EffectExecutionStatus::Executed {
                None
            } else {
                Some("secret tool payload".into())
            },
            evidence,
        })
    }
}
struct Verifier {
    scenario: Scenario,
}
impl VerifierPort for Verifier {
    fn verify(
        &mut self,
        context: VerificationContext<'_>,
    ) -> Result<PortObservation<VerificationReport>, RuntimePortError> {
        if matches!(self.scenario, Scenario::Unverified) {
            return Err(RuntimePortError::new("secret verifier output"));
        }
        Ok(PortObservation {
            value: VerificationReport {
                subject_digest: if matches!(self.scenario, Scenario::Stale) {
                    hash('5')
                } else {
                    context.current_subject.digest.clone()
                },
                verifier_id: "independent".into(),
                verdict: VerificationVerdict::Pass,
                evidence: vec![
                    EvidenceReference {
                        evidence_type: EvidenceType::CommandOutput,
                        digest: hash('6'),
                        locator: None,
                    },
                    EvidenceReference {
                        evidence_type: EvidenceType::Artifact,
                        digest: hash('7'),
                        locator: None,
                    },
                ],
            },
            usage: zero(),
        })
    }
}
fn run(scenario: Scenario) -> (AgentRunExecution, Vec<String>) {
    let mut request: AgentRunRequest = serde_json::from_str(include_str!(
        "../../../examples/contracts/v0.1.0/agent-run-request.json"
    ))
    .unwrap();
    request.task.instructions = "secret prompt and repository contents".into();
    request.run_id = "/secret/run?token=secret".into();
    let support = ContractSupport::new(request.policies.clone());
    let mut effect: ProtectedEffectRequest = serde_json::from_str(include_str!(
        "../../../examples/contracts/protected-effect/v0.1.0/protected-effect-request.json"
    ))
    .unwrap();
    effect.run_id = request.run_id.clone();
    effect.agent_run_request_digest = validate_request(&request, &support).unwrap();
    let calls = Rc::new(RefCell::new(Vec::new()));
    let policy = match scenario {
        Scenario::Denied => PolicyDecision::Deny,
        Scenario::Ask => PolicyDecision::Ask,
        _ => PolicyDecision::Allow,
    };
    let runtime_support = RuntimeSupport::new(
        support.clone(),
        vec![request.requested_capability.clone()],
        PermissionPolicy {
            policy_decision: Some(policy),
            risk_tags: vec![],
            wrapper_chain: vec![],
            approvals: vec![],
        },
        EffectUsage {
            cost_micros: 10,
            elapsed_ms: 1,
            model_tokens: 0,
            tool_calls: 1,
        },
        vec![],
        vec!["independent".into()],
    );
    let config = RuntimeConfig {
        implementer_id: "fixture".into(),
        executor_identity: ExecutorIdentity {
            name: "fixture".into(),
            version: "1".into(),
            digest: hash('4'),
        },
        max_effects: 2,
        budget_thresholds: BudgetThresholds {
            warn_micros: 800_000,
            approval_micros: 900_000,
            hard_stop_micros: 1_000_000,
        },
    };
    let mut engine = AgentRunEngine::new(
        runtime_support,
        config,
        Agent {
            steps: VecDeque::from([
                AgentStep::ProtectedEffect(Box::new(ProtectedEffectProposal {
                    request: effect.clone(),
                })),
                AgentStep::Finish,
            ]),
        },
        Executor {
            scenario,
            calls: calls.clone(),
        },
        Verifier { scenario },
    );
    let execution = engine.run(request.clone()).unwrap();
    verify_terminal_receipt(&request, &support, &execution.receipt).unwrap();
    for result in &execution.protected_effect_results {
        verify_protected_effect_result(&request, &support, &effect, result).unwrap();
    }
    let recorded = calls.borrow().clone();
    (execution, recorded)
}
fn attributes(value: &Value) -> std::collections::BTreeMap<String, Value> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|v| (v["key"].as_str().unwrap().into(), v["value"].clone()))
        .collect()
}
fn document(execution: &AgentRunExecution) -> Value {
    serde_json::from_slice(
        &project(
            execution,
            &Correlation::default(),
            1_800_000_000_000_000_000,
        )
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn real_engine_outcomes_project_without_collapsing_authority_execution_or_verification() {
    for scenario in SCENARIOS {
        let (execution, calls) = run(scenario);
        let before = serde_jcs::to_vec(&execution.receipt).unwrap();
        let doc = document(&execution);
        let spans = &doc["resourceSpans"][0]["scopeSpans"][0]["spans"];
        let attrs = attributes(&spans[0]["attributes"]);
        assert_eq!(
            attrs["gaap.run.terminal_status"]["stringValue"],
            json!(execution.receipt.body.terminal_status),
            "{scenario:?}"
        );
        let effect_attrs = attributes(&spans[1]["attributes"]);
        assert_eq!(
            effect_attrs["gaap.effect.execution_status"]["stringValue"],
            json!(execution.protected_effect_results[0].body.execution_status)
        );
        assert_eq!(spans[0]["status"]["code"], 0);
        assert!(spans[0].get("parentSpanId").is_none());
        assert_eq!(
            spans[0]["events"].as_array().unwrap().len(),
            execution.receipt.body.events.len()
        );
        assert_eq!(before, serde_jcs::to_vec(&execution.receipt).unwrap());
        match scenario {
            Scenario::Completed => assert_eq!(
                execution.receipt.body.terminal_status,
                AgentRunStatus::Completed
            ),
            Scenario::Denied | Scenario::Ask => {
                assert!(calls.is_empty());
                assert_eq!(
                    execution.receipt.body.terminal_status,
                    AgentRunStatus::Blocked
                );
            }
            Scenario::Stale => assert_eq!(
                attrs["gaap.run.reason"]["stringValue"],
                "verification.stale_subject"
            ),
            Scenario::Unverified => {
                assert_ne!(
                    execution.receipt.body.terminal_status,
                    AgentRunStatus::Completed
                );
                assert_eq!(
                    execution.protected_effect_results[0].body.execution_status,
                    EffectExecutionStatus::Executed
                );
            }
            Scenario::Failed => assert_eq!(
                execution.receipt.body.terminal_status,
                AgentRunStatus::Failed
            ),
            Scenario::Interrupted => assert_eq!(
                execution.receipt.body.terminal_status,
                AgentRunStatus::Interrupted
            ),
            Scenario::Unknown => assert_eq!(
                execution.protected_effect_results[0].body.execution_status,
                EffectExecutionStatus::UnknownOutcome
            ),
        }
    }
}

#[test]
fn default_redaction_versions_correlation_and_projection_times_are_explicit() {
    let (execution, _) = run(Scenario::Completed);
    let correlation = Correlation {
        workflow_run: Some("secret-workflow".into()),
        action_request: Some("secret-action".into()),
        execution_claim: Some("secret-claim".into()),
        attempt: Some("secret-attempt".into()),
    };
    let bytes = project(&execution, &correlation, 12345).unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    for omitted in [
        "secret",
        "example.invalid",
        "/workspace",
        "/usr/bin",
        "tool payload",
        "actor_id",
        "instructions",
        "cost_micros",
        "model_tokens",
        "gen_ai.",
        "locator",
    ] {
        assert!(!text.contains(omitted), "{omitted}");
    }
    let doc: Value = serde_json::from_slice(&bytes).unwrap();
    let resource = attributes(&doc["resourceSpans"][0]["resource"]["attributes"]);
    assert_eq!(
        resource["gaap.otel.protocol.version"]["stringValue"],
        OTEL_PROTOCOL_VERSION
    );
    assert_eq!(
        resource["gaap.otel.semconv.version"]["stringValue"],
        SEMCONV_VERSION
    );
    let spans = &doc["resourceSpans"][0]["scopeSpans"][0]["spans"];
    assert_eq!(spans[0]["startTimeUnixNano"], "12345");
    assert_eq!(spans[0]["endTimeUnixNano"], "12345");
    let attrs = attributes(&spans[0]["attributes"]);
    for key in [
        "threadloop.workflow_run.id_sha256",
        "threadloop.action_request.id_sha256",
        "threadloop.execution_claim.id_sha256",
        "threadloop.attempt.id_sha256",
    ] {
        assert_eq!(attrs[key]["stringValue"].as_str().unwrap().len(), 64);
    }
    let absent = document(&execution).to_string();
    assert!(!absent.contains("threadloop."));
    assert_eq!(bytes, project(&execution, &correlation, 12345).unwrap());
    assert_eq!(spans[0]["traceId"].as_str().unwrap().len(), 32);
    assert_eq!(spans[0]["spanId"].as_str().unwrap().len(), 16);
}

#[test]
fn blocked_sink_sampling_overflow_shutdown_and_disable_leave_engine_results_identical() {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let mut blocked = Telemetry::with_sink(
        QueueConfig {
            capacity: 1,
            sample_every: 1,
        },
        move |_| {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Delivery::Failed
        },
    )
    .unwrap();
    let (execution, _) = run(Scenario::Completed);
    assert_eq!(
        blocked.observe(&execution, &Correlation::default()),
        Submission::Queued
    );
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(
        blocked.observe(&execution, &Correlation::default()),
        Submission::Queued
    );
    let disabled = Telemetry::default();
    let mut sampled = Telemetry::with_sink(
        QueueConfig {
            capacity: 1,
            sample_every: 0,
        },
        |_| panic!("sampled sink must stay unused"),
    )
    .unwrap();
    for scenario in SCENARIOS {
        let (baseline, calls) = run(scenario);
        let (active, active_calls) = run(scenario);
        let bytes = serde_jcs::to_vec(&active.receipt).unwrap();
        assert_eq!(
            disabled.observe(&active, &Correlation::default()),
            Submission::Disabled
        );
        assert_eq!(
            sampled.observe(&active, &Correlation::default()),
            Submission::SampledOut
        );
        assert_eq!(
            blocked.observe(&active, &Correlation::default()),
            Submission::Full
        );
        assert_eq!(baseline, active, "{scenario:?}");
        assert_eq!(calls, active_calls);
        assert_eq!(bytes, serde_jcs::to_vec(&active.receipt).unwrap());
    }
    assert_eq!(blocked.statistics().dropped, 8);
    assert_eq!(sampled.statistics().sampled_out, 8);
    assert_eq!(
        blocked.shutdown(Duration::from_millis(5)),
        Shutdown::TimedOut
    );
    assert_eq!(
        blocked.observe(&execution, &Correlation::default()),
        Submission::Disconnected
    );
    release_tx.send(()).unwrap();
    assert_eq!(blocked.shutdown(Duration::from_secs(2)), Shutdown::Complete);
    assert_eq!(sampled.shutdown(Duration::from_secs(2)), Shutdown::Complete);
}

fn http_sink(
    status: u16,
    body: &'static str,
    delay: Duration,
) -> (SocketAddr, thread::JoinHandle<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut buf = [0; 4096];
        loop {
            let bytes_read = stream.read(&mut buf).unwrap();
            if bytes_read == 0 {
                break;
            }
            bytes.extend_from_slice(&buf[..bytes_read]);
            if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                let length: usize = headers
                    .lines()
                    .find_map(|header| header.strip_prefix("content-length: "))
                    .unwrap()
                    .parse()
                    .unwrap();
                if bytes.len() >= end + 4 + length {
                    break;
                }
            }
        }
        thread::sleep(delay);
        let _ = write!(
            stream,
            "HTTP/1.1 {status} fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        bytes
    });
    (address, handle)
}

#[test]
fn actual_otlp_http_success_failure_partial_rejection_and_slow_collector_are_isolated() {
    for (status, body, delay, expected) in [
        (200, "{}", Duration::ZERO, Delivery::Delivered),
        (503, "{}", Duration::ZERO, Delivery::Failed),
        (
            200,
            "{\"partial_success\":{\"rejected_spans\":\"1\"}}",
            Duration::ZERO,
            Delivery::Delivered,
        ),
        (
            200,
            "{\"partialSuccess\":{\"rejectedSpans\":null,\"errorMessage\":null}}",
            Duration::ZERO,
            Delivery::Delivered,
        ),
        (
            200,
            "{\"partialSuccess\":{\"rejectedSpans\":1}}",
            Duration::ZERO,
            Delivery::Partial,
        ),
        (
            200,
            "{\"partialSuccess\":{\"rejectedSpans\":\"0\",\"errorMessage\":\"fixture warning\"}}",
            Duration::ZERO,
            Delivery::Partial,
        ),
        (
            200,
            "{\"partialSuccess\":{\"rejectedSpans\":\"-1\"}}",
            Duration::ZERO,
            Delivery::Failed,
        ),
        (
            200,
            "{\"partialSuccess\":false}",
            Duration::ZERO,
            Delivery::Failed,
        ),
        (
            200,
            "{\"partialSuccess\":[]}",
            Duration::ZERO,
            Delivery::Failed,
        ),
        (
            200,
            "{\"partialSuccess\":null}",
            Duration::ZERO,
            Delivery::Delivered,
        ),
        (
            200,
            "{\"partialSuccess\":{\"rejectedSpans\":false}}",
            Duration::ZERO,
            Delivery::Failed,
        ),
        (
            200,
            "{\"partialSuccess\":{\"errorMessage\":[]}}",
            Duration::ZERO,
            Delivery::Failed,
        ),
        (
            200,
            "{\"partialSuccess\":{}}",
            Duration::ZERO,
            Delivery::Delivered,
        ),
        (
            200,
            "{\"partialSuccess\":{\"rejectedSpans\":\"0\"}}",
            Duration::ZERO,
            Delivery::Delivered,
        ),
        (
            200,
            "{\"partialSuccess\":{\"rejectedSpans\":\"1\"}}",
            Duration::ZERO,
            Delivery::Partial,
        ),
        (200, "{}", Duration::from_millis(150), Delivery::Failed),
        (200, "not-json", Duration::ZERO, Delivery::Failed),
        (302, "{}", Duration::ZERO, Delivery::Failed),
    ] {
        for scenario in SCENARIOS {
            let (address, server) = http_sink(status, body, delay);
            let mut telemetry =
                Telemetry::otlp(address, Duration::from_millis(60), QueueConfig::default())
                    .unwrap();
            let (execution, calls) = run(scenario);
            let (baseline, baseline_calls) = run(scenario);
            let before = serde_jcs::to_vec(&execution.receipt).unwrap();
            assert_eq!(
                telemetry.observe(&execution, &Correlation::default()),
                Submission::Queued
            );
            assert_eq!(
                telemetry.shutdown(Duration::from_secs(2)),
                Shutdown::Complete
            );
            let stats = telemetry.statistics();
            assert_eq!(
                (stats.delivered, stats.partial, stats.failed),
                match expected {
                    Delivery::Delivered => (1, 0, 0),
                    Delivery::Partial => (0, 1, 0),
                    Delivery::Failed => (0, 0, 1),
                }
            );
            let received = server.join().unwrap();
            assert!(received.starts_with(b"POST /v1/traces HTTP/1.1\r\n"));
            let end = received
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .unwrap();
            let payload: Value = serde_json::from_slice(&received[end + 4..]).unwrap();
            assert_eq!(
                payload["resourceSpans"][0]["scopeSpans"][0]["spans"][0]["name"],
                "gaap.agent_run"
            );
            assert_eq!(before, serde_jcs::to_vec(&execution.receipt).unwrap());
            assert_eq!(execution, baseline);
            assert_eq!(calls, baseline_calls);
        }
    }
}

#[test]
fn absent_collector_panicking_sink_restart_and_drop_are_bounded() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let (execution, _) = run(Scenario::Unknown);
    let before = serde_jcs::to_vec(&execution.receipt).unwrap();
    let mut absent =
        Telemetry::otlp(address, Duration::from_millis(30), QueueConfig::default()).unwrap();
    absent.observe(&execution, &Correlation::default());
    assert_eq!(absent.shutdown(Duration::from_secs(2)), Shutdown::Complete);
    assert_eq!(absent.statistics().failed, 1);
    let mut panicking =
        Telemetry::with_sink(QueueConfig::default(), |_| panic!("fixture panic")).unwrap();
    panicking.observe(&execution, &Correlation::default());
    assert_eq!(
        panicking.shutdown(Duration::from_secs(2)),
        Shutdown::Complete
    );
    assert_eq!(panicking.statistics().failed, 1);
    let saved = Arc::new(Mutex::new(Vec::new()));
    for _ in 0..2 {
        let saved = saved.clone();
        let mut restarted = Telemetry::with_sink(QueueConfig::default(), move |bytes| {
            saved
                .lock()
                .unwrap()
                .push(serde_json::from_slice::<Value>(bytes).unwrap());
            Delivery::Delivered
        })
        .unwrap();
        restarted.observe(&execution, &Correlation::default());
        assert_eq!(
            restarted.shutdown(Duration::from_secs(2)),
            Shutdown::Complete
        );
    }
    let saved = saved.lock().unwrap();
    assert_eq!(
        saved[0]["resourceSpans"][0]["scopeSpans"][0]["spans"][0]["traceId"],
        saved[1]["resourceSpans"][0]["scopeSpans"][0]["spans"][0]["traceId"]
    );
    assert_eq!(before, serde_jcs::to_vec(&execution.receipt).unwrap());
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let telemetry = Telemetry::with_sink(QueueConfig::default(), move |_| {
        entered_tx.send(()).unwrap();
        release_rx.recv().unwrap();
        done_tx.send(()).unwrap();
        Delivery::Delivered
    })
    .unwrap();
    telemetry.observe(&execution, &Correlation::default());
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let start = Instant::now();
    drop(telemetry);
    assert!(start.elapsed() < Duration::from_millis(100));
    release_tx.send(()).unwrap();
    done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
}

#[test]
fn malformed_or_oversized_inputs_and_unsafe_configuration_never_start_export() {
    let (mut execution, _) = run(Scenario::Completed);
    execution.receipt.receipt_digest = "/secret/not-a-digest".into();
    assert_eq!(
        project(&execution, &Correlation::default(), 0),
        Err(ProjectionError::InvalidDigest)
    );
    let (mut execution, _) = run(Scenario::Completed);
    execution
        .receipt
        .body
        .events
        .resize(MAX_EVENTS + 1, execution.receipt.body.events[0].clone());
    assert_eq!(
        project(&execution, &Correlation::default(), 0),
        Err(ProjectionError::TooLarge)
    );
    assert!(matches!(
        Telemetry::with_sink(
            QueueConfig {
                capacity: 0,
                sample_every: 1
            },
            |_| Delivery::Delivered
        ),
        Err(ConfigError::Capacity)
    ));
    assert!(matches!(
        Telemetry::otlp(
            "192.0.2.1:4318".parse().unwrap(),
            Duration::from_secs(1),
            QueueConfig::default()
        ),
        Err(ConfigError::Endpoint)
    ));
    assert!(matches!(
        Telemetry::otlp(
            "127.0.0.1:4318".parse().unwrap(),
            Duration::ZERO,
            QueueConfig::default()
        ),
        Err(ConfigError::Timeout)
    ));
}

#[test]
fn concurrent_producers_drop_at_the_bound_while_engine_execution_continues() {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let telemetry = Arc::new(
        Telemetry::with_sink(
            QueueConfig {
                capacity: 2,
                sample_every: 1,
            },
            move |_| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Delivery::Delivered
            },
        )
        .unwrap(),
    );
    let (execution, _) = run(Scenario::Completed);
    let execution = Arc::new(execution);
    assert_eq!(
        telemetry.observe(&execution, &Correlation::default()),
        Submission::Queued
    );
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let producers: Vec<_> = (0..8)
        .map(|_| {
            let telemetry = telemetry.clone();
            let execution = execution.clone();
            thread::spawn(move || {
                (0..4)
                    .map(|_| telemetry.observe(&execution, &Correlation::default()))
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let results: Vec<_> = producers
        .into_iter()
        .flat_map(|producer| producer.join().unwrap())
        .collect();
    assert_eq!(
        results
            .iter()
            .filter(|submission| **submission == Submission::Queued)
            .count(),
        2
    );
    assert_eq!(
        results
            .iter()
            .filter(|submission| **submission == Submission::Full)
            .count(),
        30
    );
    let (baseline, baseline_calls) = run(Scenario::Ask);
    let (active, active_calls) = run(Scenario::Ask);
    assert_eq!(active, baseline);
    assert_eq!(active_calls, baseline_calls);
    assert!(active_calls.is_empty());
    let mut telemetry = Arc::try_unwrap(telemetry).ok().unwrap();
    assert_eq!(telemetry.statistics().queued, 3);
    assert_eq!(telemetry.statistics().dropped, 30);
    assert_eq!(telemetry.shutdown(Duration::ZERO), Shutdown::TimedOut);
    release_tx.send(()).unwrap();
    assert_eq!(
        telemetry.shutdown(Duration::from_secs(2)),
        Shutdown::Complete
    );
}

#[test]
fn nonzero_fractional_sampling_is_stable_for_the_same_receipt() {
    use sha2::{Digest, Sha256};
    let mut telemetry = Telemetry::with_sink(
        QueueConfig {
            capacity: 256,
            sample_every: 3,
        },
        |_| Delivery::Delivered,
    )
    .unwrap();
    let mut selected = 0;
    for scenario in SCENARIOS {
        let (execution, _) = run(scenario);
        let hash = Sha256::digest(execution.receipt.receipt_digest.as_bytes());
        let sample = u32::from_be_bytes(hash[..4].try_into().unwrap());
        let expected = if sample % 3 == 0 {
            selected += 1;
            Submission::Queued
        } else {
            Submission::SampledOut
        };
        for _ in 0..2 {
            assert_eq!(
                telemetry.observe(&execution, &Correlation::default()),
                expected
            );
        }
    }
    assert!(selected > 0 && selected < SCENARIOS.len());
    assert_eq!(
        telemetry.shutdown(Duration::from_secs(2)),
        Shutdown::Complete
    );
    assert_eq!(telemetry.statistics().queued, selected as u64 * 2);
    assert_eq!(
        telemetry.statistics().sampled_out,
        (SCENARIOS.len() - selected) as u64 * 2
    );
}

#[test]
#[ignore = "prints sanitized recorded fixtures for explicit offline protocol/viewer checks"]
fn print_sanitized_recorded_fixtures() {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    for scenario in SCENARIOS {
        let (execution, _) = run(scenario);
        let bytes = project(&execution, &Correlation::default(), timestamp).unwrap();
        println!(
            "OTLP_FIXTURE\t{scenario:?}\t{}",
            String::from_utf8(bytes).unwrap()
        );
    }
}

#[test]
fn malformed_receipt_digest_is_rejected_before_sampling() {
    let (mut execution, _) = run(Scenario::Completed);
    execution.receipt.receipt_digest = "x".repeat(262_144);
    let mut telemetry = Telemetry::with_sink(
        QueueConfig {
            capacity: 1,
            sample_every: 0,
        },
        |_| panic!("invalid input must never reach the sink"),
    )
    .unwrap();
    assert_eq!(
        telemetry.observe(&execution, &Correlation::default()),
        Submission::Invalid(ProjectionError::InvalidDigest)
    );
    assert_eq!(telemetry.statistics().dropped, 1);
    assert_eq!(telemetry.statistics().sampled_out, 0);
    assert_eq!(
        telemetry.shutdown(Duration::from_secs(2)),
        Shutdown::Complete
    );
}

#[test]
fn full_export_budget_rejects_without_allocating_a_projection() {
    let (_, calibration) = allocated_by(|| {
        std::hint::black_box(vec![0_u8; 1024]);
        Submission::Disabled
    });
    assert!(
        calibration >= 1024,
        "the allocation probe must observe its positive control"
    );
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let mut telemetry = Telemetry::with_sink(
        QueueConfig {
            capacity: 1,
            sample_every: 1,
        },
        move |_| {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            Delivery::Delivered
        },
    )
    .unwrap();
    let (execution, _) = run(Scenario::Completed);
    let correlation = Correlation::default();
    assert_eq!(
        telemetry.observe(&execution, &correlation),
        Submission::Queued
    );
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(
        telemetry.observe(&execution, &correlation),
        Submission::Queued
    );
    let (submission, allocated) = allocated_by(|| telemetry.observe(&execution, &correlation));
    assert_eq!(telemetry.shutdown(Duration::ZERO), Shutdown::TimedOut);
    release_tx.send(()).unwrap();
    assert_eq!(
        telemetry.shutdown(Duration::from_secs(2)),
        Shutdown::Complete
    );
    assert_eq!(submission, Submission::Full);
    assert_eq!(
        allocated, 0,
        "a full exporter must reject before building JSON"
    );
}

#[test]
fn invalid_projection_releases_capacity_for_later_valid_runs() {
    let (execution, _) = run(Scenario::Completed);
    let mut invalid = execution.clone();
    invalid.receipt.body.request_digest = "invalid".into();
    let mut telemetry = Telemetry::with_sink(
        QueueConfig {
            capacity: 1,
            sample_every: 1,
        },
        |_| Delivery::Delivered,
    )
    .unwrap();
    for _ in 0..10 {
        assert_eq!(
            telemetry.observe(&invalid, &Correlation::default()),
            Submission::Invalid(ProjectionError::InvalidDigest)
        );
    }
    assert_eq!(
        telemetry.observe(&execution, &Correlation::default()),
        Submission::Queued
    );
    assert_eq!(
        telemetry.shutdown(Duration::from_secs(2)),
        Shutdown::Complete
    );
    assert_eq!(telemetry.statistics().dropped, 10);
    assert_eq!(telemetry.statistics().delivered, 1);
}
