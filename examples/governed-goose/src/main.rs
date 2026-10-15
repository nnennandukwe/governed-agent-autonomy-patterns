//! One bounded run owns the real Goose conversation and all mediated proposals.
#![forbid(unsafe_code)]
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gaap::contracts::*;
use gaap::runtime::*;
use gaap_acp::goose::{ConversationEvent, GooseConversation, GooseProfile};
use gaap_acp::{AgentConfig, Limits};
use gaap_mcp::{McpExecutor, ServerConfig, SubjectObservation, TransportLimits};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn port_error(error: impl std::fmt::Display) -> RuntimePortError {
    RuntimePortError::new(error.to_string())
}
fn hash(value: &impl serde::Serialize) -> String {
    format!(
        "sha256:{:x}",
        Sha256::digest(serde_jcs::to_vec(value).expect("canonical fixture value"))
    )
}
fn usage(elapsed: u64) -> ResourceUsage {
    ResourceUsage {
        cost_micros: 0,
        elapsed_ms: elapsed,
        model_tokens: 0,
        tool_calls: 0,
    }
}

#[derive(Deserialize)]
struct Config {
    agent: AgentConfig,
    proposal_server: AgentConfig,
    python: PathBuf,
    server: PathBuf,
    socket: PathBuf,
    token: String,
    output: PathBuf,
    case: String,
}

fn snapshot(root: &Path) -> std::result::Result<Subject, RuntimePortError> {
    fn collect(
        root: &Path,
        directory: &Path,
        files: &mut BTreeMap<String, Value>,
        entries: &mut usize,
    ) -> std::result::Result<(), RuntimePortError> {
        for entry in fs::read_dir(directory).map_err(port_error)? {
            let path = entry.map_err(port_error)?.path();
            if directory == root && path.file_name().is_some_and(|name| name == ".git") {
                continue;
            }
            *entries += 1;
            let relative = path.strip_prefix(root).map_err(port_error)?;
            if *entries > 64 || relative.components().count() > 8 {
                return Err(port_error("reference.subject_limit"));
            }
            let metadata = fs::symlink_metadata(&path).map_err(port_error)?;
            if metadata.file_type().is_symlink() || path.canonicalize().map_err(port_error)? != path
            {
                return Err(port_error("reference.subject_boundary"));
            }
            let key = relative
                .to_str()
                .ok_or_else(|| port_error("reference.subject_name"))?
                .to_owned();
            let mode = metadata.permissions().mode() & 0o7777;
            if metadata.is_dir() {
                files.insert(key, json!({"kind":"directory","mode":mode}));
                collect(root, &path, files, entries)?;
            } else if metadata.is_file() && metadata.len() <= 8192 {
                let mut bytes = Vec::new();
                fs::File::open(&path)
                    .map_err(port_error)?
                    .take(8193)
                    .read_to_end(&mut bytes)
                    .map_err(port_error)?;
                if bytes.len() > 8192 {
                    return Err(port_error("reference.subject_limit"));
                }
                files.insert(key,json!({"kind":"file","mode":mode,"digest":format!("sha256:{:x}",Sha256::digest(bytes))}));
            } else {
                return Err(port_error("reference.subject_boundary"));
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::from([(
        ".".into(),
        json!({"kind":"directory","mode":fs::metadata(root).map_err(port_error)?.permissions().mode() & 0o7777}),
    )]);
    collect(root, root, &mut files, &mut 0)?;
    Ok(Subject {
        kind: SubjectKind::Repository,
        locator: root.display().to_string(),
        digest: hash(&files),
    })
}

fn read_proposal(
    stream: &mut UnixStream,
    deadline: Instant,
) -> std::result::Result<Vec<u8>, RuntimePortError> {
    let mut line = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(port_error("reference.proposal_timeout"));
        }
        stream
            .set_read_timeout(Some(remaining))
            .map_err(port_error)?;
        let mut buffer = [0u8; 1024];
        let count = stream.read(&mut buffer).map_err(port_error)?;
        if count == 0 {
            return Err(port_error("reference.proposal_eof"));
        }
        line.extend_from_slice(&buffer[..count]);
        if line.len() > 16384 {
            return Err(port_error("reference.proposal_limit"));
        }
        if let Some(newline) = line.iter().position(|byte| *byte == b'\n') {
            if newline + 1 != line.len() {
                return Err(port_error("reference.multiple_proposals"));
            }
            return Ok(line);
        }
    }
}

struct Pending {
    stream: UnixStream,
    request: ProtectedEffectRequest,
}
struct Ports {
    conversation: GooseConversation,
    mcp: McpExecutor,
    pending: Option<Pending>,
    last_meter: Instant,
    ingress_responses: Vec<Value>,
}
struct AgentPort {
    shared: Arc<Mutex<Ports>>,
    listener: UnixListener,
    token: String,
    support: ContractSupport,
    deadline: Instant,
    effects: u64,
    refusal_status: String,
}
impl AgentPort {
    fn meter(&self) -> ResourceUsage {
        let mut ports = self.shared.lock().expect("ports");
        let now = Instant::now();
        let elapsed = now.duration_since(ports.last_meter).as_millis() as u64;
        ports.last_meter = now;
        usage(elapsed)
    }
}
impl AgentAdapter for AgentPort {
    fn cancellation(&self) -> Option<Cancellation> {
        self.shared
            .lock()
            .expect("ports")
            .conversation
            .cancellation()
    }
    fn claim_completion(&mut self) -> std::result::Result<Option<Cancellation>, RuntimePortError> {
        self.shared
            .lock()
            .expect("ports")
            .conversation
            .claim_completion()
    }
    fn close(&mut self) {
        let mut ports = self.shared.lock().expect("ports");
        if let Some(mut pending) = ports.pending.take() {
            let response = json!({"content":[{"type":"text","text":"The host stopped this proposal before dispatch; no authority was granted."}],"isError":true,"structuredContent":{"status":self.refusal_status,"dispatched":false}});
            let written = writeln!(pending.stream, "{response}").is_ok();
            ports.ingress_responses.push(json!({"effect_id":pending.request.effect_id,"response":response,"written":written}));
        }
        ports.conversation.close();
    }
    fn plan(
        &mut self,
        request: &AgentRunRequest,
    ) -> std::result::Result<PortObservation<PlanProposal>, RuntimePortError> {
        self.deadline =
            Instant::now() + Duration::from_millis(request.resource_budget.max_elapsed_ms);
        let digest = self
            .shared
            .lock()
            .expect("ports")
            .conversation
            .plan(request, &self.support)?;
        Ok(PortObservation {
            value: PlanProposal {
                plan_digest: digest,
            },
            usage: self.meter(),
        })
    }
    fn next_effect(
        &mut self,
        context: AgentRunContext<'_>,
    ) -> std::result::Result<PortObservation<AgentStep>, RuntimePortError> {
        self.shared
            .lock()
            .expect("ports")
            .conversation
            .start_prompt()?;
        loop {
            if Instant::now() >= self.deadline {
                return Err(port_error("reference.run_timeout"));
            }
            let event = self
                .shared
                .lock()
                .expect("ports")
                .conversation
                .poll(Duration::from_millis(2))?;
            if let Some(ConversationEvent::Finished) = event {
                if self.effects != 3 {
                    return Err(port_error("reference.incomplete_conversation"));
                }
                return Ok(PortObservation {
                    value: AgentStep::Finish,
                    usage: self.meter(),
                });
            }
            match self.listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).map_err(port_error)?;
                    stream
                        .set_write_timeout(Some(Duration::from_secs(2)))
                        .map_err(port_error)?;
                    let line = read_proposal(
                        &mut stream,
                        self.deadline.min(Instant::now() + Duration::from_secs(2)),
                    )?;
                    let value: Value = serde_json::from_slice(&line).map_err(port_error)?;
                    if value["token"] != self.token {
                        return Err(port_error("reference.ingress_auth"));
                    }
                    let sequence = context.effects_seen + 1;
                    let effect = self
                        .shared
                        .lock()
                        .expect("ports")
                        .mcp
                        .prepare_effect(
                            "governed_operation",
                            value["arguments"].clone(),
                            format!("goose-{sequence}"),
                            sequence,
                            context.current_subject.clone(),
                            EffectClass::Mutation,
                        )
                        .map_err(port_error)?;
                    self.shared.lock().expect("ports").pending = Some(Pending {
                        stream,
                        request: effect.clone(),
                    });
                    self.effects += 1;
                    return Ok(PortObservation {
                        value: AgentStep::ProtectedEffect(Box::new(ProtectedEffectProposal {
                            request: effect,
                        })),
                        usage: self.meter(),
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => (),
                Err(error) => return Err(port_error(error)),
            }
        }
    }
}
struct Executor {
    shared: Arc<Mutex<Ports>>,
}
impl ExecutorPort for Executor {
    fn execute(
        &mut self,
        request: &ProtectedEffectRequest,
    ) -> std::result::Result<ExecutorObservation, RuntimePortError> {
        let mut ports = self.shared.lock().expect("ports");
        let Some(mut pending) = ports.pending.take() else {
            return Err(RuntimePortError::before_dispatch(
                "reference.no_pending_proposal",
            ));
        };
        if pending.request != *request {
            return Err(RuntimePortError::before_dispatch(
                "reference.proposal_binding",
            ));
        }
        ports.conversation.claim_dispatch(request)?;
        let result = ports.mcp.execute(request);
        let session_evidence = ports.conversation.effect_evidence()?;
        let result = match result {
            Ok(mut observation) => {
                observation.evidence.push(session_evidence);
                Ok(observation)
            }
            Err(error) => Err(error.with_evidence(vec![session_evidence])),
        };
        let succeeded = result.as_ref().is_ok_and(|observation| {
            observation.execution_status == EffectExecutionStatus::Executed
        });
        let response = json!({"content":[{"type":"text","text":if succeeded {"Host executed the admitted effect."} else {"Host didn't confirm the effect."}}],"isError":!succeeded});
        // Delivery failure can't erase an already attempted effect.
        let written = writeln!(pending.stream, "{response}").is_ok();
        ports
            .ingress_responses
            .push(json!({"effect_id":request.effect_id,"response":response,"written":written}));
        ports.last_meter = Instant::now();
        result
    }
}

struct Verifier {
    root: PathBuf,
    python: PathBuf,
    report: Arc<Mutex<Value>>,
    shared: Arc<Mutex<Ports>>,
}
impl VerifierPort for Verifier {
    fn verify(
        &mut self,
        context: VerificationContext<'_>,
    ) -> std::result::Result<PortObservation<VerificationReport>, RuntimePortError> {
        let started = Instant::now();
        let before = snapshot(&self.root)?;
        let source = fs::read_to_string(self.root.join("shipping.py")).map_err(port_error)?;
        let mut child = Command::new(&self.python)
            .args(["-I", "-S", "-B", "-c", include_str!("../verify.py")])
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(port_error)?;
        let payload = json!({"source":source,"inputs":[[1,20],[4,20],[5,20],[5,25],[1,100],[6,19.99],[0,20],[-1,20]]});
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(payload.to_string().as_bytes())
            .map_err(port_error)?;
        let stdout = child.stdout.take().expect("stdout");
        let stderr = child.stderr.take().expect("stderr");
        let out = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout.take(32769).read_to_end(&mut bytes).map(|_| bytes)
        });
        let err = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stderr.take(8193).read_to_end(&mut bytes).map(|_| bytes)
        });
        let status = loop {
            if let Some(status) = child.try_wait().map_err(port_error)? {
                break status;
            }
            if started.elapsed() > Duration::from_secs(5) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(port_error("reference.verifier_timeout"));
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        let output = out
            .join()
            .map_err(|_| port_error("reference.verifier_reader"))?
            .map_err(port_error)?;
        let errors = err
            .join()
            .map_err(|_| port_error("reference.verifier_reader"))?
            .map_err(port_error)?;
        if !status.success() || !errors.is_empty() || output.len() > 32768 {
            return Err(port_error("reference.verifier_failed"));
        }
        let observed: Value = serde_json::from_slice(&output).map_err(port_error)?;
        let expected = [
            Some(27.0),
            Some(87.0),
            Some(97.0),
            Some(112.5),
            Some(100.0),
            Some(107.95),
            None,
            None,
        ];
        let cases = expected
            .iter()
            .enumerate()
            .all(|(index, expected)| match expected {
                Some(value) => observed["observations"][index]["value"]
                    .as_f64()
                    .is_some_and(|actual| (actual - value).abs() < 0.00001),
                None => observed["observations"][index]["error"] == "ValueError",
            });
        let after = snapshot(&self.root)?;
        let passed = cases
            && before == after
            && before == *context.current_subject
            && fs::read_to_string(self.root.join("terminal.txt"))
                .ok()
                .as_deref()
                == Some("terminal witnessed")
            && fs::read_to_string(self.root.join("mcp.txt"))
                .ok()
                .as_deref()
                == Some("mcp witnessed");
        let report = json!({"passed":passed,"observed":observed,"fresh":before==after,"subject_digest":after.digest});
        *self.report.lock().expect("report") = report.clone();
        self.shared.lock().expect("ports").last_meter = Instant::now();
        Ok(PortObservation {
            usage: usage(started.elapsed().as_millis() as u64),
            value: VerificationReport {
                subject_digest: after.digest.clone(),
                verifier_id: "independent-shipping-oracle".into(),
                verdict: if passed {
                    VerificationVerdict::Pass
                } else {
                    VerificationVerdict::Fail
                },
                evidence: vec![
                    EvidenceReference {
                        evidence_type: EvidenceType::CommandOutput,
                        digest: hash(&report),
                        locator: None,
                    },
                    EvidenceReference {
                        evidence_type: EvidenceType::Artifact,
                        digest: after.digest,
                        locator: None,
                    },
                ],
            },
        })
    }
}

fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: gaap-governed-goose CONFIG.json")?;
    let config: Config = serde_json::from_slice(&fs::read(path)?)?;
    let root = config.agent.working_directory.clone();
    let listener = UnixListener::bind(&config.socket)?;
    fs::set_permissions(&config.socket, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    let mut server = ServerConfig::new(config.python.clone(), "gaap-effect", "1");
    server.arguments = vec![config.server.display().to_string(), "effect".into()];
    server.working_directory = root.clone();
    server.artifacts = config
        .proposal_server
        .artifacts
        .iter()
        .map(|artifact| gaap_mcp::LaunchArtifact {
            path: artifact.path.clone(),
            digest: artifact.digest.clone(),
        })
        .collect();
    server.artifact_digest = server.artifact_manifest_digest()?;
    let mut mcp = McpExecutor::discover(server, TransportLimits::default())?;
    if mcp.tools().len() != 1 || mcp.tools()[0].tool_name != "governed_operation" {
        return Err("Unexpected backend inventory".into());
    }
    let capability = mcp.tools()[0].capability.clone();
    let mut request: AgentRunRequest = serde_json::from_str(include_str!(
        "../../contracts/v0.1.0/agent-run-request.json"
    ))?;
    request.run_id = format!("governed-goose-{}", config.case);
    request.request_id = format!("request-{}", config.case);
    request.subject = snapshot(&root)?;
    request.requested_capability = capability.clone();
    request.approval_context = vec![ApprovalReference {
        approval_id: "fixture-subject-approval".into(),
        actor_id: "reference-fixture-host".into(),
        scope: "subject".into(),
        subject_digest: request.subject.digest.clone(),
        evidence: EvidenceReference {
            evidence_type: EvidenceType::Approval,
            digest: hash(
                &json!({"fixture_subject":request.subject,"operations":["file_write","terminal_echo","mcp_echo"]}),
            ),
            locator: None,
        },
    }];
    request.task.instructions =
        "Apply the three bounded reference operations through governed_operation.".into();
    request.task.constraints = vec!["No ambient tools, external services or credentials.".into()];
    request.resource_budget.max_elapsed_ms = 40000;
    request.resource_budget.max_tool_calls = 3;
    let support = ContractSupport::new(request.policies.clone());
    let permission = PermissionPolicy {
        policy_decision: Some(match config.case.as_str() {
            "ask" => PolicyDecision::Ask,
            "block" => PolicyDecision::Deny,
            _ => PolicyDecision::Allow,
        }),
        risk_tags: vec![],
        wrapper_chain: vec![],
        approvals: vec![],
    };
    let observer_root = root.clone();
    let mut previous = request.subject.clone();
    mcp.bind_run(
        request.clone(),
        support.clone(),
        permission.clone(),
        move |_: &ProtectedEffectRequest| {
            let subject = snapshot(&observer_root).map_err(|error| gaap_mcp::McpError {
                code: "reference.observation_failed",
                detail: error.to_string(),
            })?;
            let changed = subject != previous;
            previous = subject.clone();
            Ok(SubjectObservation {
                subject,
                effect_confirmed: changed,
                cost_micros: Some(0),
                model_tokens: Some(0),
            })
        },
    )?;
    mcp.approve_capability(&capability)?;
    let identity = mcp.identity().clone();
    let conversation = GooseConversation::new(
        config.agent,
        GooseProfile {
            proposal_server: config.proposal_server,
            server_name: "gaap-proposals".into(),
        },
        Limits {
            operation_timeout: Duration::from_secs(20),
            ..Limits::default()
        },
    )?;
    let evidence = conversation.evidence();
    let shared = Arc::new(Mutex::new(Ports {
        conversation,
        mcp,
        pending: None,
        last_meter: Instant::now(),
        ingress_responses: Vec::new(),
    }));
    let report = Arc::new(Mutex::new(Value::Null));
    let runtime_support = RuntimeSupport::new(
        support.clone(),
        vec![capability],
        permission,
        EffectUsage {
            cost_micros: 0,
            elapsed_ms: 0,
            model_tokens: 0,
            tool_calls: 1,
        },
        vec![],
        vec!["independent-shipping-oracle".into()],
    );
    let runtime_config = RuntimeConfig {
        implementer_id: "gaap-governed-goose-reference".into(),
        executor_identity: identity,
        max_effects: 3,
        budget_thresholds: BudgetThresholds {
            warn_micros: 800000,
            approval_micros: 900000,
            hard_stop_micros: 1000000,
        },
    };
    let mut engine = AgentRunEngine::new(
        runtime_support,
        runtime_config,
        AgentPort {
            shared: shared.clone(),
            listener,
            token: config.token,
            support,
            deadline: Instant::now(),
            effects: 0,
            refusal_status: match config.case.as_str() {
                "ask" => "authority_required",
                "block" => "denied",
                _ => "not_dispatched",
            }
            .into(),
        },
        Executor {
            shared: shared.clone(),
        },
        Verifier {
            root: root.clone(),
            python: config.python,
            report: report.clone(),
            shared: shared.clone(),
        },
    );
    let execution = engine.run(request.clone())?;
    let calls = shared.lock().expect("ports").mcp.call_evidence().to_vec();
    let output = json!({"request":request,"receipt":execution.receipt,"protected_effect_results":execution.protected_effect_results,"acp":evidence.snapshot(),"mcp":calls,"ingress_responses":shared.lock().expect("ports").ingress_responses,"verification":*report.lock().expect("report"),"after":snapshot(&root)?});
    fs::write(config.output, serde_json::to_vec_pretty(&output)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_proposal_cant_extend_the_total_read_deadline() {
        let (mut reader, mut writer) = UnixStream::pair().unwrap();
        let sender = std::thread::spawn(move || {
            for _ in 0..8 {
                if writer.write_all(b"x").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(40));
            }
        });
        let start = Instant::now();
        assert!(read_proposal(&mut reader, start + Duration::from_millis(100)).is_err());
        assert!(start.elapsed() < Duration::from_millis(300));
        drop(reader);
        sender.join().unwrap();
    }
}
