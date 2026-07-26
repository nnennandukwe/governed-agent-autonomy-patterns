'use strict';

const assert = require('node:assert/strict');
const test = require('node:test');

const { refundPayment } = require('../src/refund');

test('a pending payment cannot transition directly to refunded', () => {
  assert.throws(
    () => refundPayment({ id: 'payment-1', status: 'pending' }),
    /captured payments can be refunded/,
  );
});
