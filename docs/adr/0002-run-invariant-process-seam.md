# ADR 0002: Use An External Conformance Seam

Status: Accepted

## Context

GAAP needs conformance evidence that does not make the test kit depend on the
Rust crate or make the harness depend on the test kit's JavaScript evaluator.
An in-process test adapter would couple language, package manager, and release
lifecycle.

## Decision

GAAP implements RunInvariant subject protocol `0.1.0` as a CLI adapter.
RunInvariant starts the binary directly, sends one versioned JSON request on
stdin, and reads one versioned JSON response from stdout.

The request contains normalized inputs but not expected decisions. GAAP pins
the supported decision-protocol and fixture digests, evaluates each case
through `RunCoordinator`, and binds the response to the exact request bytes.

## Consequences

- RunInvariant can test GAAP without importing Rust code.
- GAAP can evolve its internal implementation without changing the process
  interface.
- Other languages can implement the same subject protocol.
- Process, schema, or validation changes require a new subject-protocol
  version.
- Passing conformance covers normalized decisions only; integration and
  failure-path claims require separate evidence.
