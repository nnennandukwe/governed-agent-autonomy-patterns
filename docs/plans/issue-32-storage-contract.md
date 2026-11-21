# Journal Enrollment and Historical Lookup Build Plan

## Summary

The GAAP maintainer accepted ADR 0009 and requested a storage contract for the first recovery slice. Start from main at `4dcb0e809b9857d3279df68233633f636e5da3aa`. This documentation PR records the decision and proposes how a host can enroll a workspace and read an earlier submission after restart. The maintainer will decide whether to adopt that storage contract and authorize implementation. [Recovery issue #32](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/32)

## Skills To Use

| Stage | Skill | Review artifact |
| --- | --- | --- |
| Scope | Software Build Plan, qodo-get-rules | Apply applicable repository standards to this proposal and its acceptance criteria. |
| Design | transaction-proof, workflow-invariants | Use one state and commit model for enrollment, publication, refusal and repeat lookup. Name the owner at each transition. |
| Verification | failure-path-testing | Map operational faults and restarts to the surviving bytes, observed result and permitted recovery. |
| Writing and review | voice-canon, qodo-review, qodo-review-resolver | Check authored prose. Review the full diff locally and the same head through the hosted service. |

## Scope

Publish the proposed storage profile, byte and record limits, encoding, compatibility policy, access rules and required process tests. Record the accepted recovery design, evidence admission rules and deferred approval continuation. Production storage, local execution and operator reconciliation retain their ordered prerequisites in [ADR 0009](../adr/0009-durable-agent-run-reconciliation.md).

## Package Layout / File-to-Task Mapping

| File or record | Responsibility |
| --- | --- |
| [Journal contract](../contracts/journal-enrollment-v0.1.md) (new) | Specify enrollment and lookup, with its qualification and failure matrix. |
| [ADR 0009](../adr/0009-durable-agent-run-reconciliation.md) | Record acceptance of the design and link its first proposed contract. |
| [Program checklist](gaap-program.md) and [README](../../README.md) | Link each artifact with its current decision and scope. |
| This plan and [issue #32](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/32) | Bound the PR and record its verified delivery. Update dependent tracker entries where their decision status changes. |

## Dependencies And Settings

Use the frozen Agent Run and Protected Effect validators for their existing identity and digest domains. The proposal must name the eligible filesystem and mount behavior, directory ownership, file and lock identity, writer assumptions, encoding, numerical limits and version-mismatch behavior. These are prospective deployment requirements.

## Canonical Contracts

Explicit enrollment binds one private store to one workspace. Lookup checks current read authority, principal, submission key, canonical proposal bytes and history integrity. Matching content returns saved facts. Changed content conflicts. A missing entry can indicate absence only in a validated, enrolled store. Return unsealed outcomes and immutable sealed receipts as separate facts. A future reader for witness rulings needs a separately accepted trust policy. Reading any of them confers no execution or lifecycle authority.

## TDD And BDD Implementation Strategy

Use example-driven review for this document. Trace a first enrollment, a repeated enrollment, two competing processes and an authorized lookup after restart. Trace changed content, unauthorized access, store loss and uncertain publication against the same model. For each, identify the observer, expected durable bytes, blocked actions and next permitted action. Reserve executable adapter tests for the implementation decision.

## Component Design

The host owns enrollment and serializes cooperating writers with a stable lock. The reader verifies the store's binding, format and complete committed generation before interpreting history. The design must identify the enrollment commit and how readers detect incomplete initialization across every artifact. Journal rename makes a new generation visible. The writer syncs the journal directory before publishing the confirming anchor. It syncs the anchor directory before acknowledging durability. The later executor's target rename commits a separate workspace effect. ThreadLoop retains outer lifecycle authority.

## Failure And Recovery Rules

The contract's matrix must cover every storage operation that acquires ownership, validates a binding, stages bytes, publishes a generation or removes scratch. Include races between observation and mutation, process stops on both sides of each commit, file and directory sync failures, capacity exhaustion, corrupt or incompatible bytes, relocation and lost acknowledgment. State what a fresh process sees, which original bytes it preserves and how the caller can inspect or retry. Cleanup after a committed generation affects warnings only. A surviving dispatch without adequate evidence holds the workspace.

## Commit Plan

Commit the accepted ADR status, proposed contract, plan and navigation together. Keep verified review repairs traceable to the finding and revision.

## Branch And PR Flow

1. Use `design/journal-enrollment-contract` at the pinned pushed base. Link #32 in the PR and keep it open for the outstanding decisions.
2. Run focused and final checks and commit the full intended diff. This worktree and its published base qualify for Qodo Local Review. Run final auto-depth review against the base SHA with the contract scope and issue link. Reproduce findings and repeat verification and review after any correction.
3. Open the PR. Require passing CI and a completed structured hosted review whose `commit_sha` equals its current head. Follow each review skill's bounded recovery instructions on service failure. Keep any unverified gate open.
4. Recheck head and gates before a SHA-guarded merge. Compare merged and reviewed trees and check post-merge CI before recording delivery on owning trackers.

## Test Plan

Check the proposed bytes, limits, refusal outcomes and restart cases against the ADR. Verify file and section links, the scoped diff and the final authored prose. Run the existing recovery fixtures and root contributor checks. The CI classifier selects the contract lane for edits under `docs/contracts/`. The proposed adapter's process tests will need separate evidence on each supported storage profile. [Contributor checks](../../CONTRIBUTING.md)

```sh
git diff --check 4dcb0e809b9857d3279df68233633f636e5da3aa
python3 -B tests/durable_run_model/test_recovery.py -v
bash .github/scripts/test-select-ci-lanes.sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked
cargo run --locked --example generate-contract-artifacts -- --check
```

## Definition Of Done

The reviewed proposal gives the maintainer a storage profile to accept, revise or decline. Its commit model, ownership invariants, fault matrix and lifecycle checks name the evidence a production implementation must supply. The ADR, README, program checklist and owning trackers agree on which decision has been made. Report document verification, model results, exact-head reviews and post-merge status separately from future process-level evidence.

## Assumptions And Defaults

ADR 0009 assumes one host, one private workspace, intact storage and cooperating writers. A hostile same-user writer and rollback to a previously valid snapshot are outside that trust model. Contract acceptance and implementation authorization remain with the maintainer.
