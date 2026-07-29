import OpenAI from 'openai';

import type {
  ModelAdapter,
  ModelTurnInput,
  ModelTurnResult,
} from './types.js';

export interface OpenAIAdapterOptions {
  apiKey?: string;
  model?: string;
  pricing?: {
    inputPerMillion: number;
    cachedInputPerMillion: number;
    cacheWritePerMillion: number;
    outputPerMillion: number;
  };
  client?: unknown;
}

interface OpenAIResponseLike {
  _request_id?: string;
  id?: string;
  status?: string;
  output_text?: string;
  output: Array<Record<string, unknown>>;
  usage?: {
    input_tokens?: number;
    output_tokens?: number;
    input_tokens_details?: {
      cached_tokens?: number;
      cache_write_tokens?: number;
    };
  };
}

interface OpenAIClientLike {
  responses: {
    create(
      body: Record<string, unknown>,
      options: { timeout: number },
    ): Promise<OpenAIResponseLike>;
  };
}

interface OpenAIContinuation {
  kind: 'openai-responses-v1';
  items: unknown[];
}

function continuation(value: unknown): OpenAIContinuation | undefined {
  if (value === undefined) return undefined;
  if (
    typeof value !== 'object'
    || value === null
    || !('kind' in value)
    || value.kind !== 'openai-responses-v1'
    || !('items' in value)
    || !Array.isArray(value.items)
  ) {
    throw new Error('Invalid OpenAI continuation state.');
  }
  return value as OpenAIContinuation;
}

function parseArguments(value: unknown): Record<string, unknown> {
  if (typeof value !== 'string') {
    throw new Error('OpenAI function call arguments were not JSON text.');
  }
  const parsed: unknown = JSON.parse(value);
  if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
    throw new Error('OpenAI function call arguments were not a JSON object.');
  }
  return parsed as Record<string, unknown>;
}

function estimatedMicros(
  inputTokens: number,
  outputTokens: number,
  cachedInputTokens: number,
  cacheWriteInputTokens: number,
  pricing: Required<OpenAIAdapterOptions>['pricing'],
): number {
  const uncached = Math.max(
    0,
    inputTokens - cachedInputTokens - cacheWriteInputTokens,
  );
  return Math.round(
    uncached * pricing.inputPerMillion
    + cachedInputTokens * pricing.cachedInputPerMillion
    + cacheWriteInputTokens * pricing.cacheWritePerMillion
    + outputTokens * pricing.outputPerMillion,
  );
}

export class OpenAIResponsesAdapter implements ModelAdapter {
  readonly provider = 'openai' as const;
  readonly model: string;
  private readonly client: OpenAIClientLike;
  private readonly pricing: Required<OpenAIAdapterOptions>['pricing'];

  constructor(options: OpenAIAdapterOptions = {}) {
    this.model = options.model ?? 'gpt-5.6-terra';
    this.pricing = options.pricing ?? {
      inputPerMillion: 2.5,
      cachedInputPerMillion: 0.25,
      cacheWritePerMillion: 3.125,
      outputPerMillion: 15,
    };
    this.client = options.client
      ? options.client as OpenAIClientLike
      : new OpenAI({
          ...(options.apiKey ? { apiKey: options.apiKey } : {}),
          maxRetries: 0,
          timeout: 120_000,
        }) as unknown as OpenAIClientLike;
  }

  async nextTurn(input: ModelTurnInput): Promise<ModelTurnResult> {
    const previous = continuation(input.continuation);
    const items = previous
      ? [...previous.items]
      : [{
          role: 'user',
          content: input.initialPrompt ?? '',
        }];
    for (const result of input.toolResults) {
      items.push({
        type: 'function_call_output',
        call_id: result.toolCallId,
        output: result.output,
      });
    }
    const response = await this.client.responses.create({
      model: this.model,
      instructions: input.system,
      input: items,
      tools: input.tools.map(tool => ({
        type: 'function',
        name: tool.name,
        description: tool.description,
        parameters: tool.inputSchema,
        strict: true,
      })),
      store: false,
      reasoning: { effort: input.effort },
      max_output_tokens: input.maxOutputTokens,
    }, {
      timeout: input.requestTimeoutMs,
    });
    if (
      !response.usage
      || !Number.isSafeInteger(response.usage.input_tokens)
      || !Number.isSafeInteger(response.usage.output_tokens)
    ) {
      throw new Error(
        'OpenAI response omitted valid token usage; runtime accounting fails closed.',
      );
    }
    const inputTokens = response.usage?.input_tokens ?? 0;
    const outputTokens = response.usage?.output_tokens ?? 0;
    const cachedInputTokens = (
      response.usage?.input_tokens_details?.cached_tokens ?? 0
    );
    const cacheWriteInputTokens = (
      response.usage?.input_tokens_details?.cache_write_tokens ?? 0
    );
    if (
      !Number.isSafeInteger(cachedInputTokens)
      || cachedInputTokens < 0
      || !Number.isSafeInteger(cacheWriteInputTokens)
      || cacheWriteInputTokens < 0
      || cachedInputTokens + cacheWriteInputTokens > inputTokens
    ) {
      throw new Error(
        'OpenAI cache usage categories were invalid; runtime accounting fails closed.',
      );
    }
    const requestId = response._request_id ?? response.id;
    if (!requestId) {
      throw new Error(
        'OpenAI response omitted a request ID; evidence capture fails closed.',
      );
    }
    const toolCalls = response.output
      .filter(item => item.type === 'function_call')
      .map(item => ({
        id: String(item.call_id ?? ''),
        name: String(item.name ?? ''),
        arguments: parseArguments(item.arguments),
      }));
    return {
      requestId,
      text: response.output_text ?? '',
      toolCalls,
      usage: {
        inputTokens,
        outputTokens,
        cachedInputTokens,
        cacheWriteInputTokens,
        estimatedCostMicros: estimatedMicros(
          inputTokens,
          outputTokens,
          cachedInputTokens,
          cacheWriteInputTokens,
          this.pricing,
        ),
      },
      providerUsage: {
        input_tokens: inputTokens,
        output_tokens: outputTokens,
        cached_input_tokens: cachedInputTokens,
        cache_write_input_tokens: cacheWriteInputTokens,
      },
      continuation: {
        kind: 'openai-responses-v1',
        items: [...items, ...response.output],
      } satisfies OpenAIContinuation,
      stopReason: response.status ?? (
        toolCalls.length > 0 ? 'tool_use' : 'completed'
      ),
    };
  }
}
