/**
 * Agent Client Protocol (ACP) types, limited to what DocuFlow's agent endpoint needs.
 *
 * Transport is JSON-RPC 2.0 over stdio, one JSON object per line (newline-delimited). Zed spawns the
 * command from `agent_servers` and speaks this protocol; DocuFlow is the agent side and Hermes sits
 * behind our own adapter, so ACP never becomes the task-scheduling protocol (design 4.4).
 */
export interface AcpInitializeParams {
  protocolVersion: number;
  clientCapabilities?: Record<string, unknown>;
}

export interface AcpSessionNewParams {
  cwd: string;
  mcpServers?: unknown[];
}

export interface AcpPromptParams {
  sessionId: string;
  prompt: Array<{ type: "text"; text: string }>;
}

export type AcpStopReason = "end_turn" | "max_tokens" | "max_turn_requests" | "refusal" | "cancelled";

export interface AcpSessionUpdate {
  sessionUpdate: string;
  [key: string]: unknown;
}

export interface AcpBackend {
  createSession(params: AcpSessionNewParams): Promise<{ sessionId: string }>;
  /** Streams updates through `emit` and resolves with why the turn ended. */
  prompt(
    sessionId: string,
    text: string,
    emit: (update: AcpSessionUpdate) => void
  ): Promise<AcpStopReason>;
  cancel(sessionId: string): Promise<void>;
}

export interface JsonRpcRequest {
  jsonrpc: "2.0";
  id: number | string;
  method: string;
  params?: unknown;
}

export interface JsonRpcNotification {
  jsonrpc: "2.0";
  method: string;
  params?: unknown;
}

export interface JsonRpcResponse {
  jsonrpc: "2.0";
  id: number | string | null;
  result?: unknown;
  error?: { code: number; message: string; data?: unknown };
}

export const ACP_PROTOCOL_VERSION = 1;
