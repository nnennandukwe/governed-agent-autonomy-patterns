'use strict';

const assert = require('node:assert/strict');
const test = require('node:test');

const { retry } = require('../src/retry');

test('maxAttempts is the total operation-attempt budget', async () => {
  let attempts = 0;
  await assert.rejects(
    () => retry(async () => {
      attempts += 1;
      throw new Error('still failing');
    }, {
      maxAttempts: 3,
      deadlineMs: 100,
      now: () => 0,
      sleep: async () => {},
    }),
    /still failing/,
  );

  assert.equal(attempts, 3);
});
