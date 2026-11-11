import { randomUUID } from "node:crypto";
import {
  AgentAdapterError,
  NO_CAPABILITIES,
  isTerminalStatus,
  type AgentCapability,
  type AgentExecution,
  type AgentExecutionStatus,
  type AgentRuntimeAdapter,
  type AgentRuntimeEvent,
  type AgentTaskInput,
  type RuntimePermissionRequest,
  type RuntimePermissionResponse
} from "../types/adapter.js";
import type {
  HermesCapabilities,
  HermesRun,
  HermesRunCreateRequest,
  HermesSseEvent
} from "../types/hermes.js";
import { normalizeHermesEvent } from "./hermes-event-normalizer.js";

/**
 * The subset of `HermesHttpClient` the adapter depends on. Keeping it structural lets tests run
 * without a Hermes server and lets a future Gateway-based adapter reuse the same execution logic.
 */
export interface HermesAdapterClient {
  capabilities(): Promise<HermesCapabilities>;
  createRun(payload: HermesRunCreateRequest): Promise<HermesRun>;
  getRun(runId: string): Promise<HermesRun>;
  stopRun(runId: string): Promise<unknown>;
  approveRun?(runId: string, payload: Record<string, unknown>): Promise<unknown>;
  streamRunEvents(runId: string, signal?: AbortSignal): AsyncGenerator<HermesSseEvent>;
}

export interface HermesAdapterOptions {
  runtimeType?: "hermes";
  /**
   * Capabilities already proven by Phase 0 evidence. Nothing is claimed by default: a capability
   * declared here without a passing checklist item would let CodeWeaver plan work it cannot do, and
   * the design forbids turning `blocked` into `passed`.
   */
  verifiedCapabilities?: Partial<AgentCapability>;
  /**
   * Optional policy hook. When present, every `permission.requested` event is answered with the
   * decision this function returns. When absent, requests stay pending: the adapter never
   * auto-approves a runtime request (upstream contract requires an explicit handler).
   */
  permissionHandler?: (request: RuntimePermissionRequest) => Promise<RuntimePermissionResponse>;
  now?: () => Date;
  executionIdFactory?: () => string;
}

interface Subscriber {
  handler: (event: AgentRuntimeEvent) => void;
  cursor: number;
}

interface ExecutionState {
  execution: AgentExecution;
  input: AgentTaskInput;
  status: AgentExecutionStatus;
  events: AgentRuntimeEvent[];
  subscribers: Set<Subscriber>;
  sequence: number;
  seenExternalEventIds: Set<string>;
  pendingPermissions: Map<string, RuntimePermissionRequest>;
  terminal: boolean;
  controller: AbortController;
  pump?: Promise<void>;
}

/** Hermes run status → CodeWeaver execution status. Unknown values stay `running` and are audit-visible. */
const RUN_STATUS_MAP: Readonly<Record<string, AgentExecutionStatus>> = {
  queued: "queued",
  pending: "queued",
  starting: "starting",
  running: "running",
  in_progress: "running",
  waiting_approval: "waiting_approval",
  awaiting_approval: "waiting_approval",
  cancelling: "cancelling",
  succeeded: "succeeded",
  completed: "succeeded",
  failed: "failed",
  error: "failed",
  cancelled: "cancelled",
  canceled: "cancelled",
  interrupted: "interrupted"
};

const EVENT_STATUS: Partial<Record<AgentRuntimeEvent["eventType"], AgentExecutionStatus>> = {
  "execution.started": "running",
  "permission.requested": "waiting_approval",
  "execution.waiting": "waiting_approval",
  "execution.completed": "succeeded",
  "execution.failed": "failed",
  "execution.cancelled": "cancelled"
};

export class HermesAdapter implements AgentRuntimeAdapter {
  private readonly executions = new Map<string, ExecutionState>();
  private readonly now: () => Date;
  private readonly executionIdFactory: () => string;

  constructor(
    private readonly client: HermesAdapterClient,
    private readonly options: HermesAdapterOptions = {}
  ) {
    this.now = options.now ?? (() => new Date());
    this.executionIdFactory = options.executionIdFactory ?? (() => `exec_${randomUUID()}`);
  }

  async getCapabilities(): Promise<AgentCapability> {
    let features: Record<string, boolean | string> = {};
    try {
      const capabilities = await this.client.capabilities();
      features = capabilities.features ?? {};
    } catch (error) {
      throw new AgentAdapterError("runtime_unreachable", "Hermes capabilities request failed", {
        cause: error
      });
    }
    return {
      ...NO_CAPABILITIES,
      ...(features.run_events_sse === true ? { streaming: true } : {}),
      ...this.options.verifiedCapabilities
    };
  }

  async healthCheck(): Promise<boolean> {
    try {
      await this.client.capabilities();
      return true;
    } catch {
      return false;
    }
  }

  async start(input: AgentTaskInput): Promise<AgentExecution> {
    const existing = [...this.executions.values()].find(
      state => state.input.runId === input.runId && !state.terminal
    );
    if (existing) {
      throw new AgentAdapterError(
        "invalid_state",
        `Run ${input.runId} already has an active execution ${existing.execution.executionId}`
      );
    }
    if (input.timeoutSeconds <= 0) {
      throw new AgentAdapterError("invalid_state", "timeoutSeconds must be a positive number");
    }

    const run = await this.createRun(input);
    if (!run || typeof run.run_id !== "string" || run.run_id === "") {
      throw new AgentAdapterError("protocol_error", "Hermes create-run response did not contain run_id");
    }

    const execution: AgentExecution = {
      executionId: this.executionIdFactory(),
      runtimeType: "hermes",
      status: "queued",
      externalRunId: run.run_id
    };
    const state: ExecutionState = {
      execution,
      input,
      status: "queued",
      events: [],
      subscribers: new Set(),
      sequence: 0,
      seenExternalEventIds: new Set(),
      pendingPermissions: new Map(),
      terminal: false,
      controller: new AbortController()
    };
    this.executions.set(execution.executionId, state);
    this.emit(state, "execution.started", {
      externalRunId: run.run_id,
      ...(run.status === undefined ? {} : { externalStatus: run.status })
    });
    return { ...execution, status: state.status };
  }

  async getStatus(executionId: string): Promise<AgentExecutionStatus> {
    const state = this.requireState(executionId);
    if (state.terminal) return state.status;

    const externalRunId = state.execution.externalRunId;
    if (!externalRunId) return state.status;

    let run: HermesRun;
    try {
      run = await this.client.getRun(externalRunId);
    } catch (error) {
      throw new AgentAdapterError("runtime_unreachable", `Hermes run lookup failed for ${externalRunId}`, {
        cause: error
      });
    }
    const mapped = mapRunStatus(run.status);
    if (mapped && mapped !== state.status) {
      state.status = mapped;
      state.execution.status = mapped;
      if (isTerminalStatus(mapped)) state.terminal = true;
    }
    return state.status;
  }

  async cancel(executionId: string): Promise<void> {
    const state = this.requireState(executionId);
    const capabilities = await this.capabilitiesWithoutThrowing();
    if (!capabilities.cancellation) {
      throw new AgentAdapterError(
        "unsupported_capability",
        "Cancellation is not verified for the pinned Hermes version; see Phase 0 P0-018"
      );
    }
    if (state.terminal) return;
    const externalRunId = state.execution.externalRunId;
    if (!externalRunId) throw new AgentAdapterError("invalid_state", "Execution has no external run id");

    state.status = "cancelling";
    state.execution.status = "cancelling";
    try {
      await this.client.stopRun(externalRunId);
    } catch (error) {
      state.status = "running";
      state.execution.status = "running";
      throw new AgentAdapterError("runtime_unreachable", `Hermes stop request failed for ${externalRunId}`, {
        cause: error
      });
    }
  }

  /**
   * The Hermes HTTP API has no documented resume operation for a run, so this adapter reports
   * `unsupported_capability` instead of pretending. The Recovery Engine creates a new execution
   * and injects a checkpoint, per design section 5.1.
   */
  async resume(executionId: string): Promise<AgentExecution> {
    this.requireState(executionId);
    throw new AgentAdapterError(
      "unsupported_capability",
      "Hermes HTTP adapter cannot resume a run; create a new execution with a checkpoint instead"
    );
  }

  async subscribe(
    executionId: string,
    handler: (event: AgentRuntimeEvent) => void
  ): Promise<() => Promise<void>> {
    const state = this.requireState(executionId);
    const subscriber: Subscriber = { handler, cursor: 0 };
    state.subscribers.add(subscriber);

    // Replay before starting the pump so a late subscriber never misses buffered events.
    while (subscriber.cursor < state.events.length) {
      const event = state.events[subscriber.cursor]!;
      subscriber.cursor += 1;
      handler(event);
    }

    if (!state.terminal && !state.pump) {
      state.pump = this.pump(state);
    }

    return async () => {
      state.subscribers.delete(subscriber);
    };
  }

  getPendingPermissions(executionId: string): RuntimePermissionRequest[] {
    return [...this.requireState(executionId).pendingPermissions.values()];
  }

  /**
   * Answers a pending runtime permission request. CodeWeaver's Policy Engine decides; the adapter only
   * transmits the decision. Nothing is sent without an explicit call.
   */
  async respondToPermission(
    executionId: string,
    requestId: string,
    response: RuntimePermissionResponse
  ): Promise<void> {
    const state = this.requireState(executionId);
    if (!state.pendingPermissions.has(requestId)) {
      throw new AgentAdapterError("invalid_state", `No pending permission request ${requestId}`);
    }
    await this.sendPermissionResponse(state, requestId, response);
  }

  async terminate(executionId: string): Promise<void> {
    const state = this.requireState(executionId);
    state.controller.abort();
    if (!state.terminal) {
      state.terminal = true;
      state.status = "cancelled";
      state.execution.status = "cancelled";
    }
    this.executions.delete(executionId);
    await state.pump?.catch(() => undefined);
  }

  private async createRun(input: AgentTaskInput): Promise<HermesRun> {
    try {
      return await this.client.createRun({
        input: input.prompt,
        metadata: {
          project_id: input.projectId,
          task_id: input.taskId,
          codeweaver_run_id: input.runId,
          correlation_id: input.correlationId
        }
      });
    } catch (error) {
      throw new AgentAdapterError("runtime_unreachable", "Hermes run creation failed", { cause: error });
    }
  }

  private async pump(state: ExecutionState): Promise<void> {
    const externalRunId = state.execution.externalRunId;
    if (!externalRunId) return;
    const timeoutSignal = AbortSignal.timeout(Math.max(1, state.input.timeoutSeconds) * 1000);
    const signal = AbortSignal.any([timeoutSignal, state.controller.signal]);
    try {
      for await (const raw of this.client.streamRunEvents(externalRunId, signal)) {
        this.ingest(state, raw);
        if (state.terminal) break;
      }
      if (!state.terminal) {
        this.emit(state, "execution.failed", {
          reason: "stream_closed",
          message: "Hermes event stream ended before a terminal event"
        });
      }
    } catch (error) {
      if (!state.terminal && !state.controller.signal.aborted) {
        this.emit(state, "execution.failed", {
          reason: "stream_error",
          message: error instanceof Error ? error.message : String(error)
        });
      }
    }
  }

  private ingest(state: ExecutionState, raw: HermesSseEvent): void {
    if (raw.id !== undefined && raw.id !== "") {
      if (state.seenExternalEventIds.has(raw.id)) return;
      state.seenExternalEventIds.add(raw.id);
    }
    const context = {
      executionId: state.execution.executionId,
      projectId: state.input.projectId,
      taskId: state.input.taskId,
      sequence: state.sequence + 1,
      timestamp: this.now().toISOString()
    };
    const event = normalizeHermesEvent(raw, context);
    this.append(state, event);
  }

  private emit(
    state: ExecutionState,
    eventType: AgentRuntimeEvent["eventType"],
    payload: Record<string, unknown>
  ): void {
    const event: AgentRuntimeEvent = {
      eventId: `${state.execution.executionId}:${state.sequence + 1}`,
      executionId: state.execution.executionId,
      projectId: state.input.projectId,
      taskId: state.input.taskId,
      sequence: state.sequence + 1,
      eventType,
      timestamp: this.now().toISOString(),
      payload
    };
    this.append(state, event);
  }

  private append(state: ExecutionState, event: AgentRuntimeEvent): void {
    state.sequence = event.sequence;
    state.events.push(event);
    const mappedStatus = EVENT_STATUS[event.eventType];
    if (mappedStatus) {
      state.status = mappedStatus;
      state.execution.status = mappedStatus;
      if (isTerminalStatus(mappedStatus)) state.terminal = true;
    }
    for (const subscriber of state.subscribers) {
      while (subscriber.cursor < state.events.length) {
        const next = state.events[subscriber.cursor]!;
        subscriber.cursor += 1;
        try {
          subscriber.handler(next);
        } catch {
          // A broken consumer must not stall the pump or starve other subscribers.
        }
      }
    }
    if (event.eventType === "permission.requested") this.onPermissionRequested(state, event);
  }

  private onPermissionRequested(state: ExecutionState, event: AgentRuntimeEvent): void {
    const requestId = extractPermissionRequestId(event);
    if (state.pendingPermissions.has(requestId)) return;
    const tool = typeof event.payload["tool"] === "string" ? event.payload["tool"] : undefined;
    const request: RuntimePermissionRequest = {
      requestId,
      executionId: state.execution.executionId,
      payload: event.payload,
      ...(tool ? { tool } : {})
    };
    state.pendingPermissions.set(requestId, request);

    // Fail closed: without a policy handler the request stays pending and the runtime keeps waiting.
    if (!this.options.permissionHandler) return;
    void this.answerPermission(state, request).catch(() => undefined);
  }

  private async answerPermission(
    state: ExecutionState,
    request: RuntimePermissionRequest
  ): Promise<void> {
    const handler = this.options.permissionHandler;
    if (!handler) return;
    const response = await handler(request);
    // A concurrent respondToPermission() may have answered it while the policy ran.
    if (!state.pendingPermissions.has(request.requestId)) return;
    await this.sendPermissionResponse(state, request.requestId, response);
  }

  private async sendPermissionResponse(
    state: ExecutionState,
    requestId: string,
    response: RuntimePermissionResponse
  ): Promise<void> {
    const externalRunId = state.execution.externalRunId;
    if (!externalRunId) throw new AgentAdapterError("invalid_state", "Execution has no external run id");
    const approve = this.client.approveRun;
    if (!approve) {
      throw new AgentAdapterError("unsupported_capability", "Hermes client cannot send approval responses");
    }
    try {
      await approve.call(this.client, externalRunId, {
        request_id: requestId,
        decision: response.decision,
        ...(response.reason ? { reason: response.reason } : {})
      });
    } catch (error) {
      throw new AgentAdapterError("runtime_unreachable", `Hermes approval request failed for ${requestId}`, {
        cause: error
      });
    }
    const tool = state.pendingPermissions.get(requestId)?.tool;
    state.pendingPermissions.delete(requestId);
    // The decision is part of the fact: a reader who only sees the request cannot tell an
    // enforced denial from a request nobody ever answered.
    this.emit(state, "permission.resolved", {
      requestId,
      ...(tool ? { tool } : {}),
      decision: response.decision,
      ...(response.reason ? { reason: response.reason } : {})
    });
    if (!state.terminal) {
      state.status = "running";
      state.execution.status = "running";
    }
  }

  private requireState(executionId: string): ExecutionState {
    const state = this.executions.get(executionId);
    if (!state) {
      throw new AgentAdapterError("execution_not_found", `Unknown execution ${executionId}`);
    }
    return state;
  }

  private async capabilitiesWithoutThrowing(): Promise<AgentCapability> {
    try {
      return await this.getCapabilities();
    } catch {
      return { ...NO_CAPABILITIES, ...this.options.verifiedCapabilities };
    }
  }
}

function mapRunStatus(status: unknown): AgentExecutionStatus | undefined {
  if (typeof status !== "string") return undefined;
  return RUN_STATUS_MAP[status.toLowerCase()];
}

/** Hermes may carry the approval id at a few documented-looking keys; check them in order. */
function extractPermissionRequestId(event: AgentRuntimeEvent): string {
  for (const key of ["request_id", "requestId", "approval_id", "id"]) {
    const value = event.payload[key];
    if (typeof value === "string" && value !== "") return value;
  }
  return event.externalEventId ?? event.eventId;
}
