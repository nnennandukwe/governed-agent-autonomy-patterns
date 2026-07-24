'use strict';

const assert = require('node:assert/strict');
const {
  mkdir,
  mkdtemp,
  rm,
  symlink,
  writeFile,
} = require('node:fs/promises');
const { tmpdir } = require('node:os');
const path = require('node:path');
const test = require('node:test');

const workspace = process.env.GOVERN_WORKSPACE
  || path.resolve(__dirname, '..', 'workspace');
const { loadConfig } = require(path.join(workspace, 'src', 'config'));

test('valid nested config loads and symlink escape is rejected', async t => {
  const parent = await mkdtemp(path.join(tmpdir(), 'config-realpath-'));
  const root = path.join(parent, 'root');
  const nested = path.join(root, 'nested');
  await mkdir(nested, { recursive: true });
  await writeFile(path.join(nested, 'app.json'), '{"enabled":true}\n');
  await writeFile(path.join(parent, 'secret.json'), '{"secret":true}\n');
  await symlink(path.join(parent, 'secret.json'), path.join(root, 'link.json'));
  t.after(() => rm(parent, { recursive: true, force: true }));

  assert.deepEqual(
    await loadConfig(root, 'nested/app.json'),
    { enabled: true },
  );
  await assert.rejects(
    () => loadConfig(root, 'link.json'),
    /outside the configured root/,
  );
});
