# Operator reconciliation implementation Build Plan

## Summary

An interrupted shipping worker can leave the same dispatch record on either side of the target rename. A workspace operator needs to record what the enrolled host observer saw while preserving the original run. Implement slice 3 on the verified tree from PR #57. Qualify the change on disposable Linux ext4 slots. The maintainer accepted the v0.2 contract and authorized its implementation once that PR passed post-merge verification. [Decision](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/32#issuecomment-7071556507) · [Contract and eight qualification cases](../contracts/journal-reconciliation-v0.2.md)

## Skills To Use

Use software-build-plan, codebase-design and domain-modeling for the shared model, qodo-get-rules for compatibility and review rules, workflow-invariants and transaction-proof for ownership and publication, and failure-path-testing for independent process observations. Use cli-interactive-parity for the operator arguments and disclosure checks. Apply voice-canon to prose and both Qodo review skills to the implementation head.

## Scope

Implement explicit single-slot conversion, host-observer evidence admission and private historical readback for the accepted shipping executor. Keep the public contracts, earlier history, receipts, approval policies and ThreadLoop lifecycle ownership intact. Qualification uses only disposable local storage. A deployed conversion needs separate authorization.

## Package Layout / File-to-Task Mapping

The journal adapter owns format recognition, lineage, operator commands, private lookup, authority fences and recovery. Its storage module owns disk transitions. Its history module checks original attempts. Its recovery module checks independent evidence and supervisor handoffs. The shipping entry point participates in the writer lease and optional observation gate. The existing Linux qualification harness owns process interruption and raw artifacts. [Adapter](../../adapters/journal) · [Shipping path](../../examples/submitted-proposal)

## Dependencies And Settings

Use the enrolled private directories, pinned file identities, ordered ext4, a permanent slot lock and an independent health file. The supervisor also pins the host observer archive, current operator rights, authority lock and launch lease. A supervisor must preserve independent custody from before launch through the last possible commit, including a worker that outlives its launcher. The operator never provisions or manufactures these sources. Retain the contract's existing byte, count, canonical encoding and integer bounds.

## Canonical Contracts

The immutable origin authenticates the confirmed v0.1 prefix, both profiles and the initial v0.2 snapshot. Each later original fact or admitted ruling adds one generation. Only original workers can create facts or seal receipts. The configured host observer supplies raw evidence. Only a currently authorized operator can admit it. Lookup discloses one authenticated principal's matching attempt. Fresh dispatch requires a clear workspace and fresh authority.

## TDD And BDD Implementation Strategy

Start with the released CLI refusing both new commands. Enroll an empty disposable slot, convert it at generation zero and reopen it from a new process. Repeat on interrupted and sealed attempts. Drive the real shipping command through each side of its rename. Retain independent kernel and target observations before comparing the adapter's answer. Run each rejection through the same command a caller would use.

## Component Design

| Durable state before the action | Owner, action and commit point | What a new process may do |
| --- | --- | --- |
| Confirmed v0.1 pair at G | The authorized converter verifies exact source, quiescence, permanent locks, source history and bounds. Publish and confirm the cutover anchor before touching the journal. | Refuse old writers after the fence. Only an authorized converter can retry the same origin. |
| Confirmed cutover fence | Publish the initial journal with identical attempts at G, sync and verify it, then publish and confirm the active anchor at G. | Resume only an exact bound pair. An active repeat preserves later data. |
| Active v0.2, clear intake | The operator validates current rights, complete history and selector metadata, then writes and confirms the pending pair before reading the new source. | Readers retain private history. All fresh submissions stay held. |
| Active v0.2, pending pair | Reconstruct raw evidence, hold the current authority fence, append one ruling at G+1, sync the journal and confirm the anchor with pending intact. | A one-ahead journal stays held. A confirmed identical ruling retains its admitting principal. A successor can admit an uncommitted pair under their own rights. |
| Confirmed ruling or definitive rejection | Reconfirm the pair, check current rights and clear only that pending pointer with a confirmed anchor write. | Reconstruct all effects and any dispute. A dispute holds all future submissions. Neither operator command can execute or seal a run. |

Acquire the supervisor's exclusive writer-quiescence lease before the health and permanent slot locks. Hold all three across conversion or evidence intake. Take the current authority read fence inside that protected section at each decision. Live workers use the corresponding shared writer lease before taking storage locks. An authority writer serializes revocation against decision publication. Decode the entire original history and insert each validated ruling at its whole-attempt fact frontier before admitting later work.

## Failure And Recovery Rules

| Operation and interruption | Evidence and required response |
| --- | --- |
| Stage create, write, flush, sync, checked close, identity check, rename and directory sync | Retain exclusive stage identity, current pair, pending pointer and independent health. Preserve foreign stages. Fail closed on uncertain publication or failed health control. |
| Converter dies around the fence, new journal or active anchor | Compare both files, their directories, health and origin against the verified source. Retry only an exact authorized conversion. |
| Intake dies or authority changes before a verdict | Reopen the exact pointer and verify current operator rights. Missing source, partial access, an active writer or uncertain health keeps the hold. An intact insufficient source permits a definitive rejection. |
| Ruling exceeds capacity, loses its reply or conflicts with earlier observations | Keep evidence and pending across capacity failure. A confirmed repeat appends nothing. Preserve every contradictory ruling, any intervening authorized attempt and all original seals. |
| Observer stops early, misses a bound move, or can't prove the writer's definitive stop | Withhold a ruling. Matching file contents alone can't establish execution or non-entry. Retain independent before and after identities and bytes. |

## Commit Plan

Commit this model before production edits. Implement the versioned storage path, source admission and historical projection as one coherent slice. Record process fixtures, working commands and their proof limits. Keep qualification outputs tied to the tested revisions.

## Branch And PR Flow

Use a fresh isolated branch from the verified merge of PR #57. Review the complete local diff at the intended commit, check the completed hosted review at that same head, assess findings and run required CI. Guard the merge with its reviewed SHA, compare the merged tree and verify main with its own evidence artifact.

## Test Plan

Run the eight process cases in the accepted contract on Linux with the actual v0.1 reader and shipping writer from the released tree. Cover old and new lock contenders, every conversion publication boundary, intake failure and restart, real rights changes, an independently stopped worker, all three supported outcomes, duplicates, contradictory observer deposits, immutable seals, loss of pinned evidence, safe bounds and private lookup. Independently compare canonical files, generations, health, raw target events, retained source bytes and receipt seals. Retain existing slice 1 and slice 2 qualification and frozen contract and model checks. Label deliberately conflicting observer reports as equivocation fixtures.

## Definition Of Done

Each contract row has a process result and independent observations tied to the reviewed revision. The journal reconstructs the original history, pending state, ruling timeline and workspace hold after restart. Both review gates, all required PR jobs and main checks pass on their respective revisions. Reconcile issue #32, its parent and the program inventory against that evidence.

## Assumptions And Defaults

Only one cooperating foreground shipping executor can commit in the enrolled workspace. Process interruption leaves the kernel running. Hardware power loss, hostile same-user writers, deployed conversion, additional observers and approval continuation keep their existing boundaries.
