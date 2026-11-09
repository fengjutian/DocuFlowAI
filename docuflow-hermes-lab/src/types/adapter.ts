/**
 * DocuFlow Agent Adapter contract.
 *
 * These types are DocuFlow's stable surface, taken from design document V1.0 section 5.
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
  subscribe(
    executionId: string,
    handler: (event: AgentRuntimeEvent) => void
  ): Promise<() => Promise<void>>;
  terminate(executionId: string): Promise<void>;
}

export type AgentAdapterErrorCode =
  | "unsupported_capability"
  | "runtime_unreachable"
  | "protocol_error"
  | "execution_not_found"
  | "invalid_state";

/** A permission question raised by the runtime, to be answered by DocuFlow policy. */
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
