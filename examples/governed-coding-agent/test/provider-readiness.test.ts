import assert from 'node:assert/strict';
import test from 'node:test';

import { verifyProviderModelAvailability } from '../src/provider-readiness.js';

test('provider readiness requires the exact frozen model identity', async () => {
  const fetchImpl = async () => new Response(
    JSON.stringify({ id: 'substituted-model' }),
    {
      status: 200,
      headers: { 'content-type': 'application/json' },
    },
  );

  await assert.rejects(
    () => verifyProviderModelAvailability({
      provider: 'openai',
      model: 'gpt-5.6-terra',
      apiKey: 'fixture',
      fetchImpl,
    }),
    /instead of frozen/,
  );
});
