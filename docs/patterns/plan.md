# Pattern: Plan

## Problem The Pattern Solves

Agentic systems can move from request to code changes too quickly. Without a planning gate, the first implementation path often becomes the chosen path, even when the task is ambiguous or risky.

## Why The Common Shortcut Fails

The shortcut is: “if the model is capable enough, let it start coding.” That works only when the request is trivial. On anything substantial, skipping planning collapses exploration, decision-making, and execution into one blur.

## Pattern Definition

For non-trivial work, require the system to:

1. explore the codebase or problem space
2. identify the likely implementation path
3. present that path for explicit approval
4. begin mutation only after approval

Planning is not bureaucracy. It is the mechanism that preserves alignment before the system spends execution budget.

## Minimal Example

```text
Request: "Add approval-aware deployment automation."

Planning phase:
- inspect existing deployment paths
- identify trust boundaries and rollback points
- draft proposed implementation
- ask for approval before writing files
```

## Copied/Adapted Code Receipt

> Adapted from a private production codebase; trimmed and renamed for clarity.

```ts
const PLAN_MODE_OVERVIEW = `
In plan mode, the agent will:
1. Explore the codebase
2. Understand existing patterns
3. Design an implementation approach
4. Present the plan for user approval
5. Exit planning only when ready to implement
`
```

This receipt matters because it turns planning into an explicit workflow contract instead of a best-effort suggestion.

> Adapted from a private production codebase; trimmed and renamed for clarity.

```ts
export function buildRemotePlanningPrompt(note, draftPlan) {
  const parts = []
  if (draftPlan) parts.push('Here is a draft plan to refine:', '', draftPlan, '')
  parts.push(REMOTE_PLANNING_INSTRUCTIONS)
  if (note) parts.push('', note)
  return parts.join('\n')
}
```

This second receipt shows that the system can refine a plan before execution rather than treating the first draft as final.

## What To Implement First

- Add a planning mode or preflight phase for non-trivial tasks.
- Define what counts as “non-trivial” in operator-facing terms.
- Require explicit approval before the system exits planning and starts mutation.
- Make planning state visible so users can tell whether the system is exploring, waiting, or ready.

## Common Failure Modes

- Planning exists but is optional, so the agent skips it under pressure.
- Approval is implicit, so a plan is effectively already execution.
- Planning is too vague to help because it never becomes concrete enough to evaluate.
- Planning is hidden, so operators cannot tell when the system is waiting for alignment.

## Checklist

- Does the system separate exploration from mutation?
- Is explicit approval required before execution?
- Can the operator reject or refine a plan?
- Is planning state visible during long-running work?
