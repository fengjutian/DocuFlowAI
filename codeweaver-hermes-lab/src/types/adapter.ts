/**
 * CodeWeaver Agent Adapter contract.
 *
 * These types are CodeWeaver's stable surface, taken from design document V1.0 section 5.
 * They must not change when a runtime's wire protocol changes; each adapter is responsible
 * for translating between this contract and whatever the pinned runtime version speaks.
 */

export type AgentRuntimeType = "hermes" | "native" | "acp" | "external";

export type AgentExecutionStatus =
  | "queued"
  | "starting"
  | "running"
  | "waiting_approval"
  | "cancelling"
  | "succeeded"
  | "failed"
  | "cancelled"
  | "interrupted";

export interface AgentCapability {
  streaming: boolean;
  cancellation: boolean;
  sessionResume: boolean;
  toolEvents: boolean;
  permissionEvents: boolean;
  subAgents: boolean;
  skills: boolean;
}

export interface AgentTaskInput {
  projectId: string;
  featureId?: string;
  taskId: string;
  runId: string;

  prompt: string;

  contextSnapshotId: string;
  policySnapshotId: string;

  worktreeId: string;
  workingDirectory: string;

  allowedTools: string[];
  timeoutSeconds: number;

  correlationId: string;
}

export interface AgentExecution {
  executionId: string;
  runtimeType: AgentRuntimeType;
  status: AgentExecutionStatus;
  externalSessionId?: string;
  externalRunId?: string;
}

/**
 * `runtime.unknown` is an extension to the V1.0 union. Requirement 5.2.6 forbids dropping
 * events the runtime emits but this contract does not model yet, so unknown runtime events are
 * forwarded under this type with the original name and payload preserved.
 */
export type AgentRuntimeEventType =
  | "execution.started"
  | "message.delta"
  | "message.completed"
  | "tool.started"
  | "tool.progress"
  | "tool.completed"
  | "tool.failed"
  | "permission.requested"
  /**
   * The answer to a permission request.
   *
   * The request alone is half a fact: a reader who sees "the runtime asked" but not "CodeWeaver
   * answered" cannot tell an enforced denial from a request that was never settled. Design 15 asks
   * exactly this question — what was a tool call answered with — so the decision travels on the
   * same timeline as the request.
   */
  | "permission.resolved"
  | "execution.waiting"
  | "execution.completed"
  | "execution.failed"
  | "execution.cancelled"
  | "runtime.unknown";

export interface AgentRuntimeEvent {
  eventId: string;
  executionId: string;
  projectId: string;
  taskId: string;

  sequence: number;
  eventType: AgentRuntimeEventType;

  timestamp: string;

  payload: Record<string, unknown>;

  externalEventId?: string;
}

export interface AgentRuntimeAdapter {
  getCapabilities(): Promise<AgentCapability>;
  healthCheck(): Promise<boolean>;
  start(input: AgentTaskInput): Promise<AgentExecution>;
  getStatus(executionId: string): Promise<AgentExecutionStatus>;
  cancel(executionId: string): Promise<void>;
  resume(executionId: string): Promise<AgentExecution>;
  /**
   * Optional: a follow-up prompt on the session this execution already used.
   *
   * A second turn is not a second execution. The runtime keeps the transcript, so the answer can
   * depend on what was said before — that continuity is the whole point. A runtime that cannot
   * continue a finished turn omits this method and says so, rather than starting a fresh run that
   * only looks like a continuation while carrying none of the context.
   */
  followUp?(executionId: string, prompt: string): Promise<AgentExecution>;
  subscribe(
    executionId: string,
    handler: (event: AgentRuntimeEvent) => void
  ): Promise<() => Promise<void>>;
  terminate(executionId: string): Promise<void>;

  /**
   * Whether the runtime still holds the session behind this execution.
   *
   * The four failures the design lists do not share one recovery strategy: a client that
   * disconnected changes nothing, while a worker that died took the session with it unless the
   * runtime keeps it somewhere else. An adapter that cannot answer omits the method, and the
   * answer is then "no" — a session that cannot be proven is a session that must not be resumed.
   */
  isSessionResumable?(executionId: string): boolean;

  /**
   * Optional approval surface. An adapter that cannot express approvals simply omits these; the API
   * layer then reports no pending approvals instead of inventing an answer.
   */
  getPendingPermissions?(executionId: string): RuntimePermissionRequest[];  respondToPermission?(
    executionId: string,
    requestId: string,
    response: RuntimePermissionResponse
  ): Promise<void>;
}

export type AgentAdapterErrorCode =
  | "unsupported_capability"
  | "runtime_unreachable"
  | "protocol_error"
  | "execution_not_found"
  | "invalid_state";

/** A permission question raised by the runtime, to be answered by CodeWeaver policy. */
export interface RuntimePermissionRequest {
  requestId: string;
  executionId: string;
  payload: Record<string, unknown>;
  tool?: string;
}

/**
 * The adapter never invents an answer. `respondToPermission` sends exactly what the caller decided,
 * and a runtime request without a handler stays pending instead of being auto-approved.
 */
export interface RuntimePermissionResponse {
  decision: "approve" | "deny";
  reason?: string;
}

export class AgentAdapterError extends Error {
  constructor(
    public readonly code: AgentAdapterErrorCode,
    message: string,
    options?: { cause?: unknown }
  ) {
    super(message, options);
    this.name = "AgentAdapterError";
  }
}

/** Capability set used before any runtime fact is observed: nothing is claimed. */
export const NO_CAPABILITIES: AgentCapability = {
  streaming: false,
  cancellation: false,
  sessionResume: false,
  toolEvents: false,
  permissionEvents: false,
  subAgents: false,
  skills: false
};

export function isTerminalStatus(status: AgentExecutionStatus): boolean {
  return status === "succeeded" || status === "failed" || status === "cancelled" || status === "interrupted";
}
