'use strict';

const assert = require('node:assert/strict');
const path = require('node:path');
const test = require('node:test');

const workspace = process.env.GOVERN_WORKSPACE
  || path.resolve(__dirname, '..', 'workspace');
const { handleWebhook } = require(path.join(workspace, 'src', 'webhook'));

test('a failed delivery is not recorded and can be retried', async () => {
  const processed = new Set();
  const event = { deliveryId: 'delivery-1' };
  await assert.rejects(
    () => handleWebhook(processed, event, async () => {
      throw new Error('temporary failure');
    }),
    /temporary failure/,
  );
  let calls = 0;
  const result = await handleWebhook(processed, event, async () => {
    calls += 1;
  });

  assert.equal(calls, 1);
  assert.deepEqual(result, { duplicate: false });
  assert.equal(processed.has('delivery-1'), true);
});
