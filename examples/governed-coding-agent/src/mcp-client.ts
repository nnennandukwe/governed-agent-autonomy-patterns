import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StdioClientTransport } from '@modelcontextprotocol/sdk/client/stdio.js';

import type {
  ModelToolCall,
  ToolCallResult,
  ToolClient,
  ToolDefinition,
} from './types.js';

export interface StdioToolClientOptions {
  command: string;
  args: string[];
  cwd?: string;
  env: Record<string, string>;
}

export class StdioRepositoryToolClient implements ToolClient {
  private client: Client | undefined;
  private transport: StdioClientTransport | undefined;

  constructor(private readonly options: StdioToolClientOptions) {}

  async connect(): Promise<void> {
    if (this.client) return;
    this.transport = new StdioClientTransport({
      command: this.options.command,
      args: this.options.args,
      ...(this.options.cwd ? { cwd: this.options.cwd } : {}),
      env: this.options.env,
      stderr: 'pipe',
    });
    this.client = new Client({
      name: 'governed-coding-agent',
      version: '0.1.0',
    });
    await this.client.connect(this.transport);
  }

  async listTools(): Promise<ToolDefinition[]> {
    if (!this.client) {
      throw new Error('MCP client is not connected.');
    }
    const result = await this.client.listTools();
    return result.tools.map(tool => ({
      name: tool.name,
      description: tool.description ?? '',
      inputSchema: tool.inputSchema as Record<string, unknown>,
      ...(tool.outputSchema
        ? { outputSchema: tool.outputSchema as Record<string, unknown> }
        : {}),
      ...(tool.annotations
        ? { annotations: tool.annotations as Record<string, unknown> }
        : {}),
    }));
  }

  async callTool(call: ModelToolCall): Promise<ToolCallResult> {
    if (!this.client) {
      throw new Error('MCP client is not connected.');
    }
    const result = await this.client.callTool({
      name: call.name,
      arguments: call.arguments,
    });
    const text = Array.isArray(result.content)
      ? result.content
          .filter(
            (item): item is { type: 'text'; text: string } => (
              typeof item === 'object'
              && item !== null
              && item.type === 'text'
              && 'text' in item
              && typeof item.text === 'string'
            ),
          )
          .map(item => item.text)
          .join('\n')
      : JSON.stringify(result.content);
    return {
      content: text,
      ...(result.isError ? { isError: true } : {}),
    };
  }

  async close(): Promise<void> {
    await this.client?.close();
    this.client = undefined;
    this.transport = undefined;
  }
}
