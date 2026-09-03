# ADR 0005: Add A Bounded Agent Run Engine

Status: Accepted

## Decision

GAAP will expose a Rust-only bounded Agent Run runtime around
`RunCoordinator`. The runtime validates one `AgentRunRequest`, asks an
`AgentAdapter` for an approved plan and bounded next steps, evaluates every
protected effect through the coordinator, executes only fully allowed
`ProtectedEffectRequest` values through an `ExecutorPort`, invokes independent
verification through a `VerifierPort`, and seals exactly one
`TerminalRunReceipt`.

The runtime records each attempted or declined protected effect as a sealed
`ProtectedEffectResult`. It terminalizes one-shot `ask` outcomes as
`blocked`, without treating missing authority as an allow. It preserves
`failed`, `interrupted`, `blocked`, and `unknown_outcome` as distinct terminal
paths in the receipt ledger.

Tool-trust authority is runtime support metadata, not a new contract field.
`RuntimeSupport` carries exact trusted `CapabilityIdentity` values. When a
proposed effect uses one of those capabilities, the runtime translates that
support into the coordinator's capability-bound `ToolTrust` input shape. The
frozen Agent Run and Protected Effect `0.1.0` approval references remain
subject-bound evidence references.

## Consequences

- `RunCoordinator` remains the only gate decision authority.
- Contract validators remain consistency checkers; they do not grant runtime
  authority.
- The public runtime API is a Rust module first. No CLI, provider adapter,
  persistence, signing, sandbox implementation, MCP/ACP bridge, or ThreadLoop
  integration is introduced by this slice.
- In-memory deterministic adapters are enough to prove lifecycle behavior,
  protected-effect sealing, terminal receipt sealing, budget gating, and
  stale-verification rejection.
- Any later cross-language runtime boundary or persisted run state must be
  versioned separately from the frozen `0.1.0` contracts.
