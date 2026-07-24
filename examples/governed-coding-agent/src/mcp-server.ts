import { pathToFileURL } from 'node:url';

import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import { z } from 'zod';

import { createRepositoryTools } from './repository-tools.js';

export async function startRepositoryMcpServer(): Promise<void> {
  const workspace = process.env.GOVERN_WORKSPACE;
  if (!workspace) {
    throw new Error(
      'GOVERN_WORKSPACE is required. Recovery: start the server through the governed sandbox adapter.',
    );
  }
  const tools = createRepositoryTools(workspace);
  const server = new McpServer({
    name: 'governed-repository-tools',
    version: '0.1.0',
  });
  const result = async (
    name: string,
    args: Record<string, unknown>,
  ) => {
    const response = await tools.call(name, args);
    return {
      content: [{ type: 'text' as const, text: response.content }],
      ...(response.isError ? { isError: true } : {}),
    };
  };
  const closed = {
    readOnlyHint: true,
    destructiveHint: false,
    idempotentHint: true,
    openWorldHint: false,
  };

  server.registerTool('repo.list', {
    description: 'List files under a workspace-relative directory.',
    inputSchema: z.object({
      path: z.string().default('.'),
    }).strict(),
    annotations: closed,
  }, args => result('repo.list', args));
  server.registerTool('repo.read', {
    description: 'Read one UTF-8 workspace file.',
    inputSchema: z.object({
      path: z.string(),
    }).strict(),
    annotations: closed,
  }, args => result('repo.read', args));
  server.registerTool('repo.apply_patch', {
    description: 'Validate and apply a unified diff inside the workspace.',
    inputSchema: z.object({
      patch: z.string(),
    }).strict(),
    annotations: {
      readOnlyHint: false,
      destructiveHint: true,
      idempotentHint: false,
      openWorldHint: false,
    },
  }, args => result('repo.apply_patch', args));
  server.registerTool('repo.run', {
    description: 'Run an allowlisted test or repository inspection command.',
    inputSchema: z.object({
      executable: z.enum(['node', 'npm', 'git']),
      args: z.array(z.string()),
      cwd: z.string().default('.'),
    }).strict(),
    annotations: {
      readOnlyHint: false,
      destructiveHint: false,
      idempotentHint: false,
      openWorldHint: false,
    },
  }, args => result('repo.run', args));
  server.registerTool('repo.diff', {
    description: 'Return the current workspace diff against its base commit.',
    inputSchema: z.object({}).strict(),
    annotations: closed,
  }, args => result('repo.diff', args));

  await server.connect(new StdioServerTransport());
}

if (
  process.argv[1]
  && import.meta.url === pathToFileURL(process.argv[1]).href
) {
  await startRepositoryMcpServer();
}
