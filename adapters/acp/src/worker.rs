use std::path::Path;
use std::process::Stdio;
use std::sync::mpsc;
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::*;
use agent_client_protocol::{Agent, Client, ConnectionTo, Lines};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::{AgentConfig, Event, EvidenceHandle, FileOperation, Limits, error};
use gaap::contracts::AgentRunRequest;
use gaap::runtime::RuntimePortError;

pub(crate) fn directory_identity(path: &Path) -> Result<String, RuntimePortError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::metadata(path).map_err(|_| error("acp.directory_unreadable"))?;
        Ok(format!("{}:{}", meta.dev(), meta.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(error("acp.platform_unsupported"))
    }
}

pub(crate) fn validate_launch(config: &AgentConfig) -> Result<(), RuntimePortError> {
    if config.expected_name.is_empty()
        || config.expected_version.is_empty()
        || !config.working_directory.is_absolute()
        || !config.working_directory.is_dir()
        || config.working_directory.canonicalize().ok().as_ref() != Some(&config.working_directory)
        || config.artifacts.is_empty()
        || config.artifacts.len() > 32
        || config.arguments.len() > 64
        || config.environment.len() > 32
    {
        return Err(error("acp.invalid_launch"));
    }
    if config
        .environment
        .keys()
        .any(|key| key.is_empty() || key.contains('=') || key.contains('\0'))
        || config
            .environment
            .values()
            .any(|v| v.len() > 8192 || v.contains('\0'))
        || config
            .arguments
            .iter()
            .any(|a| a.len() > 8192 || a.contains('\0'))
    {
        return Err(error("acp.invalid_launch"));
    }
    // Explicit loader injection isn't part of this trusted launch profile.
    if config.environment.keys().any(|key| {
        key.starts_with("LD_")
            || key.starts_with("DYLD_")
            || key.starts_with("PYTHON")
            || matches!(
                key.as_str(),
                "NODE_OPTIONS"
                    | "NODE_PATH"
                    | "RUBYOPT"
                    | "RUBYLIB"
                    | "PERL5OPT"
                    | "PERL5LIB"
                    | "BASH_ENV"
                    | "ENV"
            )
    }) {
        return Err(error("acp.loader_environment_unsupported"));
    }
    for artifact in &config.artifacts {
        if !artifact.path.is_absolute()
            || artifact.path.canonicalize().ok().as_ref() != Some(&artifact.path)
        {
            return Err(error("acp.noncanonical_artifact"));
        }
        let metadata =
            std::fs::metadata(&artifact.path).map_err(|_| error("acp.artifact_missing"))?;
        if !metadata.is_file() || metadata.len() > 128 * 1024 * 1024 {
            return Err(error("acp.artifact_limit"));
        }
        let bytes = std::fs::read(&artifact.path).map_err(|_| error("acp.artifact_unreadable"))?;
        if artifact.digest != format!("sha256:{:x}", Sha256::digest(&bytes)) {
            return Err(error("acp.artifact_changed"));
        }
    }
    if !config.artifacts.iter().any(|a| a.path == config.executable) {
        return Err(error("acp.executable_unapproved"));
    }
    let file = std::fs::read(&config.executable).map_err(|_| error("acp.executable_unreadable"))?;
    if file.starts_with(b"#!") {
        return Err(error("acp.shebang_unsupported"));
    }
    let name = config
        .executable
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if matches!(
        name,
        "sh" | "bash" | "zsh" | "env" | "npx" | "uvx" | "cargo" | "deno" | "ruby" | "perl"
    ) {
        return Err(error("acp.launcher_unsupported"));
    }
    if name.starts_with("python") || name == "node" {
        let script = config
            .arguments
            .first()
            .ok_or_else(|| error("acp.script_required"))?;
        let script = config
            .working_directory
            .join(script)
            .canonicalize()
            .map_err(|_| error("acp.script_unapproved"))?;
        if !config.artifacts.iter().any(|a| a.path == script) {
            return Err(error("acp.script_unapproved"));
        }
    }
    for arg in &config.arguments {
        let path = config.working_directory.join(arg);
        if path.is_file()
            && !config
                .artifacts
                .iter()
                .any(|a| path.canonicalize().ok().as_ref() == Some(&a.path))
        {
            return Err(error("acp.argument_unapproved"));
        }
    }
    Ok(())
}

pub(crate) fn start(
    config: AgentConfig,
    limits: Limits,
    request: AgentRunRequest,
    evidence: EvidenceHandle,
    cancel: CancellationToken,
) -> (
    mpsc::Receiver<Event>,
    oneshot::Sender<()>,
    mpsc::Receiver<()>,
) {
    let (events, receiver) = mpsc::channel();
    let (start, start_receiver) = oneshot::channel();
    let (stopped, stopped_receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build();
        let result = match runtime {
            Ok(runtime) => runtime.block_on(run(
                config,
                limits,
                request,
                evidence.clone(),
                cancel,
                events.clone(),
                start_receiver,
            )),
            Err(_) => Err("acp.runtime_unavailable"),
        };
        let _ = stopped.send(());
        if let Err(code) = result {
            let code = evidence.record_failure(code);
            // Error codes come from fixed transport checks, never raw server diagnostics.
            let _ = events.send(Event::Fault(code));
        }
    });
    (receiver, start, stopped_receiver)
}

#[allow(clippy::too_many_arguments)]
async fn run(
    config: AgentConfig,
    limits: Limits,
    request: AgentRunRequest,
    evidence: EvidenceHandle,
    cancel: CancellationToken,
    events: mpsc::Sender<Event>,
    start: oneshot::Receiver<()>,
) -> Result<(), &'static str> {
    validate_launch(&config).map_err(|_| "acp.launch_changed")?;
    if config
        .capability()
        .map_err(|_| "acp.launch_changed")?
        .digest
        != evidence.snapshot().capability_digest
    {
        return Err("acp.launch_changed");
    }
    let mut child = tokio::process::Command::new(&config.executable)
        .args(&config.arguments)
        .env_clear()
        .envs(&config.environment)
        .current_dir(&config.working_directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| "acp.launch_failed")?;
    let stdin = child.stdin.take().ok_or("acp.pipe_failed")?;
    let stdout = child.stdout.take().ok_or("acp.pipe_failed")?;
    let mut stderr = child.stderr.take().ok_or("acp.pipe_failed")?;
    let stderr_limit = async move {
        let mut count = 0usize;
        let mut buffer = [0u8; 4096];
        loop {
            let length = stderr
                .read(&mut buffer)
                .await
                .map_err(|_| "acp.stderr_failed")?;
            if length == 0 {
                return std::future::pending::<Result<(), &'static str>>().await;
            }
            count = count.saturating_add(length);
            if count > limits.max_stderr_bytes {
                return Err("acp.stderr_limit");
            }
        }
    };
    let (outgoing, incoming) = crate::wire::transport(stdin, stdout, limits, evidence.clone());
    let read_events = events.clone();
    let write_events = events.clone();
    let permission_events = events.clone();
    let permission_evidence = evidence.clone();
    let setup_evidence = evidence.clone();
    let notification_events = events.clone();
    let notification_evidence = evidence.clone();
    let run_budget = request.resource_budget.max_elapsed_ms;
    let prompt_cancel = cancel.clone();
    let session=Client.builder()
        .on_receive_notification(async move |notification:SessionNotification,_cx| {
            let update=serde_json::to_value(notification.update).map_err(|_|agent_client_protocol::Error::internal_error())?;
            let kind=update["sessionUpdate"].as_str().unwrap_or("");
            if matches!(kind,"tool_call"|"tool_call_update") {
                let tool_id_digest=crate::digest(&update["toolCallId"]).map_err(|_|agent_client_protocol::Error::internal_error())?;
                let progress = crate::ToolProgress {
                    tool_id_digest,
                    status:update["status"].as_str().map(str::to_owned),
                    is_update:kind=="tool_call_update",
                };
                notification_evidence.0.lock().expect("evidence lock").tool_progress.push(progress.clone());
                notification_events.send(Event::ToolProgress(progress)).map_err(|_|agent_client_protocol::Error::internal_error())?;
            }
            Ok(())
        },agent_client_protocol::on_receive_notification!())
        .on_receive_request(async move |request:ReadTextFileRequest,responder,_cx| {
            let (reply,result)=oneshot::channel();
            read_events.send(Event::Effect(FileOperation::Read { path:request.path,line:request.line,limit:request.limit },reply))
                .map_err(|_|agent_client_protocol::Error::internal_error())?;
            match result.await {
                Ok(Ok(Some(text)))=>responder.respond(ReadTextFileResponse::new(text)),
                _=>responder.respond_with_error(agent_client_protocol::Error::internal_error()),
            }
        },agent_client_protocol::on_receive_request!())
        .on_receive_request(async move |request:WriteTextFileRequest,responder,_cx| {
            let (reply,result)=oneshot::channel();
            write_events.send(Event::Effect(FileOperation::Write { path:request.path,content:request.content },reply))
                .map_err(|_|agent_client_protocol::Error::internal_error())?;
            match result.await {
                Ok(Ok(_))=>responder.respond(WriteTextFileResponse::new()),
                _=>responder.respond_with_error(agent_client_protocol::Error::internal_error()),
            }
        },agent_client_protocol::on_receive_request!())
        .on_receive_request(async move |_request:RequestPermissionRequest,responder,_cx| {
            permission_evidence.record_failure("acp.authority_required");
            let result=responder.respond(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled));
            let _=permission_events.send(Event::Fault("acp.authority_required".into()));
            result
        },agent_client_protocol::on_receive_request!())
        .connect_with(Lines::new(outgoing,incoming),async move |connection:ConnectionTo<Agent>| {
            let init=connection.send_request(InitializeRequest::new(ProtocolVersion::V1)
                .client_capabilities(ClientCapabilities::new().fs(FileSystemCapabilities::new().read_text_file(true).write_text_file(true))))
                .block_task().await?;
            let identity=init.agent_info.ok_or_else(agent_client_protocol::Error::invalid_request)?;
            if identity.name!=config.expected_name || identity.version!=config.expected_version {
                setup_evidence.record_failure("acp.agent_identity_mismatch");
                return Err(agent_client_protocol::Error::invalid_request());
            }
            let session=connection.send_request(NewSessionRequest::new(config.working_directory)).block_task().await?;
            events.send(Event::Ready).map_err(|_|agent_client_protocol::Error::internal_error())?;
            start.await.map_err(|_|agent_client_protocol::Error::internal_error())?;
            let prompt=format!("{}\n\nConstraints:\n{}",request.task.instructions,request.task.constraints.join("\n"));
            let response=connection.send_request(PromptRequest::new(session.session_id.clone(),vec![ContentBlock::Text(TextContent::new(prompt))]));
            let result=tokio::select! {
                response=response.block_task()=>response?,
                _=prompt_cancel.cancelled()=>{
                    let _=connection.send_notification(CancelNotification::new(session.session_id));
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    return Err(agent_client_protocol::Error::internal_error());
                }
            };
            let reason=serde_json::to_value(result.stop_reason).map_err(|_|agent_client_protocol::Error::internal_error())?;
            events.send(Event::Finished(reason.as_str().unwrap_or("unknown").into())).map_err(|_|agent_client_protocol::Error::internal_error())?;
            Ok(())
        });
    let duration = limits
        .operation_timeout
        .saturating_add(Duration::from_millis(run_budget));
    let result = tokio::select! {
        result=session=>result.map_err(|_|"acp.transport_failed"),
        result=stderr_limit=>result,
        _=tokio::time::sleep(duration)=>Err("acp.timeout"),
        // Covers setup and a blocked plan; prompt cancellation also sends session/cancel.
        _=cancelled_before_prompt(cancel.clone())=>Err("acp.cancelled"),
    };
    let _ = child.kill().await;
    let _ = tokio::time::timeout(Duration::from_millis(500), child.wait()).await;
    result
}

async fn cancelled_before_prompt(cancel: CancellationToken) {
    cancel.cancelled().await;
    // Allow the in-prompt cancellation handler to flush its notification.
    tokio::time::sleep(Duration::from_millis(25)).await;
}
