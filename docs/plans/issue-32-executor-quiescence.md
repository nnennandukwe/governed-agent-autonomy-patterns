# Local executor quiescence Build Plan

## Summary

A lost connection can leave a file replacement pending. Before integrating an executor with the journal, GAAP needs evidence of who can still rename the target after interruption. This plan covers qualification of the existing foreground executor from `88b8ce897a7988357b1cc8f4d3951a93fd3b78d3` and prepares the slice 2 decision. [Issue #32](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/32) · [Accepted recovery design](../adr/0009-durable-agent-run-reconciliation.md#accepted-profile-and-ownership)

## Skills To Use

Apply the repository rules from `qodo-get-rules` before choosing test seams. Use `transaction-proof` for the commit points and independent observations below, and `failure-path-testing` to check refusal after restart through the CLI. Use `qodo-review` for the complete verified diff and `voice-canon` for the decision record.

## Scope

Qualify one invocation of `coding-submit` for the fixture's `shipping.py` on the declared Linux ext4 profile. Test the existing Rust binary with source breakpoints. The candidate is one synchronous local `FileExecutor`. Distributed or detached writers require different fencing. Operator reconciliation belongs to slice 3.

## Package Layout / File-to-Task Mapping

- The [reference executor](../../examples/submitted-proposal/src/coding/mod.rs) and its [transaction rules](../../examples/submitted-proposal/TRANSACTION.md) identify the target rename, the admission lock and the separate receipt publication.
- The new [executor process test](../../adapters/journal/tests/qual_executor_quiescence.py) controls the foreground CLI and observes target identity and lock contention from another process.
- The [ext4 runner](../../adapters/journal/qualify-ext4.sh) and [CI workflow](../../.github/workflows/ci.yml) select the volume, build the reference with source locations and retain evidence. The [adapter guide](../../adapters/journal/README.md) and [program checklist](gaap-program.md) record how to interpret the qualification.

## Dependencies And Settings

Use the existing hosted Linux runner with a private, disposable ext4 mount, ordered data journaling and barriers enabled. Use the locked Rust build with source locations, Python 3 and LLDB. Record the binary digest, revision, filesystem and mount options. Label any macOS debugger observations separately. This qualification uses local fixtures and zero provider calls.

## Canonical Contracts

The reference holds `operator/lock` and a lock on the operator directory. `operator/state.json` records its in-flight marker and eventual receipt. Journal v0.1 instead uses the provisioned control-directory lock and confirms the canonical journal snapshot against its independently held anchor. Qualification of the reference can establish the worker's behavior at its own locks and commits. Integration must demonstrate that behavior with the journal's actual lock, authenticated principal, key and evidence bindings. [Reference transaction](../../examples/submitted-proposal/TRANSACTION.md) · [Journal contract](../contracts/journal-enrollment-v0.1.md)

## TDD And BDD Implementation Strategy

1. Run a clean submission. Inspect its receipt in a fresh CLI and resubmit the same input. Check that the duplicate returns the saved receipt and leaves the target and ledger intact.
2. For each interruption, use a fresh workspace. Stop the worker on the respective side of its target rename. An independent observer attempts the reference locks and records the target inode and content digest and the ledger while the worker is stopped.
3. Signal that worker through its Linux pidfd, a descriptor tied to the live process. Require the debugger to reap it and observe the pidfd's exit notification. Probe the locks again, inspect in a new process and repeat the submission. Compare the observations with the expected outcomes below. A missed breakpoint, wrong process, unexpectedly available lock or unobserved exit invalidates the case.

## Component Design

The proposed integration uses one single-use foreground worker. It takes the journal's exclusive lock before admission and holds it through dispatch, the last possible target rename and confirmed receipt publication. Only that worker can replace the target. Any verifier child receives bounded inputs and must have no path to commit the protected write. A process ID identifies a candidate worker. The host must establish its identity and observe its exit and the absence of another commit-capable actor. A transport disconnect, verifier return or newly available lock alone leaves that question open.

Under the lock the worker validates the journal and checks caller, key and canonical proposal before allocating fresh IDs. Before dispatch it binds the exact request, executor, subject, policy, capability, schema, approvals and budget reservation. Immediately before rename it rechecks the pinned target and current authority, including expiry, revocation, cancellation and remaining budget. Rename commits the workspace effect. The worker records the original effect outcome, usage lower bound and independent verifier evidence. The journal rename publishes the receipt. Directory sync and the confirming anchor establish the acknowledged journal generation. The run engine owns completion. ThreadLoop owns outer workflow state.

## Failure And Recovery Rules

| Cut or observation | What to check in the reference | Consequence for integration |
| --- | --- | --- |
| Worker paused before target rename | Both locks exclude a contender. The target still has its original inode and bytes. The ledger contains the in-flight marker. | Intent and dispatch have different meanings. If dispatch survives without sufficient evidence, hold fresh dispatch for every key. |
| Worker paused after rename, before receipt publication | The target has the replacement inode and proposed bytes. The marker remains. Interrupting the worker preserves the write. | A changed target and a failed run can coexist. Later byte equality lacks invocation provenance. |
| Worker killed at either cut | Observe exit and lock reacquisition. A new process inspects the marker. An exact duplicate is refused. The target and ledger survive retry unchanged. | Quiescence ends the opportunity for another write by that invocation. The surviving facts govern whether its past effect is known. |
| Receipt published before delivery | A separate inspector and a duplicate recover the original sealed receipt. Compare effect count, target and receipt digest. | A lost reply grants no replay authority. Cleanup or sync warnings preserve the earlier visible commit and its durability status. |

A later integrated build must exercise termination, file and directory sync errors, stale authority, competing callers, cleanup and lost delivery around the actual target and journal commits. Its operator must preserve uncertain state and the independent storage-health hold. Process interruption on a running kernel and hardware power loss need separate evidence.

## Commit Plan

Keep the qualification harness, hosted run, evidence guide and decision record in one reviewable PR for #32. Include any repairs with their regression cases.

## Branch And PR Flow

Use the isolated `test/issue-32-executor-quiescence` branch. Run the focused cases and repository checks. Review the complete diff against the pinned base with Qodo Local Review. Reproduce reported findings, repair supported failures and re-review changed work. Require hosted review at the exact PR head and required CI. Before recording delivery, compare the merged tree and check post-merge results.

## Test Plan

Run the new test through the hosted `journal` job on its provisioned ext4 volume. Retain observations for each cut, including the exact breakpoint, worker identity and exit, lock probes, ledger state, target identity and bytes, and the fresh inspector and duplicate responses. Read the captured packet along with the job status. Label skipped cases, failures and unobserved operations. Run the existing reference checks for historical restart and lost delivery.

```sh
python3 -B adapters/journal/tests/qual_executor_quiescence.py \
  --reference "$reference" --root "$private_ext4_dir" --evidence "$evidence_dir"
cargo test --locked --manifest-path examples/submitted-proposal/Cargo.toml
cargo test --locked --manifest-path adapters/journal/Cargo.toml
bash .github/scripts/test-select-ci-lanes.sh
```

## Definition Of Done

The held and reacquired locks, observed process exit and target evidence distinguish the two target-rename cuts. Both interrupted sessions refuse replay, and a completed session returns its original receipt on duplicate delivery. The PR identifies which revision and filesystem each observation covers and links the retained Linux artifact after CI. These results inform the owner decision below.

## Assumptions And Defaults

The service and workspace writers cooperate on one host. An unrelated same-account process could ignore the lock or replace files. The proposed profile requires all commit-capable writers to follow the protocol. The executor candidate's bounded verifier and the independent witness trust policy address different responsibilities.

**Owner decision.** After reviewing the evidence, accept, revise or defer the proposal to integrate this one foreground local-file executor with the accepted journal. Acceptance would authorize an implementation with the worker lifetime, authority checks, separate commit points and integrated failure tests above. Revision should identify the writer, fencing assumption or test that needs to change. Deferral leaves the existing read-only journal available. The owner of slice 3 must separately accept a witness trust policy before authenticated reconciliation can proceed.
