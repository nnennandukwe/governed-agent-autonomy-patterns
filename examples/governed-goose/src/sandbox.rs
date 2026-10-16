//! Host-owned composition of the neutral OCI adapter with the reference MCP backend.
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use gaap::contracts::{ProtectedEffectRequest, SandboxProfileIdentity, Subject};
use gaap::sandbox::{
    SandboxAdapter, SandboxCleanup, SandboxImage, SandboxInvocationKind, SandboxLimits,
    SandboxMount, SandboxNetwork, SandboxObservation, SandboxOutputs, SandboxRequest,
    SandboxStatus,
};
use gaap_sandbox::{LocalOciAdapter, OciConfig};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Result, hash};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceSandbox {
    pub executable: PathBuf,
    pub docker_executable: PathBuf,
    pub docker_environment: BTreeMap<String, String>,
    pub engine_version: String,
    pub runtime_version: String,
    pub image: SandboxImage,
    pub user: String,
    pub state_directory: PathBuf,
}

pub struct SandboxHost {
    config: ReferenceSandbox,
    root: PathBuf,
    pub profile: SandboxProfileIdentity,
    pub manifest: Value,
}

pub struct PreparedSandbox {
    request: SandboxRequest,
    observation: PathBuf,
}

fn write_new(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(&serde_jcs::to_vec(value)?)?;
    file.sync_all()?;
    Ok(())
}

impl SandboxHost {
    pub fn new(config: ReferenceSandbox, root: PathBuf, subject: &Subject) -> Result<Self> {
        if fs::metadata(&config.state_directory)?.permissions().mode() & 0o077 != 0
            || fs::canonicalize(&config.docker_executable)? != config.docker_executable
            || fs::canonicalize(&config.state_directory)? != config.state_directory
            || config.state_directory.starts_with(&root)
            || root.starts_with(&config.state_directory)
            || fs::canonicalize(&config.executable)? != config.executable
        {
            return Err("Select fresh private sandbox state outside the subject and an absolute adapter executable".into());
        }
        let mut host = Self {
            config,
            root,
            profile: SandboxProfileIdentity {
                name: "governed-goose-local-oci".into(),
                version: "0.1".into(),
                digest: String::new(),
            },
            manifest: Value::Null,
        };
        let mut profiles = BTreeMap::new();
        for operation in ["file_write", "terminal_echo", "mcp_echo", "verify"] {
            profiles.insert(
                operation,
                host.request(
                    subject.clone(),
                    format!("sha256:{}", "0".repeat(64)),
                    operation,
                    json!({"operation":operation,"content":""}),
                )?
                .profile_digest()?,
            );
        }
        host.manifest = json!({"domain":"gaap.governed-goose-oci/0.1","profiles":profiles,"adapter_binary":super::raw_hash(&fs::read(&host.config.executable)?),"docker_binary":super::raw_hash(&fs::read(&host.config.docker_executable)?),"operation_program":super::raw_hash(include_bytes!("../oci_operation.py")),"verifier_program":super::raw_hash(include_bytes!("../verify.py")),"transport":"trusted-host-MCP-stdio","isolation_claim":"local OCI reproducibility boundary"});
        host.manifest["engine_version"] = json!(host.config.engine_version);
        host.manifest["runtime"] = json!({"name":"runc","version":host.config.runtime_version});
        host.profile.digest = hash(&host.manifest);
        let profile_path = host.config.state_directory.join("profile.json");
        let bytes = serde_jcs::to_vec(&host.manifest)?;
        if profile_path.exists() {
            if fs::read(&profile_path)? != bytes {
                return Err("Retained sandbox profile differs from current host adoption".into());
            }
        } else {
            write_new(&profile_path, &host.manifest)?;
        }
        Ok(host)
    }

    pub fn ensure_fresh_run(&self) -> Result<()> {
        if self.context_path().exists() {
            return Err("Select fresh private sandbox state for a new run; retained invocation evidence can't authorize replay".into());
        }
        Ok(())
    }

    pub fn context_path(&self) -> PathBuf {
        self.config.state_directory.join("current-effect.json")
    }
    pub fn executable(&self) -> &Path {
        &self.config.executable
    }

    pub fn profile_artifact(&self) -> gaap_mcp::LaunchArtifact {
        gaap_mcp::LaunchArtifact {
            path: self.config.state_directory.join("profile.json"),
            digest: self.profile.digest.clone(),
        }
    }

    pub fn docker_artifact(&self) -> Result<gaap_mcp::LaunchArtifact> {
        Ok(gaap_mcp::LaunchArtifact {
            path: self.config.docker_executable.clone(),
            digest: super::raw_hash(&fs::read(&self.config.docker_executable)?),
        })
    }

    fn request(
        &self,
        subject: Subject,
        effect_digest: String,
        operation: &str,
        arguments: Value,
    ) -> Result<SandboxRequest> {
        let file = match operation {
            "file_write" => "shipping.py",
            "terminal_echo" => "terminal.txt",
            "mcp_echo" => "mcp.txt",
            "verify" => "shipping.py",
            _ => return Err("Unsupported sandbox operation".into()),
        };
        let source = self.root.join(file);
        if !fs::symlink_metadata(&source)?.is_file() || fs::canonicalize(&source)? != source {
            return Err("The sandbox profile requires its predeclared regular subject file".into());
        }
        let program = if operation == "verify" {
            format!(
                "import io,json,pathlib,sys\nsys.stdin=io.StringIO(json.dumps({{'source':pathlib.Path('/workspace/shipping.py').read_text(),'inputs':[[1,20],[4,20],[5,20],[5,25],[1,100],[6,19.99],[0,20],[-1,20]]}}))\n{}",
                include_str!("../verify.py")
            )
        } else {
            include_str!("../oci_operation.py").into()
        };
        let mut command = vec![
            "/usr/local/bin/python3".into(),
            "-I".into(),
            "-S".into(),
            "-B".into(),
            "-c".into(),
            program,
        ];
        if operation != "verify" {
            command.push(serde_json::to_string(&arguments)?);
        }
        Ok(SandboxRequest {
            schema_version: "gaap.sandbox/0.1".into(),
            profile_id: "local-oci/0.1".into(),
            invocation_kind: if operation == "verify" {
                SandboxInvocationKind::Verification
            } else {
                SandboxInvocationKind::ProtectedEffect
            },
            invocation_digest: effect_digest,
            image: self.config.image.clone(),
            command,
            working_directory: "/workspace".into(),
            user: self.config.user.clone(),
            environment: BTreeMap::new(),
            mounts: vec![SandboxMount {
                source: source.to_str().ok_or("Non-UTF8 subject path")?.into(),
                destination: format!("/workspace/{file}"),
                writable: operation != "verify",
            }],
            network: SandboxNetwork::Disabled,
            limits: SandboxLimits {
                cpu_millis_per_second: 1000,
                memory_bytes: 64 * 1024 * 1024,
                processes: 16,
                stdout_bytes: 32768,
                stderr_bytes: 8192,
                wall_clock_ms: 10000,
                cleanup_ms: 2000,
            },
            input_subject: subject,
            outputs: SandboxOutputs {
                paths: if operation == "verify" {
                    vec![]
                } else {
                    vec![format!("/workspace/{file}")]
                },
                max_total_bytes: 8192,
            },
        })
    }

    fn adopted(&self, request: &SandboxRequest) -> Result<OciConfig> {
        Ok(OciConfig {
            executable: self.config.docker_executable.clone(),
            environment: self.config.docker_environment.clone(),
            state_directory: self.config.state_directory.clone(),
            approved_profile_digest: request.profile_digest()?,
            engine_version: self.config.engine_version.clone(),
            runtime: "runc".into(),
            runtime_version: self.config.runtime_version.clone(),
        })
    }

    pub fn prepare(
        &self,
        effect: &ProtectedEffectRequest,
        arguments: Value,
    ) -> Result<PreparedSandbox> {
        let operation = arguments["operation"]
            .as_str()
            .ok_or("Missing bounded operation")?;
        if operation == "verify" {
            return Err("Verification isn't an agent operation".into());
        }
        let request = self.request(
            effect.subject.clone(),
            hash(effect),
            operation,
            arguments.clone(),
        )?;
        let key = hash(effect);
        let key = &key[7..];
        let config_path = self.config.state_directory.join(format!("{key}.host.json"));
        let observation = self
            .config
            .state_directory
            .join(format!("{key}.effect.json"));
        write_new(&config_path, &self.adopted(&request)?)?;
        let context = json!({"arguments":arguments,"request":request,"executable":self.config.executable,"config":config_path,"observation":observation});
        let staged = self
            .config
            .state_directory
            .join(format!("{key}.context.json"));
        write_new(&staged, &context)?;
        fs::rename(staged, self.context_path())?;
        fs::File::open(&self.config.state_directory)?.sync_all()?;
        Ok(PreparedSandbox {
            request,
            observation,
        })
    }

    pub fn observe(&self, prepared: &PreparedSandbox) -> Result<SandboxObservation> {
        let bytes = fs::read(&prepared.observation)?;
        if bytes.len() > 1024 * 1024 {
            return Err("Oversized sandbox observation".into());
        }
        let observation: SandboxObservation = serde_json::from_slice(&bytes)?;
        self.check(&prepared.request, &observation)?;
        Ok(observation)
    }

    fn check(&self, request: &SandboxRequest, observation: &SandboxObservation) -> Result<()> {
        if observation.schema_version != "gaap.sandbox-observation/0.1"
            || observation.request_digest != hash(request)
            || observation.stdout_digest != super::raw_hash(&observation.stdout)
            || observation.stderr_digest != super::raw_hash(&observation.stderr)
            || observation
                .artifacts
                .iter()
                .any(|artifact| artifact.digest != super::raw_hash(&artifact.bytes))
        {
            return Err("Sandbox evidence doesn't bind the exact effect, profile and bytes".into());
        }
        if let Some(runtime) = &observation.runtime {
            if runtime.runtime != "runc"
                || runtime.engine_version != self.config.engine_version
                || runtime.runtime_version != self.config.runtime_version
                || self.manifest["docker_binary"].as_str()
                    != Some(runtime.executable_digest.as_str())
                || observation.image_digest.as_ref() != Some(&self.config.image.digest)
            {
                return Err("Actual OCI runtime or image differs from host adoption".into());
            }
        } else if !matches!(
            observation.status,
            SandboxStatus::SetupFailed | SandboxStatus::Cancelled | SandboxStatus::TimedOut
        ) {
            return Err("Execution has no observed OCI identity".into());
        }
        Ok(())
    }

    pub fn verify(&self, subject: Subject, request_digest: String) -> Result<SandboxObservation> {
        let request = self.request(subject, request_digest, "verify", Value::Null)?;
        let observation =
            LocalOciAdapter::new(self.adopted(&request)?).execute(&request, &|| false);
        self.check(&request, &observation)?;
        Ok(observation)
    }
}

pub fn succeeded(observation: &SandboxObservation) -> bool {
    observation.status == SandboxStatus::Exited
        && observation.exit_code == Some(0)
        && observation.cleanup == SandboxCleanup::Removed
        && observation.publication_error.is_none()
}
