import Anthropic from '@anthropic-ai/sdk';

import type {
  ModelAdapter,
  ModelTurnInput,
  ModelTurnResult,
} from './types.js';

export interface AnthropicAdapterOptions {
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

interface AnthropicMessageLike {
  _request_id?: string;
  id?: string;
  role: 'assistant';
  stop_reason?: string | null;
  content: Array<Record<string, unknown>>;
  usage?: {
    input_tokens?: number;
    output_tokens?: number;
    cache_creation_input_tokens?: number;
    cache_read_input_tokens?: number;
  };
}

interface AnthropicClientLike {
  messages: {
    create(
      body: Record<string, unknown>,
      options: { timeout: number },
    ): Promise<AnthropicMessageLike>;
  };
}

interface AnthropicContinuation {
  kind: 'anthropic-messages-v1';
  messages: Array<Record<string, unknown>>;
}

function continuation(value: unknown): AnthropicContinuation | undefined {
  if (value === undefined) return undefined;
  if (
    typeof value !== 'object'
    || value === null
    || !('kind' in value)
    || value.kind !== 'anthropic-messages-v1'
    || !('messages' in value)
    || !Array.isArray(value.messages)
  ) {
    throw new Error('Invalid Anthropic continuation state.');
  }
  return value as AnthropicContinuation;
}

function toolArguments(value: unknown): Record<string, unknown> {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new Error('Anthropic tool input was not a JSON object.');
  }
  return value as Record<string, unknown>;
}

function estimatedMicros(
  inputTokens: number,
  outputTokens: number,
  cachedInputTokens: number,
  cacheWriteInputTokens: number,
  pricing: Required<AnthropicAdapterOptions>['pricing'],
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

export class AnthropicMessagesAdapter implements ModelAdapter {
  readonly provider = 'anthropic' as const;
  readonly model: string;
  private readonly client: AnthropicClientLike;
  private readonly pricing: Required<AnthropicAdapterOptions>['pricing'];

  constructor(options: AnthropicAdapterOptions = {}) {
    this.model = options.model ?? 'claude-sonnet-5';
    this.pricing = options.pricing ?? {
      inputPerMillion: 2,
      cachedInputPerMillion: 0.2,
      cacheWritePerMillion: 2.5,
      outputPerMillion: 10,
    };
    this.client = options.client
      ? options.client as AnthropicClientLike
      : new Anthropic({
          ...(options.apiKey ? { apiKey: options.apiKey } : {}),
          maxRetries: 0,
          timeout: 120_000,
        }) as unknown as AnthropicClientLike;
  }

  async nextTurn(input: ModelTurnInput): Promise<ModelTurnResult> {
    const previous = continuation(input.continuation);
    const messages = previous
      ? [...previous.messages]
      : [{
          role: 'user',
          content: input.initialPrompt ?? '',
        }];
    if (input.toolResults.length > 0) {
      messages.push({
        role: 'user',
        content: input.toolResults.map(result => ({
          type: 'tool_result',
          tool_use_id: result.toolCallId,
          content: result.output,
          ...(result.isError ? { is_error: true } : {}),
        })),
      });
    }
    const response = await this.client.messages.create({
      model: this.model,
      system: input.system,
      max_tokens: input.maxOutputTokens,
      output_config: { effort: input.effort },
      messages,
      tools: input.tools.map(tool => ({
        name: tool.name,
        description: tool.description,
        input_schema: tool.inputSchema,
      })),
    }, {
      timeout: input.requestTimeoutMs,
    });
    if (
      !response.usage
      || !Number.isSafeInteger(response.usage.input_tokens)
      || !Number.isSafeInteger(response.usage.output_tokens)
    ) {
      throw new Error(
        'Anthropic response omitted valid token usage; runtime accounting fails closed.',
      );
    }
    const content = response.content;
    const toolCalls = content
      .filter(item => item.type === 'tool_use')
      .map(item => ({
        id: String(item.id ?? ''),
        name: String(item.name ?? ''),
        arguments: toolArguments(item.input),
      }));
    const text = content
      .filter(item => item.type === 'text')
      .map(item => String(item.text ?? ''))
      .join('\n');
    const baseInputTokens = response.usage?.input_tokens ?? 0;
    const outputTokens = response.usage?.output_tokens ?? 0;
    const cacheCreationInputTokens = (
      response.usage?.cache_creation_input_tokens ?? 0
    );
    const cachedInputTokens = (
      response.usage?.cache_read_input_tokens ?? 0
    );
    if (
      !Number.isSafeInteger(cacheCreationInputTokens)
      || cacheCreationInputTokens < 0
      || !Number.isSafeInteger(cachedInputTokens)
      || cachedInputTokens < 0
    ) {
      throw new Error(
        'Anthropic cache usage categories were invalid; runtime accounting fails closed.',
      );
    }
    const inputTokens = (
      baseInputTokens + cacheCreationInputTokens + cachedInputTokens
    );
    if (!Number.isSafeInteger(inputTokens)) {
      throw new Error(
        'Anthropic cache usage categories exceeded safe accounting limits; runtime accounting fails closed.',
      );
    }
    const requestId = response._request_id ?? response.id;
    if (!requestId) {
      throw new Error(
        'Anthropic response omitted a request ID; evidence capture fails closed.',
      );
    }
    return {
      requestId,
      text,
      toolCalls,
      usage: {
        inputTokens,
        outputTokens,
        cachedInputTokens,
        cacheWriteInputTokens: cacheCreationInputTokens,
        estimatedCostMicros: estimatedMicros(
          inputTokens,
          outputTokens,
          cachedInputTokens,
          cacheCreationInputTokens,
          this.pricing,
        ),
      },
      providerUsage: {
        input_tokens: baseInputTokens,
        output_tokens: outputTokens,
        cache_creation_input_tokens: cacheCreationInputTokens,
        cache_read_input_tokens: cachedInputTokens,
      },
      continuation: {
        kind: 'anthropic-messages-v1',
        messages: [
          ...messages,
          {
            role: 'assistant',
            content,
          },
        ],
      } satisfies AnthropicContinuation,
      stopReason: response.stop_reason ?? (
        toolCalls.length > 0 ? 'tool_use' : 'end_turn'
      ),
    };
  }
}
