# Rehearse a feedback repair study offline

A repair can pass the check an agent sees and still remove a required setting. Experiment B asks whether a lesson drawn from feedback helps repair that kind of error at equal cost. This directory lets a reviewer test the measurement tools on public configuration repairs before anyone funds a provider comparison.

On Linux or macOS, use Python 3.11 or newer. Run these commands from this directory with a fresh scratch location.

```sh
python3 -B -m unittest discover -v
python3 -B corpus.py verify
scratch=$(mktemp -d)
python3 -B recorder.py rehearse "$scratch/packet" > "$scratch/expected-root"
python3 -B recorder.py verify "$scratch/packet" --expected-root "$(cat "$scratch/expected-root")"
```

Keep the root digest separately if another person will verify the packet. The verifier needs the same study source and public fixtures. Anyone who can replace both the packet and its expected root can substitute a different recording. A checkout of a reviewed commit establishes which source you're checking. Verification reconstructs the selected public repairs in fresh scratch directories. These replay checks are separate from the scoring calls charged to each rehearsal.

## What the rehearsal measures

A scripted proposal supplies replacement JSON. The host starts each attempt from known file contents, applies the permitted replacement and observes its effect. Public checks can guide selection. After selection, the local evaluator reads the workspace and checks the selected result and protected files against the task’s expected state. The script runs as trusted host code. The submitted JSON never executes. A public decoy exercises the case where the public check passes and final scoring rejects the change. The rehearsal also exercises a working repair and a missing proposal. Its counts describe these controls.

The proposed study has **48 assigned cells and zero provider trials run**. Each of twelve families has one assignment per arm. Two families are public development inputs. Two validation families and eight held-out families have opaque public identifiers and commitments. The custodian keeps the original private inputs, expected answers and salts outside the repository. The following owner-only check recomputes commitments without publishing those inputs.

```sh
python3 -B corpus.py verify --private-dir "$OWNER_BUNDLE"
```

Public CI can verify the development fixtures and split structure. A private commitment alone can't prove that an eventual candidate had no access to an answer. The task author has seen the private answers. A candidate with access to the preparation session or bundle has also seen those inputs for study purposes. Custody loss or prior exposure invalidates the affected comparison. Before a live run, demonstrate the isolation of the actual candidate process and evaluator, bind the immutable runner image and record who had access. Keep private evaluators and cross-cell history out of the candidate's workspace and tools.

## What changes between arms

Use the same starting subject, base instructions, model, tools, policy and maximum resources for each arm. All arms get the same first request, with no arm label. The raw arm submits its first proposal. The revision arm may revise once after a failed public check. The lesson arm links the same revision to its own observed failure. Resampling starts fresh and selects the first public pass in generation order, or the first proposal when none passes. The final scorer can't guide this choice. Charge every candidate, retry, check and selection to its originating cell. [Arm instructions and selection rules](study.json)

A completion claim belongs to the selected replacement only when its response explicitly claims completion. Bind that flag to the selected bytes before final scoring. Retain unknown claims and unresolved verdicts.

Pair results by family. Report verified repairs per attempted task, false completions per attempt and per claimed completion, false blocks per legal control, and escapes per exposed opportunity. Preserve failed, blocked, timed-out, incomplete and unattempted assignments. Report evidence failures and uncertainty, and keep zero-denominator rates undefined. Record total and per-task latency and reconciled cost including unsuccessful work. An interrupted trial with an uncertain effect stays unresolved.

## When a larger test would be warranted

On the eight held-out families, the lesson arm would need at least two more verified repairs than each of raw feedback and equal-budget resampling, zero new mandatory failures and median per-task cost and latency no higher than resampling. Report revision outcomes separately. Missing evidence for a required comparison leaves the decision open. One run per cell supports exploratory reporting. Adoption needs independent replication on newly frozen families and an accepted cost and latency tradeoff.

The maintainer selected offline preparation. Authorized spend is **$0**. Before running any provider trial, freeze the exact request settings and base prompt, candidate/runner identity, independent evaluator, family and split commitments, exposure record, total resource limits and decision rule. Verify current prices and implement pre-dispatch budget enforcement, isolated execution and native usage reconciliation. Record a separate spending decision. The dated estimate identifies its service assumptions and the maximum provider cost under the proposed limits. [Cost estimate](COST.md) · [Maintainer decision](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/29#issuecomment-7071790843)

