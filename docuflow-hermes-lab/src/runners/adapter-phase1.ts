import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { AgentRuntimeRegistry } from "../adapters/runtime-registry.js";
import { HermesAdapter } from "../adapters/hermes-adapter.js";
import { HermesHttpClient } from "../clients/http/hermes-http-client.js";
import { writeTestReport } from "../reporting/report-writer.js";
import {
  isTerminalStatus,
  type AgentExecutionStatus,
  type AgentRuntimeEvent,
  type AgentTaskInput
} from "../types/adapter.js";
import type { TestResult } from "../types/hermes.js";

/**
 * Phase 1 acceptance checks AT-001..AT-004 (design section 19) exercised against a live runtime:
 * registration + health, task submission, event reception, terminal status recognition.
 *
 * AT-005 (cancellation) is intentionally absent: it is gated on verified cancellation and belongs
 * to a separate run once P0-018 has evidence.
 */
export interface AdapterPhase1Options {
  registry: AgentRuntimeRegistry;
  runtimeId: string;
  taskInput: AgentTaskInput;
  /** Upper bound for health checks, first-event arrival and terminal-state arrival. */
  timeoutMs: number;
  now?: () => Date;
}

const CHECKS = [
  ["AT-001", "Runtime registration and health check"],
  ["AT-002", "Task submission to the runtime"],
  ["AT-003", "Execution event reception"],
  ["AT-004", "Terminal status recognition"]
] as const;

export async function executeAdapterPhase1(options: AdapterPhase1Options): Promise<TestResult[]> {
  const now = options.now ?? (() => new Date());
  const results: TestResult[] = [];
  const adapter = options.registry.require(options.runtimeId);

  const healthStarted = now().toISOString();
  const health = await options.registry.healthCheckAll(options.timeoutMs);
  const runtime = health.runtimes.find(item => item.id === options.runtimeId);
  results.push(
    check("AT-001", healthStarted, now(), runtime?.healthy === true, [
      `runtime=${options.runtimeId}`,
      `latencyMs=${runtime?.latencyMs ?? -1}`
    ], runtime?.error)
  );

  const startStarted = now().toISOString();
  const executionId = await startExecution(adapter, options.taskInput, startStarted, now, results);
  if (!executionId) {
    results.push(blocked("AT-003", "Event reception requires a started execution", now));
    results.push(blocked("AT-004", "Terminal status requires a started execution", now));
    return results;
  }

  const events: AgentRuntimeEvent[] = [];
  const firstRuntimeEvent = deferred();
  const terminalEvent = deferred();
  const unsubscribe = await adapter.subscribe(executionId, event => {
    events.push(event);
    if (event.payload["hermesEvent"] !== undefined) firstRuntimeEvent.resolve();
    if (isTerminalEvent(event)) terminalEvent.resolve();
  });

  const streamStarted = now().toISOString();
  const firstArrived = await settle(firstRuntimeEvent.promise, options.timeoutMs, adapter, executionId);
  results.push(
    check("AT-003", streamStarted, now(), firstArrived, [
      `events=${events.length}`,
      `firstRuntimeEvent=${events.find(event => event.payload["hermesEvent"] !== undefined)?.eventType ?? "none"}`
    ], firstArrived ? undefined : `no runtime event arrived within ${options.timeoutMs}ms`)
  );

  const terminalStarted = now().toISOString();
  const terminalArrived = await settle(terminalEvent.promise, options.timeoutMs, adapter, executionId);
  const status = await readStatus(adapter, executionId);
  const terminal = events.filter(isTerminalEvent).at(-1);
  results.push(
    check("AT-004", terminalStarted, now(), terminalArrived && status !== undefined && isTerminalStatus(status), [
      `terminalEvent=${terminal?.eventType ?? "none"}`,
      `status=${status ?? "unknown"}`
    ], terminalArrived ? undefined : `no terminal event arrived within ${options.timeoutMs}ms`)
  );

  await unsubscribe();
  await adapter.terminate(executionId).catch(() => undefined);
  return results;
}

async function startExecution(
  adapter: { start(input: AgentTaskInput): Promise<{ executionId: string; externalRunId?: string; status: string }> },
  input: AgentTaskInput,
  startedAt: string,
  now: () => Date,
  results: TestResult[]
): Promise<string | undefined> {
  try {
    const execution = await adapter.start(input);
    results.push(check("AT-002", startedAt, now(), Boolean(execution.externalRunId), [
      `executionId=${execution.executionId}`,
      `externalRunId=${execution.externalRunId ?? "missing"}`,
      `status=${execution.status}`
    ], execution.externalRunId ? undefined : "start() returned no external run id"));
    return execution.executionId;
  } catch (error) {
    results.push(check("AT-002", startedAt, now(), false, [], errorMessage(error)));
    return undefined;
  }
}

async function readStatus(
  adapter: { getStatus(id: string): Promise<AgentExecutionStatus> },
  executionId: string
): Promise<AgentExecutionStatus | undefined> {
  try {
    return await adapter.getStatus(executionId);
  } catch {
    return undefined;
  }
}

/**
 * Waits for a marker promise. On timeout the execution is terminated so a stalled runtime cannot
 * keep the process alive; that termination is cleanup, not evidence.
 */
async function settle(
  promise: Promise<void>,
  timeoutMs: number,
  adapter: { terminate(id: string): Promise<void> },
  executionId: string
): Promise<boolean> {
  let timer: NodeJS.Timeout | undefined;
  let timedOut = false;
  const timeout = new Promise<boolean>(resolveTimeout => {
    timer = setTimeout(() => {
      timedOut = true;
      resolveTimeout(false);
    }, timeoutMs);
  });
  try {
    return await Promise.race([promise.then(() => true), timeout]);
  } finally {
    if (timer) clearTimeout(timer);
    if (timedOut) await adapter.terminate(executionId).catch(() => undefined);
  }
}

function deferred(): { promise: Promise<void>; resolve: () => void } {
  let resolvePromise: () => void = () => undefined;
  const promise = new Promise<void>(resolve => { resolvePromise = resolve; });
  return { promise, resolve: resolvePromise };
}

function isTerminalEvent(event: AgentRuntimeEvent): boolean {
  return (
    event.eventType === "execution.completed" ||
    event.eventType === "execution.failed" ||
    event.eventType === "execution.cancelled"
  );
}

async function main(): Promise<void> {
  const reportPath = resolve(process.env.HERMES_PHASE1_REPORT_PATH ?? "reports/phase1-adapter.json");
  const baseUrl = process.env.HERMES_HTTP_BASE_URL;
  let results: TestResult[];

  if (!baseUrl) {
    const timestamp = new Date().toISOString();
    results = CHECKS.map(([id, title]) =>
      blockedResult(id, title, timestamp, new Date(), "HERMES_HTTP_BASE_URL is not configured")
    );
  } else {
    const client = new HermesHttpClient({
      baseUrl,
      ...(process.env.HERMES_API_KEY ? { apiKey: process.env.HERMES_API_KEY } : {}),
      timeoutMs: positiveNumber("HERMES_TIMEOUT_MS", 30_000)
    });
    const adapter = new HermesAdapter(client);
    const registry = new AgentRuntimeRegistry();
    registry.register({ id: "hermes-local", runtimeType: "hermes", adapter, version: "pinned" });

    const timeoutMs = positiveNumber("HERMES_PHASE1_TIMEOUT_MS", 300_000);
    results = await executeAdapterPhase1({
      registry,
      runtimeId: "hermes-local",
      taskInput: {
        projectId: process.env.HERMES_PHASE1_PROJECT_ID ?? "phase1-project",
        taskId: process.env.HERMES_PHASE1_TASK_ID ?? `phase1-task-${Date.now()}`,
        runId: `phase1-run-${Date.now()}`,
        prompt: process.env.HERMES_PHASE1_PROMPT ?? "Reply with exactly: DOCUFLOW_PHASE1_OK",
        contextSnapshotId: "phase1-context",
        policySnapshotId: "phase1-policy",
        worktreeId: "phase1-worktree",
        workingDirectory: process.env.HERMES_PHASE1_WORKDIR ?? process.cwd(),
        allowedTools: [],
        timeoutSeconds: Math.ceil(timeoutMs / 1000),
        correlationId: `phase1-${Date.now()}`
      },
      timeoutMs
    });
  }

  await writeTestReport(reportPath, results);
  console.log(`Phase 1 adapter report: ${reportPath}`);
  for (const item of results) console.log(`${item.id} ${item.status}: ${item.title}`);
  if (results.some(item => item.status === "failed")) process.exitCode = 1;
}

function check(
  id: string,
  startedAt: string,
  finishedAt: Date,
  condition: boolean,
  evidence: string[],
  notes?: string
): TestResult {
  return {
    id,
    title: titleOf(id),
    status: condition ? "passed" : "failed",
    startedAt,
    finishedAt: finishedAt.toISOString(),
    evidence,
    ...(notes ? { notes } : {})
  };
}

function blocked(id: string, note: string, now: () => Date): TestResult {
  return blockedResult(id, titleOf(id), now().toISOString(), now(), note);
}

function blockedResult(
  id: string,
  title: string,
  startedAt: string,
  finishedAt: Date,
  note: string
): TestResult {
  return {
    id,
    title,
    status: "blocked",
    startedAt,
    finishedAt: finishedAt.toISOString(),
    evidence: [],
    notes: note
  };
}

function titleOf(id: string): string {
  return CHECKS.find(([checkId]) => checkId === id)?.[1] ?? id;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function positiveNumber(name: string, fallback: number): number {
  const value = Number(process.env[name] ?? fallback);
  if (!Number.isFinite(value) || value <= 0) throw new Error(`${name} must be a positive number`);
  return value;
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : undefined;
if (invokedPath === import.meta.url) {
  await main();
}
