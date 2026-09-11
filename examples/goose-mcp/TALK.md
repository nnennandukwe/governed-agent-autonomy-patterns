# Current live segment

Use [the coding demo and live inspector](./CODING-DEMO.md) for the hands-on
portion: Goose implements a shipping function, the full engine verifies it, and
a later protected deployment-configuration request is denied. Each change is
one bounded run; all five pillars are visible in the inspector. Qodo supports
the development walkthrough; ThreadLoop remains the outer lifecycle roadmap.

The release-file approval sequence below is retained as an earlier alternative.

# Governed Agent Autonomy: Building a Control Plane for Agentic Systems

AGNTCon + MCPCon Japan · September 11, 2026 · 15:35–16:00 JST · Hall C.
[Published session](https://sched.co/2QlES). Advanced session, delivered in English.

## The premise

Governed autonomy is an architectural property of the harness around an agent.
Engineers can make planning, permissions, tool access, verification, and resource
limits explicit, then preserve those controls as execution strategies change.

This is a case study in building those controls with an open-source Rust engine,
connecting the ideas to a software-delivery workflow, and using goose as the
agent making real requests. Qodo appears as part of the development process.
The evidence on stage is the code and its behavior.

## 0–3 minutes: why build this?

> An agent harness determines what an AI system can see, what it can do, how it
> gets feedback, and when it stops. Once agents can change files, call external
> tools, and consume resources, those become engineering decisions with consequences.
>
> My investigation of Claude Code's architecture led me to document five patterns
> for governed autonomy. I've been turning those patterns into an open-source Rust
> engine, connecting them to a broader software-delivery workflow, and exploring
> how they apply to an open agent like goose.
>
> I'll walk through the working architecture, demonstrate a governed operation,
> and show which patterns you can adopt when building your own harness. Then
> we'll look ahead: what controls need to persist when agents start improving
> the harnesses themselves?

Give the architecture investigation its historical context, including the public
source-code leak that prompted it. Then move to the patterns and your original
implementation. Explain the decisions engineers can reproduce without requiring
attendees to know either project's history.

Open loop: **What exactly did the operator approve—and does it still describe
what the agent is about to do?** Close it with the stale-file demonstration.

## 3–8 minutes: five patterns, one working engine

Walk through execution order:

**Request → plan → authorize operation → execute → account for usage →
independently verify → authorize completion → produce receipt.**

| Pattern | Concrete engineering choice | Show |
| --- | --- | --- |
| Planning | Make intended work and approved scope inspectable | Request, plan digest, proposed effect |
| Permissions | Authorize a concrete operation immediately before execution | `RunCoordinator`, `ask` / `allow` / `block` |
| Tool trust | Bind trust to the actual capability and schema | Capability digest |
| Verification | Require evidence for the current resulting artifact | Separate verifier, subject digest, completion decision |
| Runtime accountability | Track usage and retain distinct outcomes | Engine events, usage, terminal receipt |

Run `python3 demo.py engine completed`. Label this **Engine view: deterministic
ports, existing AgentRunEngine**. Point to a few execution events and the
verified receipt. The output labels its fixture usage and evidence. Keep
`--json` available, but don't read every field aloud.

Show `engine stale-verification` if time permits. The effect occurred; completion
still needs current verification. A completed tool process and a completed run
are different states in the architecture.

Explain the three seams: the **agent** proposes, the **executor** performs an
authorized operation, and the **verifier** evaluates the result. Adapters
translate requests and observations; they do not get to invent authority.

Give ThreadLoop one minute here:

> GAAP governs one bounded Agent Run. ThreadLoop governs which action is next
> and whether the surrounding delivery workflow may advance. The intended
> connection is a receipt from the run informing the lifecycle decision.
> That runtime connection is a subsequent integration milestone.

## 8–10 minutes: building it with OSS tools and Qodo

Show the source tree, the narrow MCP adapter, and a saved structured Qodo review
for this implementation. Explain why you supplied the design context with the
local diff: the reviewer needs to know which decisions the engine owns and what
the adapter is permitted to do.

> I'm using Qodo as I develop this: loading relevant engineering rules and
> reviewing the implementation with its architectural context. It helps me
> strengthen the code as the reference implementation grows.

Use the actual reviewed diff and result. Label local review as local review;
it does not imply a PR was merged or a release shipped. Keep this to one source
view and one review view. Return to the runtime behavior.

## 10–17 minutes: real goose through the MCP boundary

Introduce [goose](https://aaif.io/projects/goose) as an AAIF project and the
replaceable agent runtime for this slice. Goose uses its ChatGPT Codex provider
with `gpt-5.5` and an existing ChatGPT sign-in. Goose owns the model interaction;
GAAP evaluates the requested operation. The core has no provider dependency.

Label this **Goose view: real model-driven MCP requests, RunCoordinator decisions,
real file effects**. This narrower adapter has decision records, not the full
engine's Terminal Run Receipt.

For the app presentation, launch `python3 demo.py desktop /tmp/conference-desktop-1`
with a fresh directory. Use the Goose Desktop conversation for requests and tool
results, and a separate terminal for the operator's diff, approval, and decision
log. Check that only the recipe's `gaap` extension is enabled. Trusting the recipe
in Goose loads the tools; it does not grant approval for a proposed file change.

1. Ask goose to read `release.json` and propose version `1.1.0`. Explain the
   request ID: a digest of the exact proposal and its bindings.
2. Inspect the decision and unchanged file. `ask` is a normal governed state.
3. In the operator pane, approve the exact diff. There is no approval tool in
   goose's tool list.
4. Ask goose to apply the ID. Inspect version `1.1.0`, the actual diff, and the
   recorded decision and execution observation.
5. Ask for version `1.2.0`, approve it, and change the underlying file to `9.0.0`.
   Pause: **“I approved this request. Should the same request still proceed?”**
6. Ask goose to apply the old ID. Reveal `demo.stale_subject`. The operator edit
   remains intact.

Narrate the successful control: the harness checks whether the approval still
matches the operation and its subject. The file is deliberately small so the
audience can inspect the entire effect and reason about the decision.

Keep two successful real rehearsal transcripts ready. If the network fails,
announce “This is a recorded real goose run captured during rehearsal.” Replay
that evidence, then continue with the locally runnable engine.

## 17–22 minutes: what happens when the harness can change?

Define before using the term:

> A meta-harness can propose and evaluate changes to the harness itself:
> context management, prompts, tools, or execution code.

[Meta-Harness research](https://arxiv.org/html/2603.28052v1) explores an outer loop
that proposes harness code using previous candidates, scores, and execution
traces. Explain the mechanism; don't treat research results as guarantees for
this implementation.

Your design position:

> We should make the execution strategy adaptable while keeping changes to
> authority explicit, reviewable, and testable.

These are **proposed design requirements and roadmap direction**:

| Future capability | Control that should persist |
| --- | --- |
| Change prompts, skills, memory, or harness code | Identify the exact configuration behind each result; version and evaluate candidates before promotion |
| Delegate to other agents or harnesses | Keep child authority within explicitly delegated scope; preserve parent–child identity |
| Run deeper or longer loops | Account for descendant work; enforce shared limits, cancellation, and stop conditions |
| Optimize the harness automatically | Separate candidate development from acceptance; measure task success, boundary behavior, cost, and latency separately |
| Recover or revert | Preserve checkpoints and uncertain outcomes; reverting harness code does not undo earlier external effects |

Connect these requirements to GAAP's future adapters, signed receipts,
isolation, and recovery work. ThreadLoop could govern candidate evaluation and
promotion. Describe that as a future connection, not an integrated system
already running today.

Audience question: **If an agent improves its success rate by changing its tool
access, what evidence would you require before promoting that harness?**

## 22–25 minutes: a starting point engineers can use

> Define authority. Mediate one real effect. Verify its outcome. Preserve the
> evidence. Expand capability under those controls.

Give attendees a small first implementation: one effect, one concrete approval,
one current-state precondition, one verifier, one inspectable result. They can
then replace the agent or extend the execution strategy while retaining those
boundaries.

Leave the source and example README on screen for questions. The maturity claim
is a working, inspectable reference implementation. Full ACP orchestration,
production isolation, signed receipts, and ThreadLoop receipt ingestion are
subsequent milestones.
