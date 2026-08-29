# ADR 0001: Own The Agent Loop

Status: Accepted

## Decision

GAAP will implement its agent-run lifecycle in Rust rather than delegate tool
execution and lifecycle advancement to a provider-owned agent runtime.

The harness will own planning state, exact-subject authority, protected-effect
decisions, resource accounting, independent verification, completion
authorization, evidence, and recovery. Provider adapters will translate model
messages and tool requests only. They will not decide whether a protected
effect proceeds.

## Current Implementation State

The first Rust slice implements the `RunCoordinator` decision module and its
RunInvariant process adapter. It does not yet contain a model loop, tool
adapter, repository mutation path, sandbox, verifier, persistence, or crash
recovery.

Calling this slice conformant means only that its normalized decisions match
the frozen RunInvariant cases. It is not evidence that a complete agent loop is
present or governed.

## Consequences

- The same coordinator interface must be called by conformance tests and by
  the future runtime before protected effects.
- Provider-specific types must remain outside the coordinator.
- A provider adapter cannot advance lifecycle state or execute a tool without
  a coordinator decision.
- Completion requires current mutation authority and independent verification
  bound to the current artifact digest.
- Runtime work must add explicit evidence and recovery behavior rather than
  inheriting hidden retries or state from a provider SDK.
