import type { HermesCapabilities, HermesRun, HermesRunCreateRequest } from "../../types/hermes.js";

export interface HermesHttpClientOptions {
  baseUrl: string;
  apiKey?: string;
  timeoutMs?: number;
  fetchImpl?: typeof fetch;
}

export class HermesHttpError extends Error {
  constructor(
    message: string,
    public readonly status: number,
    public readonly responseBody: string
  ) {
    super(message);
    this.name = "HermesHttpError";
  }
}

export class HermesHttpClient {
  private readonly baseUrl: string;
  private readonly fetchImpl: typeof fetch;

  constructor(private readonly options: HermesHttpClientOptions) {
    this.baseUrl = options.baseUrl.replace(/\/+$/, "");
    this.fetchImpl = options.fetchImpl ?? fetch;
  }

  async capabilities(): Promise<HermesCapabilities> {
    return this.request<HermesCapabilities>("/v1/capabilities");
  }

  async models(): Promise<unknown> {
    return this.request<unknown>("/v1/models");
  }

  async createRun(payload: HermesRunCreateRequest): Promise<HermesRun> {
    return this.request<HermesRun>("/v1/runs", {
      method: "POST",
      body: JSON.stringify(payload)
    });
  }

  async getRun(runId: string): Promise<HermesRun> {
    return this.request<HermesRun>(`/v1/runs/${encodeURIComponent(runId)}`);
  }

  async stopRun(runId: string): Promise<unknown> {
    return this.request(`/v1/runs/${encodeURIComponent(runId)}/stop`, { method: "POST" });
  }

  async approveRun(runId: string, payload: Record<string, unknown>): Promise<unknown> {
    return this.request(`/v1/runs/${encodeURIComponent(runId)}/approval`, {
      method: "POST",
      body: JSON.stringify(payload)
    });
  }

  async *streamRunEvents(runId: string, signal?: AbortSignal): AsyncGenerator<{event: string; data: unknown; id?: string}> {
    const response = await this.fetchImpl(
      `${this.baseUrl}/v1/runs/${encodeURIComponent(runId)}/events`,
      { headers: this.headers({ Accept: "text/event-stream" }), signal }
    );
    if (!response.ok) await this.throwHttpError(response);
    if (!response.body) throw new Error("Hermes SSE response has no body");

    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let buffer = "";
    try {
      while (true) {
        const { value, done } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        let boundary: number;
        while ((boundary = buffer.indexOf("\n\n")) >= 0) {
          const block = buffer.slice(0, boundary).replace(/\r/g, "");
          buffer = buffer.slice(boundary + 2);
          const parsed = parseSseBlock(block);
          if (parsed) yield parsed;
        }
      }
      buffer += decoder.decode();
      const finalEvent = parseSseBlock(buffer.replace(/\r/g, ""));
      if (finalEvent) yield finalEvent;
    } finally {
      reader.releaseLock();
    }
  }

  private async request<T>(path: string, init: RequestInit = {}): Promise<T> {
    const response = await this.fetchImpl(`${this.baseUrl}${path}`, {
      ...init,
      headers: {
        ...this.headers({ "Content-Type": "application/json" }),
        ...(init.headers ?? {})
      },
      signal: init.signal ?? AbortSignal.timeout(this.options.timeoutMs ?? 30_000)
    });
    if (!response.ok) await this.throwHttpError(response);
    if (response.status === 204) return undefined as T;
    return await response.json() as T;
  }

  private headers(extra: Record<string, string>): Record<string, string> {
    return {
      ...extra,
      ...(this.options.apiKey ? { Authorization: `Bearer ${this.options.apiKey}` } : {})
    };
  }

  private async throwHttpError(response: Response): Promise<never> {
    const body = await response.text();
    throw new HermesHttpError(`Hermes API returned HTTP ${response.status}`, response.status, body);
  }
}

export function parseSseBlock(block: string): {event: string; data: unknown; id?: string} | null {
  if (!block.trim() || block.startsWith(":")) return null;
  let event = "message";
  let id: string | undefined;
  const dataLines: string[] = [];
  for (const line of block.split("\n")) {
    if (!line || line.startsWith(":")) continue;
    const colon = line.indexOf(":");
    const field = colon < 0 ? line : line.slice(0, colon);
    const value = colon < 0 ? "" : line.slice(colon + 1).replace(/^ /, "");
    if (field === "event") event = value;
    if (field === "id") id = value;
    if (field === "data") dataLines.push(value);
  }
  if (dataLines.length === 0) return null;
  const raw = dataLines.join("\n");
  let data: unknown = raw;
  try { data = JSON.parse(raw); } catch { /* keep plain text */ }
  return { event, data, ...(id !== undefined ? { id } : {}) };
}
