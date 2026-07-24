'use strict';

const assert = require('node:assert/strict');
const path = require('node:path');
const test = require('node:test');

const workspace = process.env.GOVERN_WORKSPACE
  || path.resolve(__dirname, '..', 'workspace');
const { capturePayment } = require(path.join(workspace, 'src', 'capture'));

test('a capture retry returns the exact stored receipt without mutation', () => {
  const store = new Map();
  const events = [];
  const first = capturePayment(
    store,
    'payment-1',
    event => events.push(event),
  );
  const second = capturePayment(
    store,
    'payment-1',
    event => events.push(event),
  );

  assert.equal(second, first);
  assert.equal(second.attempts, 1);
  assert.deepEqual(events, [{
    type: 'payment.captured',
    paymentId: 'payment-1',
  }]);
});
