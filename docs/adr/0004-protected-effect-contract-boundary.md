# ADR 0004: Separate Protected Effect Contracts From Execution

Status: Accepted

## Decision

GAAP will represent each proposed protected effect and its observed outcome as
separate provider-neutral, versioned, immutable contracts. The request binds
the effect to its exact parent run, current subject, capability, policy,
budget, requested scopes, sandbox profile, normalized input digest, and
repeatability. The result envelopes a canonical body that binds the exact
request to the decisive `RunCoordinator` decision, observed identities,
execution status, usage, and content-addressed evidence.

Contract validation checks structural and identity consistency only.
`RunCoordinator` remains the sole decision authority. Effect execution,
cross-effect sequencing, persistence, retry, recovery, and reconciliation are
owned by the later bounded runtime, while the Terminal Run Receipt remains the
authority for full lifecycle chronology.

## Consequences

- Providers and transports can exchange effects without importing SDK types.
- An `ask` or `block` decision cannot be represented as an executed effect.
- Stale-subject and schema-drift denials preserve observations without
  implying that an executor ran.
- `unknown_outcome` remains distinct from failure, so a non-repeatable request
  requires reconciliation rather than automatic retry.
- Result digests provide tamper evidence and replay binding, not producer
  authenticity.
- New fields, operation families, statuses, evidence types, or repeatability
  classes require a new Protected Effect contract version and updates to every
  exhaustive consumer.
