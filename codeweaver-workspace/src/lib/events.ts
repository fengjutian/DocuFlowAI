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
 * A console that stays open outlives many socket failures (sleep, a network switch, a server
 * restart), so a dropped stream reconnects with a growing backoff instead of degrading to a
 * one-shot poll. Only an environment that cannot read a streaming response at all falls back to
 * polling the same journal through `GET /executions/{id}/events?since=`, which is the contract the
 * SSE frames are built from.
 */
export type StreamStatus = "connecting" | "open" | "reconnecting" | "polling" | "closed" | "error";

export interface EventStreamOptions {
  executionId?: string;
  since?: number;
  onEvent: (event: CodeWeaverEvent) => void;
  onStatus?: (status: StreamStatus, detail?: string) => void;
  /**
   * Fired once the stream is usable again after a break (including the polling fallback).
   *
   * The socket only carries what happens while it is listening, so whoever owns the page has to
   * re-read what it missed — that is where the execution list comes back from.
   */
  onReconnect?: () => void;
}

const POLL_INTERVAL_MS = 2000;
/** Backoff ladder for a broken stream, capped at 8 seconds. */
const RECONNECT_BACKOFF_MS = [1000, 2000, 4000, 8000];
/** A connection that lived this long counts as stable and restarts the backoff ladder. */
const STABLE_CONNECTION_MS = 10_000;
/** How many event ids are remembered for deduplication; enough to cover a replay boundary. */
const MAX_SEEN_IDS = 4096;

export function subscribeEvents(options: EventStreamOptions): () => void {
  const controller = new AbortController();
  let closed = false;
  let pollTimer: ReturnType<typeof setTimeout> | undefined;
  let reconnectTimer: ReturnType<typeof setTimeout> | undefined;
  /** The journal position this client already holds; a reconnect asks for everything after it. */
  let since = options.since ?? 0;
  /** Consecutive failures; they pick the step of the backoff ladder. */
  let failures = 0;
  /** True between a break and the moment the stream is usable again. */
  let broken = false;
  let openedAt = 0;
  const seenIds = new Set<string>();
  const seenOrder: string[] = [];

  const emit = (event: CodeWeaverEvent) => {
    if (closed) return;
    // The same id is handed over once: a reconnect replays the entries the socket already carried,
    // and a timeline that shows them twice would count one event as two.
    if (event.id) {
      if (seenIds.has(event.id)) return;
      seenIds.add(event.id);
      seenOrder.push(event.id);
      if (seenOrder.length > MAX_SEEN_IDS) {
        const oldest = seenOrder.shift();
        if (oldest !== undefined) seenIds.delete(oldest);
      }
    }
    options.onEvent(event);
  };

  const startPolling = (detail: string) => {
    if (closed) return;
    const executionId = options.executionId;
    // Polling replays a single execution's journal. There is no equivalent for the global stream:
    // the server only journals per execution, so say the stream is gone instead of pretending.
    if (!executionId) {
      options.onStatus?.("error", `${detail}；全局事件流不可轮询，执行清单改由服务端清单接口恢复`);
      return;
    }
    options.onStatus?.("polling", detail);
    const tick = async () => {
      if (closed) return;
      try {
        const { events } = await api.getExecutionEvents(executionId, since);
        for (const event of events) {
          since = Math.max(since, event.sequence);
          emit(event);
        }
      } catch (error) {
        options.onStatus?.("error", error instanceof Error ? error.message : String(error));
      }
      if (!closed) pollTimer = setTimeout(() => void tick(), POLL_INTERVAL_MS);
    };
    void tick();
  };

  const connect = async () => {
    if (closed) return;
    const base = normalizeBaseUrl(useConnection.getState().baseUrl);
    const token = useConnection.getState().token;
    const params = new URLSearchParams();
    if (options.executionId) {
      params.set("executionId", options.executionId);
      // `since` is a journal position: the server replays every entry after it and then joins the
      // live frames, so the same entry never arrives twice.
      if (since > 0) params.set("since", String(since));
    }
    // The global stream carries no `since`: the journal is stored per execution, so there is no
    // global position to resume from. What the global stream missed is recovered from the
    // execution list instead.
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
        // This is the only case that degrades to polling: the environment cannot read a streaming
        // response, so the frames have to be fetched one page at a time.
        startPolling("当前环境不支持流式响应，已切换到轮询");
        options.onReconnect?.();
        return;
      }

      const recovered = broken;
      broken = false;
      // A connection that held for a while proves the server is fine, so the ladder starts over.
      if (openedAt > 0 && Date.now() - openedAt > STABLE_CONNECTION_MS) failures = 0;
      openedAt = Date.now();
      options.onStatus?.("open");
      if (recovered) options.onReconnect?.();

      const reader = body.getReader();
      const decoder = new TextDecoder();
      let buffer = "";

      for (;;) {
        const { value, done } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        let boundary = frameBoundary(buffer);
        while (boundary) {
          const frame = buffer.slice(0, boundary.index);
          buffer = buffer.slice(boundary.index + boundary.length);
          const event = parseFrame(frame);
          if (event) {
            if (options.executionId) since = Math.max(since, event.sequence);
            emit(event);
          }
          boundary = frameBoundary(buffer);
        }
      }

      if (!closed) {
        broken = true;
        failures += 1;
        scheduleReconnect("事件流已断开");
      }
    } catch (error) {
      if (closed || controller.signal.aborted) return;
      const message = error instanceof Error ? error.message : String(error);
      broken = true;
      failures += 1;
      scheduleReconnect(`事件流不可用（${message}）`);
    }
  };

  const scheduleReconnect = (detail: string) => {
    if (closed) return;
    const delay = RECONNECT_BACKOFF_MS[Math.min(failures - 1, RECONNECT_BACKOFF_MS.length - 1)];
    options.onStatus?.("reconnecting", `${detail}；${Math.round(delay / 1000)} 秒后重连`);
    reconnectTimer = setTimeout(() => void connect(), delay);
  };

  options.onStatus?.("connecting");
  void connect();

  return () => {
    closed = true;
    controller.abort();
    if (pollTimer) clearTimeout(pollTimer);
    if (reconnectTimer) clearTimeout(reconnectTimer);
    options.onStatus?.("closed");
  };
}

/** Finds the end of the next SSE frame; both line endings are accepted. */
function frameBoundary(buffer: string): { index: number; length: number } | undefined {
  const lf = buffer.indexOf("\n\n");
  const crlf = buffer.indexOf("\r\n\r\n");
  if (lf === -1 && crlf === -1) return undefined;
  if (crlf !== -1 && (lf === -1 || crlf < lf)) return { index: crlf, length: 4 };
  return { index: lf, length: 2 };
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
