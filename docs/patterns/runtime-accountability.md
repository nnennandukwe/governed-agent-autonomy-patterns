# Pattern: Runtime Accountability

## Problem The Pattern Solves

Autonomous workflows can be technically running while remaining operationally invisible. Runtime accountability exists so teams can supervise authority, trace activity, and intervene when spend or quota thresholds are crossed.

## Why The Common Shortcut Fails

The shortcut is: "log a few events and add a dashboard later." That misses the real job. Runtime accountability is not passive observability. It is the gate that makes execution state, traceability, quota, and spend governable while the system is still acting.

## Pattern Definition

Runtime accountability should:

1. expose meaningful execution state and current phase
2. tie usage and spend back to a request, session, team, or workflow
3. evaluate quota, balance, or policy thresholds during execution
4. require explicit allow, ask, or stop behavior when thresholds are crossed

The standard is simple: if the system can act autonomously, operators should be able to see what it is doing, where it is running, and what it is costing while it runs.

## Minimal Example

```text
Runtime accountability layer:
- record execution target and current phase
- attach usage to a request or trace ID
- evaluate quota and spend thresholds
- alert, ask, block, or stop explicitly when limits are crossed
```

## Copied/Adapted Code Receipt

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

This receipt matters because remote authority should not change silently. Operators need inspectable state while a workflow is still live.

> Adapted from a private production codebase; trimmed and renamed for clarity.

```ts
export function checkOverageGate(context) {
  if (!context.extraUsageEnabled) return { outcome: 'block' }
  if (context.lowBalance) return { outcome: 'ask', reason: 'low_balance' }
  return { outcome: 'continue' }
}
```

This second receipt shows that spend is not outside governance. When a system can trigger remote work, low-balance and overage states become control surfaces too.

## What To Implement First

- Define the execution states operators need to see.
- Attach usage and spend to stable identifiers such as request IDs, trace IDs, or session IDs.
- Define the thresholds that trigger alert, approval, block, or stop behavior.
- Record enough state to reconstruct execution target, phase transitions, and intervention decisions later.

## Common Failure Modes

- The system is running, but operators cannot tell what phase it is in.
- Usage and spend exist, but cannot be attributed to a request, workflow, or team.
- Low-balance or quota-exhaustion states surface only after the run finishes.
- Runtime accountability becomes a dashboard project instead of an execution gate.

## Release-Time Application

The same gate applies to packaging and publish workflows.

- Record the publish result, artifact provenance, and rollback handle after release.
- Make release approvals, publish outcomes, and intervention points visible.
- Apply explicit stop or approval behavior when release or cost thresholds are crossed.

## What To Copy First

- Use the [execution state record](../../examples/runtime-accountability/execution-state-record.md) to define the operator-visible state surface.
- Use the [quota threshold policy](../../examples/runtime-accountability/quota-threshold-policy.md) to specify alert, approval, block, or stop behavior.
- Use the [cost attribution example](../../examples/runtime-accountability/cost-attribution-example.md) to tie usage and spend back to request or session IDs.
- Use the [overage approval record](../../examples/runtime-accountability/overage-approval-record.md) when extra usage requires explicit approval.

## Checklist

- Can operators see current phase and execution target while the system is running?
- Can usage and spend be tied back to a stable request, trace, or session ID?
- Are threshold crossings handled with explicit allow, ask, block, or stop behavior?
- Can a later incident review reconstruct what the system did and what it cost?
