# GAAP MCP stdio client

This Rust adapter discovers external MCP tools, binds their exact descriptors to host-approved capabilities, and
executes a prepared invocation through GAAP's existing `ExecutorPort`. GAAP's `AgentRunEngine` still owns run state,
permission, budgets, independent verification and terminal receipts.

## Build and verify

From the GAAP repository root, use Rust 1.88 or newer and Python 3:

```bash
cargo test --manifest-path adapters/mcp/Cargo.toml --locked
cargo clippy --manifest-path adapters/mcp/Cargo.toml --locked --all-targets -- -D warnings
cargo build --manifest-path adapters/mcp/Cargo.toml --locked
cargo install cargo-audit --version 0.22.2 --locked
cargo audit --file adapters/mcp/Cargo.lock
```

The independent test server is a deterministic local Python process. It doesn't call a model, use a provider account,
or incur external model/tool billing. Tests exercise actual subprocess launch, protocol exchange, dispatch refusal,
changed descriptors, mutations, cancellation and engine receipts. They don't establish live Goose interoperability.

## Host integration

1. Construct `ServerConfig` with an absolute executable and working directory, explicit arguments and environment,
   expected server name/version, and host-approved artifact digests for the executable and launch dependencies.
   Set the manifest identity using `artifact_manifest_digest`; discovery checks the listed files against those expected bytes.
2. Call `McpExecutor::discover(config, limits)` to obtain descriptors. Discovery grants no capability authority.
3. Bind one accepted run, `ContractSupport`, trusted `PermissionPolicy`, and a `SubjectObserver`. The observer supplies
   independent subject observations, execution confirmation and known external cost/model usage. Confirmation must come from host evidence outside the MCP response; unavailable measurements return an error.
4. Approve the exact discovered `CapabilityIdentity` through trusted host configuration. Prepare a tool invocation
   with its arguments, unique effect identity, sequence, subject and host-selected effect class.
5. Compose the executor with `AgentRunEngine`, using its reported executor identity and local profile. Supply an
   independent verifier and the same accepted run/policy/capability identities to the engine.

`prepare_effect` returns the existing Protected Effect Request. It hashes canonical server/tool/argument input and
retains the complete prepared request identity. Execution refuses altered input, scope, policy, run, subject or profile
bindings. It rediscovers descriptors before dispatch and evaluates ToolTrust and Permission with RunCoordinator.
Server annotations, output and self-reported success can't provide approvals, usage measurements, subject observations
or verification authority. A normal tool response remains an unknown effect unless the host independently confirms execution.

The synchronous executor owns its asynchronous SDK session on a dedicated runtime thread. An existing Tokio host can
compose it without creating a nested runtime. Its cancellation handle can be used from another host thread. Cancellation
ends the session; it can't reopen a terminal run.

## Compatibility profile

- Official rmcp SDK: exactly `3.5.1`.
- Initial protocol: exactly MCP `2025-11-25`, with the session-based initialization handshake.
- Transport: direct subprocess stdio. The child inherits only the explicitly supplied environment map.
- Content: text blocks and schema-validated structured content. Other content types and incomplete results fail closed.
- Canonical identities: RFC 8785 JSON through `serde_jcs`, with SHA-256 digest prefixes.
- HTTP, OAuth, sampling, elicitation, roots, task continuations and the newer stateless lifecycle aren't enabled.

Unknown protocol versions don't trigger automatic fallback. Server identity, protocol, full advertised tool descriptor,
input/output schemas and annotations contribute to capability identity. Changed descriptors require fresh host approval.
Annotations are hashed hints; they aren't an authorization policy.

Default bounds are a 10-second operation timeout, 1 MiB per protocol frame, 64 KiB total server stderr, 128 discovered
tools, 128 prepared invocations and 16 MiB retained inputs/evidence. Limits have hard validation bounds. Pagination
refuses repeated cursors. Incoming JSON rejects duplicate keys and unsupported messages. The SDK answers control-plane pings without effect authority. Raw server stderr isn't copied
into receipts. Retained output can be omitted when its byte budget is exhausted; its digest and omission flag stay
available. The host must persist needed raw evidence before dropping the executor.

## Effects, outcomes and recovery

| Situation | Result and recovery |
| --- | --- |
| Missing capability approval or changed descriptor | No tool dispatch; supply fresh exact host authority. |
| Permission denied | No tool dispatch; inspect the policy decision. |
| Altered prepared effect or stale host subject | Refuse invocation; inspect and prepare a new authorized action. |
| Tool error after dispatch | Preserve output and independently observed subject; failed completion doesn't imply rollback. |
| Timeout, cancellation, server exit or invalid/partial output after dispatch | Record an unknown effect outcome; inspect retained evidence before any new attempt. |
| Late duplicate response | Poison further session operations; preserve the first effect and its evidence. |
| Independent verification fails after mutation | Preserve the changed subject and executed effect; the engine blocks completion. |

The local profile is `local-stdio-unisolated/0.1`. It records host subprocess/environment settings and doesn't provide an
OS sandbox. Cancellation and fatal transport health stop further dispatch. Cleanup kills and reaps the direct server process, including when stderr overflows while idle; it doesn't certify that descendants or remote effects
stopped. A cancellation notification is advisory, and process termination isn't rollback.

Discovery verifies the listed executable and launch-artifact bytes before spawning. The host must approve their expected
digests independently and include the package dependencies relevant to its identity.
The same-user filesystem and external server remain outside an isolation guarantee. Host observation is a point-in-time
check; the host must serialize relevant subject changes when it requires stronger freshness guarantees.

Cost/model usage comes from the host observer, rather than an assumed zero for an external provider. Pure local fixtures
explicitly report zero because they don't use model or billed tool services. Missing measurements prevent successful
completion and remain explicit in retained evidence.

Receipts are integrity-checked GAAP evidence, without signing or durable recovery added by this adapter. MCP compatibility
doesn't establish safety or authority. ACP #12, live Goose #14, signed receipts #15, isolation #16/#17, recovery #32 and
ThreadLoop #111 remain separate work.
