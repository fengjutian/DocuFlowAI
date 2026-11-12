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
  /**
   * What the checks themselves printed.
   *
   * A check that passes says nothing in the tool's text - the text carries the verdict, and the
   * reason only for a check that failed. The output of a passing check is the only place where
   * "the check really did read that directory" is visible, so it is read from the details.
   */
  checkOutputs?: string[];
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
        | {
            content?: Array<{ text?: string }>;
            details?: { denied?: boolean; reason?: string; checks?: Array<{ output?: string }> };
          }
        | undefined;
      const checkOutputs = (result?.details?.checks ?? [])
        .map(check => check.output ?? "")
        .filter(output => output.trim().length > 0);
      results.push({
        tool: String(payload["toolName"] ?? "unknown"),
        text: (result?.content ?? []).map(part => part.text ?? "").join("\n"),
        denied: result?.details?.denied === true,
        ...(result?.details?.reason ? { reason: result.details.reason } : {}),
        ...(checkOutputs.length > 0 ? { checkOutputs } : {})
      });
      continue;
    }
    if (entry.category === "message.delta" && typeof payload["delta"] === "string") {
      answer += payload["delta"];
    }
  }
  return { answer, calls, results, journalText: JSON.stringify(entries) };
}
