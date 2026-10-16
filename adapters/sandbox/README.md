# Local OCI execution

`gaap-sandbox` executes one host-adopted request in a Docker-compatible Linux
container. It records the actual image, engine, runc version, outputs and cleanup
outcome. This is a local reproducibility boundary; it isn't sufficient isolation
for hostile code. The host's ACP/MCP processes and policy/evidence files stay
outside the container. The optional [Linux gVisor profile](#linux-gvisor-profile) adds a separate workload isolation boundary.

## Select the profile

The [Goose reference](../../examples/governed-goose/README.md) creates the host
configuration and binds its digest into the approved MCP capability. With Docker
running, explicitly install the immutable image first:

```sh
docker pull python@sha256:2d9aefe2fef018a7eb2c13064c89c71929800fd2e5dccdbf52ea5da5bb8d929a
python3 examples/governed-goose/run.py \
  --goose /tmp/gaap-goose-1.54.0/goose \
  --output /tmp/gaap-goose-oci-evidence --oci
```

The output directory must be new. The adapter never pulls an image, changes the
Docker context, substitutes a runtime or falls back to host execution. Missing
images and mismatched engine/runtime versions fail before workload start.

## Host API and process contract

`gaap::sandbox::SandboxAdapter` defines the provider-neutral request and
observation. `LocalOciAdapter` implements it. The optional process interface is:

```sh
cargo build --manifest-path adapters/sandbox/Cargo.toml --locked
adapters/sandbox/target/debug/gaap-sandbox host-config.json < request.json > observation.json
```

Supply one JSON request and close stdin within five seconds. Input is bounded to
1 MiB and host configuration to 64 KiB. The command returns one canonical JSON
observation followed by a newline. Exit zero means an observation was returned;
it doesn't mean the workload succeeded. Inspect `status`, `exit_code`, `cleanup`
and `publication_error` independently.

The trusted `OciConfig` selects a canonical Docker executable, explicit daemon
environment, private state directory, approved profile digest, engine version
and runc version. Keep that configuration outside agent-writable content. The
profile digest binds image, executable, directory, numeric non-root user,
environment, mounts, network, resource bounds and output collection. Invocation
arguments, input subject and invocation identity are separate request fields;
the caller must authorize those before using the adapter. An adopted environment
profile doesn't grant effect authority.

The local profile only supports `network: {"mode":"disabled"}`. It rejects
endpoint allowlists because it can't enforce them. The root filesystem is read
only; capabilities are dropped and privilege escalation is disabled. Only
declared host paths are bind-mounted, implicit image volumes are rejected, and the
workload receives a cleared environment. CPU, memory/swap, process count, file
size, output retention and wall-clock bounds are explicit. Output collection
accepts only bounded regular files under selected writable mounts and rejects
symlink traversal.

The Goose composition exposes one preexisting writable file per effect. Its
arithmetic verifier gets only a read-only shipping file. Fixed operation code
and verifier code are pinned by the host; agent content stays data. OCI stdout,
stderr and file artifacts enter the content archive, and Protected Effect
Results reference the original sandbox observation. Verification retains its
own observation separately. ACP and MCP domain contracts remain unchanged.

## Interruption and retained evidence

The adapter fsyncs a private launch-intent record before creating its stopped
container. It inspects the created configuration before start and checks the
owner nonce before removal. Cancellation, timeout and output overflow stop the
owned CLI process group; cleanup has its own bounded deadline. Control calls, executable hashing, artifact collection and
inherited output pipes obey deadlines. Read-only host workers let cleanup start
even when a filesystem call stalls. A blocked reader may outlive a timed-out
library call until the filesystem returns or its host process exits; its result
is discarded. Use the process interface when the host also needs to bound that
reader lifetime. `elapsed_ms` includes cleanup; the
execution deadline excludes its separately bounded cleanup. A workload exit
followed by deadline exhaustion during evidence collection reports `timed_out`,
retaining the observed exit and collected artifacts. SIGINT, SIGTERM and SIGHUP set the
process adapter's cancellation flag.

`status` separates setup failure, ordinary exit, cancellation, timeout, overflow,
resource limit, runtime crash and unknown outcome. Resource evidence identifies the Docker OOM marker, an owned-container OOM event, or an exit consistent with Linux SIGXFSZ under the configured file-size bound; an exit code alone can't prove its kernel cause. If exit 137 lacks the OOM marker, the adapter reads the daemon's OOM events for up to 250 ms within the original execution deadline. It requires the exact container ID and owner nonce, then rechecks terminal state if no matching event arrives. An unconfirmed cause remains an ordinary nonzero exit with an explicit reason. See [Docker's event interface](https://docs.docker.com/reference/cli/docker/system/events/). `cleanup` independently
reports `not_created`, `removed` or `unconfirmed`. The adapter publishes a private
observation after cleanup. A publication failure returns its error and retains
the original intent. A hard kill can leave only the intent and an owned container.
A lost response, failed cleanup or missing observation never authorizes a retry.

Retain the private state directory and inspect its intent's resource name and
owner nonce. For manual cleanup, first inspect that exact container and require
its `org.gaap.owner` label to equal the recorded nonce; remove only that owned
resource. Cleanup doesn't establish whether a protected effect occurred. Subject
reconciliation and resumable orchestration remain #32.

Docker daemon access and same-user host processes remain trusted. This profile
doesn't prevent a privileged host process from altering policy/evidence, a
host-side bypass, or escaped descendants. Image identity and an observed runc
version aren't a production security certification.

## Run checks

```sh
cargo fmt --manifest-path adapters/sandbox/Cargo.toml --check
cargo clippy --manifest-path adapters/sandbox/Cargo.toml --locked --all-targets -- -D warnings
cargo test --manifest-path adapters/sandbox/Cargo.toml --locked
GAAP_DOCKER_EXECUTABLE="$(command -v docker)" \
GAAP_DOCKER_HOST=unix:///var/run/docker.sock \
  cargo test --manifest-path adapters/sandbox/Cargo.toml --locked --test native native_ -- --ignored --test-threads=1
cargo +1.88.0 test --manifest-path adapters/sandbox/Cargo.toml --locked
cargo audit --file adapters/sandbox/Cargo.lock
```

Select the actual local daemon socket on macOS. Native tests are explicitly
opted in; they fail if their configured runtime or pinned image is unavailable.
They create disposable owned workspaces and test declared writes, one-file
mounts, denied environment/path/network access, read-only mounts, OOM, timeout,
cancellation, output overflow, runtime mismatch and cleanup failure. CI requires
the real Docker lane and the OCI Goose matrix.


## Linux gVisor profile

Select `gvisor/0.1` through the same `SandboxAdapter`, request and observation.
`OciConfig.runtime` must be `runsc`. Its optional `gvisor` object contains an
absolute canonical `executable` path and an exact `executable_digest`. Adopt the
complete trimmed `runsc --version` stdout as `runtime_version`; don't use the
Docker server's runc component version for runsc. Keep configuration and state
outside agent-writable paths. Request profile digests and the Goose profile
manifest bind the host's selected image, commands, environment, mounts and limits.
The manifest also binds the runsc adoption and fixed flags.

This version supports Linux x86_64 and aarch64 hosts with a local Docker daemon
and gVisor's systrap platform. CI exercises x86_64 on Ubuntu 24.04. Native macOS
and remote Docker daemons aren't supported. Other Linux kernels and arm64 need
host-specific validation before an isolation claim. The release installer
rejects unsupported platforms and checksum mismatches without installing a
runtime. It pins official release `release-20261005.0` and records the archive
checksum, binary hash and version. Configure a trusted Linux host explicitly:

```sh
python3 adapters/sandbox/install_runsc.py /tmp/gaap-runsc
sudo install -m 0755 /tmp/gaap-runsc/runsc /usr/local/bin/runsc
sudo /usr/local/bin/runsc install --runtime runsc -- --platform=systrap --network=none --directfs=false
sudo systemctl restart docker
```

Restarting Docker affects its running containers. Use an isolated development
or CI host for this setup. The fixed registration must name that exact binary
and contain exactly those three flags, in order. The adapter checks the binary
hash, bounded version output and Docker runtime registration before creation,
again before start, and afterward. The stopped container must report runsc and
the entire adopted security configuration. Missing runsc, an alternate path,
weakened flags, changed bytes or a substituted runtime fails closed. There's no
retry with runc. Workload output such as `dmesg` can't establish runtime identity.
A trusted host must prevent concurrent daemon/configuration or binary changes
between these checks and launch.

Run both the native suite and the unchanged signed Agent Run matrix:

```sh
GAAP_DOCKER_EXECUTABLE=/usr/bin/docker \
GAAP_DOCKER_HOST=unix:///var/run/docker.sock \
GAAP_RUNSC_EXECUTABLE=/usr/local/bin/runsc \
GAAP_SANDBOX_EVIDENCE=/tmp/gaap-runsc-native \
  cargo test --manifest-path adapters/sandbox/Cargo.toml --locked --test native -- --ignored --test-threads=1
GAAP_RUNSC_EXECUTABLE=/usr/local/bin/runsc \
  python3 examples/governed-goose/run.py --goose /tmp/gaap-goose-1.54.0/goose --output /tmp/gaap-goose-gvisor-evidence --gvisor
```

The host selects `--oci` or `--gvisor`; agent arguments can't change profiles.
Both exercise the same Agent Run engine, ACP/MCP adapters, effect schemas and
signed receipt verification. The required `gvisor` CI lane runs actual Linux
runsc and uploads original runtime/native records and signed Goose archives,
including failed runs. A missing runtime fails the lane. Default local tests
leave native execution explicitly unexecuted.

Private launch intents retain original runtime version bytes, binary identity
and daemon registration. Separate request-bound records retain the stopped
container and post-execution runtime checks. Goose includes these original
records in its content-addressed signed archive for effects and verification.
Cancellation, timeout, setup failure, crash and unknown outcomes keep their
existing statuses. A sandbox-denied write keeps its nonzero workload exit and
original stderr. Cleanup failure stays independent of execution status. A
post-execution identity probe has its own cleanup-sized bound, followed by the
separate resource-removal allowance; neither can suppress cleanup. An
unconfirmed post-execution runtime changes an otherwise successful exit to an
unknown outcome. Lost evidence never permits automatic replay.

### Threat model and residual trust

Treat agent-generated workload code as untrusted. gVisor intercepts its Linux
system calls in a userspace kernel, reducing direct exposure to the host kernel.
The profile denies network at Docker and runsc, disables directfs, mounts only
declared inputs/outputs, runs a numeric non-root UID/GID, drops capabilities and
bounds CPU, memory/swap, process count, file size, output and execution time.
The image's own filesystem and standard virtual system files remain visible;
arbitrary host paths, host credentials and the Docker socket aren't mounted.
A workload can still corrupt any writable file the host explicitly exposes.

The host kernel, hardware, Docker/containerd, runsc, image supply chain, adapter,
policy, verifier and evidence store remain trusted. Same-user or privileged host
processes can bypass this boundary or replace configuration. Hardware side
channels, all denial-of-service modes and bugs in those trusted components
aren't eliminated. Sandbox configuration remains security-critical. This isn't
perfect isolation, a security certification, Kubernetes integration or hosted
multitenant execution. Durable reconciliation remains issue 32.

Primary references: [gVisor Docker registration and identity warning](https://gvisor.dev/docs/user_guide/quick_start/docker/),
[systrap platform support](https://gvisor.dev/docs/user_guide/platforms/),
[gVisor security model](https://gvisor.dev/docs/architecture_guide/security/),
[filesystem configuration](https://gvisor.dev/docs/user_guide/filesystem/).
