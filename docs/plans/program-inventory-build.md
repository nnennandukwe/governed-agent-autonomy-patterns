# GAAP Program Inventory and Reference Runtime Evidence Build Plan

## Summary

Give maintainers a checklist of documented GAAP work and show which parts of the accepted reference runtime have delivery evidence. Start from main at `f9d4d99d00c04ecafb6f916635d37ad0cf146c11`. One documentation PR covers the inventory and evidence for the first end-to-end run. The maintainer owns adoption decisions. [Reference runtime tracker](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/21)

## Skills To Use

| Stage | Skill | Responsibility |
| --- | --- | --- |
| Scope | Software Build Plan and qodo-get-rules | Bound this PR to the inventory and apply the repository rules retrieved for it. |
| Writing | voice-canon | Name the owner, acceptance test and evidence for each item. Check the final prose. |
| Review | qodo-review and qodo-review-resolver | Review the full diff locally against the pushed base, then inspect the hosted review for the same head. |

The published base and isolated Git worktree support Qodo Local Review. The change concerns documentation and evidence links.

## Scope

**In scope.** Inventory the open issues, milestones, accepted plans, ADRs and explicit outstanding work. Include closed work where it supplies evidence for an open parent. Trace all eight original #21 exit criteria and account for the ThreadLoop export entry.

**Out of scope.** Recovery implementation belongs to #32 after an adoption decision. Research experiments, a paid pilot and the explorer follow the gates in #29, #7 and #1. ThreadLoop owns its controller implementation.

## Package Layout / File-to-Task Mapping

- `docs/plans/gaap-program.md` (new) will hold the checklist, evidence and decision gates.
- `docs/plans/program-inventory-build.md` (new) will define this PR's scope and verification.
- Use existing documentation entry points for a link or a substantiated status correction where needed.

## Dependencies And Settings

The plan uses Markdown, Git and read access to the three repositories' published records. It requires no package, toolchain, schema, environment or storage changes.

## Canonical Contracts

Each checklist row identifies its owning source, acceptance criteria, status, prerequisites, next action and owner. A delivered claim points to the exact revision or retained evidence that supports it. A decision request identifies who can decide. Current status belongs in the checklist.

## TDD And BDD Implementation Strategy

1. Start with the issue inventory. For each entry, find the source that defines acceptance and assign its documented owner.
2. For each #21 exit criterion, look for evidence of the behavior as written. When child results cover only part of a criterion, record the missing observation.
3. Compare the tracker and checklist. Correct stale status using the owning delivery record and identify the next eligible action for each open item.

## Component Design

GAAP governs one bounded run. ThreadLoop owns lifecycle advancement and verifies evidence before evaluating its own guards. RunInvariant observes a separately versioned public contract. Link evidence at the boundary where the behavior was observed. A cross-system success claim requires evidence tying the participating operations to the same run.

## Failure And Recovery Rules

Preserve a missing observation as an open criterion with its owner and the evidence needed to resolve it. When an owner changes a decision or a source advances during review, retain prior evidence, inspect the new source and reverify the affected claim. An unresolved decision holds its dependent work while independent work can proceed.

## Commit Plan

Commit the inventory, this plan and any directly related status or navigation edits as one documentation change.

## Branch And PR Flow

1. Use `docs/gaap-program-inventory` in the isolated worktree. Link #21 and the program issues in the PR.
2. Run the focused checks below and the applicable final verification in [CONTRIBUTING.md](../../CONTRIBUTING.md). Commit the intended diff and record its SHA.
3. Run Qodo Local Review in auto mode with the complete change from the pinned base, the acceptance criteria and source links. Reproduce findings, repair supported errors, verify and repeat final review on a changed head. Follow the review skill's retry limit if the service fails.
4. Open the PR after final local review. Require completed structured hosted review whose `commit_sha` matches the head and passing required CI. Recheck the head, merge that SHA, compare the merged tree and verify post-merge checks before updating the owning record.

## Test Plan

Check the reported file list and run the repository's lane selector using all changed paths. Inspect links to confirm that each source supports its row. A closed child or a model fixture alone can't demonstrate a cross-process handoff.

```sh
git diff --check f9d4d99d00c04ecafb6f916635d37ad0cf146c11
git diff --name-only f9d4d99d00c04ecafb6f916635d37ad0cf146c11
bash .github/scripts/test-select-ci-lanes.sh
```

Verify authored prose with `verify_language.py`. The maintainer decides any unresolved adoption question.

## Definition Of Done

- Every documented item in the initial inventory has an owner, acceptance criteria, disposition or remaining action, and a source.
- All eight #21 criteria have supporting evidence or an explicit gap. The tracker points to verified ThreadLoop export delivery.
- The final diff passes the language and repository checks. Completed local and hosted review cover the merged change, with findings assessed and post-merge results recorded.

## Assumptions And Defaults

Use November 21, 2026 as the inventory date. Reopen a delivered item only when an owning criterion needs evidence. Keep the frozen contracts, benchmark and conformance corpora, and original reference checkout intact.
