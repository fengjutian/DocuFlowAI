import { mkdir, readFile, rename, unlink, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";

/**
 * The persisted runtime configuration (V0.3.0 / DF-0301).
 *
 * A runtime's place in the product — is it on, how high is its tier, what is it called — is an
 * operator decision, and an operator decision that does not survive a restart is not a decision.
 * This file is where that decision lives.
 *
 * Two rules:
 * 1. The write is atomic: a temporary file is written and then renamed over the target, so a
 *    process that dies mid-write leaves the previous configuration intact instead of half a file.
 * 2. The read is strict: every field is validated, and an invalid file is an error with a path and
 *    a reason. A configuration that cannot be understood is never partially applied (DF-0302).
 */

export const RUNTIME_CONFIG_VERSION = 1;

export interface RuntimeConfigEntry {
  /** Disabled runtimes stay visible but receive no new work. */
  enabled: boolean;
  /** Scheduling tier. Only the highest tier among the candidates is eligible. */
  priority: number;
  /** Free-form labels the operator attached to this runtime. */
  tags: string[];
}

export interface RuntimeConfigDocument {
  version: number;
  /** The runtime that unnamed work goes to, when one is configured. */
  defaultRuntimeId?: string;
  runtimes: Record<string, RuntimeConfigEntry>;
}

/**
 * A configuration that could not be read or could not be understood.
 *
 * The path and the reason are part of the error: "the config is broken" is not actionable, and the
 * server fails closed on this error (DF-0302), so the diagnostic is the only thing the operator has.
 */
export class RuntimeConfigError extends Error {
  constructor(
    message: string,
    public readonly path: string,
    public readonly reason: string
  ) {
    super(message);
    this.name = "RuntimeConfigError";
  }
}

export function emptyRuntimeConfig(): RuntimeConfigDocument {
  return { version: RUNTIME_CONFIG_VERSION, runtimes: {} };
}

export function defaultRuntimeConfigEntry(): RuntimeConfigEntry {
  return { enabled: true, priority: 0, tags: [] };
}

/**
 * Parses and validates a configuration document.
 *
 * The parser is the only place that decides what a valid configuration is; the store and the
 * service both go through it, so a file that the store writes is a file the store can read.
 */
export function parseRuntimeConfig(raw: string, path: string): RuntimeConfigDocument {
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch (error) {
    throw new RuntimeConfigError(
      `Runtime configuration at ${path} is not valid JSON: ${error instanceof Error ? error.message : String(error)}`,
      path,
      "invalid_json"
    );
  }
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new RuntimeConfigError(
      `Runtime configuration at ${path} must be a JSON object`,
      path,
      "not_an_object"
    );
  }
  const document = parsed as Record<string, unknown>;
  if (document["version"] !== RUNTIME_CONFIG_VERSION) {
    throw new RuntimeConfigError(
      `Runtime configuration at ${path} has version ${JSON.stringify(document["version"])}, but this build reads version ${RUNTIME_CONFIG_VERSION}`,
      path,
      "unsupported_version"
    );
  }
  const runtimes = document["runtimes"];
  if (!runtimes || typeof runtimes !== "object" || Array.isArray(runtimes)) {
    throw new RuntimeConfigError(
      `Runtime configuration at ${path} must have a 'runtimes' object`,
      path,
      "invalid_runtimes"
    );
  }
  const entries: Record<string, RuntimeConfigEntry> = {};
  for (const [runtimeId, value] of Object.entries(runtimes as Record<string, unknown>)) {
    entries[runtimeId] = parseEntry(value, runtimeId, path);
  }
  const defaultRuntimeId = document["defaultRuntimeId"];
  if (defaultRuntimeId !== undefined && (typeof defaultRuntimeId !== "string" || defaultRuntimeId.trim() === "")) {
    throw new RuntimeConfigError(
      `Runtime configuration at ${path} has a 'defaultRuntimeId' that is not a non-empty string`,
      path,
      "invalid_default_runtime"
    );
  }
  return {
    version: RUNTIME_CONFIG_VERSION,
    ...(typeof defaultRuntimeId === "string" ? { defaultRuntimeId } : {}),
    runtimes: entries
  };
}

function parseEntry(value: unknown, runtimeId: string, path: string): RuntimeConfigEntry {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new RuntimeConfigError(
      `Runtime configuration at ${path} has an entry for '${runtimeId}' that is not an object`,
      path,
      "invalid_runtime_entry"
    );
  }
  const entry = value as Record<string, unknown>;
  const enabled = entry["enabled"];
  if (typeof enabled !== "boolean") {
    throw new RuntimeConfigError(
      `Runtime configuration at ${path} has a non-boolean 'enabled' for '${runtimeId}'`,
      path,
      "invalid_enabled"
    );
  }
  const priority = entry["priority"];
  if (typeof priority !== "number" || !Number.isFinite(priority)) {
    throw new RuntimeConfigError(
      `Runtime configuration at ${path} has a non-numeric 'priority' for '${runtimeId}'`,
      path,
      "invalid_priority"
    );
  }
  const tags = entry["tags"];
  if (!Array.isArray(tags) || tags.some(tag => typeof tag !== "string")) {
    throw new RuntimeConfigError(
      `Runtime configuration at ${path} has a 'tags' value for '${runtimeId}' that is not an array of strings`,
      path,
      "invalid_tags"
    );
  }
  return { enabled, priority, tags: [...(tags as string[])] };
}

/**
 * The runtime configuration on disk.
 *
 * A missing file is not an error: a server that has never been configured starts with an empty
 * configuration. A file that exists but cannot be read is an error, and it stops the server.
 */
export class RuntimeConfigStore {
  constructor(private readonly root: string, private readonly fileName = "runtimes.json") {}

  get path(): string {
    return join(this.root, "config", this.fileName);
  }

  async load(): Promise<RuntimeConfigDocument> {
    let raw: string;
    try {
      raw = await readFile(this.path, "utf8");
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === "ENOENT") return emptyRuntimeConfig();
      throw new RuntimeConfigError(
        `Runtime configuration at ${this.path} could not be read: ${error instanceof Error ? error.message : String(error)}`,
        this.path,
        "unreadable"
      );
    }
    return parseRuntimeConfig(raw, this.path);
  }

  /** Writes the configuration atomically: a temporary file, then a rename over the target. */
  async save(document: RuntimeConfigDocument): Promise<void> {
    const target = this.path;
    const temporary = `${target}.tmp`;
    await mkdir(dirname(target), { recursive: true });
    try {
      await writeFile(temporary, `${JSON.stringify(document, null, 2)}\n`, "utf8");
      await rename(temporary, target);
    } catch (error) {
      // The temporary file is the only thing that may be left behind; the target keeps its content.
      await unlink(temporary).catch(() => undefined);
      throw new RuntimeConfigError(
        `Runtime configuration at ${target} could not be written: ${error instanceof Error ? error.message : String(error)}`,
        target,
        "unwritable"
      );
    }
  }
}
