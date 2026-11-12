import { appendFile, mkdir, readFile } from "node:fs/promises";
import { join } from "node:path";
import { randomUUID } from "node:crypto";

/**
 * The audit trail of runtime configuration changes (V0.3.0 / DF-0304).
 *
 * This log is deliberately *not* the execution journal. A configuration change belongs to no
 * execution, and writing it into the journal would make it look like one: the score report counts
 * executions without a runtime attribution, and the audit timeline builds one entry per execution.
 * A config event smuggled in there would show up as an unattributed run.
 *
 * The log is append-only and a malformed line is reported with its number rather than skipped: a
 * silently dropped audit line is worse than a loud failure.
 */

export interface RuntimeConfigChange {
  /** The configuration field that changed, for example `enabled` or `priority`. */
  field: string;
  from: unknown;
  to: unknown;
}

export interface RuntimeConfigAuditEntry {
  id: string;
  recordedAt: string;
  /** Who made the change: `console`, `api` or `system` for the startup restore. */
  actor: string;
  /** What was done: `enable`, `disable`, `update`, `default` or `restore`. */
  action: string;
  /** The runtime the change is about. */
  runtimeId: string;
  /** The before/after values, so the log says what actually changed. */
  changes: RuntimeConfigChange[];
  note?: string;
}

export class RuntimeConfigAuditError extends Error {
  constructor(
    message: string,
    public readonly path: string,
    public readonly line: number
  ) {
    super(message);
    this.name = "RuntimeConfigAuditError";
  }
}

export class RuntimeConfigAuditLog {
  constructor(private readonly root: string, private readonly fileName = "runtime-config.jsonl") {}

  get path(): string {
    return join(this.root, "audit", this.fileName);
  }

  async append(entry: RuntimeConfigAuditEntry): Promise<RuntimeConfigAuditEntry> {
    await mkdir(join(this.root, "audit"), { recursive: true });
    await appendFile(this.path, `${JSON.stringify(entry)}\n`, "utf8");
    return entry;
  }

  /** The recorded changes, newest first. */
  async list(limit = 100): Promise<RuntimeConfigAuditEntry[]> {
    let content: string;
    try {
      content = await readFile(this.path, "utf8");
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === "ENOENT") return [];
      throw error;
    }
    const entries: RuntimeConfigAuditEntry[] = [];
    const lines = content.split(/\r?\n/);
    for (const [index, line] of lines.entries()) {
      if (!line.trim()) continue;
      let parsed: unknown;
      try {
        parsed = JSON.parse(line);
      } catch {
        throw new RuntimeConfigAuditError(
          `Runtime configuration audit log at ${this.path} has malformed JSON at line ${index + 1}`,
          this.path,
          index + 1
        );
      }
      entries.push(parsed as RuntimeConfigAuditEntry);
    }
    return entries.reverse().slice(0, limit);
  }
}

/** A fresh audit entry with its identity filled in. */
export function newRuntimeConfigAuditEntry(input: {
  actor: string;
  action: string;
  runtimeId: string;
  changes: RuntimeConfigChange[];
  note?: string;
  recordedAt?: string;
  id?: string;
}): RuntimeConfigAuditEntry {
  return {
    id: input.id ?? `rcfg_${randomUUID()}`,
    recordedAt: input.recordedAt ?? new Date().toISOString(),
    actor: input.actor,
    action: input.action,
    runtimeId: input.runtimeId,
    changes: input.changes,
    ...(input.note ? { note: input.note } : {})
  };
}
