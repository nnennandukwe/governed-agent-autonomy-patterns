# Own the coding-agent loop

The example uses a repository-owned provider-neutral harness rather than Codex, Claude Code, OpenAI Agents SDK, or Anthropic's tool runner. This keeps lifecycle advancement, approvals, MCP execution, budgets, verification, and evidence observable at one interface, while provider adapters translate only model-specific messages and tool calls.
