import type { TestResult } from "../types/hermes.js";

export const phase0Checks = [
  ["P0-001", "HTTP create run"],
  ["P0-002", "HTTP SSE event stream"],
  ["P0-003", "TUI Gateway session/prompt"],
  ["P0-004", "ACP editor connection"],
  ["P0-005", "Single-step task"],
  ["P0-006", "Multi-tool task"],
  ["P0-007", "Multi-turn session"],
  ["P0-008", "Subagent delegation"],
  ["P0-009", "Allowed file read"],
  ["P0-010", "Restricted file denied"],
  ["P0-011", "Unauthorized write blocked"],
  ["P0-012", "Dangerous command blocked/approved"],
  ["P0-013", "Unauthorized network blocked"],
  ["P0-014", "Parallel Agent runs"],
  ["P0-015", "Worktree isolation"],
  ["P0-016", "Sandbox resource limits"],
  ["P0-017", "Host path protection"],
  ["P0-018", "Cancellation"],
  ["P0-019", "Client disconnect"],
  ["P0-020", "Runtime crash"],
  ["P0-021", "Session resume/rebuild"],
  ["P0-022", "Audit timeline"],
  ["P0-023", "Token/model/time accounting"],
  ["P0-024", "End-to-end traceability"]
] as const;

export function mergePhase0Results(
  partialResults: readonly TestResult[],
  generatedAt = new Date()
): TestResult[] {
  const knownIds = new Set<string>(phase0Checks.map(([id]) => id));
  const latestById = new Map<string, TestResult>();
  for (const item of partialResults) {
    if (!knownIds.has(item.id)) throw new Error(`Unknown Phase 0 check id: ${item.id}`);
    latestById.set(item.id, item);
  }
  const timestamp = generatedAt.toISOString();
  return phase0Checks.map(([id, title]) => latestById.get(id) ?? {
    id,
    title,
    status: "not_run",
    startedAt: timestamp,
    finishedAt: timestamp,
    evidence: [],
    notes: "No evidence report was supplied"
  });
}
