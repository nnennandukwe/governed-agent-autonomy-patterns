# GAAP ACP client

This crate drives one external coding agent over ACP stable v1 while GAAP retains plan, permission, runtime,
protected-effect and completion authority. It uses the official `agent-client-protocol` Rust SDK **3.3.0**, the
locked schema crate **1.11.0**, and no unstable protocol features. SDK types stay inside this crate.

## Supported profile

The initial profile is local stdio on Linux/macOS with a trusted agent that delegates filesystem access to its client.
The client advertises `fs.readTextFile` and `fs.writeTextFile`. It doesn't advertise terminals, MCP-over-ACP, session
resumption, elicitation or draft v2. No MCP server declarations are injected into the agent's session.

A permission prompt is a request to let the agent perform an operation itself. This adapter never grants that authority,
even when the prompt includes raw input. It returns cancelled and ends this one-shot run with `acp.authority_required`.
Use a delegated filesystem request for the supported effect path. Terminal requests and other unsupported operations
fail closed. A supplied title, raw input, tool status or success message isn't execution or approval evidence.

ACP isn't a sandbox. A malicious local process could bypass the protocol with its own filesystem or network calls.
The host must trust the selected agent's delegation behavior and serialize workspace/launch-file changes. Canonical
paths, directory device/inode binding, approved artifact hashes and repeated launch checks don't provide atomic
confinement. Isolation remains #16/#17. The conference proposal server remains preserved and isn't an ACP implementation.

## Compose the runtime

1. Build `AgentConfig` with a canonical executable and working directory, exact expected agent name/version, explicit
   environment, and independently approved SHA-256 hashes for every launch artifact. Python/Node launchers require a
   pinned script as their first argument. Shells, package launchers and executable shebang scripts aren't supported.
2. Implement `AcpHost`. `execute` receives the full file operation and exact `ProtectedEffectRequest` only after engine
   admission. Enforce filesystem scope/freshness in the actual host executor, execute once, and return independently
   observed subject, executor/profile identity, usage and the evidence required by the frozen effect-result contract.
   For reads, return text separately in `HostEffect.read_text`. It goes to the agent and isn't stored in protocol traces.
3. Implement `AcpHost::usage` with host-owned incremental model/cost/elapsed measurements. These exclude resources
   already reported by `execute`. Unknown measurement is an error. Explicit zero is appropriate for an unbilled local
   fixture; agent-reported usage updates aren't trusted as the meter. Bound the host callback's own external I/O.
4. Call `AcpAdapter::new`, approve its exact `capability()` in `RuntimeSupport`, and put that capability in the accepted
   `AgentRunRequest`. Compose its matching `AcpExecutor` with `AgentRunEngine` and an independent `VerifierPort`.
   No prompt starts before plan admission. Each new filesystem operation pauses for the next engine decision.
5. Read `EvidenceHandle::snapshot` after the run. Persist that record alongside the returned effect and terminal receipts
   if recovery evidence is needed. `cancel()` interrupts the active session; a fresh run needs fresh authority.

[The integration test](tests/client.rs) is an executable composition using a real Python process, actual file writes,
a host meter and a separate verifier. It also validates sealed receipts with the core validators.

The engine calls the adapter's `close` hook after every return, including invalid input and rejected gates. Close
cancels the session, abandons suspended requests and waits up to one second for worker shutdown. The worker kills and
reaps its direct child; it doesn't prove descendant termination, remote cancellation or rollback. A host callback already
executing at timeout can continue until it returns. Its late result isn't admitted, the host port is quarantined, and no
new callback is queued. Such an effect needs out-of-band reconciliation before another run.

## Evidence and compatibility

Capability identity binds SDK/protocol version, the launch manifest/configuration and directory identity. Initialization
requires the expected agent identity and protocol v1. Advertised authentication methods are options, not proof that login
is currently required; an actual authentication-required response ends the run before a prompt. Negotiated capabilities and the session response are retained by
hash. Effect metadata binds protocol, SDK and session identity; effect evidence also binds that identity to the run.

The SDK handles typed requests and callbacks. A strict wire guard additionally validates SDK-generated JSON schemas,
rejects duplicate keys/IDs, unsupported or mismatched messages, foreign sessions and invalid tool-update order. Default
limits are 1 MiB per frame, 1,024 total incoming/outgoing messages, 16 MiB total wire bytes including newline delimiters, 256-byte request/tool IDs, 64 KiB total stderr and ten seconds per wait, bounded
by the accepted run deadline. Numeric values stay within the exact canonical integer range. Traces retain only direction,
method and frame hash; they don't retain raw prompts, file content, file paths or stderr. Don't enable raw SDK tracing
around secret-bearing input.

A missing read payload ends the session while preserving the host's known execution observation. A queued effect loses
authority as soon as the wire guard records a protocol failure. The host worker preserves that fixed failure code
when it refuses a queued operation.

A session can't complete solely from an agent's `end_turn` or tool-success report: this delegated-effect profile requires
at least one host-executed effect, and the engine independently verifies the resulting subject. Approval of an earlier
subject isn't copied onto later filesystem states. A later mutation without matching authority blocks the run.

| Event | Outcome and recovery |
| --- | --- |
| Permission or engine gate refuses | No host dispatch; inspect the reason and start a newly authorized run. There isn't durable resumption. |
| Cancellation | Explicit cancellation before dispatch seals a denied, zero-use effect and an interrupted run. A dispatched host effect retains the executor's actual observation. No replay. |
| Timeout, agent exit, partial frame/output or transport failure | Separate `acp.*` diagnostics; the run can't complete. Earlier confirmed effects and subject changes remain in receipts. |
| Host execution times out | Unknown attempted effect with one known tool attempt; cost/tokens remain lower bounds. The host port is quarantined and late results are ignored. Reconcile the physical state before retrying. |
| Host usage is unavailable | Failed run with the last known usage lower bound; it isn't a settled total. |
| Version/capability/session/input drift | Fail closed; use a new compatible, host-approved session. |
| Independent verification fails | Completion is blocked; an executed mutation remains an executed mutation. |

SDK upgrades require an intentional version/lockfile change and the fixture matrix. Stable v1 remains the only negotiated
protocol; enabling draft v2 or another transport needs a separate compatibility decision. A conductor proxy isn't required
for this first client profile; the same host effect boundary can support one later without moving authority into ACP.

## Checks

From the repository root, run the ACP matrix in [CONTRIBUTING](../../CONTRIBUTING.md). Python 3 is the only fixture
runtime; no provider credentials, model calls or internet access are used by the tests. The SDK and local fixture tests
don't establish real Goose compatibility (#14), signed receipts (#15), isolation (#16/#17), durable recovery (#32), or
ThreadLoop lifecycle admission (#111).
