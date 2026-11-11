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
 * P0-018: cancellation, against the Pi runtime and a real model.
 *
 * The question is not "did CodeWeaver record a cancellation" — it always can. The question is
 * whether the runtime actually stopped: a cancel request that the model never noticed is a
 * request, not a cancellation. So the probe cancels a running answer and then checks three
 * things that only a stopped runtime can produce:
 *
 * - the execution reaches the cancelled state and not some other terminal state;
 * - the runtime's own account of the turn says the abort reached it (`runtimeConfirmed`, and the
 *   stop reason it reported);
 * - the answer the model was going to finish with never arrives.
 */

/** The token the model only produces at the very end of the answer. */
const COMPLETION_TOKEN = "CODEWEAVER_CANCELLED_BEFORE_FINISH";

/**
 * A long answer, so the cancel lands while the model is still producing it. A one-word answer
 * would race the cancel request and prove nothing.
 */
const DEFAULT_PROMPT =
  "Write the integers from 1 to 400, one per line, each followed by a short remark. " +
  `After the last line, write exactly: ${COMPLETION_TOKEN}`;

export interface PiPhase0Event {
  type: string;
  data?: Record<string, unknown>;
}

/** The slice of the HTTP API this probe needs; the real one talks to the server over the wire. */
export interface PiCancellationClient {
  /** Starts an execution and returns its id. */
  start(): Promise<string>;
  /** The execution's current status. */
  status(executionId: string): Promise<string>;
  /** Asks the execution to stop. */
  cancel(executionId: string): Promise<void>;
  /** The execution's events, in journal order. */
  events(executionId: string): Promise<PiPhase0Event[]>;
}

export interface PiCancellationOptions {
  prompt?: string;
  completionToken?: string;
  /** How long to wait for the model to start producing output. */
  startTimeoutMs?: number;
  /** How long to wait for the terminal state after the cancel request. */
  terminalTimeoutMs?: number;
  now?: () => Date;
}

const TERMINAL_STATUSES = new Set(["succeeded", "failed", "cancelled", "interrupted"]);

/**
 * The events that end an execution.
 *
 * `agent.execution.started` is not one of them: it shares the prefix, and a probe that counts it
 * as terminal reports a second terminal event that never happened.
 */
const TERMINAL_EVENT_TYPES = new Set([
  "agent.execution.completed",
  "agent.execution.failed",
  "agent.execution.cancelled",
  "agent.execution.interrupted"
]);

export async function executePiCancellationPhase0(
  client: PiCancellationClient,
  options: PiCancellationOptions = {}
): Promise<TestResult[]> {
  const now = options.now ?? (() => new Date());
  const startedAt = now().toISOString();
  const token = options.completionToken ?? COMPLETION_TOKEN;
  const startTimeoutMs = options.startTimeoutMs ?? 120_000;
  const terminalTimeoutMs = options.terminalTimeoutMs ?? 60_000;
  const evidence: string[] = [];

  const executionId = await client.start();
  evidence.push(`executionId=${executionId}`);

  // The cancel has to land while the model is producing its answer. A cancel that arrives before
  // the runtime said anything proves nothing about the runtime.
  const streaming = await waitForOutput(client, executionId, startTimeoutMs);
  if (!streaming) {
    return [
      {
        id: "P0-018",
        title: "Cancellation",
        status: "failed",
        startedAt,
        finishedAt: now().toISOString(),
        evidence,
        notes: "The runtime never produced output, so there was no running turn to cancel"
      }
    ];
  }
  evidence.push("runtimeWasStreaming=true");

  await client.cancel(executionId);
  const terminalStatus = await waitForTerminal(client, executionId, terminalTimeoutMs);
  evidence.push(`terminalStatus=${terminalStatus}`);

  const events = await client.events(executionId);
  const terminalEvents = events.filter(event => TERMINAL_EVENT_TYPES.has(event.type));
  const cancelled = terminalEvents.find(event => event.type === "agent.execution.cancelled");
  evidence.push(`events=${events.length}`);
  evidence.push(`terminalEvents=${terminalEvents.map(event => event.type).join(",") || "none"}`);

  const failures: string[] = [];

  if (terminalStatus !== "cancelled") {
    failures.push(`the execution ended as '${terminalStatus}' instead of 'cancelled'`);
  }
  if (!cancelled) {
    failures.push("no cancelled event was recorded");
  }
  if (terminalEvents.length !== 1) {
    failures.push(`expected exactly one terminal event, found ${terminalEvents.length}`);
  }
  const confirmed = cancelled?.data?.["runtimeConfirmed"];
  const stopReason = cancelled?.data?.["runtimeStopReason"];
  evidence.push(`runtimeConfirmed=${String(confirmed)}`);
  evidence.push(`runtimeStopReason=${String(stopReason)}`);
  if (confirmed !== true) {
    failures.push("the runtime did not confirm that the abort reached it");
  }
  if (stopReason !== "aborted") {
    failures.push(`the runtime reported stop reason '${String(stopReason)}' instead of 'aborted'`);
  }

  // The answer the model was going to finish with must never arrive.
  const answer = events
    .filter(event => event.type === "agent.message.delta")
    .map(event => String(event.data?.["delta"] ?? ""))
    .join("");
  if (answer.includes(token)) {
    failures.push("the model finished its answer, so the turn was not actually cut off");
  } else {
    evidence.push(`answerChars=${answer.length}`);
  }

  // Nothing may arrive after the terminal event: a runtime that keeps talking was not stopped.
  const terminalIndex = events.findIndex(event => event.type === "agent.execution.cancelled");
  const afterTerminal = terminalIndex >= 0 ? events.slice(terminalIndex + 1) : [];
  if (afterTerminal.length > 0) {
    failures.push(`${afterTerminal.length} event(s) arrived after the terminal event`);
  }

  return [
    {
      id: "P0-018",
      title: "Cancellation",
      status: failures.length === 0 ? "passed" : "failed",
      startedAt,
      finishedAt: now().toISOString(),
      evidence,
      ...(failures.length > 0 ? { notes: failures.join("; ") } : {})
    }
  ];
}

async function waitForOutput(
  client: PiCancellationClient,
  executionId: string,
  timeoutMs: number
): Promise<boolean> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const events = await client.events(executionId);
    if (events.some(event => event.type === "agent.message.delta")) return true;
    const status = await client.status(executionId);
    if (TERMINAL_STATUSES.has(status)) return false;
    await sleep(200);
  }
  return false;
}

async function waitForTerminal(
  client: PiCancellationClient,
  executionId: string,
  timeoutMs: number
): Promise<string> {
  const deadline = Date.now() + timeoutMs;
  let status = "unknown";
  while (Date.now() < deadline) {
    status = await client.status(executionId);
    if (TERMINAL_STATUSES.has(status)) return status;
    await sleep(200);
  }
  return status;
}

function sleep(ms: number): Promise<void> {
  return new Promise(resolveSleep => setTimeout(resolveSleep, ms));
}

async function main(): Promise<void> {
  const reportPath = resolve(process.env.PI_CANCELLATION_REPORT_PATH ?? "reports/phase0-pi-cancellation.json");
  const model = process.env.PI_PROBE_MODEL ?? "MiniMax-M3";
  if (!process.env.MINIMAX_CN_API_KEY) {
    throw new Error("MINIMAX_CN_API_KEY is not set; nothing to probe.");
  }

  const repo = await mkdtemp(join(tmpdir(), "codeweaver-pi-cancel-repo-"));
  const dataDir = await mkdtemp(join(tmpdir(), "codeweaver-pi-cancel-data-"));
  git(["init", "-b", "main"], repo);
  git(["config", "user.name", "CodeWeaver Probe"], repo);
  git(["config", "user.email", "probe@codeweaver.local"], repo);
  await writeFile(join(repo, "README.md"), "# Cancellation probe\n", "utf8");
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

  const client: PiCancellationClient = {
    start: async () => {
      const response = await fetch(`${server.url}/api/v1/tasks/task-pi-cancel/executions`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          projectId: "probe-pi-cancel",
          prompt: DEFAULT_PROMPT,
          worktreePath: repo,
          documents: ["README.md"],
          selectedFiles: ["README.md"],
          architectureVersion: "1.0",
          policy: {
            snapshotId: "snap-cancel",
            projectId: "probe-pi-cancel",
            taskId: "task-pi-cancel",
            executionId: "",
            worktreeId: "wt-cancel",
            policyVersion: "1.0.0",
            grantedCapabilities: ["*"],
            allowedPaths: [repo],
            expiresAt: "2099-01-01T00:00:00.000Z"
          },
          template: { name: "coding-agent", version: "1.4.2", contentHash: "e".repeat(64) },
          modelConfig: { model },
          correlationId: "probe-pi-cancel",
          timeoutSeconds: 600,
          allowedTools: [],
          runtimeId: "pi-local"
        })
      });
      if (response.status !== 201) {
        throw new Error(`the task was refused: ${response.status} ${await response.text()}`);
      }
      const created = await response.json();
      return String(created.execution.executionId);
    },
    status: async executionId => {
      const response = await fetch(`${server.url}/api/v1/executions/${executionId}`);
      const body = await response.json();
      return String(body.status ?? "unknown");
    },
    cancel: async executionId => {
      const response = await fetch(`${server.url}/api/v1/executions/${executionId}/cancel`, { method: "POST" });
      if (response.status !== 202) {
        throw new Error(`the cancel request was refused: ${response.status} ${await response.text()}`);
      }
    },
    events: async executionId => {
      const response = await fetch(`${server.url}/api/v1/executions/${executionId}/events`);
      const body = await response.json();
      return (body.events ?? []) as PiPhase0Event[];
    }
  };

  const results = await executePiCancellationPhase0(client);
  await writeTestReport(reportPath, results);
  console.log(`Phase 0 cancellation report: ${reportPath}`);
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

function git(args: string[], cwd: string): void {
  const result = spawnSync("git", args, { cwd, encoding: "utf8" });
  if (result.status !== 0) {
    throw new Error(`git ${args.join(" ")} failed: ${result.stderr}`);
  }
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : undefined;
if (invokedPath === import.meta.url) {
  await main();
}
