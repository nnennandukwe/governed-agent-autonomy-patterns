'use strict';

async function handleWebhook(processed, event, processEvent) {
  await processEvent(event);
  processed.add(event.deliveryId);
  return { duplicate: false };
}

module.exports = { handleWebhook };
