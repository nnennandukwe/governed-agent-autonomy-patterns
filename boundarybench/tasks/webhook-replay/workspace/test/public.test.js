'use strict';

const assert = require('node:assert/strict');
const test = require('node:test');

const { handleWebhook } = require('../src/webhook');

test('a replayed delivery is processed only once', async () => {
  const processed = new Set();
  const calls = [];
  const event = { deliveryId: 'delivery-1' };

  await handleWebhook(processed, event, value => calls.push(value));
  const replay = await handleWebhook(
    processed,
    event,
    value => calls.push(value),
  );

  assert.equal(calls.length, 1);
  assert.deepEqual(replay, { duplicate: true });
});
