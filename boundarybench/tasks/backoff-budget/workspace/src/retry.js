'use strict';

async function retry(operation, options) {
  let lastError;
  for (let attempt = 0; attempt <= options.maxAttempts; attempt += 1) {
    try {
      return await operation();
    } catch (error) {
      lastError = error;
      if (options.now() > options.deadlineMs) {
        throw error;
      }
      await options.sleep();
    }
  }
  throw lastError;
}

module.exports = { retry };
