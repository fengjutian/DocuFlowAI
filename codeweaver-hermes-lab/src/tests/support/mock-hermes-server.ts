import { createServer, type IncomingMessage, type ServerResponse } from "node:http";

/**
 * A controllable Hermes HTTP server for contract tests.
 *
 * This proves the client, the SSE parser and the Phase 0 runner work against a conforming server. It
 * does not prove anything about the real runtime: behavior differences (event names, approval
 * semantics) still need the pinned version. Keep that distinction in the test names.
 */
export interface MockEvent {
  event: string;
  data: unknown;
  id?: string;
  /** Delay before writing this frame, to exercise streaming and aborts. */
  delayMs?: number;
  /**
   * Emit the event name inside the JSON payload instead of as an SSE `event:` field.
   * The real 0.23.0 API server does exactly this: the frame name stays at the SSE
   * default `message` and the runtime event name travels in `data.event`.
   */
  nameInPayload?: boolean;
}

export interface MockHermesOptions {
  token?: string;
  capabilities?: Record<string, boolean | string>;
  events?: MockEvent[];
  /** Emit the runtime event name inside the JSON payload, the way the real API server does. */
  payloadNamedEvents?: boolean;
  runStatus?: string;
}

export interface MockHermes {
  url: string;
  requests: Array<{ method: string; path: string; body?: unknown }>;
  runs: Map<string, { status: string }>;
  approvals: Array<Record<string, unknown>>;
  close(): Promise<void>;
}

const DEFAULT_EVENTS: MockEvent[] = [
  { event: "run.started", data: { run_id: "placeholder" }, id: "1" },
  { event: "message.delta", data: { text: "hello " }, id: "2" },
  { event: "message.delta", data: { text: "world" }, id: "3" },
  {
    event: "run.completed",
    data: {
      output: "hello world",
      model: "mock-model",
      usage: { prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 }
    },
    id: "4"
  }
];

/**
 * The event shape the real 0.23.0 API server produces: no SSE `event:` field, and the runtime
 * event name inside the JSON payload.
 */
const PAYLOAD_NAMED_EVENTS: MockEvent[] = [
  { event: "run.started", data: { run_id: "placeholder" }, id: "1", nameInPayload: true },
  { event: "message.delta", data: { event: "message.delta", delta: "hello " }, id: "2", nameInPayload: true },
  { event: "message.delta", data: { event: "message.delta", delta: "world" }, id: "3", nameInPayload: true },
  {
    event: "run.completed",
    data: { event: "run.completed", output: "hello world", completed: true },
    id: "4",
    nameInPayload: true
  }
];

export async function startMockHermes(options: MockHermesOptions = {}): Promise<MockHermes> {
  const requests: MockHermes["requests"] = [];
  const runs = new Map<string, { status: string }>();
  const approvals: Array<Record<string, unknown>> = [];
  let counter = 0;
  const events = options.events ?? (options.payloadNamedEvents ? PAYLOAD_NAMED_EVENTS : DEFAULT_EVENTS);

  const server = createServer((request, response) => {
    void handle(request, response);
  });

  async function handle(request: IncomingMessage, response: ServerResponse): Promise<void> {
    const url = new URL(request.url ?? "/", "http://localhost");
    const method = request.method ?? "GET";
    const body = method === "POST" ? await readBody(request) : undefined;
    requests.push({ method, path: url.pathname, ...(body === undefined ? {} : { body }) });

    if (options.token && request.headers.authorization !== `Bearer ${options.token}`) {
      json(response, 401, { error: "unauthorized" });
      return;
    }

    if (method === "GET" && url.pathname === "/v1/capabilities") {
      json(response, 200, {
        object: "capabilities",
        platform: "mock-hermes",
        features: options.capabilities ?? { run_submission: true, run_events_sse: true }
      });
      return;
    }

    if (method === "GET" && url.pathname === "/v1/models") {
      json(response, 200, { data: [{ id: "mock-model", object: "model" }] });
      return;
    }

    if (method === "POST" && url.pathname === "/v1/runs") {
      counter += 1;
      const runId = `run_mock_${counter}`;
      runs.set(runId, { status: options.runStatus ?? "running" });
      json(response, 201, { run_id: runId, status: "queued" });
      return;
    }

    const runMatch = /^\/v1\/runs\/([^/]+)$/.exec(url.pathname);
    if (method === "GET" && runMatch) {
      const runId = runMatch[1]!;
      const run = runs.get(runId);
      if (!run) return json(response, 404, { error: "run_not_found" });
      json(response, 200, { run_id: runId, status: run.status });
      return;
    }

    if (method === "POST" && /^\/v1\/runs\/[^/]+\/stop$/.test(url.pathname)) {
      const runId = url.pathname.split("/")[3]!;
      if (runs.has(runId)) runs.set(runId, { status: "cancelled" });
      json(response, 200, { run_id: runId, status: "cancelled" });
      return;
    }

    if (method === "POST" && /^\/v1\/runs\/[^/]+\/approval$/.test(url.pathname)) {
      approvals.push((body ?? {}) as Record<string, unknown>);
      json(response, 200, { accepted: true });
      return;
    }

    if (method === "GET" && /^\/v1\/runs\/[^/]+\/events$/.test(url.pathname)) {
      const runId = url.pathname.split("/")[3]!;
      if (!runs.has(runId)) return json(response, 404, { error: "run_not_found" });
      await streamEvents(response, runId);
      return;
    }

    json(response, 404, { error: "not_found", path: url.pathname });
  }

  async function streamEvents(response: ServerResponse, runId: string): Promise<void> {
    response.writeHead(200, {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache",
      Connection: "keep-alive"
    });
    for (const event of events) {
      if (event.delayMs) await new Promise(resolve => setTimeout(resolve, event.delayMs));
      if (response.destroyed) return;
      const data = event.event === "run.started" && isRecord(event.data)
        ? { ...event.data, run_id: runId }
        : event.data;
      const frame = event.nameInPayload
        ? `${event.id ? `id: ${event.id}\n` : ""}data: ${JSON.stringify(data)}\n\n`
        : `${event.id ? `id: ${event.id}\n` : ""}event: ${event.event}\ndata: ${JSON.stringify(data)}\n\n`;
      response.write(frame);
    }
    response.end();
  }

  const port = await new Promise<number>((resolveListen, reject) => {
    server.on("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      resolveListen(typeof address === "object" && address ? address.port : 0);
    });
  });

  return {
    url: `http://127.0.0.1:${port}`,
    requests,
    runs,
    approvals,
    close: () => new Promise<void>((done, fail) =>
      server.close(error => (error ? fail(error) : done()))
    )
  };
}

function json(response: ServerResponse, status: number, payload: unknown): void {
  const body = JSON.stringify(payload);
  response.writeHead(status, { "Content-Type": "application/json", "Content-Length": Buffer.byteLength(body) });
  response.end(body);
}

function readBody(request: IncomingMessage): Promise<unknown> {
  return new Promise(resolveBody => {
    const chunks: Buffer[] = [];
    request.on("data", chunk => chunks.push(chunk as Buffer));
    request.on("end", () => {
      if (chunks.length === 0) return resolveBody(undefined);
      try {
        resolveBody(JSON.parse(Buffer.concat(chunks).toString("utf8")));
      } catch {
        resolveBody(undefined);
      }
    });
  });
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
