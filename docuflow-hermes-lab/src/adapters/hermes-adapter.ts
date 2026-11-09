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
  type AgentTaskInput
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
  streamRunEvents(runId: string, signal?: AbortSignal): AsyncGenerator<HermesSseEvent>;
}

export interface HermesAdapterOptions {
  runtimeType?: "hermes";
  /**
   * Capabilities already proven by Phase 0 evidence. Nothing is claimed by default: a capability
   * declared here without a passing checklist item would let DocuFlow plan work it cannot do, and
   * the design forbids turning `blocked` into `passed`.
   */
  verifiedCapabilities?: Partial<AgentCapability>;
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
  terminal: boolean;
  controller: AbortController;
  pump?: Promise<void>;
}

/** Hermes run status → DocuFlow execution status. Unknown values stay `running` and are audit-visible. */
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
          docuflow_run_id: input.runId,
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
