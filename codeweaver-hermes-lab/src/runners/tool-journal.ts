/**
 * What an execution's journal says about the tools it used.
 *
 * Two probes need to answer the same question — "did the agent really call the tool, and what
 * came back?" — so they read the journal the same way.
 */

export interface ToolJournalEntry {
  category: string;
  payload?: Record<string, unknown>;
}

export interface ToolCall {
  tool: string;
  args: Record<string, unknown>;
}

export interface ToolResult {
  tool: string;
  text: string;
  denied: boolean;
  reason?: string;
}

export interface ToolFacts {
  /** Everything the agent said, joined. */
  answer: string;
  /** The tool calls the agent really made. */
  calls: ToolCall[];
  /** The results the agent really got. */
  results: ToolResult[];
  /** The whole journal, as text: a leak anywhere in it is a leak. */
  journalText: string;
}

export function collectToolFacts(entries: readonly ToolJournalEntry[]): ToolFacts {
  const calls: ToolCall[] = [];
  const results: ToolResult[] = [];
  let answer = "";
  for (const entry of entries) {
    const payload = entry.payload ?? {};
    if (entry.category === "tool.started") {
      calls.push({
        tool: String(payload["toolName"] ?? "unknown"),
        args: (payload["args"] as Record<string, unknown>) ?? {}
      });
      continue;
    }
    if (entry.category === "tool.completed" || entry.category === "tool.failed") {
      const result = payload["result"] as
        | { content?: Array<{ text?: string }>; details?: { denied?: boolean; reason?: string } }
        | undefined;
      results.push({
        tool: String(payload["toolName"] ?? "unknown"),
        text: (result?.content ?? []).map(part => part.text ?? "").join("\n"),
        denied: result?.details?.denied === true,
        ...(result?.details?.reason ? { reason: result.details.reason } : {})
      });
      continue;
    }
    if (entry.category === "message.delta" && typeof payload["delta"] === "string") {
      answer += payload["delta"];
    }
  }
  return { answer, calls, results, journalText: JSON.stringify(entries) };
}
