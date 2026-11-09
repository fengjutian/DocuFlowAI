export interface JsonRpcRequest {
  jsonrpc: "2.0";
  id: string | number;
  method: string;
  params?: unknown;
}

export interface JsonRpcResponse<T = unknown> {
  jsonrpc: "2.0";
  id: string | number;
  result?: T;
  error?: { code: number; message: string; data?: unknown };
}

/**
 * Protocol-neutral JSON-RPC request tracker.
 * A real stdio/WebSocket transport must be added and validated against the pinned Hermes build.
 */
export class JsonRpcPendingRequests {
  private readonly pending = new Map<string | number, {
    resolve: (value: unknown) => void;
    reject: (reason: Error) => void;
  }>();

  register<T>(id: string | number): Promise<T> {
    if (this.pending.has(id)) throw new Error(`Duplicate JSON-RPC request id: ${id}`);
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, { resolve: value => resolve(value as T), reject });
    });
  }

  settle(message: JsonRpcResponse): boolean {
    const entry = this.pending.get(message.id);
    if (!entry) return false;
    this.pending.delete(message.id);
    if (message.error) {
      entry.reject(new Error(`JSON-RPC ${message.error.code}: ${message.error.message}`));
    } else {
      entry.resolve(message.result);
    }
    return true;
  }

  rejectAll(reason: Error): void {
    for (const entry of this.pending.values()) entry.reject(reason);
    this.pending.clear();
  }

  get size(): number { return this.pending.size; }
}
