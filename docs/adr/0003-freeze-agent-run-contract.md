# ADR 0003: Freeze The Agent Run Contract As Canonical Evidence

Status: Accepted

## Decision

GAAP will exchange Agent Run Requests and Terminal Run Receipts as provider-neutral, versioned JSON contracts. SHA-256 digests cover RFC 8785 canonical JSON; a Terminal Run Receipt envelopes a digest-bearing immutable body whose ordered, closed event ledger is the authority for lifecycle and subject freshness.

Policy support is an exact caller-supplied allowlist, not a second policy evaluator. Receipt evidence is content-addressed metadata rather than embedded provider output, and contract versions fail closed on unknown fields, identities, states, events, or evidence types.

## Consequences

- Equivalent JSON objects have the same digest across languages while event order remains significant.
- New fields, states, event types, evidence types, or validation rules require a new contract version.
- Receipt digests prove integrity and addressing, not signer authenticity.
- RunInvariant remains a separate process conformance protocol for `RunCoordinator` decisions.
