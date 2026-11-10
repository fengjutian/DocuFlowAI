import { useConnection, normalizeBaseUrl } from "./settings";
import type {
  AgentExecution,
  CommitTrace,
  CompletionGate,
  DocuFlowEvent,
  ExecutionCheckpoint,
  ExecutionDetail,
  ResumeResponse,
  RetryResponse,
  RuntimeEntry,
  RuntimeHealth,
  RuntimePermissionRequest,
  SkillBinding,
  SkillManifest,
  StartTaskInput,
  StartTaskResponse,
  UsageRecord,
  UsageSummary
} from "./types";

/**
 * Client for the DocuFlow lab server.
 *
 * Everything the UI knows about the backend goes through here. The server is the only source of truth
 * for runtime state: no page keeps its own copy of "what is running" beyond what the API returned.
 */
export class ApiError extends Error {
  constructor(
    public readonly status: number,
    public readonly code: string,
    message: string
  ) {
    super(message);
    this.name = "ApiError";
  }
}

interface RequestOptions {
  method?: "GET" | "POST" | "PATCH" | "DELETE";
  body?: unknown;
  signal?: AbortSignal;
}

function authHeaders(): Record<string, string> {
  const token = useConnection.getState().token;
  return token ? { Authorization: `Bearer ${token}` } : {};
}

async function request<T>(path: string, options: RequestOptions = {}): Promise<T> {
  const base = normalizeBaseUrl(useConnection.getState().baseUrl);
  const url = `${base}${path}`;
  let response: Response;
  try {
    response = await fetch(url, {
      method: options.method ?? "GET",
      headers: {
        Accept: "application/json",
        ...(options.body === undefined ? {} : { "Content-Type": "application/json" }),
        ...authHeaders()
      },
      ...(options.body === undefined ? {} : { body: JSON.stringify(options.body) }),
      ...(options.signal ? { signal: options.signal } : {})
    });
  } catch (error) {
    // A transport failure is not a server verdict; say so instead of pretending the server said 0.
    throw new ApiError(0, "network_error", `无法连接后端 ${base}：${error instanceof Error ? error.message : String(error)}`);
  }

  const text = await response.text();
  let payload: unknown = undefined;
  if (text !== "") {
    try {
      payload = JSON.parse(text);
    } catch {
      payload = undefined;
    }
  }

  if (!response.ok) {
    const record = (payload ?? {}) as { error?: string; message?: string };
    throw new ApiError(
      response.status,
      record.error ?? "http_error",
      record.message ?? `请求失败：HTTP ${response.status}`
    );
  }
  return payload as T;
}

export const api = {
  listRuntimes: () => request<{ runtimes: RuntimeEntry[] }>("/api/v1/runtimes"),

  healthCheck: (runtimeId: string) =>
    request<RuntimeHealth>(`/api/v1/runtimes/${encodeURIComponent(runtimeId)}/health-check`, {
      method: "POST"
    }),

  startTask: (taskId: string, input: StartTaskInput) =>
    request<StartTaskResponse>(`/api/v1/tasks/${encodeURIComponent(taskId)}/executions`, {
      method: "POST",
      body: input
    }),

  getExecution: (executionId: string) =>
    request<ExecutionDetail>(`/api/v1/executions/${encodeURIComponent(executionId)}`),

  getExecutionEvents: (executionId: string, since = 0) =>
    request<{ events: DocuFlowEvent[] }>(
      `/api/v1/executions/${encodeURIComponent(executionId)}/events?since=${since}`
    ),

  listApprovals: (executionId: string) =>
    request<{ approvals: RuntimePermissionRequest[] }>(
      `/api/v1/executions/${encodeURIComponent(executionId)}/approvals`
    ),

  decideApproval: (
    executionId: string,
    requestId: string,
    decision: "approve" | "deny",
    reason?: string
  ) =>
    request<{ executionId: string; requestId: string; decision: string }>(
      `/api/v1/executions/${encodeURIComponent(executionId)}/approvals/${encodeURIComponent(requestId)}`,
      { method: "POST", body: { decision, ...(reason ? { reason } : {}) } }
    ),

  cancelExecution: (executionId: string) =>
    request<{ executionId: string; status: string }>(
      `/api/v1/executions/${encodeURIComponent(executionId)}/cancel`,
      { method: "POST" }
    ),

  retryExecution: (executionId: string) =>
    request<RetryResponse>(`/api/v1/executions/${encodeURIComponent(executionId)}/retry`, {
      method: "POST"
    }),

  resumeExecution: (executionId: string) =>
    request<ResumeResponse>(`/api/v1/executions/${encodeURIComponent(executionId)}/resume`, {
      method: "POST"
    }),

  getExecutionUsage: (executionId: string) =>
    request<{ usage: UsageRecord }>(`/api/v1/executions/${encodeURIComponent(executionId)}/usage`),

  getUsage: () => request<{ summary: UsageSummary; records: UsageRecord[] }>("/api/v1/usage"),

  listSkills: () => request<{ skills: SkillManifest[] }>("/api/v1/skills"),

  registerSkill: (manifest: SkillManifest) =>
    request<{ registered: boolean }>("/api/v1/skills", { method: "POST", body: manifest }),

  listExecutionSkills: (executionId: string) =>
    request<{ bindings: SkillBinding[] }>(`/api/v1/executions/${encodeURIComponent(executionId)}/skills`),

  traceCommit: (sha: string) =>
    request<CommitTrace>(`/api/v1/trace/commits/${encodeURIComponent(sha)}`)
};

export type { AgentExecution, CompletionGate, ExecutionCheckpoint };
