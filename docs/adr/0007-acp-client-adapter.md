# ACP stable-v1 client with delegated protected effects

Authority: [GAAP #12](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/12).
Baseline: `241dc1dcb09bc13d59891db356c904581df14be7`.

## Problem and decision

An external coding agent owns its conversation, but that doesn't make its permission UI or tool-success messages
GAAP's authority. GAAP needs a typed session client that suspends executable requests while its existing engine decides
whether to admit an exact protected effect and independently verifies the resulting subject.

Use a separate `gaap-acp` crate with official Rust SDK 3.3.0 and stable ACP v1. One SDK worker owns one child and session
for one accepted run. Client filesystem callbacks hand full operations to the synchronous engine; the matching executor
releases the callback only after host-owned execution. The adapter doesn't approve agent-internal permission requests.
Terminals and other unsupported capabilities aren't advertised. A strict bounded line transport wraps the SDK instead
of relying on tolerant deserialization as a security boundary.

Agent Run and Protected Effect schemas stay unchanged. `AgentAdapter` gains a default no-op `close` method, called after
every engine return, so blocked gates and error returns release an external adapter's resources without waiting for the
caller to drop the engine. Existing deterministic adapters retain their behavior. RunCoordinator's decisions stay intact.

## Reuse and authority

[ADR 0006](0006-mcp-client-adapter.md) maps the preserved conference branch. Its submitted proposal, real filesystem
executor, independent verifier, stale-subject cases and uncertain-effect cases inform the ACP tests. The inbound MCP
server isn't copied into the client, and the conference checkout and saved revisions remain untouched.

The host approves launch artifacts and the session capability, supplies permission policy, performs file effects and
meters actual resource usage. An independent verifier supplies completion evidence. ACP supplies typed interoperability,
progress and session identity. Neither protocol status nor the peer's usage update is trusted execution or metering.

## Invariants and evidence

| State | Allowed action | Rejected action |
| --- | --- | --- |
| New | Stable-v1 initialization | Effect requests, foreign notifications or a second run |
| Initialized | Create one session | Prompt before setup or mismatched identity/version |
| Ready | Return a host-bound plan; prompt after engine admission | Ambient permission or terminal authority |
| Prompt active | Propose one full filesystem operation | Concurrent/duplicate requests, foreign sessions, invalid tool order |
| Effect pending | Exact matching engine executor dispatch | Changed input, replay, expired authority or an unapproved capability |
| Terminal | Close child and retain evidence | New prompts, replay or resumption |

Frame schema/shape checks, duplicate rejection, exact numeric limits, message/stderr bounds and bounded waits apply to
both positive and adversarial process fixtures. Receipts retain host execution observations even when a later disconnect
or verifier failure prevents completion. Payload-free traces retain protocol/session negotiation identity and allow
host-owned debugging without copying prompts, file text or stderr into receipts.

## Limits and next owners

This is a trusted local delegated-filesystem profile, not OS enforcement. Filesystem checks are observations; the host
must serialize changes and the actual executor must enforce scope at dispatch. A callback already performing an external
effect can't safely be stopped by the caller's timeout. The port refuses further work and requires reconciliation.

The fixture is an independent local Python process without billing. Its green tests aren't live Goose proof. Goose #14
is next after both adapters merge; signed receipts #15 follow it, then ThreadLoop #111 independently verifies, persists
and admits that evidence through its own lifecycle guards. Isolation #16/#17 and broader runtime conformance #21 remain
separate. A conductor-chain proxy is optional future composition, not a replacement for this boundary.

Primary references: [official Rust SDK](https://github.com/agentclientprotocol/rust-sdk),
[stable-v1 protocol](https://agentclientprotocol.com/protocol/v1/overview).
