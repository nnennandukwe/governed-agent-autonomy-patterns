# Phase 9 Optional Telemetry Build Plan

## Summary

Export sealed Agent Run executions through an optional read-only adapter under
[#20](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/20),
after the completed #15/#18 prerequisites in [#21](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/21).
Baseline: `cf49db5ca262a5b43156f48c46b74b9b11a4dac2`. One PR will own the adapter,
its integration tests, mapping documentation and required CI, including the
declared minimum Rust and pinned dependency-audit checks. Telemetry will
consume the existing sealed receipt ledger after the engine returns.

## Skills To Use

| Stage | Skill | Responsibility and evidence |
|---|---|---|
| Planning | Software Build Plan | Bind source criteria, seams, compatibility and checks before coding. |
| Context | qodo-get-rules, qodo-codebase-wisdom | Read applicable rules and coupling; pin actual local source. |
| Hardening | failure-path-testing | Exercise blocked, failed, slow, dropped, sampled and disconnected export through real engine and HTTP boundaries. |
| Workflow compatibility | workflow-invariants | Assert existing decision/effect/verification/receipt results remain identical across export modes; preserve the accepted state model. |
| Interface checks | dx-audit | Recheck this adapter's API, setup, errors and mapping docs only. |
| Review | qodo-review, qodo-review-resolver | Review the committed full diff locally, then require completed current-head hosted review and CI. |
| Prose | voice-canon | Apply plain English, contractions and the language verifier. |

No Rust-specific implementation skill is installed. Repository-native Rust
checks will cover that stage. Transaction-proof doesn't apply: this adapter
will own no durable state, canonical writes or recovery transaction. TDD/BDD
weren't requested; integration behavior will guide the implementation.
Qodo Local Review is eligible: Git origin and pushed baseline are available;
authentication succeeded. Service access will be verified at submission.

## Scope

In scope: disabled/no-op configuration; bounded local OTLP/HTTP JSON export;
pinned OpenTelemetry protocol and semantic-convention metadata; safe identity
correlation; default content omission; authoritative event/result projection;
failure isolation and real interface tests; signed-evidence guidance; compatible
`threadloop.*` correlation fields with absent values left absent.

Out of scope: #33's viewer selection, #32's recovery design, runtime/lifecycle
controls, new contract fields, live model instrumentation, hosted ingestion,
ThreadLoop edits, releases and mandatory collectors.

## Package Layout / File-to-Task Mapping

- `adapters/telemetry/` (new): independent Rust crate, projection and bounded worker/OTLP transport.
- `adapters/telemetry/tests/` (new): real engine, deterministic local sinks and HTTP failure tests.
- `adapters/telemetry/README.md` (new): mapping, version policy, privacy, configuration and evidence limits.
- `.github/workflows/ci.yml`: run locked adapter verification and require its result.
- `README.md`, `CONTRIBUTING.md`: link the optional adapter and verification commands.

The core engine, frozen contracts, generated artifacts and inspector will keep
their existing implementation.

## Dependencies And Settings

The separate adapter will depend on GAAP by path, pinned serde_json/sha2 and
ureq 3.4.3 without optional TLS/proxy features. Core dependencies will stay
unchanged. The adapter will implement the OpenTelemetry Protocol v1.9.0 JSON
wire profile and pin semantic conventions v1.42.0. It's a protocol producer,
not an OpenTelemetry Rust SDK distribution; metadata will state that honestly.
No GenAI convention or fabricated model operation will be emitted.

Configuration will be explicit Rust values; environment overrides will be
absent. Disabled will be the default. HTTP endpoints will accept only numeric
loopback socket addresses, a fixed `/v1/traces` path, no redirects, no proxy,
and a bounded total timeout. Queue depth, per-projection event/effect count,
identity length and payload size will have hard upper bounds. Rust 1.88 will
match the other standalone adapters.

## Canonical Contracts

`Telemetry::disabled()` will allocate no worker or socket.
`Telemetry::otlp(config)` will create one bounded worker. `observe` will accept
an immutable sealed `AgentRunExecution` and explicit correlation metadata,
returning an enqueue/drop outcome without waiting for the sink. The host will
call it after `AgentRunEngine::run`; telemetry will receive no mutable engine,
executor, verifier or ThreadLoop reference. Export counters will describe
transport delivery only.

A run span will project ledger events in sequence order. Effect child spans
will preserve decision and execution status independently. All event timestamps
will identify projection time because native events lack wall-clock timestamps;
no turn duration or upstream parent will be invented. Agent turns aren't
recorded by the current ledger, so they're deliberately absent. Tool-execution
and sandbox evidence will map only when present in the sealed source. Unknown
model usage will remain unknown instead of becoming a zero-cost metric.

Raw IDs will be domain-separated SHA-256 identities; content digests will remain
exact. Known Workflow Run/Action Request/Claim/Attempt identities will be
host-supplied and hashed under documented `threadloop.*` keys. Evidence links
will be relative content-addressed references without tokens or source paths.
Frozen native bytes and digest definitions will remain unchanged. OTLP v1.9.0
will govern acknowledgement field casing: lowerCamelCase is valid; unknown
field names are ignored. The general Protobuf JSON allowance for original
field names won't override that pinned OTLP rule.

## TDD And BDD Implementation Strategy

1. Prove sanitized deterministic projection from real sealed executions, preserving all source event types, effect status and verification results.
2. Prove disabled, sampled and bounded submission behavior with controlled sinks and saturation barriers.
3. Prove HTTP success/failure/partial rejection/slowness/restart without receipt or executor-call changes.
4. Add documentation and CI; run focused interface checks and full required review gates.

## Component Design

One adapter module will own projection and worker lifecycle. It'll copy only
allowlisted values into bounded JSON before enqueueing. The worker will own
network I/O, catch sink panic, and retain aggregate delivery/drop counters.
Admission will reserve at most queue capacity plus one worker slot before
projection begins. A private ownership guard will retain each slot through
construction, queueing and sink execution, and release it on every exit path.
This will bound simultaneous caller allocations as well as queued payloads.
A public-API allocation probe under a held/full sink will verify that rejected
submissions allocate no projection. An invalid-projection recovery case will
check that an error releases its slot.
Dropping telemetry will signal stop and won't join a blocked sink. An explicit
bounded shutdown will allow callers to collect delivery results outside the
run path. Each sink invocation will get no canonical writer or authority port.

## Failure And Recovery Rules

| Failure | Runtime/receipt behavior | Telemetry outcome and recovery |
|---|---|---|
| Disabled or sampling excludes run | Identical engine output | Disabled/sampled outcome; enable explicitly or change sampling. |
| Full queue or oversized projection | Identical engine output | Drop outcome/counter; reduce load or increase bounded capacity. |
| Missing, slow or failing collector | Identical engine output | Export failure counter; repair collector and observe later runs. |
| Sink panic or disconnected worker | Identical engine output | Failure/disconnection outcome; recreate adapter. |
| Shutdown deadline | Identical engine output | Unfinished delivery stays explicit; process may discard telemetry. |

No telemetry retry will replay an effect. A new exporter can receive the same
sealed execution again; stable trace IDs identify it. Durable evidence will
survive independently under the host's existing evidence policy.

## Commit Plan

1. `feat(telemetry): project sealed runs through bounded optional OTLP export` with integration tests.
2. `docs(telemetry): document mappings and require adapter CI` with current verification evidence.
3. Review repairs will keep each reproduced behavior and its regression test together.

## Branch And PR Flow

Worktree: `gaap-issue-20`; branch: `feat/issue-20-telemetry`, from the pinned
pushed baseline. Preserve original dirty demo/history files and their hashes.
Commit only this issue's files. Run focused and full checks, then a context-bearing
Qodo checkpoint and final local review of the committed intended head. Reproduce
and repair supported findings, reverify and review each updated head.

Open a ready-for-review PR closing #20 after the eligible local gate passes.
Require hosted `status=completed` and `commit_sha` equal to the PR head, all
supported actionable findings settled by code and automatic re-attribution,
and required CI. Recheck head immediately before SHA-guarded merge. Verify
merged tree and post-merge CI before reconciling #20/#21. No manual finding
status writes. Only the repository's CI workflow currently runs on PR/main;
no release or deployment workflow is present.

## Test Plan

Focused tests will compare receipt JCS bytes, effect result digests, executor
calls and verification status across disabled/active/failure/sampling/overflow
modes for completed, denied, authority-required, failed, interrupted,
unknown-effect, stale-verification and executed-but-unverified runs.

```bash
cargo fmt --manifest-path adapters/telemetry/Cargo.toml --check
cargo clippy --manifest-path adapters/telemetry/Cargo.toml --locked --all-targets -- -D warnings
cargo test --manifest-path adapters/telemetry/Cargo.toml --locked
cargo build --manifest-path adapters/telemetry/Cargo.toml --locked
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked
cargo run --locked --example generate-contract-artifacts -- --check
bash .github/scripts/test-select-ci-lanes.sh
```

State-machine, adapter and pinned public conformance checks will run through
current CI. Local disk is about 2 GiB free; use the existing GAAP core cache,
one build job and no debug/incremental expansion. Large Linux isolation and
mutation matrices will use established CI. Required unavailable checks will
remain explicit. #33 will own actual viewer ingestion, screenshots and retention
observations after #20's gates.

## Definition Of Done

All eight #20 criteria will have traceable tests/docs and observed results.
Core contracts/artifacts will remain identical to baseline. Optional failure
and drop behavior will leave authority, effects, verification, receipt bytes
and lifecycle results identical. Focused DX checks, final local review,
current-head hosted review, required CI and post-merge CI will pass or retain
an explicit blocker; incomplete outcomes won't be marked complete.

## Assumptions And Defaults

The accepted immutable execution output is sufficient for completed-run
telemetry; live in-progress event timing isn't available in the frozen ledger.
Local OTLP JSON is the initial transport profile. Remote ingestion, auth policy,
live model spans and viewer adoption require later scope. ThreadLoop #88 keeps
its canonical verifiable export; sanitized telemetry will carry only selected
correlation hashes. Semantic-convention upgrades will require an adapter version,
a documented mapping diff and compatibility tests, without rehashing old evidence.
