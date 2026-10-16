//! Explicit local OCI reproducibility profile; ordinary containers aren't a hostile-code sandbox.
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use gaap::sandbox::{
    SandboxAdapter, SandboxArtifact, SandboxCleanup, SandboxNetwork, SandboxObservation,
    SandboxRequest, SandboxRuntimeIdentity, SandboxStatus,
};
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

type Checked<T> = Result<T, String>;

/// Trusted host selection. The adapter never changes context or selects another runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OciConfig {
    pub executable: PathBuf,
    pub environment: BTreeMap<String, String>,
    pub state_directory: PathBuf,
    pub approved_profile_digest: String,
    pub engine_version: String,
    pub runtime: String,
    pub runtime_version: String,
}

pub struct LocalOciAdapter {
    config: OciConfig,
}

pub fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn canonical<T: Serialize>(value: &T) -> Checked<Vec<u8>> {
    serde_jcs::to_vec(value).map_err(|error| error.to_string())
}

fn absolute(path: &str) -> bool {
    path.starts_with('/')
        && (path == "/" || !path.ends_with('/'))
        && !path.contains("//")
        && !path.contains(['\0', '\n', '\r', ','])
        && !Path::new(path)
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
        && !path.split('/').any(|part| part == "." || part == "..")
}

fn safe_host_path(path: &str) -> Checked<PathBuf> {
    if !absolute(path) || path == "/" {
        return Err("sandbox.mount_path: select an absolute bounded host path".into());
    }
    let selected = PathBuf::from(path);
    if fs::canonicalize(&selected).map_err(|error| error.to_string())? != selected {
        return Err("sandbox.mount_path: symlinked or noncanonical host path".into());
    }
    let metadata = fs::symlink_metadata(&selected).map_err(|error| error.to_string())?;
    if !metadata.is_file() && !metadata.is_dir() {
        return Err("sandbox.mount_path: select a regular file or directory".into());
    }
    Ok(selected)
}

fn valid_digest(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn write_record(directory: &Path, name: &str, bytes: &[u8]) -> Checked<PathBuf> {
    let path = directory.join(name);
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&path)
        .map_err(|error| format!("sandbox.record {}: {error}", path.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("sandbox.record {}: {error}", path.display()))?;
    fs::File::open(directory)
        .and_then(|parent| parent.sync_all())
        .map_err(|error| format!("sandbox.record {}: {error}", path.display()))?;
    Ok(path)
}

fn validate(request: &SandboxRequest) -> Checked<()> {
    let limits = &request.limits;
    if !valid_digest(&request.invocation_digest) || !valid_digest(&request.input_subject.digest) {
        return Err("sandbox.binding: require exact protected-effect and subject digests".into());
    }
    if request.schema_version != "gaap.sandbox/0.1" || request.profile_id != "local-oci/0.1" {
        return Err("sandbox.version: unsupported local OCI profile".into());
    }
    if request.image.digest.len() != 71
        || !request.image.digest.starts_with("sha256:")
        || !request.image.digest[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        || !request
            .image
            .reference
            .ends_with(&format!("@{}", request.image.digest))
        || request.image.reference.len() > 512
        || request.image.reference.starts_with('-')
    {
        return Err("sandbox.image: select an immutable image reference and exact digest".into());
    }
    if !matches!(request.network, SandboxNetwork::Disabled) {
        return Err(
            "sandbox.network: this profile supports only disabled networking; no fallback".into(),
        );
    }
    if request.command.is_empty()
        || request.command.len() > 64
        || !absolute(&request.command[0])
        || request
            .command
            .iter()
            .any(|argument| argument.contains('\0') || argument.len() > 32768)
        || !absolute(&request.working_directory)
    {
        return Err(
            "sandbox.command: use an absolute executable, working directory and bounded arguments"
                .into(),
        );
    }
    let user: Vec<_> = request.user.split(':').collect();
    if user.len() != 2
        || user
            .iter()
            .any(|value| value.parse::<u32>().ok().is_none_or(|id| id == 0))
    {
        return Err("sandbox.user: use an explicit non-root numeric UID and GID".into());
    }
    if request.environment.len() > 64
        || request.environment.iter().any(|(name, value)| {
            name.is_empty()
                || name.len() > 128
                || name.as_bytes()[0].is_ascii_digit()
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                || value.contains('\0')
                || value.len() > 8192
        })
    {
        return Err("sandbox.environment: invalid explicit environment allowlist".into());
    }
    if !(1..=64000).contains(&limits.cpu_millis_per_second)
        || !(16 * 1024 * 1024..=8 * 1024 * 1024 * 1024).contains(&limits.memory_bytes)
        || !(1..=1024).contains(&limits.processes)
        || !(1..=1024 * 1024).contains(&limits.stdout_bytes)
        || !(1..=1024 * 1024).contains(&limits.stderr_bytes)
        || !(1..=300000).contains(&limits.wall_clock_ms)
        || !(100..=10000).contains(&limits.cleanup_ms)
        || request.outputs.max_total_bytes > 16 * 1024 * 1024
        || request.outputs.paths.len() > 64
    {
        return Err(
            "sandbox.limits: explicit resource, output and cleanup bounds are required".into(),
        );
    }
    if request.mounts.is_empty() || request.mounts.len() > 16 {
        return Err("sandbox.mounts: declare bounded input mounts".into());
    }
    for (index, mount) in request.mounts.iter().enumerate() {
        safe_host_path(&mount.source)?;
        if !absolute(&mount.destination)
            || mount.destination == "/"
            || [
                "/proc", "/sys", "/dev", "/etc", "/usr", "/bin", "/sbin", "/lib", "/lib64",
            ]
            .iter()
            .any(|base| Path::new(&mount.destination).starts_with(base))
        {
            return Err("sandbox.mounts: mount destination shadows a system path".into());
        }
        if request.mounts[..index].iter().any(|prior| {
            Path::new(&mount.destination).starts_with(&prior.destination)
                || Path::new(&prior.destination).starts_with(&mount.destination)
        }) {
            return Err("sandbox.mounts: overlapping mount destinations are unsupported".into());
        }
    }
    for output in &request.outputs.paths {
        if !absolute(output)
            || !request
                .mounts
                .iter()
                .any(|mount| mount.writable && Path::new(output).starts_with(&mount.destination))
        {
            return Err(
                "sandbox.outputs: collect only paths inside declared writable mounts".into(),
            );
        }
    }
    Ok(())
}

struct ProcessOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    code: Option<i32>,
    stopped: Option<SandboxStatus>,
}

fn bounded_read(
    mut stream: impl Read,
    maximum: usize,
    overflow: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
) -> std::io::Result<Vec<u8>> {
    let mut retained = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let read = match stream.read(&mut chunk) {
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if stop.load(Ordering::Acquire) {
                    return Ok(retained);
                }
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if read == 0 {
            return Ok(retained);
        }
        let remaining = maximum.saturating_sub(retained.len());
        retained.extend_from_slice(&chunk[..read.min(remaining)]);
        if read > remaining {
            overflow.store(true, Ordering::Release);
        }
    }
}

impl LocalOciAdapter {
    pub fn new(config: OciConfig) -> Self {
        Self { config }
    }

    fn process(
        &self,
        arguments: &[String],
        deadline: Instant,
        output_limit: usize,
        error_limit: usize,
        cancelled: &dyn Fn() -> bool,
    ) -> Checked<ProcessOutput> {
        if Instant::now() >= deadline {
            return Err("sandbox.control_timeout: deadline elapsed before dispatch".into());
        }
        let mut child = Command::new(&self.config.executable)
            .args(arguments)
            .env_clear()
            .envs(&self.config.environment)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .map_err(|error| error.to_string())?;
        let overflow = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let group = Pid::from_raw(child.id() as i32);
        let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("sandbox.transport: missing owned output pipe".into());
        };
        if let Err(error) = fcntl(&stdout, FcntlArg::F_SETFL(OFlag::O_NONBLOCK))
            .and_then(|_| fcntl(&stderr, FcntlArg::F_SETFL(OFlag::O_NONBLOCK)))
        {
            let _ = killpg(group, Signal::SIGKILL);
            let _ = child.wait();
            return Err(error.to_string());
        }
        let output_overflow = overflow.clone();
        let error_overflow = overflow.clone();
        let output_stop = stop.clone();
        let error_stop = stop.clone();
        let output = std::thread::spawn(move || {
            bounded_read(stdout, output_limit, output_overflow, output_stop)
        });
        let error = std::thread::spawn(move || {
            bounded_read(stderr, error_limit, error_overflow, error_stop)
        });
        let mut stopped = None;
        let mut exited = None;
        let status = loop {
            match if exited.is_none() {
                child.try_wait()
            } else {
                Ok(None)
            } {
                Ok(Some(status)) => exited = Some(status),
                Ok(None) => (),
                Err(error) => {
                    let _ = killpg(group, Signal::SIGKILL);
                    let _ = child.wait();
                    break Err(error.to_string());
                }
            }
            if let Some(status) = exited
                && output.is_finished()
                && error.is_finished()
            {
                break Ok(status);
            }
            stopped = if cancelled() {
                Some(SandboxStatus::Cancelled)
            } else if overflow.load(Ordering::Acquire) {
                Some(SandboxStatus::OutputOverflow)
            } else if Instant::now() >= deadline {
                Some(SandboxStatus::TimedOut)
            } else {
                None
            };
            if stopped.is_some() {
                let _ = killpg(group, Signal::SIGKILL);
                break match exited {
                    Some(status) => Ok(status),
                    None => child.wait().map_err(|error| error.to_string()),
                };
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        stop.store(true, Ordering::Release);
        let stdout = output.join();
        let stderr = error.join();
        let status = status?;
        let stdout = stdout
            .map_err(|_| "sandbox.stdout: reader failed")?
            .map_err(|error| error.to_string())?;
        let stderr = stderr
            .map_err(|_| "sandbox.stderr: reader failed")?
            .map_err(|error| error.to_string())?;
        if stopped.is_none() && overflow.load(Ordering::Acquire) {
            stopped = Some(SandboxStatus::OutputOverflow);
        }
        Ok(ProcessOutput {
            stdout,
            stderr,
            code: status.code(),
            stopped,
        })
    }

    fn control(
        &self,
        arguments: &[&str],
        deadline: Instant,
        cancelled: &dyn Fn() -> bool,
    ) -> Checked<Vec<u8>> {
        let result = self.process(
            &arguments
                .iter()
                .map(|value| (*value).into())
                .collect::<Vec<_>>(),
            deadline,
            1024 * 1024,
            65536,
            cancelled,
        )?;
        if let Some(status) = result.stopped {
            let code = match status {
                SandboxStatus::Cancelled => "sandbox.control_cancelled",
                SandboxStatus::TimedOut => "sandbox.control_timeout",
                _ => "sandbox.control_overflow",
            };
            return Err(format!("{code}: bounded metadata operation stopped"));
        }
        if result.code != Some(0) {
            return Err(format!(
                "sandbox.control_failed: {}: {}",
                arguments.first().unwrap_or(&"control"),
                String::from_utf8_lossy(&result.stderr)
            ));
        }
        Ok(result.stdout)
    }

    fn inspect(
        &self,
        name: &str,
        deadline: Instant,
        cancelled: &dyn Fn() -> bool,
    ) -> Checked<Value> {
        let raw = self.control(&["container", "inspect", name], deadline, cancelled)?;
        let values: Vec<Value> = serde_json::from_slice(&raw).map_err(|error| error.to_string())?;
        values
            .into_iter()
            .next()
            .ok_or("sandbox.inspect: empty result".into())
    }
}

impl LocalOciAdapter {
    fn oom_event(
        &self,
        name: &str,
        owner: &str,
        container_id: &str,
        deadline: Instant,
        cancelled: &dyn Fn() -> bool,
    ) -> Checked<Option<Value>> {
        // The daemon can emit OOM independently of its terminal State marker.
        // Read its cached/live event stream within this execution's deadline;
        // the bounded process reader retains complete events before stopping it.
        let probe_deadline = deadline.min(Instant::now() + Duration::from_millis(250));
        let output = self.process(
            &[
                "events".into(),
                "--since".into(),
                "1".into(),
                "--filter".into(),
                format!("container={name}"),
                "--filter".into(),
                "event=oom".into(),
                "--format".into(),
                "{{json .}}".into(),
            ],
            probe_deadline,
            32768,
            8192,
            cancelled,
        )?;
        check_budget(deadline, cancelled)?;
        for line in output.stdout.split(|byte| *byte == b'\n') {
            let Ok(event) = serde_json::from_slice::<Value>(line) else {
                continue;
            };
            if event["Type"] == "container"
                && event["Action"] == "oom"
                && event["Actor"]["ID"] == container_id
                && event["Actor"]["Attributes"]["org.gaap.owner"] == owner
            {
                return Ok(Some(serde_json::json!({
                    "type":"container", "action":"oom", "container_id":container_id, "owner":owner,
                    "time":event["time"],
                    "time_nano":event["timeNano"].as_u64().map(|value| value.to_string()),
                })));
            }
        }
        Ok(None)
    }

    fn preflight(
        &mut self,
        request: &SandboxRequest,
        deadline: Instant,
        cancelled: &dyn Fn() -> bool,
    ) -> Checked<(SandboxRuntimeIdentity, String)> {
        if request.profile_digest()? != self.config.approved_profile_digest
            || !valid_digest(&self.config.approved_profile_digest)
        {
            return Err("sandbox.profile_authority: request environment differs from the host-approved profile".into());
        }
        if self.config.environment.contains_key("DOCKER_CONTEXT")
            || !self
                .config
                .environment
                .get("DOCKER_HOST")
                .is_some_and(|endpoint| {
                    endpoint
                        .strip_prefix("unix://")
                        .is_some_and(|path| absolute(path) && path != "/")
                })
        {
            return Err(
                "sandbox.daemon: select an explicit local Unix socket without a context override"
                    .into(),
            );
        }
        validate(request)?;
        let state = &self.config.state_directory;
        let metadata = fs::symlink_metadata(state).map_err(|error| error.to_string())?;
        if !state.is_absolute()
            || fs::canonicalize(state).map_err(|error| error.to_string())? != *state
            || !metadata.is_dir()
            || metadata.permissions().mode() & 0o077 != 0
            || request.mounts.iter().any(|mount| {
                state.starts_with(&mount.source) || Path::new(&mount.source).starts_with(state)
            })
        {
            return Err("sandbox.state: select a private canonical host directory outside all workload mounts".into());
        }
        if !self.config.executable.is_absolute()
            || self.config.runtime != "runc"
            || self.config.engine_version.is_empty()
            || self.config.runtime_version.is_empty()
        {
            return Err(
                "sandbox.runtime: explicitly select the local runc engine and versions".into(),
            );
        }
        self.config.executable =
            fs::canonicalize(&self.config.executable).map_err(|error| error.to_string())?;
        let executable_digest = hash_executable(&self.config.executable, deadline, cancelled)?;
        let version: Value = serde_json::from_slice(&self.control(
            &["version", "--format", "{{json .Server}}"],
            deadline,
            cancelled,
        )?)
        .map_err(|error| error.to_string())?;
        let runtime_version = version["Components"]
            .as_array()
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item["Name"] == self.config.runtime)
            })
            .and_then(|item| item["Version"].as_str())
            .ok_or("sandbox.runtime: selected runtime version wasn't observed")?;
        if version["Version"] != self.config.engine_version
            || runtime_version != self.config.runtime_version
            || version["Os"] != "linux"
        {
            return Err(
                "sandbox.runtime: actual engine/runtime version differs from host adoption".into(),
            );
        }
        let runtime = SandboxRuntimeIdentity {
            engine: "docker-compatible".into(),
            engine_version: self.config.engine_version.clone(),
            runtime: self.config.runtime.clone(),
            runtime_version: runtime_version.into(),
            executable_digest,
        };
        let images: Vec<Value> = serde_json::from_slice(&self.control(
            &["image", "inspect", &request.image.reference],
            deadline,
            cancelled,
        )?)
        .map_err(|error| error.to_string())?;
        let image = images
            .first()
            .ok_or("sandbox.image: image isn't installed")?;
        if !image["RepoDigests"].as_array().is_some_and(|entries| {
            entries.iter().any(|entry| {
                entry
                    .as_str()
                    .is_some_and(|value| value.ends_with(&format!("@{}", request.image.digest)))
            })
        }) {
            return Err(
                "sandbox.image: installed image doesn't resolve to the selected digest".into(),
            );
        }
        if image["Config"]["Volumes"]
            .as_object()
            .is_some_and(|volumes| !volumes.is_empty())
        {
            return Err("sandbox.image: implicit image volumes aren't allowed".into());
        }
        let image_id = image["Id"]
            .as_str()
            .ok_or("sandbox.image: missing image configuration identity")?
            .to_owned();
        Ok((runtime, image_id))
    }

    fn create_arguments(&self, request: &SandboxRequest, name: &str, owner: &str) -> Vec<String> {
        let limits = &request.limits;
        let mut arguments: Vec<String> = [
            "container",
            "create",
            "--pull=never",
            "--name",
            name,
            "--label",
            &format!("org.gaap.owner={owner}"),
            "--runtime",
            &self.config.runtime,
            "--network",
            "none",
            "--read-only",
            "--cap-drop",
            "ALL",
            "--security-opt",
            "no-new-privileges",
            "--user",
            &request.user,
            "--workdir",
            &request.working_directory,
            "--ipc",
            "private",
            "--no-healthcheck",
            "--log-driver",
            "none",
            "--entrypoint",
            "/usr/bin/env",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        arguments.extend([
            "--cpus".into(),
            format!(
                "{}.{:03}",
                limits.cpu_millis_per_second / 1000,
                limits.cpu_millis_per_second % 1000
            ),
            "--memory".into(),
            limits.memory_bytes.to_string(),
            "--memory-swap".into(),
            limits.memory_bytes.to_string(),
            "--pids-limit".into(),
            limits.processes.to_string(),
            "--ulimit".into(),
            "core=0:0".into(),
            "--ulimit".into(),
            format!("fsize={0}:{0}", request.outputs.max_total_bytes.max(8192)),
        ]);
        for mount in &request.mounts {
            let option = if mount.writable { "" } else { ",readonly" };
            arguments.extend(["--mount".into(), format!("type=bind,src={},dst={},bind-recursive=disabled,bind-propagation=rprivate{option}", mount.source, mount.destination)]);
        }
        arguments.extend([request.image.reference.clone(), "-i".into()]);
        arguments.extend(
            request
                .environment
                .iter()
                .map(|(name, value)| format!("{name}={value}")),
        );
        arguments.extend(request.command.iter().cloned());
        arguments
    }

    fn validate_container(
        &self,
        value: &Value,
        request: &SandboxRequest,
        owner: &str,
        image_id: &str,
    ) -> Checked<()> {
        let host = &value["HostConfig"];
        let config = &value["Config"];
        let no_extra_access = ["CapAdd", "Devices", "DeviceRequests"].iter().all(|field| {
            host[*field].is_null() || host[*field].as_array().is_some_and(Vec::is_empty)
        });
        if config["Labels"]["org.gaap.owner"] != owner
            || value["Image"] != image_id
            || config["User"] != request.user
            || config["WorkingDir"] != request.working_directory
            || config["Entrypoint"] != serde_json::json!(["/usr/bin/env"])
            || host["Runtime"] != self.config.runtime
            || host["NetworkMode"] != "none"
            || host["ReadonlyRootfs"] != true
            || host["Privileged"] != false
            || host["IpcMode"] != "private"
            || host["PidMode"].as_str().is_none_or(|mode| !mode.is_empty())
            || host["Memory"].as_u64() != Some(request.limits.memory_bytes)
            || host["MemorySwap"].as_u64() != Some(request.limits.memory_bytes)
            || host["NanoCpus"].as_u64()
                != Some(u64::from(request.limits.cpu_millis_per_second) * 1_000_000)
            || host["PidsLimit"].as_u64() != Some(u64::from(request.limits.processes))
            || host["CapDrop"] != serde_json::json!(["ALL"])
            || !no_extra_access
            || !host["SecurityOpt"].as_array().is_some_and(|options| {
                options.len() == 1
                    && options.iter().all(|option| {
                        option.as_str().is_some_and(|text| {
                            text == "no-new-privileges" || text == "no-new-privileges:true"
                        })
                    })
            })
        {
            return Err(
                "sandbox.profile_mismatch: created container differs from the adopted profile"
                    .into(),
            );
        }
        let mounts = value["Mounts"]
            .as_array()
            .ok_or("sandbox.mounts: missing observed mounts")?;
        if mounts.len() != request.mounts.len()
            || request.mounts.iter().any(|expected| {
                !mounts.iter().any(|observed| {
                    observed["Type"] == "bind"
                        && observed["Source"] == expected.source
                        && observed["Destination"] == expected.destination
                        && observed["RW"] == expected.writable
                        && observed["Propagation"] == "rprivate"
                })
            })
        {
            return Err("sandbox.profile_mismatch: observed mounts differ from adoption".into());
        }
        let mut command = vec!["-i".to_owned()];
        command.extend(
            request
                .environment
                .iter()
                .map(|(name, value)| format!("{name}={value}")),
        );
        command.extend(request.command.iter().cloned());
        if config["Cmd"] != serde_json::json!(command) {
            return Err("sandbox.profile_mismatch: observed command differs from adoption".into());
        }
        Ok(())
    }

    fn cleanup(&self, name: &str, owner: &str, deadline: Instant) -> Checked<()> {
        // Cancellation can't skip cleanup; this independent deadline bounds its acknowledgement.
        let value = self.inspect(name, deadline, &|| false)?;
        if value["Config"]["Labels"]["org.gaap.owner"] != owner {
            return Err("sandbox.cleanup: selected resource isn't owned by this execution".into());
        }
        self.control(
            &["container", "rm", "--force", "--volumes", name],
            deadline,
            &|| false,
        )?;
        let remaining = self.control(
            &[
                "container",
                "ls",
                "--all",
                "--filter",
                &format!("name=^/{name}$"),
                "--format",
                "{{.ID}}",
            ],
            deadline,
            &|| false,
        )?;
        if !remaining.iter().all(u8::is_ascii_whitespace) {
            return Err("sandbox.cleanup: resource removal wasn't confirmed".into());
        }
        Ok(())
    }
}

fn check_budget(deadline: Instant, cancelled: &dyn Fn() -> bool) -> Checked<()> {
    if cancelled() {
        Err("sandbox.control_cancelled: execution cancelled".into())
    } else if Instant::now() >= deadline {
        Err("sandbox.control_timeout: execution deadline elapsed".into())
    } else {
        Ok(())
    }
}

// Stalled read-only host I/O mustn't prevent owned-container cleanup. A timed-out
// worker may finish later; its discarded result can't authorize execution.
fn bounded_io<T: Send + 'static>(
    deadline: Instant,
    cancelled: &dyn Fn() -> bool,
    operation: impl FnOnce(Arc<AtomicBool>) -> Checked<T> + Send + 'static,
) -> Checked<T> {
    check_budget(deadline, cancelled)?;
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = stop.clone();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let _ = sender.send(operation(worker_stop));
    });
    let result = loop {
        if let Err(error) = check_budget(deadline, cancelled) {
            break Err(error);
        }
        match receiver.recv_timeout(Duration::from_millis(5)) {
            Ok(result) => break check_budget(deadline, cancelled).and(result),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => (),
            Err(_) => {
                break Err(
                    "sandbox.evidence_worker: host reader exited without an observation".into(),
                );
            }
        }
    };
    stop.store(true, Ordering::Release);
    result
}

fn hash_executable(
    path: &Path,
    deadline: Instant,
    cancelled: &dyn Fn() -> bool,
) -> Checked<String> {
    let path = path.to_owned();
    bounded_io(deadline, cancelled, move |stop| {
        hash_executable_inner(&path, deadline, &|| stop.load(Ordering::Acquire))
    })
}

fn hash_executable_inner(
    path: &Path,
    deadline: Instant,
    cancelled: &dyn Fn() -> bool,
) -> Checked<String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.len() > 128 * 1024 * 1024 {
        return Err("sandbox.file: expected bounded regular executable".into());
    }
    let mut file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut bytes = [0u8; 65536];
    let mut total = 0;
    loop {
        check_budget(deadline, cancelled)?;
        let count = file.read(&mut bytes).map_err(|error| error.to_string())?;
        total += count;
        if total > 128 * 1024 * 1024 {
            return Err("sandbox.file: executable exceeds its byte limit".into());
        }
        hasher.update(&bytes[..count]);
        if count == 0 {
            break;
        }
    }
    check_budget(deadline, cancelled)?;
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn read_file(
    path: &Path,
    maximum: usize,
    deadline: Instant,
    cancelled: &dyn Fn() -> bool,
) -> Checked<Vec<u8>> {
    check_budget(deadline, cancelled)?;
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.len() > maximum as u64 {
        return Err("sandbox.file: expected bounded regular file".into());
    }
    let mut bytes = Vec::new();
    let mut file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut chunk = [0u8; 65536];
    loop {
        check_budget(deadline, cancelled)?;
        let count = file.read(&mut chunk).map_err(|error| error.to_string())?;
        check_budget(deadline, cancelled)?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.len() > maximum {
            break;
        }
    }
    if bytes.len() > maximum {
        return Err("sandbox.file: output exceeds its retained byte limit".into());
    }
    Ok(bytes)
}

fn artifacts(
    request: &SandboxRequest,
    deadline: Instant,
    cancelled: &dyn Fn() -> bool,
) -> Checked<Vec<SandboxArtifact>> {
    let request = request.clone();
    bounded_io(deadline, cancelled, move |stop| {
        collect_artifacts(&request, deadline, &|| stop.load(Ordering::Acquire))
    })
}

fn collect_artifacts(
    request: &SandboxRequest,
    deadline: Instant,
    cancelled: &dyn Fn() -> bool,
) -> Checked<Vec<SandboxArtifact>> {
    let mut remaining = request.outputs.max_total_bytes as usize;
    let mut artifacts = Vec::new();
    for selected in &request.outputs.paths {
        check_budget(deadline, cancelled)?;
        let mount = request
            .mounts
            .iter()
            .find(|mount| mount.writable && Path::new(selected).starts_with(&mount.destination))
            .ok_or("sandbox.outputs: output isn't writable")?;
        let relative = Path::new(selected)
            .strip_prefix(&mount.destination)
            .map_err(|error| error.to_string())?;
        let host = if relative.as_os_str().is_empty() {
            PathBuf::from(&mount.source)
        } else {
            Path::new(&mount.source).join(relative)
        };
        if fs::canonicalize(&host).map_err(|error| error.to_string())? != host {
            return Err("sandbox.outputs: output traverses a symlink".into());
        }
        let bytes = read_file(&host, remaining, deadline, cancelled)?;
        remaining -= bytes.len();
        artifacts.push(SandboxArtifact {
            path: selected.clone(),
            digest: digest(&bytes),
            bytes,
        });
    }
    check_budget(deadline, cancelled)?;
    Ok(artifacts)
}

impl SandboxAdapter for LocalOciAdapter {
    fn execute(
        &mut self,
        request: &SandboxRequest,
        cancelled: &dyn Fn() -> bool,
    ) -> SandboxObservation {
        let started = Instant::now();
        let mut observation = SandboxObservation {
            schema_version: "gaap.sandbox-observation/0.1".into(),
            request_digest: canonical(request)
                .map(|bytes| digest(&bytes))
                .unwrap_or_default(),
            runtime: None,
            image_digest: None,
            resolved_image_id: None,
            owned_resource: None,
            intent_path: None,
            status: SandboxStatus::SetupFailed,
            exit_code: None,
            stdout: Vec::new(),
            stderr: Vec::new(),
            stdout_digest: digest(&[]),
            stderr_digest: digest(&[]),
            artifacts: Vec::new(),
            elapsed_ms: 0,
            cleanup: SandboxCleanup::NotCreated,
            reason: None,
            cleanup_reason: None,
            publication_error: None,
        };
        let result = (|| -> Checked<()> {
            // Clamp before reading untrusted bounds; validation rejects out-of-range requests.
            let deadline =
                started + Duration::from_millis(request.limits.wall_clock_ms.min(300000));
            if cancelled() {
                observation.status = SandboxStatus::Cancelled;
                return Ok(());
            }
            let (runtime, image_id) = self.preflight(request, deadline, cancelled)?;
            observation.runtime = Some(runtime);
            observation.image_digest = Some(request.image.digest.clone());
            observation.resolved_image_id = Some(image_id.clone());
            let mut nonce = [0u8; 16];
            fs::File::open("/dev/urandom")
                .and_then(|mut source| source.read_exact(&mut nonce))
                .map_err(|error| error.to_string())?;
            let owner = nonce
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            let name = format!("gaap-{owner}");
            observation.owned_resource = Some(name.clone());
            let intent = canonical(&serde_json::json!({
                "schema_version":"gaap.sandbox-intent/0.1", "owned_resource":name, "owner":owner,
                "request_digest":observation.request_digest, "invocation_kind":request.invocation_kind,"invocation_digest":request.invocation_digest,
                "runtime":observation.runtime, "image_digest":request.image.digest,
            }))?;
            let intent_path = self
                .config
                .state_directory
                .join(format!("{name}.intent.json"));
            observation.intent_path = Some(intent_path.to_string_lossy().into_owned());
            write_record(
                &self.config.state_directory,
                &format!("{name}.intent.json"),
                &intent,
            )?;
            observation.cleanup = SandboxCleanup::Unconfirmed;
            let run = (|| -> Checked<()> {
                let arguments = self.create_arguments(request, &name, &owner);
                let created = self.process(&arguments, deadline, 1024, 65536, cancelled)?;
                if created.code != Some(0) || created.stopped.is_some() {
                    observation.reason = Some(format!(
                        "sandbox.create_failed: {}",
                        String::from_utf8_lossy(&created.stderr)
                    ));
                    if let Some(status) = created.stopped {
                        observation.status = status;
                    }
                    return Ok(());
                }
                let created = self.inspect(&name, deadline, cancelled)?;
                self.validate_container(&created, request, &owner, &image_id)?;
                if cancelled() {
                    observation.status = SandboxStatus::Cancelled;
                    return Ok(());
                }
                // A lost start response is an uncertain effect, even if the CLI itself failed.
                observation.status = SandboxStatus::UnknownOutcome;
                let output = self.process(
                    &[
                        "container".into(),
                        "start".into(),
                        "--attach".into(),
                        name.clone(),
                    ],
                    deadline,
                    request.limits.stdout_bytes as usize,
                    request.limits.stderr_bytes as usize,
                    cancelled,
                )?;
                observation.stdout = output.stdout;
                observation.stderr = output.stderr;
                if let Some(status) = output.stopped {
                    observation.status = status;
                    return Ok(());
                }
                let inspected = self.inspect(&name, deadline, cancelled)?;
                self.validate_container(&inspected, request, &owner, &image_id)?;
                let state = &inspected["State"];
                if state["Running"] != false || state["Status"] != "exited" {
                    return Err("sandbox.exit: terminal workload status wasn't observed".into());
                }
                observation.exit_code = state["ExitCode"].as_i64();
                let file_limit_signal = observation.exit_code == Some(128 + 25);
                let oom_event = if state["OOMKilled"] != true && observation.exit_code == Some(137)
                {
                    match inspected["Id"].as_str() {
                        Some(id) => self.oom_event(&name, &owner, id, deadline, cancelled)?,
                        None => None,
                    }
                } else {
                    None
                };
                observation.status = if state["OOMKilled"] == true || oom_event.is_some() {
                    observation.reason = Some(match oom_event {
                        Some(event) => {
                            format!("sandbox.resource_limit: Docker reported an OOM event {event}")
                        }
                        None => "sandbox.resource_limit: Docker reports OOMKilled".into(),
                    });
                    SandboxStatus::ResourceLimit
                } else if file_limit_signal {
                    // OCI reports an exit code, so this is signal-compatible evidence,
                    // not a claim that an arbitrary program couldn't exit with 153 itself.
                    observation.reason = Some("sandbox.resource_limit: exit is consistent with SIGXFSZ under the configured file-size bound".into());
                    SandboxStatus::ResourceLimit
                } else if state["Error"]
                    .as_str()
                    .is_some_and(|error| !error.is_empty())
                {
                    SandboxStatus::RuntimeCrash
                } else {
                    if observation.exit_code == Some(137) {
                        observation.reason = Some("sandbox.resource_observation: exit 137; Docker hasn't confirmed an OOM cause".into());
                    }
                    SandboxStatus::Exited
                };
                if output.code
                    != observation
                        .exit_code
                        .and_then(|code| i32::try_from(code).ok())
                {
                    observation.status = SandboxStatus::RuntimeCrash;
                    observation.reason =
                        Some("sandbox.exit: attached CLI and workload exits disagree".into());
                }
                observation.artifacts = artifacts(request, deadline, cancelled)?;
                let current_executable =
                    hash_executable(&self.config.executable, deadline, cancelled)?;
                if observation
                    .runtime
                    .as_ref()
                    .is_some_and(|runtime| current_executable != runtime.executable_digest)
                {
                    observation.status = SandboxStatus::UnknownOutcome;
                    return Err("sandbox.runtime: executable changed during execution".into());
                }
                check_budget(deadline, cancelled)?;
                Ok(())
            })();
            let cleanup_deadline =
                Instant::now() + Duration::from_millis(request.limits.cleanup_ms.min(10000));
            match self.cleanup(&name, &owner, cleanup_deadline) {
                Ok(()) => observation.cleanup = SandboxCleanup::Removed,
                Err(error) => observation.cleanup_reason = Some(error),
            }
            run
        })();
        if let Err(error) = result {
            if error.starts_with("sandbox.control_cancelled") {
                observation.status = SandboxStatus::Cancelled;
            } else if error.starts_with("sandbox.control_timeout") {
                observation.status = SandboxStatus::TimedOut;
            } else if observation.status == SandboxStatus::Exited {
                observation.status = SandboxStatus::UnknownOutcome;
            }
            observation.reason = Some(error);
        }
        observation.elapsed_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        observation.stdout_digest = digest(&observation.stdout);
        observation.stderr_digest = digest(&observation.stderr);
        if let Some(name) = &observation.owned_resource {
            let publication = canonical(&observation).and_then(|bytes| {
                write_record(
                    &self.config.state_directory,
                    &format!("{name}.observation.json"),
                    &bytes,
                )
            });
            if let Err(error) = publication {
                observation.publication_error = Some(error);
            }
        }
        observation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn control_adapter() -> LocalOciAdapter {
        LocalOciAdapter::new(OciConfig {
            executable: fs::canonicalize("/bin/sh").unwrap(),
            environment: BTreeMap::new(),
            state_directory: PathBuf::new(),
            approved_profile_digest: String::new(),
            engine_version: String::new(),
            runtime: "runc".into(),
            runtime_version: String::new(),
        })
    }

    #[test]
    fn inherited_pipes_cant_outlive_the_control_deadline() {
        let started = Instant::now();
        let result = control_adapter()
            .process(
                &[
                    "-c".into(),
                    "/bin/sleep 30 & printf retained; exit 0".into(),
                ],
                started + Duration::from_millis(200),
                1024,
                1024,
                &|| false,
            )
            .unwrap();
        assert_eq!(result.code, Some(0));
        assert_eq!(result.stopped, Some(SandboxStatus::TimedOut));
        assert_eq!(result.stdout, b"retained");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn cancellation_interrupts_a_control_call_and_its_descendants() {
        let started = Instant::now();
        let result = control_adapter()
            .process(
                &["-c".into(), "/bin/sleep 30 & wait".into()],
                started + Duration::from_secs(10),
                1024,
                1024,
                &|| started.elapsed() >= Duration::from_millis(100),
            )
            .unwrap();
        assert_eq!(result.stopped, Some(SandboxStatus::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn stalled_host_reads_leave_time_for_independent_cleanup() {
        let started = Instant::now();
        let finished = Arc::new(AtomicBool::new(false));
        let worker_finished = finished.clone();
        let result: Checked<()> =
            bounded_io(started + Duration::from_millis(30), &|| false, move |_| {
                // Models a filesystem syscall that hasn't returned to its chunk loop.
                std::thread::sleep(Duration::from_secs(2));
                worker_finished.store(true, Ordering::Release);
                Ok(())
            });
        assert!(result.unwrap_err().starts_with("sandbox.control_timeout"));
        assert!(!finished.load(Ordering::Acquire));
        assert!(started.elapsed() < Duration::from_secs(1));
        let cleanup = control_adapter()
            .process(
                &["-c".into(), "exit 0".into()],
                Instant::now() + Duration::from_secs(1),
                1024,
                1024,
                &|| false,
            )
            .unwrap();
        assert_eq!(cleanup.code, Some(0));
    }

    #[test]
    fn executable_hashing_obeys_cancellation_between_chunks() {
        let executable = fs::canonicalize("/bin/sh").unwrap();
        let checks = std::cell::Cell::new(0);
        let result = hash_executable(
            &executable,
            Instant::now() + Duration::from_secs(5),
            &|| {
                checks.set(checks.get() + 1);
                checks.get() >= 2
            },
        );
        assert!(result.unwrap_err().starts_with("sandbox.control_cancelled"));
        assert!(checks.get() >= 2);
        let result = hash_executable(&executable, Instant::now(), &|| false);
        assert!(result.unwrap_err().starts_with("sandbox.control_timeout"));
        let result = hash_executable(
            &executable,
            Instant::now() + Duration::from_secs(5),
            &|| false,
        )
        .unwrap();
        assert_eq!(result, digest(&fs::read(executable).unwrap()));
    }

    #[test]
    fn mount_paths_require_canonical_container_spelling() {
        assert!(absolute("/workspace/input"));
        for path in [
            "/workspace/",
            "/workspace//input",
            "/workspace/./input",
            "/workspace/../input",
        ] {
            assert!(!absolute(path), "{path}");
        }
    }
}
