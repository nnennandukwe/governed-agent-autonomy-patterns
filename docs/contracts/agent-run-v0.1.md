# Agent Run Contract 0.1.0

This contract defines the provider-neutral JSON boundary for starting one
governed Agent Run and recording its terminal result. It freezes evidence
shapes and validation rules only; it does not implement the model/tool loop,
adapters, persistence, recovery, or receipt signing.

## Contract Documents

- `gaap.agent-run-request/0.1.0` starts one Agent Run.
- `gaap.terminal-run-receipt/0.1.0` records one terminal outcome.
- The generated JSON Schemas use Draft 2020-12 and reject unknown fields.
- Any new field, state, event, evidence type, or validation rule requires a new
  contract version.

The Rust types and validators live in `gaap::contracts`. The generated schemas
live under `schemas/agent-run/v0.1.0`, and complete examples live under
`examples/contracts/v0.1.0`.

## Agent Run Request

An `AgentRunRequest` binds immutable run intent to:

- opaque request and run IDs;
- an exact repository or artifact locator and SHA-256 subject digest;
- a requested engineering capability name, version, and digest;
- task instructions and ordered textual constraints;
- one or more exact policy name/version/digest identities;
- safe-integer cost, elapsed-time, token, and tool-call budgets;
- existing approval evidence bound to actor, scope, and exact subject; and
- required independent verification evidence types.

`ContractSupport` is an exact allowlist of supported policy identities. It has
no wildcard and does not evaluate policy. `validate_request` rejects an
otherwise well-formed request when any identity is absent from that allowlist.
Approval context supplied with a request must target that request's exact
subject digest; approvals recorded later in the ledger remain bound to the
subject named by their own scope and digest.

## Lifecycle

The initial state is `accepted`. The ordered event ledger permits:

```text
accepted -> planning
planning -> awaiting_authority | executing
awaiting_authority -> planning | executing
executing -> awaiting_authority | verifying
verifying -> executing | completed
any non-terminal state -> blocked | failed | interrupted
terminal state -> no transition
```

`awaiting_authority` is resumable. `blocked`, `failed`, and `interrupted` are
distinct terminal outcomes. A terminal receipt's last event must transition to
the exact status declared by its body.

## Terminal Run Receipt

The envelope avoids a self-referential digest:

```json
{
  "receipt_digest": "sha256:<64 lowercase hex>",
  "body": {
    "schema_version": "gaap.terminal-run-receipt/0.1.0"
  }
}
```

The body binds the request digest, IDs, initial and resulting subject digests,
terminal status and reason, cumulative usage, and a contiguous event ledger
numbered from `1`. The closed event union records:

- lifecycle transitions;
- plan and approval evidence;
- `RunCoordinator` protected-effect decisions;
- authorized tool executions and observed mutations;
- independent verification;
- cumulative resource usage; and
- interruption evidence.

Evidence entries contain a closed evidence type, content digest, and optional
locator. Raw tool output and provider messages are not embedded.

Every `protected_effect_digest` in a decision, tool-execution, or mutation
event is the RFC 8785 canonical Protected Effect Request digest defined by the
separate `gaap.protected-effect-request/0.1.0` contract. The Terminal Run
Receipt remains authoritative for the ordered, multi-gate event chronology.

## Canonicalization And Digests

GAAP uses RFC 8785 JSON Canonicalization Scheme bytes and lowercase SHA-256:

```text
request_digest = SHA-256(JCS(AgentRunRequest))
receipt_digest = SHA-256(JCS(TerminalRunReceiptBody))
```

Protected Effect Request and Result digests are versioned separately and are
defined in [`Protected Effect Contract 0.1.0`](./protected-effect-v0.1.md).

Contract numbers are non-negative integers no larger than
`9_007_199_254_740_991`; floating-point values are absent. Array order remains
significant, including task constraints, policy identities, required evidence,
and receipt events.

Receipt digests detect mutation and provide content addressing. They do not
prove who produced a receipt; authenticity and signatures are later work.

## Completion Invariants

`seal_terminal_receipt` and `verify_terminal_receipt` require a
completed run to demonstrate all of the following:

1. Every tool execution references an earlier `permission` decision, and every
   mutation references an earlier `workflow` decision, with the same
   protected-effect and subject digests and an `allow` outcome.
2. `ask` and `block` never authorize an observed effect.
3. The resulting subject equals the last observed mutation result.
4. Passing verification comes from a different actor, carries all required
   evidence types, and targets the latest subject after its last mutation.
5. A later `workflow.completion_authorized` decision is bound to that same
   verified subject.
6. Final cumulative usage matches the receipt body and remains within the
   request budget.

A later mutation invalidates earlier verification for completion. Non-completed
receipts may preserve stale or failed evidence so operators can diagnose why
the run stopped.

## RunInvariant Is Separate

RunInvariant sends normalized decision cases to the `gaap run-invariant`
process interface and checks `RunCoordinator` conformance. An Agent Run Request
starts a future bounded runtime; a Terminal Run Receipt records a runtime
outcome. Neither contract is a RunInvariant request, response, fixture, or
Evidence Packet.

## Reproduce The Artifacts

```bash
cargo run --locked --example generate-contract-schemas -- --check
cargo run --locked --example generate-contract-examples -- --check
cargo run --locked --example generate-protected-effect-examples -- --check
cargo test --locked --test contracts --test schema_contracts
```
