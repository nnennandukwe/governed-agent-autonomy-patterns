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

mod sandbox;
use sandbox::{ReferenceSandbox, SandboxHost};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn port_error(error: impl std::fmt::Display) -> RuntimePortError {
    RuntimePortError::new(error.to_string())
}
fn raw_hash(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
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
#[serde(deny_unknown_fields)]
struct Config {
    agent: AgentConfig,
    proposal_server: AgentConfig,
    python: PathBuf,
    server: PathBuf,
    socket: PathBuf,
    token: String,
    output: PathBuf,
    case: String,
    #[serde(default)]
    agent_run_request: Option<AgentRunRequest>,
    #[serde(default)]
    sandbox: Option<ReferenceSandbox>,
}

type EvidenceArchive = Arc<Mutex<BTreeMap<String, Vec<u8>>>>;
fn snapshot(
    root: &Path,
    archive: &EvidenceArchive,
) -> std::result::Result<Subject, RuntimePortError> {
    fn collect(
        root: &Path,
        directory: &Path,
        files: &mut BTreeMap<String, Value>,
        blobs: &mut BTreeMap<String, Vec<u8>>,
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
                collect(root, &path, files, blobs, entries)?;
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
                let digest = format!("sha256:{:x}", Sha256::digest(&bytes));
                blobs.insert(digest.clone(), bytes);
                files.insert(key, json!({"kind":"file","mode":mode,"digest":digest}));
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
    let mut blobs = BTreeMap::new();
    collect(root, root, &mut files, &mut blobs, &mut 0)?;
    let bytes = serde_jcs::to_vec(&files).map_err(port_error)?;
    let digest = format!("sha256:{:x}", Sha256::digest(&bytes));
    blobs.insert(digest.clone(), bytes);
    archive.lock().map_err(port_error)?.extend(blobs);
    Ok(Subject {
        kind: SubjectKind::Repository,
        locator: root.display().to_string(),
        digest,
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

fn reply_no_dispatch(stream: &mut UnixStream, effect_id: &str) -> Value {
    let response = json!({"content":[{"type":"text","text":"The host stopped this proposal before dispatch; no authority was granted."}],"isError":true,"structuredContent":{"status":"not_dispatched","dispatched":false}});
    let written = writeln!(stream, "{response}").is_ok();
    json!({"effect_id":effect_id,"response":response,"written":written})
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
    protected_effect_requests: Vec<ProtectedEffectRequest>,
    raw_proposals: Vec<Value>,
}
struct AgentPort {
    shared: Arc<Mutex<Ports>>,
    listener: UnixListener,
    token: String,
    support: ContractSupport,
    deadline: Instant,
    effects: u64,
    cancel_before_dispatch: bool,
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
            let response = reply_no_dispatch(&mut pending.stream, &pending.request.effect_id);
            ports.ingress_responses.push(response);
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
                    {
                        let mut ports = self.shared.lock().expect("ports");
                        ports.protected_effect_requests.push(effect.clone());
                        ports.raw_proposals.push(value["arguments"].clone());
                    }
                    self.shared.lock().expect("ports").pending = Some(Pending {
                        stream,
                        request: effect.clone(),
                    });
                    if self.cancel_before_dispatch {
                        self.shared
                            .lock()
                            .expect("ports")
                            .conversation
                            .cancellation_handle()
                            .cancel();
                    }
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
fn sandbox_execution_evidence(
    result: &mut std::result::Result<ExecutorObservation, RuntimePortError>,
    effect_digest: &str,
    archive: &EvidenceArchive,
    transport: impl FnOnce() -> Result<Vec<Vec<u8>>>,
    observe: impl FnOnce() -> Result<gaap::sandbox::SandboxObservation>,
) -> std::result::Result<Vec<EffectEvidenceReference>, RuntimePortError> {
    let not_dispatched = match result {
        Ok(effect) => matches!(
            effect.execution_status,
            EffectExecutionStatus::Denied | EffectExecutionStatus::AwaitingAuthority
        ),
        Err(error) => error.was_not_dispatched(),
    };
    if not_dispatched {
        return Ok(vec![]);
    }
    let mut evidence = Vec::new();
    let mut retain = |bytes: Vec<u8>, evidence_type| -> std::result::Result<(), RuntimePortError> {
        let digest = raw_hash(&bytes);
        archive
            .lock()
            .map_err(port_error)?
            .insert(digest.clone(), bytes);
        let reference = EffectEvidenceReference {
            evidence_type,
            digest,
            locator: None,
        };
        if !evidence.contains(&reference) {
            evidence.push(reference);
        }
        Ok(())
    };
    if let Ok(blobs) = transport() {
        for bytes in blobs {
            retain(bytes, EffectEvidenceType::Artifact)?;
        }
    }
    match observe() {
        Ok(observation) => {
            let bytes = serde_jcs::to_vec(&observation).map_err(port_error)?;
            retain(bytes.clone(), EffectEvidenceType::Sandbox)?;
            if !sandbox::succeeded(&observation) {
                retain(bytes, EffectEvidenceType::UnknownOutcome)?;
                if let Ok(effect) = result {
                    effect.execution_status = EffectExecutionStatus::UnknownOutcome;
                    effect.reason = Some(format!(
                        "reference.sandbox_{:?}_cleanup_{:?}",
                        observation.status, observation.cleanup
                    ));
                }
            }
        }
        Err(error) => {
            let reason = format!("reference.sandbox_evidence: {error}");
            retain(serde_jcs::to_vec(&json!({"schema_version":"gaap.sandbox-evidence-missing/0.1","effect_request_digest":effect_digest,"reason":reason})).map_err(port_error)?, EffectEvidenceType::UnknownOutcome)?;
            if let Ok(effect) = result {
                effect.execution_status = EffectExecutionStatus::UnknownOutcome;
                effect.reason = Some(reason);
            }
        }
    }
    if let Ok(effect) = result {
        effect.evidence.extend(evidence.clone());
    }
    Ok(evidence)
}

struct Executor {
    shared: Arc<Mutex<Ports>>,
    sandbox: Option<Arc<SandboxHost>>,
    archive: EvidenceArchive,
}
impl ExecutorPort for Executor {
    fn execute(
        &mut self,
        request: &ProtectedEffectRequest,
    ) -> std::result::Result<ExecutorObservation, RuntimePortError> {
        let mut ports = self.shared.lock().expect("ports");
        let Some(pending) = ports.pending.as_ref() else {
            return Err(RuntimePortError::before_dispatch(
                "reference.no_pending_proposal",
            ));
        };
        if pending.request != *request {
            return Err(RuntimePortError::before_dispatch(
                "reference.proposal_binding",
            ));
        }
        let prepared = if let Some(host) = &self.sandbox {
            Some(
                host.prepare(
                    request,
                    ports.raw_proposals.last().cloned().ok_or_else(|| {
                        RuntimePortError::before_dispatch("reference.missing_arguments")
                    })?,
                )
                .map_err(|error| RuntimePortError::before_dispatch(error.to_string()))?,
            )
        } else {
            None
        };
        ports.conversation.claim_dispatch(request)?;
        let mut pending = ports.pending.take().expect("admitted proposal");
        let mut result = ports.mcp.execute(request);
        let mut retained = if let (Some(host), Some(prepared)) = (&self.sandbox, &prepared) {
            sandbox_execution_evidence(
                &mut result,
                &hash(request),
                &self.archive,
                || host.transport(prepared),
                || host.observe(prepared),
            )?
        } else {
            vec![]
        };
        let session_evidence = ports.conversation.effect_evidence()?;
        retained.push(session_evidence.clone());
        let result = match result {
            Ok(mut observation) => {
                observation.evidence.push(session_evidence);
                Ok(observation)
            }
            Err(error) => Err(error.with_evidence(retained)),
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

fn verifier_output(success: bool, output: &[u8], errors: &[u8]) -> (Value, bool) {
    if !success || !errors.is_empty() || output.len() > 32768 {
        return (json!({"failure":"reference.verifier_failed"}), false);
    }
    match serde_json::from_slice(output) {
        Ok(value) => (value, true),
        Err(error) => (
            json!({"failure":"reference.verifier_output","reason":error.to_string()}),
            false,
        ),
    }
}

struct Verifier {
    sandbox: Option<Arc<SandboxHost>>,
    archive: EvidenceArchive,
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
        let before = snapshot(&self.root, &self.archive)?;
        let (success, output, errors, sandbox_evidence, rejection) = if let Some(host) =
            &self.sandbox
        {
            let (observation, rejection) = host
                .verify(context.current_subject.clone(), hash(context.request))
                .map_err(port_error)?;
            let bytes = serde_jcs::to_vec(&observation).map_err(port_error)?;
            let digest = raw_hash(&bytes);
            self.archive
                .lock()
                .map_err(port_error)?
                .insert(digest.clone(), bytes);
            (
                sandbox::succeeded(&observation) && rejection.is_none(),
                observation.stdout,
                observation.stderr,
                Some(EvidenceReference {
                    evidence_type: EvidenceType::Artifact,
                    digest,
                    locator: None,
                }),
                rejection,
            )
        } else {
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
            (status.success(), output, errors, None, None)
        };
        let (observed, output_valid) = verifier_output(success, &output, &errors);
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
        let (after, subject_error) = match snapshot(&self.root, &self.archive) {
            Ok(after) => (after, None),
            Err(error) => (context.current_subject.clone(), Some(error.to_string())),
        };
        let passed = output_valid
            && subject_error.is_none()
            && cases
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
        let report = json!({"passed":passed,"observed":observed,"fresh":subject_error.is_none() && before==after,"subject_digest":after.digest,"subject_error":subject_error,"sandbox_rejection":rejection,"stdout_digest":raw_hash(&output),"stderr_digest":raw_hash(&errors)});
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
                ]
                .into_iter()
                .chain(sandbox_evidence)
                .collect(),
            },
        })
    }
}

/// Caller identities may change; the bounded task, observations and authority stay host-selected.
fn select_request(
    mut template: AgentRunRequest,
    supplied: Option<AgentRunRequest>,
) -> Result<AgentRunRequest> {
    let Some(request) = supplied else {
        return Ok(template);
    };
    let support = ContractSupport::new(template.policies.clone());
    validate_request(&request, &support)?;
    template.request_id = request.request_id.clone();
    template.run_id = request.run_id.clone();
    if canonical_request_bytes(&request)? != canonical_request_bytes(&template)? {
        return Err("Controller request differs from the freshly observed bounded template".into());
    }
    Ok(request)
}

fn main() -> Result<()> {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    let (describe, path) = match arguments.as_slice() {
        [path] => (false, path),
        [flag, path] if flag == "--describe" => (true, path),
        _ => return Err("usage: gaap-governed-goose [--describe] CONFIG.json".into()),
    };
    let config: Config = serde_json::from_slice(&fs::read(path)?)?;
    let root = config.agent.working_directory.clone();
    let archive: EvidenceArchive = Arc::default();
    let initial_subject = snapshot(&root, &archive)?;
    let sandbox = config
        .sandbox
        .clone()
        .map(|settings| SandboxHost::new(settings, root.clone(), &initial_subject).map(Arc::new))
        .transpose()?;
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
    if let Some(host) = &sandbox {
        server.environment.insert(
            "GAAP_SANDBOX_CONTEXT".into(),
            host.context_path().display().to_string(),
        );
        server.artifacts.push(gaap_mcp::LaunchArtifact {
            path: host.executable().to_path_buf(),
            digest: raw_hash(&fs::read(host.executable())?),
        });
        server.artifacts.push(host.docker_artifact()?);
        server.artifacts.push(host.profile_artifact());
        archive.lock().expect("archive").insert(
            host.profile.digest.clone(),
            serde_jcs::to_vec(&host.manifest)?,
        );
        archive
            .lock()
            .expect("archive")
            .insert(hash(&host.profile), serde_jcs::to_vec(&host.profile)?);
    }
    server.artifact_digest = server.artifact_manifest_digest()?;
    // OCI includes engine/image probes, launch checks, cleanup and publication.
    // Keep the ordinary stdio profile's bounds unchanged.
    let limits = TransportLimits {
        timeout: Duration::from_secs(if sandbox.is_some() { 20 } else { 10 }),
        ..TransportLimits::default()
    };
    let mut mcp = McpExecutor::discover(server, limits)?;
    if let Some(host) = &sandbox {
        mcp.adopt_sandbox_profile(host.profile.clone())?;
    }
    if mcp.tools().len() != 1 || mcp.tools()[0].tool_name != "governed_operation" {
        return Err("Unexpected backend inventory".into());
    }
    let capability = mcp.tools()[0].capability.clone();
    let mut request: AgentRunRequest = serde_json::from_str(include_str!(
        "../../contracts/v0.1.0/agent-run-request.json"
    ))?;
    request.run_id = format!("governed-goose-{}", config.case);
    request.request_id = format!("request-{}", config.case);
    request.subject = snapshot(&root, &archive)?;
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
    // Three MCP calls (20s each), one verifier (12s), plus host/ACP work.
    request.resource_budget.max_elapsed_ms = if sandbox.is_some() { 120000 } else { 40000 };
    request.resource_budget.max_tool_calls = 3;
    if describe {
        let mut output = std::io::stdout().lock();
        output.write_all(&canonical_request_bytes(&request)?)?;
        output.write_all(b"\n")?;
        return Ok(());
    }
    let request = select_request(request, config.agent_run_request)?;
    if let Some(host) = &sandbox {
        host.ensure_fresh_run()?;
    }
    let listener = UnixListener::bind(&config.socket)?;
    fs::set_permissions(&config.socket, fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
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
    let observer_archive = archive.clone();
    let mut previous = request.subject.clone();
    mcp.bind_run(
        request.clone(),
        support.clone(),
        permission.clone(),
        move |_: &ProtectedEffectRequest| {
            let subject = snapshot(&observer_root, &observer_archive).map_err(|error| {
                gaap_mcp::McpError {
                    code: "reference.observation_failed",
                    detail: error.to_string(),
                }
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
        protected_effect_requests: Vec::new(),
        raw_proposals: Vec::new(),
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
            cancel_before_dispatch: config.case == "cancel_before_dispatch",
        },
        Executor {
            shared: shared.clone(),
            sandbox: sandbox.clone(),
            archive: archive.clone(),
        },
        Verifier {
            sandbox: sandbox.clone(),
            archive: archive.clone(),
            root: root.clone(),
            python: config.python,
            report: report.clone(),
            shared: shared.clone(),
        },
    );
    let execution = engine.run(request.clone())?;
    let calls = shared.lock().expect("ports").mcp.call_evidence().to_vec();
    let ports = shared.lock().expect("ports");
    let after = snapshot(&root, &archive)?;
    let report = report.lock().expect("report").clone();
    if !report.is_null() {
        let bytes = serde_jcs::to_vec(&report)?;
        archive
            .lock()
            .expect("archive")
            .insert(hash(&report), bytes);
    }
    for (effect, raw) in ports
        .protected_effect_requests
        .iter()
        .zip(&ports.raw_proposals)
    {
        let service = effect
            .requested_scopes
            .iter()
            .find_map(|scope| match scope {
                RequestedScope::ExternalService { service, .. } => Some(service),
                _ => None,
            })
            .ok_or("missing MCP scope")?;
        let input = json!({"server":service,"tool":"governed_operation","arguments":raw});
        let bytes = serde_jcs::to_vec(&input)?;
        if hash(&input) != effect.input_digest {
            return Err("input identity mismatch".into());
        }
        archive
            .lock()
            .expect("archive")
            .insert(effect.input_digest.clone(), bytes);
    }
    for tool in ports.mcp.tools() {
        let schema = &tool.descriptor["inputSchema"];
        if hash(schema) != tool.schema_digest {
            return Err("schema identity mismatch".into());
        }
        archive
            .lock()
            .expect("archive")
            .insert(tool.schema_digest.clone(), serde_jcs::to_vec(schema)?);
    }
    for call in &calls {
        if let (Some(digest), Some(value)) = (&call.output_digest, &call.output) {
            if hash(value) != *digest {
                return Err("output identity mismatch".into());
            }
            archive
                .lock()
                .expect("archive")
                .insert(digest.clone(), serde_jcs::to_vec(value)?);
        }
    }
    // JSON byte arrays preserve arbitrary file bytes without text transcoding.
    let output = json!({"evidence_blobs":*archive.lock().expect("archive"),"request":request,"receipt":execution.receipt,"protected_effect_requests":ports.protected_effect_requests,"raw_proposals":ports.raw_proposals,"protected_effect_results":execution.protected_effect_results,"tool_inventory":ports.mcp.tools(),"acp":evidence.snapshot(),"mcp":calls,"ingress_responses":ports.ingress_responses,"verification":report,"after":after});
    fs::write(config.output, serde_json::to_vec_pretty(&output)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn template_request() -> AgentRunRequest {
        serde_json::from_str(include_str!(
            "../../contracts/v0.1.0/agent-run-request.json"
        ))
        .unwrap()
    }

    #[test]
    fn misspelled_controller_binding_cant_fall_back_to_default_identities() {
        // Reject the key before absent required fields can mask the binding error.
        let error = serde_json::from_str::<Config>(r#"{"agent_run_requst":{}}"#)
            .err()
            .expect("unknown binding key must fail")
            .to_string();
        assert!(error.contains("unknown field `agent_run_requst`"));
    }

    #[test]
    fn controller_can_bind_only_its_request_and_run_identities() {
        let template = template_request();
        let mut supplied = template.clone();
        supplied.request_id = "threadloop_request_exact_digest".into();
        supplied.run_id = "threadloop_attempt_exact_claim_digest".into();
        let selected = select_request(template, Some(supplied.clone())).unwrap();
        assert_eq!(
            canonical_request_bytes(&selected).unwrap(),
            canonical_request_bytes(&supplied).unwrap()
        );
    }

    #[test]
    fn controller_cant_change_any_bounded_template_parameter() {
        for (path, replacement) in [
            ("/subject/locator", json!("/another/workspace")),
            (
                "/subject/digest",
                json!(format!("sha256:{}", "e".repeat(64))),
            ),
            ("/requested_capability/name", json!("another_capability")),
            ("/task/instructions", json!("Another task")),
            ("/task/constraints", json!([])),
            (
                "/policies/0/digest",
                json!(format!("sha256:{}", "e".repeat(64))),
            ),
            ("/resource_budget/max_tool_calls", json!(4)),
            ("/resource_budget/max_elapsed_ms", json!(80000)),
            ("/approval_context", json!([])),
            ("/required_verification/evidence_types", json!(["artifact"])),
        ] {
            let template = template_request();
            let mut value = serde_json::to_value(&template).unwrap();
            // Each replacement differs from the selected contract fixture.
            if value.pointer(path) == Some(&replacement) {
                continue;
            }
            *value.pointer_mut(path).unwrap() = replacement;
            let supplied = serde_json::from_value(value).unwrap();
            assert!(select_request(template, Some(supplied)).is_err(), "{path}");
        }
    }

    #[test]
    fn a_closed_peer_cant_erase_the_hosts_refusal_record() {
        let (mut host, peer) = UnixStream::pair().unwrap();
        drop(peer);
        let retained = reply_no_dispatch(&mut host, "effect-1");
        assert_eq!(retained["written"], false);
        assert_eq!(
            retained["response"]["structuredContent"]["status"],
            "not_dispatched"
        );
        assert_eq!(
            retained["response"]["structuredContent"]["dispatched"],
            false
        );
    }
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
    fn evidence_test_observation(status: EffectExecutionStatus) -> ExecutorObservation {
        ExecutorObservation {
            execution_status: status,
            observed_post_effect_subject: None,
            exit: None,
            usage: EffectUsage {
                cost_micros: 0,
                elapsed_ms: 0,
                model_tokens: 0,
                tool_calls: if status == EffectExecutionStatus::Denied {
                    0
                } else {
                    1
                },
            },
            executor: None,
            sandbox_profile: None,
            reason: None,
            evidence: vec![],
        }
    }

    #[test]
    fn a_pre_dispatch_refusal_never_requires_an_oci_observation() {
        let archive = EvidenceArchive::default();
        let mut result = Ok(evidence_test_observation(EffectExecutionStatus::Denied));
        let before = result.clone();
        let references = sandbox_execution_evidence(
            &mut result,
            "effect",
            &archive,
            || panic!("A refused call has no transport"),
            || panic!("A refused call has no OCI observation"),
        )
        .unwrap();
        assert_eq!(result, before);
        assert!(references.is_empty());
        assert!(archive.lock().unwrap().is_empty());
        let mut result = Err(RuntimePortError::before_dispatch("known refusal"));
        sandbox_execution_evidence(
            &mut result,
            "effect",
            &archive,
            || panic!("No transport was dispatched"),
            || panic!("No OCI invocation exists"),
        )
        .unwrap();
        assert!(result.unwrap_err().was_not_dispatched());
    }

    #[test]
    fn missing_observation_keeps_raw_transport_in_the_effects_evidence() {
        let archive = EvidenceArchive::default();
        let raw = br#"{"stdout":[112,97,114,116,105,97,108],"stderr":[101,114,114],"exit_code":7}"#
            .to_vec();
        let mut result = Ok(evidence_test_observation(EffectExecutionStatus::Failed));
        let references = sandbox_execution_evidence(
            &mut result,
            "effect",
            &archive,
            || Ok(vec![raw.clone()]),
            || Err("invalid observation".into()),
        )
        .unwrap();
        let effect = result.unwrap();
        assert_eq!(
            effect.execution_status,
            EffectExecutionStatus::UnknownOutcome
        );
        assert_eq!(effect.usage.tool_calls, 1);
        assert!(
            effect
                .evidence
                .iter()
                .any(|reference| reference.digest == raw_hash(&raw))
        );
        assert_eq!(archive.lock().unwrap()[&raw_hash(&raw)], raw);
        assert!(
            references
                .iter()
                .any(|reference| reference.evidence_type == EffectEvidenceType::UnknownOutcome)
        );
    }

    #[test]
    fn failed_or_malformed_verifier_output_produces_a_failed_report_input() {
        for (success, stdout, stderr) in [
            (false, b"{}".as_slice(), b"".as_slice()),
            (true, b"not JSON".as_slice(), b"".as_slice()),
            (true, b"{}".as_slice(), b"error".as_slice()),
        ] {
            let (observed, valid) = verifier_output(success, stdout, stderr);
            assert!(!valid);
            assert!(observed["failure"].is_string());
        }
        assert!(verifier_output(true, b"{}", b"").1);
    }

    #[test]
    fn failed_oci_verification_returns_linked_original_execution_evidence() {
        struct Fixture(PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let path = std::env::temp_dir().join(format!(
            "gaap-verifier-evidence-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        let fixture = Fixture(path.canonicalize().unwrap());
        let root = fixture.0.join("repository");
        fs::create_dir(&root).unwrap();
        for (name, content) in [
            ("shipping.py", "def shipping_quote(q,p): return 0"),
            ("terminal.txt", "terminal witnessed"),
            ("mcp.txt", "mcp witnessed"),
        ] {
            fs::write(root.join(name), content).unwrap();
        }
        let state = fixture.0.join("state");
        fs::create_dir(&state).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
        let archive = EvidenceArchive::default();
        let subject = snapshot(&root, &archive).unwrap();
        let echo = fs::canonicalize("/bin/echo").unwrap();
        let host = SandboxHost::new(
            ReferenceSandbox {
                executable: echo.clone(),
                docker_executable: echo.clone(),
                // Refuse locally before any Docker process: this is real setup-failure evidence.
                docker_environment: BTreeMap::new(),
                engine_version: "unadopted".into(),
                runtime_version: "unadopted".into(),
                image: gaap::sandbox::SandboxImage {
                    reference: format!("python@sha256:{}", "0".repeat(64)),
                    digest: format!("sha256:{}", "0".repeat(64)),
                },
                user: "1000:1000".into(),
                state_directory: state,
            },
            root.clone(),
            &subject,
        )
        .unwrap();
        let python = Command::new("python3")
            .args(["-c", "import sys;print(sys.executable)"])
            .output()
            .unwrap();
        let python = fs::canonicalize(String::from_utf8(python.stdout).unwrap().trim()).unwrap();
        let server = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("server.py");
        let mut mcp_config = ServerConfig::new(python.clone(), "gaap-effect", "1");
        mcp_config.arguments = vec![server.display().to_string(), "effect".into()];
        mcp_config.working_directory = root.clone();
        mcp_config.artifacts = [&python, &server]
            .into_iter()
            .map(|path| gaap_mcp::LaunchArtifact {
                path: path.clone(),
                digest: raw_hash(&fs::read(path).unwrap()),
            })
            .collect();
        mcp_config.artifact_digest = mcp_config.artifact_manifest_digest().unwrap();
        let mcp = McpExecutor::discover(mcp_config, TransportLimits::default()).unwrap();
        let ingress = AgentConfig {
            executable: python.clone(),
            arguments: vec![server.display().to_string(), "effect".into()],
            environment: BTreeMap::new(),
            working_directory: root.clone(),
            expected_name: "gaap-proposals".into(),
            expected_version: "1".into(),
            artifacts: [&python, &server]
                .into_iter()
                .map(|path| gaap_acp::LaunchArtifact {
                    path: path.clone(),
                    digest: raw_hash(&fs::read(path).unwrap()),
                })
                .collect(),
        };
        let agent = AgentConfig {
            executable: echo.clone(),
            arguments: vec![],
            environment: BTreeMap::new(),
            working_directory: root.clone(),
            expected_name: "goose".into(),
            expected_version: "1.54.0".into(),
            artifacts: vec![gaap_acp::LaunchArtifact {
                path: echo.clone(),
                digest: raw_hash(&fs::read(&echo).unwrap()),
            }],
        };
        // No Goose conversation starts; Ports supplies only the verifier's usage meter.
        let conversation = GooseConversation::new(
            agent,
            GooseProfile {
                proposal_server: ingress,
                server_name: "gaap-proposals".into(),
            },
            Limits::default(),
        )
        .unwrap();
        let shared = Arc::new(Mutex::new(Ports {
            conversation,
            mcp,
            pending: None,
            last_meter: Instant::now(),
            ingress_responses: vec![],
            protected_effect_requests: vec![],
            raw_proposals: vec![],
        }));
        let mut verifier = Verifier {
            sandbox: Some(Arc::new(host)),
            archive: archive.clone(),
            root,
            python,
            report: Arc::default(),
            shared,
        };
        let request = template_request();
        let usage = usage(0);
        let report = verifier
            .verify(VerificationContext {
                request: &request,
                current_subject: &subject,
                usage: &usage,
            })
            .unwrap()
            .value;
        assert_eq!(report.verdict, VerificationVerdict::Fail);
        let blobs = archive.lock().unwrap();
        assert!(report.evidence.iter().any(|reference| {
            blobs
                .get(&reference.digest)
                .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
                .is_some_and(|observation| {
                    observation["schema_version"] == "gaap.sandbox-observation/0.1"
                        && observation["status"] == "setup_failed"
                        && observation["runtime"].is_null()
                })
        }));
    }
}
