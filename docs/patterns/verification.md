# Pattern: Verification

## Problem The Pattern Solves

Implementation agents are usually rewarded for finishing. That makes them poor judges of whether the result is actually correct. Verification exists to create distance between completion pressure and truth-seeking.

## Why The Common Shortcut Fails

The shortcut is: “run the test suite and call it done,” or worse, “inspect the code and say it looks right.” That is how teams end up with confident narratives instead of evidence.

## Pattern Definition

Verification should:

1. be assigned to a separate role, process, or agent
2. attempt to break the implementation, not confirm it politely
3. require commands, outputs, and an explicit verdict
4. distinguish real verification from explanation or code reading

The standard is simple: if there is no evidence-bearing check, there is no verification result.

## Minimal Example

```text
Verifier task:
- run the changed workflow
- probe one happy path
- probe one edge case
- capture exact command and output
- end with VERDICT: PASS, FAIL, or PARTIAL
```

## Copied/Adapted Code Receipt

> Adapted from a private production codebase; trimmed and renamed for clarity.

```ts
const VERIFIER_SYSTEM_PROMPT = `
You are a verification specialist.
Your job is not to confirm the implementation works — it is to try to break it.

You are strictly prohibited from modifying the project.
`
```

This receipt matters because it gives the verifier a different job from the implementer. Without that separation, the workflow collapses back into self-review.

> Adapted from a private production codebase; trimmed and renamed for clarity.

```ts
const REQUIRED_REPORT = `
### Check: [what you're verifying]
**Command run:**
  [exact command]
**Output observed:**
  [actual output]
**Result: PASS|FAIL**

VERDICT: PASS|FAIL|PARTIAL
`
```

This is what turns verification from a vibe into a contract.

## What To Implement First

- Separate verification from implementation.
- Require evidence-bearing checks with exact command and output capture.
- Standardize verdict tokens so downstream tooling can parse outcomes.
- Define when `PARTIAL` is acceptable and when it is not.

## Common Failure Modes

- The implementer also acts as the verifier.
- A passing test suite substitutes for targeted verification.
- Reports describe what should happen instead of what did happen.
- `PARTIAL` becomes a polite way to avoid making a decision.

## Release-Time Application

Release verification should validate the shipped artifact, not only the source tree.

- Inspect the built package contents before publish and fail on unexpected files.
- Re-fetch the published artifact from the registry and verify it matches the approved checksum.
- Record the exact commands, outputs, and verdict for the release verification pass.

## Checklist

- Is verification independent from implementation?
- Does every check include command and output evidence?
- Is there a strict verdict contract?
- Does the verifier try edge cases and failure paths, not just the happy path?
