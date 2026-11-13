import { createHash, randomUUID } from "node:crypto";
import type { ExecutionCoordinator } from "../orchestrator/execution-coordinator.js";
import type { PolicySnapshot } from "../policy/permission-policy.js";
import type { TaskExecutionService } from "../tasks/task-execution-service.js";
import type { AgentRuntimeEvent, AgentExecution, AgentTaskInput } from "../types/adapter.js";
import type { AcpBackend, AcpSessionNewParams, AcpSessionUpdate, AcpStopReason } from "./protocol.js";

/**
 * Bridges ACP sessions to CodeWeaver executions.
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
  /**
   * The runtime an editor session runs on.
   *
   * The editor is a client of CodeWeaver, not a runtime of its own: without a name, the task would
   * land on whatever the server happens to default to, and the product's primary runtime (D-060)
   * would be unreachable from the editor. "auto" asks the scheduler for a healthy one; any other
   * id names one exactly.
   */
  runtimeId?: string;
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

export interface CodeWeaverAcpBackendOptions {
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
  /** Set while a prompt is in flight so a cancel can settle it immediately. */
  settle?: (reason: AcpStopReason) => void;
}

export function createCodeWeaverAcpBackend(options: CodeWeaverAcpBackendOptions): AcpBackend {
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
        allowedTools: taskInput.allowedTools,
        ...(options.defaults.runtimeId ? { runtimeId: options.defaults.runtimeId } : {})
      });
      return result.execution;
    } catch (error) {
      // A cwd that is not a git repository cannot be snapshotted. Degrade with a warning rather than
      // blocking the editor session, but never pretend a snapshot was taken.
      if (!isContextCaptureFailure(error)) throw error;
      const reason = error instanceof Error ? error.message : String(error);
      options.onWarning?.(`ACP snapshots skipped for ${cwd}: ${reason}`);
      const execution = await options.coordinator.start(taskInput);
      // The skip itself is evidence: an auditor must see that this run has no snapshot, and why.
      await options.coordinator
        .recordAuditFact(execution.executionId, "context.snapshot.skipped", { cwd, reason })
        .catch(() => undefined);
      return execution;
    }
  }

  async function answerPermission(
    executionId: string,
    sessionId: string,
    event: AgentRuntimeEvent
  ): Promise<void> {
    const ask = options.requestPermission;
    const adapter = options.coordinator.adapterFor(executionId);
    if (!ask || !adapter.respondToPermission) return; // fail closed: leave the request pending

    const requestId = permissionRequestId(event);
    try {
      const outcome = await ask({
        sessionId,
        toolCallId: String(event.payload["toolCallId"] ?? requestId),
        title: toolTitle(event, "permission requested"),
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
      session.settle = settle;

      const unsubscribe = await options.coordinator.adapterFor(execution.executionId).subscribe(execution.executionId, event => {
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
        void options.coordinator.adapterFor(execution.executionId).cancel(execution.executionId).catch(() => undefined);
      }, timeoutSeconds * 1000);

      try {
        return await terminal;
      } finally {
        clearTimeout(timer);
        await unsubscribe().catch(() => undefined);
        delete session.current;
        delete session.settle;
      }
    },

    async cancel(sessionId) {
      const session = sessions.get(sessionId);
      const executionId = session?.current;
      if (!session || !executionId) return;

      try {
        await options.coordinator.cancel(executionId);
      } catch (error) {
        // The runtime may not support remote cancellation. Stop listening locally and make that
        // explicit in the journal; the editor still gets a cancelled turn instead of a hang.
        await options.coordinator
          .recordAuditFact(executionId, "execution.cancel_unsupported", {
            reason: error instanceof Error ? error.message : String(error)
          })
          .catch(() => undefined);
        await options.coordinator.adapterFor(executionId).terminate(executionId).catch(() => undefined);
      } finally {
        session.settle?.("cancelled");
      }
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
    case "tool.started": {
      const title = toolTitle(event);
      const content = toolCallContent(event);
      return {
        sessionUpdate: "tool_call",
        toolCallId: String(event.payload["toolCallId"] ?? event.payload["tool"] ?? event.eventId),
        title,
        kind: toolKind(title),
        status: "in_progress",
        ...(content.length > 0 ? { content } : {})
      };
    }
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
    case "execution.failed": {
      // A run that dies without saying why looks like a run that never started. The reason is in
      // the event, and the ACP layer is the only thing that carries it across, so the editor does
      // not have to go looking for it.
      const reason = failureReason(event);
      return reason
        ? { sessionUpdate: "agent_message_chunk", content: { type: "text", text: reason } }
        : undefined;
    }
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

/**
 * What kind of tool this is, as the editor draws it.
 *
 * The kind is what the editor keys its behaviour on: a tool that proposes a
 * change is not a tool that reads a file, and the panel shows them
 * differently.
 */
export function toolKind(title: string): string {
  if (title === "request_diff" || title === "request_apply_diff") return "edit";
  if (title === "read_file") return "read";
  if (title === "evaluate_changes") return "check";
  return "other";
}

/**
 * The content a tool call carries, taken from the arguments the runtime sent.
 *
 * A tool that proposes a change passes the change as a unified diff. It
 * travels as the tool call's content so the editor can show the change and
 * let the user decide about it, instead of writing it to the worktree behind
 * their back.
 */
export function toolCallContent(event: AgentRuntimeEvent): Array<Record<string, unknown>> {
  const patch = event.payload["patch"];
  if (typeof patch !== "string" || patch.trim() === "") return [];
  const path = patchPath(patch);
  if (!path) return [];
  return [{ type: "diff", path, patch }];
}

/**
 * The file a unified diff is about.
 *
 * The path comes from the diff itself rather than from the tool's arguments:
 * the diff names the file it changes, and that is the file the editor has to
 * show the change in.
 */
export function patchPath(patch: string): string | undefined {
  for (const rawLine of patch.split(/\r?\n/)) {
    if (rawLine.startsWith("+++ ") && rawLine.slice(4).trim() !== "/dev/null") {
      const candidate = rawLine.slice(4).split("\t")[0]!.trim();
      return candidate.replace(/^[ab]\//, "");
    }
    if (rawLine.startsWith("--- ") && rawLine.slice(4).trim() !== "/dev/null") {
      const candidate = rawLine.slice(4).split("\t")[0]!.trim();
      return candidate.replace(/^[ab]\//, "");
    }
  }
  return undefined;
}

/**
 * The tool's name, as the runtime reported it.
 *
 * The runtimes do not agree on the key: Pi carries `toolName`, Hermes carries `tool`. Reading only one
 * of them makes the editor label every tool call "tool", which is exactly what the first
 * editor probe saw — the call was real, its name was not.
 */
export function toolTitle(event: AgentRuntimeEvent, fallback = "tool"): string {
  const value = event.payload["toolName"] ?? event.payload["tool"];
  return typeof value === "string" && value !== "" ? value : fallback;
}

/**
 * Why the run failed, as the runtime said it.
 *
 * An empty reason is not a reason: a failure with nothing in it is reported as "the run failed",
 * which is the one thing the person in front of the editor cannot act on.
 */
export function failureReason(event: AgentRuntimeEvent): string | undefined {
  for (const key of ["error", "message", "reason"]) {
    const value = event.payload[key];
    if (typeof value === "string" && value.trim() !== "") return value.trim();
  }
  return undefined;
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
