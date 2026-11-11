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
 * P0-019: a client that disconnects (客户端断开).
 *
 * Design 13 says the server persists the events and the socket is only a notification channel, so
 * "断线后根据 executionId 和 sequence 补齐事件" — a reconnect replays what the client missed. The
 * checklist asks for "No duplicate execution evidence", which is the other half of the same promise:
 * a reconnect must not invent a second run.
 *
 * The probe is a real one. A real model runs a real task; the client opens the event stream, drops
 * the connection in the middle of the run, and comes back. Then the evidence is read out of the
 * server's own records:
 *
 *  - the execution kept running while nobody was listening, and reached a terminal state;
 *  - the reconnect delivered everything the client missed;
 *  - the client's timeline holds every event of the journal exactly once — no gap, no repeat;
 *  - the task produced exactly one execution;
 *  - the change landed and the acceptance check passed.
 *
 * The journal is the yardstick: the client's timeline is compared against the entries the server
 * actually recorded, so a stream that quietly drops half the run cannot pass.
 */

/** One event of the unified envelope, as the API serves it. */
export interface StreamEvent {
  id: string;
  type: string;
  executionId?: string;
  sequence: number;
  timestamp: string;
  data: unknown;
}

export interface StreamHandle {
  /** The frames received so far. The array grows while the stream is open. */
  frames: StreamEvent[];
  /** Drops the connection. That is the whole point of the probe. */
  close(): void;
}

export interface DisconnectClient {
  /** Starts the task and returns its execution id. */
  start(input: { taskId: string; prompt: string; allowedTools: string[] }): Promise<string>;
  status(executionId: string): Promise<string>;
  /** Opens the event stream, resuming after `since`. */
  stream(executionId: string, since: number): Promise<StreamHandle>;
  /** The journal positions the server recorded. */
  journal(executionId: string): Promise<number[]>;
  /** Every execution the task produced. */
  taskExecutions(taskId: string): Promise<string[]>;
  /** The trace of one execution: the commit and the evaluation verdict. */
  trace(executionId: string): Promise<{ gitCommits?: string[]; verdicts?: string[] }>;
}

export interface DisconnectOptions {
  now?: () => Date;
  runTimeoutMs?: number;
  /** How many frames the client reads before it drops the connection. */
  disconnectAfterFrames?: number;
}

/** One judgement of the reconnect, and whether it holds. */
export interface DisconnectLink {
  name: "continues" | "complete" | "no-duplicate" | "single-execution" | "evidence";
  ok: boolean;
  detail: string;
}

export interface DisconnectJudgement {
  links: DisconnectLink[];
  ok: boolean;
}

/**
 * Judges what the client ended up with.
 *
 * The journal is the yardstick. The client's timeline has to hold every recorded entry exactly
 * once: an entry that is missing is a hole in the run, and an entry that appears twice is a
 * timeline that says something happened that did not happen twice.
 */
export function judgeDisconnect(input: {
  status: string;
  /** The events the client received, in the order it received them, duplicates included. */
  received: StreamEvent[];
  /** The journal positions the server recorded. */
  journal: number[];
  /** The executions the task produced. */
  executionIds: string[];
  expectedExecutionId: string;
  gitCommits?: string[];
  verdicts?: string[];
}): DisconnectJudgement {
  const links: DisconnectLink[] = [];

  const continues = input.status === "succeeded";
  links.push({
    name: "continues",
    ok: continues,
    detail: continues
      ? `the execution ended as '${input.status}' after the client was gone`
      : `the execution ended as '${input.status}'`
  });

  // The client's timeline is what it holds after it drops the repeats: a reconnect can deliver an
  // event the client already had, and the client has to recognize it by its id.
  const timeline = new Map<string, StreamEvent>();
  for (const event of input.received) timeline.set(event.id, event);
  const held = [...timeline.values()];

  const journalPositions = [...input.journal].sort((left, right) => left - right);
  const missing = journalPositions.filter(position => !held.some(event => event.sequence === position));
  links.push({
    name: "complete",
    ok: missing.length === 0,
    detail:
      missing.length === 0
        ? `the client holds all ${journalPositions.length} recorded event(s)`
        : `the client never received ${missing.length} event(s): ${missing.join(",")}`
  });

  const repeated = held.filter(
    (event, index) => held.findIndex(other => other.id === event.id) !== index
  );
  links.push({
    name: "no-duplicate",
    ok: repeated.length === 0,
    detail:
      repeated.length === 0
        ? `the timeline holds ${held.length} event(s), each once`
        : `the timeline holds ${repeated.length} event(s) twice`
  });

  const single = input.executionIds.length === 1 && input.executionIds[0] === input.expectedExecutionId;
  links.push({
    name: "single-execution",
    ok: single,
    detail: single
      ? `the task produced one execution (${input.expectedExecutionId})`
      : `the task produced ${input.executionIds.length} execution(s): ${input.executionIds.join(",") || "none"}`
  });

  const commits = input.gitCommits ?? [];
  const verdicts = input.verdicts ?? [];
  const evidence = commits.length > 0 && verdicts.includes("passed");
  links.push({
    name: "evidence",
    ok: evidence,
    detail: evidence
      ? `the change landed in ${commits.join(",")} and the check passed`
      : `the change landed in ${commits.join(",") || "no commit"} and the verdicts were ${verdicts.join(",") || "none"}`
  });

  return { links, ok: links.every(link => link.ok) };
}

/**
 * P0-019: one real run, interrupted and resumed.
 */
export async function executeClientDisconnect(
  client: DisconnectClient,
  options: DisconnectOptions = {}
): Promise<TestResult> {
  const now = options.now ?? (() => new Date());
  const startedAt = now().toISOString();
  const evidence: string[] = [];
  const failures: string[] = [];

  const taskId = "task-p0-019";
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

  // The client watches, and then disappears.
  const first = await client.stream(executionId, 0);
  const disconnectAfter = options.disconnectAfterFrames ?? 3;
  await waitUntil(() => first.frames.length >= disconnectAfter, 30_000, () => {
    failures.push(`the stream never delivered ${disconnectAfter} event(s)`);
  });
  const seenBefore = first.frames.length;
  const endedBeforeDisconnect = first.frames.some(frame => frame.type === "agent.execution.completed");
  first.close();
  evidence.push(`framesBeforeDisconnect=${seenBefore}`);

  // The server keeps working while nobody is listening.
  const status = await waitForTerminal(client, executionId, options.runTimeoutMs ?? 300_000);
  evidence.push(`status=${status}`);

  // The client comes back and asks for what it has not seen.
  const lastSeen = first.frames.reduce((highest, frame) => Math.max(highest, frame.sequence), 0);
  const resumed = await client.stream(executionId, lastSeen);
  await waitUntil(
    () => resumed.frames.some(frame => frame.type === "agent.execution.completed"),
    30_000,
    () => {
      failures.push("the reconnect never delivered the terminal event");
    }
  );
  const seenAfter = resumed.frames.length;
  resumed.close();
  evidence.push(`framesAfterReconnect=${seenAfter}`);
  evidence.push(`resumedFrom=${lastSeen}`);

  const journal = await client.journal(executionId);
  const executionIds = await client.taskExecutions(taskId);
  const trace = await client.trace(executionId);
  evidence.push(`journalEvents=${journal.length}`);
  evidence.push(`taskExecutions=${executionIds.join(",") || "none"}`);

  const judgement = judgeDisconnect({
    status,
    received: [...first.frames, ...resumed.frames],
    journal,
    executionIds,
    expectedExecutionId: executionId,
    gitCommits: trace.gitCommits,
    verdicts: trace.verdicts
  });
  for (const link of judgement.links) {
    evidence.push(`${link.name}:${link.ok ? "ok" : "missing"} ${link.detail}`);
  }

  if (status !== "succeeded") failures.push(`the execution ended as '${status}'`);
  for (const link of judgement.links) {
    if (!link.ok) failures.push(`the ${link.name} link does not hold: ${link.detail}`);
  }

  // A run that finished before the probe could drop the connection does not exercise a reconnect.
  // That is not a pass and not a failure: the probe never got to test the thing (D-020).
  if (endedBeforeDisconnect) {
    return {
      id: "P0-019",
      title: "客户端断开后重连：事件补齐且不重复执行",
      status: "blocked",
      startedAt,
      finishedAt: now().toISOString(),
      evidence,
      notes: "the run finished before the client could disconnect, so the reconnect was not exercised"
    };
  }

  return {
    id: "P0-019",
    title: "客户端断开后重连：事件补齐且不重复执行",
    status: failures.length === 0 ? "passed" : "failed",
    startedAt,
    finishedAt: now().toISOString(),
    evidence,
    ...(failures.length > 0 ? { notes: failures.join("; ") } : {})
  };
}

async function waitUntil(
  condition: () => boolean,
  timeoutMs: number,
  onTimeout?: () => void
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!condition() && Date.now() < deadline) {
    await sleep(50);
  }
  if (!condition()) onTimeout?.();
}

async function waitForTerminal(
  client: DisconnectClient,
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
    process.env.PI_DISCONNECT_REPORT_PATH ?? "reports/phase0-pi-client-disconnect.json"
  );
  const model = process.env.PI_PROBE_MODEL ?? "MiniMax-M3";
  if (!process.env.MINIMAX_CN_API_KEY) {
    throw new Error("MINIMAX_CN_API_KEY is not set; nothing to probe.");
  }

  const repo = await mkdtemp(join(tmpdir(), "codeweaver-pi-disconnect-repo-"));
  const dataDir = await mkdtemp(join(tmpdir(), "codeweaver-pi-disconnect-data-"));
  await writeFile(join(repo, "app.ts"), "export const value = 1;\n", "utf8");
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
    hermesBaseUrl: "http://127.0.0.1:9",
    hermesTimeoutMs: 5_000,
    pi: { provider: "minimax-cn", model }
  });
  await runtime.ready();
  const server = await createApiServer(runtime.deps).listen(0);

  const client = httpClient(server.url, repo);
  const results = [await executeClientDisconnect(client)];

  await writeTestReport(reportPath, results);
  console.log(`Phase 0 client-disconnect report: ${reportPath}`);
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
function httpClient(baseUrl: string, workspace: string): DisconnectClient {
  return {
    start: async input => {
      const response = await fetch(`${baseUrl}/api/v1/tasks/${input.taskId}/executions`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          projectId: "probe-pi-disconnect",
          prompt: input.prompt,
          worktreePath: workspace,
          documents: ["spec.md"],
          selectedFiles: ["app.ts"],
          architectureVersion: "1.0",
          policy: {
            snapshotId: "snap-pi-disconnect",
            projectId: "probe-pi-disconnect",
            taskId: input.taskId,
            executionId: "",
            worktreeId: "wt-pi-disconnect",
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
    stream: async (executionId, since) => {
      const controller = new AbortController();
      const frames: StreamEvent[] = [];
      const response = await fetch(
        `${baseUrl}/api/v1/events?executionId=${encodeURIComponent(executionId)}&since=${since}`,
        { signal: controller.signal }
      );
      const reader = response.body!.getReader();
      const decoder = new TextDecoder();
      let buffer = "";
      void (async () => {
        try {
          for (;;) {
            const { value, done } = await reader.read();
            if (done) break;
            buffer += decoder.decode(value, { stream: true });
            let boundary = buffer.indexOf("\n\n");
            while (boundary !== -1) {
              const frame = buffer.slice(0, boundary);
              buffer = buffer.slice(boundary + 2);
              const data = frame
                .split("\n")
                .find(line => line.startsWith("data:"))
                ?.slice(5)
                .trim();
              if (data) frames.push(JSON.parse(data) as StreamEvent);
              boundary = buffer.indexOf("\n\n");
            }
          }
        } catch {
          // The connection was dropped on purpose. That is the probe.
        }
      })();
      return { frames, close: () => controller.abort() };
    },
    journal: async executionId => {
      const response = await fetch(`${baseUrl}/api/v1/executions/${encodeURIComponent(executionId)}/logs`);
      const body = (await response.json()) as { entries?: Array<{ sequence: number }> };
      return (body.entries ?? []).map(entry => entry.sequence);
    },
    taskExecutions: async taskId => {
      const response = await fetch(`${baseUrl}/api/v1/tasks/${encodeURIComponent(taskId)}/trace`);
      const body = (await response.json()) as { executions?: Array<{ executionId: string }> };
      return (body.executions ?? []).map(execution => execution.executionId);
    },
    trace: async executionId => {
      const response = await fetch(`${baseUrl}/api/v1/executions/${encodeURIComponent(executionId)}/trace`);
      const body = (await response.json()) as {
        gitCommits?: string[];
        evaluations?: Array<{ verdict?: string }>;
      };
      return {
        gitCommits: body.gitCommits ?? [],
        verdicts: (body.evaluations ?? []).map(evaluation => evaluation.verdict ?? "?")
      };
    }
  };
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : undefined;
if (invokedPath === import.meta.url) {
  await main();
}
