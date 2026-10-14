# Official MCP client adapter and conference reuse

Authority: [GAAP #13](https://github.com/nnennandukwe/governed-agent-autonomy-patterns/issues/13).
Baseline: `712875cbf4be6dd02093f50f93ae826f20cb4890`.

## Engineering problem

GAAP's engine already separates proposed work, protected-effect execution and independent verification. The conference
example demonstrates that flow for a submitted coding proposal, with an MCP server called by Goose. Issue #13 requires
GAAP to discover and invoke external MCP tools while retaining its existing authority and evidence rules.

## Reuse mapping

The conference source inspected for this build is local branch `feat/goose-mcp-conference-demo` at
`95a0b83d89a35729ddb14c0053653ea11b7c1229`, including the preserved uncommitted persistence/history changes.

| Existing artifact or pattern | Carry-forward use | Remaining owner |
| --- | --- | --- |
| `src/runtime.rs`: AgentAdapter, ExecutorPort, VerifierPort | Compose the client through the existing executor seam; retain RunCoordinator decisions and independent verification. | #13 adapter; #12 agent-session integration. |
| `examples/goose-mcp/src/coding/mcp.rs`: bound descriptors | Bind external server identity, full per-tool descriptors and input/output schemas to exact host capability approval. | #13; the inbound proposal server remains #31. |
| `examples/goose-mcp/src/coding/mod.rs`: executed effect with failed verification | Preserve a real changed subject while blocking completion; validate both effect and terminal receipts. | #13 regression; live Goose proof remains #14. |
| Demo stale subject, denied effect and caller-authority injection cases | Reject before dispatch using immutable prepared bindings and trusted host inputs. | #13 tests. |
| Demo persistent intent, duplicate proposal and uncertain-effect history | Refuse silent replay of attempted invocations; retain uncertainty without claiming durable resumption. | #32 design and later implementation. |
| Inspector and `terminal_receipt_verified` display | Keep hash/semantic checks separate from authenticated evidence and lifecycle authority. | #15 signing, ThreadLoop #111 admission, #20/#33 projections. |

The shipping task, restricted Python verifier, local filesystem/inode identity, Goose recipe, authentication, saved
sessions and inspector aren't transplanted into the outbound client. The conference checkout and uncommitted artifacts
stay preserved.

## Design decision

Use a separate `gaap-mcp` crate under `adapters/mcp`. Its exact SDK and asynchronous transport dependencies don't enter
GAAP's provider-neutral contracts. A dedicated runtime thread owns the SDK session behind the synchronous executor
interface. The host supplies launch configuration, capability grants, permission policy, subject observations and usage.
RunCoordinator remains the authorization evaluator; server annotations and success messages don't confer authority.

The initial stdio profile pins the official rmcp 3.5.1 SDK and MCP 2025-11-25. That specification uses the initialization
handshake and tool-list notifications required by this first adapter. Newer stateless lifecycle support requires an
explicit compatibility update, rather than automatic negotiation into a different execution model. Only declared text
and schema-validated structured output are supported initially.

Each prepared invocation retains the exact accepted run, subject, policy, profile, capability and argument identity.
Descriptor drift requires fresh authority. After dispatch, cancellation, transport failure and malformed output retain
unknown outcomes. Independent host observation supplies the actual resulting subject; verification remains a separate
engine step. A failed verifier can't erase an already executed mutation.

## Evidence boundaries

The test server is a deterministic local subprocess with no model or external billing. Engine integration tests validate
returned Protected Effect Results and Terminal Run Receipts through GAAP's independent validators. These checks establish
this adapter's controlled behavior; they don't establish broad MCP-server safety, live Goose interoperability, signing,
persistence/recovery, isolation, ThreadLoop lifecycle advancement, or production readiness.

The profile reports unisolated local stdio honestly. Direct child cleanup doesn't prove that descendants or remote
effects stopped. Discovery verifies host-approved launch artifacts before spawning; the host must include the relevant dependencies
and serialize subject changes when stronger freshness guarantees are needed.
Late duplicate responses poison future operations while preserving earlier effects and evidence.

## Verification and next owners

Follow [the adapter guide](../../adapters/mcp/README.md) for the public host interface, exact dependency/security commands,
limits and recovery table. Root contract-generation and frozen RunInvariant checks retain their existing semantics.
The separate adapter CI job is required alongside the existing selected root lanes.

ACP #12 owns external agent-session control. #14 will prove the governed Goose run over ACP/MCP. #15 will emit signed
receipts before ThreadLoop #111 independently verifies, persists and admits them. #32 owns durable uncertain-effect
reconciliation. This build doesn't change that accepted order.
