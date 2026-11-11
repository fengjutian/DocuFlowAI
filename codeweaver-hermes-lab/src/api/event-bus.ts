import { randomUUID } from "node:crypto";
import type { AgentRuntimeEvent, AgentRuntimeEventType } from "../types/adapter.js";
import type { JournalEntry } from "../journal/journal.js";

/**
 * Unified frontend event envelope (design 13).
 *
 * The transport is SSE today and WebSocket later: the envelope and the replay-by-sequence rule are
 * the same, so the client contract does not change when the transport does.
 */
export interface CodeWeaverEvent<T = unknown> {
  id: string;
  type: string;
  projectId: string;
  taskId?: string;
  executionId?: string;
  sequence: number;
  timestamp: string;
  data: T;
}

const TYPE_MAP: Record<AgentRuntimeEventType, string> = {
  "execution.started": "agent.execution.started",
  "message.delta": "agent.message.delta",
  "message.completed": "agent.message.completed",
  "tool.started": "agent.tool.started",
  "tool.progress": "agent.tool.progress",
  "tool.completed": "agent.tool.completed",
  "tool.failed": "agent.tool.failed",
  "permission.requested": "agent.permission.requested",
  "execution.waiting": "agent.execution.waiting",
  "execution.completed": "agent.execution.completed",
  "execution.failed": "agent.execution.failed",
  "execution.cancelled": "agent.execution.cancelled",
  "runtime.unknown": "agent.runtime.unknown"
};

export function toCodeWeaverEvent(event: AgentRuntimeEvent): CodeWeaverEvent {
  return {
    id: event.eventId,
    type: TYPE_MAP[event.eventType] ?? "agent.runtime.unknown",
    projectId: event.projectId,
    taskId: event.taskId,
    executionId: event.executionId,
    sequence: event.sequence,
    timestamp: event.timestamp,
    data: event.payload
  };
}

/** Replays persisted journal entries into the same envelope, for reconnect catch-up. */
export function journalEntryToEvent(entry: JournalEntry): CodeWeaverEvent {
  const type = entry.kind === "audit" ? `codeweaver.${entry.category}` : `agent.${entry.category}`;
  return {
    id: `${entry.executionId}:${entry.sequence}`,
    type,
    projectId: typeof entry.payload["projectId"] === "string" ? entry.payload["projectId"] : "",
    executionId: entry.executionId,
    sequence: entry.sequence,
    timestamp: entry.recordedAt,
    data: entry.payload
  };
}

export type EventListener = (event: CodeWeaverEvent) => void;

/**
 * Fan-out for live subscribers.
 *
 * The bus is not the source of truth: design 13 requires the server to persist events and the
 * socket to be a notification channel only. A client that missed events replays from the journal
 * using `since`.
 */
export class EventBus {
  private readonly listeners = new Set<EventListener>();

  subscribe(listener: EventListener): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  publish(event: CodeWeaverEvent): void {
    for (const listener of this.listeners) {
      try {
        listener(event);
      } catch {
        // A broken SSE/WS client must not stop delivery to the others.
      }
    }
  }

  publishRuntimeEvent(event: AgentRuntimeEvent): void {
    this.publish(toCodeWeaverEvent(event));
  }

  get size(): number {
    return this.listeners.size;
  }
}

/** Formats one SSE frame; the event id lets browsers resume with Last-Event-ID. */
export function formatSseFrame(event: CodeWeaverEvent): string {
  const payload = JSON.stringify(event);
  return `id: ${event.id}\nevent: ${event.type}\ndata: ${payload}\n\n`;
}

export function newEventId(): string {
  return `evt_${randomUUID()}`;
}
