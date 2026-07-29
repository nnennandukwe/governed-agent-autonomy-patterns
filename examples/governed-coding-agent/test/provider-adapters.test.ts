import assert from 'node:assert/strict';
import test from 'node:test';

import { AnthropicMessagesAdapter } from '../src/anthropic-adapter.js';
import { OpenAIResponsesAdapter } from '../src/openai-adapter.js';
import type { ModelTurnInput } from '../src/types.js';

const tool = {
  name: 'repo.read',
  description: 'Read a repository file.',
  inputSchema: {
    type: 'object',
    properties: { path: { type: 'string' } },
    required: ['path'],
  },
};

function input(overrides: Partial<ModelTurnInput> = {}): ModelTurnInput {
  return {
    system: 'Use tools through the harness.',
    initialPrompt: 'Inspect value.js',
    toolResults: [],
    tools: [tool],
    maxOutputTokens: 4096,
    requestTimeoutMs: 120_000,
    effort: 'medium',
    ...overrides,
  };
}

test('OpenAI adapter uses a stateless manual Responses tool loop', async () => {
  const requests: Array<{ body: Record<string, unknown>; options: unknown }> = [];
  const client = {
    responses: {
      async create(body: Record<string, unknown>, options: unknown) {
        requests.push({ body, options });
        return {
          _request_id: 'req_openai_1',
          status: 'completed',
          output_text: '',
          output: [{
            type: 'function_call',
            call_id: 'call-1',
            name: 'repo.read',
            arguments: '{"path":"value.js"}',
          }],
          usage: {
            input_tokens: 100,
            output_tokens: 20,
            input_tokens_details: {
              cached_tokens: 10,
              cache_write_tokens: 4,
            },
          },
        };
      },
    },
  };
  const adapter = new OpenAIResponsesAdapter({
    client,
    pricing: {
      inputPerMillion: 2.5,
      cachedInputPerMillion: 0.25,
      cacheWritePerMillion: 3.125,
      outputPerMillion: 15,
    },
  });

  const first = await adapter.nextTurn(input());

  assert.equal(requests[0]?.body.store, false);
  assert.deepEqual(requests[0]?.body.reasoning, { effort: 'medium' });
  assert.equal(requests[0]?.body.max_output_tokens, 4096);
  assert.deepEqual(first.toolCalls, [{
    id: 'call-1',
    name: 'repo.read',
    arguments: { path: 'value.js' },
  }]);
  assert.equal(first.requestId, 'req_openai_1');
  assert.equal(first.usage.cachedInputTokens, 10);
  assert.equal(first.usage.cacheWriteInputTokens, 4);
  assert.equal(first.usage.estimatedCostMicros, 530);
  assert.deepEqual(first.providerUsage, {
    input_tokens: 100,
    output_tokens: 20,
    cached_input_tokens: 10,
    cache_write_input_tokens: 4,
  });
  assert.ok(first.continuation);

  await adapter.nextTurn(input({
    initialPrompt: undefined,
    continuation: first.continuation,
    toolResults: [{
      toolCallId: 'call-1',
      name: 'repo.read',
      output: 'export const value = 1;',
    }],
  }));
  const secondInput = requests[1]?.body.input;
  assert.ok(Array.isArray(secondInput));
  assert.equal(
    secondInput.some(
      item => (
        typeof item === 'object'
        && item !== null
        && 'type' in item
        && item.type === 'function_call_output'
      ),
    ),
    true,
  );
});

test('Anthropic adapter owns a manual Messages tool_use/tool_result loop', async () => {
  const requests: Array<{ body: Record<string, unknown>; options: unknown }> = [];
  const client = {
    messages: {
      async create(body: Record<string, unknown>, options: unknown) {
        requests.push({ body, options });
        return {
          _request_id: 'req_anthropic_1',
          role: 'assistant',
          stop_reason: 'tool_use',
          content: [{
            type: 'tool_use',
            id: 'toolu-1',
            name: 'repo.read',
            input: { path: 'value.js' },
          }],
          usage: {
            input_tokens: 90,
            output_tokens: 30,
            cache_creation_input_tokens: 7,
            cache_read_input_tokens: 5,
          },
        };
      },
    },
  };
  const adapter = new AnthropicMessagesAdapter({
    client,
    pricing: {
      inputPerMillion: 2,
      cachedInputPerMillion: 0.2,
      cacheWritePerMillion: 2.5,
      outputPerMillion: 10,
    },
  });

  const first = await adapter.nextTurn(input());

  assert.deepEqual(
    requests[0]?.body.output_config,
    { effort: 'medium' },
  );
  assert.equal(requests[0]?.body.max_tokens, 4096);
  assert.deepEqual(first.toolCalls, [{
    id: 'toolu-1',
    name: 'repo.read',
    arguments: { path: 'value.js' },
  }]);
  assert.equal(first.requestId, 'req_anthropic_1');
  assert.equal(first.usage.cachedInputTokens, 5);
  assert.equal(first.usage.cacheWriteInputTokens, 7);
  assert.equal(first.usage.inputTokens, 102);
  assert.equal(first.usage.estimatedCostMicros, 499);
  assert.deepEqual(first.providerUsage, {
    input_tokens: 90,
    output_tokens: 30,
    cache_creation_input_tokens: 7,
    cache_read_input_tokens: 5,
  });

  await adapter.nextTurn(input({
    initialPrompt: undefined,
    continuation: first.continuation,
    toolResults: [{
      toolCallId: 'toolu-1',
      name: 'repo.read',
      output: 'export const value = 1;',
    }],
  }));
  const secondMessages = requests[1]?.body.messages;
  assert.ok(Array.isArray(secondMessages));
  assert.equal(
    secondMessages.some(
      message => (
        typeof message === 'object'
        && message !== null
        && 'content' in message
        && Array.isArray(message.content)
        && message.content.some(
          item => (
            typeof item === 'object'
            && item !== null
            && 'type' in item
            && item.type === 'tool_result'
          ),
        )
      ),
    ),
    true,
  );
});

test('provider adapters fail closed when usage accounting is missing', async () => {
  const openai = new OpenAIResponsesAdapter({
    client: {
      responses: {
        async create() {
          return {
            id: 'openai-no-usage',
            output: [],
          };
        },
      },
    },
  });
  const anthropic = new AnthropicMessagesAdapter({
    client: {
      messages: {
        async create() {
          return {
            id: 'anthropic-no-usage',
            role: 'assistant',
            content: [],
          };
        },
      },
    },
  });

  await assert.rejects(
    () => openai.nextTurn(input()),
    /usage.*fails closed/i,
  );
  await assert.rejects(
    () => anthropic.nextTurn(input()),
    /usage.*fails closed/i,
  );
});

test('provider adapters fail closed when cache usage categories are invalid', async () => {
  const openai = new OpenAIResponsesAdapter({
    client: {
      responses: {
        async create() {
          return {
            id: 'openai-invalid-cache-usage',
            output: [],
            usage: {
              input_tokens: 100,
              output_tokens: 1,
              input_tokens_details: {
                cached_tokens: 80,
                cache_write_tokens: 30,
              },
            },
          };
        },
      },
    },
  });
  const anthropic = new AnthropicMessagesAdapter({
    client: {
      messages: {
        async create() {
          return {
            id: 'anthropic-invalid-cache-usage',
            role: 'assistant',
            content: [],
            usage: {
              input_tokens: 100,
              output_tokens: 1,
              cache_creation_input_tokens: -1,
              cache_read_input_tokens: 0,
            },
          };
        },
      },
    },
  });

  await assert.rejects(
    () => openai.nextTurn(input()),
    /cache usage categories.*fails? closed/i,
  );
  await assert.rejects(
    () => anthropic.nextTurn(input()),
    /cache usage categories.*fails? closed/i,
  );
});
