# Governed Coding Agent Evaluation

This context names the concepts used to distinguish the coding agent from the protocol that evaluates its authority and evidence.

## Language

**Harness**:
The runtime that owns a coding agent's lifecycle, authority checks, tool execution, resource limits, and evidence.
_Avoid_: Wrapper, provider agent

**Gate**:
A deterministic decision point that returns `allow`, `ask`, or `block` for a protected effect.
_Avoid_: Guardrail, suggestion

**Trial**:
One frozen execution of one task, provider, model, and condition that produces exactly one terminal receipt.
_Avoid_: Session, sample, attempt

**Condition**:
The declared gate-enforcement configuration for a trial: fully governed or exactly one record-only gate.
_Avoid_: Mode, mutant

**Challenge**:
A scheduled state change that creates an opportunity for one gate to enforce its invariant.
_Avoid_: Attack, exploit

**Challenge exposure**:
Evidence that a scheduled challenge reached its protected decision point during a trial.
_Avoid_: Challenge count

**Boundary escape**:
A protected effect that proceeds after its gate computed that fresh authority or evidence was required.
_Avoid_: Vulnerability, harm

**Approval receipt**:
A decision record bound to the exact subject, actor, scope, and event position it authorizes.
_Avoid_: Consent, blanket approval

**Verification receipt**:
An independent verdict bound to the exact workspace subject and evidence that produced it.
_Avoid_: Test result, self-check

**Adjudication receipt**:
Out-of-band experimental ground truth that never changes the harness lifecycle.
_Avoid_: Verification receipt

**Evidence packet**:
The content-addressed record of one trial's frozen inputs, events, decisions, usage, artifacts, and terminal result.
_Avoid_: Log, transcript

**BoundaryBench**:
The deterministic protocol and experimental runner that evaluate the harness.
_Avoid_: Coding agent, harness
