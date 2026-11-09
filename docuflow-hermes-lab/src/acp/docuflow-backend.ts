import { createHash, randomUUID } from "node:crypto";
import type { ExecutionCoordinator } from "../orchestrator/execution-coordinator.js";
import type { PolicySnapshot } from "../policy/permission-policy.js";
import type { TaskExecutionService } from "../tasks/task-execution-service.js";
import type { AgentRuntimeEvent, AgentExecution, AgentTaskInput } from "../types/adapter.js";
import type { AcpBackend, AcpSessionNewParams, AcpSessionUpdate, AcpStopReason } from "./protocol.js";

/**
 * Bridges ACP sessions to DocuFlow executions.
 *
 * One ACP session maps to a workspace and may start more than one execution (one per prompt), because
 * the Hermes HTTP adapter cannot continue a finished run. The execution ids are journaled like any
 * other run, so editor-driven work is audited exactly like platform-driven work.
 */
export interface AcpBackendDefaults {
  projectId: string;
  contextSnapshotId: string;
  policySnapshotId: string;
  allowedTools?: string[];
  timeoutSeconds?: number;
}

export interface AcpPermissionOption {
  optionId: string;
  name: string;
  kind?: "allow_once" | "allow_always" | "reject_once" | "reject_always";
}

export interface AcpPermissionRequest {
  sessionId: string;
  toolCallId: string;
  title: string;
  options: AcpPermissionOption[];
}

export type AcpPermissionOutcome = { optionId: string } | "cancelled";

export interface DocuFlowAcpBackendOptions {
  coordinator: ExecutionCoordinator;
  defaults: AcpBackendDefaults;
  idFactory?: () => string;
  runIdFactory?: () => string;
  /**
   * Optional snapshot-backed path. When present, prompts go through `TaskExecutionService` so the
   * editor run gets context/policy/prompt snapshots and audit entries like a platform run. Without it
   * the backend falls back to a bare coordinator start (journal only).
   */
  snapshots?: {
    taskService: TaskExecutionService;
    policyFor: (context: { cwd: string; taskId: string }) => PolicySnapshot | Promise<PolicySnapshot>;
    template?: { name: string; version: string; contentHash: string };
  };
  /**
   * Asked when Hermes raises a permission request. Without it the request stays pending in the adapter
   * and the run waits, which is the same fail-closed behavior as the API path.
   */
  requestPermission?: (request: AcpPermissionRequest) => Promise<AcpPermissionOutcome>;
  /** Non-fatal problems (snapshot capture skipped); the ACP session continues. */
  onWarning?: (message: string) => void;
}

interface AcpSession {
  sessionId: string;
  cwd: string;
  executions: string[];
  current?: string;
}

export function createDocuFlowAcpBackend(options: DocuFlowAcpBackendOptions): AcpBackend {
  const sessions = new Map<string, AcpSession>();
  const idFactory = options.idFactory ?? (() => `sess_${randomUUID()}`);
  const runIdFactory = options.runIdFactory ?? (() => `run_acp_${randomUUID()}`);
  const timeoutSeconds = options.defaults.timeoutSeconds ?? 300;

  async function startExecution(
    taskInput: AgentTaskInput,
    text: string,
    cwd: string,
    taskId: string,
    timeout: number
  ): Promise<AgentExecution> {
    const snapshotPath = options.snapshots;
    if (!snapshotPath) return options.coordinator.start(taskInput);

    try {
      const policy = await snapshotPath.policyFor({ cwd, taskId });
      const result = await snapshotPath.taskService.startTask({
        projectId: taskInput.projectId,
        taskId,
        prompt: text,
        worktreePath: cwd,
        documents: [],
        selectedFiles: [],
        architectureVersion: "acp",
        policy,
        template: snapshotPath.template ?? {
          name: "zed-acp",
          version: "1.0.0",
          contentHash: createHash("sha256").update("zed-acp").digest("hex")
        },
        modelConfig: {},
        correlationId: taskInput.correlationId,
        timeoutSeconds: timeout,
        allowedTools: taskInput.allowedTools
      });
      return result.execution;
    } catch (error) {
      // A cwd that is not a git repository cannot be snapshotted. Degrade with a warning rather than
      // blocking the editor session, but never pretend a snapshot was taken.
      if (!isContextCaptureFailure(error)) throw error;
      options.onWarning?.(
        `ACP snapshots skipped for ${cwd}: ${error instanceof Error ? error.message : String(error)}`
      );
      return options.coordinator.start(taskInput);
    }
  }

  async function answerPermission(
    executionId: string,
    sessionId: string,
    event: AgentRuntimeEvent
  ): Promise<void> {
    const ask = options.requestPermission;
    const adapter = options.coordinator.getAdapter();
    if (!ask || !adapter.respondToPermission) return; // fail closed: leave the request pending

    const requestId = permissionRequestId(event);
    try {
      const outcome = await ask({
        sessionId,
        toolCallId: String(event.payload["toolCallId"] ?? requestId),
        title: String(event.payload["tool"] ?? event.payload["command"] ?? "permission requested"),
        options: [
          { optionId: "allow", name: "Allow", kind: "allow_once" },
          { optionId: "reject", name: "Reject", kind: "reject_once" }
        ]
      });
      const decision = outcome !== "cancelled" && outcome.optionId === "allow" ? "approve" : "deny";
      await adapter.respondToPermission(executionId, requestId, { decision });
    } catch {
      // The editor closed or timed out: deny rather than leaving the runtime waiting forever.
      await adapter
        .respondToPermission(executionId, requestId, {
          decision: "deny",
          reason: "ACP client did not answer the permission request"
        })
        .catch(() => undefined);
    }
  }

  return {
    async createSession(params: AcpSessionNewParams) {
      const sessionId = idFactory();
      sessions.set(sessionId, { sessionId, cwd: params.cwd, executions: [] });
      return { sessionId };
    },

    async prompt(sessionId, text, emit) {
      const session = sessions.get(sessionId);
      if (!session) throw new Error(`Unknown ACP session ${sessionId}`);

      const taskId = `acp-${sessionId}`;
      const taskInput: AgentTaskInput = {
        projectId: options.defaults.projectId,
        taskId,
        runId: runIdFactory(),
        prompt: text,
        contextSnapshotId: options.defaults.contextSnapshotId,
        policySnapshotId: options.defaults.policySnapshotId,
        worktreeId: sessionId,
        workingDirectory: session.cwd,
        allowedTools: [...(options.defaults.allowedTools ?? [])],
        timeoutSeconds,
        correlationId: `acp-${sessionId}`
      };

      const execution = await startExecution(taskInput, text, session.cwd, taskId, timeoutSeconds);
      session.executions.push(execution.executionId);
      session.current = execution.executionId;

      let settle: (reason: AcpStopReason) => void = () => undefined;
      const terminal = new Promise<AcpStopReason>(resolve => { settle = resolve; });

      const unsubscribe = await options.coordinator.getAdapter().subscribe(execution.executionId, event => {
        const update = toSessionUpdate(event);
        if (update) emit(update);
        if (event.eventType === "permission.requested") {
          void answerPermission(execution.executionId, sessionId, event);
        }
        const reason = toStopReason(event);
        if (reason) settle(reason);
      });

      const timer = setTimeout(() => {
        settle("cancelled");
        void options.coordinator.getAdapter().cancel(execution.executionId).catch(() => undefined);
      }, timeoutSeconds * 1000);

      try {
        return await terminal;
      } finally {
        clearTimeout(timer);
        await unsubscribe().catch(() => undefined);
        delete session.current;
      }
    },

    async cancel(sessionId) {
      const session = sessions.get(sessionId);
      if (!session?.current) return;
      await options.coordinator.cancel(session.current);
    }
  };
}

export function toSessionUpdate(event: AgentRuntimeEvent): AcpSessionUpdate | undefined {
  switch (event.eventType) {
    case "message.delta": {
      const text = payloadText(event.payload);
      if (!text) return undefined;
      return {
        sessionUpdate: "agent_message_chunk",
        content: { type: "text", text }
      };
    }
    case "message.completed": {
      const text = payloadText(event.payload);
      return text
        ? { sessionUpdate: "agent_message_chunk", content: { type: "text", text } }
        : undefined;
    }
    case "tool.started":
      return {
        sessionUpdate: "tool_call",
        toolCallId: String(event.payload["toolCallId"] ?? event.payload["tool"] ?? event.eventId),
        title: String(event.payload["tool"] ?? "tool"),
        status: "in_progress"
      };
    case "tool.completed":
      return {
        sessionUpdate: "tool_call_update",
        toolCallId: String(event.payload["toolCallId"] ?? event.payload["tool"] ?? event.eventId),
        status: "completed"
      };
    case "tool.failed":
      return {
        sessionUpdate: "tool_call_update",
        toolCallId: String(event.payload["toolCallId"] ?? event.payload["tool"] ?? event.eventId),
        status: "failed"
      };
    default:
      return undefined;
  }
}

export function toStopReason(event: AgentRuntimeEvent): AcpStopReason | undefined {
  switch (event.eventType) {
    case "execution.completed": return "end_turn";
    case "execution.cancelled": return "cancelled";
    // ACP has no "error" stop reason; a failed run is reported as a refusal so the editor stops.
    case "execution.failed": return "refusal";
    default: return undefined;
  }
}

function payloadText(payload: Record<string, unknown>): string {
  for (const key of ["text", "delta", "content"]) {
    const value = payload[key];
    if (typeof value === "string" && value) return value;
  }
  return "";
}

/** Hermes may carry the approval id at a few keys; the adapter uses the same order. */
export function permissionRequestId(event: AgentRuntimeEvent): string {
  for (const key of ["request_id", "requestId", "approval_id", "id"]) {
    const value = event.payload[key];
    if (typeof value === "string" && value !== "") return value;
  }
  return event.externalEventId ?? event.eventId;
}

function isContextCaptureFailure(error: unknown): boolean {
  const message = error instanceof Error ? error.message : String(error);
  return /Cannot read context file|not a git repository|unknown revision|rev-parse|escapes the worktree/i.test(
    message
  );
}
