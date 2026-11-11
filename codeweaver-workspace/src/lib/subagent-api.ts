import { ApiError } from "./api";
import { normalizeBaseUrl, useConnection } from "./settings";
import type { AgentExecution, ExecutionDetail } from "./types";

/**
 * Client for the subagent delegation endpoints.
 *
 * The server owns the lineage: a parent execution knows its children, and every execution knows the
 * chain it belongs to. The console therefore never infers a parent/child relation from event order
 * — it asks, and shows what the server answered.
 */

export type WorktreeMode = "isolated" | "shared";

/** The child execution the server created, with the lineage it recorded for it. */
export interface SpawnedSubagent {
  execution: AgentExecution;
  worktreeMode: WorktreeMode;
  workingDirectory: string;
  /** The root of the task tree this child belongs to. */
  rootExecutionId: string;
  /** The chain from the child up to the root, child first. */
  lineage: string[];
}

/** The children of an execution and the lineage it belongs to. */
export interface SubagentTree {
  children: string[];
  /** The chain from this execution up to the root, this execution first. */
  lineage: string[];
  rootExecutionId: string;
}

/** A child execution request. The parent decides the lineage; the caller supplies the work. */
export interface SpawnSubagentInput {
  projectId: string;
  taskId: string;
  runId: string;
  prompt: string;
  contextSnapshotId: string;
  policySnapshotId: string;
  worktreeId: string;
  workingDirectory?: string;
  allowedTools?: string[];
  timeoutSeconds?: number;
  correlationId: string;
  /** "isolated" (the default) gives the child its own worktree; "shared" reuses the parent's. */
  worktree?: WorktreeMode;
  /** Required for "isolated": the commit the child worktree is created at. */
  baseSha?: string;
  label?: string;
}

interface RequestOptions {
  method?: "GET" | "POST";
  body?: unknown;
}

function authHeaders(): Record<string, string> {
  const token = useConnection.getState().token;
  return token ? { Authorization: `Bearer ${token}` } : {};
}

async function request<T>(path: string, options: RequestOptions = {}): Promise<T> {
  const base = normalizeBaseUrl(useConnection.getState().baseUrl);
  let response: Response;
  try {
    response = await fetch(`${base}${path}`, {
      method: options.method ?? "GET",
      headers: {
        Accept: "application/json",
        ...(options.body === undefined ? {} : { "Content-Type": "application/json" }),
        ...authHeaders()
      },
      ...(options.body === undefined ? {} : { body: JSON.stringify(options.body) })
    });
  } catch (error) {
    // A transport failure is not a server verdict; say so instead of pretending the server said 0.
    throw new ApiError(
      0,
      "network_error",
      `无法连接后端 ${base}：${error instanceof Error ? error.message : String(error)}`
    );
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

export const subagentApi = {
  /** The children of an execution, plus the lineage it belongs to. */
  listSubagents: (executionId: string) =>
    request<SubagentTree>(`/api/v1/executions/${encodeURIComponent(executionId)}/subagents`),

  /** Delegates a child execution from a parent execution. */
  spawnSubagent: (executionId: string, input: SpawnSubagentInput) =>
    request<SpawnedSubagent>(`/api/v1/executions/${encodeURIComponent(executionId)}/subagents`, {
      method: "POST",
      body: input
    }),

  /** The status and the runtime of one execution, for the child list. */
  getExecution: (executionId: string) =>
    request<ExecutionDetail>(`/api/v1/executions/${encodeURIComponent(executionId)}`)
};
