# BoundaryBench Protocol

BoundaryBench protocol files are immutable after they are marked `frozen`.

The initial deterministic conformance release is
[`v0.1.0.json`](./v0.1.0.json). Its fixture corpus is
[`cases.v0.1.0.json`](../fixtures/cases.v0.1.0.json).

## Versioning Rule

- Editorial clarifications that do not change machine behavior belong in this
  README.
- Any change to an input field, decision outcome, precedence rule, decision
  code, workflow invariant, or evidence calculation requires a new protocol
  version and a new versioned fixture file.
- Existing frozen files remain available so prior evidence packets can be
  reproduced.
- The evidence packet records SHA-256 digests of the literal protocol and
  fixture bytes. Changing either frozen file invalidates `--check`.

Protocol `0.1.0` covers deterministic conformance only. It does not define a
model adapter, prompt, task corpus, sampling strategy, provider configuration,
or statistical claim for real coding agents.
