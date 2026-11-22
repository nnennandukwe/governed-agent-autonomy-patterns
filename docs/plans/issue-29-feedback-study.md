# Software Build Plan for the offline feedback repair study

## Summary

Issue #29 asks whether feedback helps a strategy produce more independently verified repairs at equal cost. The maintainer selected offline preparation for Experiment B after GAAP recovery passed its main checks. This change prepares the comparison and exercises its measurement tools on public development controls. The study has zero authorized provider calls or spend. [Decision and limits](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/29#issuecomment-7068231915) · [Maintainer selection](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/29#issuecomment-7071790843)

## Skills To Use

Apply the research, Software Build Plan, Qodo rules, workflow invariants, failure-path testing, CLI parity and voice skills. Use Qodo Local Review for the complete diff and the hosted review for the PR head.

## Scope

Prepare four strategies, one assigned instance per family, two public development families, two private validation families and eight private held-out families. Use local software-maintenance tasks with one permitted file replacement. Keep the published GAAP and RunInvariant corpora frozen. The working example exercises the host, the selector and the independent evaluator with inert data. Provider execution, optimization, product adoption and benchmark claims require later decisions.

## Package Layout / File-to-Task Mapping

Keep the study in `experiments/feedback-repair/`. Put the host recorder and its regression tests in `recorder.py` and `test_recorder.py`. Put the independent corpus, evaluator, public fixtures and their tests alongside them. Put the cost estimate and its arithmetic in that directory. The study guide owns the CLI sequence, strategy definitions, resource and decision rules. Update the GAAP program checklist and run offline tests in existing CI. Keep private family source and salts outside Git and the candidate workspace.

## Dependencies And Settings

Use the Python standard library, inert JSON replacements and new temporary workspaces. No network, provider credentials, containers, live model or package installation is required for controls. Pin the intended provider profile and service class in the estimate. Record the actual Python and host implementation used for each rehearsal. A future trial needs its own isolated runner and actual invocation evidence.

## Canonical Contracts

Each assignment binds a split member and arm before a run. All arms share starting bytes, model, tools, policy and limits. Only feedback changes. The selector sees public outcomes. The independent evaluator sees the final selected bytes after selection ends. The recorder owns attempt history and host observations. A candidate can't award itself credit or supply usage as fact. Exact bytes bind the packet to the inputs, policy, evaluator, fixture membership and host code. A separate expected digest detects replacement. The source of that digest controls its trust value.

## TDD And BDD Implementation Strategy

Exercise public gold, decoy and absent proposals through the normal offline path. Given a change that passes a public check but loses a required value, the independent evaluator rejects the final workspace. Given a second arm, it starts from the same initial bytes. Given no provider run, all 48 proposed assignments remain unrun. Changing a bound input or packet invalidates verification. Tests also check the private/public boundary and the selection order.

## Component Design

The host resets an empty workspace from known inputs, admits one bounded replacement, captures its result independently and retains resource observations. The evaluator reads the observed workspace with its own expected final semantics and protected file checks. Publish two development families and opaque commitments for private members. Keep validation and held-out answers in operator custody. Freeze a strategy's revision or sampling rule, lesson source and choice of candidate before scoring. Reserve one deterministic scoring invocation per assigned cell.

## Failure And Recovery Rules

An uncertain effect stops the rehearsal. No silent retries or scoring of a different output. Retain missing proposals, invalid records, verification failures and unexposed opportunities. A missing private bundle prevents any claim about recovered private inputs. Do all rehearsal work in fresh scratch directories. Treat an incomplete packet as incomplete. A complete packet requires checks against a separately retained digest and original inputs. A future provider run also needs native usage and independent account reconciliation.

## Commit Plan

Commit the plan first. Keep the corpus and host tests with the behavior they verify. Commit the study guide, price estimate, program record and CI entry with the offline preparation before requesting final review.

## Branch And PR Flow

Use the isolated branch `study/issue-29-feedback-preparation` from `81cad26459c262a0b8b5c6bec5c07deee2a71371`. Submit one PR under #29. Resolve supported review findings, retest changed behavior and repeat review. Guard the reviewed SHA at merge. Compare trees and check main at the merged SHA.

## Test Plan

Run standard-library tests and a fresh rehearsal with an independent verification command. Check the public/private split shape, duplicate IDs, altered inputs and selection on public observations. Check scorer, tool, token, retry and wall-time ceilings including resampling. Verify the worked estimate against the dated provider price. Run the CI lane selector tests when workflow wiring changes. State which process supplied each observation and which properties remain untested.

## Definition Of Done

The offline guide reproduces pass, reject and missing-output outcomes, retains the assigned denominator and reports zero provider usage. The private source can reproduce its published commitments in owner custody. An independent reader can verify the rehearsal. The packet and program record identify the spending, isolation, billing and evidence gates for any trial. A reviewed PR has matching merged content and successful main checks.

## Assumptions And Defaults

The proposed 48-cell comparison is exploratory. Keep the proposed numerical decision rule and spending cap subject to approval. An offline control supplies measurement evidence only. Tasks published in Git qualify only for development. A private split commitment identifies the custodian's bytes. Demonstrating candidate isolation during a later trial requires separate evidence.
