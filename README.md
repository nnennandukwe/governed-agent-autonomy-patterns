# Governed Agent Autonomy Patterns

GAAP is a Rust library and CLI that evaluates normalized plan, permission,
tool-trust, verification, runtime, mutation, and completion decisions for an
agent run.

The current executable slice provides the integrity coordinator that a future
agent loop will call before protected effects. It does not yet execute models,
tools, repository mutations, sandboxes, or verifiers.

## Current Interfaces And Evidence

| Path or interface | Current behavior | Developer use |
| --- | --- | --- |
| [`RunCoordinator::evaluate(gate, input)`](./src/lib.rs) | Accepts a typed `Gate` and normalized JSON input, then returns an `allow`, `ask`, or `block` decision with a code and effects. | Put one deterministic decision module behind future provider, tool, and runtime adapters. |
| [`gaap::contracts`](./src/contracts) | Defines and validates provider-neutral Agent Run and Protected Effect `0.1.0` contracts. | Parse, canonicalize, validate, seal, and verify immutable contract values without importing provider SDK types. |
| [`gaap::contracts::protected_effect`](./src/contracts/protected_effect) | Defines typed effect scopes, request binding, result-state invariants, evidence references, and replay/tamper checks. It performs no effect or policy evaluation. | Integrate a future executor at one small contract seam while keeping `RunCoordinator` authoritative. |
| [`schemas/agent-run/v0.1.0`](./schemas/agent-run/v0.1.0) | Freezes Draft 2020-12 request and receipt schemas with strict fields, versions, enums, digests, and safe integers. | Generate or validate cross-language contract documents. |
| [`schemas/protected-effect/v0.1.0`](./schemas/protected-effect/v0.1.0) | Freezes separate Draft 2020-12 Protected Effect Request and Result schemas. | Validate cross-language effect proposals and observed outcomes. |
| [`examples/contracts/v0.1.0`](./examples/contracts/v0.1.0) | Provides one request and seven digest-valid terminal outcomes. | Inspect completed and failure-path contract behavior. |
| [`examples/contracts/protected-effect/v0.1.0`](./examples/contracts/protected-effect/v0.1.0) | Provides one effect request and eight digest-valid result scenarios. | Inspect executed, non-executed, drift, failure, interruption, and unknown-outcome behavior. |
| `gaap run-invariant` | Reads a RunInvariant request from stdin and writes one response to stdout. | Test the Rust coordinator without importing its crate or depending on Rust. |
| [`evidence/run-invariant-subject-v0.1.0.json`](./evidence/run-invariant-subject-v0.1.0.json) | Records the Rust subject's exact 35/35 result and request digest. | Reproduce the current external conformance claim. |
| [`docs/patterns`](./docs/patterns) | Defines the five integrity gates and their operating intent. | Design the larger agent-run lifecycle and review other systems. |
| [`docs/scorecard.md`](./docs/scorecard.md) | Lists evaluation criteria for governed autonomy. | Review a platform, internal harness, or vendor workflow. |

The Rust module is the implementation under test. The conformance protocol and
fixture corpus live in the separate
[`run-invariant`](https://github.com/nnennandukwe/run-invariant) repository.
GAAP does not import or vendor RunInvariant.

## Status

Implemented and tested now:

- a Rust `RunCoordinator` with one public evaluation interface;
- plan approval bound to the exact plan digest;
- deny precedence and explicit approval for risky or unknown actions;
- capability approval bound to an exact capability digest;
- independent, subject-bound, evidence-bearing verification;
- fail-closed usage accounting and bounded overage approval;
- separate mutation and completion authorization;
- a language-neutral RunInvariant process adapter;
- a 35/35 black-box conformance packet for the frozen protocol;
- provider-neutral Agent Run Request and Terminal Run Receipt Rust types;
- RFC 8785 canonical request and receipt-body digests;
- fail-closed policy, lifecycle, evidence, subject-freshness, and usage validation;
- provider-neutral Protected Effect Request and Result Rust types with typed
  filesystem, process, network, and external-service scopes;
- parent-bound request digests and sealed, replay-resistant result digests;
- fail-closed decision/status, observed-identity, drift, evidence, and
  non-execution invariants;
- generated Draft 2020-12 JSON Schemas; and
- Agent Run terminal examples plus completed/executed, denied,
  awaiting-authority, failed, interrupted, stale-subject, schema-drift, and
  unknown-outcome Protected Effect examples.

Not implemented yet:

- a bounded or open-ended model and tool execution loop;
- OpenAI, Anthropic, MCP, or other provider adapters;
- Protected Effect execution, retry orchestration, reconciliation, or
  persistence;
- repository mutation interception;
- approval persistence, revocation, or delegated authority;
- sandbox, budget-meter, verifier, or evidence-ledger adapters; and
- crash recovery or resumable run state.

The conformance packet proves normalized decision compatibility for the frozen
cases. It does not prove those missing runtime properties or certify GAAP for
production use.

## Build And Test

GAAP pins Rust `1.96.0`. Install Rust with `rustup`, then run:

```bash
git clone https://github.com/nnennandukwe/governed-agent-autonomy-patterns.git
cd governed-agent-autonomy-patterns
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked
cargo run --locked --example generate-contract-artifacts -- --check
```

Schemas and examples are registered in one typed catalog. To intentionally
reconcile all managed artifacts under `schemas/` and `examples/contracts/`, run
`cargo run --locked --example generate-contract-artifacts`, review the diff,
then rerun the command with `--check`.

Inspect the CLI:

```bash
cargo run --locked -- --help
```

## Reproduce External Conformance

RunInvariant `0.2.0` starts GAAP as an opaque child process. Clone both
repositories into the same parent directory:

```bash
git clone https://github.com/nnennandukwe/run-invariant.git
git clone https://github.com/nnennandukwe/governed-agent-autonomy-patterns.git
cd governed-agent-autonomy-patterns
cargo build --locked
cd ../run-invariant
node bin/run-invariant.js subject -- \
  ../governed-agent-autonomy-patterns/target/debug/gaap run-invariant
```

The expected bounded result is:

```text
Subject: gaap 0.1.0 (rust)
Subject conformance: 35/35 cases
```

RunInvariant sends case IDs, gate names, and normalized inputs without case
titles or expected decisions. GAAP rejects unknown subject schemas, decision
protocol digests, and fixture digests. Its response is bound to the exact input
bytes with `request_sha256`.

The committed packet is derived output. CI recomputes it using the pinned
RunInvariant commit and fails if the packet changes.

## Architecture Direction

The `RunCoordinator` is the deep module at the integrity seam. Both the
RunInvariant adapter and the future agent loop call the same interface; neither
provider-specific message types nor transport details enter the coordinator.

The next runtime slice must own lifecycle advancement around the coordinator:
plan, authorize mutation, execute through explicit adapters, account for
resources, obtain independent verification for the current artifact digest,
and authorize completion only if no later mutation invalidated that evidence.

Architecture decisions:

- [`0001: Own the agent loop`](./docs/adr/0001-own-the-agent-loop.md)
- [`0002: Use an external conformance seam`](./docs/adr/0002-run-invariant-process-seam.md)
- [`0003: Freeze the Agent Run contract as canonical evidence`](./docs/adr/0003-freeze-agent-run-contract.md)
- [`0004: Separate Protected Effect contracts from execution`](./docs/adr/0004-protected-effect-contract-boundary.md)

The normative contract guide is
[`Agent Run Contract 0.1.0`](./docs/contracts/agent-run-v0.1.md). Contract
receipts are content-addressed but not signed; they provide integrity and
interoperability, not producer authenticity.

The inner effect boundary is defined by
[`Protected Effect Contract 0.1.0`](./docs/contracts/protected-effect-v0.1.md).
Its validators check contract consistency; they do not replace
`RunCoordinator`, execute effects, or implement the bounded runtime.

## Pattern Library

The implementation is accompanied by material that can be used independently:

- [Plan](./docs/patterns/plan.md)
- [Permission](./docs/patterns/permission.md)
- [Tool trust](./docs/patterns/tool-trust.md)
- [Verification](./docs/patterns/verification.md)
- [Runtime accountability](./docs/patterns/runtime-accountability.md)
- [Governed publish pipeline](./docs/applications/governed-publish-pipeline.md)
- [Scorecard](./docs/scorecard.md)
- [Diagrams](./docs/diagrams.md)

The retired Node/TypeScript harness, BoundaryBench, and exploratory pilot remain
available from the
[`legacy-ts-boundarybench-v0.1.0`](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/tree/legacy-ts-boundarybench-v0.1.0)
tag. BoundaryBench's frozen conformance history now lives in RunInvariant.

## License

GAAP is licensed under the [Apache License 2.0](./LICENSE).
