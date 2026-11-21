# ADR 0009 — Recovering evidence after an Agent Run stops

**Accepted on November 21, 2026.** The maintainer accepted this design as proposed, including its single-host cooperating-writer profile, evidence admission rules and deferral of approval continuation. The maintainer subsequently accepted the [first storage contract](../contracts/journal-enrollment-v0.1.md) and authorized slice 1. [Maintainer decision in #32](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/32#issuecomment-7068733430) · [Design mandate #29](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/29)

A caller can lose its connection after GAAP changes a file but before it receives a receipt. Running the request again could overwrite a later edit. The host must first read an intact local journal. It can return a published receipt, report a known effect without a receipt, or report an unresolved attempt. Only independently attributable evidence can resolve uncertainty after dispatch.

<a id="proposed-profile-and-ownership"></a>

## Accepted profile and ownership

Start with one host, one private workspace, cooperating writers and one synchronous local file replacement per attempt. All admissions, cancellations that stop dispatch, reconciliation decisions and publications for that workspace serialize through one host-owned lock. Hold it from admission through the last possible effect and journal publication. Recover it only when the old writer and any executor capable of committing the effect have stopped. Losing a socket, expiring a lease or obtaining a host lock while a child can still write doesn't meet that condition. Refuse asynchronous providers, detached writers, external effects and distributed takeover in this profile.

The host persists decisions and observations. The bounded run engine asks `RunCoordinator` for effect and completion decisions, executes through `ExecutorPort`, and calls an independent trusted `VerifierPort`. An inspector only reads. ThreadLoop's claims and attempts identify outer work, and ThreadLoop alone can advance its lifecycle. [Engine decision](0005-bounded-agent-run-engine.md) · [Integration order, #21](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/21)

## What the journal binds

Use a separately versioned local snapshot. These are logical records in that snapshot. Their names don't extend the public contracts. Each publication preserves prior facts and their order. Under the lock, validate field types, sizes, cross-record identities, digest references and allowed transitions before returning history or admitting work.

| Record | Required binding and owner |
| --- | --- |
| Store | Host-issued workspace/store identity, format version, expected local root and file identities, policy and capability profile. Only explicit initialization can establish an unused workspace. A missing or corrupt journal from an enrolled workspace stops admission, even if the requested key has never been seen by this process. |
| Intent | The host authenticates the caller and binds its principal plus submission key to one immutable request. Retain bounded canonical proposal bytes and their digest, host-issued request/run/attempt IDs, effect ID, one-based sequence, effect idempotency key and exact request digests. Outer claim/attempt or delivery IDs may be recorded for correlation. The host never takes authority from these IDs. |
| Subject | Bind the requested input subject and proposed artifact to the attempt. Record the observed pre-effect subject and, when known, the resulting subject and portable artifact digest. For a local file, store its byte hash separately from host-only root, device, inode, ownership and mode checks. Identical bytes can occur on another inode or after two writes. |
| Authority and dispatch | Bind the gate decisions, actor, grant reference, scope, subject, policy name/version/digest, capability name/version/digest, tool-schema digest, sandbox, executor identity and budget to the attempt. Record the approval validity or revocation source and the moment checked. Record the dispatch reservation separately from measured usage. A fence alone can’t establish that the executor ran. Preserve observed usage and its known lower bound after interruption. |
| Observation and verification | Record which executor invocation was attempted, its observed effect disposition, subject, evidence references and usage. A failed executor response alone leaves the mutation unresolved. A trusted observation of a write survives an error response. Keep verifier identity, evidence types/digests, verdict, verified subject and any later completion decision separately. An independent verifier must target the latest subject after the last mutation. |
| Publication and reconciliation | Bind the exact sealed receipt and effect result, when available, to their request and body digests. Keep subsequent rulings as separate entries with the original attempt, witness, admitting actor, evidence digest, decision and reason. A history read returns the original receipt and any later ruling as separate facts. |

For either public request use SHA-256 over its RFC 8785 canonical JSON bytes. For a public result or receipt use SHA-256 over its canonical **body**. Keep the complete parsed proposal's canonical bytes separate from those four digest domains. Object member order and transport whitespace don't change a proposal. String content and array order can. Reject malformed, ambiguous or oversized input before comparison. Existing contract validators still define every frozen field and digest. [Agent Run contract](../contracts/agent-run-v0.1.md) · [Protected Effect contract](../contracts/protected-effect-v0.1.md) · [Proposal adapter](../../adapters/submitted-proposal/README.md)

## Commit points and allowed transitions

In the live attempt the host records `intent → dispatch → observed → verified → published`. Checks that deny execution can go directly from intent to a sealed refusal. A known failed or interrupted execution can end in a non-completed receipt without passing verification. An executor can also return an unknown outcome that the engine seals into a failed run receipt. An abrupt stop can leave any earlier stage as the last durable fact. Restart opens that state for inspection. Each stage describes what survived, independently of the current workspace and whether the caller received a response.

| Before the host can… | Durable prerequisite and allowed action |
| --- | --- |
| Admit a first submission | Under the workspace lock, validate the complete history and look up the caller and key. Exact canonical bytes return the saved outcome or latest known stage. Changed bytes under a used key conflict, even while the original attempt is unresolved. Reusing a run, request, attempt or effect identity for another admission also conflicts. Never return another caller's history without read authority. |
| Invoke the executor | Save `intent`. Check the current subject and all grants, revocations, expiry, capability, tool schema, policy, cancellation and remaining budget under the lock. Persist the resulting `dispatch` authorization before entering the executor. An unreadable or unconfirmed record stops dispatch. A surviving intent alone proves no dispatch only after the previous writer is quiescent and the journal's integrity is established. |
| Commit the workspace effect | Only the invocation bound to `dispatch` may replace the target. Stage bytes privately, sync the staged file, recheck the target and current authority under the cooperating lock, then rename over the target. **The target rename commits the visible mutation.** Confirm directory durability separately. A missing post-rename observation leaves an unresolved effect. |
| Verify or seal | Persist `observed` with the known effect, including any observed write on a failed run. Record verification against that exact output subject. A failed verifier, drift, cancellation or incomplete budget evidence precludes a successful completion. Only the live engine can claim completion and seal after its required checks. A recorded passing verdict alone can't complete a run. |
| Respond or clean up | Validate the sealed objects against their requests. Publish them together with the linked records in one journal generation. **The journal rename publishes the receipt.** Confirm its directory sync before acknowledging durable publication. Lost delivery doesn't alter recorded facts. Remove only proven-owned scratch files after publication. Cleanup failure can add a warning, and can't withdraw a receipt or undo a mutation. |

There are two externally visible commits. A host can stop after the target rename but before the receipt rename. This profile can't guarantee exactly-once execution. It trades automatic recovery for observable outcomes and refusal of unsafe dispatch.

## Recovery after interruption

The caller authenticates and performs the same journal lookup before requesting any retry. A duplicate can receive a historical result even if the workspace or execution policy has since changed, provided its current read permission and historical integrity checks pass. A known outcome can be partial, including an observed mutation with no published receipt. If the live engine has stopped before publication, this profile reports a permanently unsealed attempt. No component will finish sealing it. The caller receives the last known facts immediately and can end its wait. Any later request follows the fresh-attempt rules.

| Failure window / fixture cut | What the next reader may report | Permitted next action |
| --- | --- | --- |
| `before_intent` | In an intact initialized store there's no admission and no effect. An unavailable store gives no proof of absence. | An explicit submission can make its first admission after validating the store. |
| `before_dispatch` | Durable intent only. A stopped writer couldn't have invoked the executor without a published dispatch record. | Report the unexecuted attempt. A caller can request a fresh attempt under the rules below. |
| `before_effect` or `after_effect` | Both leave the same dispatch record. The target may have zero or one mutation. A current byte match can't disambiguate these histories. | Keep the workspace closed to new dispatch until evidence resolves the attempt. |
| `during_verification` | The journal identifies the effect and its observed subject. The check hasn't yielded a durable verdict. A returned failing verdict can also coexist with an executed effect. | Return both facts separately. Preserve the mutation and the incomplete or non-completed run. |
| `during_receipt` | The prior generation may include effect and verification evidence without a receipt. If rename happened, validate the visible generation and report any uncertainty about durability. | Return the last validated facts. Never rerun the effect to obtain a receipt. |
| `before_response` | A validated published receipt survives loss of the original connection. | Return that same receipt to an authorized duplicate, including after restart. |
| `during_cleanup` | The receipt and workspace effect have already committed. Scratch or warning publication can still fail. | Return the saved receipt and any available warning. Leave unowned or ambiguous files in place. |

**Cancellation.** Before dispatch, a serialized cancellation or expired deadline can prevent execution. Between dispatch and rename the live executor may prove it aborted without committing. Otherwise the outcome stays unknown. After a committed write, retain its actual result even when cancellation interrupts the run. After the engine's completion claim, a late cancellation can't relabel its sealed receipt. Returning a duplicate never resets usage or allocates budget.

## Who can resolve an unknown effect

A designated host reconciliation operator admits evidence under current read/write authorization for this workspace. A human can supply a trusted, attributable observation, or an approved independent observer can produce a record. The admitting operator validates that source against configured trust. The executor's own unsupported claim, a caller's assertion, an inspector screenshot, a matching filename or an empty log can't establish the result.

Bind every observation to the store, run, attempt, effect/invocation, canonical content, subject and observer. Verify provenance and identity, the interval covered, and the previous writer's quiescence. Hold the same workspace lock while checking and appending a ruling. For a negative verdict, the observer must account for **every possible commit** by that invocation through its definitive stop. Absence from a partial history proves nothing.

| Admitted evidence | Recorded effect disposition |
| --- | --- |
| A trusted independent witness observed the target replacement by this invocation and bound the output artifact and subject to it. | Executed. Preserve the mutation even if the run failed verification. |
| An authoritative end-of-invocation observation proves the executor never started the effect and can't start it later. | Not executed. Keep the original attempt and its evidence. |
| The invocation began and a trusted observation proves its specific failure before rename, termination and zero committed mutations. | Known failed with no committed mutation. A failure after a confirmed write preserves that write. |
| Evidence is missing, unauthenticated, contradictory, doesn't cover the full interval, or leaves an executor able to commit. | Unresolved. Report the reason, preserve prior facts and refuse fresh dispatch for this workspace. |

Reconciliation appends an effect ruling. It never reopens a sealed receipt, completes an unsealed run, grants an approval, or dispatches work. Repeating identical evidence returns the existing ruling. An unauthenticated claim is refused. If a second validated witness contradicts an admitted ruling, retain both observations, flag the dispute and hold further dispatch for investigation. Reopening the journal preserves that hold. This profile has no operation to discard either observation or clear the dispute.

## Fresh attempts and approval

A fresh attempt requires an explicit caller request with a new submission key and new host-issued request, run, attempt and effect identities. Re-read durable history under the workspace lock, require the old executor to be unable to commit, and check the current subject, approval, policy, capability and budget. Block fresh dispatch for **any** caller or key while an earlier effect on this workspace is unresolved. Changing a key, moving the store or initializing another journal for the same workspace can't establish a clear history. A known executed effect is returned for its original request. A new request to change the resulting workspace must describe and authorize a new effect on the current subject.

**Defer approval continuation.** The engine seals one-shot authority-required outcomes as blocked. Granting approval later requires a fresh request, current authority and all admission checks. Revoked or expired grants, stale subjects and unsupported policy or capability identities stop a new dispatch. The historical receipt, its usage and the original approval record remain inspectable under current read permissions. Restarting an inspector can't acquire execution authority, resume verification, edit the run or extend a budget.

## Storage and trust limits

Use an explicitly initialized, owner-private local directory outside the mutable workspace. Anchor it to the workspace's enrolled identity. Serialize cooperating writers with a stable lock independent of the file being replaced. Each journal update writes a bounded complete snapshot to a same-directory temporary file, flushes and syncs it, renames it and syncs the parent directory. Set byte, record and retention limits at enrollment. Refuse admission before exhaustion where possible. Preserve the prior valid snapshot on a failed write. A store at capacity after a mutation must retain its dispatch or observed record and refuse replay. Reclaiming historical identities would reopen duplicate execution, so this profile refuses eviction and online migration.

Readers validate the expected format, references and canonical sealed objects against their original policy bindings. Unknown format, conflicting records, missing evidence needed for interpretation, unsafe links, relocation, or an unconfirmed generation require preserving the store for diagnosis. Never initialize over it or fall back to an older copy to authorize execution. Recognized historical records can be read under their original policy bindings after a policy change. A fresh dispatch requires an accepted current profile and an explicit re-enrollment procedure that preserves all outstanding holds. This profile defers in-place policy migration.

A local digest detects changed bytes only relative to a trusted reference. Filesystem ownership, private modes, link checks and identity checks reduce accidental interference within the cooperating-writer profile. An editor that ignores locks can race a target check and rename. A hostile same-user process can replace the journal, its witnesses or the workspace. Restoring an earlier internally valid snapshot can hide a later admission. This profile assumes intact storage and has no external rollback detector. Store only bounded non-secret inputs and references with restricted read access. Refuse inputs that require storing credentials. Power-loss ordering, producer authentication and protection from hostile local writers require their own implementation evidence. [Existing local commit and recovery behavior](../../examples/submitted-proposal/TRANSACTION.md)

## Fixture and acceptance evidence

Run the design fixtures from the repository root.

```sh
python3 -B tests/durable_run_model/test_recovery.py -v
```

The fixture replaces a temporary journal and runs a scripted target with its own mutation counter, content and final witness. A new host reads the same journal after an injected stop. Test bindings and receipts use symbolic fields. Contract serialization, authenticated principals, OS process fencing and filesystem crash durability require separate implementation tests. Existing Rust engine tests exercise lost replies, changed subjects and failed verification through scripted ports. The submitted-proposal reference exercises actual local writes and lost CLI delivery. The new fixtures address journal lookup and evidence admission across restart. [Executable model](../../tests/durable_run_model/) · [Engine regressions](../../tests/state_machine.rs#L289-L355) · [Local transaction tests](../../examples/submitted-proposal/TRANSACTION.md) · [Unpublished conference evidence](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/32)

| #32 acceptance item | Review artifact |
| --- | --- |
| 1. State, records, commits and recovery | Record, transition and failure-window tables above |
| 2. Effect outcome and run completion | Observed/verified/published stages and failed-verification fixture |
| 3. Lost response | Recovery lookup and restart after publication |
| 4. Duplicate or changed content | Caller/key binding, concurrent duplicate and identity-conflict fixtures |
| 5. Unknown outcome | Admission evidence table and fenced fresh-attempt rules |
| 6. Approval continuation | Deferred continuation and sealed authority-required receipt |
| 7. Separate commits | Target rename and receipt publication |
| 8. Interrupted phases | Named failure windows from first intent through cleanup |
| 9. Deterministic failures and drift | Executable model for races, drift, stale subjects and restart |
| 10. Inspector restart | Read-only historical lookup with no execution transition |
| 11. Storage, versioning and security | Separate local format, refusal rules and local trust limits |
| 12. Bounded follow-ups | Ordered slices and their separate prerequisites below |

## Decision requested and follow-ups

The maintainer accepted the recovery design and requested the first storage contract. The maintainer also accepted the [enrollment and lookup format](../contracts/journal-enrollment-v0.1.md) and authorized its implementation. The following implementation slices retain their prerequisites and order.

1. **Journal enrollment and historical lookup.** Select supported filesystems, bounds, canonical encodings and compatibility policy. Implement an independent read path only after the storage contract is accepted. Test missing, full, incompatible and damaged stores without recreating them.
2. **One local executor and two commit points.** Require the first slice plus a demonstrated cooperating-writer and quiescence mechanism. Bind actual engine dispatch and sealed receipts to the journal. Test process termination, directory-sync failures, duplicate delivery and restart with files observed independently of host records.
3. **Operator reconciliation.** Require the earlier slices and a separately accepted witness trust policy. Demonstrate admitted and rejected evidence through an authenticated operator boundary, including incomplete negative evidence. Any later work on automatic resumption, continued approval or additional effect types requires a new decision.

These slices preserve #21's delivery order. Signing (#15), isolation (#16), generated runtime testing (#18) and ThreadLoop's independent receipt admission (#111) have their own owners and evidence. Schedulers, distributed workflow platforms and cloud synchronization stay outside this profile. [GAAP integration tracker](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/21) · [ThreadLoop receipt admission](https://github.com/nnennandukwe/threadloop/issues/111)
