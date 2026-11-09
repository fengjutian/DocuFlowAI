import { readFile, readdir, appendFile, mkdir } from "node:fs/promises";
import { join } from "node:path";

/**
 * Append-only execution journal (design 14.3 / 15).
 *
 * Entries are never updated or deleted: a correction is a new entry. The journal is the durable
 * half of event compensation — the adapter's in-memory buffer replays what it saw, the journal
 * answers what was actually recorded.
 */
export type JournalEntryKind = "runtime.event" | "audit";

export interface JournalEntry {
  executionId: string;
  /** Journal position, allocated by the writer. Unique per execution; gaps are allowed. */
  sequence: number;
  /** Runtime event sequence, when the entry came from the runtime event stream. */
  eventSequence?: number;
  kind: JournalEntryKind;
  /** Dotted category, for example `execution.completed` or `change.applied`. */
  category: string;
  recordedAt: string;
  payload: Record<string, unknown>;
}

export interface JournalStore {
  append(entry: JournalEntry): Promise<void>;
  read(executionId: string): Promise<JournalEntry[]>;
  listExecutions(): Promise<string[]>;
}

export class JournalConflictError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "JournalConflictError";
  }
}

export function assertValidEntry(entry: JournalEntry): void {
  if (!entry.executionId || entry.executionId.trim() === "") {
    throw new JournalConflictError("Journal entry requires a non-empty executionId");
  }
  if (!Number.isInteger(entry.sequence) || entry.sequence < 1) {
    throw new JournalConflictError(`Journal sequence must be a positive integer, received '${entry.sequence}'`);
  }
  if (entry.eventSequence !== undefined && (!Number.isInteger(entry.eventSequence) || entry.eventSequence < 1)) {
    throw new JournalConflictError(
      `Journal eventSequence must be a positive integer, received '${String(entry.eventSequence)}'`
    );
  }
  if (entry.kind !== "runtime.event" && entry.kind !== "audit") {
    throw new JournalConflictError(`Unknown journal entry kind '${String(entry.kind)}'`);
  }
  if (!entry.category || entry.category.trim() === "") {
    throw new JournalConflictError("Journal entry requires a non-empty category");
  }
  if (Number.isNaN(Date.parse(entry.recordedAt))) {
    throw new JournalConflictError(`Journal recordedAt is not a timestamp: '${entry.recordedAt}'`);
  }
  if (entry.payload === null || typeof entry.payload !== "object" || Array.isArray(entry.payload)) {
    throw new JournalConflictError("Journal payload must be an object");
  }
}

export function sortEntries(entries: readonly JournalEntry[]): JournalEntry[] {
  return [...entries].sort((a, b) => a.sequence - b.sequence);
}

/** Sequence numbers missing between the lowest recorded sequence and the highest. */
export function detectGaps(entries: readonly JournalEntry[], fromSequence = 1): number[] {
  const present = new Set(entries.map(entry => entry.sequence));
  if (present.size === 0) return [];
  const highest = Math.max(...present);
  const gaps: number[] = [];
  for (let sequence = fromSequence; sequence <= highest; sequence += 1) {
    if (!present.has(sequence)) gaps.push(sequence);
  }
  return gaps;
}

/**
 * Runtime event sequences that never reached the journal.
 *
 * Journal position and runtime order are different things: audit entries advance the journal but
 * carry no runtime event. Compensation must look at the runtime sequence, otherwise an audit write
 * would look like a missing event.
 */
export function detectEventGaps(entries: readonly JournalEntry[], fromSequence = 1): number[] {
  const present = new Set(
    entries
      .map(entry => entry.eventSequence)
      .filter((sequence): sequence is number => sequence !== undefined)
  );
  if (present.size === 0) return [];
  const highest = Math.max(...present);
  const gaps: number[] = [];
  for (let sequence = fromSequence; sequence <= highest; sequence += 1) {
    if (!present.has(sequence)) gaps.push(sequence);
  }
  return gaps;
}

export class InMemoryJournalStore implements JournalStore {
  private readonly entries = new Map<string, JournalEntry[]>();

  async append(entry: JournalEntry): Promise<void> {
    assertValidEntry(entry);
    const existing = this.entries.get(entry.executionId) ?? [];
    if (existing.some(item => item.sequence === entry.sequence)) {
      throw new JournalConflictError(
        `Sequence ${entry.sequence} already exists for execution ${entry.executionId}`
      );
    }
    this.entries.set(entry.executionId, sortEntries([...existing, entry]));
  }

  async read(executionId: string): Promise<JournalEntry[]> {
    return sortEntries(this.entries.get(executionId) ?? []);
  }

  async listExecutions(): Promise<string[]> {
    return [...this.entries.keys()].sort();
  }
}

/**
 * One JSONL file per execution under `root`. Append-only by construction: the store opens in
 * append mode and never rewrites a line. A malformed line is reported with its number instead of
 * being skipped silently, because a silently skipped audit line is worse than a loud failure.
 */
export class JsonlJournalStore implements JournalStore {
  constructor(private readonly root: string) {}

  async append(entry: JournalEntry): Promise<void> {
    assertValidEntry(entry);
    const existing = await this.read(entry.executionId);
    if (existing.some(item => item.sequence === entry.sequence)) {
      throw new JournalConflictError(
        `Sequence ${entry.sequence} already exists for execution ${entry.executionId}`
      );
    }
    await mkdir(this.root, { recursive: true });
    await appendFile(this.pathFor(entry.executionId), `${JSON.stringify(entry)}\n`, "utf8");
  }

  async read(executionId: string): Promise<JournalEntry[]> {
    let content: string;
    try {
      content = await readFile(this.pathFor(executionId), "utf8");
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === "ENOENT") return [];
      throw error;
    }
    const entries: JournalEntry[] = [];
    const lines = content.split(/\r?\n/);
    for (const [index, line] of lines.entries()) {
      if (!line.trim()) continue;
      let parsed: unknown;
      try {
        parsed = JSON.parse(line);
      } catch {
        throw new JournalConflictError(
          `Journal for ${executionId} has malformed JSON at line ${index + 1}`
        );
      }
      const entry = parsed as JournalEntry;
      assertValidEntry(entry);
      entries.push(entry);
    }
    return sortEntries(entries);
  }

  async listExecutions(): Promise<string[]> {
    try {
      const names = await readdir(this.root);
      return names.filter(name => name.endsWith(".jsonl")).map(name => name.slice(0, -".jsonl".length)).sort();
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === "ENOENT") return [];
      throw error;
    }
  }

  private pathFor(executionId: string): string {
    if (!/^[a-zA-Z0-9._-]+$/.test(executionId)) {
      throw new JournalConflictError(`Execution id '${executionId}' is not safe as a file name`);
    }
    return join(this.root, `${executionId}.jsonl`);
  }
}

/**
 * Merges re-fetched events into what is already recorded. Duplicate sequences are deduplicated;
 * the same sequence carrying different content is a conflict, not a silent overwrite.
 */
export function mergeCompensated(
  existing: readonly JournalEntry[],
  incoming: readonly JournalEntry[]
): JournalEntry[] {
  const bySequence = new Map<number, JournalEntry>();
  for (const entry of [...existing, ...incoming]) {
    assertValidEntry(entry);
    const current = bySequence.get(entry.sequence);
    if (!current) {
      bySequence.set(entry.sequence, entry);
      continue;
    }
    if (current.category !== entry.category || JSON.stringify(current.payload) !== JSON.stringify(entry.payload)) {
      throw new JournalConflictError(
        `Conflicting journal entries at sequence ${entry.sequence} for execution ${entry.executionId}`
      );
    }
  }
  return [...bySequence.values()].sort((a, b) => a.sequence - b.sequence);
}
