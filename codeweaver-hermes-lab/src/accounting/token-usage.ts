/**
 * Token usage and the model, read out of a runtime payload.
 *
 * One implementation, because two of them is how a payload ends up accounted for in one place and
 * not the other. The runtime score already knew both shapes; the usage ledger knew only one, and
 * the runtime it missed was the primary one — a whole runtime's spending was invisible because the
 * two readers disagreed about where the counters live.
 *
 * The shapes:
 *
 *  - Hermes gateway: `usage: { input_tokens, output_tokens, total_tokens }` on the terminal event.
 *  - Pi: the counters sit on every assistant message inside the event's `messages`, one set per
 *    model request, so a run that asked the model five times spent the sum of five sets.
 *
 * A payload with no usage yields nothing rather than a zero that would look like a free run.
 */

export interface TokenUsage {
  /** Tokens the runtime counted as prompt/input. */
  promptTokens?: number;
  /** Tokens the runtime counted as completion/output. */
  completionTokens?: number;
  /** Tokens served from the provider's cache, when the runtime reports them separately. */
  cachedTokens?: number;
  /** What the runtime counted in total. */
  totalTokens?: number;
  /** The model the runtime says it used, when it says. */
  model?: string;
}

const PROMPT_KEYS = ["prompt_tokens", "input_tokens", "tokens_in", "promptTokens", "input"] as const;
const COMPLETION_KEYS = [
  "completion_tokens",
  "output_tokens",
  "tokens_out",
  "completionTokens",
  "output"
] as const;
const CACHED_KEYS = [
  "cached_tokens",
  "cache_read_tokens",
  "cache_read_input_tokens",
  "cachedTokens",
  "cacheRead"
] as const;
const TOTAL_KEYS = ["total_tokens", "tokens_total", "totalTokens", "total"] as const;

export function extractTokenUsage(payload: Record<string, unknown>): TokenUsage {
  const usage = isRecord(payload["usage"])
    ? payload["usage"]
    : isRecord(payload["token_usage"])
      ? payload["token_usage"]
      : undefined;

  let promptTokens = usage ? pickNumber(usage, PROMPT_KEYS) : undefined;
  let completionTokens = usage ? pickNumber(usage, COMPLETION_KEYS) : undefined;
  let cachedTokens = usage ? pickNumber(usage, CACHED_KEYS) : undefined;
  let totalTokens = usage ? pickNumber(usage, TOTAL_KEYS) : undefined;
  let model = typeof payload["model"] === "string" && payload["model"] !== ""
    ? payload["model"]
    : undefined;

  // Flat shape: the counters sit at the top level of the payload.
  if (totalTokens === undefined) {
    promptTokens ??= pickNumber(payload, PROMPT_KEYS);
    completionTokens ??= pickNumber(payload, COMPLETION_KEYS);
    cachedTokens ??= pickNumber(payload, CACHED_KEYS);
    totalTokens = pickNumber(payload, TOTAL_KEYS);
  }

  // Pi: the counters sit on the assistant messages, one set per model request.
  if (totalTokens === undefined) {
    const messages = Array.isArray(payload["messages"]) ? payload["messages"] : [];
    let prompt = 0;
    let completion = 0;
    let cached = 0;
    let total = 0;
    let counted = 0;
    for (const message of messages) {
      if (!isRecord(message)) continue;
      const messageUsage = isRecord(message["usage"]) ? message["usage"] : undefined;
      if (!messageUsage) continue;
      const messagePrompt = pickNumber(messageUsage, PROMPT_KEYS);
      const messageCompletion = pickNumber(messageUsage, COMPLETION_KEYS);
      const messageCached = pickNumber(messageUsage, CACHED_KEYS);
      const messageTotal = pickNumber(messageUsage, TOTAL_KEYS);
      if (messageTotal === undefined && messagePrompt === undefined && messageCompletion === undefined) continue;
      counted += 1;
      prompt += messagePrompt ?? 0;
      completion += messageCompletion ?? 0;
      cached += messageCached ?? 0;
      total += messageTotal ?? (messagePrompt ?? 0) + (messageCompletion ?? 0);
      model ??= typeof message["model"] === "string" && message["model"] !== ""
        ? message["model"]
        : undefined;
    }
    if (counted > 0) {
      promptTokens = prompt;
      completionTokens = completion;
      cachedTokens = cached;
      totalTokens = total;
    }
  }

  model ??= usage && typeof usage["model"] === "string" && usage["model"] !== ""
    ? usage["model"]
    : undefined;

  // A runtime that reports the parts but not the sum still accounted for the run. The cached tokens
  // are part of the total when the runtime counts them separately, so they are not left out.
  if (totalTokens === undefined && promptTokens !== undefined && completionTokens !== undefined) {
    totalTokens = promptTokens + completionTokens + (cachedTokens ?? 0);
  }

  return {
    ...(promptTokens !== undefined ? { promptTokens } : {}),
    ...(completionTokens !== undefined ? { completionTokens } : {}),
    ...(cachedTokens !== undefined ? { cachedTokens } : {}),
    ...(totalTokens !== undefined ? { totalTokens } : {}),
    ...(model !== undefined ? { model } : {})
  };
}

function pickNumber(source: Record<string, unknown>, keys: readonly string[]): number | undefined {
  for (const key of keys) {
    const value = source[key];
    if (typeof value === "number" && Number.isFinite(value) && value >= 0) return value;
  }
  return undefined;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
