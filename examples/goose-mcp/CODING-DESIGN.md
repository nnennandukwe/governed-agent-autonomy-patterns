# Live coding demo: implementation contract

A Goose session groups bounded Agent Runs. Each submitted change uses the real
AgentRunEngine, a Goose proposal adapter, one filesystem executor, and an
independent verifier. Existing contracts and the older example remain intact.

The task is a shipping-price function. Goose can change shipping.py under a
preauthorized local policy. deployment.json is protected. The verifier checks
real Python execution against acceptance cases; generated code is restricted
to a bounded arithmetic/conditional function before execution. This is a
language restriction and mediated filesystem boundary, not a production OS
sandbox. No arbitrary shell or other Goose extensions are enabled.

## State and commit points

Canonical data: workspace files and operator/state.json. The latter contains
complete per-run requests, receipts, results, and evidence. The inspector reads
atomic snapshots. An exclusive operator lock serializes all calls.

| Starting state | Operation | Visible state | Recovery |
| --- | --- | --- | --- |
| Fresh directory | create fixture, sync, publish manifest | ready | incomplete initialization stays explicit; use a fresh directory |
| Ready | sync running marker before dispatch | running with exact input and prior digest | restart never replays this operation |
| Running | stage file; recheck all identities and bytes | old workspace | abort preserves old workspace |
| Staged | rename staged shipping.py over original | changed workspace | commit point; no automatic rollback |
| Changed | observe bytes, run verifier, validate sealed receipt | result ready | verifier failure retains actual changed code |
| Result ready | atomic state snapshot rename | terminal record visible | directory-sync failure is a durability warning |
| Crash / receipt persistence failure | leave running marker | outcome uncertain | inspect actual files; create fresh session; never retry automatically |

There is no claim that the workspace rename and receipt publication are one
transaction. A running marker is deliberately conservative after interruption.
No cleanup/reset/delete command is provided; previous sessions are preserved.

## Invariants and fault matrix

- Only shipping.py may be mutated, after the engine allows all gates.
- Always bind plan, proposed bytes, policy, capability/schema, and initial subject.
- Always recheck the complete observed workspace before the rename.
- Never follow links or resume an uncertain run. Never knowingly replace a
  changed subject: mediated writers serialize and recheck immediately before
  rename. Native edits in the final check/rename interval are not atomically
  excluded; operators should edit between calls.
- Exactly one durable running marker precedes any effect attempt.
- Completion requires independent tests of the current subject; failed tests
  never imply that an already executed file change was rolled back.
- Cost/tokens outside the MCP boundary are unavailable, never displayed as zero.
- Dashboard data comes from validated records and uses escaped text rendering.

| Failpoint | Expected state / observation | Verification |
| --- | --- | --- |
| lstat / open / read / parse / identity validation | no dispatch, preserve files | invalid roots, symlink and tamper tests |
| create staging file | running marker; old subject | injected precommit failure |
| write / flush / file sync / chmod | old subject; aborted attempt | each stage injected separately |
| running-marker create/write/flush/sync/rename | no effect | persistence failure tests |
| concurrent change before rename | preserve concurrent bytes; aborted | stale snapshot test |
| file rename | old subject if failed | injected rename failure |
| observation after rename | changed file; unknown outcome | injected postcommit failure |
| verifier parse / execution / timeout | change retained; completion blocked | invalid program and deadline tests |
| receipt snapshot create/write/flush/sync/rename | running marker survives | result persistence tests |
| directory sync after publication | visible result plus durability warning | injected postcommit warning |
| restart / duplicate call | no automatic replay | uncertain and duplicate tests |

## Presentation

Visual thesis: a dark, restrained run console with a prominent outcome and five
plain-language pillar columns. Content: session/task header; chronological runs;
selected run's decisions, actual diff, test cases, budget, and evidence.
Interaction: live snapshot updates, selectable runs, expandable evidence.
No decorative metrics or synthetic all-green state.
