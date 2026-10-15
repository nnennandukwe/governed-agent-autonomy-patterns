# Contributing

GAAP is currently building the Rust harness one evidence-backed runtime slice
at a time.

## Focused Checks During Implementation

Use the smallest check that exercises the behavior being changed:

```bash
cargo test --locked --example generate-contract-artifacts
cargo run --locked --example generate-contract-artifacts -- --check
cargo test --locked --test contracts
cargo test --locked --test protected_effect_contracts
bash .github/scripts/test-select-ci-lanes.sh
```

The artifact command validates the complete typed catalog, renders every
dependency in memory, checks schemas and semantic expectations, and reconciles
the managed `schemas/` and `examples/contracts/` roots. Generation prunes
unregistered JSON files in those roots, so review its diff before committing.
`--check` is read-only and reports all detected drift.

## Final Review Gate

Run the complete current matrix once after the implementation is settled:

```bash
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked
cargo run --locked --example generate-contract-artifacts -- --check
```

The separately pinned MCP adapter requires Rust 1.88 or newer and Python 3 for its independent stdio test fixture.
Its dependency graph and SDK types stay outside the GAAP core crate. Run its matrix from the repository root:

```bash
cargo fmt --manifest-path adapters/mcp/Cargo.toml --check
cargo clippy --manifest-path adapters/mcp/Cargo.toml --locked --all-targets -- -D warnings
cargo test --manifest-path adapters/mcp/Cargo.toml --locked
cargo build --manifest-path adapters/mcp/Cargo.toml --locked
cargo audit --file adapters/mcp/Cargo.lock
```

CI installs `cargo-audit` 0.22.2 with its lockfile and requires the MCP adapter lane on every PR. Local dependency
verification uses that same version. See [the adapter guide](adapters/mcp/README.md) for its compatibility profile and
recovery rules.

The separate ACP adapter also requires Rust 1.88 and Python 3. Its required CI lane tests the host toolchain and 1.88:

```bash
cargo fmt --manifest-path adapters/acp/Cargo.toml --check
cargo clippy --manifest-path adapters/acp/Cargo.toml --locked --all-targets -- -D warnings
cargo test --manifest-path adapters/acp/Cargo.toml --locked
cargo +1.88.0 test --manifest-path adapters/acp/Cargo.toml --locked
cargo build --manifest-path adapters/acp/Cargo.toml --locked
cargo audit --file adapters/acp/Cargo.lock
```

The [ACP guide](adapters/acp/README.md) describes trusted host responsibilities, supported operations, evidence and
failure recovery. Root schemas and coordinator behavior remain the same; runtime adapters gain a default no-op `close`
hook that the engine invokes after every return, including errors. Executor ports also gain a default empty cancellation
query so a refused operation can retain its non-execution result while the run records an explicit interruption. Lost
executor responses count one known tool attempt; other usage stays a lower bound until reconciliation.

Changes to `RunCoordinator` decisions must also reproduce the committed packet
through RunInvariant commit `e3eb0dfb36390c32d2cd84bbbdf903f5dc55de44`.
Don't edit the conformance packet by hand or update it to make a failing
implementation appear conformant.

## Risk-Scoped CI

Every pull request and push to `main` runs formatting, Clippy, Rust tests, and a
build. The path classifier adds the contract lane for contract types, catalog,
schemas, examples, and their focused tests; it adds the coordinator lane for
the process adapter, coordinator tests, and RunInvariant evidence. Shared
toolchain, workflow, and public-root changes run both lanes. Unrelated
documentation runs only the baseline. Unknown paths, an unavailable diff, or a
classification failure fail closed by selecting both current specialized
lanes. The final CI job requires the baseline and every selected lane to pass.

Runtime changes must gain an executable lifecycle, recovery, replay, and
integration lane when issue #11 introduces runtime code and tests. A release
lane should be added only when broader conformance and evaluation suites exist;
release-boundary changes must then run it. These future obligations are policy,
not placeholder CI jobs.

## Interface Changes

- Keep provider and transport types outside `RunCoordinator`.
- Add new protected effects behind a deterministic coordinator decision.
- Treat unknown authority, evidence, usage, and schema versions as failures.
- Version the external subject protocol instead of adding unrecognized fields
  to an existing version.
- Version Agent Run contracts when adding or changing a field, lifecycle state,
  event type, evidence type, canonicalization rule, or validation rule.
- Version Protected Effect contracts independently when adding or changing a
  field, operation family, scope shape, execution status, evidence type,
  repeatability class, canonicalization rule, or validation rule.
- When a closed enum changes, update every exhaustive validator, schema,
  generator, example, test, and documentation consumer in the same change.
- Regenerate schemas and examples intentionally; never edit their digests or
  generated schema documents to hide contract drift.
- Keep implementation evidence separate from claims about a complete agent
  loop or production outcomes.

## Historical Implementation

The retired Node/TypeScript implementation is preserved at the
`legacy-ts-boundarybench-v0.1.0` tag. New changes should target the Rust harness
rather than restore the legacy workspace to `main`.
