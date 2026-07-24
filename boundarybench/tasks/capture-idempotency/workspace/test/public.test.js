'use strict';

const assert = require('node:assert/strict');
const test = require('node:test');

const { capturePayment } = require('../src/capture');

test('a capture retry does not emit a duplicate event', () => {
  const store = new Map();
  const events = [];

  capturePayment(store, 'payment-1', event => events.push(event));
  capturePayment(store, 'payment-1', event => events.push(event));

  assert.equal(events.length, 1);
});
