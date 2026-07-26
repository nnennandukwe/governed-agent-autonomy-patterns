'use strict';

function refundPayment(payment) {
  if (payment.status === 'refunded') {
    return payment;
  }

  return {
    ...payment,
    status: 'refunded',
    refundedAt: '2026-07-23T00:00:00.000Z',
  };
}

module.exports = { refundPayment };
