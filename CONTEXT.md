# Governed Agent Run Integrity

This context names the target runtime concepts and distinguishes the Rust
harness from the external protocol that evaluates its decisions. Concepts for
trials and live execution describe the intended harness; they are not all
implemented by the current `RunCoordinator` slice.

## Language

**Harness**:
The runtime that owns a coding agent's lifecycle, authority checks, tool execution, resource limits, and evidence.
_Avoid_: Wrapper, provider agent

**Run coordinator**:
The Rust module that returns the decision required before a protected effect.
_Avoid_: Provider adapter, policy wrapper

**Agent Run Request**:
The immutable, versioned input that binds one Agent Run to its exact subject, requested capability, constraints, policy identities, resource budget, approval context, and verification requirements.
_Avoid_: Provider request, workflow run request

**Agent Run**:
One bounded execution governed through GAAP's integrity decisions that produces exactly one Terminal Run Receipt.
_Avoid_: ThreadLoop Workflow Run, provider session

**Protected effect**:
One externally observable operation that must be proposed, decided, and recorded against an exact Agent Run, subject, and capability before a runtime may proceed.
_Avoid_: Tool call, provider action, authorized action

**Protected Effect Request**:
The immutable, versioned description of one proposed protected effect, including its parent-run binding, current subject, normalized operation and input digest, requested scopes, identities, and repeatability.
_Avoid_: Tool invocation, permission grant

**Protected Effect Result**:
The content-addressed record that binds one exact Protected Effect Request to the decisive RunCoordinator decision, observed identities, execution status, effect-local usage, and effect evidence.
_Avoid_: Tool response, Terminal Run Receipt

**Execution status**:
The closed result classification that distinguishes executed, awaiting-authority, denied, failed, interrupted, and unknown-outcome effects without changing the RunCoordinator decision.
_Avoid_: Decision, lifecycle state

**Observed subject**:
The repository or artifact identity measured immediately before or after a protected-effect attempt and recorded independently from the subject proposed in the request.
_Avoid_: Requested subject, assumed workspace

**Schema drift**:
A denied pre-execution observation that the requested tool-schema digest is no longer current.
_Avoid_: Parse error, provider incompatibility

**Repeatability**:
The request's declaration that an identical effect is repeatable, idempotent, or non-repeatable; it informs a future runtime but never initiates retry.
_Avoid_: Retry policy, execution status

**Effect evidence reference**:
Content-addressed metadata naming one observed effect fact by closed type, digest, and optional locator without embedding raw output or secrets.
_Avoid_: Raw output, provider message

**Terminal Run Receipt**:
The content-addressed record of an Agent Run's terminal status, decisions, observed effects, evidence, resource usage, and exact resulting subject.
_Avoid_: Log, transcript, Trial result

**Gate**:
A deterministic decision point that returns `allow`, `ask`, or `block` for a protected effect.
_Avoid_: Guardrail, suggestion

**Trial**:
One frozen experimental execution of one task, provider, model, and condition that contributes to an Evidence Packet.
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

**Conformance subject**:
An executable implementation evaluated through the RunInvariant stdin/stdout
process interface.
_Avoid_: Test fixture, imported evaluator

**RunInvariant**:
The separate implementation-independent protocol, fixture corpus, and runner
that evaluate a conformance subject.
_Avoid_: Coding agent, harness
