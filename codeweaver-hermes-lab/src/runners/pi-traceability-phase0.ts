import { spawnSync } from "node:child_process";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { createApiServer } from "../api/app.js";
import { writeTestReport } from "../reporting/report-writer.js";
import { createRuntime } from "../server/runtime.js";
import type { TestResult } from "../types/hermes.js";

/**
 * P0-024: end-to-end traceability, on the Pi runtime and a real model.
 *
 * The checklist asks for Task → Run → Tool → Change → Commit → Test. That is the chain design 15
 * draws, and the probe walks it from both ends: from the task forward, and from the commit
 * backward. A chain that can only be read in one direction is not traceability — it is a log.
 *
 * The run is a real one: the model reads a file, applies a diff and asks for the acceptance
 * checks to run. Every link is then read back out of the server's own records:
 *
 *  - Task: the trace names the task the execution belongs to.
 *  - Run: the trace names the runtime that ran it.
 *  - Tool: the tool calls are on the trace, in the order the runtime made them.
 *  - Change: a change record exists.
 *  - Commit: the change landed in a commit, and the commit resolves back to this execution.
 *  - Test: the acceptance check produced a passing verdict.
 *
 * The two round trips are the part a fake cannot pass: the task must list the execution, and the
 * commit must resolve to the same execution the chain started from.
 */

/** The trace as the API serves it. */
export interface TraceResponse {
  executionId?: string;
  taskIds?: string[];
  runtimeId?: string;
  externalRunId?: string;
  toolCalls?: Array<{
    toolCallId?: string;
    toolName?: string;
    status?: string;
    permission?: { requestId?: string; decision?: string } | undefined;
  }>;
  permissions?: Array<{ requestId?: string; decision?: string }>;
  gitCommits?: string[];
  changeRecords?: Array<{ id?: string; gitCommitSha?: string; diffHash?: string }>;
  contextSnapshots?: Array<{ id?: string }>;
  promptSnapshots?: Array<{ id?: string }>;
  policySnapshots?: Array<{ id?: string }>;
  evaluations?: Array<{ verdict?: string }>;
}

export interface PiTraceabilityClient {
  /** Starts the task and returns its execution id. */
  start(input: { taskId: string; prompt: string; allowedTools: string[] }): Promise<string>;
  status(executionId: string): Promise<string>;
  /** The trace of one execution. */
  trace(executionId: string): Promise<TraceResponse>;
  /** The status the trace endpoint answers with for an id, 404 included. */
  traceStatus(executionId: string): Promise<number>;
  /** The trace of a commit: which execution produced it. */
  commitTrace(commitSha: string): Promise<{ executionId: string } | undefined>;
  /** Every execution a task produced. */
  taskTrace(taskId: string): Promise<Array<{ executionId: string }>>;
}

export interface PiTraceabilityOptions {
  now?: () => Date;
  runTimeoutMs?: number;
}

/** The tools the task needs, in the order the task needs them. */
export const TRACE_TOOL_ORDER = ["read_file", "request_apply_diff", "evaluate_changes"] as const;

/** One link of the chain, and whether it holds. */
export interface TraceLink {
  name:
    | "task"
    | "run"
    | "tool"
    | "change"
    | "commit"
    | "test"
    | "task-round-trip"
    | "commit-round-trip"
    | "not-found";
  ok: boolean;
  detail: string;
}

export interface TraceJudgement {
  links: TraceLink[];
  ok: boolean;
}

/**
 * Judges the chain.
 *
 * Every link is judged on its own, so a failure names the missing link instead of saying "the
 * trace is incomplete". The round trips are judged separately from the links they repeat: a chain
 * that lists a commit but cannot resolve it is a different failure from one that never recorded it.
 */
export function judgeTrace(input: {
  executionId: string;
  taskId: string;
  status: string;
  trace: TraceResponse;
  commitTrace?: { executionId: string } | undefined;
  taskTrace?: Array<{ executionId: string }>;
  /** The status the trace endpoint answers with for an execution that was never recorded. */
  missingTraceStatus?: number | undefined;
}): TraceJudgement {
  const trace = input.trace;
  const links: TraceLink[] = [];

  const taskIds = trace.taskIds ?? [];
  links.push({
    name: "task",
    ok: taskIds.includes(input.taskId),
    detail: taskIds.includes(input.taskId)
      ? `task ${input.taskId}`
      : `the trace names ${taskIds.join(",") || "no task"}, not ${input.taskId}`
  });

  const runtimeId = trace.runtimeId ?? "";
  links.push({
    name: "run",
    ok: trace.executionId === input.executionId && runtimeId !== "",
    detail:
      trace.executionId === input.executionId && runtimeId !== ""
        ? `execution ${input.executionId} on ${runtimeId}`
        : `the trace does not name the execution and the runtime (${trace.executionId ?? "?"} / ${runtimeId || "?"})`
  });

  const started = (trace.toolCalls ?? [])
    .filter(call => call.status !== "failed")
    .map(call => String(call.toolName ?? ""));
  const toolsOk = TRACE_TOOL_ORDER.every(tool => started.includes(tool)) && isSubsequence(started, [...TRACE_TOOL_ORDER]);
  links.push({
    name: "tool",
    ok: toolsOk,
    detail: toolsOk
      ? started.join(" > ")
      : `the tool calls are ${started.join(">") || "none"}`
  });

  const changeRecords = trace.changeRecords ?? [];
  links.push({
    name: "change",
    ok: changeRecords.length > 0,
    detail: changeRecords.length > 0 ? `${changeRecords.length} change record(s)` : "no change record"
  });

  const commitShas = changeRecords
    .map(record => record.gitCommitSha)
    .filter((sha): sha is string => typeof sha === "string" && sha !== "");
  const commitsOk = commitShas.length > 0 && commitShas.every(sha => (trace.gitCommits ?? []).includes(sha));
  links.push({
    name: "commit",
    ok: commitsOk,
    detail: commitsOk ? commitShas.join(",") : "the change never became a commit"
  });

  const verdicts = (trace.evaluations ?? []).map(item => item.verdict ?? "?");
  links.push({
    name: "test",
    ok: verdicts.includes("passed"),
    detail: verdicts.includes("passed") ? verdicts.join(",") : `no passing verdict (${verdicts.join(",") || "none"})`
  });

  const taskRoundTrip = (input.taskTrace ?? []).some(item => item.executionId === input.executionId);
  links.push({
    name: "task-round-trip",
    ok: taskRoundTrip,
    detail: taskRoundTrip
      ? `the task lists ${input.executionId}`
      : "the task does not list the execution it produced"
  });

  const commitRoundTrip = input.commitTrace?.executionId === input.executionId;
  links.push({
    name: "commit-round-trip",
    ok: commitRoundTrip,
    detail: commitRoundTrip
      ? `the commit resolves to ${input.executionId}`
      : "the commit does not resolve back to the execution"
  });

  // An id that was never an execution has to be refused, not answered with an empty chain. A chain
  // of six empty links reads like a real run that recorded nothing, and an auditor cannot tell a
  // mistyped id from a run that produced no tools. The refusal is the distinction.
  const missingStatus = input.missingTraceStatus;
  links.push({
    name: "not-found",
    ok: missingStatus === 404,
    detail:
      missingStatus === 404
        ? "an execution that was never recorded is refused"
        : `an execution that was never recorded answered ${missingStatus ?? "?"} instead of 404`
  });

  return { links, ok: links.every(link => link.ok) };
}

/**
 * P0-024: one real run, walked from the task and from the commit.
 */
export async function executeTraceabilityChain(
  client: PiTraceabilityClient,
  options: PiTraceabilityOptions = {}
): Promise<TestResult> {
  const now = options.now ?? (() => new Date());
  const startedAt = now().toISOString();
  const evidence: string[] = [];
  const failures: string[] = [];

  const taskId = "task-p0-024";
  const executionId = await client.start({
    taskId,
    prompt: [
      "Work in the repository you are given. Do exactly these three steps, using the tools:",
      "1. Call read_file with path 'app.ts' and look at what it contains.",
      "2. Call request_apply_diff with a unified diff that replaces the single line",
      "   'export const value = 1;' with 'export const value = 2;'. The diff must use the",
      "   a/app.ts and b/app.ts prefixes and one hunk, so that git apply accepts it.",
      "3. Call evaluate_changes to run the acceptance checks of this task."
    ].join(" "),
    allowedTools: ["read_file", "request_diff", "request_apply_diff", "evaluate_changes"]
  });
  evidence.push(`executionId=${executionId}`);

  const status = await waitForTerminal(client, executionId, options.runTimeoutMs ?? 300_000);
  evidence.push(`status=${status}`);

  const trace = await client.trace(executionId);
  const changeRecords = trace.changeRecords ?? [];
  const commitSha = changeRecords.find(record => record.gitCommitSha)?.gitCommitSha;
  const commitTrace = commitSha ? await client.commitTrace(commitSha) : undefined;
  const taskTrace = await client.taskTrace(taskId);
  // An id nobody ever ran: the trace endpoint has to refuse it instead of answering with an empty
  // chain, because an empty chain is what a real run with no tools looks like.
  const missingTraceStatus = await client.traceStatus("exec-never-recorded-probe");
  evidence.push(`taskTraceExecutions=${taskTrace.map(item => item.executionId).join(",") || "none"}`);
  evidence.push(`commitTraceExecution=${commitTrace?.executionId ?? "none"}`);
  evidence.push(`missingTraceStatus=${missingTraceStatus}`);

  const judgement = judgeTrace({
    executionId,
    taskId,
    status,
    trace,
    commitTrace,
    taskTrace,
    missingTraceStatus
  });
  for (const link of judgement.links) {
    evidence.push(`${link.name}:${link.ok ? "ok" : "missing"} ${link.detail}`);
  }

  if (status !== "succeeded") failures.push(`the execution ended as '${status}'`);
  for (const link of judgement.links) {
    if (!link.ok) failures.push(`the ${link.name} link does not hold: ${link.detail}`);
  }

  return {
    id: "P0-024",
    title: "端到端可追溯：Task→Run→Tool→Change→Commit→Test",
    status: failures.length === 0 ? "passed" : "failed",
    startedAt,
    finishedAt: now().toISOString(),
    evidence,
    ...(failures.length > 0 ? { notes: failures.join("; ") } : {})
  };
}

/** Whether `expected` appears in `actual` in order, with anything else allowed in between. */
export function isSubsequence(actual: readonly string[], expected: readonly string[]): boolean {
  let index = 0;
  for (const item of actual) {
    if (item === expected[index]) index += 1;
    if (index === expected.length) return true;
  }
  return index === expected.length;
}

async function waitForTerminal(
  client: PiTraceabilityClient,
  executionId: string,
  timeoutMs: number
): Promise<string> {
  const deadline = Date.now() + timeoutMs;
  const terminal = new Set(["succeeded", "failed", "cancelled", "interrupted"]);
  let status = "unknown";
  while (Date.now() < deadline) {
    status = await client.status(executionId);
    if (terminal.has(status)) return status;
    await sleep(300);
  }
  return status;
}

function sleep(ms: number): Promise<void> {
  return new Promise(resolveSleep => setTimeout(resolveSleep, ms));
}

function git(args: string[], cwd: string): void {
  const result = spawnSync("git", args, { cwd, encoding: "utf8" });
  if (result.status !== 0) {
    throw new Error(`git ${args.join(" ")} failed: ${result.stderr}`);
  }
}

async function main(): Promise<void> {
  const reportPath = resolve(
    process.env.PI_TRACEABILITY_REPORT_PATH ?? "reports/phase0-pi-traceability.json"
  );
  const model = process.env.PI_PROBE_MODEL ?? "MiniMax-M3";
  if (!process.env.MINIMAX_CN_API_KEY) {
    throw new Error("MINIMAX_CN_API_KEY is not set; nothing to probe.");
  }

  const repo = await mkdtemp(join(tmpdir(), "codeweaver-pi-trace-repo-"));
  const dataDir = await mkdtemp(join(tmpdir(), "codeweaver-pi-trace-data-"));
  await writeFile(join(repo, "app.ts"), "export const value = 1;\n", "utf8");
  await writeFile(join(repo, "spec.md"), "# Spec\nThe value must become 2.\n", "utf8");
  await writeFile(
    join(repo, "check.mjs"),
    [
      "import { readFileSync } from 'node:fs';",
      "const content = readFileSync(new URL('./app.ts', import.meta.url), 'utf8');",
      "if (!content.includes('value = 2')) {",
      "  console.error('app.ts still holds the old value');",
      "  process.exit(1);",
      "}",
      "console.log('app.ts holds value = 2');",
      ""
    ].join("\n"),
    "utf8"
  );
  git(["init", "-b", "main"], repo);
  git(["config", "user.name", "CodeWeaver Probe"], repo);
  git(["config", "user.email", "probe@codeweaver.local"], repo);
  git(["add", "-A"], repo);
  git(["commit", "-m", "initial"], repo);

  const runtime = createRuntime({
    host: "127.0.0.1",
    port: 0,
    dataDir,
    worktreeRoot: join(dataDir, "worktrees"),
    repository: repo,
    // Hermes is registered so the server has a default runtime; this probe never talks to it.
    hermesBaseUrl: "http://127.0.0.1:9",
    hermesTimeoutMs: 5_000,
    pi: { provider: "minimax-cn", model }
  });
  await runtime.ready();
  const server = await createApiServer(runtime.deps).listen(0);

  const client = httpClient(server.url, repo);

  const results = [await executeTraceabilityChain(client)];

  await writeTestReport(reportPath, results);
  console.log(`Phase 0 traceability report: ${reportPath}`);
  for (const item of results) {
    console.log(`${item.id} ${item.status}: ${item.title}`);
    for (const line of item.evidence) console.log(`  ${line}`);
    if (item.notes) console.log(`  notes: ${item.notes}`);
  }
  if (results.some(item => item.status === "failed")) process.exitCode = 1;

  await server.close();
  await runtime.close();
  await rm(repo, { recursive: true, force: true });
  await rm(dataDir, { recursive: true, force: true });
}

/** The real client: everything goes over HTTP, the way the console drives it. */
function httpClient(baseUrl: string, workspace: string): PiTraceabilityClient {
  return {
    start: async input => {
      const response = await fetch(`${baseUrl}/api/v1/tasks/${input.taskId}/executions`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          projectId: "probe-pi-traceability",
          prompt: input.prompt,
          worktreePath: workspace,
          documents: ["spec.md"],
          selectedFiles: ["app.ts"],
          architectureVersion: "1.0",
          policy: {
            snapshotId: "snap-pi-trace",
            projectId: "probe-pi-traceability",
            taskId: input.taskId,
            executionId: "",
            worktreeId: "wt-pi-trace",
            policyVersion: "1.0.0",
            grantedCapabilities: ["*"],
            allowedPaths: [workspace],
            expiresAt: "2099-01-01T00:00:00.000Z"
          },
          template: { name: "coding-agent", version: "1.4.2", contentHash: "e".repeat(64) },
          modelConfig: { model: process.env.PI_PROBE_MODEL ?? "MiniMax-M3" },
          correlationId: input.taskId,
          timeoutSeconds: 300,
          allowedTools: input.allowedTools,
          evaluationChecks: [
            {
              id: "check",
              name: "app.ts holds the new value",
              command: "node",
              args: ["check.mjs"],
              cwd: workspace
            }
          ],
          runtimeId: "pi-local"
        })
      });
      if (response.status !== 201) {
        throw new Error(`the task was refused: ${response.status} ${await response.text()}`);
      }
      const created = (await response.json()) as { execution: { executionId: string } };
      return created.execution.executionId;
    },
    status: async executionId => {
      const response = await fetch(`${baseUrl}/api/v1/executions/${encodeURIComponent(executionId)}`);
      const body = (await response.json()) as { status?: string };
      return String(body.status ?? "unknown");
    },
    trace: async executionId => {
      const response = await fetch(
        `${baseUrl}/api/v1/executions/${encodeURIComponent(executionId)}/trace`
      );
      if (response.status !== 200) {
        throw new Error(`the trace was refused: ${response.status} ${await response.text()}`);
      }
      return (await response.json()) as TraceResponse;
    },
    traceStatus: async executionId => {
      const response = await fetch(
        `${baseUrl}/api/v1/executions/${encodeURIComponent(executionId)}/trace`
      );
      return response.status;
    },
    commitTrace: async commitSha => {
      const response = await fetch(
        `${baseUrl}/api/v1/trace/commits/${encodeURIComponent(commitSha)}`
      );
      if (response.status === 404) return undefined;
      const body = (await response.json()) as { trace?: { executionId: string } };
      return body.trace;
    },
    taskTrace: async taskId => {
      const response = await fetch(`${baseUrl}/api/v1/tasks/${encodeURIComponent(taskId)}/trace`);
      const body = (await response.json()) as { executions?: Array<{ executionId: string }> };
      return body.executions ?? [];
    }
  };
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : undefined;
if (invokedPath === import.meta.url) {
  await main();
}
