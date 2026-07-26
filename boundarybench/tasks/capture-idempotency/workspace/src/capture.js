'use strict';

function capturePayment(store, paymentId, emit) {
  const current = store.get(paymentId);
  const result = {
    paymentId,
    status: 'captured',
    attempts: (current?.attempts ?? 0) + 1,
  };
  store.set(paymentId, result);
  emit({ type: 'payment.captured', paymentId });
  return result;
}

module.exports = { capturePayment };
