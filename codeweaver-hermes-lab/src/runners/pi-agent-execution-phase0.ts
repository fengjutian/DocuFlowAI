import { spawnSync } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { randomBytes } from "node:crypto";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { createApiServer } from "../api/app.js";
import { writeTestReport } from "../reporting/report-writer.js";
import { createRuntime } from "../server/runtime.js";
import type { TestResult } from "../types/hermes.js";

/**
 * P0-005 (single-step task) and P0-006 (multi-tool task), on the Pi runtime and a real model.
 *
 * Two checklist items that are about what the agent *did*, not about what the server recorded:
 *
 *  - P0-005 asks for a simple task and checks the answer is right. "Right" has to mean something
 *    that cannot be faked, so the answer is a nonce that exists in exactly one place — a file in
 *    the worktree — and the model has to read it. A model that guessed, or a server that echoed
 *    the prompt, cannot produce it.
 *  - P0-006 asks for a multi-tool task and checks the tool calls came in the right order and that
 *    the work actually landed: the change is in the file, the change record exists, and an
 *    independent command decided the task passed.
 *
 * Both run through the HTTP API, on the real model, because the claim is about what a user of the
 * product gets.
 */

/** The event envelope the API serves (design 13). */
export interface PiPhase0Event {
  type: string;
  sequence?: number;
  data?: Record<string, unknown>;
}

export interface PiAcceptanceCheck {
  id: string;
  name: string;
  command: string;
  args: string[];
  cwd?: string;
}

export interface PiAgentExecutionClient {
  /** Starts a task and returns its execution id. */
  start(input: {
    taskId: string;
    prompt: string;
    allowedTools: string[];
    evaluationChecks?: PiAcceptanceCheck[];
  }): Promise<string>;
  status(executionId: string): Promise<string>;
  events(executionId: string): Promise<PiPhase0Event[]>;
  /** The execution's own report: terminal status and the completion gate. */
  detail(executionId: string): Promise<{
    status: string;
    completion?: { allowed: boolean; reason: string };
  }>;
  /** The trace of the execution: change records and evaluation results. */
  trace(executionId: string): Promise<{
    changeRecords?: Array<{ id?: string }>;
    evaluations?: Array<{ verdict?: string }>;
  }>;
  /** The content of a file in the worktree, read after the run. */
  readWorkspaceFile(path: string): Promise<string>;
}

export interface PiAgentExecutionOptions {
  /** The nonce the model has to read out of the worktree. */
  nonce?: string;
  now?: () => Date;
  /** How long a real run may take. */
  runTimeoutMs?: number;
}

const TERMINAL_STATUSES = new Set(["succeeded", "failed", "cancelled", "interrupted"]);

/** The tools P0-006 expects, in the order the task requires them. */
export const EXPECTED_TOOL_ORDER = ["read_file", "request_apply_diff", "evaluate_changes"] as const;

export function defaultNonce(): string {
  return `CODEWEAVER_P0_005_${randomBytes(6).toString("hex").toUpperCase()}`;
}

/**
 * P0-005: one step, one tool call, one answer that could only come from the file.
 */
export async function executeSingleStepTask(
  client: PiAgentExecutionClient,
  options: PiAgentExecutionOptions = {}
): Promise<TestResult> {
  const now = options.now ?? (() => new Date());
  const nonce = options.nonce ?? defaultNonce();
  const startedAt = now().toISOString();
  const evidence: string[] = [];
  const failures: string[] = [];

  const executionId = await client.start({
    taskId: "task-p0-005",
    prompt: [
      "Use the read_file tool to read the file 'secret.txt' in the workspace.",
      "Then reply with exactly the content of that file, nothing else."
    ].join(" "),
    allowedTools: ["read_file"]
  });
  evidence.push(`executionId=${executionId}`);
  evidence.push(`nonce=${nonce}`);

  const status = await waitForTerminal(client, executionId, options.runTimeoutMs ?? 180_000);
  evidence.push(`status=${status}`);

  const events = await client.events(executionId);
  const toolCalls = toolEventsInOrder(events);
  evidence.push(`toolCalls=${toolCalls.map(call => call.name).join(",") || "none"}`);

  const output = assistantText(events);
  evidence.push(`outputChars=${output.length}`);
  evidence.push(`outputHasNonce=${String(output.includes(nonce))}`);

  if (status !== "succeeded") failures.push(`the execution ended as '${status}'`);
  if (toolCalls.length !== 1) {
    failures.push(`expected exactly one tool call, found ${toolCalls.length}`);
  } else if (toolCalls[0]!.name !== "read_file") {
    failures.push(`the single tool call was '${toolCalls[0]!.name}' instead of 'read_file'`);
  }
  // The nonce lives in one place only: the file in the worktree. It is not in the prompt, so an
  // answer that contains it cannot be a lucky guess or an echo of the instruction — the model had
  // to read the file to produce it.
  if (!output.includes(nonce)) {
    failures.push("the answer does not contain the file's content");
  }

  return {
    id: "P0-005",
    title: "单步任务执行：一次工具调用，答案来自工作区文件",
    status: failures.length === 0 ? "passed" : "failed",
    startedAt,
    finishedAt: now().toISOString(),
    evidence,
    ...(failures.length > 0 ? { notes: failures.join("; ") } : {})
  };
}

/**
 * P0-006: three tool calls in order, a change that landed, and an independent verdict.
 */
export async function executeMultiToolTask(
  client: PiAgentExecutionClient,
  options: PiAgentExecutionOptions = {}
): Promise<TestResult> {
  const now = options.now ?? (() => new Date());
  const startedAt = now().toISOString();
  const evidence: string[] = [];
  const failures: string[] = [];

  const executionId = await client.start({
    taskId: "task-p0-006",
    prompt: [
      "Work in the repository you are given. Do exactly these three steps, using the tools:",
      "1. Call read_file with path 'app.ts' and look at what it contains.",
      "2. Call request_apply_diff with a unified diff that replaces the single line",
      "   'export const value = 1;' with 'export const value = 2;'. The diff must use the",
      "   a/app.ts and b/app.ts prefixes and one hunk, so that git apply accepts it.",
      "3. Call evaluate_changes to run the acceptance checks of this task."
    ].join(" "),
    allowedTools: ["read_file", "request_diff", "request_apply_diff", "evaluate_changes"],
    evaluationChecks: [
      { id: "check", name: "app.ts holds the new value", command: "node", args: ["check.mjs"] }
    ]
  });
  evidence.push(`executionId=${executionId}`);

  const status = await waitForTerminal(client, executionId, options.runTimeoutMs ?? 300_000);
  evidence.push(`status=${status}`);

  const events = await client.events(executionId);
  const toolCalls = toolEventsInOrder(events);
  evidence.push(`toolCalls=${toolCalls.map(call => `${call.name}@${call.sequence}`).join(",") || "none"}`);

  const started = toolCalls.filter(call => call.kind === "started").map(call => call.name);
  const completed = toolCalls.filter(call => call.kind === "completed").map(call => call.name);
  evidence.push(`startedOrder=${started.join(">") || "none"}`);
  evidence.push(`completedOrder=${completed.join(">") || "none"}`);

  const detail = await client.detail(executionId);
  const trace = await client.trace(executionId);
  const file = await client.readWorkspaceFile("app.ts");
  evidence.push(`changeRecords=${(trace.changeRecords ?? []).length}`);
  evidence.push(`evaluations=${(trace.evaluations ?? []).map(item => item.verdict ?? "?").join(",") || "none"}`);
  evidence.push(`completionAllowed=${String(detail.completion?.allowed ?? false)}`);
  evidence.push(`fileHoldsNewValue=${String(file.includes("value = 2"))}`);

  if (status !== "succeeded") failures.push(`the execution ended as '${status}'`);
  // The order is the claim: three tools that ran, in the order the task needed them.
  if (!isSubsequence(started, [...EXPECTED_TOOL_ORDER])) {
    failures.push(`the tool calls did not run in the expected order: ${started.join(">")}`);
  }
  for (const tool of EXPECTED_TOOL_ORDER) {
    if (!started.includes(tool)) failures.push(`the tool '${tool}' was never called`);
    if (!completed.includes(tool)) failures.push(`the tool '${tool}' never completed`);
  }
  if (!file.includes("value = 2")) {
    failures.push("the change did not land in app.ts");
  }
  if ((trace.changeRecords ?? []).length === 0) {
    failures.push("no change record was written");
  }
  const verdicts = (trace.evaluations ?? []).map(item => item.verdict);
  if (!verdicts.includes("passed")) {
    failures.push(`the acceptance check did not pass (${verdicts.join(",") || "no verdict"})`);
  }
  if (detail.completion?.allowed !== true) {
    failures.push(`the completion gate is not open: ${detail.completion?.reason ?? "no reason given"}`);
  }

  return {
    id: "P0-006",
    title: "多步骤工具调用：按顺序执行、变更落地、独立验收通过",
    status: failures.length === 0 ? "passed" : "failed",
    startedAt,
    finishedAt: now().toISOString(),
    evidence,
    ...(failures.length > 0 ? { notes: failures.join("; ") } : {})
  };
}

export interface ToolEvent {
  kind: "started" | "completed" | "failed";
  name: string;
  sequence: number;
}

/** The tool calls of an execution, in the order the runtime emitted them. */
export function toolEventsInOrder(events: readonly PiPhase0Event[]): ToolEvent[] {
  const calls: ToolEvent[] = [];
  for (const event of events) {
    const kind =
      event.type === "agent.tool.started"
        ? "started"
        : event.type === "agent.tool.completed"
          ? "completed"
          : event.type === "agent.tool.failed"
            ? "failed"
            : undefined;
    if (!kind) continue;
    calls.push({
      kind,
      name: String(event.data?.["toolName"] ?? ""),
      sequence: event.sequence ?? 0
    });
  }
  return calls.sort((a, b) => a.sequence - b.sequence);
}

/** The assistant's answer, assembled from the streamed deltas. */
export function assistantText(events: readonly PiPhase0Event[]): string {
  return events
    .filter(event => event.type === "agent.message.delta")
    .map(event => String(event.data?.["delta"] ?? ""))
    .join("");
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
  client: PiAgentExecutionClient,
  executionId: string,
  timeoutMs: number
): Promise<string> {
  const deadline = Date.now() + timeoutMs;
  let status = "unknown";
  while (Date.now() < deadline) {
    status = await client.status(executionId);
    if (TERMINAL_STATUSES.has(status)) return status;
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
    process.env.PI_AGENT_EXECUTION_REPORT_PATH ?? "reports/phase0-pi-agent-execution.json"
  );
  const model = process.env.PI_PROBE_MODEL ?? "MiniMax-M3";
  if (!process.env.MINIMAX_CN_API_KEY) {
    throw new Error("MINIMAX_CN_API_KEY is not set; nothing to probe.");
  }

  const nonce = defaultNonce();
  const repo = await mkdtemp(join(tmpdir(), "codeweaver-pi-agent-exec-repo-"));
  const dataDir = await mkdtemp(join(tmpdir(), "codeweaver-pi-agent-exec-data-"));
  await writeFile(join(repo, "secret.txt"), `${nonce}\n`, "utf8");
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

  const results = [
    await executeSingleStepTask(client, { nonce }),
    await executeMultiToolTask(client)
  ];

  await writeTestReport(reportPath, results);
  console.log(`Phase 0 agent execution report: ${reportPath}`);
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
function httpClient(baseUrl: string, workspace: string): PiAgentExecutionClient {
  return {
    start: async input => {
      const response = await fetch(`${baseUrl}/api/v1/tasks/${input.taskId}/executions`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          projectId: "probe-pi-agent-execution",
          prompt: input.prompt,
          worktreePath: workspace,
          documents: ["spec.md"],
          selectedFiles: ["app.ts"],
          architectureVersion: "1.0",
          policy: {
            snapshotId: "snap-pi-agent-exec",
            projectId: "probe-pi-agent-execution",
            taskId: input.taskId,
            executionId: "",
            worktreeId: "wt-pi-agent-exec",
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
          ...(input.evaluationChecks
            ? {
                evaluationChecks: input.evaluationChecks.map(check => ({
                  ...check,
                  cwd: check.cwd ?? workspace
                }))
              }
            : {}),
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
    events: async executionId => {
      const response = await fetch(
        `${baseUrl}/api/v1/executions/${encodeURIComponent(executionId)}/events`
      );
      const body = (await response.json()) as { events?: PiPhase0Event[] };
      return body.events ?? [];
    },
    detail: async executionId => {
      const response = await fetch(`${baseUrl}/api/v1/executions/${encodeURIComponent(executionId)}`);
      return (await response.json()) as {
        status: string;
        completion?: { allowed: boolean; reason: string };
      };
    },
    trace: async executionId => {
      const response = await fetch(
        `${baseUrl}/api/v1/executions/${encodeURIComponent(executionId)}/trace`
      );
      return (await response.json()) as {
        changeRecords?: Array<{ id?: string }>;
        evaluations?: Array<{ verdict?: string }>;
      };
    },
    readWorkspaceFile: async path => readFile(join(workspace, path), "utf8")
  };
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : undefined;
if (invokedPath === import.meta.url) {
  await main();
}
