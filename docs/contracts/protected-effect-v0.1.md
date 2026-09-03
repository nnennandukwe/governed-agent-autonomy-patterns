# Protected Effect Contract 0.1.0

This contract defines the provider-neutral JSON boundary for proposing one
protected effect inside an Agent Run and recording whether it was declined,
attempted, or completed. It defines immutable data and consistency validation;
it does not execute an effect, evaluate policy, grant authority, persist state,
retry work, reconcile an unknown outcome, or enforce a sandbox.

## Contract Documents

- `gaap.protected-effect-request/0.1.0` describes one proposed effect.
- `gaap.protected-effect-result/0.1.0` records the decisive coordinator
  decision and the observed execution outcome.
- The generated JSON Schemas use Draft 2020-12 and reject unknown fields.
- The Rust types, parsers, validators, canonicalizers, sealing function, and
  verifier are exposed through `gaap::contracts`.

The generated schemas live under `schemas/protected-effect/v0.1.0`. A request
and eight digest-valid result scenarios live under
`examples/contracts/protected-effect/v0.1.0`.

## Protected Effect Request

A `ProtectedEffectRequest` binds a proposed effect to:

- an effect ID and one-based JCS-safe sequence within one Agent Run;
- the run ID and exact canonical Agent Run Request digest;
- the current proposed repository or artifact subject;
- a closed operation family and lower-snake dotted operation name;
- the exact capability and optional tool-schema digest;
- a digest of normalized provider input and bounded non-secret metadata;
- typed filesystem, process, network, or external-service scopes;
- the parent run's exact policy identities and resource-budget digest;
- approval evidence identities, a sandbox profile, and an idempotency key; and
- closed repeatability and expected effect classifications.

The expected effect classes are `observation` and `mutation`.

The subject is the proposed current subject. It may differ from the parent
Agent Run Request's initial subject after an earlier mutation. The effect
validator does not decide whether that change is authorized; the result must
record what was observed immediately before execution.

`effect_sequence` establishes identity and position, not cross-effect
ordering. The bounded runtime defined after this contract is responsible for
contiguous sequencing, persistence, and recovery.

### Input And Metadata

Provider input is normalized outside the contract and is not embedded:

```text
input_digest = SHA-256(JCS(normalized_input))
```

`input_metadata` may contain at most 32 entries. Names are unique values of
1–64 characters; values are 1–256 characters. Producers must
not put secrets or raw provider input in metadata. The validator can enforce
shape and bounds, but it cannot infer whether arbitrary text is sensitive.

### Operations And Scopes

The closed operation families are `filesystem`, `process`, `network`, and
`external_service`. A normalized operation begins with its family and uses
lower-snake dotted segments, such as `process.spawn`.

Requested scopes are a tagged union:

```json
{"scope_type":"filesystem","root":"/workspace","access":["read","modify"],"recursive":true}
{"scope_type":"process","executable":"/usr/bin/cargo","working_directory":"/workspace"}
{"scope_type":"network","protocol":"https","host":"crates.io","port":443}
{"scope_type":"external_service","service":"github","operation":"pull_request.comment","resource":"nnennandukwe/repo#22"}
```

A scope describes the authority being requested; it never grants that
authority by itself. Paths remain platform-neutral strings in this contract;
the future executor is responsible for resolving them against its environment
and enforcing the requested boundary.

Filesystem access classes are `read`, `create`, `modify`, and `delete`.
Network protocols are `tcp`, `udp`, `http`, and `https`.

### Parent Binding And Authority

The run ID, Agent Run Request digest, capability, policy identities, and
resource-budget digest must exactly match a validated `AgentRunRequest`.
Effect-specific approval references identify evidence bound to the Protected
Effect Request's current subject digest. They do not authorize execution by
themselves.

`RunCoordinator` remains the sole decision authority. The contract validator
does not recompute policy or reinterpret a decision code.

### Repeatability

The closed repeatability classes are:

- `repeatable`: an identical request is declared safe to repeat;
- `idempotent`: repetition is expected to converge on the same effect; and
- `non_repeatable`: automatic retry is unsafe without reconciliation.

These values are declarations for a future runtime. This contract performs no
retry. In particular, `non_repeatable` and `unknown_outcome` remain separate so
the runtime cannot turn uncertainty into an ordinary retryable failure.

## Protected Effect Result

The result uses a digest-bearing envelope:

```json
{
  "result_digest": "sha256:<64 lowercase hex>",
  "body": {
    "schema_version": "gaap.protected-effect-result/0.1.0"
  }
}
```

The body binds the effect and run identities, both request digests, observed
pre-effect identities, the decisive `RunCoordinator` decision, execution
status, optional post-effect subject and process exit, effect-local usage, optional executor
and sandbox identities, a reason, and typed evidence references.

The decisive decision record contains its ID, gate, exact Protected Effect
Request digest, observed subject digest, and the existing `Decision`. Full
multi-gate chronology remains in the Terminal Run Receipt event ledger.
An attempted effect requires an `allow` decision from the `permission` gate;
an `allow` from another gate cannot authorize execution.

### Decision And Execution Matrix

| Execution status | Required consistency |
| --- | --- |
| `executed` | `allow`; exact observed request identities; post-effect subject, executor, sandbox, usage, and subject-observation evidence |
| `awaiting_authority` | `ask`; non-empty reason; zero usage and no execution-derived fields or evidence |
| `denied` | `block`; non-empty reason; zero usage and no execution-derived fields or evidence |
| `failed` | `allow`; known attempted failure with a post-effect subject, executor, sandbox, usage, and failure evidence |
| `interrupted` | `allow`; known interrupted attempt with a post-effect subject, executor, sandbox, usage, and interruption evidence |
| `unknown_outcome` | `allow`; dispatched but unreconciled attempt with executor, sandbox, last-known usage, reason, and unknown-outcome evidence; post-effect subject may be absent |

`ask` and `block` never coexist with execution-derived state. Attempted statuses
require content-addressed executor, sandbox, and usage evidence. Contradictory
failure, interruption, and unknown-outcome evidence is rejected.

An observation result with a known post-effect subject preserves the subject
digest. A mutation result with a known post-effect subject requires
both mutation and artifact evidence even if the resulting digest is unchanged.
Known `process` execution and failure records contain exactly one exit code or
signal plus exit evidence. Non-process results cannot contain an exit status or
exit evidence.

Process exits are a tagged union:

```json
{"exit_type":"code","code":0}
{"exit_type":"signal","signal":"SIGTERM"}
```

### Drift Denials

Subject or capability-schema drift is evidence that the requested identities
were no longer current, not evidence that execution occurred. A valid drift
result therefore uses `denied`, a `block` decision, zero usage, no executor or
sandbox result, and the requested-versus-observed identities already present
in the linked request and result.

The precise reason values and required evidence are:

| Drift | Reason | Evidence |
| --- | --- | --- |
| Subject only | `protected_effect.stale_subject` | `subject_observation` |
| Tool schema only | `protected_effect.capability_schema_drift` | `capability_schema` |
| Both | `protected_effect.subject_and_capability_schema_drift` | both evidence types |

The observed capability identity still matches the request. A subject or
tool-schema drift mismatch paired with any status other than `denied` fails
closed.

### Evidence

Effect evidence types are closed to `exit`, `output`, `artifact`, `mutation`,
`usage`, `executor`, `sandbox`, `failure`, `interruption`,
`subject_observation`, `capability_schema`, and `unknown_outcome`.

Each reference contains only its type, a SHA-256 digest, and an optional
locator. There is no raw-output, provider-message, or secret field. These
effect-specific evidence types do not expand the frozen Agent Run 0.1.0
evidence vocabulary.

## Canonicalization, Integrity, And Replay

GAAP uses RFC 8785 JSON Canonicalization Scheme bytes and lowercase SHA-256:

```text
effect_request_digest = SHA-256(JCS(ProtectedEffectRequest))
result_digest = SHA-256(JCS(ProtectedEffectResultBody))
```

Result verification checks the envelope digest before validating the body
against the exact parent Agent Run and Protected Effect Request. Cross-run,
cross-request, effect identity, sequence, decision digest, subject, capability,
or sandbox mismatches fail closed. A modified result body returns the distinct
`result_tampering` error.

The digests provide integrity and content addressing. They do not prove who
produced the values; signing and producer authenticity are later work.

## Public Rust Interface

The `gaap::contracts::protected_effect` implementation is exposed as one
contract interface through `gaap::contracts`:

- strict request and result parsing;
- RFC 8785 request and result-body canonicalization;
- parent-bound request and result-body validation;
- result sealing and verification; and
- generated request and result JSON Schemas.

All operations are pure over caller-supplied values. They perform no protected
effect and no persistence or network I/O.

## Relationship To Agent Run 0.1.0

Every `protected_effect_digest` in an Agent Run 0.1.0 decision, execution, or
mutation event is the canonical Protected Effect Request digest defined here.
The existing Agent Run Rust interfaces, schemas, examples, evidence enum, and
`RunCoordinator` decisions remain unchanged.

The Terminal Run Receipt remains authoritative for ordered lifecycle and
multi-gate chronology. The bounded runtime will connect these contracts to that
ledger in a later phase.

## Reproduce The Artifacts

```bash
cargo run --locked --example generate-contract-artifacts -- --check
cargo test --locked --test protected_effect_contracts
```
