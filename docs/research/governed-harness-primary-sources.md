# Governed Harness Primary Sources

Checked against first-party documentation and source on July 23, 2026.

This note records the external contracts and evaluation precedents used by the
governed coding-agent example. It is provenance for public implementation
choices, not an experimental result or a private build plan.

## Provider-Owned Inference, Harness-Owned Control

### OpenAI Responses API

**Source facts**

- The Responses API returns client function requests as `function_call` output
  items. The application executes each function and sends its result back as a
  `function_call_output` with the original `call_id`. The official guide
  preserves the response output in the next request, so the application can own
  the loop rather than delegate tool execution to an agent framework.
  [OpenAI function-calling guide](https://developers.openai.com/api/docs/guides/function-calling)
- With `store: false`, client-owned conversation state requires replaying the
  relevant output items in order. OpenAI also cautions that `store: false` does
  not, by itself, enable Zero Data Retention.
  [OpenAI conversation state](https://developers.openai.com/api/docs/guides/conversation-state),
  [OpenAI programmatic tool-calling guide](https://developers.openai.com/api/docs/guides/tools-programmatic-tool-calling)
- Responses include a `usage` object with input, output, total, cached, and
  reasoning-token fields. OpenAI identifies `x-request-id` as the
  server-generated troubleshooting identifier and recommends logging it.
  [OpenAI reasoning usage](https://developers.openai.com/api/docs/guides/reasoning),
  [OpenAI API request IDs](https://developers.openai.com/api/reference/overview#debugging-requests)
- The official Node SDK automatically retries selected transient failures twice
  by default and supports `maxRetries: 0`. It exposes the response request ID as
  `_request_id`.
  [OpenAI Node SDK](https://github.com/openai/openai-node#retries)
- `gpt-5.6-terra` is a documented Responses API model with function calling.
  OpenAI describes it as balancing intelligence and cost. GPT-5.6 supports
  `reasoning.effort: "medium"` as a balanced starting point. As of this check,
  the Terra model page does not publish a distinct dated snapshot ID.
  [GPT-5.6 Terra model page](https://developers.openai.com/api/docs/models/gpt-5.6-terra),
  [GPT-5.6 guidance](https://developers.openai.com/api/docs/guides/latest-model)

**Design inference**

The OpenAI adapter should translate Responses API items but must not decide
whether a tool runs. The harness should inspect every `function_call`, apply its
own gates, execute only an allowed MCP call, and return the resulting
`function_call_output`. It should use `store: false`, preserve the required
stateless history, set `maxRetries: 0`, and record usage and request IDs. Those
settings make retry count, tool authority, and cost evidence properties of the
harness rather than hidden SDK behavior. A frozen trial should record both the
requested and provider-returned model IDs; it should not describe the current
Terra identifier as a dated snapshot.

### Anthropic Messages API

**Source facts**

- For client tools, Claude returns a `tool_use` block. The application executes
  the operation and sends a `tool_result` block in the next user message. The
  documented loop repeats while `stop_reason` is `tool_use`; Claude does not
  execute a client tool itself.
  [Anthropic tool-use contract](https://platform.claude.com/docs/en/agents-and-tools/tool-use/how-tool-use-works)
- A Message includes billing and rate-limit `usage`. Anthropic defines total
  input as `input_tokens + cache_creation_input_tokens +
  cache_read_input_tokens`; visible content and usage are not expected to map
  one-to-one.
  [Anthropic Messages API reference](https://platform.claude.com/docs/en/api/messages)
- The TypeScript SDK exposes `request-id` as `_request_id`. Its transient-error
  default is two automatic retries after the original request, and
  `maxRetries: 0` disables automatic retries.
  [Anthropic TypeScript SDK](https://platform.claude.com/docs/en/cli-sdks-libraries/sdks/typescript),
  [Anthropic API errors](https://platform.claude.com/docs/en/api/errors)
- `claude-sonnet-5` is the documented API model ID. Claude Sonnet 5 supports the
  effort control, and `output_config: { effort: "medium" }` selects the
  documented balanced level. Anthropic documents adaptive thinking as enabled
  by default for Sonnet 5 and counts thinking and visible output against
  `max_tokens`.
  [Claude Sonnet 5](https://platform.claude.com/docs/en/about-claude/models/whats-new-sonnet-5),
  [Anthropic effort control](https://platform.claude.com/docs/en/build-with-claude/effort)

**Design inference**

The Anthropic adapter should implement the Messages loop directly rather than
use the SDK tool runner. The harness should authorize each `tool_use` before
producing a `tool_result`, set `maxRetries: 0`, and preserve raw usage categories
and `_request_id`. Provider usage is evidence for an attributed cost estimate;
it is not an invoice-reconciled cost. A low `max_tokens` ceiling must be
preflighted because it can truncate thinking, tool arguments, or the visible
answer. The two providers' settings named `medium` are provider-native
configurations, not evidence of equivalent compute.

## Local MCP Tool Boundary

**Source facts**

- The currently published `@modelcontextprotocol/sdk` production line is v1,
  with `1.29.0` published under the `latest` tag. The SDK repository states that
  v2 is still prerelease and recommends v1.x for production use until v2 is
  stable.
  [npm package](https://www.npmjs.com/package/@modelcontextprotocol/sdk?activeTab=versions),
  [MCP TypeScript SDK status](https://github.com/modelcontextprotocol/typescript-sdk)
- In v1.29.0, `StdioClientTransport` launches a local child process and
  `Client.connect()` initializes the connection. The client exposes
  `listTools()` for discovery and `callTool()` for invocation.
  [MCP v1.29 client guide](https://github.com/modelcontextprotocol/typescript-sdk/blob/v1.29.0/docs/client.md)
- A local server uses `McpServer` with `StdioServerTransport`; registered tools
  expose schemas and handlers through the protocol.
  [MCP v1.29 server guide](https://github.com/modelcontextprotocol/typescript-sdk/blob/v1.29.0/docs/server.md),
  [MCP v1.29 protocol guide](https://github.com/modelcontextprotocol/typescript-sdk/blob/v1.29.0/docs/protocol.md)

**Design inference**

Use the stable v1 import paths. This repository pins
`@modelcontextprotocol/sdk@1.29.0` and overrides its transitive
`@hono/node-server` dependency to the patched `2.0.11` release because the July
23 dependency audit reported a path-traversal advisory in the SDK's declared
1.x range. The harness uses only the stable stdio client/server surface.
Discover the full tool list before execution, canonicalize the server identity
and tool descriptors into a capability digest, and route every `callTool()`
through the tool-trust and permission gates. Re-discovery and same-name schema
changes are governance policy supplied by this project, not guarantees made by
the MCP SDK.

## Container Isolation and Evidence

**Source facts**

- SWE-bench uses Docker for reproducible evaluation. Its runner creates an
  instance container, writes the submitted patch, test output, diff, and report
  to per-instance paths, and invokes container cleanup in a `finally` block.
  [SWE-bench repository](https://github.com/SWE-bench/SWE-bench),
  [SWE-bench evaluation runner](https://github.com/SWE-bench/SWE-bench/blob/main/swebench/harness/run_evaluation.py#L71-L273)
- Inspect's Docker sandbox runs each sample in its own container. Its sandbox
  contract separates sample initialization and cleanup, task cleanup, and
  manual cleanup. Inspect also distinguishes expected tool errors from
  unexpected infrastructure errors.
  [Inspect sandbox environments](https://inspect.aisi.org.uk/extensions-sandboxes.html),
  [Inspect per-sample sandboxing](https://inspect.aisi.org.uk/sandboxing.html#per-sample-setup)
- Inspect writes one evaluation log per evaluated task and records the tested
  model, generation configuration, source revision, package versions, sample
  events, status, and scores. Failed and interrupted runs retain logs, and an
  Inspect retry writes a new log rather than overwriting the original.
  [Inspect evaluation logs](https://inspect.aisi.org.uk/eval-logs.html),
  [Inspect log schema](https://inspect.aisi.org.uk/reference/inspect_ai.log.html)

**Design inference**

Use a fresh task container for every trial and a separate clean verifier
container. Pin image digests, keep model-provider credentials outside those
containers, disable task networking, and record cleanup failure as an
infrastructure outcome. A patch, test log, receipt, and content digest should
remain distinct artifacts so a successful task cannot be mistaken for a valid
governance result.

## Evaluation Reporting and Repetition

**Source facts**

- OpenAI's third-party evaluation guidance says a useful report should identify
  the claim, task distribution, tested model and reasoning setting, tool access,
  harness, safeguards, turn/token/retry/time/cost budgets, elicitation method,
  and validity checks. It explicitly treats harness choices and validity checks
  as part of the evaluation result.
  [OpenAI third-party evaluation playbook](https://openai.com/index/trustworthy-third-party-evaluations-foundations/)
- SWE-bench defines pass@1 as one submitted prediction per task instance. It
  distinguishes this from pass@k, where multiple attempts are all evaluated
  and any success counts, and best@k, where a separate selection mechanism
  chooses among attempts without using benchmark test knowledge.
  [SWE-bench submission checklist](https://github.com/swe-bench/experiments/blob/main/checklist.md#checklist)
- METR separates scaffold elicitation on a small development set from
  measurement on a larger test set. Its current time-horizon process launches
  six independent runs per task and then performs validity review, including
  reward-hack and token-budget checks.
  [METR time-horizon methodology](https://metr.org/time-horizons/#what-does-running-a-time-horizon-evaluation-involve)
- METR publishes run-level inputs and success outcomes and applies bootstrap
  analysis for its time-horizon estimates.
  [METR public analysis repository](https://github.com/METR/eval-analysis-public)

**Design inference**

The first BoundaryBench live-model matrix is a single-attempt exploratory
pilot, not a confirmatory estimate of model reliability. Report every attempted
trial, including provider, timeout, refusal, infrastructure, and budget
failures. Do not silently rerun a failed cell or report a best-of result.
Separate development runs from frozen pilot runs and label raw counts as
descriptive. A later confirmatory study should use a new held-out corpus,
multiple independent runs per task-condition cell, and a predeclared analysis.

## Claim Boundary

These sources support the feasibility of a harness-owned provider loop, local
stdio MCP tools, per-trial container isolation, evidence-rich evaluation, and
transparent reporting. They do not establish that this repository's five gates
work. Only the frozen BoundaryBench conformance suite and subsequently reviewed
experimental evidence can support that project-specific claim.
