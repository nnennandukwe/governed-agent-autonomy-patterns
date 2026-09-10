# File-effect transaction model

This example governs the tools exposed to a deliberately restricted goose
session. It is not a sandbox against another native process running as the
same operating-system user. All example commands share one process lock.

## Ownership and commit point

The initialized demo has two separate directories: a disposable workspace
containing `release.json`, and an operator-owned state directory outside the
workspace. One versioned JSON snapshot is canonical for proposals, approvals,
attempt states, and decision records. Goose tools never expose the state
directory or an approval-writing operation.

| State | Transition | Durable meaning |
| --- | --- | --- |
| proposed | Save immutable request and current subject digest | No file effect; approval required |
| approved | Operator approves exact stored request | Authority exists only for that request |
| executing | Persist consumed approval and attempt before dispatch | Retry prohibited; an interrupted attempt is uncertain |
| staged | Write and sync replacement in the workspace | Original file remains visible |
| executed | Atomically rename replacement to `release.json`, then save observed result | File commit occurred; inspect actual content |
| aborted | Pre-commit operation fails and result can be recorded | Original preserved; create a new proposal |
| uncertain | Interrupted executing state, or evidence recording fails after dispatch | Inspect before proceeding; never replay automatically |

The file commit point is replacement of `release.json` by same-directory
rename. The state snapshot commit point is its own same-directory rename.
These are intentionally not presented as one atomic transaction. The durable
`executing` marker bridges the gap and prevents a duplicate effect after a
crash. Directory-sync failure after rename is a warning; it never authorizes
replay. No rollback overwrites the target.

## Invariants

- Only an exact approved proposal may reach the file executor.
- A proposal binds the session, path, old bytes, new bytes, policy, and tool
  capability. These bindings are recomputed by the server, not supplied as
  authority by the agent.
- A consumed request is never executed again, including after restart.
- Denial always wins over approval. Unknown request fields grant no authority.
- Paths are limited to named fixture files. Traversal, symlinks, non-regular
  files, and an unexpected workspace identity are rejected.
- Every cooperating command takes the same exclusive lock. Current identity (including permission bits)
  and bytes are checked again immediately before replacement.
- Pre-commit failures preserve the old target; post-commit reporting failures
  preserve the attempt marker and expose uncertainty or a warning.
- Concurrent changes visible at recheck are preserved. An uncooperative native
  process racing the final check and rename is outside this example's scope;
  production isolation requires the later sandbox work.

## Fault matrix and lifecycle closure

Unit tests will exercise the production store with injected operation faults.
No failpoint can be set through an MCP tool or a production environment flag.

| Operation / event | Expected state | Recovery |
| --- | --- | --- |
| Workspace/state metadata, open, read, parse, validation failure before target rename | Target unchanged, request rejected | Repair operator state; inspect before retry |
| Post-rename observation or validation failure | File was replaced; resulting observation is uncertain | Inspect current bytes and consumed attempt; never replay |
| State staging create, write, flush, sync, or rename failure | Previous snapshot or explicit uncertain marker | Inspect snapshot; no effect without durable attempt |
| State directory sync after rename fails | New snapshot visible, durability warning; dispatch stops if its attempt snapshot is not durably synced | Inspect; never replay a consumed request |
| Target staging create, write, metadata, flush, or sync fails | Old target, consumed attempt | Inspect and propose anew |
| Target changes or becomes a symlink before recheck | Concurrent target preserved, abort | Read current state and propose anew |
| Target rename fails | Old target, abort | Inspect and propose anew |
| Target directory sync after rename fails | New target, committed with warning | Inspect new bytes; no replay |
| Result recording fails before result-snapshot rename | New target and retained executing marker | Report uncertain; inspect actual bytes |
| Result snapshot directory sync fails after its rename | Executed/aborted/unknown observation is visible; response carries a durability warning | Preserve observed status; inspect snapshot and file; never replay |
| Staging cleanup fails | Owned staging file may remain; canonical target unchanged or committed | Report cleanup warning; never remove unrelated files |
| Process interruption before/after rename | Executing marker retained | Inspector reports uncertain with observed hashes |
| Concurrent approvals/applies and duplicate delivery | At most one file effect | Other requests are blocked or observe committed state |

The lifecycle tests cover initialization without overwrite, real inspection,
proposal reuse, authority drift, byte drift, capability/policy drift, denied
paths, approval tampering, interrupted attempts, duplicate execution, and
restart. There is no reset/delete command; each rehearsal uses a fresh owned
directory, preserving all evidence from previous rehearsals.

Initialization creates the session root with mode 0700 and rejects shared
non-sticky parents and ancestors owned by other non-root users. Sticky `/tmp`
and private directories are supported. This bounds setup against a different
user replacing a newly created session in an untrusted parent; the same-user
native-process exclusion above still applies.
