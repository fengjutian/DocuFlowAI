import { applyEdits, modify, parse, printParseErrorCode, type ParseError } from "jsonc-parser";

/**
 * Zed external-agent registration.
 *
 * Zed's settings file is JSONC (comments and trailing commas are normal), so edits are applied through
 * a JSONC-aware modifier that preserves the user's comments and formatting. Merging must never destroy
 * unrelated settings, and must be idempotent so running the installer twice is harmless.
 */
export interface AgentServerEntry {
  command: string;
  args?: string[];
  env?: Record<string, string>;
}

export interface MergeResult {
  settings: Record<string, unknown>;
  changed: boolean;
}

export interface TextEditResult {
  text: string;
  changed: boolean;
}

export function mergeAgentServer(
  settings: Record<string, unknown>,
  name: string,
  entry: AgentServerEntry
): MergeResult {
  if (!name.trim()) throw new Error("Agent server name must not be empty");
  if (!entry.command.trim()) throw new Error("Agent server command must not be empty");

  const existing = (settings["agent_servers"] ?? {}) as Record<string, unknown>;
  const current = existing[name] as AgentServerEntry | undefined;
  const normalized = normalizeEntry(entry);

  if (current && sameEntry(current, normalized)) {
    return { settings, changed: false };
  }

  return {
    settings: {
      ...settings,
      agent_servers: { ...existing, [name]: normalized }
    },
    changed: true
  };
}

export function removeAgentServer(settings: Record<string, unknown>, name: string): MergeResult {
  const existing = (settings["agent_servers"] ?? {}) as Record<string, unknown>;
  if (!(name in existing)) return { settings, changed: false };
  const next = { ...existing };
  delete next[name];
  return {
    settings: { ...settings, agent_servers: next },
    changed: true
  };
}

/**
 * Applies the agent entry to the raw settings text, preserving comments, key order and indentation.
 * Throws when the file is not valid JSONC instead of silently replacing the user's file.
 */
export function applyAgentServerToText(
  text: string,
  name: string,
  entry: AgentServerEntry
): TextEditResult {
  if (!name.trim()) throw new Error("Agent server name must not be empty");
  if (!entry.command.trim()) throw new Error("Agent server command must not be empty");

  if (text.trim() === "") {
    return {
      text: `${JSON.stringify({ agent_servers: { [name]: normalizeEntry(entry) } }, null, 2)}\n`,
      changed: true
    };
  }

  const errors: ParseError[] = [];
  const settings = parse(text, errors, { allowTrailingComma: true, disallowComments: false }) as
    | Record<string, unknown>
    | undefined;
  if (errors.length > 0) {
    const first = errors[0]!;
    throw new Error(
      `settings are not valid JSONC (${printParseErrorCode(first.error)} at offset ${first.offset})`
    );
  }
  if (!settings || typeof settings !== "object" || Array.isArray(settings)) {
    throw new Error("settings must contain a JSON object");
  }

  const existing = (settings["agent_servers"] ?? {}) as Record<string, unknown>;
  const current = existing[name] as AgentServerEntry | undefined;
  const normalized = normalizeEntry(entry);
  if (current && sameEntry(current, normalized)) return { text, changed: false };

  const edits = modify(text, ["agent_servers", name], normalized, {
    formattingOptions: { insertSpaces: true, tabSize: 2 }
  });
  return { text: applyEdits(text, edits), changed: true };
}

export function removeAgentServerFromText(text: string, name: string): TextEditResult {
  const settings = parse(text, [], { allowTrailingComma: true, disallowComments: false }) as
    | Record<string, unknown>
    | undefined;
  const existing = (settings?.["agent_servers"] ?? {}) as Record<string, unknown>;
  if (!(name in existing)) return { text, changed: false };

  const edits = modify(text, ["agent_servers", name], undefined, {
    formattingOptions: { insertSpaces: true, tabSize: 2 }
  });
  return { text: applyEdits(text, edits), changed: true };
}

function normalizeEntry(entry: AgentServerEntry): AgentServerEntry {
  return {
    command: entry.command,
    ...(entry.args && entry.args.length > 0 ? { args: [...entry.args] } : {}),
    ...(entry.env && Object.keys(entry.env).length > 0 ? { env: { ...entry.env } } : {})
  };
}

function sameEntry(a: AgentServerEntry, b: AgentServerEntry): boolean {
  return a.command === b.command
    && JSON.stringify(a.args ?? []) === JSON.stringify(b.args ?? [])
    && JSON.stringify(a.env ?? {}) === JSON.stringify(b.env ?? {});
}
