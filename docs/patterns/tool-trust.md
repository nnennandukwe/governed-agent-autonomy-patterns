# Pattern: Tool Trust

## Problem The Pattern Solves

The moment an agent gains access to a new tool, server, or settings surface, its effective power changes. Tool trust is the pattern that prevents those capability changes from happening silently.

## Why The Common Shortcut Fails

The shortcut is: “if the tool is useful, enable it and review behavior later.” That assumes the burden of proof runs in the wrong direction. In practice, new capability should stay untrusted until reviewed.

## Pattern Definition

Tool trust means:

1. identify tools, servers, and settings that expand agent power
2. detect new or changed trust-sensitive capability
3. block or pause until an operator reviews the risk
4. record the approval or rejection clearly

This pattern matters most where tools can execute code, access secrets, or change the operating environment.

## Minimal Example

```text
New capability detected:
- external server added
- dangerous setting changed

Required response:
- show blocking review
- approve or reject explicitly
- keep state visible
```

## Copied/Adapted Code Receipt

> Adapted from a private production codebase; trimmed and renamed for clarity.

```ts
export async function handleToolApprovals(root) {
  const pendingTools = getPendingProjectTools()
  if (pendingTools.length === 0) return

  await showApprovalDialog(root, pendingTools)
}
```

This receipt shows the right posture: unreviewed external capability stays pending until someone decides.

> Adapted from a private production codebase; trimmed and renamed for clarity.

```ts
export async function checkManagedSettingsSecurity(
  previousSettings,
  nextSettings,
) {
  if (!containsDangerousChanges(previousSettings, nextSettings)) {
    return 'no_check_needed'
  }

  return showBlockingApprovalDialog(nextSettings)
}
```

This second receipt makes the same point for configuration. Trust-sensitive settings changes should not be applied silently.

## What To Implement First

- Define which tools and settings count as trust-sensitive.
- Detect new tools, changed settings, and newly reachable external servers.
- Block activation until an operator reviews the change.
- Store approval decisions somewhere operators can inspect later.

## Common Failure Modes

- Trust review exists only in documentation, not in runtime behavior.
- Settings changes apply before the operator can react.
- Tool approvals are one-time and overly broad.
- The system has approval prompts, but no durable record of what changed.

## Release-Time Application

Packaging and publish infrastructure expands agent or operator power too.

- Changes to packaging config, release scripts, registry targets, or publish credentials should trigger explicit review.
- A new registry, artifact source, or signing path should stay untrusted until approved.
- Publish tooling should leave a durable approval record that shows what changed and who accepted the risk.

## Checklist

- Are trust-sensitive capabilities detected explicitly?
- Does the system block or pause before enabling them?
- Can an operator reject a capability change cleanly?
- Is the trust decision recorded and visible later?
