import type { AgentRuntimeEvent } from "../types/adapter.js";
import type { JournalEntry, JournalStore } from "./journal.js";

/**
 * Records a normalized runtime event in the execution journal.
 *
 * `journalSequence` is the journal position allocated by the writer; the runtime's own sequence is
 * kept separately as `eventSequence`, because audit entries advance the journal without consuming a
 * runtime event number. Mixing the two would make an audit write look like a lost event.
 */
export async function recordRuntimeEvent(
  store: JournalStore,
  event: AgentRuntimeEvent,
  journalSequence: number
): Promise<void> {
  await store.append({
    executionId: event.executionId,
    sequence: journalSequence,
    eventSequence: event.sequence,
    kind: "runtime.event",
    category: event.eventType,
    recordedAt: event.timestamp,
    payload: {
      ...event.payload,
      eventId: event.eventId,
      ...(event.externalEventId ? { externalEventId: event.externalEventId } : {})
    }
  });
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
