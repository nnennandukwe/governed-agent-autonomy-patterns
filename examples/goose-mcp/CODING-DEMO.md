# Goose coding demo and live run inspector

Run from your normal checkout:

```sh
cd ~/Code/governed-agent-autonomy-patterns
./demo start
```

This builds the Rust example, creates a fresh visible session under `demo-runs/`,
opens its local inspector, and opens the restricted Goose Desktop recipe.
In Goose, choose **Trust and Execute**, then select the coding activity.
Keep the launcher terminal open for the inspector. No terminal approvals or
request-ID copying are part of this coding flow. Goose starts its MCP server.

Prerequisites: Rust 1.96, Python 3.9+, the pinned Goose runtime installed by
`python3 examples/goose-mcp/install_goose.py`, and Goose Desktop signed in to
ChatGPT Codex. The recipe uses the laptop's verified `gpt-5.5` model. Only the
three GAAP tools are enabled; normal Goose extension defaults stay unchanged.

## The live story

1. Goose reads a small Python project and its task.
2. It proposes a plan and the complete shipping function implementation.
3. The existing `AgentRunEngine` records the plan, evaluates permissions and
   tool trust, admits resource use, authorizes the write, executes it, invokes
   an independent verifier, and authorizes or blocks completion.
4. Eight acceptance cases check actual Python outputs. Incorrect code remains
   visible with failed verification; Goose can propose a corrected change.
5. After a verified change, Goose requests activation in `deployment.json`.
   GAAP's permission gate denies the protected write. The code remains fixed;
   the deployment settings remain unchanged. Goose explains both outcomes.

The deployment file is a local fixture, not a live production service. Each
proposed change is one bounded Agent Run; the inspector groups these into the
Goose session. It displays actual engine events and validated Terminal Run
Receipts, not the older example's coordinator-only decision log. Events appear
when each short bounded run finishes; a durable running marker covers attempts
in progress or left uncertain after an interruption.

## Walk through the five pillars

Select the completed implementation run to show the full sequence. Select the
production request to show the permission block. Later gates correctly read
**Not evaluated** when an earlier gate stops execution.

The run explorer uses a compact run table and selectable engine-event trace.
Selecting an event opens its attributes, decision code, and evidence references.
The five pillar controls jump to their recorded evidence. **Changes** shows the
before and observed-after files, **Verification** shows the actual acceptance
cases, and **Receipt** shows identities, usage, and the complete source record.
The trace is ordered by event sequence, not time-scaled: per-event timestamps
are not recorded. Local run duration and accounted execution time are displayed
separately; session elapsed is explicitly the sum of bounded run durations.

| Pillar | Working mechanism |
| --- | --- |
| Planning | Goose supplies a concrete plan. Its digest binds the proposed contents and base subject; the engine records the local policy's preauthorized task scope. This is policy authorization, not a claimed human review of model prose. |
| Permissions | The adapter derives allow for `shipping.py`, deny for other paths. The existing engine evaluates the permission gate before any executor call. |
| Tool trust | Runtime-owned trust is bound to the actual advertised MCP descriptors and schemas. |
| Verification | A separate verifier executes a restricted Python function against eight acceptance cases, rechecks the current project, and returns evidence bound into the terminal receipt. |
| Runtime accountability | One effect and two accounted calls per run; a ten-second engine accounting budget, a five-second verifier process deadline, a twelve-run session ceiling, and explicit execution/terminal outcomes. |

The verifier permits a small arithmetic/conditional Python subset. Imports,
attributes, loops, arbitrary calls, and unbounded integer growth are excluded.
This is an example execution boundary, not production OS isolation. Other
native processes running as the same user are outside the threat model. The
file lock serializes mediated writers; it is not an atomic compare-and-swap
against native editors. Make operator edits between calls. Do not
enable ambient shell/filesystem tools in this Goose recipe.

Engine usage covers local execution and verification. Goose's model tokens,
provider cost, and reasoning time are not measured by this MCP boundary, and the
inspector says so. The CLI rehearsal has a separate 90-second process watchdog;
Goose Desktop uses its recipe turn limit and MCP timeout. Receipts are locally
integrity-checked, not signed attestations.

## Inspect, rehearse, and recover

```sh
./demo rehearse
./demo status demo-runs/YOUR-SESSION
./demo view demo-runs/YOUR-SESSION
```

A rehearsal makes a real model-driven Goose request, verifies completed code and
a denied deployment change, and saves a transcript plus `rehearsal.json` beside
the session. Two successful runs under seven minutes are the stage acceptance
check. Saved sessions can be reopened in the inspector as a clearly labeled
recorded run if the network fails.

The inspector binds only to loopback, uses a session-specific URL, serves fixed
assets, and provides read-only access. It revalidates receipts and evidence when
reading state. Stopping the inspector preserves files and records. Initialization
never resets an existing session. A surviving running marker after a crash
means the outcome requires inspection; the system never silently retries it.
Keep sessions in their original locations because records bind filesystem identity.

The original release-file/approval example and deterministic engine scenarios
remain available through `examples/goose-mcp/demo.py`. Full multi-step ACP
orchestration, provider-wide budgets, signed receipts, general-purpose process
isolation, and ThreadLoop receipt ingestion remain separate milestones.
