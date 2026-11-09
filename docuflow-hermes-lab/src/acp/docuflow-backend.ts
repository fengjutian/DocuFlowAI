import { randomUUID } from "node:crypto";
import type { ExecutionCoordinator } from "../orchestrator/execution-coordinator.js";
import type { AgentRuntimeEvent, AgentTaskInput } from "../types/adapter.js";
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

export interface DocuFlowAcpBackendOptions {
  coordinator: ExecutionCoordinator;
  defaults: AcpBackendDefaults;
  idFactory?: () => string;
  runIdFactory?: () => string;
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

  return {
    async createSession(params: AcpSessionNewParams) {
      const sessionId = idFactory();
      sessions.set(sessionId, { sessionId, cwd: params.cwd, executions: [] });
      return { sessionId };
    },

    async prompt(sessionId, text, emit) {
      const session = sessions.get(sessionId);
      if (!session) throw new Error(`Unknown ACP session ${sessionId}`);

      const taskInput: AgentTaskInput = {
        projectId: options.defaults.projectId,
        taskId: `acp-${sessionId}`,
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

      const execution = await options.coordinator.start(taskInput);
      session.executions.push(execution.executionId);
      session.current = execution.executionId;

      let settle: (reason: AcpStopReason) => void = () => undefined;
      const terminal = new Promise<AcpStopReason>(resolve => { settle = resolve; });

      const unsubscribe = await options.coordinator.getAdapter().subscribe(execution.executionId, event => {
        const update = toSessionUpdate(event);
        if (update) emit(update);
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
