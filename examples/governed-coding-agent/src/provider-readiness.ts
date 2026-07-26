export interface ProviderReadinessRequest {
  provider: 'openai' | 'anthropic';
  model: string;
  apiKey: string;
  timeoutMs?: number;
  fetchImpl?: typeof fetch;
}

export async function verifyProviderModelAvailability(
  request: ProviderReadinessRequest,
): Promise<void> {
  const fetchImpl = request.fetchImpl ?? fetch;
  const controller = new AbortController();
  const timer = setTimeout(
    () => controller.abort(),
    request.timeoutMs ?? 15_000,
  );
  const url = request.provider === 'openai'
    ? `https://api.openai.com/v1/models/${encodeURIComponent(request.model)}`
    : `https://api.anthropic.com/v1/models/${encodeURIComponent(request.model)}`;
  const headers = request.provider === 'openai'
    ? { authorization: `Bearer ${request.apiKey}` }
    : {
        'x-api-key': request.apiKey,
        'anthropic-version': '2023-06-01',
      };
  try {
    const response = await fetchImpl(url, {
      method: 'GET',
      headers,
      signal: controller.signal,
    });
    const body = await response.json() as { id?: unknown; error?: unknown };
    if (!response.ok) {
      throw new Error(
        `${request.provider} model lookup returned HTTP ${response.status}: ${JSON.stringify(body.error ?? body)}`,
      );
    }
    if (body.id !== request.model) {
      throw new Error(
        `${request.provider} returned model ${String(body.id)} instead of frozen ${request.model}.`,
      );
    }
  } catch (error) {
    if (error instanceof Error && error.name === 'AbortError') {
      throw new Error(
        `${request.provider} model availability check timed out.`,
      );
    }
    throw error;
  } finally {
    clearTimeout(timer);
  }
}
