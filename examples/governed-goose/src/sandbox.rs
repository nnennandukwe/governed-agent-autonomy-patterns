//! Host-owned composition of the neutral OCI adapter with the reference MCP backend.
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use gaap::contracts::{ProtectedEffectRequest, SandboxProfileIdentity, Subject};
use gaap::sandbox::{
    SandboxAdapter, SandboxCleanup, SandboxImage, SandboxInvocationKind, SandboxLimits,
    SandboxMount, SandboxNetwork, SandboxObservation, SandboxOutputs, SandboxRequest,
    SandboxStatus,
};
use gaap_sandbox::{GVISOR_FLAGS, GVISOR_PROFILE, GvisorConfig, LocalOciAdapter, OciConfig};
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
    #[serde(default)]
    pub gvisor: Option<GvisorConfig>,
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
    transport: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SandboxTransport {
    schema_version: String,
    request_digest: String,
    exit_code: Option<i32>,
    termination: Option<String>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    stdout_truncated: bool,
    stderr_truncated: bool,
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
        if host.config.gvisor.is_some() {
            host.profile.name = "governed-goose-gvisor".into();
        }
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
        host.manifest["engine"] = json!("docker-compatible");
        host.manifest["engine_version"] = json!(host.config.engine_version);
        host.manifest["runtime"] =
            json!({"name":host.runtime_name(),"version":host.config.runtime_version});
        if let Some(config) = &host.config.gvisor {
            host.manifest["domain"] = json!("gaap.governed-goose-gvisor/0.1");
            host.manifest["gvisor"] = json!(config);
            host.manifest["flags"] = json!(GVISOR_FLAGS);
            host.manifest["isolation_claim"] =
                json!("Linux gVisor workload isolation; trusted host and runtime");
        }
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

    fn runtime_name(&self) -> &str {
        if self.config.gvisor.is_some() {
            "runsc"
        } else {
            "runc"
        }
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
            profile_id: if self.config.gvisor.is_some() {
                GVISOR_PROFILE
            } else {
                "local-oci/0.1"
            }
            .into(),
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
            gvisor: self.config.gvisor.clone(),
            executable: self.config.docker_executable.clone(),
            environment: self.config.docker_environment.clone(),
            state_directory: self.config.state_directory.clone(),
            approved_profile_digest: request.profile_digest()?,
            engine_version: self.config.engine_version.clone(),
            runtime: self.runtime_name().into(),
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
        let transport = self
            .config
            .state_directory
            .join(format!("{key}.transport.json"));
        let context = json!({"arguments":arguments,"request":request,"request_digest":hash(&request),"executable":self.config.executable,"config":config_path,"observation":observation,"transport":transport});
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
            transport,
        })
    }

    pub fn transport(&self, prepared: &PreparedSandbox) -> Result<Vec<Vec<u8>>> {
        let file = fs::File::open(&prepared.transport)?;
        let mut bytes = Vec::new();
        file.take(6 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > 6 * 1024 * 1024 {
            return Err("Oversized sandbox transport evidence".into());
        }
        let transport: SandboxTransport = serde_json::from_slice(&bytes)?;
        if transport.schema_version != "gaap.sandbox-transport/0.1"
            || transport.request_digest != hash(&prepared.request)
            || transport.stdout.len() > 1024 * 1024
            || transport.stderr.len() > 8192
            || transport
                .termination
                .as_ref()
                .is_some_and(|reason| reason.len() > 512)
        {
            return Err("Sandbox transport doesn't bind the admitted request and bounds".into());
        }
        // Keep each raw stream within the attestation adapter's 1 MiB blob bound.
        let metadata = serde_jcs::to_vec(&json!({
            "schema_version":transport.schema_version,"request_digest":transport.request_digest,
            "exit_code":transport.exit_code,"termination":transport.termination,
            "stdout_digest":super::raw_hash(&transport.stdout),"stderr_digest":super::raw_hash(&transport.stderr),
            "stdout_truncated":transport.stdout_truncated,"stderr_truncated":transport.stderr_truncated,
        }))?;
        let mut evidence = vec![transport.stdout, transport.stderr, metadata];
        if self.config.gvisor.is_some()
            && let Some(observation) = fs::read(&prepared.observation)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<SandboxObservation>(&bytes).ok())
        {
            evidence.extend(self.runtime_records(&observation).0);
        }
        Ok(evidence)
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
            if self.manifest["engine"].as_str() != Some(runtime.engine.as_str())
                || runtime.runtime != self.runtime_name()
                || runtime.engine_version != self.config.engine_version
                || runtime.runtime_version != self.config.runtime_version
                || Some(runtime.executable_digest.as_str())
                    != self
                        .config
                        .gvisor
                        .as_ref()
                        .map(|config| config.executable_digest.as_str())
                        .or_else(|| self.manifest["docker_binary"].as_str())
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

    pub fn runtime_records(
        &self,
        observation: &SandboxObservation,
    ) -> (Vec<Vec<u8>>, Option<String>) {
        if self.config.gvisor.is_none() || observation.owned_resource.is_none() {
            return (vec![], None);
        }
        let resource = observation.owned_resource.as_deref().unwrap();
        let mut records = Vec::new();
        let mut errors = Vec::new();
        if resource.len() != 37
            || !resource.starts_with("gaap-")
            || !resource[5..].bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            errors.push("Invalid owned sandbox resource identity".to_owned());
        } else {
            for suffix in ["intent", "container", "runtime"] {
                let path = self
                    .config
                    .state_directory
                    .join(format!("{resource}.{suffix}.json"));
                let result = (|| -> Result<()> {
                    if !path.exists() && !succeeded(observation) {
                        return Ok(());
                    }
                    let mut bytes = Vec::new();
                    fs::File::open(path)?
                        .take(1024 * 1024 + 1)
                        .read_to_end(&mut bytes)?;
                    if bytes.len() > 1024 * 1024 {
                        return Err("Oversized sandbox runtime evidence".into());
                    }
                    // Original bounded bytes survive even when parsing or binding fails.
                    records.push(bytes.clone());
                    let value: Value = serde_json::from_slice(&bytes)?;
                    if value["request_digest"].as_str() != Some(&observation.request_digest) {
                        return Err("Sandbox runtime evidence doesn't bind this request".into());
                    }
                    Ok(())
                })();
                if let Err(error) = result {
                    errors.push(format!("{suffix}: {error}"));
                }
            }
        }
        let rejection = if errors.is_empty() {
            None
        } else {
            Some(errors.join("; "))
        };
        if let Some(reason) = &rejection {
            records.push(
                serde_jcs::to_vec(&json!({
                    "schema_version":"gaap.runsc-evidence-missing/0.1",
                    "request_digest":observation.request_digest,"reason":reason,
                }))
                .expect("JSON evidence serializes"),
            );
        }
        (records, rejection)
    }

    pub fn verify(
        &self,
        subject: Subject,
        request_digest: String,
    ) -> Result<(SandboxObservation, Option<String>)> {
        let request = self.request(subject, request_digest, "verify", Value::Null)?;
        let observation =
            LocalOciAdapter::new(self.adopted(&request)?).execute(&request, &|| false);
        let rejection = self
            .check(&request, &observation)
            .err()
            .map(|error| error.to_string());
        Ok((observation, rejection))
    }
}

pub fn succeeded(observation: &SandboxObservation) -> bool {
    observation.status == SandboxStatus::Exited
        && observation.exit_code == Some(0)
        && observation.cleanup == SandboxCleanup::Removed
        && observation.publication_error.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_observation_cant_substitute_another_engine_name() {
        let digest = format!("sha256:{}", "0".repeat(64));
        let request: SandboxRequest = serde_json::from_value(json!({
            "schema_version":"gaap.sandbox/0.1", "profile_id":"local-oci/0.1",
            "invocation_kind":"protected_effect", "invocation_digest":digest,
            "image":{"reference":format!("python@{digest}"),"digest":digest},
            "command":["/usr/local/bin/python3"],"working_directory":"/workspace","user":"1000:1000",
            "environment":{},"mounts":[{"source":"/subject/shipping.py","destination":"/workspace/shipping.py","writable":true}],
            "network":{"mode":"disabled"},
            "limits":{"cpu_millis_per_second":1000,"memory_bytes":67108864,"processes":16,"stdout_bytes":32768,"stderr_bytes":8192,"wall_clock_ms":10000,"cleanup_ms":2000},
            "input_subject":{"kind":"repository","locator":"/subject","digest":digest},
            "outputs":{"paths":["/workspace/shipping.py"],"max_total_bytes":8192},
        })).unwrap();
        let host = SandboxHost {
            config: ReferenceSandbox {
                executable: "/adapter".into(),
                docker_executable: "/docker".into(),
                docker_environment: BTreeMap::new(),
                engine_version: "expected".into(),
                runtime_version: "expected".into(),
                gvisor: None,
                image: request.image.clone(),
                user: "1000:1000".into(),
                state_directory: "/state".into(),
            },
            root: "/subject".into(),
            profile: SandboxProfileIdentity {
                name: "test".into(),
                version: "0.1".into(),
                digest: digest.clone(),
            },
            manifest: json!({"docker_binary":digest,"engine":"docker-compatible"}),
        };
        let mut observation: SandboxObservation = serde_json::from_value(json!({
            "schema_version":"gaap.sandbox-observation/0.1","request_digest":hash(&request),
            "runtime":{"engine":"docker-compatible","engine_version":"expected","runtime":"runc","runtime_version":"expected","executable_digest":digest},
            "image_digest":digest,"resolved_image_id":digest,"owned_resource":"owned","intent_path":"/state/intent",
            "status":"exited","exit_code":0,"stdout":[],"stderr":[],"stdout_digest":super::super::raw_hash(b""),"stderr_digest":super::super::raw_hash(b""),
            "artifacts":[],"elapsed_ms":1,"cleanup":"removed","reason":null,"cleanup_reason":null,"publication_error":null,
        })).unwrap();
        host.check(&request, &observation).unwrap();
        observation.runtime.as_mut().unwrap().engine = "another-engine".into();
        assert!(host.check(&request, &observation).is_err());
    }
    #[test]
    fn malformed_runtime_records_retain_prior_bytes_and_the_original_observation() {
        let state = std::env::temp_dir().join(format!(
            "gaap-runsc-evidence-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&state).unwrap();
        let digest = super::super::raw_hash(b"");
        let resource = format!("gaap-{}", "a".repeat(32));
        let observation: SandboxObservation = serde_json::from_value(json!({
            "schema_version":"gaap.sandbox-observation/0.1", "request_digest":digest,
            "runtime":null,"image_digest":null,"resolved_image_id":null,"owned_resource":resource,"intent_path":null,
            "status":"exited","exit_code":0,"stdout":[],"stderr":[],"stdout_digest":digest,"stderr_digest":digest,
            "artifacts":[],"elapsed_ms":1,"cleanup":"removed","reason":null,"cleanup_reason":null,"publication_error":null,
        })).unwrap();
        let host = SandboxHost {
            config: ReferenceSandbox {
                executable: "/adapter".into(),
                docker_executable: "/docker".into(),
                docker_environment: BTreeMap::new(),
                engine_version: "test".into(),
                runtime_version: "test".into(),
                gvisor: Some(GvisorConfig {
                    executable: "/runsc".into(),
                    executable_digest: digest.clone(),
                }),
                image: SandboxImage {
                    reference: format!("python@{digest}"),
                    digest: digest.clone(),
                },
                user: "1000:1000".into(),
                state_directory: state.clone(),
            },
            root: "/subject".into(),
            profile: SandboxProfileIdentity {
                name: "test".into(),
                version: "0.1".into(),
                digest: digest.clone(),
            },
            manifest: Value::Null,
        };
        let original = serde_jcs::to_vec(&observation).unwrap();
        let valid =
            serde_jcs::to_vec(&json!({"request_digest":digest,"original":"intent"})).unwrap();
        for broken in ["container", "runtime"] {
            for suffix in ["intent", "container", "runtime"] {
                fs::write(
                    state.join(format!("{resource}.{suffix}.json")),
                    if suffix == broken {
                        b"malformed original bytes".as_slice()
                    } else {
                        valid.as_slice()
                    },
                )
                .unwrap();
            }
            let (records, rejection) = host.runtime_records(&observation);
            assert!(rejection.unwrap().contains(broken));
            assert!(records.contains(&valid));
            assert!(records.contains(&b"malformed original bytes".to_vec()));
            assert_eq!(original, serde_jcs::to_vec(&observation).unwrap());
        }
        fs::remove_dir_all(state).unwrap();
    }
}
