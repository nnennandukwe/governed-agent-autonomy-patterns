# Governed Agent Autonomy Patterns

AI coding systems need a control plane, not just a better model.

This repo is a public, docs-first reference for teams that want agentic coding without chaos. It focuses on the patterns that protect code integrity while still preserving speed: `plan`, `permission`, `verification`, and `tool trust`.

The core reframe is simple: boundaries are not bottlenecks. Good boundaries are how teams get sustainable velocity.

## Why this repo exists

Most discussion about AI coding systems still centers on generation speed. That misses the harder problem. The risk is not that agents can write code quickly. The risk is that they can write and execute changes quickly without enough planning, review, verification, or trust controls.

This repo packages a reusable framework for evaluating and designing governed agent autonomy. It is meant to be easy to repurpose into:

- talk and workshop material
- blog posts and teardown pieces
- internal platform standards
- procurement and evaluation checklists
- lightweight team rollout guides

The same control-plane logic applies after code generation too. Packaging and publish workflows are part of code integrity, not a separate concern.

## The four patterns

| Pattern | Purpose | Core question |
| --- | --- | --- |
| `plan` | Separate exploration from execution | Can the system pause, inspect, and propose before it mutates code? |
| `permission` | Gate risky actions with explicit policy | Can the system distinguish safe, risky, and disallowed behavior? |
| `verification` | Keep implementation and validation independent | Does a separate verifier produce evidence instead of self-grading? |
| `tool trust` | Review risky tools and settings before enablement | Are external capabilities explicitly approved before the agent can rely on them? |

`Visibility` is cross-cutting. If approvals, rejections, status changes, and risk decisions are not visible, the rest of the control plane becomes performative.

## Architecture At A Glance

```mermaid
flowchart TD
  U["Operator"] --> P["Planner"]
  P --> A["Plan Approval"]
  A --> E["Execution Path"]

  E --> G["Permission Gate"]
  G --> T["Tools and Commands"]
  G --> R["Tool Trust Review"]
  R --> T

  E --> V["Independent Verifier"]
  V --> D["Evidence + Verdict"]

  P --> S["Visibility Surfaces"]
  A --> S
  G --> S
  R --> S
  V --> S
```

This control plane keeps generation power inside explicit operational boundaries. The point is not to stop work. The point is to make safe work easy and unsafe work obvious.

## How To Use This Repo

If you are an engineering leader:

- use the [scorecard](docs/scorecard.md) to evaluate tools or internal platforms
- use the [diagrams](docs/diagrams.md) to explain why controls accelerate safe adoption
- use the pattern pages to define rollout expectations for teams

If you are a platform or developer tooling team:

- start with the [pattern pages](docs/patterns/plan.md)
- review the [governed publish pipeline](docs/applications/governed-publish-pipeline.md) if you own release automation
- copy the [examples](examples/plan/planning-prompt.md) into internal docs or prototypes
- adapt the scorecard into design review gates or vendor questionnaires

If you are a practitioner or staff engineer:

- use the pattern docs as a checklist for what to demand from agentic workflows
- use the examples as a starting point for policy files, verifier contracts, and approval records

## What’s In Here

- [Scorecard](docs/scorecard.md): evaluate integrity support instead of speed alone
- [Diagrams](docs/diagrams.md): portable visuals for talks, posts, and internal docs
- [Plan pattern](docs/patterns/plan.md): pre-mutation planning and explicit approval
- [Permission pattern](docs/patterns/permission.md): allow, ask, deny, and dangerous overrides
- [Verification pattern](docs/patterns/verification.md): independent validation with evidence
- [Tool trust pattern](docs/patterns/tool-trust.md): explicit approval for external tools and risky settings
- [Governed publish pipeline](docs/applications/governed-publish-pipeline.md): apply the framework to packaging and release workflows
- [Examples](examples/plan/planning-prompt.md): copyable templates and tiny dependency-free demos

## Source Methodology

The examples and receipts in this repo are adapted from a private production codebase. They are deliberately trimmed, lightly renamed, and annotated for teaching value. The goal is not to publish a hidden product. The goal is to surface the control-plane patterns that matter.

Each adapted excerpt is marked with this note:

> Adapted from a private production codebase; trimmed and renamed for clarity.

## Reading Paths

If you want a quick evaluation path:

1. Read the [scorecard](docs/scorecard.md).
2. Skim the [diagrams](docs/diagrams.md).
3. Go deeper on the weakest pattern in your current workflow.

If you want an implementation path:

1. Start with [plan](docs/patterns/plan.md).
2. Add [permission](docs/patterns/permission.md).
3. Add [verification](docs/patterns/verification.md).
4. Add [tool trust](docs/patterns/tool-trust.md).
5. Make visibility explicit across all four.
6. Apply the same controls to packaging and release with the [governed publish pipeline](docs/applications/governed-publish-pipeline.md).

If you want material to repurpose:

1. Pull the control-plane diagram from [diagrams](docs/diagrams.md).
2. Pull the evaluation criteria from the [scorecard](docs/scorecard.md).
3. Pull one adapted receipt from each pattern page.

## Repurposing Guide

This repo is structured so the same core material can be lifted into multiple formats with minimal rewriting.

- `README` becomes a talk opening, landing page, or long-form article backbone.
- `scorecard` becomes a buyer guide, internal rubric, or platform review worksheet.
- `diagrams` become presentation slides, blog visuals, or onboarding illustrations.
- `pattern pages` become policy docs, team standards, or technical teardown sections.
- `examples` become copyable templates for pilots and internal prototypes.

## Design Principle

Agentic coding systems should not be judged only by how much code they can produce. They should be judged by whether they help teams preserve code integrity while moving quickly enough to matter.
