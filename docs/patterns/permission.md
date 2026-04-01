# Pattern: Permission

## Problem The Pattern Solves

An agent with tool access can turn a small misunderstanding into a destructive action very quickly. Permission controls keep the system from treating every available capability as equally safe.

## Why The Common Shortcut Fails

The shortcut is: “ask for confirmation only on obviously bad commands.” That breaks down fast. The hard cases are not just obviously bad commands. They are commands that look routine until wrappers, flags, path expansion, or broad allow rules change their real effect.

## Pattern Definition

Permission control should:

1. classify tool actions as allow, ask, or deny
2. use clear precedence rules
3. escalate dangerous operations even when broad allow rules exist
4. prevent wrapper commands from bypassing policy intent

The safest model is not “approve once and hope.” It is explicit policy with dangerous-operation overrides.

## Minimal Example

```yaml
permission_policy:
  allow:
    - "git status"
    - "npm test"
  ask:
    - "git push"
    - "rm *"
  deny:
    - "rm -rf /"
```

The important point is not the syntax. The important point is that policy intent stays clear even when commands get more complex.

## Copied/Adapted Code Receipt

> Adapted from a private production codebase; trimmed and renamed for clarity.

```ts
if (isDangerousRemovalPath(targetPath)) {
  return {
    behavior: 'ask',
    message:
      `Dangerous ${command} operation detected: '${targetPath}'\n\n` +
      `This requires explicit approval and cannot be auto-allowed by rules.`,
    suggestions: [],
  }
}
```

This is the core lesson: dangerous actions should trigger a hard approval boundary even if the surrounding policy is permissive.

> Adapted from a private production codebase; trimmed and renamed for clarity.

```ts
const BARE_SHELL_PREFIXES = new Set([
  'sh',
  'bash',
  'zsh',
  'env',
  'xargs',
  'nice',
])
```

This receipt shows another important guardrail. Wrapper commands can otherwise become policy escape hatches.

## What To Implement First

- Define the allow, ask, and deny outcomes.
- Add dangerous-operation checks for destructive paths and risky flags.
- Add wrapper detection so users cannot bypass intent with shell indirection.
- Make approval prompts specific enough that operators understand what they are approving.

## Common Failure Modes

- Allow rules are too broad, so risky commands slip through.
- Dangerous-path checks happen too late or not at all.
- Wrapper commands bypass the real policy boundary.
- Permission prompts are vague, so users approve without understanding risk.

## Checklist

- Are allow, ask, and deny outcomes explicit?
- Do dangerous actions escalate even when allow rules exist?
- Can wrappers or shell tricks bypass policy?
- Do prompts explain why approval is required?
