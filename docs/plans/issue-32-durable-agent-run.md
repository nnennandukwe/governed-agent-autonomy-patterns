# Issue 32 Durable Agent Run Recovery Build Plan

## Summary

Deliver one proposed design and an executable fault model for [issue #32](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/32) against main at `6c0c681c0b4764ea9b29526cebd8b344a497b364`. #29 accepts design work. ADR 0005 and #21 retain engine and integration ownership.

## Skills To Use

| Stage | Skill | Evidence |
| --- | --- | --- |
| Before design | qodo-get-rules | Repository-scoped rules will guide descriptive English names, comments, ASCII string literals and Python function docstrings. |
| Design | transaction-proof, workflow-invariants | One commit/recovery table will tie allowed and forbidden transitions to effect, run and publication facts. |
| Verification | failure-path-testing, pythonic | Scripted interruption and duplicate cases will check independent mutation counts and readable Python. |
| Writing | voice-canon | Language verification will cover the plan, ADR, PR and handoff. |
| Review | qodo-review, qodo-review-resolver | The pushed base qualifies for complete local review. The hosted session must cover the current PR head. |

Source inspection covers the small set of GAAP owners. The fixture uses standard Python I/O at the test boundary. The build introduces no production I/O service or Rust API.

## Scope

Cover the 12 source criteria in order in ADR 0009, including approval deferral, read-only inspection and prerequisites for later implementation. Production adoption, automatic replay and ThreadLoop transitions require separate authority.

## Package Layout / File-to-Task Mapping

- New `docs/adr/0009-durable-agent-run-reconciliation.md` will specify the proposed records, ownership, recovery, acceptance mapping and follow-ups.
- New `tests/durable_run_model/` will hold a standalone Python model and its executable scenarios.
- The existing baseline in `.github/workflows/ci.yml` will run the scenarios. Core, frozen artifacts and conference fixtures have no planned edits.

- New `docs/plans/issue-32-durable-agent-run.md` will retain this plan beside the earlier issue plans. The architecture decision list in `README.md` will link to the proposal.

## Dependencies And Settings

Use the Python standard library with temporary fixture storage. Model a separately versioned single-host snapshot and one scripted workspace replacement, synchronized among cooperating writers. Reuse each frozen contract's existing byte and digest rules. The proposal introduces no new runtime settings.

## Canonical Contracts

The authenticated caller and submission key identify a lookup. Exact canonical bytes return the prior known result or current uncertainty. Different bytes conflict. Validate authority and subject again for first admission. A fresh attempt needs new identities, an explicit request and current authority. A new key can't bypass an unresolved effect on the workspace. Unknown storage versions refuse interpretation. Historical receipt reads don't confer current execution authority.

## TDD And BDD Implementation Strategy

1. State the expected surviving journal record, response, permitted transition and independent mutation count for each cut.
2. Exercise duplicate submissions, interruption, independent evidence admission and later reads through the same model interface.
3. Check untrusted evidence, stale subjects, lost authority, schema/policy drift, conflicts and prohibited completion or replay alongside successful historical lookup.

## Component Design

RunCoordinator owns effect decisions. The proposed local host orders intent, dispatch authorization, effect observation, independent verification and receipt publication. Workspace replace and snapshot publication have different commit points. A trusted host reconciliation role may admit independently attributable evidence as an append-only ruling. Prior receipts remain sealed. Separate storage facts from current workspace witnesses. Model an inspector with read-only access.

## Failure And Recovery Rules

Before durable intent, a verified intact workspace journal can admit a first submission. A proven pre-dispatch stop permits a fresh admission under new identities. Crossing the dispatch fence leaves an unresolved outcome until qualified evidence establishes what happened. Failure after an observed mutation preserves it through failed verification, receipt loss, response loss or cleanup failure. Read the journal before considering another effect. Untrusted, partial or contradictory evidence leaves the original facts in place.

## Commit Plan

1. Specify the proposed recovery protocol, prior evidence and evidence limits.
2. Land the model, deterministic scenarios and one CI invocation together. Keep later review repairs individually attributable.

## Branch And PR Flow

Use `design/issue-32-durable-agent-run` on the verified pushed base. Commit only owned files before the fast asynchronous checkpoint. Collect its findings. Run applicable local checks and a full auto-depth Qodo review against the pinned base at the intended head before opening or updating the PR. Reproduce actionable findings at their revision and rerun checks and review after changes. Use a nonclosing reference to #32. Verify hosted status, reviewed SHA, findings, CI and base immediately before a SHA-bound merge. Compare merged tree and check main CI before recording delivery. Leave acceptance with the maintainer.

## Test Plan

Run the Python model suite, language verification, classifier tests and the root contributor matrix. Check scoped diff and frozen paths. Use CI for Linux, adapters, external conformance and isolation profiles. Record each executed revision.

```sh
(cd tests/durable_run_model && python3 -B -m unittest -v test_recovery)
bash .github/scripts/test-select-ci-lanes.sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked
cargo run --locked --example generate-contract-artifacts -- --check
```

## Definition Of Done

All 12 criteria have a reviewable artifact or explicit acceptance gate. The fixture exposes mutation counts and checks forbidden transitions. A verified PR preserves the proposed status, frozen bytes, reference checkout and issue ownership. Checks, review, merge and owner acceptance have separate evidence.

## Assumptions And Defaults

The first candidate uses one local serialized writer and an intact private store. The fixture can substitute an abstract publication operation and a scripted witness. A host crash, power loss, hostile writers, authenticated provenance and production recovery need separate implementation evidence.
