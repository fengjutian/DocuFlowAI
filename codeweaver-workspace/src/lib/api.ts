import { useConnection, normalizeBaseUrl } from "./settings";
import type {
  AgentExecution,
  AuditTimeline,
  CommitTrace,
  CompletionGate,
  CodeWeaverEvent,
  ExecutionCheckpoint,
  ExecutionDetail,
  ExecutionSummaryRecord,
  ExecutionTrace,
  ResumeResponse,
  RetryResponse,
  RuntimeEntry,
  RuntimeConfig,
  RuntimeConfigAuditEntry,
  RuntimeHealth,
  RuntimePermissionRequest,
  RuntimeScoreReport,
  SkillBinding,
  SkillManifest,
  StartTaskInput,
  StartTaskResponse,
  TaskTrace,
  UsageRecord,
  UsageSummary
} from "./types";

/**
 * Client for the CodeWeaver lab server.
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

  /** The capability score of every runtime, computed from what the journal recorded. */
  runtimeScores: () => request<RuntimeScoreReport>("/api/v1/runtimes/scores"),

  healthCheck: (runtimeId: string) =>
    request<RuntimeHealth>(`/api/v1/runtimes/${encodeURIComponent(runtimeId)}/health-check`, {
      method: "POST"
    }),

  setRuntimeEnabled: (runtimeId: string, enabled: boolean) =>
    request<{ id: string; enabled: boolean; runningExecutions: string[] }>(
      `/api/v1/runtimes/${encodeURIComponent(runtimeId)}/${enabled ? "enable" : "disable"}`,
      { method: "POST", body: { actor: "console" } }
    ),

  /**
   * The persisted runtime configuration.
   *
   * The console shows what the server has stored; it never keeps its own copy of what it thinks the
   * configuration is.
   */
  runtimeConfig: () => request<RuntimeConfig>("/api/v1/runtimes/config"),

  /** The recorded configuration changes, newest first. */
  runtimeConfigHistory: (limit = 50) =>
    request<{ history: RuntimeConfigAuditEntry[] }>(`/api/v1/runtimes/config/history?limit=${limit}`),

  /** Changes the priority, the tags or the enabled switch of one runtime. */
  updateRuntime: (
    runtimeId: string,
    patch: { enabled?: boolean; priority?: number; tags?: string[] }
  ) =>
    request<{ id: string; enabled: boolean; priority: number; tags: string[] }>(
      `/api/v1/runtimes/${encodeURIComponent(runtimeId)}`,
      { method: "PATCH", body: { ...patch, actor: "console" } }
    ),

  /** Names the runtime that unnamed work goes to; `null` clears it. */
  setDefaultRuntime: (runtimeId: string | null) =>
    request<{ defaultRuntimeId: string | null }>("/api/v1/runtimes/default", {
      method: "POST",
      body: { runtimeId, actor: "console" }
    }),

  startTask: (taskId: string, input: StartTaskInput) =>
    request<StartTaskResponse>(`/api/v1/tasks/${encodeURIComponent(taskId)}/executions`, {
      method: "POST",
      body: input
    }),

  getExecution: (executionId: string) =>
    request<ExecutionDetail>(`/api/v1/executions/${encodeURIComponent(executionId)}`),

  /**
   * Every execution the journal holds, newest first.
   *
   * This is the only source for the executions a disconnected client never saw: the live stream
   * only carries what happens while it is listening.
   */
  listExecutions: () =>
    request<{ executions: ExecutionSummaryRecord[] }>("/api/v1/executions"),

  getExecutionEvents: (executionId: string, since = 0) =>
    request<{ events: CodeWeaverEvent[] }>(
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

  /**
   * Continues a finished turn on the session the execution already used.
   *
   * The runtime keeps the transcript, so the answer can depend on the earlier turn. A runtime that
   * cannot continue refuses this (501) rather than starting a fresh run that only looks like a
   * continuation.
   */
  followUp: (executionId: string, prompt: string) =>
    request<{ executionId: string; status: string }>(
      `/api/v1/executions/${encodeURIComponent(executionId)}/follow-up`,
      { method: "POST", body: { prompt } }
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

  /**
   * The trace of one execution: task, run, tool calls, change, commit and test.
   *
   * Everything in it is read from the server's own records — the console never rebuilds the chain
   * from the event stream, because a chain assembled by the client is not a chain.
   */
  traceExecution: (executionId: string) =>
    request<ExecutionTrace>(`/api/v1/executions/${encodeURIComponent(executionId)}/trace`),

  /** Every execution a task produced. */
  traceTask: (taskId: string) =>
    request<TaskTrace>(`/api/v1/tasks/${encodeURIComponent(taskId)}/trace`),

  traceCommit: (sha: string) =>
    request<CommitTrace>(`/api/v1/trace/commits/${encodeURIComponent(sha)}`),

  /**
   * The audit timeline: the recorded events of every execution, correlated and ordered.
   *
   * It is read from the server's journal — the same append-only record the live stream replays —
   * so the console never rebuilds the history from what its own socket happened to receive.
   */
  auditTimeline: (query: { taskId?: string; executionId?: string; category?: string } = {}) => {
    const params = new URLSearchParams();
    if (query.taskId) params.set("taskId", query.taskId);
    if (query.executionId) params.set("executionId", query.executionId);
    if (query.category) params.set("category", query.category);
    const suffix = params.toString();
    return request<AuditTimeline>(`/api/v1/audit/timeline${suffix ? `?${suffix}` : ""}`);
  }
};

export type { AgentExecution, CompletionGate, ExecutionCheckpoint };
