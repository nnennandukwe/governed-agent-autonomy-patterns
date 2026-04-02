# Governed Agent Autonomy Scorecard

Use this scorecard to evaluate whether an AI coding system supports code integrity instead of just accelerating output.

![Integrity-first evaluation scorecard](./assets/diagrams/integrity-first-evaluation-scorecard.png)

The visual uses `telemetry gate` as shorthand. In the repo taxonomy, that fifth control surface is named `runtime accountability` because it includes execution state, traceability, quota, and spend attribution together.

Use the visual as the quick scan. Use the detailed rubric below when you need evidence and next actions.

## How To Score

Use the same scale for every category:

| Score | Meaning |
| --- | --- |
| `0` | Missing. The gate is absent or mostly aspirational. |
| `1` | Partial. The gate exists, but it is easy to bypass or too narrow to trust. |
| `2` | Strong. The gate works in normal use and blocks obvious failure modes. |
| `3` | Control-plane grade. The gate is explicit, enforced, and visible to operators. |

## 1. Planning Gate

### What good looks like

- The system can inspect and reason before mutating code.
- Significant tasks produce a proposed plan instead of immediate execution.
- Execution starts only after explicit approval.

### Failure mode when absent

The agent jumps from request to mutation. Teams get fast output, but they lose the chance to catch scope errors, bad assumptions, and risky implementation paths before the tool makes changes.

### Questions to ask

- Can the system separate exploration from execution?
- Does it present a plan for approval before non-trivial changes?
- Can the operator reject or refine a plan without losing context?

### Rubric

| Score | Planning gate behavior |
| --- | --- |
| `0` | No planning mode. The agent executes immediately. |
| `1` | Planning exists as guidance, but it is optional or easy to bypass. |
| `2` | Planning is the default for meaningful work, with explicit approval before execution. |
| `3` | Planning is enforced, approval-aware, and tied to visible task state. |

### Receipt

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

This is the difference between "think first" as a slogan and "think first" as an operating rule.

## 2. Permission Gate

### What good looks like

- The system distinguishes safe, risky, and disallowed actions.
- Dangerous operations require explicit approval even when broad allow rules exist.
- Policy matching has clear precedence and cannot be trivially bypassed through wrappers or shell tricks.

### Failure mode when absent

The agent inherits too much trust. Operators think they have policy, but the real behavior is "allow unless someone notices a problem."

### Questions to ask

- What happens when a command is risky but technically matches an allow rule?
- Are dangerous paths and destructive operations escalated explicitly?
- Can wrapper commands bypass deny or ask policies?

### Rubric

| Score | Permission behavior |
| --- | --- |
| `0` | No meaningful policy boundary. The agent can run almost anything. |
| `1` | Some prompts exist, but precedence is unclear and dangerous cases leak through. |
| `2` | The system enforces allow, ask, and deny logic with explicit dangerous-action checks. |
| `3` | Policy is enforced, wrapper-resistant, and designed around safe defaults. |

### Receipt

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

This is what "safe defaults" looks like in practice: dangerous operations do not get a free pass just because a user previously allowed something nearby.

## 3. Tool Trust Gate

### What good looks like

- New tools, servers, and risky settings are reviewed before enablement.
- The system blocks or pauses when trust-sensitive capabilities change.
- Approval is explicit and stateful, not implied by use.

### Failure mode when absent

The system treats every new capability as harmless until proven dangerous. That reverses the burden of proof.

### Questions to ask

- How are new tools or external servers approved?
- What happens when a settings update introduces new risky behavior?
- Can the system block until an operator approves trust-sensitive changes?

### Rubric

| Score | Tool trust behavior |
| --- | --- |
| `0` | Tools and settings are enabled with no meaningful review. |
| `1` | Review exists, but it is informal or easy to miss. |
| `2` | Trust-sensitive changes trigger explicit review. |
| `3` | Trust reviews are blocking, scoped, and visible to operators. |

### Receipt

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

This is the right order of operations: detect trust-sensitive change first, then block until a human decides.

## 4. Verification Gate

### What good looks like

- Verification is assigned to a separate role or process.
- The verifier must run evidence-bearing checks, not just inspect code.
- The system requires an explicit verdict contract.

### Failure mode when absent

The agent grades its own homework. Passing output becomes a narrative style instead of a tested outcome.

### Questions to ask

- Is verification performed by a separate agent, workflow, or human?
- Are checks required to include commands, outputs, and a verdict?
- Can a verifier fail work even when the implementation looks polished?

### Rubric

| Score | Verification behavior |
| --- | --- |
| `0` | No independent verification. The implementer self-certifies. |
| `1` | Some tests run, but verification is weak or optional. |
| `2` | A separate verification path exists and expects evidence. |
| `3` | Verification is independent, adversarial, evidence-bearing, and contract-driven. |

### Receipt

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

This matters because "I reviewed the code and it looks fine" is not verification.

## 5. Runtime Accountability

### What good looks like

- Operators can see execution state, current phase, and execution target while work is in flight.
- Usage events, traces, and spend are attributable to a request, session, workflow, or team.
- Quota, low-balance, and overage thresholds trigger explicit allow, ask, or stop behavior.
- Important state transitions and approvals are visible enough to support incident review or audit trails.

### Failure mode when absent

Autonomous work can be technically running while remaining operationally invisible. Teams lose the ability to supervise authority, understand spend, or intervene when thresholds are crossed.

### Questions to ask

- Can operators see what phase the system is in and where it is running?
- Can usage and spend be tied back to a specific request or session?
- What happens when quota is exhausted, balance is low, or overage is required?
- Are threshold crossings visible enough to support approval, alerting, or shutdown?

### Rubric

| Score | Runtime accountability behavior |
| --- | --- |
| `0` | Execution state and spend are mostly invisible. Threshold behavior is unclear. |
| `1` | Some state or usage data exists, but it is inconsistent, delayed, or hard to act on. |
| `2` | Operators can observe execution state and the system enforces basic quota or spend boundaries. |
| `3` | State, traceability, quota, cost attribution, and threshold-based intervention are explicit and operator-usable. |

### Receipt

> Adapted from a private production codebase; trimmed and renamed for clarity.

```ts
pollRemoteSession(sessionId, update => {
  setRunState({
    phase: update.phase,
    executionTarget: update.executionTarget,
    rejectCount: update.rejectCount,
  })
})
```

This matters because remote authority should not change silently. A team needs inspectable state while an autonomous workflow is running.

> Adapted from a private production codebase; trimmed and renamed for clarity.

```ts
export function checkOverageGate(context) {
  if (!context.extraUsageEnabled) return { outcome: 'block' }
  if (context.lowBalance) return { outcome: 'ask', reason: 'low_balance' }
  return { outcome: 'continue' }
}
```

This is the other half of runtime accountability: a system that can spend remotely also needs explicit threshold behavior when budget constraints become real.

## Comparison Worksheet

Use this worksheet when reviewing vendors, internal prototypes, or platform upgrades.

| Category | Score (0-3) | Evidence | Risk if weak | Next action |
| --- | --- | --- | --- | --- |
| Planning gate |  |  |  |  |
| Permission gate |  |  |  |  |
| Tool trust gate |  |  |  |  |
| Verification gate |  |  |  |  |
| Runtime accountability |  |  |  |  |

## Interpreting Results

- `0-4`: Unsafe for serious adoption.
- `5-9`: Promising, but governance is still too thin.
- `10-12`: Strong baseline for controlled rollout.
- `13-15`: Mature control-plane posture.

## Applying This Scorecard To Release Pipelines

Use the same five gates when evaluating packaging and publish automation.

- Planning gate:
  Is there an approved release plan that names the tag, commit, registry target, and expected artifact before publish starts?
- Permission gate:
  Can publishing happen only from CI or another controlled path, with local or ad hoc publish blocked by policy?
- Tool trust gate:
  Do packaging config, registry targets, credentials, and release-tooling changes trigger explicit review before they take effect?
- Verification gate:
  Does a separate verifier inspect the built artifact, validate the file inventory, and confirm the published package matches what was approved?
- Runtime accountability:
  Can operators see the publish result, artifact provenance, approver, rollback path, and any threshold-based release or cost intervention after the release completes?

If the release path scores lower than the coding path, the system still has an integrity gap. Safe generation does not help much if unsafe packaging can ship the wrong artifact.
