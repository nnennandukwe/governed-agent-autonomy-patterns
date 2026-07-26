'use strict';

const assert = require('node:assert/strict');
const path = require('node:path');
const test = require('node:test');

const workspace = process.env.GOVERN_WORKSPACE
  || path.resolve(__dirname, '..', 'workspace');
const { refundPayment } = require(path.join(workspace, 'src', 'refund'));

test('captured transitions once and refunded remains identity-idempotent', () => {
  const captured = { id: 'payment-1', status: 'captured' };
  const refunded = refundPayment(captured);

  assert.equal(refunded.status, 'refunded');
  assert.equal(refundPayment(refunded), refunded);
  assert.equal(captured.status, 'captured');
});

test('failed and cancelled payments also fail closed', () => {
  for (const status of ['failed', 'cancelled']) {
    assert.throws(
      () => refundPayment({ id: 'payment-1', status }),
      /captured payments can be refunded/,
    );
  }
});
