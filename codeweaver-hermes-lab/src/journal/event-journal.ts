import type { AgentRuntimeEvent } from "../types/adapter.js";
import type { JournalEntry, JournalStore } from "./journal.js";

/**
 * Records a normalized runtime event in the execution journal.
 *
 * `journalSequence` is the journal position allocated by the writer; the runtime's own sequence is
 * kept separately as `eventSequence`, because audit entries advance the journal without consuming a
 * runtime event number. Mixing the two would make an audit write look like a lost event.
 *
 * The written entry is returned so the caller can publish *what was recorded*. The live stream and
 * the replay then speak the same sequence space — a client that reconnects with `since` can trust
 * the number it was given (design 13: "断线后根据 executionId 和 sequence 补齐事件").
 */
export async function recordRuntimeEvent(
  store: JournalStore,
  event: AgentRuntimeEvent,
  journalSequence: number
): Promise<JournalEntry> {
  const entry: JournalEntry = {
    executionId: event.executionId,
    sequence: journalSequence,
    eventSequence: event.sequence,
    kind: "runtime.event",
    category: event.eventType,
    recordedAt: event.timestamp,
    payload: {
      ...event.payload,
      eventId: event.eventId,
      // The envelope needs to know which execution and task the event belongs to, and the replayed
      // frames are built from the payload alone. The runtime event carries both outside its payload.
      projectId: event.projectId,
      ...(event.taskId ? { taskId: event.taskId } : {}),
      ...(event.externalEventId ? { externalEventId: event.externalEventId } : {})
    }
  };
  await store.append(entry);
  return entry;
}

export interface AuditEntryInput {
  executionId: string;
  /** Next journal position; the caller owns allocation. */
  sequence: number;
  category: string;
  payload: Record<string, unknown>;
  recordedAt?: string;
}

/** Records a CodeWeaver-side fact (change applied, policy denied, approval granted) in the journal. */
export async function recordAudit(store: JournalStore, input: AuditEntryInput): Promise<void> {
  const entry: JournalEntry = {
    executionId: input.executionId,
    sequence: input.sequence,
    kind: "audit",
    category: input.category,
    recordedAt: input.recordedAt ?? new Date().toISOString(),
    payload: input.payload
  };
  await store.append(entry);
}

/** Runtime event numbers already journaled, used to make compensation idempotent. */
export function journaledEventSequences(entries: readonly JournalEntry[]): Set<number> {
  return new Set(
    entries
      .map(entry => entry.eventSequence)
      .filter((sequence): sequence is number => sequence !== undefined)
  );
}
