# Interactive runtime subject

`cargo build --locked` builds `target/debug/gaap`. Its additive `runtime-subject`
command drives one `AgentRunEngine` through its existing agent, executor and
verifier ports. It doesn't load a case corpus or expected outcomes.

RunInvariant owns the separately versioned runtime protocol and corpus in
[nnennandukwe/run-invariant issue #2](https://github.com/nnennandukwe/run-invariant/issues/2).
The old `gaap run-invariant` decision interface and its evidence stay unchanged.

The host starts one process per case, writes a canonical JSON `start` frame and
replies to ordered `call` frames. Each frame ends with one LF. The adapter checks
canonical bytes before deserializing and limits each input frame to four MiB.
The host owns the total process deadline and output limits. Diagnostics use stderr.

The start frame has `kind`, `protocol`, `request` and `support` fields. Protocol
`run-invariant.runtime/0.2.0` uses the frozen GAAP request/effect/receipt contracts.
Support supplies trusted policies, capabilities, tool-schema approvals bound to capability identities, permission,
executor identity, verifier IDs, the maximum effect count and a host-owned effect usage estimate. These are host-owned
runtime configuration. Tool-schema pins are optional for existing Rust callers;
this subject requires them. A changed or absent schema can't borrow trust from a
same-name capability. The existing coordinator makes the tool-trust decision.

Ports are `clock.adapter_elapsed_ms`, `agent.plan`, `agent.next`, `executor.execute`,
`verifier.verify`, `subject.observe` and `signer.sign`. Calls carry an increasing sequence, port name
and input. Replies carry that sequence and a value. Clock observations follow each adapter operation and account for
elapsed adapter time from run start zero, excluding executor time, including verification and the final subject observation; executor usage is accounted separately. It isn't an absolute monotonic timestamp or wall-clock reading. Time moving backwards
fails the port. Failed operations retain sampled adapter usage through typed
RuntimePortError metadata; without an observation, usage remains a lower bound. The executor never performs file, network or shell operations here;
its host-supplied observations exercise the real engine's effect admission and
receipt path. Cancellation is a scripted agent step. Lost executor responses remain
unknown outcomes, while process death produces no successful conformance receipt.

The terminal frame includes the subject identity, full sealed receipt and ordered
protected-effect results. Its start digest hashes the exact start frame with LF. Its request digest hashes
the canonical Agent Run Request without LF, matching the receipt.
Its transcript digest hashes all call/reply frames with LF, excluding start and
terminal. The signer receives the start-frame, Agent Run Request and receipt-body digests and returns
a public fixture signature. The host independently verifies that binding. This
signature isn't a GAAP in-toto/DSSE statement or production signer trust; the accepted
attestation adapter retains that separate responsibility.

After a valid verification, `VerifierPort::completion_check` takes a fresh host
subject observation before completion. A changed subject fails the run; a clock
observation over budget blocks it. The failed receipt retains the last governed
subject; the transcript separately records the later physical observation.

One terminal frame ends a run. The adapter accepts EOF and exits 0; any later input
is refused with exit 1 without further stdout. Consumers must wait for the process exit before accepting
a packet, including when a valid receipt precedes a rejected follow-up input. There's no persistence, resume,
sandbox-strength or production-safety claim. Passing proves only the named scripted
cases against the recorded subject binary and bytes.

A lost executor reply retains host-provided evidence in its original order. The
engine adds references only for required categories the host didn't supply; it
retains the known one-call usage lower bound without inventing an external result.

The subject uses GAAP's existing inclusive cost hard-stop policy at the request's
maximum cost: an estimate that reaches that threshold is blocked before dispatch.
The request budget remains an upper bound, and the runtime policy can stop earlier.
Sampled executor-failure usage survives in the receipt, including setup usage when
dispatch was refused. An attempted effect still counts at least one tool call.

Invalid frame bytes or reply envelopes end the protocol without another call or a
terminal frame. A valid reply describing a failed operation can still produce a
failed receipt. When an operation fails while exceeding a budget, its failure
remains the primary terminal reason and the receipt retains all observed usage.
Completion-claim failures follow the same accounting rule. Contradictory evidence
on an unknown executor outcome is rejected; known failures or interruptions must
use the matching executor observation instead of a lost-response error.
