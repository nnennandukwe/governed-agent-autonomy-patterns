# Contributing

GAAP is currently building the Rust harness one evidence-backed runtime slice
at a time.

## Before Opening A Pull Request

Run:

```bash
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked
```

Changes to `RunCoordinator` decisions must also be tested through the pinned
RunInvariant process interface. Do not edit the committed conformance packet by
hand or update it to make a failing implementation appear conformant.

## Interface Changes

- Keep provider and transport types outside `RunCoordinator`.
- Add new protected effects behind a deterministic coordinator decision.
- Treat unknown authority, evidence, usage, and schema versions as failures.
- Version the external subject protocol instead of adding unrecognized fields
  to an existing version.
- Keep implementation evidence separate from claims about a complete agent
  loop or production outcomes.

## Historical Implementation

The retired Node/TypeScript implementation is preserved at the
`legacy-ts-boundarybench-v0.1.0` tag. New changes should target the Rust harness
rather than restore the legacy workspace to `main`.
