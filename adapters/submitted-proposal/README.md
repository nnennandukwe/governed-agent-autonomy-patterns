# Submitted-proposal adapter

Use this adapter when an external agent supplies a complete proposal and the host needs to govern one bounded Agent Run. It implements the existing `AgentAdapter` port. The host supplies the executor, verifier and trusted runtime configuration.

The crate has no provider SDK, transport, task filenames, expected answers or persistence. The [inbound MCP shipping reference](../../examples/submitted-proposal/README.md) shows the host wiring. The [outbound MCP client](../mcp/README.md) owns a separate transport role.

## Host wiring

1. Strictly deserialize the task's proposal type, rejecting unknown fields and enforcing size limits.
2. Allocate host-owned run/request IDs, observe the exact subject, and build the frozen canonical requests.
3. Build `SubmittedProposal` from the full parsed input, its plan and its base subject digest.
4. Construct `SubmittedProposalAdapter` with those values, contract support and a `RunControl`.
5. Pass the adapter and task-specific ports to `AgentRunEngine` with trusted `RuntimeSupport` and `RuntimeConfig`.

`SubmittedProposalAdapter::new` validates the canonical requests and binds the input hash, plan, exact subject, capability, policy list and first effect sequence. The full input uses RFC 8785 canonical JSON before hashing; transport whitespace and object ordering aren't separate proposals. String content stays part of the hash. The adapter checks the request again during planning and permits one engine invocation and one effect.

The Rust constructor is a trusted host API. The host chooses how task fields map to the plan and subject; it must pass the same complete parsed input used by its executor. Exposing request, approval, policy, verifier or terminal-state fields in untrusted input would violate that integration contract. The shipping server's closed `Change` type accepts only `path`, `content`, `plan` and `base_digest`.

## Cancellation and usage

`RunControl` accepts host-timed deadlines and a caller cancellation signal. It creates interruption evidence without accepting caller-supplied authority or evidence. Completion and cancellation compete under one lock. A successful completion claim prevents a later cancellation from changing that run's outcome.

Executors must check cancellation before committing an effect and retain the observed outcome afterward. Cancellation remains cooperative: a racing or already-committed write can remain executed while the run ends interrupted. Verifier failure likewise preserves the effect. The reference's persisted ledger determines duplicate-delivery behavior; this one-shot adapter doesn't provide a durable admission service.

The adapter performs no inference or tool execution. Its zero port counters describe that local work. Consumers must represent unobserved external model tokens and cost separately as unavailable. A proposal's budget covers only its admitted host work; the external agent's conversation belongs to another scope.

## Verification

```bash
cargo test --manifest-path adapters/submitted-proposal/Cargo.toml --locked
cargo clippy --manifest-path adapters/submitted-proposal/Cargo.toml --locked --all-targets -- -D warnings
```

Public integration tests run two text-task configurations through the same adapter and engine, with actual disposable file writes and host-owned expected output. They cover binding tampering, strict input parsing, changed descriptors, untrusted verifier identity, cancellation before and after execution, expired deadlines, and uncertain outcomes with refused adapter reuse. The separate shipping reference covers MCP delivery, saved history and its restricted Python verifier.

The frozen Agent Run and Protected Effect contracts, `AgentRunEngine`, and `RunCoordinator` stay authoritative. ThreadLoop owns lifecycle advancement. Authenticated receipt exchange, runtime isolation and durable reconciliation retain their existing project scopes.

`RunControl::new` treats a timeout beyond the monotonic clock's representable range as an immediate interruption.
