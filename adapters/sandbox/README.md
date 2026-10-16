# Local OCI execution

`gaap-sandbox` executes one host-adopted request in a Docker-compatible Linux
container. It records the actual image, engine, runc version, outputs and cleanup
outcome. This is a local reproducibility boundary; it isn't sufficient isolation
for hostile code. The host's ACP/MCP processes and policy/evidence files stay
outside the container. Linux gVisor isolation is separate work in #17.

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
  cargo test --manifest-path adapters/sandbox/Cargo.toml --locked --test native -- --ignored --test-threads=1
cargo +1.88.0 test --manifest-path adapters/sandbox/Cargo.toml --locked
cargo audit --file adapters/sandbox/Cargo.lock
```

Select the actual local daemon socket on macOS. Native tests are explicitly
opted in; they fail if their configured runtime or pinned image is unavailable.
They create disposable owned workspaces and test declared writes, one-file
mounts, denied environment/path/network access, read-only mounts, OOM, timeout,
cancellation, output overflow, runtime mismatch and cleanup failure. CI requires
the real Docker lane and the OCI Goose matrix.
