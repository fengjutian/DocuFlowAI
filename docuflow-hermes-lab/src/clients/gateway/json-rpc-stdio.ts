import { spawn, type ChildProcessWithoutNullStreams, type SpawnOptionsWithoutStdio } from "node:child_process";
import { createInterface, type Interface as ReadLineInterface } from "node:readline";
import {
  JsonRpcPendingRequests,
  type JsonRpcNotification,
  type JsonRpcRequest,
  type JsonRpcResponse
} from "./json-rpc.js";

export interface JsonRpcStdioClientOptions {
  command: string;
  args?: string[];
  cwd?: string;
  env?: NodeJS.ProcessEnv;
  requestTimeoutMs?: number;
  framing?: "ndjson" | "content-length";
  maxMessageBytes?: number;
  onNotification?: (notification: JsonRpcNotification) => void;
  onRequest?: (request: JsonRpcRequest) => unknown | Promise<unknown>;
  onStderr?: (chunk: string) => void;
}

export interface JsonRpcRequestOptions {
  timeoutMs?: number;
  signal?: AbortSignal;
}

export class JsonRpcStdioClient {
  private readonly pending = new JsonRpcPendingRequests();
  private child: ChildProcessWithoutNullStreams | undefined;
  private lines: ReadLineInterface | undefined;
  private nextId = 1;
  private stopping = false;
  private frameBuffer = Buffer.alloc(0);

  constructor(private readonly options: JsonRpcStdioClientOptions) {}

  start(): void {
    if (this.child) throw new Error("JSON-RPC stdio client is already started");
    this.stopping = false;
    const spawnOptions: SpawnOptionsWithoutStdio = {
      stdio: "pipe",
      ...(this.options.cwd ? { cwd: this.options.cwd } : {}),
      ...(this.options.env ? { env: this.options.env } : {})
    };
    const child = spawn(this.options.command, this.options.args ?? [], spawnOptions);
    this.child = child;
    if ((this.options.framing ?? "ndjson") === "ndjson") {
      this.lines = createInterface({ input: child.stdout, crlfDelay: Infinity });
      this.lines.on("line", line => this.handleMessage(line));
    } else {
      child.stdout.on("data", chunk => this.handleContentLengthData(Buffer.from(chunk)));
    }
    child.stderr.setEncoding("utf8");
    child.stderr.on("data", chunk => this.options.onStderr?.(String(chunk)));
    child.once("error", error => this.handleTermination(error));
    child.once("exit", (code, signal) => {
      if (this.stopping) return;
      const detail = signal ? `signal ${signal}` : `code ${code ?? "unknown"}`;
      this.handleTermination(new Error(`JSON-RPC process exited with ${detail}`));
    });
  }

  async request<T>(
    method: string,
    params?: unknown,
    timeoutOrOptions: number | JsonRpcRequestOptions = this.options.requestTimeoutMs ?? 30_000
  ): Promise<T> {
    const child = this.requireRunningChild();
    const requestOptions = typeof timeoutOrOptions === "number"
      ? { timeoutMs: timeoutOrOptions }
      : timeoutOrOptions;
    const timeoutMs = requestOptions.timeoutMs ?? this.options.requestTimeoutMs ?? 30_000;
    requestOptions.signal?.throwIfAborted();
    const id = this.nextId++;
    const response = this.pending.register<T>(id);
    // Cancellation or process exit can reject while stdin is still flushing.
    // Observe the internal promise immediately; the awaited promise below still propagates it.
    void response.catch(() => {});
    const request: JsonRpcRequest = {
      jsonrpc: "2.0",
      id,
      method,
      ...(params !== undefined ? { params } : {})
    };
    const timer = setTimeout(() => {
      this.pending.reject(id, new Error(`JSON-RPC request timed out after ${timeoutMs}ms: ${method}`));
    }, timeoutMs);
    timer.unref();
    const abort = () => {
      const reason = requestOptions.signal?.reason;
      this.pending.reject(id, reason instanceof Error ? reason : new Error(`JSON-RPC request aborted: ${method}`));
    };
    requestOptions.signal?.addEventListener("abort", abort, { once: true });
    try {
      await this.writeMessage(child, JSON.stringify(request));
      return await response;
    } catch (error) {
      this.pending.reject(id, error instanceof Error ? error : new Error(String(error)));
      throw error;
    } finally {
      clearTimeout(timer);
      requestOptions.signal?.removeEventListener("abort", abort);
    }
  }

  notify(method: string, params?: unknown): Promise<void> {
    const notification: JsonRpcNotification = {
      jsonrpc: "2.0",
      method,
      ...(params !== undefined ? { params } : {})
    };
    return this.writeMessage(this.requireRunningChild(), JSON.stringify(notification));
  }

  async stop(): Promise<void> {
    const child = this.child;
    if (!child) return;
    this.stopping = true;
    this.lines?.close();
    this.lines = undefined;
    this.child = undefined;
    this.frameBuffer = Buffer.alloc(0);
    this.pending.rejectAll(new Error("JSON-RPC stdio client stopped"));
    if (child.exitCode !== null || child.signalCode !== null) return;
    await new Promise<void>(resolve => {
      child.once("exit", () => resolve());
      if (!child.kill()) resolve();
    });
  }

  private handleMessage(text: string): void {
    if (!text.trim()) return;
    let message: unknown;
    try {
      message = JSON.parse(text);
    } catch {
      this.handleTermination(new Error("JSON-RPC process emitted invalid JSON"));
      return;
    }
    if (!isRecord(message) || message.jsonrpc !== "2.0") {
      this.handleTermination(new Error("JSON-RPC process emitted an invalid message"));
      return;
    }
    if (typeof message.method === "string") {
      if (!("id" in message)) {
        this.options.onNotification?.(message as unknown as JsonRpcNotification);
        return;
      }
      if (typeof message.id === "string" || typeof message.id === "number") {
        void this.handleServerRequest(message as unknown as JsonRpcRequest);
        return;
      }
    }
    if ((typeof message.id === "string" || typeof message.id === "number") &&
        ("result" in message || "error" in message)) {
      this.pending.settle(message as unknown as JsonRpcResponse);
      return;
    }
    this.handleTermination(new Error("JSON-RPC process emitted an unsupported message"));
  }

  private requireRunningChild(): ChildProcessWithoutNullStreams {
    if (!this.child || this.child.exitCode !== null || this.child.signalCode !== null) {
      throw new Error("JSON-RPC stdio client is not running");
    }
    return this.child;
  }

  private async handleServerRequest(request: JsonRpcRequest): Promise<void> {
    const child = this.child;
    if (!child) return;
    try {
      if (!this.options.onRequest) {
        await this.writeMessage(child, JSON.stringify({
          jsonrpc: "2.0",
          id: request.id,
          error: { code: -32601, message: `No client handler for server request: ${request.method}` }
        } satisfies JsonRpcResponse));
        return;
      }
      const result = await this.options.onRequest(request);
      await this.writeMessage(child, JSON.stringify({ jsonrpc: "2.0", id: request.id, result } satisfies JsonRpcResponse));
    } catch (error) {
      if (!this.child) return;
      await this.writeMessage(child, JSON.stringify({
        jsonrpc: "2.0",
        id: request.id,
        error: { code: -32000, message: error instanceof Error ? error.message : String(error) }
      } satisfies JsonRpcResponse));
    }
  }

  private handleContentLengthData(chunk: Buffer): void {
    this.frameBuffer = Buffer.concat([this.frameBuffer, chunk]);
    const maxMessageBytes = this.options.maxMessageBytes ?? 10 * 1024 * 1024;
    while (this.frameBuffer.length > 0) {
      const headerEnd = this.frameBuffer.indexOf("\r\n\r\n");
      if (headerEnd < 0) {
        if (this.frameBuffer.length > 8_192) this.handleTermination(new Error("JSON-RPC frame headers are too large"));
        return;
      }
      const headers = this.frameBuffer.subarray(0, headerEnd).toString("ascii");
      const match = /^Content-Length:\s*(\d+)\s*$/im.exec(headers);
      if (!match?.[1]) {
        this.handleTermination(new Error("JSON-RPC frame is missing Content-Length"));
        return;
      }
      const contentLength = Number(match[1]);
      if (!Number.isSafeInteger(contentLength) || contentLength < 0 || contentLength > maxMessageBytes) {
        this.handleTermination(new Error(`JSON-RPC frame exceeds the ${maxMessageBytes}-byte limit`));
        return;
      }
      const bodyStart = headerEnd + 4;
      const frameEnd = bodyStart + contentLength;
      if (this.frameBuffer.length < frameEnd) return;
      const body = this.frameBuffer.subarray(bodyStart, frameEnd).toString("utf8");
      this.frameBuffer = this.frameBuffer.subarray(frameEnd);
      this.handleMessage(body);
      if (!this.child) return;
    }
  }

  private writeMessage(child: ChildProcessWithoutNullStreams, message: string): Promise<void> {
    if ((this.options.framing ?? "ndjson") === "content-length") {
      return writeChunk(child, `Content-Length: ${Buffer.byteLength(message, "utf8")}\r\n\r\n${message}`);
    }
    return writeChunk(child, `${message}\n`);
  }

  private handleTermination(error: Error): void {
    const child = this.child;
    this.lines?.close();
    this.lines = undefined;
    this.child = undefined;
    this.frameBuffer = Buffer.alloc(0);
    this.pending.rejectAll(error);
    if (child && child.exitCode === null && child.signalCode === null) child.kill();
  }
}

function writeChunk(child: ChildProcessWithoutNullStreams, chunk: string): Promise<void> {
  return new Promise<void>((resolve, reject) => {
    child.stdin.write(chunk, "utf8", error => error ? reject(error) : resolve());
  });
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}
