/**
 * Zed external-agent registration.
 *
 * Zed reads `agent_servers` from its settings file; DocuFlow's ACP endpoint is launched by Zed as a
 * child process. Merging must never destroy unrelated settings, and must be idempotent so running the
 * installer twice is harmless.
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

export function mergeAgentServer(
  settings: Record<string, unknown>,
  name: string,
  entry: AgentServerEntry
): MergeResult {
  if (!name.trim()) throw new Error("Agent server name must not be empty");
  if (!entry.command.trim()) throw new Error("Agent server command must not be empty");

  const existing = (settings["agent_servers"] ?? {}) as Record<string, unknown>;
  const current = existing[name] as AgentServerEntry | undefined;
  const normalized: AgentServerEntry = {
    command: entry.command,
    ...(entry.args && entry.args.length > 0 ? { args: [...entry.args] } : {}),
    ...(entry.env && Object.keys(entry.env).length > 0 ? { env: { ...entry.env } } : {})
  };

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

function sameEntry(a: AgentServerEntry, b: AgentServerEntry): boolean {
  return a.command === b.command
    && JSON.stringify(a.args ?? []) === JSON.stringify(b.args ?? [])
    && JSON.stringify(a.env ?? {}) === JSON.stringify(b.env ?? {});
}
