import { api, ApiError } from "./api";
import { normalizeBaseUrl, useConnection } from "./settings";
import type { CodeWeaverEvent } from "./types";

/**
 * Live event subscription (design 13).
 *
 * The transport is SSE over `GET /api/v1/events`, and the server persists every event: the socket is
 * a notification channel, never the source of truth. Two rules follow from that and are enforced
 * here:
 *
 * 1. A reconnect replays from `since`, so a gap in the socket is repaired instead of silently
 *    shortening the timeline.
 * 2. Receiving the last message is never treated as "the run succeeded" — the UI reads the terminal
 *    state and the evaluation gate from the execution endpoint instead.
 *
 * If the socket cannot be established (some webview builds refuse streaming responses), the client
 * falls back to polling the same journal through `GET /executions/{id}/events?since=`, which is the
 * contract the SSE frames are built from.
 */
export type StreamStatus = "connecting" | "open" | "polling" | "closed" | "error";

export interface EventStreamOptions {
  executionId?: string;
  since?: number;
  onEvent: (event: CodeWeaverEvent) => void;
  onStatus?: (status: StreamStatus, detail?: string) => void;
}

const POLL_INTERVAL_MS = 2000;

export function subscribeEvents(options: EventStreamOptions): () => void {
  const controller = new AbortController();
  let closed = false;
  let pollTimer: ReturnType<typeof setTimeout> | undefined;

  const emit = (event: CodeWeaverEvent) => {
    if (!closed) options.onEvent(event);
  };

  const startPolling = (detail: string) => {
    if (closed) return;
    // Polling replays a single execution's journal. There is no equivalent for the global stream:
    // the server only journals per execution, so say the stream is gone instead of pretending.
    if (!options.executionId) {
      options.onStatus?.("error", `${detail}；全局事件流不可用，执行清单只能来自实时事件`);
      return;
    }
    options.onStatus?.("polling", detail);
    const tick = async () => {
      if (closed) return;
      try {
        if (options.executionId) {
          const { events } = await api.getExecutionEvents(options.executionId, options.since ?? 0);
          for (const event of events) {
            options.since = Math.max(options.since ?? 0, event.sequence);
            emit(event);
          }
        }
      } catch (error) {
        options.onStatus?.("error", error instanceof Error ? error.message : String(error));
      }
      if (!closed) pollTimer = setTimeout(() => void tick(), POLL_INTERVAL_MS);
    };
    void tick();
  };

  const connect = async () => {
    options.onStatus?.("connecting");
    const base = normalizeBaseUrl(useConnection.getState().baseUrl);
    const token = useConnection.getState().token;
    const params = new URLSearchParams();
    if (options.executionId) params.set("executionId", options.executionId);
    if (options.since) params.set("since", String(options.since));
    const query = params.toString();

    try {
      const response = await fetch(`${base}/api/v1/events${query ? `?${query}` : ""}`, {
        headers: {
          Accept: "text/event-stream",
          ...(token ? { Authorization: `Bearer ${token}` } : {})
        },
        signal: controller.signal
      });

      if (!response.ok) {
        const text = await response.text();
        throw new ApiError(response.status, "stream_http_error", text || `HTTP ${response.status}`);
      }

      const body = response.body;
      if (!body || typeof body.getReader !== "function") {
        startPolling("当前环境不支持流式响应，已切换到轮询");
        return;
      }

      options.onStatus?.("open");
      const reader = body.getReader();
      const decoder = new TextDecoder();
      let buffer = "";

      for (;;) {
        const { value, done } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        let boundary = buffer.indexOf("\n\n");
        while (boundary !== -1) {
          const frame = buffer.slice(0, boundary);
          buffer = buffer.slice(boundary + 2);
          const event = parseFrame(frame);
          if (event) {
            if (options.executionId) {
              options.since = Math.max(options.since ?? 0, event.sequence);
            }
            emit(event);
          }
          boundary = buffer.indexOf("\n\n");
        }
      }

      if (!closed) startPolling("事件流已断开，已切换到轮询");
    } catch (error) {
      if (closed || controller.signal.aborted) return;
      const message = error instanceof Error ? error.message : String(error);
      startPolling(`事件流不可用（${message}），已切换到轮询`);
    }
  };

  void connect();

  return () => {
    closed = true;
    controller.abort();
    if (pollTimer) clearTimeout(pollTimer);
    options.onStatus?.("closed");
  };
}

/** Parses one SSE frame. Heartbeat comments and unknown frames are ignored. */
function parseFrame(frame: string): CodeWeaverEvent | undefined {
  const dataLines: string[] = [];
  for (const rawLine of frame.split("\n")) {
    const line = rawLine.replace(/\r$/, "");
    if (line === "" || line.startsWith(":")) continue;
    if (line.startsWith("data:")) dataLines.push(line.slice(5).trimStart());
  }
  if (dataLines.length === 0) return undefined;
  try {
    return JSON.parse(dataLines.join("\n")) as CodeWeaverEvent;
  } catch {
    return undefined;
  }
}
