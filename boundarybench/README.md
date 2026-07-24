# BoundaryBench

BoundaryBench is a small executable example of the five governed-autonomy gates
in this repository. Protocol `0.1.0` is deliberately narrow: it tests
deterministic decision records, not model behavior.

The conformance layer answers two reviewable questions:

1. Does the reference evaluator return the exact frozen decision for each
   public boundary case?
2. Can the same fixture corpus detect deliberately unsafe implementations of
   each gate?

## Run It

BoundaryBench has no package dependencies, credentials, network calls, or
environment variables. It requires Node.js 20 or newer.

```bash
npm test
npm run boundarybench
node boundarybench/bin/boundarybench.js --json
```

Use the write command only when intentionally refreshing evidence for a new
implementation or protocol:

```bash
npm run boundarybench:update
```

`npm run boundarybench` recomputes the packet and fails if it does not match
[`evidence/conformance-v0.1.0.json`](./evidence/conformance-v0.1.0.json).

## What Is Frozen

[`protocol/v0.1.0.json`](./protocol/v0.1.0.json) defines:

- `allow`, `ask`, and `block`, with `block` taking highest precedence
- exact-subject approval for plans, actions, capabilities, and budget
- deny and hard-stop decisions that approval cannot override
- independent, subject-bound, evidence-bearing verification
- fail-closed behavior when authority or usage is unknown
- separate mutation and completion authorization

[`fixtures/cases.v0.1.0.json`](./fixtures/cases.v0.1.0.json) supplies the
canonical examples. The evidence packet includes SHA-256 digests for both
files, so a frozen-input change is visible immediately.

## How To Read The Data

Reference conformance is the share of frozen cases where the evaluator’s full
decision matches the expected outcome, reason code, and effects.

Mutation score is the share of deliberately unsafe implementations detected by
at least one public case. Version `0.1.0` includes one mutant for each gate:

- skip exact plan approval
- allow an unknown action
- trust a changed capability by display name
- allow implementer self-verification
- continue when usage data is missing

A perfect result is useful but bounded. It proves that this implementation
conforms to this deterministic protocol and that the fixture corpus is
sensitive to those five named regressions. It does not show that an open-ended
coding agent follows the gates, that the gates improve production outcomes, or
that token telemetry equals invoice cost.

## Experimental Layer

The repository now includes a separate
[`experiment`](./experiment/README.md) package and a
[`governed coding-agent harness`](../examples/governed-coding-agent/README.md).
The harness reuses this frozen evaluator, while the experiment adds real model
adapters, a five-task corpus, controlled challenges, one-gate record-only
conditions, evidence receipts, and claim-gated reporting.

That layer does not change protocol `0.1.0`. Deterministic conformance and
real-agent outcomes remain separate results. No live pilot result has been
published; any future aggregate report must come from a clean commit and an
immutable manifest.
