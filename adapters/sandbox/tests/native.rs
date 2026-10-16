//! Real Docker boundary tests. Run explicitly; missing runtime configuration fails the suite.
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gaap::sandbox::{
    SandboxAdapter, SandboxCleanup, SandboxImage, SandboxInvocationKind, SandboxLimits,
    SandboxMount, SandboxNetwork, SandboxOutputs, SandboxRequest, SandboxStatus,
};
use gaap_sandbox::{
    GVISOR_FLAGS, GVISOR_PROFILE, GvisorConfig, LocalOciAdapter, OciConfig, digest,
};
use serde_json::Value;

const IMAGE: &str = "sha256:2d9aefe2fef018a7eb2c13064c89c71929800fd2e5dccdbf52ea5da5bb8d929a";
struct Fixture {
    root: PathBuf,
    config: OciConfig,
    docker: PathBuf,
    request: SandboxRequest,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(directory) = std::env::var_os("GAAP_SANDBOX_EVIDENCE") {
            let destination = PathBuf::from(directory).join(self.root.file_name().unwrap());
            fs::create_dir_all(&destination).unwrap();
            fs::write(
                destination.join("request.json"),
                serde_jcs::to_vec(&self.request).unwrap(),
            )
            .unwrap();
            for entry in fs::read_dir(&self.config.state_directory)
                .unwrap()
                .flatten()
            {
                fs::copy(entry.path(), destination.join(entry.file_name())).unwrap();
            }
        }
        if let Ok(entries) = fs::read_dir(&self.config.state_directory) {
            for entry in entries.flatten() {
                if !entry
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".intent.json")
                {
                    continue;
                }
                let Some(intent) = fs::read(entry.path())
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                else {
                    continue;
                };
                let (Some(name), Some(owner)) =
                    (intent["owned_resource"].as_str(), intent["owner"].as_str())
                else {
                    continue;
                };
                let inspected = Command::new(&self.docker)
                    .env_clear()
                    .envs(&self.config.environment)
                    .args([
                        "container",
                        "inspect",
                        "--format",
                        "{{ index .Config.Labels \"org.gaap.owner\" }}",
                        name,
                    ])
                    .output();
                if inspected.is_ok_and(|result| {
                    result.status.success()
                        && String::from_utf8_lossy(&result.stdout).trim() == owner
                }) {
                    let _ = Command::new(&self.docker)
                        .env_clear()
                        .envs(&self.config.environment)
                        .args(["container", "rm", "--force", "--volumes", name])
                        .output();
                }
            }
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn fixture() -> Fixture {
    let executable = fs::canonicalize(
        std::env::var_os("GAAP_DOCKER_EXECUTABLE")
            .expect("Set GAAP_DOCKER_EXECUTABLE to the Docker CLI"),
    )
    .unwrap();
    let socket = std::env::var("GAAP_DOCKER_HOST")
        .expect("Set GAAP_DOCKER_HOST to an explicit local daemon endpoint");
    let environment = BTreeMap::from([
        ("DOCKER_HOST".into(), socket),
        ("GAAP_UNDECLARED_SECRET".into(), "host-only-canary".into()),
    ]);
    let version = Command::new(&executable)
        .env_clear()
        .envs(&environment)
        .args(["version", "--format", "{{json .Server}}"])
        .output()
        .unwrap();
    assert!(version.status.success());
    let version: Value = serde_json::from_slice(&version.stdout).unwrap();
    let docker = executable.clone();
    let mut config = OciConfig {
        gvisor: None,
        executable,
        environment,
        state_directory: PathBuf::new(),
        approved_profile_digest: String::new(),
        engine_version: version["Version"].as_str().unwrap().into(),
        runtime: "runc".into(),
        runtime_version: version["Components"]
            .as_array()
            .unwrap()
            .iter()
            .find(|component| component["Name"] == "runc")
            .unwrap()["Version"]
            .as_str()
            .unwrap()
            .into(),
    };
    let root = std::env::temp_dir().join(format!(
        "gaap-native-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&root).unwrap();
    let root = root.canonicalize().unwrap();
    config.state_directory = root.join("state");
    fs::create_dir(&config.state_directory).unwrap();
    fs::set_permissions(&config.state_directory, fs::Permissions::from_mode(0o700)).unwrap();
    let workspace = root.join("workspace");
    fs::create_dir(&workspace).unwrap();
    fs::write(workspace.join("source.txt"), b"initial owned subject\n").unwrap();
    fs::write(root.join("undeclared.txt"), b"host-only-canary").unwrap();
    let uid = String::from_utf8(Command::new("id").arg("-u").output().unwrap().stdout).unwrap();
    let gid = String::from_utf8(Command::new("id").arg("-g").output().unwrap().stdout).unwrap();
    let subject: Value = serde_json::from_str(include_str!(
        "../../../examples/contracts/v0.1.0/agent-run-request.json"
    ))
    .unwrap();
    let mut request = SandboxRequest {
        schema_version: "gaap.sandbox/0.1".into(),
        profile_id: "local-oci/0.1".into(),
        invocation_kind: SandboxInvocationKind::ProtectedEffect,
        invocation_digest: digest(b"native protected effect fixture"),
        image: SandboxImage {
            reference: format!("python@{IMAGE}"),
            digest: IMAGE.into(),
        },
        command: vec![
            "/usr/local/bin/python3".into(),
            "-I".into(),
            "-S".into(),
            "-B".into(),
            "-c".into(),
            "print('native execution')".into(),
        ],
        working_directory: "/workspace".into(),
        user: format!(
            "{}:{}",
            if uid.trim() == "0" {
                "1000"
            } else {
                uid.trim()
            },
            if gid.trim() == "0" {
                "1000"
            } else {
                gid.trim()
            }
        ),
        environment: BTreeMap::new(),
        mounts: vec![SandboxMount {
            source: workspace.to_str().unwrap().into(),
            destination: "/workspace".into(),
            writable: true,
        }],
        network: SandboxNetwork::Disabled,
        limits: SandboxLimits {
            cpu_millis_per_second: 1000,
            memory_bytes: 64 * 1024 * 1024,
            processes: 32,
            stdout_bytes: 8192,
            stderr_bytes: 8192,
            wall_clock_ms: 30000,
            cleanup_ms: 5000,
        },
        input_subject: serde_json::from_value(subject["subject"].clone()).unwrap(),
        outputs: SandboxOutputs {
            paths: vec![],
            max_total_bytes: 65536,
        },
    };
    if uid.trim() == "0" {
        assert!(
            Command::new("chown")
                .args(["-R", &request.user, workspace.to_str().unwrap()])
                .status()
                .unwrap()
                .success()
        );
    }
    if let Some(path) = std::env::var_os("GAAP_RUNSC_EXECUTABLE") {
        assert_eq!(
            std::env::consts::OS,
            "linux",
            "Real runsc tests require Linux"
        );
        let executable = fs::canonicalize(path).unwrap();
        let version = Command::new(&executable).arg("--version").output().unwrap();
        assert!(version.status.success());
        config.runtime = "runsc".into();
        config.runtime_version = String::from_utf8(version.stdout).unwrap().trim().into();
        config.gvisor = Some(GvisorConfig {
            executable_digest: digest(&fs::read(&executable).unwrap()),
            executable,
        });
        request.profile_id = GVISOR_PROFILE.into();
    }
    config.approved_profile_digest = request.profile_digest().unwrap();
    Fixture {
        root,
        config,
        docker,
        request,
    }
}
fn run(fixture: &Fixture) -> gaap::sandbox::SandboxObservation {
    let result = LocalOciAdapter::new(fixture.config.clone()).execute(&fixture.request, &|| false);
    let path = fixture.config.state_directory.join(format!(
        "result-{}.json",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::write(path, serde_jcs::to_vec(&result).unwrap()).unwrap();
    result
}
fn proxy(fixture: &mut Fixture, condition: &str) {
    let executable = fixture.root.join("docker-proxy");
    let real = fixture
        .config
        .executable
        .to_str()
        .unwrap()
        .replace('\'', "'\\''");
    fs::write(
        &executable,
        format!("#!/bin/sh\n{condition}\nexec '{real}' \"$@\"\n"),
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    fixture.config.executable = executable;
}

#[test]
#[ignore = "requires an explicitly configured real Docker daemon and the pinned image"]
fn native_single_file_mount_collects_the_exact_output() {
    let mut fixture = fixture();
    fixture.request.mounts[0].source = fixture
        .root
        .join("workspace/source.txt")
        .to_str()
        .unwrap()
        .into();
    fixture.request.mounts[0].destination = "/workspace/source.txt".into();
    fixture.request.command[5] = "import pathlib;pathlib.Path('/workspace/source.txt').write_text('narrow effect');assert not pathlib.Path('/workspace/undeclared.txt').exists()".into();
    fixture.request.outputs.paths = vec!["/workspace/source.txt".into()];
    fixture.config.approved_profile_digest = fixture.request.profile_digest().unwrap();
    let result = run(&fixture);
    assert_eq!(result.status, SandboxStatus::Exited, "{result:?}");
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.cleanup, SandboxCleanup::Removed);
    assert_eq!(result.artifacts.len(), 1);
    assert_eq!(result.artifacts[0].bytes, b"narrow effect");
}

#[test]
#[ignore = "requires an explicitly configured real Docker daemon and the pinned image"]
fn native_declared_effect_and_cleanup() {
    let mut fixture = fixture();
    fixture.request.command[5] = "import pathlib;pathlib.Path('/workspace/source.txt').write_text('effect occurred\\n');print('completed effect')".into();
    fixture.request.outputs.paths = vec!["/workspace/source.txt".into()];
    fixture.config.approved_profile_digest = fixture.request.profile_digest().unwrap();
    let result = run(&fixture);
    assert_eq!(result.status, SandboxStatus::Exited, "{result:?}");
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.cleanup, SandboxCleanup::Removed);
    let intent: Value =
        serde_json::from_slice(&fs::read(result.intent_path.as_ref().unwrap()).unwrap()).unwrap();
    assert_eq!(intent["request_digest"], result.request_digest);
    assert_eq!(
        intent["invocation_digest"],
        fixture.request.invocation_digest
    );
    let published: Value = serde_json::from_slice(
        &fs::read(fixture.config.state_directory.join(format!(
            "{}.observation.json",
            result.owned_resource.as_ref().unwrap()
        )))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(published["cleanup"], "removed");
    assert_eq!(published["stdout_digest"], result.stdout_digest);
    assert!(result.publication_error.is_none());
    assert_eq!(result.image_digest.as_deref(), Some(IMAGE));
    assert_eq!(
        result.runtime.as_ref().unwrap().runtime,
        fixture.config.runtime
    );
    assert_eq!(result.artifacts[0].bytes, b"effect occurred\n");
    assert_eq!(result.artifacts[0].digest, digest(b"effect occurred\n"));
    assert_eq!(result.stdout_digest, digest(&result.stdout));
    assert_eq!(result.stderr_digest, digest(&result.stderr));
}

#[test]
#[ignore = "requires an explicitly configured real Docker daemon and the pinned image"]
fn native_undeclared_environment_path_and_network_are_unavailable() {
    let mut fixture = fixture();
    fixture
        .request
        .environment
        .insert("ALLOWED".into(), "yes".into());
    fixture.request.command[5] = format!(
        "import json,os,pathlib,socket\nnetwork=False\ntry:\n socket.create_connection(('1.1.1.1',53),timeout=0.2);network=True\nexcept OSError: pass\nprint(json.dumps({{'allowed':os.getenv('ALLOWED'),'secret':os.getenv('GAAP_UNDECLARED_SECRET'),'host_path':pathlib.Path({:?}).exists(),'network':network}}))",
        fixture.root.join("undeclared.txt").to_str().unwrap()
    );
    fixture.config.approved_profile_digest = fixture.request.profile_digest().unwrap();
    let result = run(&fixture);
    assert_eq!(result.exit_code, Some(0), "{result:?}");
    assert_eq!(result.cleanup, SandboxCleanup::Removed);
    let output: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(
        output,
        serde_json::json!({"allowed":"yes","secret":null,"host_path":false,"network":false})
    );
}

#[test]
#[ignore = "requires an explicitly configured real Docker daemon and the pinned image"]
fn native_read_only_mount_refuses_mutation() {
    let mut fixture = fixture();
    fixture.request.mounts[0].writable = false;
    fixture.request.command[5] =
        "import pathlib;pathlib.Path('/workspace/source.txt').write_text('forbidden')".into();
    fixture.config.approved_profile_digest = fixture.request.profile_digest().unwrap();
    let result = run(&fixture);
    assert_eq!(result.status, SandboxStatus::Exited, "{result:?}");
    assert_ne!(result.exit_code, Some(0));
    assert_eq!(result.cleanup, SandboxCleanup::Removed);
    assert_eq!(
        fs::read(fixture.root.join("workspace/source.txt")).unwrap(),
        b"initial owned subject\n"
    );
}

#[test]
#[ignore = "requires an explicitly configured real Docker daemon and the pinned image"]
fn native_timeout_removes_the_owned_container() {
    let mut fixture = fixture();
    fixture.request.limits.wall_clock_ms = 4000;
    fixture.request.command[5] = "import time;time.sleep(30)".into();
    fixture.config.approved_profile_digest = fixture.request.profile_digest().unwrap();
    let result = run(&fixture);
    assert_eq!(result.status, SandboxStatus::TimedOut, "{result:?}");
    assert_eq!(result.cleanup, SandboxCleanup::Removed);
}

#[test]
#[ignore = "requires an explicitly configured real Docker daemon and the pinned image"]
fn native_cancellation_removes_the_owned_container() {
    let mut fixture = fixture();
    fixture.request.command[5] = "import time;time.sleep(30)".into();
    fixture.config.approved_profile_digest = fixture.request.profile_digest().unwrap();
    let start = Instant::now();
    let result = LocalOciAdapter::new(fixture.config.clone()).execute(&fixture.request, &|| {
        start.elapsed() >= Duration::from_secs(3)
    });
    assert_eq!(result.status, SandboxStatus::Cancelled, "{result:?}");
    assert_eq!(result.cleanup, SandboxCleanup::Removed);
}

#[test]
#[ignore = "requires an explicitly configured real Docker daemon and the pinned image"]
fn native_output_overflow_is_bounded_and_cleaned_up() {
    let mut fixture = fixture();
    fixture.request.limits.stdout_bytes = 1024;
    fixture.request.command[5] =
        "import sys;sys.stdout.write('x'*1000000);sys.stdout.flush()".into();
    fixture.config.approved_profile_digest = fixture.request.profile_digest().unwrap();
    let result = run(&fixture);
    assert_eq!(result.status, SandboxStatus::OutputOverflow, "{result:?}");
    assert_eq!(result.stdout.len(), 1024);
    assert_eq!(result.cleanup, SandboxCleanup::Removed);
}

#[test]
#[ignore = "requires an explicitly configured real Docker daemon and the pinned image"]
fn native_memory_limit_is_recorded() {
    let mut fixture = fixture();
    fixture.request.limits.memory_bytes = 32 * 1024 * 1024;
    fixture.request.command[5] =
        "allocation=bytearray(256*1024*1024);print(len(allocation))".into();
    fixture.config.approved_profile_digest = fixture.request.profile_digest().unwrap();
    let result = run(&fixture);
    assert_eq!(result.status, SandboxStatus::ResourceLimit, "{result:?}");
    assert_eq!(result.exit_code, Some(137));
    assert_eq!(result.cleanup, SandboxCleanup::Removed);
}

#[test]
#[ignore = "requires an explicitly configured real Docker daemon and the pinned image"]
fn native_runtime_mismatch_never_falls_back() {
    let mut fixture = fixture();
    fixture.config.runtime_version = "unapproved-runtime-version".into();
    fixture.config.approved_profile_digest = fixture.request.profile_digest().unwrap();
    let result = run(&fixture);
    assert_eq!(result.status, SandboxStatus::SetupFailed);
    assert_eq!(result.cleanup, SandboxCleanup::NotCreated);
    assert!(result.owned_resource.is_none());
    assert!(
        result
            .reason
            .unwrap()
            .contains("differs from host adoption")
    );
}

#[test]
#[ignore = "requires an explicitly configured real Docker daemon and the pinned image"]
fn native_output_symlink_cannot_import_host_content() {
    let mut fixture = fixture();
    fixture.request.command[5] = "import os;os.symlink('/etc/passwd','/workspace/leak')".into();
    fixture.request.outputs.paths = vec!["/workspace/leak".into()];
    fixture.config.approved_profile_digest = fixture.request.profile_digest().unwrap();
    let result = run(&fixture);
    assert_eq!(result.status, SandboxStatus::UnknownOutcome, "{result:?}");
    assert!(result.artifacts.is_empty());
    assert_eq!(result.cleanup, SandboxCleanup::Removed);
}

#[test]
#[ignore = "requires an explicitly configured real Docker daemon and the pinned image"]
fn native_cleanup_failure_preserves_result_and_reports_owned_resource() {
    let mut fixture = fixture();
    let real = fixture.config.executable.clone();
    proxy(
        &mut fixture,
        "if [ \"$1\" = container ] && [ \"$2\" = rm ]; then echo 'simulated cleanup rejection' >&2; exit 73; fi",
    );
    fixture.config.approved_profile_digest = fixture.request.profile_digest().unwrap();
    let result = run(&fixture);
    assert_eq!(result.status, SandboxStatus::Exited, "{result:?}");
    assert_eq!(result.cleanup, SandboxCleanup::Unconfirmed);
    assert!(
        result
            .cleanup_reason
            .as_ref()
            .unwrap()
            .contains("simulated cleanup rejection")
    );
    let owned = result.owned_resource.unwrap();
    // The proxy fault leaves our real stopped container; remove only this test's owned resource.
    let cleanup = Command::new(real)
        .env_clear()
        .envs(&fixture.config.environment)
        .args(["container", "rm", "--force", "--volumes", &owned])
        .output()
        .unwrap();
    assert!(cleanup.status.success());
}

#[test]
#[ignore = "requires an explicitly configured real Docker daemon and the pinned image"]
fn native_cli_sigterm_retains_effect_and_publishes_cleanup() {
    let mut fixture = fixture();
    fixture.request.command[5] = "import pathlib,time;pathlib.Path('/workspace/started').write_text('effect before cancellation');time.sleep(30)".into();
    let config = fixture.root.join("host.json");
    fs::write(&config, serde_json::to_vec(&fixture.config).unwrap()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_gaap-sandbox"))
        .arg(config)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&fixture.request).unwrap())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while !fixture.root.join("workspace/started").exists() {
        assert!(
            Instant::now() < deadline,
            "workload didn't reach its effect"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        Command::new("/bin/kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let observation: gaap::sandbox::SandboxObservation =
        serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(observation.status, SandboxStatus::Cancelled);
    assert_eq!(observation.cleanup, SandboxCleanup::Removed);
    assert!(observation.publication_error.is_none());
    assert_eq!(
        fs::read(fixture.root.join("workspace/started")).unwrap(),
        b"effect before cancellation"
    );
    assert!(std::path::Path::new(observation.intent_path.as_ref().unwrap()).is_file());
}

#[test]
#[ignore = "requires an explicitly configured real Docker daemon and the pinned image"]
fn native_request_cannot_expand_host_adopted_mounts() {
    let mut fixture = fixture();
    fixture.request.mounts[0].source = fixture.root.to_str().unwrap().into();
    let result = run(&fixture);
    assert_eq!(result.status, SandboxStatus::SetupFailed);
    assert_eq!(result.cleanup, SandboxCleanup::NotCreated);
    assert!(result.owned_resource.is_none());
    assert!(result.reason.unwrap().contains("host-approved profile"));
}

#[test]
#[ignore = "requires an explicitly configured real Docker daemon and the pinned image"]
fn native_runtime_transport_crash_retains_observed_effect() {
    let mut fixture = fixture();
    fixture.request.command[5] = "import pathlib;pathlib.Path('/workspace/source.txt').write_text('effect before transport crash')".into();
    fixture.request.outputs.paths = vec!["/workspace/source.txt".into()];
    let real = fixture
        .config
        .executable
        .to_str()
        .unwrap()
        .replace('\'', "'\\''");
    proxy(
        &mut fixture,
        &format!(
            "if [ \"$1\" = container ] && [ \"$2\" = start ]; then '{real}' \"$@\"; exit 17; fi"
        ),
    );
    fixture.config.approved_profile_digest = fixture.request.profile_digest().unwrap();
    let result = run(&fixture);
    assert_eq!(result.status, SandboxStatus::RuntimeCrash, "{result:?}");
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.artifacts[0].bytes, b"effect before transport crash");
    assert_eq!(result.cleanup, SandboxCleanup::Removed);
}

#[test]
#[ignore = "requires an explicitly configured real Docker daemon and the pinned image"]
fn native_lost_terminal_observation_preserves_effect_as_unknown() {
    let mut fixture = fixture();
    fixture.request.command[5] = "import pathlib;pathlib.Path('/workspace/source.txt').write_text('effect before lost observation')".into();
    let real = fixture
        .config
        .executable
        .to_str()
        .unwrap()
        .replace('\'', "'\\''");
    let marker = fixture
        .root
        .join("lose-next-inspection")
        .to_str()
        .unwrap()
        .replace('\'', "'\\''");
    proxy(
        &mut fixture,
        &format!(
            "if [ \"$1\" = container ] && [ \"$2\" = start ]; then '{real}' \"$@\"; result=$?; : > '{marker}'; exit \"$result\"; fi\nif [ \"$1\" = container ] && [ \"$2\" = inspect ] && [ -f '{marker}' ]; then /bin/rm '{marker}'; echo 'simulated lost terminal observation' >&2; exit 74; fi"
        ),
    );
    fixture.config.approved_profile_digest = fixture.request.profile_digest().unwrap();
    let result = run(&fixture);
    assert_eq!(result.status, SandboxStatus::UnknownOutcome, "{result:?}");
    assert_eq!(result.cleanup, SandboxCleanup::Removed);
    assert!(
        result
            .reason
            .as_ref()
            .unwrap()
            .contains("simulated lost terminal observation")
    );
    assert_eq!(
        fs::read(fixture.root.join("workspace/source.txt")).unwrap(),
        b"effect before lost observation"
    );
    assert!(std::path::Path::new(result.intent_path.as_ref().unwrap()).is_file());
}

#[test]
#[ignore = "requires an explicitly configured real Docker daemon and the pinned image"]
fn native_file_size_signal_retains_partial_output() {
    let mut fixture = fixture();
    fixture.request.outputs.max_total_bytes = 8192;
    fixture.request.outputs.paths = vec!["/workspace/source.txt".into()];
    // PID 1 ignores default fatal signals. Run the writer as a child so Linux
    // can deliver SIGXFSZ; the parent preserves its signal-compatible exit.
    let writer = "import os,signal;signal.signal(signal.SIGXFSZ,signal.SIG_DFL);f=os.open('/workspace/source.txt',os.O_WRONLY|os.O_TRUNC);os.write(f,b'x'*8192);os.close(f);f=os.open('/dev/shm/limit-proof',os.O_WRONLY|os.O_CREAT,0o600);os.write(f,b'x'*16384);os.write(f,b'x')";
    fixture.request.command[5] = format!(
        "import subprocess,sys;r=subprocess.run(['/usr/local/bin/python3','-I','-S','-B','-c',{writer:?}]);sys.exit(128-r.returncode if r.returncode<0 else r.returncode)"
    );
    fixture.config.approved_profile_digest = fixture.request.profile_digest().unwrap();
    let result = run(&fixture);
    assert_eq!(result.status, SandboxStatus::ResourceLimit);
    assert_eq!(result.exit_code, Some(153));
    assert!(
        result
            .reason
            .as_ref()
            .unwrap()
            .contains("consistent with SIGXFSZ")
    );
    assert_eq!(result.artifacts[0].bytes.len(), 8192);
    assert_eq!(result.cleanup, SandboxCleanup::Removed);
}

#[test]
#[ignore = "requires an explicitly configured real Docker daemon and the pinned image"]
fn native_weakened_container_profiles_never_start() {
    for extra in ["--security-opt seccomp=unconfined", "--cap-add SYS_ADMIN"] {
        let mut fixture = fixture();
        fixture.request.command[5] =
            "import pathlib;pathlib.Path('/workspace/source.txt').write_text('weaker profile ran')"
                .into();
        let real = fixture
            .config
            .executable
            .to_str()
            .unwrap()
            .replace('\'', "'\\''");
        proxy(
            &mut fixture,
            &format!(
                "if [ \"$1\" = container ] && [ \"$2\" = create ]; then shift 2; exec '{real}' container create {extra} \"$@\"; fi"
            ),
        );
        fixture.config.approved_profile_digest = fixture.request.profile_digest().unwrap();
        let observation = run(&fixture);
        assert_eq!(
            observation.status,
            SandboxStatus::SetupFailed,
            "{extra}: {observation:?}"
        );
        assert_eq!(observation.exit_code, None);
        assert_eq!(observation.cleanup, SandboxCleanup::Removed);
        assert_eq!(
            fs::read(fixture.root.join("workspace/source.txt")).unwrap(),
            b"initial owned subject\n"
        );
    }
}

#[test]
#[ignore = "requires an explicitly configured real Docker daemon and the pinned image"]
fn native_oom_event_recovers_a_missing_state_marker() {
    let mut fixture = fixture();
    fixture.request.limits.memory_bytes = 32 * 1024 * 1024;
    fixture.request.command[5] =
        "allocation=bytearray(256*1024*1024);print(len(allocation))".into();
    let real = fixture
        .config
        .executable
        .to_str()
        .unwrap()
        .replace('\'', "'\\''");
    let marker = fixture
        .root
        .join("mask-oom-state")
        .to_str()
        .unwrap()
        .replace('\'', "'\\''");
    proxy(
        &mut fixture,
        &format!(
            "if [ \"$1\" = container ] && [ \"$2\" = start ]; then '{real}' \"$@\"; result=$?; : > '{marker}'; exit \"$result\"; fi\nif [ \"$1\" = container ] && [ \"$2\" = inspect ] && [ -f '{marker}' ]; then '{real}' \"$@\" | /usr/bin/python3 -I -S -B -c 'import json,sys;v=json.load(sys.stdin);v[0][\"State\"][\"OOMKilled\"]=False;json.dump(v,sys.stdout)'; exit $?; fi"
        ),
    );
    fixture.config.approved_profile_digest = fixture.request.profile_digest().unwrap();
    let observation = run(&fixture);
    assert_eq!(
        observation.status,
        SandboxStatus::ResourceLimit,
        "{observation:?}"
    );
    assert_eq!(observation.exit_code, Some(137));
    assert!(observation.reason.as_ref().unwrap().contains("OOM event"));
    assert_eq!(observation.cleanup, SandboxCleanup::Removed);
}

#[test]
#[ignore = "requires an explicitly configured real Docker daemon and the pinned image"]
fn native_exit_137_alone_doesnt_prove_oom() {
    let mut fixture = fixture();
    fixture.request.command[5] = "raise SystemExit(137)".into();
    fixture.config.approved_profile_digest = fixture.request.profile_digest().unwrap();
    let observation = run(&fixture);
    assert_eq!(observation.status, SandboxStatus::Exited);
    assert_eq!(observation.exit_code, Some(137));
    assert!(
        observation
            .reason
            .as_ref()
            .unwrap()
            .contains("hasn't confirmed")
    );
    assert_eq!(observation.cleanup, SandboxCleanup::Removed);
}

#[test]
#[ignore = "requires an explicitly configured real Docker daemon and the pinned image"]
fn native_terminal_oom_marker_can_arrive_after_event_probe() {
    let mut fixture = fixture();
    fixture.request.limits.memory_bytes = 32 * 1024 * 1024;
    fixture.request.command[5] =
        "allocation=bytearray(256*1024*1024);print(len(allocation))".into();
    let real = fixture
        .config
        .executable
        .to_str()
        .unwrap()
        .replace('\'', "'\\''");
    let marker = fixture
        .root
        .join("mask-first-oom-state")
        .to_str()
        .unwrap()
        .replace('\'', "'\\''");
    proxy(
        &mut fixture,
        &format!(
            "if [ \"$1\" = events ]; then exit 0; fi\nif [ \"$1\" = container ] && [ \"$2\" = start ]; then '{real}' \"$@\"; result=$?; : > '{marker}'; exit \"$result\"; fi\nif [ \"$1\" = container ] && [ \"$2\" = inspect ] && [ -f '{marker}' ]; then /bin/rm '{marker}'; '{real}' \"$@\" | /usr/bin/python3 -I -S -B -c 'import json,sys;v=json.load(sys.stdin);v[0][\"State\"][\"OOMKilled\"]=False;json.dump(v,sys.stdout)'; exit $?; fi"
        ),
    );
    fixture.config.approved_profile_digest = fixture.request.profile_digest().unwrap();
    let observation = run(&fixture);
    assert_eq!(
        observation.status,
        SandboxStatus::ResourceLimit,
        "{observation:?}"
    );
    assert_eq!(observation.exit_code, Some(137));
    assert!(
        observation
            .reason
            .as_ref()
            .unwrap()
            .contains("terminal reconciliation")
    );
    assert_eq!(observation.cleanup, SandboxCleanup::Removed);
}

#[test]
#[ignore = "requires a real Linux runsc host and the pinned image"]
fn runsc_identity_flags_and_stopped_container_are_original_evidence() {
    let fixture = fixture();
    let expected = fixture
        .config
        .gvisor
        .as_ref()
        .expect("Select GAAP_RUNSC_EXECUTABLE");
    let result = run(&fixture);
    assert_eq!(result.status, SandboxStatus::Exited, "{result:?}");
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.cleanup, SandboxCleanup::Removed);
    let runtime = result.runtime.unwrap();
    assert_eq!(runtime.runtime, "runsc");
    assert_eq!(runtime.runtime_version, fixture.config.runtime_version);
    assert_eq!(runtime.executable_digest, expected.executable_digest);
    let resource = result.owned_resource.unwrap();
    let read = |suffix: &str| -> Value {
        serde_json::from_slice(
            &fs::read(
                fixture
                    .config
                    .state_directory
                    .join(format!("{resource}.{suffix}.json")),
            )
            .unwrap(),
        )
        .unwrap()
    };
    let evidence = read("runtime");
    assert_eq!(evidence["request_digest"], result.request_digest);
    assert_eq!(evidence["before"], evidence["after"]);
    assert_eq!(evidence["before"]["flags"], serde_json::json!(GVISOR_FLAGS));
    assert!(evidence["after_error"].is_null());
    let container = read("container");
    assert_eq!(container["container"]["State"]["Status"], "created");
    assert_eq!(container["container"]["HostConfig"]["Runtime"], "runsc");
    assert_eq!(container["container"]["HostConfig"]["NetworkMode"], "none");
}

#[test]
#[ignore = "requires a real Linux runsc host and the pinned image"]
fn runsc_hash_and_registration_mismatch_fail_before_any_workload() {
    for fault in ["hash", "missing", "flags", "fallback"] {
        let mut fixture = fixture();
        let selected = fixture
            .config
            .gvisor
            .as_mut()
            .expect("Select GAAP_RUNSC_EXECUTABLE");
        match fault {
            "hash" => selected.executable_digest = digest(b"wrong runsc bytes"),
            "missing" => proxy(
                &mut fixture,
                "if [ \"$1\" = info ]; then echo '{\"OSType\":\"linux\",\"Runtimes\":{}}'; exit 0; fi",
            ),
            "flags" => proxy(
                &mut fixture,
                "if [ \"$1\" = info ]; then echo '{\"OSType\":\"linux\",\"Runtimes\":{\"runsc\":{\"path\":\"/usr/local/bin/runsc\",\"runtimeArgs\":[\"--network=host\"]}}}'; exit 0; fi",
            ),
            _ => fixture.config.runtime = "runc".into(),
        }
        let result = run(&fixture);
        assert_eq!(
            result.status,
            SandboxStatus::SetupFailed,
            "{fault}: {result:?}"
        );
        assert_eq!(result.cleanup, SandboxCleanup::NotCreated);
        assert!(result.owned_resource.is_none());
        assert!(result.runtime.is_none());
    }
}

#[test]
#[ignore = "requires a real Linux runsc host and the pinned image"]
fn runsc_post_probe_failure_keeps_original_exits_and_cleanup() {
    for exit in [0, 7] {
        let mut fixture = fixture();
        assert!(
            fixture.config.gvisor.is_some(),
            "Select GAAP_RUNSC_EXECUTABLE"
        );
        fixture.request.command[5] =
            format!("import sys;print('before failed post probe');sys.exit({exit})");
        fixture.config.approved_profile_digest = fixture.request.profile_digest().unwrap();
        let counter = fixture.root.join("info-count");
        proxy(
            &mut fixture,
            &format!(
                "if [ \"$1\" = info ]; then count=0; [ ! -f '{0}' ] || read -r count < '{0}'; count=$((count+1)); echo \"$count\" > '{0}'; if [ \"$count\" -ge 3 ]; then echo 'injected post-execution registration failure' >&2; exit 71; fi; fi",
                counter.display()
            ),
        );
        let result = run(&fixture);
        assert_eq!(
            result.status,
            if exit == 0 {
                SandboxStatus::UnknownOutcome
            } else {
                SandboxStatus::Exited
            },
            "{result:?}"
        );
        assert_eq!(result.exit_code, Some(exit));
        assert_eq!(result.stdout, b"before failed post probe\n");
        assert_eq!(result.cleanup, SandboxCleanup::Removed);
        assert!(
            result
                .reason
                .unwrap()
                .contains("injected post-execution registration failure")
        );
    }
}
