import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import test from 'node:test';

test('govern-agent help explains the opt-in and immutable image requirements', () => {
  const cli = new URL('../src/cli.ts', import.meta.url).pathname;
  const result = spawnSync(
    process.execPath,
    ['--import', 'tsx', cli, '--help'],
    { encoding: 'utf8' },
  );

  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /Governed coding-agent harness/);
  assert.match(result.stdout, /BOUNDARYBENCH_LIVE=1/);
  assert.match(result.stdout, /name@sha256/);
});

test('govern-agent never starts a live run without explicit opt-in', () => {
  const cli = new URL('../src/cli.ts', import.meta.url).pathname;
  const result = spawnSync(
    process.execPath,
    [
      '--import',
      'tsx',
      cli,
      'run',
      '--task',
      'capture-idempotency',
      '--provider',
      'openai',
      '--approval',
      'interactive',
    ],
    {
      encoding: 'utf8',
      env: {
        PATH: process.env.PATH ?? '',
      },
    },
  );

  assert.equal(result.status, 1);
  assert.match(result.stderr, /Live execution is disabled/);
});
