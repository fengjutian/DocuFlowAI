import type { Readable, Writable } from "node:stream";
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
 * keeps the protocol testable with in-memory streams and keeps DocuFlow policy/audit out of the
 * transport.
 */
export class AcpAgentServer {
  private buffer = "";
  private started = false;

  constructor(
    private readonly backend: AcpBackend,
    private readonly streams: { input: Readable; output: Writable }
  ) {}

  start(): void {
    if (this.started) return;
    this.started = true;
    this.streams.input.setEncoding?.("utf8");
    this.streams.input.on("data", (chunk: string) => this.consume(chunk));
  }

  stop(): void {
    this.streams.input.removeAllListeners("data");
  }

  /** Writes one JSON-RPC frame; exposed for tests and for backend-driven notifications. */
  send(message: JsonRpcResponse | JsonRpcNotification): void {
    this.streams.output.write(`${JSON.stringify(message)}\n`);
  }

  notifySessionUpdate(sessionId: string, update: AcpSessionUpdate): void {
    this.send({ jsonrpc: "2.0", method: "session/update", params: { sessionId, update } });
  }

  private consume(chunk: string): void {
    this.buffer += chunk;
    let newline = this.buffer.indexOf("\n");
    while (newline >= 0) {
      const line = this.buffer.slice(0, newline).trim();
      this.buffer = this.buffer.slice(newline + 1);
      if (line) void this.dispatch(line);
      newline = this.buffer.indexOf("\n");
    }
  }

  private async dispatch(line: string): Promise<void> {
    let message: JsonRpcRequest | JsonRpcNotification;
    try {
      message = JSON.parse(line) as JsonRpcRequest | JsonRpcNotification;
    } catch {
      this.send({ jsonrpc: "2.0", id: null, error: { code: -32700, message: "Parse error" } });
      return;
    }

    const hasId = "id" in message && message.id !== undefined;
    if (!hasId) {
      // Notifications must never receive a response, but cancellation still has to reach the backend.
      if (message.method === "session/cancel") {
        const params = message.params as { sessionId?: string } | undefined;
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
          agentInfo: { name: "docuflow", version: "0.1.0" }
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
