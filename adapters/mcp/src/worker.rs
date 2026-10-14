use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::io::Read;
use std::process::Stdio;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use gaap::contracts::{CapabilityIdentity, EffectExecutionStatus, ProtectedEffectRequest};
use gaap::runtime::PermissionPolicy;
use gaap::{Gate, Outcome, RunCoordinator};
use rmcp::{
    ClientHandler, RoleClient, ServiceExt,
    model::{
        CallToolRequestParams, ClientCapabilities, ClientConfig, Implementation,
        PaginatedRequestParams, ProtocolVersion,
    },
    service::RunningService,
};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio_util::sync::CancellationToken;

use crate::transport::{SharedState, StdioTransport, fail, health};
use crate::{
    MCP_PROTOCOL_VERSION, McpError, ServerConfig, ToolCapability, TransportLimits, WorkerCommand,
    digest,
};

pub(crate) struct Invocation {
    pub status: EffectExecutionStatus,
    pub output: Option<Value>,
    pub reason: Option<String>,
    pub dispatched: bool,
}

struct Client;
impl ClientHandler for Client {
    fn get_info(&self) -> ClientConfig {
        ClientConfig::new(
            ClientCapabilities::default(),
            Implementation::new("gaap-mcp", env!("CARGO_PKG_VERSION")),
        )
        .with_protocol_version(ProtocolVersion::V_2025_11_25)
    }
}

struct Session {
    client: RunningService<RoleClient, Client>,
    child: Child,
    stderr: tokio::task::JoinHandle<()>,
    state: SharedState,
    cancellation: CancellationToken,
    server_digest: String,
    limits: TransportLimits,
    discovery_generation: u64,
}

pub(crate) fn validate_config(
    config: &ServerConfig,
    limits: TransportLimits,
) -> Result<(), McpError> {
    if !config.executable.is_absolute()
        || !config.executable.is_file()
        || !config.working_directory.is_absolute()
        || !config.working_directory.is_dir()
        || config.expected_name.trim().is_empty()
        || config.expected_version.trim().is_empty()
        || !config.artifact_digest.starts_with("sha256:")
        || config.artifact_digest.len() != 71
        || !config.artifact_digest[7..]
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(McpError::new(
            "mcp.invalid_config",
            "absolute executable/directory and trusted server artifact identity are required",
        ));
    }
    if config.executable.canonicalize().ok().as_ref() != Some(&config.executable) {
        return Err(McpError::new(
            "mcp.launch_profile",
            "use a canonical native executable path without a mutable alias",
        ));
    }
    let mut executable = std::fs::File::open(&config.executable)
        .map_err(|e| McpError::new("mcp.launch_profile", e.to_string()))?;
    let mut header = [0; 4];
    executable
        .read_exact(&mut header)
        .map_err(|e| McpError::new("mcp.launch_profile", e.to_string()))?;
    let native = [
        b"\x7fELF",
        b"\xfe\xed\xfa\xce",
        b"\xce\xfa\xed\xfe",
        b"\xfe\xed\xfa\xcf",
        b"\xcf\xfa\xed\xfe",
        b"\xca\xfe\xba\xbe",
        b"\xbe\xba\xfe\xca",
        b"\xca\xfe\xba\xbf",
        b"\xbf\xba\xfe\xca",
    ]
    .contains(&&header)
        || &header[..2] == b"MZ";
    if !native {
        return Err(McpError::new(
            "mcp.launch_profile",
            "direct shebang launches aren't supported; configure the approved native interpreter and script explicitly",
        ));
    }
    let program = config
        .executable
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if [
        "env",
        "sh",
        "bash",
        "zsh",
        "fish",
        "cmd",
        "powershell",
        "pwsh",
        "npx",
        "npm",
        "uvx",
        "java",
        "dotnet",
        "ruby",
        "perl",
        "lua",
        "deno",
        "bun",
        "php",
        "rscript",
    ]
    .contains(&program.as_str())
    {
        return Err(McpError::new(
            "mcp.launch_profile",
            "shells and implicit module/package launchers aren't supported by this profile",
        ));
    }
    if program.starts_with("python") || program == "node" {
        let script = config
            .arguments
            .first()
            .map(std::path::PathBuf::from)
            .filter(|path| {
                path.is_absolute()
                    && path.is_file()
                    && path.canonicalize().ok().as_ref() == Some(path)
            });
        if script
            .as_ref()
            .is_none_or(|path| !config.artifacts.iter().any(|asset| &asset.path == path))
        {
            return Err(McpError::new(
                "mcp.launch_profile",
                "Python/Node launches require an explicit canonical script argument in the approved manifest",
            ));
        }
    }
    for argument in &config.arguments {
        let candidate = std::path::Path::new(
            argument
                .split_once('=')
                .map_or(argument.as_str(), |(_, value)| value),
        );
        let candidate = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            config.working_directory.join(candidate)
        };
        if candidate.is_file() {
            let canonical = candidate
                .canonicalize()
                .map_err(|e| McpError::new("mcp.artifact_identity", e.to_string()))?;
            if !config.artifacts.iter().any(|asset| asset.path == canonical) {
                return Err(McpError::new(
                    "mcp.artifact_identity",
                    "file arguments must be included in the approved launch manifest",
                ));
            }
        }
    }
    if limits.timeout.is_zero()
        || limits.timeout > Duration::from_secs(60)
        || !(1024..=16 * 1024 * 1024).contains(&limits.max_frame_bytes)
        || !(1..=1024 * 1024).contains(&limits.max_stderr_bytes)
        || !(1..=1024).contains(&limits.max_tools)
        || !(1..=1024).contains(&limits.max_calls)
        || !(1024..=64 * 1024 * 1024).contains(&limits.max_retained_bytes)
    {
        return Err(McpError::new(
            "mcp.invalid_limits",
            "use bounded time, frame, stderr, tool and invocation limits",
        ));
    }
    if config
        .environment
        .iter()
        .any(|(name, value)| name.is_empty() || name.contains(['=', '\0']) || value.contains('\0'))
        || config.arguments.iter().any(|arg| arg.contains('\0'))
    {
        return Err(McpError::new(
            "mcp.invalid_config",
            "invalid argument or environment entry",
        ));
    }
    if config.artifacts.is_empty()
        || config.artifacts.len() > 32
        || !config
            .artifacts
            .iter()
            .any(|artifact| artifact.path == config.executable)
        || config.artifact_manifest_digest()? != config.artifact_digest
    {
        return Err(McpError::new(
            "mcp.artifact_identity",
            "approve a manifest containing the exact executable and launch dependencies",
        ));
    }
    let mut consumed = 0usize;
    let mut paths = BTreeSet::new();
    for artifact in &config.artifacts {
        if !artifact.path.is_absolute()
            || artifact.path.canonicalize().ok().as_ref() != Some(&artifact.path)
            || !paths.insert(artifact.path.clone())
        {
            return Err(McpError::new(
                "mcp.artifact_identity",
                "artifact paths must be absolute and unique",
            ));
        }
        let mut file = std::fs::File::open(&artifact.path)
            .map_err(|error| McpError::new("mcp.artifact_identity", error.to_string()))?;
        if !file
            .metadata()
            .map_err(|error| McpError::new("mcp.artifact_identity", error.to_string()))?
            .is_file()
        {
            return Err(McpError::new(
                "mcp.artifact_identity",
                "launch artifact must be a regular file",
            ));
        }
        let mut hasher = Sha256::new();
        let mut buffer = [0; 65536];
        loop {
            let size = file
                .read(&mut buffer)
                .map_err(|error| McpError::new("mcp.artifact_identity", error.to_string()))?;
            if size == 0 {
                break;
            }
            consumed = consumed.saturating_add(size);
            if consumed > 128 * 1024 * 1024 {
                return Err(McpError::new(
                    "mcp.artifact_limit",
                    "launch artifacts exceed the total byte bound",
                ));
            }
            hasher.update(&buffer[..size]);
        }
        if format!("sha256:{:x}", hasher.finalize()) != artifact.digest {
            return Err(McpError::new(
                "mcp.artifact_identity",
                "launch bytes differ from the host-approved artifact digest",
            ));
        }
    }
    Ok(())
}

pub(crate) fn validate_arguments(descriptor: &Value, arguments: &Value) -> Result<(), McpError> {
    let schema = descriptor
        .get("inputSchema")
        .ok_or_else(|| McpError::new("mcp.input_schema", "tool input schema is absent"))?;
    let validator = jsonschema::validator_for(schema)
        .map_err(|e| McpError::new("mcp.input_schema", e.to_string()))?;
    if !arguments.is_object() || !validator.is_valid(arguments) {
        return Err(McpError::new(
            "mcp.invalid_arguments",
            "arguments don't satisfy the exact discovered input schema",
        ));
    }
    Ok(())
}

pub(crate) fn run(
    config: ServerConfig,
    limits: TransportLimits,
    server_digest: String,
    cancellation: CancellationToken,
    state: SharedState,
    receiver: mpsc::Receiver<WorkerCommand>,
    init: mpsc::Sender<Result<Vec<ToolCapability>, McpError>>,
) {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = init.send(Err(McpError::new("mcp.runtime_start", error.to_string())));
            return;
        }
    };
    let mut session = match runtime.block_on(Session::start(
        config,
        limits,
        server_digest,
        cancellation,
        state,
    )) {
        Ok(mut session) => match runtime.block_on(session.discover()) {
            Ok(tools) => {
                if init.send(Ok(tools)).is_err() {
                    runtime.block_on(session.close());
                    return;
                }
                session
            }
            Err(error) => {
                let _ = init.send(Err(error));
                runtime.block_on(session.close());
                return;
            }
        },
        Err(error) => {
            let _ = init.send(Err(error));
            return;
        }
    };
    loop {
        if health(&session.state).is_err() || session.cancellation.is_cancelled() {
            break;
        }
        let command = match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(command) => command,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        match command {
            WorkerCommand::Refresh(reply) => {
                let _ = reply.send(runtime.block_on(session.discover()));
            }
            WorkerCommand::Invoke {
                effect,
                arguments,
                approved,
                policy,
                observer,
                deadline,
                reply,
            } => {
                let _ = reply.send(runtime.block_on(
                    session.invoke(&effect, arguments, &approved, &policy, &observer, deadline),
                ));
            }
            WorkerCommand::Stop => break,
        }
    }
    runtime.block_on(session.close());
}

impl Session {
    async fn start(
        config: ServerConfig,
        limits: TransportLimits,
        server_digest: String,
        cancellation: CancellationToken,
        state: SharedState,
    ) -> Result<Self, McpError> {
        // Recheck on the owning thread immediately before launch. This isn't OS isolation.
        validate_config(&config, limits)?;
        let mut command = Command::new(&config.executable);
        command
            .args(&config.arguments)
            .env_clear()
            .envs(&config.environment)
            .current_dir(&config.working_directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|e| McpError::new("mcp.spawn_failed", e.to_string()))?;
        let mut stderr = child.stderr.take().expect("piped stderr");
        let stderr_state = state.clone();
        let stderr_cancel = cancellation.clone();
        let stderr_task = tokio::spawn(async move {
            let mut count = 0usize;
            let mut buffer = [0; 4096];
            loop {
                match stderr.read(&mut buffer).await {
                    Ok(0) => break,
                    Ok(size) => {
                        count = count.saturating_add(size);
                        if count > limits.max_stderr_bytes {
                            fail(
                                &stderr_state,
                                "mcp.stderr_limit",
                                "server diagnostics exceed configured bound",
                            );
                            stderr_cancel.cancel();
                            break;
                        }
                    }
                    Err(error) => {
                        fail(&stderr_state, "mcp.stderr_read", error.to_string());
                        stderr_cancel.cancel();
                        break;
                    }
                }
            }
        });
        let transport = StdioTransport {
            reader: tokio::io::BufReader::new(child.stdout.take().expect("piped stdout")),
            writer: Arc::new(tokio::sync::Mutex::new(
                child.stdin.take().expect("piped stdin"),
            )),
            state: state.clone(),
            cancellation: cancellation.clone(),
            max_frame: limits.max_frame_bytes,
        };
        let startup = tokio::time::timeout(limits.timeout, Client.serve(transport)).await;
        let client = match startup {
            Ok(Ok(client)) => client,
            result => {
                let error = health(&state).err().unwrap_or_else(|| {
                    McpError::new(
                        "mcp.initialize_failed",
                        match result {
                            Ok(Err(e)) => e.to_string(),
                            _ => "initialization timed out".into(),
                        },
                    )
                });
                let _ = child.kill().await;
                let _ = child.wait().await;
                stderr_task.abort();
                return Err(error);
            }
        };
        let peer = client
            .peer_info()
            .ok_or_else(|| McpError::new("mcp.server_identity", "server identity is absent"))?;
        if peer.protocol_version.as_str() != MCP_PROTOCOL_VERSION
            || peer.server_info.as_ref().is_none_or(|identity| {
                identity.name != config.expected_name || identity.version != config.expected_version
            })
            || peer.capabilities.tools.is_none()
        {
            let _ = client.cancel().await;
            let _ = child.kill().await;
            let _ = child.wait().await;
            stderr_task.abort();
            return Err(McpError::new(
                "mcp.server_identity",
                "server protocol, identity or tools capability differs from trusted configuration",
            ));
        }
        Ok(Self {
            client,
            child,
            stderr: stderr_task,
            state,
            cancellation,
            server_digest,
            limits,
            discovery_generation: 0,
        })
    }

    async fn discover(&mut self) -> Result<Vec<ToolCapability>, McpError> {
        health(&self.state)?;
        if self.cancellation.is_cancelled() {
            return Err(McpError::new("mcp.cancelled", "session is terminal"));
        }
        let generation = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .generation;
        let deadline = Instant::now() + self.limits.timeout;
        let mut pages = 0usize;
        let mut descriptor_bytes = 0usize;
        let mut tools = Vec::new();
        let mut names = BTreeSet::new();
        let mut cursors = BTreeSet::new();
        let mut cursor = None;
        loop {
            pages += 1;
            let remaining = deadline.saturating_duration_since(Instant::now());
            if pages > self.limits.max_tools || remaining.is_zero() {
                return Err(McpError::new(
                    "mcp.pagination",
                    "discovery exceeded its page or total time bound",
                ));
            }
            let result = tokio::select! {
                biased;
                _=self.cancellation.cancelled()=>return Err(McpError::new("mcp.cancelled","discovery cancelled")),
                result=tokio::time::timeout(remaining,self.client.list_tools(Some(PaginatedRequestParams::default().with_cursor(cursor.clone()))))=>result,
            };
            let page = match result {
                Ok(Ok(page)) => page,
                other => {
                    return Err(health(&self.state).err().unwrap_or_else(|| {
                        McpError::new(
                            "mcp.discovery_failed",
                            match other {
                                Ok(Err(e)) => e.to_string(),
                                _ => "tool discovery timed out".into(),
                            },
                        )
                    }));
                }
            };
            health(&self.state)?;
            if tools.len() + page.tools.len() > self.limits.max_tools {
                return Err(McpError::new(
                    "mcp.tool_limit",
                    "discovered tool inventory exceeds configured bound",
                ));
            }
            for tool in page.tools {
                if tool.name.trim().is_empty() || !names.insert(tool.name.to_string()) {
                    return Err(McpError::new(
                        "mcp.invalid_descriptor",
                        "empty or duplicated tool name",
                    ));
                }
                let descriptor = serde_json::to_value(&tool)
                    .map_err(|e| McpError::new("mcp.invalid_descriptor", e.to_string()))?;
                descriptor_bytes += serde_json::to_vec(&descriptor)
                    .map_err(|e| McpError::new("mcp.invalid_descriptor", e.to_string()))?
                    .len();
                if descriptor_bytes > self.limits.max_retained_bytes {
                    return Err(McpError::new(
                        "mcp.tool_limit",
                        "descriptor inventory exceeds the retained byte bound",
                    ));
                }
                let schema = descriptor.get("inputSchema").expect("SDK input schema");
                jsonschema::validator_for(schema)
                    .map_err(|e| McpError::new("mcp.input_schema", e.to_string()))?;
                if let Some(output) = descriptor.get("outputSchema") {
                    jsonschema::validator_for(output)
                        .map_err(|e| McpError::new("mcp.output_schema", e.to_string()))?;
                }
                let capability_digest = digest(
                    &json!({"domain":"gaap.mcp-capability/0.1","server":self.server_digest,"protocol":MCP_PROTOCOL_VERSION,"tool":descriptor}),
                )?;
                tools.push(ToolCapability {
                    tool_name: tool.name.to_string(),
                    capability: CapabilityIdentity {
                        name: format!("mcp.{}", tool.name),
                        version: MCP_PROTOCOL_VERSION.into(),
                        digest: capability_digest,
                    },
                    schema_digest: digest(schema)?,
                    descriptor,
                });
            }
            match page.next_cursor {
                Some(next) => {
                    if next.is_empty() || !cursors.insert(next.clone()) {
                        return Err(McpError::new(
                            "mcp.pagination",
                            "repeated or empty discovery cursor",
                        ));
                    }
                    cursor = Some(next);
                }
                None => break,
            }
        }
        if self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .generation
            != generation
        {
            return Err(McpError::new(
                "mcp.capability_drift",
                "inventory changed during discovery; fetch a stable inventory",
            ));
        }
        self.discovery_generation = generation;
        tools.sort_by(|a, b| a.tool_name.cmp(&b.tool_name));
        Ok(tools)
    }

    async fn invoke(
        &mut self,
        effect: &ProtectedEffectRequest,
        arguments: Value,
        approved: &[CapabilityIdentity],
        policy: &PermissionPolicy,
        observer: &Arc<Mutex<Box<dyn crate::SubjectObserver>>>,
        deadline: Instant,
    ) -> Result<Invocation, McpError> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(McpError::new(
                "mcp.budget_elapsed",
                "run deadline expired before dispatch",
            ));
        }
        let tools = tokio::time::timeout(remaining, self.discover())
            .await
            .map_err(|_| {
                McpError::new(
                    "mcp.budget_elapsed",
                    "run deadline expired during discovery",
                )
            })??;
        let generation = self.discovery_generation;
        let tool = tools.iter().find(|tool| {
            tool.capability == effect.capability
                && Some(&tool.schema_digest) == effect.tool_schema_digest.as_ref()
        });
        let coordinator = RunCoordinator;
        let tool_decision=coordinator.evaluate(Gate::ToolTrust,&json!({
            "capability_digest":effect.capability.digest,
            "capability_name":effect.capability.name,
            "approval":if tool.is_some() && approved.contains(&effect.capability){json!({"status":"approved","subject_digest":effect.capability.digest})}else{Value::Null},
        }));
        if tool_decision.outcome != Outcome::Allow {
            return Ok(Invocation {
                status: EffectExecutionStatus::AwaitingAuthority,
                output: None,
                reason: Some(tool_decision.code),
                dispatched: false,
            });
        }
        let tool = tool.expect("allowed exact descriptor");
        validate_arguments(&tool.descriptor, &arguments)?;
        let effect_digest = digest(effect)?;
        let approval = policy
            .approvals
            .iter()
            .find(|approval| approval.subject_digest == effect_digest);
        let permission=coordinator.evaluate(Gate::Permission,&json!({
            "action_digest":effect_digest,"policy_decision":policy.policy_decision.map(|decision|match decision{gaap::runtime::PolicyDecision::Allow=>"allow",gaap::runtime::PolicyDecision::Ask=>"ask",gaap::runtime::PolicyDecision::Deny=>"deny"}),
            "risk_tags":policy.risk_tags,"wrapper_chain":policy.wrapper_chain,
            "approval":approval.map(|approval|json!({"status":"approved","subject_digest":approval.subject_digest})),
        }));
        if permission.outcome != Outcome::Allow {
            return Ok(Invocation {
                status: if permission.outcome == Outcome::Ask {
                    EffectExecutionStatus::AwaitingAuthority
                } else {
                    EffectExecutionStatus::Denied
                },
                output: None,
                reason: Some(permission.code),
                dispatched: false,
            });
        }
        let fresh = observer
            .lock()
            .map_err(|_| {
                McpError::new(
                    "mcp.observer_unavailable",
                    "host observation lock is poisoned",
                )
            })?
            .observe(effect)?;
        if !crate::usage_is_valid(&fresh) {
            return Err(McpError::new(
                "mcp.invalid_usage",
                "host usage exceeds the frozen contract range",
            ));
        }
        if fresh.subject != effect.subject {
            return Err(McpError::new(
                "mcp.stale_subject",
                "subject changed during capability discovery",
            ));
        }
        let call_timeout = self
            .limits
            .timeout
            .min(deadline.saturating_duration_since(Instant::now()));
        if call_timeout.is_zero() {
            return Err(McpError::new(
                "mcp.budget_elapsed",
                "run deadline expired before dispatch",
            ));
        }
        health(&self.state)?;
        if self.cancellation.is_cancelled() {
            return Ok(Invocation {
                status: EffectExecutionStatus::Denied,
                output: None,
                reason: Some("mcp.cancelled_before_dispatch".into()),
                dispatched: false,
            });
        }
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.call_generation = Some(generation);
            state.call_started = false;
        }
        let params = CallToolRequestParams::new(tool.tool_name.clone())
            .with_arguments(arguments.as_object().expect("validated arguments").clone());
        let call = tokio::select! {
            biased;
            _=self.cancellation.cancelled()=>Err(health(&self.state).err().unwrap_or_else(||McpError::new("mcp.cancelled","call cancellation prevents establishing its outcome"))),
            response=tokio::time::timeout(call_timeout,self.client.peer().call_tool(params))=>match response {
                Ok(Ok(response))=>Ok(response),
                Ok(Err(error))=>Err(health(&self.state).err().unwrap_or_else(||McpError::new("mcp.call_failed",error.to_string()))),
                Err(_)=>Err(McpError::new("mcp.timeout","call was dispatched; effect outcome is unknown")),
            }
        };
        tokio::task::yield_now().await;
        let call = call.and_then(|response| {
            health(&self.state)?;
            if self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .generation
                != generation
            {
                return Err(McpError::new(
                    "mcp.capability_drift",
                    "tool inventory changed during invocation",
                ));
            }
            Ok(response)
        });
        match call {
            Ok(result) => {
                let output = serde_json::to_value(&result)
                    .map_err(|e| McpError::new("mcp.invalid_result", e.to_string()))?;
                if let Some(schema) = tool.descriptor.get("outputSchema") {
                    let valid = result.structured_content.as_ref().is_some_and(|content| {
                        jsonschema::validator_for(schema)
                            .is_ok_and(|validator| validator.is_valid(content))
                    });
                    if !valid {
                        fail(
                            &self.state,
                            "mcp.output_schema",
                            "structured output doesn't satisfy the advertised schema",
                        );
                        return Ok(Invocation {
                            status: EffectExecutionStatus::UnknownOutcome,
                            output: Some(output),
                            reason: Some("mcp.output_schema".into()),
                            dispatched: true,
                        });
                    }
                }
                Ok(Invocation {
                    status: if result.is_error == Some(true) {
                        EffectExecutionStatus::Failed
                    } else {
                        EffectExecutionStatus::Executed
                    },
                    output: Some(output),
                    reason: if result.is_error == Some(true) {
                        Some("mcp.tool_error".into())
                    } else {
                        None
                    },
                    dispatched: true,
                })
            }
            Err(error) => {
                if matches!(error.code, "mcp.cancelled" | "mcp.timeout") {
                    let id = self
                        .state
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .pending
                        .iter()
                        .find(|(_, method)| method.as_str() == "tools/call")
                        .map(|(id, _)| id.clone());
                    if let Some(id) = id.and_then(|id| serde_json::from_str(&id).ok()) {
                        // Cancellation is advisory; neither delivery nor cleanup proves no effect.
                        let _ = tokio::time::timeout(
                            Duration::from_millis(100),
                            self.client.notify_cancelled(
                                rmcp::model::CancelledNotificationParam::new(
                                    Some(id),
                                    Some("GAAP ended the bounded call".into()),
                                ),
                            ),
                        )
                        .await;
                    }
                }
                fail(&self.state, error.code, error.detail.clone());
                let _ = self.child.kill().await;
                let _ = self.child.wait().await;
                let dispatched = self
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .call_started;
                Ok(Invocation {
                    status: if dispatched {
                        EffectExecutionStatus::UnknownOutcome
                    } else {
                        EffectExecutionStatus::Denied
                    },
                    output: None,
                    reason: Some(error.to_string()),
                    dispatched,
                })
            }
        }
    }

    async fn close(mut self) {
        self.cancellation.cancel();
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
        let _ = tokio::time::timeout(self.limits.timeout, self.client.cancel()).await;
        self.stderr.abort();
        let _ = self.stderr.await;
    }
}
