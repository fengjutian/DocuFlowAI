import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { mergePhase0Results } from "../reporting/phase0-checklist.js";
import { writeTestReport } from "../reporting/report-writer.js";
import type { TestResult, TestStatus } from "../types/hermes.js";

// The agent-security report maps onto P0-009/010/011/013. It is included by default so a probe
// that ran and was blocked is reported as `blocked` rather than `not_run`: the two mean different
// things and only the first one tells a reader that the probe was actually attempted.
// The security-boundary report is not merged: its SEC-B* ids are a separate series.
const defaultInputs = [
  "reports/phase0-http.json",
  "reports/phase0-gateway.json",
  "reports/phase0-agent-security.json",
  "reports/phase0-pi-cancellation.json",
  "reports/phase0-pi-session-recovery.json",
  "reports/phase0-pi-subagent.json",
  "reports/phase0-pi-agent-execution.json",
  "reports/phase0-pi-multi-turn.json",
  "reports/phase0-pi-traceability.json",
  "reports/phase0-pi-client-disconnect.json",
  "reports/phase0-runtime-crash.json",
  "reports/phase0-audit-timeline.json",
  "reports/phase0-accounting.json",
  "reports/phase0-parallel-agents.json",
  "reports/phase0-worktree-isolation.json",
  "reports/phase0-command-approval.json",
  // The editor's own evidence: the editor's ACP client drives the real endpoint. It lives in the
  // editor crate because the client is the editor's code, and it lands here in the same shape as
  // every other Phase 0 report.
  "reports/phase0-acp-editor.json",
  // The sandbox limits: the acceptance checks run inside a Windows job object with memory, CPU
  // and process limits, and the probe shows the limits biting.
  "reports/phase0-sandbox-limits.json",
  // The host path protection: the acceptance check runs inside a WSL2 boundary where the host
  // drives are not mounted, and the probe shows the agent being told why.
  "reports/phase0-host-path-protection.json"
];

async function main(): Promise<void> {
  const inputs = process.env.PHASE0_REPORT_INPUTS
    ? parseInputPaths(process.env.PHASE0_REPORT_INPUTS)
    : defaultInputs;
  const output = resolve(process.env.PHASE0_REPORT_PATH ?? "reports/phase0.json");
  const partialResults: TestResult[] = [];
  for (const input of inputs) partialResults.push(...await readResultsIfPresent(resolve(input)));
  const results = mergePhase0Results(partialResults);
  await writeTestReport(output, results);
  console.log(`Consolidated Phase 0 report: ${output}`);
  for (const status of ["passed", "failed", "blocked", "not_run"] as const) {
    console.log(`${status}: ${results.filter(item => item.status === status).length}`);
  }
  if (results.some(item => item.status === "failed")) process.exitCode = 1;
}

async function readResultsIfPresent(path: string): Promise<TestResult[]> {
  let text: string;
  try {
    text = await readFile(path, "utf8");
  } catch (error) {
    if (isNodeError(error) && error.code === "ENOENT") return [];
    throw error;
  }
  const document: unknown = JSON.parse(text);
  if (!isRecord(document) || !Array.isArray(document.results)) {
    throw new Error(`Invalid report document: ${path}`);
  }
  return document.results.map((item, index) => validateResult(item, `${path} results[${index}]`));
}

function validateResult(value: unknown, location: string): TestResult {
  if (!isRecord(value) ||
      typeof value.id !== "string" ||
      typeof value.title !== "string" ||
      !isStatus(value.status) ||
      typeof value.startedAt !== "string" ||
      typeof value.finishedAt !== "string" ||
      !Array.isArray(value.evidence) ||
      !value.evidence.every(item => typeof item === "string") ||
      (value.notes !== undefined && typeof value.notes !== "string")) {
    throw new Error(`Invalid test result at ${location}`);
  }
  return {
    id: value.id,
    title: value.title,
    status: value.status,
    startedAt: value.startedAt,
    finishedAt: value.finishedAt,
    evidence: value.evidence,
    ...(typeof value.notes === "string" ? { notes: value.notes } : {})
  };
}

function parseInputPaths(value: string): string[] {
  const parsed: unknown = JSON.parse(value);
  if (!Array.isArray(parsed) || !parsed.every(item => typeof item === "string")) {
    throw new Error("PHASE0_REPORT_INPUTS must be a JSON array of paths");
  }
  return parsed;
}

function isStatus(value: unknown): value is TestStatus {
  return value === "passed" || value === "failed" || value === "blocked" || value === "not_run";
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isNodeError(value: unknown): value is NodeJS.ErrnoException {
  return value instanceof Error && "code" in value;
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : undefined;
if (invokedPath === import.meta.url) await main();
