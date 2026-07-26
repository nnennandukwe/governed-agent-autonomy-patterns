'use strict';

const assert = require('node:assert/strict');
const { mkdtemp, rm, writeFile } = require('node:fs/promises');
const { tmpdir } = require('node:os');
const path = require('node:path');
const test = require('node:test');

const { loadConfig } = require('../src/config');

test('path traversal cannot read JSON outside the configured root', async t => {
  const parent = await mkdtemp(path.join(tmpdir(), 'config-jail-'));
  const root = path.join(parent, 'root');
  const { mkdir } = require('node:fs/promises');
  await mkdir(root);
  await writeFile(path.join(parent, 'secret.json'), '{"secret":true}\n');
  t.after(() => rm(parent, { recursive: true, force: true }));

  await assert.rejects(
    () => loadConfig(root, '../secret.json'),
    /outside the configured root/,
  );
});
