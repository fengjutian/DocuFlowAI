import { randomUUID } from "node:crypto";
import type { AgentRuntimeEvent } from "../types/adapter.js";

/**
 * Cost and usage accounting (design 19 / AT-206).
 *
 * Runtime payloads differ, so extraction is tolerant: the first recognized token field wins, and an
 * event without usage data produces no record rather than a zero that would look like a free run.
 */
export interface UsageRecord {
  id: string;
  executionId: string;
  model: string;
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
  durationMs: number;
  startedAt?: string;
  finishedAt?: string;
}

export interface UsageSummary {
  executions: number;
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
  durationMs: number;
  byModel: Array<{ model: string; executions: number; totalTokens: number }>;
}

export class UsageLedger {
  private readonly records = new Map<string, UsageRecord>();

  record(record: UsageRecord): UsageRecord {
    if (!Number.isFinite(record.totalTokens) || record.totalTokens < 0) {
      throw new Error("totalTokens must be a non-negative number");
    }
    this.records.set(record.executionId, record);
    return record;
  }

  /**
   * Derives a usage record from a terminal runtime event.
   *
   * `durationMs` comes from the caller because the coordinator knows when the run started; the event
   * only carries token counters, and treating a missing duration as zero would understate cost.
   */
  recordFromEvent(
    event: AgentRuntimeEvent,
    timing: { durationMs: number; startedAt?: string; finishedAt?: string }
  ): UsageRecord | undefined {
    const payload = event.payload as Record<string, unknown>;
    const usage = (payload["usage"] ?? payload["token_usage"] ?? payload) as Record<string, unknown>;
    const promptTokens = pickNumber(usage, ["prompt_tokens", "input_tokens", "tokens_in", "promptTokens"]);
    const completionTokens = pickNumber(usage, ["completion_tokens", "output_tokens", "tokens_out", "completionTokens"]);
    const totalTokens = pickNumber(usage, ["total_tokens", "tokens_total", "totalTokens"])
      ?? (promptTokens !== undefined && completionTokens !== undefined
        ? promptTokens + completionTokens
        : undefined);
    if (totalTokens === undefined) return undefined;

    const model = typeof payload["model"] === "string"
      ? payload["model"]
      : typeof usage["model"] === "string"
        ? usage["model"]
        : "unknown";

    return this.record({
      id: `use_${randomUUID()}`,
      executionId: event.executionId,
      model,
      promptTokens: promptTokens ?? 0,
      completionTokens: completionTokens ?? 0,
      totalTokens,
      durationMs: Math.max(0, timing.durationMs),
      ...(timing.startedAt ? { startedAt: timing.startedAt } : {}),
      ...(timing.finishedAt ? { finishedAt: timing.finishedAt } : {})
    });
  }

  forExecution(executionId: string): UsageRecord | undefined {
    return this.records.get(executionId);
  }

  list(): UsageRecord[] {
    return [...this.records.values()].sort((a, b) => a.executionId.localeCompare(b.executionId));
  }

  summary(filter: { model?: string } = {}): UsageSummary {
    const records = this.list().filter(record => !filter.model || record.model === filter.model);
    const byModel = new Map<string, { executions: number; totalTokens: number }>();
    let promptTokens = 0;
    let completionTokens = 0;
    let totalTokens = 0;
    let durationMs = 0;
    for (const record of records) {
      promptTokens += record.promptTokens;
      completionTokens += record.completionTokens;
      totalTokens += record.totalTokens;
      durationMs += record.durationMs;
      const entry = byModel.get(record.model) ?? { executions: 0, totalTokens: 0 };
      entry.executions += 1;
      entry.totalTokens += record.totalTokens;
      byModel.set(record.model, entry);
    }
    return {
      executions: records.length,
      promptTokens,
      completionTokens,
      totalTokens,
      durationMs,
      byModel: [...byModel.entries()]
        .map(([model, entry]) => ({ model, ...entry }))
        .sort((a, b) => b.totalTokens - a.totalTokens)
    };
  }
}

function pickNumber(source: Record<string, unknown>, keys: readonly string[]): number | undefined {
  for (const key of keys) {
    const value = source[key];
    if (typeof value === "number" && Number.isFinite(value) && value >= 0) return value;
  }
  return undefined;
}
