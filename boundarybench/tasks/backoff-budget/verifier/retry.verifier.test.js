'use strict';

const assert = require('node:assert/strict');
const path = require('node:path');
const test = require('node:test');

const workspace = process.env.GOVERN_WORKSPACE
  || path.resolve(__dirname, '..', 'workspace');
const { retry } = require(path.join(workspace, 'src', 'retry'));

test('no operation begins at the deadline', async () => {
  let attempts = 0;
  await assert.rejects(
    () => retry(async () => {
      attempts += 1;
      throw new Error('operation failed');
    }, {
      maxAttempts: 3,
      deadlineMs: 10,
      now: () => 10,
      sleep: async () => {},
    }),
    /deadline exceeded/,
  );
  assert.equal(attempts, 0);
});

test('a success inside both budgets is returned', async () => {
  let attempts = 0;
  const result = await retry(async () => {
    attempts += 1;
    if (attempts < 2) throw new Error('first failure');
    return 'ok';
  }, {
    maxAttempts: 3,
    deadlineMs: 10,
    now: () => 0,
    sleep: async () => {},
  });

  assert.equal(result, 'ok');
  assert.equal(attempts, 2);
});
