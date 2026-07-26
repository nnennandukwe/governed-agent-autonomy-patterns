import assert from 'node:assert/strict';
import {
  mkdtemp,
  rm,
  symlink,
  writeFile,
} from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';

import {
  digestDirectory,
  DockerSandboxAdapter,
} from '../src/docker.js';

const mcpBundlePath = new URL(
  '../dist/mcp-server.bundle.mjs',
  import.meta.url,
).pathname;

test('workspace digests bind paths and content but ignore git metadata', async t => {
  const root = await mkdtemp(path.join(tmpdir(), 'govern-digest-test-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await writeFile(path.join(root, 'value.txt'), 'one\n');
  const first = await digestDirectory(root);
  await writeFile(path.join(root, 'value.txt'), 'two\n');
  const second = await digestDirectory(root);
  await symlink('value.txt', path.join(root, 'value-link'));
  const withSymlink = await digestDirectory(root);

  assert.match(first, /^sha256:[0-9a-f]{64}$/);
  assert.notEqual(first, second);
  assert.notEqual(second, withSymlink);
});

test('Docker doctor returns actionable readiness instead of throwing', async () => {
  const adapter = new DockerSandboxAdapter({
    mcpBundlePath,
  });

  const result = await adapter.doctor();

  assert.equal(typeof result.ok, 'boolean');
  assert.ok(result.detail.length > 0);
  if (!result.ok) {
    assert.match(result.detail, /Recovery:/);
  }
});

test('Docker doctor rejects an image whose required runtime cannot execute', async () => {
  const image = `node@sha256:${'a'.repeat(64)}`;
  const commands: string[][] = [];
  const adapter = new DockerSandboxAdapter(
    {
      mcpBundlePath,
      expectedImage: image,
    },
    async args => {
      commands.push(args);
      if (args[0] === 'version') {
        return { status: 0, stdout: '28.1.1\n', stderr: '' };
      }
      if (args[0] === 'image') {
        return { status: 0, stdout: '{}\n', stderr: '' };
      }
      if (args[0] === 'run') {
        return {
          status: 126,
          stdout: '',
          stderr: 'exec /bin/sh: exec format error',
        };
      }
      throw new Error(`Unexpected Docker command: ${args.join(' ')}`);
    },
  );

  const result = await adapter.doctor();

  assert.equal(result.ok, false);
  assert.match(result.detail, /required Node and Git runtimes/);
  assert.match(result.detail, /Recovery:/);
  assert.equal(
    commands.some(args => args[0] === 'run'),
    true,
    'doctor must execute the image instead of trusting image inspect alone',
  );
});
