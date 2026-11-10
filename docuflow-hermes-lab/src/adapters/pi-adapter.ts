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

/**
 * Adapter for the Pi agent harness (`earendil-works/pi`, `pi-agent-core`).
 *
 * Pi is an in-process TypeScript runtime, so this adapter has no wire protocol to speak: the
 * lifecycle event stream, cancellation and tool interception are the runtime's own primitives.
 * That is the reason the adapter is thin — and the reason a policy decision can be enforced
 * before a tool runs instead of being reported after the fact.
 *
 * The Pi types are kept structural on purpose. The adapter must compile and be testable without
 * the Pi packages installed; the real binding lives in `pi-agent-factory.ts`.
 */

/** A Pi lifecycle event, as seen by the adapter. Unknown shapes are forwarded, never dropped. */
export interface PiAgentEvent {
  type: string;
  [key: string]: unknown;
}

/** The subset of Pi's `Agent` the adapter uses. */
export interface PiAgentLike {
  subscribe(listener: (event: PiAgentEvent) => void): () => void;
  prompt(input: string): Promise<void>;
  abort(): void;
  waitForIdle(): Promise<void>;
  readonly state: {
    messages: unknown[];
    readonly errorMessage?: string;
    readonly isStreaming: boolean;
  };
}

/** The hook Pi calls before a tool executes. Returning `block` prevents the execution. */
export interface PiBeforeToolCallContext {
  toolCall: { name?: string; id?: string; arguments?: unknown };
  args: unknown;
}

export interface PiBeforeToolCallResult {
  block?: boolean;
  reason?: string;
}

/** How the adapter builds a Pi agent. Injected so tests can drive a real agent without a model. */
export interface PiAgentInit {
  systemPrompt?: string;
  tools: unknown[];
  model: unknown;
  beforeToolCall: (
    context: PiBeforeToolCallContext
  ) => Promise<PiBeforeToolCallResult | undefined>;
}

export type PiAgentFactory = (init: PiAgentInit) => PiAgentLike;

export interface PiAdapterOptions {
  runtimeType?: "native";
  /**
   * Optional policy hook. When present, every tool call is decided by this function before the
   * tool executes. When absent, tool calls are blocked: Pi's hook must answer before the call
   * runs, so "no decision" means "do not run" (the same fail-closed rule as the Hermes adapter,
   * where an unanswered request stays pending instead of being approved).
   */
  permissionHandler?: (request: RuntimePermissionRequest) => Promise<RuntimePermissionResponse>;
  /**
   * Tools exposed to the runtime. Calls to tools outside this list are blocked by policy.
   *
   * A function receives the task input and the execution id, because the tool set is bound to one
   * execution: the worktree and the policy snapshot belong to that execution, not to the adapter.
   */
  tools?: unknown[] | ((input: AgentTaskInput, executionId: string) => unknown[]);
  /**
   * Tools whose own implementation is the policy enforcement point.
   *
   * DocuFlow's own tools decide for themselves: `read_file` checks the path against the policy
   * snapshot, `request_apply_diff` goes through the Code Engine, which checks scope, base SHA and
   * policy before it writes. Gating them a second time would either duplicate the decision or block
   * the tool outright, because the adapter-level hook sees only a tool name and no resolved paths.
   * Every other tool still needs the permission handler.
   */
  selfEnforcedTools?: string[];
  /** The Pi model to run. */
  model?: unknown;
  systemPrompt?: string;
  now?: () => Date;
  executionIdFactory?: () => string;
}

interface Subscriber {
  handler: (event: AgentRuntimeEvent) => void;
}

interface ExecutionState {
  execution: AgentExecution;
  input: AgentTaskInput;
  status: AgentExecutionStatus;
  events: AgentRuntimeEvent[];
  subscribers: Set<Subscriber>;
  sequence: number;
  pendingPermissions: Map<string, RuntimePermissionRequest>;
  /** Decisions waiting for `respondToPermission`, keyed by request id. */
  permissionDecisions: Map<string, { resolve: (response: RuntimePermissionResponse) => void }>;
  terminal: boolean;
  cancelled: boolean;
  agent: PiAgentLike;
}

const EVENT_STATUS: Partial<Record<AgentRuntimeEvent["eventType"], AgentExecutionStatus>> = {
  "execution.started": "running",
  "permission.requested": "waiting_approval",
  "execution.waiting": "waiting_approval",
  "execution.completed": "succeeded",
  "execution.failed": "failed",
  "execution.cancelled": "cancelled"
};

export class PiAdapter implements AgentRuntimeAdapter {
  private readonly executions = new Map<string, ExecutionState>();
  private readonly now: () => Date;
  private readonly executionIdFactory: () => string;

  constructor(
    private readonly createAgent: PiAgentFactory,
    private readonly options: PiAdapterOptions = {}
  ) {
    this.now = options.now ?? (() => new Date());
    this.executionIdFactory = options.executionIdFactory ?? (() => `exec_${randomUUID()}`);
  }

  /**
   * Pi's capabilities are properties of the runtime API, not guesses: the event stream, the
   * abort signal and the tool hook are all part of `pi-agent-core`. Sub-agents and skills are not
   * claimed — DocuFlow owns both layers today.
   */
  async getCapabilities(): Promise<AgentCapability> {
    return {
      ...NO_CAPABILITIES,
      streaming: true,
      cancellation: true,
      sessionResume: true,
      toolEvents: true,
      permissionEvents: true
    };
  }

  async healthCheck(): Promise<boolean> {
    try {
      this.createAgent({
        tools: [],
        model: this.options.model,
        beforeToolCall: async () => undefined
      });
      return true;
    } catch {
      return false;
    }
  }

  async start(input: AgentTaskInput): Promise<AgentExecution> {
    const executionId = this.executionIdFactory();
    const execution: AgentExecution = {
      executionId,
      runtimeType: this.options.runtimeType ?? "native",
      status: "queued"
    };
    const state: ExecutionState = {
      execution,
      input,
      status: "queued",
      events: [],
      subscribers: new Set(),
      sequence: 0,
      pendingPermissions: new Map(),
      permissionDecisions: new Map(),
      terminal: false,
      cancelled: false,
      agent: undefined as unknown as PiAgentLike
    };
    this.executions.set(executionId, state);

    const agent = this.createAgent({
      ...(this.options.systemPrompt ? { systemPrompt: this.options.systemPrompt } : {}),
      tools: this.resolveTools(input, executionId),
      model: this.options.model,
      beforeToolCall: context => this.beforeToolCall(state, context)
    });
    state.agent = agent;

    agent.subscribe(event => {
      this.onAgentEvent(state, event);
    });

    // The prompt drives the run; failures are reported through the event stream, not thrown here.
    void agent.prompt(input.prompt).catch(error => {
      this.emit(state, "execution.failed", { error: error instanceof Error ? error.message : String(error) });
    });

    return { ...execution };
  }

  /**
   * The tool set for one execution.
   *
   * A function form is built per execution, because the tools are bound to that execution's
   * worktree and policy snapshot; a static list is shared by every run.
   */
  private resolveTools(input: AgentTaskInput, executionId: string): unknown[] {
    const tools = this.options.tools;
    if (typeof tools === "function") return tools(input, executionId);
    return tools ?? [];
  }

  async getStatus(executionId: string): Promise<AgentExecutionStatus> {
    return this.require(executionId).status;
  }

  async cancel(executionId: string): Promise<void> {
    const state = this.require(executionId);
    if (state.terminal) return;
    state.cancelled = true;
    state.agent.abort();
    this.emit(state, "execution.cancelled", { reason: "cancelled by DocuFlow" });
  }

  /**
   * Pi keeps the transcript on the agent, so a resumed execution continues the same conversation
   * rather than starting over. The original prompt is re-sent on the same agent instance.
   */
  async resume(executionId: string): Promise<AgentExecution> {
    const state = this.require(executionId);
    if (!state.terminal) {
      throw new AgentAdapterError("invalid_state", `Execution ${executionId} is still running`);
    }
    state.terminal = false;
    state.cancelled = false;
    state.status = "running";
    void state.agent.prompt(state.input.prompt).catch(error => {
      this.emit(state, "execution.failed", { error: error instanceof Error ? error.message : String(error) });
    });
    return { ...state.execution, status: "running" };
  }

  async subscribe(
    executionId: string,
    handler: (event: AgentRuntimeEvent) => void
  ): Promise<() => Promise<void>> {
    const state = this.require(executionId);
    const subscriber: Subscriber = { handler };
    state.subscribers.add(subscriber);
    // Late subscribers are replayed the buffered history first, in order.
    for (const event of state.events) {
      subscriber.handler(event);
    }
    return async () => {
      state.subscribers.delete(subscriber);
    };
  }

  async terminate(executionId: string): Promise<void> {
    const state = this.require(executionId);
    if (!state.terminal) {
      state.cancelled = true;
      state.agent.abort();
    }
    this.executions.delete(executionId);
  }

  getPendingPermissions(executionId: string): RuntimePermissionRequest[] {
    return [...this.require(executionId).pendingPermissions.values()];
  }

  async respondToPermission(
    executionId: string,
    requestId: string,
    response: RuntimePermissionResponse
  ): Promise<void> {
    const state = this.require(executionId);
    const request = state.pendingPermissions.get(requestId);
    if (!request) {
      throw new AgentAdapterError("invalid_state", `Unknown permission request ${requestId}`);
    }
    const decision = state.permissionDecisions.get(requestId);
    if (!decision) {
      throw new AgentAdapterError("invalid_state", `Permission request ${requestId} has no pending decision`);
    }
    state.permissionDecisions.delete(requestId);
    state.pendingPermissions.delete(requestId);
    decision.resolve(response);
  }

  /**
   * The policy gate. It runs before Pi executes the tool, so a denial is enforcement rather
   * than a report: the tool never runs and the reason reaches the model as an error result.
   */
  private async beforeToolCall(
    state: ExecutionState,
    context: PiBeforeToolCallContext
  ): Promise<PiBeforeToolCallResult | undefined> {
    const toolName = context.toolCall.name ?? "unknown";
    if (!state.input.allowedTools.includes(toolName)) {
      return { block: true, reason: `tool ${toolName} is not in the allowed tool list` };
    }

    // A tool that enforces the policy itself is not gated a second time here: the decision it makes
    // is the accurate one, because it knows the paths the tool would really touch.
    if (this.options.selfEnforcedTools?.includes(toolName)) {
      return undefined;
    }

    if (!this.options.permissionHandler) {
      return { block: true, reason: `no policy handler is configured for tool ${toolName}` };
    }

    const request: RuntimePermissionRequest = {
      requestId: `perm_${randomUUID()}`,
      executionId: state.execution.executionId,
      payload: { tool: toolName, args: context.args },
      tool: toolName
    };
    state.pendingPermissions.set(request.requestId, request);
    this.emit(state, "permission.requested", {
      requestId: request.requestId,
      tool: toolName,
      args: context.args
    });

    // The handler may answer inline, or leave the promise pending and let the API layer settle it
    // through `respondToPermission`. Racing both keeps one code path for the two shapes.
    const deferred = new Promise<RuntimePermissionResponse>(resolve => {
      state.permissionDecisions.set(request.requestId, { resolve });
    });
    const decision = await Promise.race([
      Promise.resolve(this.options.permissionHandler(request)),
      deferred
    ]);
    state.permissionDecisions.delete(request.requestId);
    state.pendingPermissions.delete(request.requestId);
    if (!decision || decision.decision === "deny") {
      return {
        block: true,
        reason: decision?.reason ?? `tool ${toolName} denied by policy`
      };
    }
    return undefined;
  }

  private onAgentEvent(state: ExecutionState, event: PiAgentEvent): void {
    switch (event.type) {
      case "agent_start":
        this.emit(state, "execution.started", { runtime: "pi" });
        return;
      case "message_update": {
        const detail = event.assistantMessageEvent as { type?: string; delta?: string } | undefined;
        if (detail?.type === "text_delta" && typeof detail.delta === "string") {
          this.emit(state, "message.delta", { delta: detail.delta });
          return;
        }
        this.emit(state, "runtime.unknown", { runtimeEvent: event.type, raw: event });
        return;
      }
      case "message_end": {
        // Only the assistant's own output is a completed message; the echo of the user prompt is
        // not a runtime result.
        const role = (event.message as { role?: string } | undefined)?.role;
        if (role === "assistant") {
          this.emit(state, "message.completed", { message: event.message });
          return;
        }
        this.emit(state, "runtime.unknown", { runtimeEvent: event.type, raw: event });
        return;
      }
      case "tool_execution_start":
        this.emit(state, "tool.started", {
          toolCallId: event.toolCallId,
          toolName: event.toolName,
          args: event.args
        });
        return;
      case "tool_execution_update":
        this.emit(state, "tool.progress", {
          toolCallId: event.toolCallId,
          toolName: event.toolName,
          partialResult: event.partialResult
        });
        return;
      case "tool_execution_end":
        this.emit(state, event.isError === true ? "tool.failed" : "tool.completed", {
          toolCallId: event.toolCallId,
          toolName: event.toolName,
          result: event.result,
          durationMs: event.durationMs
        });
        return;
      case "agent_end": {
        const errorMessage = state.agent.state.errorMessage;
        if (state.cancelled) {
          this.emit(state, "execution.cancelled", { reason: "cancelled by DocuFlow" });
        } else if (typeof errorMessage === "string" && errorMessage.length > 0) {
          this.emit(state, "execution.failed", { error: errorMessage });
        } else {
          this.emit(state, "execution.completed", { messages: event.messages });
        }
        return;
      }
      default:
        // Requirement 5.2.6: an event the runtime emits is never dropped.
        this.emit(state, "runtime.unknown", { runtimeEvent: event.type, raw: event });
    }
  }

  private emit(
    state: ExecutionState,
    eventType: AgentRuntimeEvent["eventType"],
    payload: Record<string, unknown>
  ): void {
    const nextStatus = EVENT_STATUS[eventType];
    // A terminal event is recorded once. `cancel()` and the runtime's own end event can both
    // report the same outcome, and two terminal events would make the audit timeline ambiguous.
    if (state.terminal && nextStatus && isTerminalStatus(nextStatus)) {
      return;
    }
    state.sequence += 1;
    const event: AgentRuntimeEvent = {
      eventId: `${state.execution.executionId}-${state.sequence}`,
      executionId: state.execution.executionId,
      projectId: state.input.projectId,
      taskId: state.input.taskId,
      sequence: state.sequence,
      eventType,
      timestamp: this.now().toISOString(),
      payload
    };
    state.events.push(event);
    if (nextStatus) {
      state.status = nextStatus;
    }
    if (isTerminalStatus(state.status)) {
      state.terminal = true;
      state.execution = { ...state.execution, status: state.status };
    }
    for (const subscriber of state.subscribers) {
      subscriber.handler(event);
    }
  }

  private require(executionId: string): ExecutionState {
    const state = this.executions.get(executionId);
    if (!state) {
      throw new AgentAdapterError("execution_not_found", `Unknown execution ${executionId}`);
    }
    return state;
  }
}
