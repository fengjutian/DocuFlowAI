import type { AgentRuntimeEvent, AgentRuntimeEventType } from "../types/adapter.js";
import type { HermesSseEvent } from "../types/hermes.js";

/**
 * Hermes event name → CodeWeaver event type.
 *
 * Every alias listed here has to be traceable to the pinned Hermes release or to a captured
 * Phase 0 transcript; do not add speculative names without evidence. Unknown names are not
 * dropped: they surface as `runtime.unknown` with the original payload intact.
 */
export const HERMES_EVENT_MAP: Readonly<Record<string, AgentRuntimeEventType>> = {
  "run.started": "execution.started",
  "run.queued": "execution.started",
  "execution.started": "execution.started",

  "message.delta": "message.delta",
  "run.message.delta": "message.delta",
  "assistant.delta": "message.delta",
  "run.output.delta": "message.delta",

  "message.completed": "message.completed",
  "run.message.completed": "message.completed",
  "assistant.message": "message.completed",

  "tool.started": "tool.started",
  "tool_call.started": "tool.started",
  "run.tool.started": "tool.started",

  "tool.progress": "tool.progress",
  "tool_call.progress": "tool.progress",
  "run.tool.progress": "tool.progress",

  "tool.completed": "tool.completed",
  "tool_call.completed": "tool.completed",
  "tool_call.finished": "tool.completed",
  "run.tool.completed": "tool.completed",

  "tool.failed": "tool.failed",
  "tool_call.failed": "tool.failed",
  "run.tool.failed": "tool.failed",

  "permission.requested": "permission.requested",
  "approval.requested": "permission.requested",
  "run.approval.requested": "permission.requested",
  approval: "permission.requested",

  "execution.waiting": "execution.waiting",
  "run.waiting": "execution.waiting",
  "run.input_required": "execution.waiting",

  "run.completed": "execution.completed",
  "run.succeeded": "execution.completed",
  "execution.completed": "execution.completed",

  "run.failed": "execution.failed",
  "run.error": "execution.failed",
  "run.interrupted": "execution.failed",
  "execution.failed": "execution.failed",

  "run.cancelled": "execution.cancelled",
  "run.canceled": "execution.cancelled",
  "execution.cancelled": "execution.cancelled"
};

export interface NormalizeContext {
  executionId: string;
  projectId: string;
  taskId: string;
  sequence: number;
  timestamp: string;
}

/**
 * Converts one raw Hermes SSE event into the CodeWeaver event model.
 *
 * The raw event name always travels in `payload.hermesEvent`, so a mapped event stays auditable
 * even if the runtime adds a field this contract does not know about.
 */
export function normalizeHermesEvent(
  raw: HermesSseEvent,
  context: NormalizeContext
): AgentRuntimeEvent {
  const hermesEvent = resolveEventName(raw);
  const eventType = HERMES_EVENT_MAP[hermesEvent] ?? "runtime.unknown";
  const payload = toPayload(raw.data);
  const event: AgentRuntimeEvent = {
    eventId: buildEventId(context.executionId, context.sequence),
    executionId: context.executionId,
    projectId: context.projectId,
    taskId: context.taskId,
    sequence: context.sequence,
    eventType,
    timestamp: context.timestamp,
    payload: {
      ...payload,
      hermesEvent
    }
  };
  if (raw.id !== undefined && raw.id !== "") event.externalEventId = raw.id;
  if (eventType === "runtime.unknown" && !("raw" in payload)) {
    event.payload.raw = raw.data;
  }
  return event;
}

/**
 * SSE frames default to the `message` event name, so the real event name may live in the JSON
 * payload instead. Check the frame name first, then the payload's `event` / `type` field.
 */
export function resolveEventName(raw: HermesSseEvent): string {
  if (raw.event && raw.event !== "message") return raw.event;
  const data = raw.data;
  if (data && typeof data === "object" && !Array.isArray(data)) {
    const record = data as Record<string, unknown>;
    for (const key of ["event", "type", "name"]) {
      const value = record[key];
      if (typeof value === "string" && value) return value;
    }
  }
  return raw.event || "message";
}

function toPayload(data: unknown): Record<string, unknown> {
  if (data && typeof data === "object" && !Array.isArray(data)) {
    return { ...(data as Record<string, unknown>) };
  }
  return { data };
}

function buildEventId(executionId: string, sequence: number): string {
  return `${executionId}:${sequence}`;
}
