import type { Readable, Writable } from "node:stream";
import { AcpFrameReader, serializeFrame, type AcpFraming } from "./framing.js";
import {
  ACP_PROTOCOL_VERSION,
  type AcpBackend,
  type AcpInitializeParams,
  type AcpPromptParams,
  type AcpSessionNewParams,
  type AcpSessionUpdate,
  type JsonRpcNotification,
  type JsonRpcRequest,
  type JsonRpcResponse
} from "./protocol.js";

/**
 * ACP agent endpoint over newline-delimited JSON-RPC on stdio.
 *
 * It owns framing and dispatch only; sessions, prompts and cancellation live in the backend. That
 * keeps the protocol testable with in-memory streams and keeps CodeWeaver policy/audit out of the
 * transport.
 */
export class AcpAgentServer {
  private readonly reader: AcpFrameReader;
  private started = false;
  private nextRequestId = 1000;
  private readonly pending = new Map<number, {
    resolve: (value: unknown) => void;
    reject: (error: Error) => void;
  }>();

  constructor(
    private readonly backend: AcpBackend,
    private readonly streams: { input: Readable; output: Writable },
    options: { framing?: "auto" | AcpFraming } = {}
  ) {
    this.reader = new AcpFrameReader(options.framing ?? "auto");
  }

  start(): void {
    if (this.started) return;
    this.started = true;
    // Keep raw bytes: Content-Length counts bytes, and decoding chunks would split multibyte characters.
    this.streams.input.on("data", (chunk: Buffer) => this.consume(chunk));
  }

  stop(): void {
    this.streams.input.removeAllListeners("data");
    for (const [, pending] of this.pending) {
      pending.reject(new Error("ACP server stopped before the client answered"));
    }
    this.pending.clear();
  }

  /**
   * Agent → client request (ACP permission prompts). Resolves with the client's `result`; rejects when
   * the client answers with an error or never answers before the server stops.
   */
  request(method: string, params: unknown, timeoutMs = 300_000): Promise<unknown> {
    this.nextRequestId += 1;
    const id = this.nextRequestId;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`ACP client did not answer ${method} within ${timeoutMs}ms`));
      }, timeoutMs);
      timer.unref?.();

      this.pending.set(id, {
        resolve: value => { clearTimeout(timer); resolve(value); },
        reject: error => { clearTimeout(timer); reject(error); }
      });
      this.send({ jsonrpc: "2.0", id, method, params });
    });
  }

  /** Writes one JSON-RPC frame in the framing the client negotiated (ndjson by default). */
  send(message: JsonRpcResponse | JsonRpcNotification | JsonRpcRequest): void {
    this.streams.output.write(serializeFrame(message, this.reader.framing ?? "ndjson"));
  }

  notifySessionUpdate(sessionId: string, update: AcpSessionUpdate): void {
    this.send({ jsonrpc: "2.0", method: "session/update", params: { sessionId, update } });
  }

  private consume(chunk: string | Buffer): void {
    for (const frame of this.reader.push(chunk)) {
      if (frame.trim()) void this.dispatch(frame);
    }
  }

  private async dispatch(line: string): Promise<void> {
    let message: JsonRpcRequest | JsonRpcNotification | JsonRpcResponse;
    try {
      message = JSON.parse(line) as JsonRpcRequest | JsonRpcNotification | JsonRpcResponse;
    } catch {
      this.send({ jsonrpc: "2.0", id: null, error: { code: -32700, message: "Parse error" } });
      return;
    }

    // A response to one of our own requests: id present, no method.
    if ("id" in message && message.id !== undefined && !("method" in message)) {
      const pending = this.pending.get(Number(message.id));
      if (pending) {
        this.pending.delete(Number(message.id));
        const error = (message as JsonRpcResponse).error;
        if (error) pending.reject(new Error(error.message));
        else pending.resolve((message as JsonRpcResponse).result);
      }
      return;
    }

    const hasId = "id" in message && message.id !== undefined;
    if (!hasId) {
      const notification = message as JsonRpcNotification;
      // Notifications must never receive a response, but cancellation still has to reach the backend.
      if (notification.method === "session/cancel") {
        const params = notification.params as { sessionId?: string } | undefined;
        if (params?.sessionId) await this.backend.cancel(params.sessionId).catch(() => undefined);
      }
      return;
    }

    const request = message as JsonRpcRequest;
    try {
      const result = await this.handle(request);
      this.send({ jsonrpc: "2.0", id: request.id, result });
    } catch (error) {
      this.send({
        jsonrpc: "2.0",
        id: request.id,
        error: {
          code: -32000,
          message: error instanceof Error ? error.message : String(error)
        }
      });
    }
  }

  private async handle(request: JsonRpcRequest): Promise<unknown> {
    switch (request.method) {
      case "initialize": {
        const params = (request.params ?? {}) as AcpInitializeParams;
        return {
          protocolVersion: params.protocolVersion ?? ACP_PROTOCOL_VERSION,
          agentCapabilities: {
            promptCapabilities: { image: false, audio: false, embeddedContext: false },
            mcpCapabilities: { http: false, sse: false }
          },
          agentInfo: { name: "codeweaver", version: "0.1.0" }
        };
      }
      case "session/new": {
        const params = (request.params ?? {}) as AcpSessionNewParams;
        if (!params.cwd || typeof params.cwd !== "string") {
          throw new Error("session/new requires a cwd");
        }
        return this.backend.createSession(params);
      }
      case "session/prompt": {
        const params = (request.params ?? {}) as AcpPromptParams;
        const text = extractPromptText(params);
        if (!params.sessionId) throw new Error("session/prompt requires a sessionId");
        const stopReason = await this.backend.prompt(params.sessionId, text, update =>
          this.notifySessionUpdate(params.sessionId, update)
        );
        return { stopReason };
      }
      default:
        throw new Error(`Unsupported ACP method: ${request.method}`);
    }
  }
}

export function extractPromptText(params: AcpPromptParams): string {
  if (!Array.isArray(params.prompt)) return "";
  return params.prompt
    .filter(block => block?.type === "text" && typeof block.text === "string")
    .map(block => block.text)
    .join("\n");
}
