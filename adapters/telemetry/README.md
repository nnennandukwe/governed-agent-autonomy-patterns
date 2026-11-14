# Optional Agent Run Telemetry

Use this separate Rust adapter to inspect completed Agent Run outcomes in an
OTLP consumer. It reads the engine's sealed `AgentRunExecution` after `run`
returns, then submits an allowlisted projection to a bounded worker. Disabled
is the default. The core crate has no telemetry dependency or callback.

The source receipt ledger retains authority, effect, verification and terminal
outcomes. Telemetry is lossy operational data. For authoritative verification,
retain the native receipt and effect results and use the
[signed-evidence adapter](../attestation/README.md) with explicit trust.
Trace presence, absence, status and completeness never satisfy verification.

## Use after the engine returns

```rust
use std::time::Duration;
use gaap::runtime::AgentRunExecution;
use gaap_telemetry::{Correlation, QueueConfig, Telemetry};

fn observe_completed(execution: &AgentRunExecution) {
    // The host already called AgentRunEngine::run and retained its result.
    let mut telemetry = Telemetry::otlp(
        "127.0.0.1:4318".parse().unwrap(),
        Duration::from_millis(250),
        QueueConfig::default(),
    ).unwrap();
    let submission = telemetry.observe(execution, &Correlation::default());
    // Record submission and transport counters in the host's diagnostics.
    let shutdown = telemetry.shutdown(Duration::from_millis(300));
    let statistics = telemetry.statistics();
    let _ = (submission, shutdown, statistics);
}
```

For sustained use, create one adapter at host startup and reuse it across runs.
Keep `shutdown` outside run execution. `Telemetry::default()` and
`Telemetry::disabled()` open no socket or worker. `observe` returns immediately
for disabled, sampled-out, full, invalid or disconnected input; accepted input
is projected within fixed size limits and enqueued without waiting for I/O.
A custom sink can be injected with `with_sink` for deterministic testing.

`Submission` describes admission to telemetry only. `Statistics` reports
queued, dropped, sampled-out, delivered, partially accepted and failed batches.
A delivered batch means one HTTP consumer acknowledged it. Neither retention
nor downstream completeness follows from that acknowledgement. A stopped
adapter returns `Disconnected`; create another adapter to resume export.

## Versions and transport

The producer implements the [OpenTelemetry Protocol v1.9.0 JSON profile](https://github.com/open-telemetry/opentelemetry-proto/blob/v1.9.0/docs/specification.md)
and pins [semantic conventions v1.42.0](https://github.com/open-telemetry/semantic-conventions/releases/tag/v1.42.0).
Both pins appear in resource attributes, along with the projection version:

| Field | Value |
|---|---|
| `gaap.otel.protocol.version` | `1.9.0` |
| `gaap.otel.semconv.version` | `1.42.0` |
| `gaap.telemetry.projection.version` | `0.1.0` |
| Resource and scope `schemaUrl` | `https://opentelemetry.io/schemas/1.42.0` |
| `service.name`, `service.version` | `gaap`, adapter package version |

This crate implements a small OTLP producer directly, using pinned ureq 3.4.3
for HTTP. It doesn't install an OpenTelemetry Rust SDK, a global tracer, a
collector, or model instrumentation. No `telemetry.sdk.*` identity is claimed.
The stable resource envelope and private governance mapping avoid depending
on the separately evolving GenAI conventions.

The initial profile sends `application/json` to `/v1/traces` at a numeric
loopback socket address. IPv4 and IPv6 loopback are supported. Redirects and
proxy routing are disabled; no DNS names, URL credentials, arbitrary paths or
environment overrides are accepted. The total HTTP timeout must be greater
than zero and at most five seconds. Responses must be HTTP 200 with a bounded
JSON object; a `partialSuccess` response is counted separately and never retried.
Other responses, refusal and timeout count as failed telemetry. There are no
automatic retries. Re-observing the same sealed output retains its stable IDs.

This transport profile has no authentication headers. Use an explicitly
configured local authenticated relay when ingestion requires credentials;
that relay's setup is outside this adapter. Browser authentication belongs to
the viewer and is separate from OTLP ingestion authentication. A loopback
listener still shares the machine's local trust boundary.

Upgrading either convention pin requires a mapping review, projection version
change and compatibility tests. Existing receipt schemas, digest definitions
and native evidence bytes keep their existing versions and meaning.

## Mapping and privacy

| Source | OTLP projection |
|---|---|
| Terminal receipt | One `gaap.agent_run` span with native receipt/request/subject digests and exact terminal-status enum. |
| Run transitions, plan and approval | Sequence-ordered `gaap.status_transition`, `gaap.plan_recorded`, `gaap.approval_recorded` events with selected statuses and content digests. |
| Protected-effect decisions | `gaap.protected_effect_decision` events retain gate, outcome, effect-request and subject digests. Decision text and effects are omitted. |
| Tool execution and mutation | Events retain request/action/capability and before/after subject digests. Tool inputs, outputs and source locations are omitted. |
| Independent verification and interruption | Verification events retain verdict and exact subject digest; interruption events retain sequence only. Actor names, messages and evidence locations are omitted. |
| Usage | Ledger events retain elapsed milliseconds and tool-call counts. Model token/cost values are omitted; `gaap.usage.model_scope=unavailable` preserves the absence of a complete model-usage observation. |
| Protected Effect Results | Child spans retain exact decision and execution-status enums, result/request/capability/tool-schema/sandbox-profile digests when present. |

Only allowlisted terminal reason codes are emitted; other free-text reasons
become `other_omitted`. Stale verification remains explicitly identified by
`verification.stale_subject`. Executed-but-unverified work remains an executed
effect under its actual non-completed run status. No generic success/error
translation flattens these outcomes: OTLP status remains UNSET (`0`), while
`gaap.*` fields retain their separate meanings.

The current ledger has neither agent-turn boundaries nor event wall-clock
measurements. Turn/model spans and runtime latency metrics are deliberately
absent. Every emitted timestamp is the supplied projection time; span start
and end are equal. `gaap.telemetry.timing=projection_time_only` makes this limit
queryable. Event sequence retains source order. An MCP tool operation uses the
existing generic tool-execution event and observed schema digest; this adapter
doesn't infer a protocol or copy a tool name from free text. Sandbox profile
digests come only from sealed effect results, without inventing process timing.

Prompts, constraints, repository contents, paths, tool payloads, output, actor
names, credentials, free-text reasons and full canonical audit records are
omitted by construction. Request/run/effect IDs are bounded and hashed with a
field-specific namespace plus a zero byte before SHA-256. Already-authoritative
content digests remain exact; the adapter rejects malformed digests. Hashes
aren't authentication or a claim that a supplied identity is trusted.

The host may supply Workflow Run, Action Request, Execution Claim and Attempt
identities through `Correlation`. They become `threadloop.workflow_run.id_sha256`,
`threadloop.action_request.id_sha256`, `threadloop.execution_claim.id_sha256` and
`threadloop.attempt.id_sha256`. Missing identities stay absent, and no upstream
parent span is fabricated. This follows [ThreadLoop #88's separate completed-export boundary](https://github.com/nnennandukwe/threadloop/issues/88):
its verifiable canonical records remain available through its own explicit
export/privacy contract; sanitized telemetry carries only selected hashes.

The run span's `gaap.evidence.ref` is a relative `receipts/<receipt-hash>` hint.
It contains no bearer token, local filesystem path or upstream URL. Hosts/viewers
must deliberately resolve it to an authorized read-only evidence surface. Whether
a viewer renders it as a working link belongs to [the bounded viewer evaluation](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/33).
The evidence inspector keeps its actual diffs, verification cases, trust state
and unavailable usage display independently of this projection.

## Bounds and lifecycle

The queue defaults to 16 batches and accepts capacities from 1 through 256.
A projection accepts at most 1,024 ledger events, 128 effect results, 4,096 bytes
per raw correlation/run/request/effect identity and 1 MiB of serialized output.
An oversized projection is dropped as a whole. Full queues drop the incoming
batch. No full canonical execution or raw payload is retained in the queue.
One worker owns the sink; sampling is deterministic from the receipt digest.
`sample_every=0` excludes all runs; `1` includes all; `N` selects a stable 1/N
subset. None of these settings changes runtime behavior.

Explicit shutdown stops admission and drains until its deadline. A timeout
signals the worker to stop taking further batches and returns `TimedOut`.
Outstanding batches may be discarded. Dropping the adapter never waits for a
sink, and the built-in HTTP sink has its own total timeout. A custom sink that
blocks indefinitely can retain one detached worker and one bounded queue until
it returns; callers must bound their own custom sink. Sink panics count as failed
batches and can't unwind into the engine caller. Recreate the adapter after
shutdown; later telemetry must never replay an effect to fill missing spans.

## Verify the boundary

```bash
cargo fmt --manifest-path adapters/telemetry/Cargo.toml --check
cargo clippy --manifest-path adapters/telemetry/Cargo.toml --locked --all-targets -- -D warnings
cargo test --manifest-path adapters/telemetry/Cargo.toml --locked
cargo build --manifest-path adapters/telemetry/Cargo.toml --locked
```

The tests execute eight real engine scenarios: completed, denied,
authority-required, failed, interrupted, unknown effect, stale verification and
executed-but-unverified. They verify native receipts/results, compare JCS receipt
bytes and executor calls across disabled/sampled/full-queue modes, hold a sink
behind a deterministic barrier, and exercise actual HTTP success, rejection,
partial rejection, slowness, malformed response, missing collector and restart.
A separate test checks nonblocking drop while the sink is held.
