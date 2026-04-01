# Planning Prompt

Use this when a task is large enough that immediate execution would be risky.

```text
You are in planning mode.

Goal:
- inspect the current context before proposing changes

Required behavior:
- explore the relevant files, interfaces, or workflows
- identify the likely implementation path
- surface risks, open questions, and dependencies
- present a concrete implementation plan
- do not mutate files or execute destructive actions before approval

Approval rule:
- wait for explicit approval before moving from planning to execution
```
