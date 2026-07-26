import assert from 'node:assert/strict';
import { mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

import { StdioRepositoryToolClient } from '../src/mcp-client.js';

const packageRoot = path.resolve(
  path.dirname(fileURLToPath(import.meta.url)),
  '..',
);

test('a real stdio MCP client discovers and calls the jailed repository server', async t => {
  const workspace = await mkdtemp(path.join(tmpdir(), 'govern-mcp-test-'));
  await writeFile(path.join(workspace, 'hello.txt'), 'hello from MCP\n');
  const client = new StdioRepositoryToolClient({
    command: process.execPath,
    args: [
      '--import',
      'tsx',
      path.join(packageRoot, 'src', 'mcp-server.ts'),
    ],
    cwd: packageRoot,
    env: {
      GOVERN_WORKSPACE: workspace,
      PATH: process.env.PATH ?? '/usr/local/bin:/usr/bin:/bin',
    },
  });
  t.after(async () => {
    await client.close();
    await rm(workspace, { recursive: true, force: true });
  });

  await client.connect();
  const tools = await client.listTools();
  assert.deepEqual(tools.map(tool => tool.name), [
    'repo.list',
    'repo.read',
    'repo.apply_patch',
    'repo.run',
    'repo.diff',
  ]);
  const result = await client.callTool({
    id: 'read-1',
    name: 'repo.read',
    arguments: { path: 'hello.txt' },
  });

  assert.equal(result.isError, undefined);
  assert.equal(result.content, 'hello from MCP\n');
});
