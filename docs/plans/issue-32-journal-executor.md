# Journal-bound shipping executor Build Plan

## Summary

A lost connection can hide a completed file replacement. A second submission must recover the original history, even when the first worker died between replacing the file and publishing its receipt. Integrate the one foreground shipping worker with the enrolled journal. The prerequisites are the [accepted storage contract](../contracts/journal-enrollment-v0.1.md), [slice 1](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/pull/54) and the [writer qualification](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/pull/55). The maintainer authorized this integration after post-merge verification. [Decision in #32](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/32#issuecomment-7070625599)

## Skills To Use

Use Qodo's project rules to pin changed dependencies and keep command responses compatible. Use codebase-design and domain-modeling to keep admission, effect evidence and the engine's completion decision with their owners. Apply workflow-invariants, transaction-proof and failure-path-testing to the transition and fault tables below. Use cli-interactive-parity for the shipping CLI, help and launcher examples. Run Qodo Local Review on the full diff and qodo-review-resolver on the reviewed PR head. Apply voice-canon to the guide and decision record.

## Scope

The enrolled Linux ext4 workspace has a single foreground executor for shipping.py. The trusted supervisor admits bounded non-secret proposals, supplies current authority and retains evidence independently. Existing enrollment, lookup and legacy reference submissions keep their existing formats and commands. The live engine alone can seal an attempt. Operator rulings, additional executors, approval continuation and policy migration belong to later decisions.

## Package Layout / File-to-Task Mapping

- The [journal adapter](../../adapters/journal/src/storage.rs) owns the pinned workspace, health lock, permanent slot lock, confirmed generation and append-only transaction. Its existing read validator checks every candidate generation before publication and again on reopen.
- The submitted-proposal reference owns the new foreground command, the shipping proposal decoder, its existing file replacement and independent verifier. Share its production implementation across both entry points. A journal-only workspace has no legacy session writer.
- The [core runtime](../../src/runtime.rs) exposes the four actual allowed gate events before entering the executor and the actual verification event when the engine records it. Keep local storage, policy transport and file operations outside the core.
- The trusted host supplies current grants and provides the independent evidence archive. Its witness observes the invocation through the target commit. The integration suite uses separate processes for the caller, witness and inspector.
- The journal qualification script and CI retain the production and fault-build hashes, revision, filesystem, observed file identities, journal/anchor digests, process status, sealed objects and retry outcomes.

## Dependencies And Settings

Use the accepted bounds, lock ordering and storage profile. Align the shipping crate's exact JSON dependency with the journal library and verify its lockfile. Keep the core's minimum Rust version and the shipping reference's Linux-only journal dependency. Load the worker's descriptors from a trusted launcher, mark inherited originals close-on-exec before starting a verifier and close unused copies. Grant and witness channels have bounded reads and deadlines.

## Canonical Contracts

Each append increments the generation once. It preserves all earlier admissions, facts and IDs, and binds each new fact to the original requests, principal, key and canonical proposal. Check capacity before mutation. A keyed conflict or duplicate returns before allocating an identity or budget. Current read permission governs historical delivery. Current execution permission and a clear workspace govern fresh admission. Receipt and effect-result envelopes are the original engine outputs.

| Starting state | Action under the permanent lock | Next observable state |
| --- | --- | --- |
| Confirmed empty or usable history | Validate every tuple, current read rights, caller/key/bytes and workspace hold. For an eligible miss, verify current grant and subject and create the intent. | The journal and independently confirmed anchor retain the original proposal and requests. A stopped intent has no dispatch. |
| Confirmed intent | Run the actual engine. Preserve an early engine refusal, or archive current grant checks and publish dispatch with the four original allow events, measured usage and reservation. | A refusal can be terminal. A dispatch holds every new key pending independent effect evidence. |
| Confirmed dispatch | Stage exact bytes, check permissions, sync and recheck identity, subject, policy, approval, budget, deadline and cancellation. Rename the stage over shipping.py under the fence. | Rename makes new target bytes visible. A missing post-effect record leaves dispatch as the last confirmed journal fact. |
| Executor returned | Archive the invocation-bound witness and executor evidence. Publish the observed disposition and usage floor. | A witnessed write can survive failure. Missing or contradictory evidence leaves the outcome unresolved. |
| Confirmed observation | Run the independent verifier on the observed subject. Archive its actual evidence and original engine event before publishing verified. Recheck the live subject and completion authority. | An observed write and a failed or incomplete verification remain separately readable. |
| Live engine returns sealed objects | Validate both envelopes against the full retained history. Sync and rename the new journal snapshot, sync its directory, replace and sync the confirming anchor, and reread both. | The journal rename publishes the receipt. Confirmed lookup reports its original sealed bytes. A stopped writer never resumes sealing. |
| Response or cleanup | Return the confirmed result and release the lock. Remove only positively identified stages. | Lost delivery can't spend another reservation. Preserve the visible effect, original receipt and any cleanup warning. |

## TDD And BDD Implementation Strategy

Start with the real journal-submission command and a separately launched lookup. Confirm the new behavior fails on the base. Prove one completed run, its exact duplicate and a changed same-key conflict. Then exercise the failure windows, refusal, historical access after current execution authority changes and competing fresh keys. Replay the existing slice 1, reference and runtime suites.

## Component Design

Acquire the independent volume-health lock before the permanent journal lock and hold both for the admission, single effect and confirmed publication. A new worker waits for that same lock. Only the foreground worker has a file-commit path. Any verifier receives just its bounded input. Journal callbacks take a borrowed writer guard and can't reopen the same slot or allocate a second attempt. Every fresh submission validates the current journal, anchor, ownership and health. A journal one generation ahead yields unconfirmed even if it contains a receipt. A quarantined or pending volume withholds keyed history.

The reader refreshes its independently pinned archive under the slot lock and validates the full v0.1 fact graph. This prevents a reader that started early from using a stale archive against a newer confirmed journal. A writer constructs a candidate by appending one fact to its current attempt or adding one new intent, validates it with independently retained evidence, publishes it to the pinned prior journal, and confirms the new generation in the anchor. Only first enrollment can create an absent journal. The live worker serializes authority revocation against the final check and rename. After process exit the kernel releases its locks. An invocation with a surviving dispatch and insufficient evidence stays held even after the writer has exited.

The [shipping CLI](../../examples/submitted-proposal/src/main.rs) accepts `journal-submit KEY` and the bounded proposal on stdin. The trusted launcher supplies configuration and health separately from a live execution handoff. Read from the independently pinned workspace and use the original shipping snapshot representation to compute the repository subject. The local file identity and raw content digest identify the target within that snapshot. The journal-only shipping workspace has its own positive host provisioning record. Legacy commands must have no write path to it.

Use a separate stable authority lock and bounded current authority record outside the journal volume. A trusted operator updates grants and cancellation under the exclusive authority lock without acquiring the worker's long-lived workspace or volume lock. Reopen current authority by name under the lock to observe atomic replacements. The worker takes a shared authority lock through its last check and actual rename. Bind the grant to the current principal, slot, proposal, profile, subject, action and resource limits before constructing the run request. An expired or revoked approval can't become a working grant from its saved historical bytes. Include the real host ApprovalReference in the engine request before computing public digests.

Start the independent observer before a possible target commit. Under sole-writer exclusion, correlate the stage and target directory events by move cookie, compare the pinned stage to the resulting target inode and hash the replacement. A matching current file alone can't identify this invocation. An overflow, loss of the witness or ambiguous event keeps the outcome open. Store original grant checks, the observation, execution evidence, verifier output and invocation-bound verifier event in a bounded, independently provisioned private archive outside the control, journal and workspace directories. The archive shares the qualified ext4 device and its independent health latch. Authority updates use separate host storage. Archive and sync each record before publishing a reference to it. An orphaned archive record after a failure grants no dispatch authority. A fresh supervisor repopulates the historical index from durable trusted records even when the worker died before delivering its response.

## Failure And Recovery Rules

| Operation or injected fault | Required outside observation and recovery |
| --- | --- |
| Authenticate, decode, inspect current authority, acquire the stable locks or validate the whole store | Denied, malformed, timed-out, unprovisioned, damaged, stale and full cases create no admission or write. A duplicate reveals only the authorized principal's retained history. |
| Stage a fact or anchor, observe collisions, short write, permissions, flush, fsync, checked close or capacity error | Recheck identity and preserve unowned stages. Repeat lookup sees the old confirmed generation, a named conflict, unconfirmed generation or health hold. A failed dispatch confirmation prevents executor entry. |
| Process stops around journal and anchor renames or directory sync | Independently hash the surviving pair and verify its ordinary reader response. Never promote a one-ahead snapshot on retry. A kernel I/O error or writeback uncertainty quarantines the volume. |
| Authority expires, grant is revoked, subject or policy changes, budget is exhausted, or cancellation wins | Recheck at admission and immediately before rename. A concurrent update participates in the same authority fence. The observer records whether this invocation ever reached a target commit. |
| Target staging and replacement, before and after each commit, target-directory sync failure | Observe stage ownership, target bytes, inode, kernel move count and worker identity outside the process. Two histories can have identical surviving dispatch facts and different physical outcomes. Both refuse replay. A target-volume fault invokes the independent health hold. |
| Witness or verifier unavailable, contradictory, missing bytes, invalid evidence, changed subject, failing verdict | Historical validation fails closed on missing required references. A live unknown holds new dispatch. A known write can coexist with a failed run. A stopped unsealed run stays unsealed. |
| Lost reply, errored cleanup, duplicate/restart and concurrent callers | No additional rename or budget allocation on duplicate. A receipt remains immutable. A different principal's key can't expose the old record or bypass a workspace hold. A failed cleanup preserves foreign state and reports its own outcome. |

## Commit Plan

Keep the shared engine events, journal transaction, foreground executor, trusted host, qualification and guide in one slice 2 PR. Commit the shared state model before production changes and retain the regression evidence for review repairs.

## Branch And PR Flow

Use a fresh worktree from verified main. Run complete-diff local review at the final commit, hosted review at the same PR head and required CI. Recheck head and base before guarded merge. Verify the merged tree and inspect the post-merge evidence packet before reconciling issue #32.

## Test Plan

Run the frozen contract and state-machine suites, reference CLI suite, journal enrollment/lookup qualification and the integrated host/worker/observer tests on Linux ext4. Exercise normal execution, failed verification, an early refusal, genuine process interruption, current-authority loss, target and receipt sync failures, duplicate delivery, cleanup, lost response and restarted inspection. Record the exact binary and head independently of the CLI's self-report. Inspect both sealed envelopes and compare actual target operations with the observer's history.

## Definition Of Done

The ordinary command performs the single authorized replacement and yields the exact engine receipt through a fresh lookup. Independent observations separate the effect, verification, terminal receipt and confirmed delivery. Every uncertain effect prevents fresh dispatch across keys and principals. Existing enrollment/lookup and shipping reference suites pass, with PR and post-merge reviews and checks tied to their commits. Process tests on a running kernel establish the stated interruption outcomes.

## Assumptions And Defaults

The host authenticates service roles and controls all commit-capable writers on the enrolled private workspace. An operator preserves a questionable volume for independent recovery. This implementation handles the original invocation's live evidence. Admission of new evidence after interruption needs slice 3's separate witness policy.
