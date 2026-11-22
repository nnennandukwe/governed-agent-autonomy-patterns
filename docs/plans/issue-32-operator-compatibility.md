# Operator reconciliation compatibility Build Plan

## Summary

A worker can die after replacing a file and before reporting the effect. The owner approved a workspace-scoped operator and the configured host observer as the only witness source. This build will propose how the journal can return an operator ruling alongside the original history and keep every writer behind a disputed or uncertain effect. Start from `209b87cf6f6feaaaaba06bb1756825920b8c1cb2`, where the foreground executor passed post-merge verification. One design PR will prepare the format decision. [Owner decision](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/32#issuecomment-7071337244) · [Slice 2 verification](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/32#issuecomment-7071309464)

## Skills To Use

| Stage | Skill and expected result |
| --- | --- |
| Plan and architecture | Use software-build-plan, codebase-design and domain-modeling to identify who can witness, admit evidence, seal a run and start a new attempt. |
| Contract | Use qodo-get-rules for compatibility rules. Inspect the producer and every reader affected by new storage tags and status fields. Keep existing public contract names and response fields. |
| State and storage | Use workflow-invariants and transaction-proof for the exclusive lock, two-file publication, old-writer fence, independently retained evidence and durable dispute. |
| Public behavior | Use cli-interactive-parity for operator and read-only command semantics. Use failure-path-testing for the old-client checks and future process interruption cases. |
| Review | Apply voice-canon to prose. The remote base is available for qodo-review on the complete local diff. Use qodo-review-resolver to assess the hosted review at the same PR head. |

## Scope

Design one command for the designated workspace operator and a versioned journal format for one cooperating Linux ext4 shipping workspace. Cover both intact existing history and a fresh slot. The accepted host-observer policy governs admission. The proposed disk transition needs an owner decision before a runtime can implement it. The existing deployment and frozen public contracts retain their present behavior. Approval continuation, additional witnesses, recovery of damaged storage and ThreadLoop lifecycle transitions keep their existing owners and boundaries.

## Package Layout / File-to-Task Mapping

The proposed [reconciliation contract](../contracts/journal-reconciliation-v0.2.md) will specify storage, commands and acceptance cases. The [ADR](../adr/0009-durable-agent-run-reconciliation.md) and [program checklist](gaap-program.md) will identify the accepted source policy and the open format decision. Check the design against the [storage reader and lock](../../adapters/journal/src/storage.rs), [fresh admission](../../adapters/journal/src/storage/writer.rs), [history validator](../../adapters/journal/src/history.rs) and [trusted archive](../../adapters/journal/src/live/archive.rs). Product code belongs to the later implementation.

## Dependencies And Settings

Use the enrolled private workspace, permanent lock, independent health authority, retained historical profile and configured observer. The design inherits the ext4 profile, canonical encoding and storage bounds. Use the existing toolchain and offline evidence. The proposed format has its own version.

## Canonical Contracts

Preserve identifiers, canonical attempts, partial records, original sealed results and receipts. Historical key lookup stays principal-scoped. Rulings describe an effect. Only the live engine can seal its run. Any unresolved effect or authenticated dispute blocks fresh dispatch for every principal and key. A host operator can't grant execution authority by recording evidence.

## TDD And BDD Implementation Strategy

Use the confirmed v0.1 pair and released reader as the starting example. Try the released reader, enrollment and submission entry points against incompatible state. Record which behavior can be tested today. Describe a future two-version process test for each proposed transition. Use original, independently observed target states as the oracle for positive, negative and unknown outcomes.

## Component Design

The trusted supervisor authenticates the operator and the source, supplies retained evidence and establishes that the original writer can't commit again. The journal validates all original attempts and later rulings under one permanent lock before answering a key or admitting fresh work. A versioned cutover anchor must exclude old writers before new state can appear. A ruling and any dispute must survive a lost reply. A match on current bytes, an expired lease or a caller's claim can't settle an invocation.

## Failure And Recovery Rules

Cover interruption before and after each snapshot and anchor replacement, a queued old writer, mismatched generations, unsupported versions, full storage and volume health holds. Specify custody when evidence is incomplete or contradictory. An exact retry must recover the original outcome. A late dispute must stop future work and preserve intervening legitimate history. State what an operator can inspect or retry under the permanent lock after each stop.

## Commit Plan

Commit the design plan, then the proposed storage contract, ADR cross-reference and checklist. Retain any supported review repairs in the same branch.

## Branch And PR Flow

Use the isolated documentation branch from the verified main revision. Run the linked checks, complete-diff Qodo Local Review and exact-head hosted review. Assess findings against v0.1 source, retained observations and the proposed state transitions. Tie CI and merge to the reviewed head, compare the merged tree and verify post-merge checks before updating #32.

## Test Plan

Run the 19 ADR recovery-model cases, frozen contract checks and applicable repository checks. Use released-client readback and the slice 2 ext4 packet to assess the current compatibility boundary. Validate document links, command names and language. Future process qualification must cover authentication, durable cutover, concurrent mixed-version operators, accepted and rejected evidence, restart, and the hold after a conflict.

## Definition Of Done

The owner can accept, request changes to or decline a specified storage transition. The contract identifies the allowed authenticated evidence and proves how the proposed format would keep old writers from observing a clear workspace while a dispute exists. Identify precisely which claims have process evidence and which still require implementation. A merged design remains proposed until its owner accepts it.

## Assumptions And Defaults

Use one host and one cooperating foreground shipping writer. Read only the original configured observer’s retained evidence. Verify original writer quiescence independently under the accepted host profile. Preserve prior storage evidence across any administrative operation.
