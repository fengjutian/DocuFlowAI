import type { AgentRuntimeEvent } from "../types/adapter.js";
import type { JournalEntry, JournalStore } from "./journal.js";

/**
 * Records a normalized runtime event in the execution journal.
 *
 * The journal reuses the event's own sequence so the journal order and the runtime order are the
 * same thing; the event id and external id travel in the payload so a duplicate delivery can be
 * recognized after a restart.
 */
export async function recordRuntimeEvent(store: JournalStore, event: AgentRuntimeEvent): Promise<void> {
  await store.append({
    executionId: event.executionId,
    sequence: event.sequence,
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
  /** Next position in the execution timeline; the caller owns allocation. */
  sequence: number;
  category: string;
  payload: Record<string, unknown>;
  recordedAt?: string;
}

/** Records a DocuFlow-side fact (change applied, policy denied, approval granted) in the journal. */
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
