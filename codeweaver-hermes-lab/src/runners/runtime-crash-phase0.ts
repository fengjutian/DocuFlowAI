import { spawnSync } from "node:child_process";
import { connect } from "node:net";
import { closeSync, openSync, readFileSync, rmSync } from "node:fs";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { createApiServer } from "../api/app.js";
import { writeTestReport } from "../reporting/report-writer.js";
import { createRuntime } from "../server/runtime.js";
import type { TestResult } from "../types/hermes.js";

/**
 * P0-020: the runtime process crashes (运行时进程崩溃).
 *
 * Design 14.2 names eight failures that must not all be answered with "call the model again", and
 * the second one is the runtime process dying. It is a different fault from the one P0-021 covers:
 * there the CodeWeaver worker died and the session went with it, here the worker survives and the
 * *runtime* disappears underneath it.
 *
 * The runtime has to be out of process for that to be a real crash, so the probe uses the pinned
 * Hermes gateway — the same process the lab talks to over HTTP in every other run — and kills it
 * while a turn is in flight. Pi is not used here: it runs inside the CodeWeaver process, so
 * killing it is the worker crash of P0-021, not a runtime crash.
 *
 * What the product owes the user after a runtime crash:
 *
 *  - the execution ends in a terminal state that says "failed", not one that hangs or reports
 *    success;
 *  - the crash is recorded, with the reason, so there is a crash log to read;
 *  - the failed execution's record is kept — design 14.3 asks for exactly that;
 *  - nobody quietly calls the model again, and the task does not grow a second execution;
 *  - the runtime registry reports the dead runtime as unhealthy, so the next task is not sent to
 *    a corpse.
 */

/** One journal entry, as the API serves it. */
export interface CrashJournalEntry {
  category: string;
  kind?: string;
  payload?: Record<string, unknown>;
}

/** What the probe needs from the server, over HTTP, the way the console drives it. */
export interface CrashClient {
  /** Starts the task and returns its execution id. */
  start(input: { taskId: string; prompt: string }): Promise<string>;
  /** The status the API reports for an execution. */
  status(executionId: string): Promise<string>;
  /** The journal entries recorded so far. */
  journal(executionId: string): Promise<CrashJournalEntry[]>;
  /** The HTTP status of the trace route — the failed execution's record must still be readable. */
  traceStatus(executionId: string): Promise<number>;
  /** Every execution the task produced. */
  taskExecutions(taskId: string): Promise<string[]>;
  /** The recovery attempt: what the server answers when someone asks to retry. */
  retry(executionId: string): Promise<{ status: number; message?: string }>;
  /** Whether the registry considers the runtime healthy. */
  runtimeHealthy(runtimeId: string): Promise<boolean>;
}

/** The result of killing the runtime process. */
export interface CrashKill {
  /** The runtime answered before the kill. */
  aliveBefore: boolean;
  /** The runtime is gone after the kill. */
  aliveAfter: boolean;
}

export interface CrashOptions {
  now?: () => Date;
  /** How long the execution may take to reach a terminal state after the crash. */
  runTimeoutMs?: number;
  /** How long the runtime may take to produce its first output before the crash is not exercised. */
  activityTimeoutMs?: number;
  /** Kills the runtime process. This is the crash itself. */
  killRuntime: () => CrashKill;
}

export type CrashLinkName =
  | "crash-was-real"
  | "crash-detected"
  | "crash-recorded"
  | "audit-kept"
  | "no-silent-rerun"
  | "runtime-unhealthy";

export interface CrashLink {
  name: CrashLinkName;
  ok: boolean;
  detail: string;
}

export interface CrashJudgement {
  links: CrashLink[];
  ok: boolean;
}

/**
 * Judges what the product did after the runtime died.
 *
 * Every link is a claim the user would make about a crashed runtime: "it failed, it says why, the
 * record is still there, nobody re-ran my task, and the runtime is known to be down".
 */
export function judgeRuntimeCrash(input: {
  status: string;
  journal: CrashJournalEntry[];
  traceStatus: number;
  executionIds: string[];
  expectedExecutionId: string;
  retry: { status: number; message?: string };
  runtimeHealthy: boolean;
  kill: CrashKill;
}): CrashJudgement {
  const links: CrashLink[] = [];

  // The probe has to have crashed something. A kill that left the runtime alive proves nothing.
  const realCrash = input.kill.aliveBefore && !input.kill.aliveAfter;
  links.push({
    name: "crash-was-real",
    ok: realCrash,
    detail: realCrash
      ? "the runtime process was alive before the kill and gone after it"
      : `the runtime was ${input.kill.aliveBefore ? "alive" : "not running"} before the kill and ${
          input.kill.aliveAfter ? "still alive" : "gone"
        } after it`
  });

  const failed = input.status === "failed";
  links.push({
    name: "crash-detected",
    ok: failed,
    detail: failed
      ? "the execution ended as 'failed' after the runtime died"
      : `the execution ended as '${input.status}'`
  });

  const failure = input.journal.find(entry => entry.category === "execution.failed");
  const reason = failure?.payload?.["reason"];
  const recorded = failure !== undefined && typeof reason === "string" && reason !== "";
  links.push({
    name: "crash-recorded",
    ok: recorded,
    detail: recorded
      ? `the journal records the failure with reason '${reason}'`
      : failure
        ? "the journal records the failure but without a reason"
        : "the journal holds no failure entry at all"
  });

  // Design 14.3: the audit record of a failed execution is kept. The request and the runtime are
  // still on the timeline, and the trace still answers.
  const keptRequest = input.journal.some(entry => entry.category === "execution.input");
  const kept = input.traceStatus === 200 && keptRequest && input.journal.length > 0;
  links.push({
    name: "audit-kept",
    ok: kept,
    detail: kept
      ? `the failed execution's record is still readable (${input.journal.length} entries, trace ${input.traceStatus})`
      : `the failed execution's record is incomplete (trace ${input.traceStatus}, ${input.journal.length} entries, request ${
          keptRequest ? "kept" : "missing"
        })`
  });

  // The point of design 14.2: a crash is not answered with "call the model again". The recovery
  // attempt has to refuse, and the task must not have grown a second execution.
  const refused =
    input.retry.status === 409 && /recovery decided/i.test(input.retry.message ?? "");
  const single = input.executionIds.length === 1 && input.executionIds[0] === input.expectedExecutionId;
  const noRerun = refused && single;
  links.push({
    name: "no-silent-rerun",
    ok: noRerun,
    detail: noRerun
      ? `the recovery attempt refused to re-run the model (${input.retry.status}) and the task still has one execution`
      : !refused
        ? `the recovery attempt answered ${input.retry.status} instead of refusing`
        : `the task produced ${input.executionIds.length} execution(s): ${input.executionIds.join(",") || "none"}`
  });

  const unhealthy = input.runtimeHealthy === false;
  links.push({
    name: "runtime-unhealthy",
    ok: unhealthy,
    detail: unhealthy
      ? "the registry reports the crashed runtime as unhealthy"
      : "the registry still reports the crashed runtime as healthy"
  });

  return { links, ok: links.every(link => link.ok) };
}

/**
 * P0-020: one real run, and the runtime dies underneath it.
 */
export async function executeRuntimeCrash(
  client: CrashClient,
  options: CrashOptions
): Promise<TestResult> {
  const now = options.now ?? (() => new Date());
  const startedAt = now().toISOString();
  const evidence: string[] = [];
  const failures: string[] = [];

  const taskId = "task-p0-020";
  const executionId = await client.start({
    taskId,
    // A turn that takes a while: the crash has to land while the runtime is working, not before
    // the model has produced anything.
    prompt:
      "Explain in at least 300 words how an append-only write-ahead log makes crash recovery " +
      "possible, and what a reader has to do to replay it. Write plain prose, no bullet points."
  });
  evidence.push(`executionId=${executionId}`);

  // Wait until the runtime is really working on it.
  const activity = await waitForOutput(client, executionId, options.activityTimeoutMs ?? 120_000);
  evidence.push(`eventsBeforeCrash=${activity.events}`);
  evidence.push(`sawModelOutput=${activity.sawOutput}`);
  if (!activity.sawOutput) {
    // The crash was never exercised against a running turn, so the probe has not tested the thing
    // (D-020). That is neither a pass nor a failure.
    return {
      id: "P0-020",
      title: "运行时进程崩溃：执行落到失败终态且不静默重跑",
      status: "blocked",
      startedAt,
      finishedAt: now().toISOString(),
      evidence,
      notes: "the runtime produced no output before the timeout, so a crash was never exercised"
    };
  }

  const kill = options.killRuntime();
  evidence.push(`runtimeAliveBeforeKill=${kill.aliveBefore}`);
  evidence.push(`runtimeAliveAfterKill=${kill.aliveAfter}`);

  const status = await waitForTerminal(client, executionId, options.runTimeoutMs ?? 120_000);
  evidence.push(`status=${status}`);

  const journal = await client.journal(executionId);
  const executionIds = await client.taskExecutions(taskId);
  const traceStatus = await client.traceStatus(executionId);
  const retry = await client.retry(executionId);
  const runtimeHealthy = await client.runtimeHealthy("hermes-local");
  evidence.push(`journalEvents=${journal.length}`);
  evidence.push(`taskExecutions=${executionIds.join(",") || "none"}`);
  evidence.push(`traceStatus=${traceStatus}`);
  evidence.push(`retryStatus=${retry.status}`);
  evidence.push(`retryMessage=${retry.message ?? ""}`);
  evidence.push(`runtimeHealthy=${runtimeHealthy}`);

  const judgement = judgeRuntimeCrash({
    status,
    journal,
    traceStatus,
    executionIds,
    expectedExecutionId: executionId,
    retry,
    runtimeHealthy,
    kill
  });
  for (const link of judgement.links) {
    evidence.push(`${link.name}:${link.ok ? "ok" : "missing"} ${link.detail}`);
  }
  for (const link of judgement.links) {
    if (!link.ok) failures.push(`the ${link.name} link does not hold: ${link.detail}`);
  }

  return {
    id: "P0-020",
    title: "运行时进程崩溃：执行落到失败终态且不静默重跑",
    status: failures.length === 0 ? "passed" : "failed",
    startedAt,
    finishedAt: now().toISOString(),
    evidence,
    ...(failures.length > 0 ? { notes: failures.join("; ") } : {})
  };
}

async function waitForOutput(
  client: CrashClient,
  executionId: string,
  timeoutMs: number
): Promise<{ events: number; sawOutput: boolean }> {
  const deadline = Date.now() + timeoutMs;
  let events = 0;
  let sawOutput = false;
  while (Date.now() < deadline) {
    const entries = await client.journal(executionId);
    events = entries.length;
    sawOutput = entries.some(entry => entry.category === "message.delta" || entry.category === "message.completed");
    if (sawOutput) break;
    if (entries.some(entry => entry.category === "execution.completed" || entry.category === "execution.failed")) break;
    await sleep(300);
  }
  return { events, sawOutput };
}

async function waitForTerminal(client: CrashClient, executionId: string, timeoutMs: number): Promise<string> {
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

/** The worktree has to be a real repository: the context snapshot reads its git base. */
function git(args: string[], cwd: string): void {
  const result = spawnSync("git", args, { cwd, encoding: "utf8" });
  if (result.status !== 0) {
    throw new Error(`git ${args.join(" ")} failed: ${result.stderr}`);
  }
}

// --------------------------------------------------------------------------- the pinned gateway

/** The lab root, so the probe can find `scripts/hermes.ps1` and `hermes.lock.json`. */
const LAB_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

interface Gateway {
  /** The port the gateway answers on. */
  port: number;
  /** The bearer token the API server demands. */
  apiKey: string;
  /** Whether the gateway was running before the probe touched it. */
  wasRunning: boolean;
  /** Kills the gateway process. */
  kill(): CrashKill;
  /** Brings the gateway back to the state the probe found it in. */
  restore(): Promise<void>;
}

async function readProfileEnv(profileDir: string): Promise<Record<string, string>> {
  const values: Record<string, string> = {};
  const text = await readFile(join(profileDir, ".env"), "utf8");
  for (const line of text.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (trimmed === "" || trimmed.startsWith("#") || !trimmed.includes("=")) continue;
    const name = trimmed.slice(0, trimmed.indexOf("=")).trim();
    values[name] = trimmed.slice(trimmed.indexOf("=") + 1).trim();
  }
  return values;
}

function portIsOpen(port: number, timeoutMs = 1500): Promise<boolean> {
  return new Promise(resolveOpen => {
    const socket = connect({ host: "127.0.0.1", port });
    const finish = (open: boolean) => {
      socket.destroy();
      resolveOpen(open);
    };
    socket.setTimeout(timeoutMs);
    socket.once("connect", () => finish(true));
    socket.once("timeout", () => finish(false));
    socket.once("error", () => finish(false));
  });
}

function listenerPid(port: number): number | undefined {
  const result = spawnSync(
    "powershell",
    [
      "-NoProfile",
      "-Command",
      `(Get-NetTCPConnection -LocalPort ${port} -State Listen -ErrorAction SilentlyContinue | Select-Object -First 1).OwningProcess`
    ],
    { encoding: "utf8", timeout: 30_000 }
  );
  const text = (result.stdout ?? "").trim();
  const pid = Number(text);
  return Number.isInteger(pid) && pid > 0 ? pid : undefined;
}

function startGateway(port: number): void {
  runHermesScript("start", port);
}

function stopGateway(port: number): void {
  runHermesScript("stop", port);
  // The script only stops a gateway it started itself. If the port is still answering afterwards,
  // the listener is somebody else's process and the probe leaves it alone.
  if (listenerPid(port) !== undefined) {
    const pid = listenerPid(port);
    if (pid !== undefined) {
      spawnSync("taskkill", ["/PID", String(pid), "/T", "/F"], { encoding: "utf8", timeout: 30_000 });
    }
  }
}

function runHermesScript(action: "start" | "stop", port: number): void {
  const script = join(LAB_ROOT, "scripts", "hermes.ps1");
  // The output goes to a file, never to a pipe: the gateway is a background process that inherits
  // the handle it was started with, and a pipe would stay open for as long as the gateway lives —
  // so the caller would wait for the gateway to exit before it could crash it.
  const logPath = join(tmpdir(), `codeweaver-hermes-${action}-${process.pid}.log`);
  const handle = openSync(logPath, "a");
  let status: number | null = null;
  let output = "";
  try {
    const result = spawnSync(
      "powershell",
      ["-ExecutionPolicy", "Bypass", "-NoProfile", "-File", script, action, "-Port", String(port)],
      { stdio: ["ignore", handle, handle], timeout: 300_000 }
    );
    status = result.status;
  } finally {
    closeSync(handle);
  }
  try {
    output = readFileSync(logPath, "utf8").trim();
  } catch {
    output = "";
  } finally {
    rmSync(logPath, { force: true });
  }
  if (action === "start" && status !== 0) {
    throw new Error(`the pinned Hermes gateway did not start on port ${port}: ${output}`);
  }
}

/**
 * The gateway the probe crashes.
 *
 * It is the pinned installation's own gateway, started through the project's own lifecycle
 * script, and the probe puts the machine back the way it found it: a gateway that was running is
 * started again, one that was not is left stopped.
 */
async function prepareGateway(): Promise<Gateway> {
  const lock = JSON.parse(await readFile(join(LAB_ROOT, "hermes.lock.json"), "utf8")) as { version: string };
  const profileDir = join(process.env.LOCALAPPDATA ?? "", "hermes", `hermes-agent-${lock.version}`, "profile");
  const port = Number(process.env.HERMES_CRASH_PORT ?? 8642);
  const env = await readProfileEnv(profileDir);
  const apiKey = env["API_SERVER_KEY"] ?? "";
  if (apiKey === "") {
    throw new Error("the Hermes profile holds no API_SERVER_KEY, so the gateway cannot be reached");
  }

  const wasRunning = await portIsOpen(port);
  if (!wasRunning) startGateway(port);

  return {
    port,
    apiKey,
    wasRunning,
    kill: () => {
      const pid = listenerPid(port);
      const aliveBefore = pid !== undefined;
      if (pid !== undefined) {
        spawnSync("taskkill", ["/PID", String(pid), "/T", "/F"], { encoding: "utf8", timeout: 30_000 });
      }
      // Give the operating system a moment to close the socket before it is looked at again.
      const deadline = Date.now() + 10_000;
      let aliveAfter = aliveBefore;
      while (Date.now() < deadline && aliveAfter) {
        aliveAfter = listenerPid(port) !== undefined;
        if (aliveAfter) sleepQuietly(300);
      }
      return { aliveBefore, aliveAfter };
    },
    restore: async () => {
      if (wasRunning) {
        // The crash took it down; the machine had it running, so it has to be running again.
        if (!(await portIsOpen(port))) startGateway(port);
        return;
      }
      // The probe started it, so the probe stops it: the machine is left the way it was found.
      if (await portIsOpen(port)) stopGateway(port);
    }
  };
}

function sleepQuietly(ms: number): void {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    // A synchronous wait: the kill path is already inside a synchronous helper.
  }
}

// --------------------------------------------------------------------------------- the real run

async function main(): Promise<void> {
  const reportPath = resolve(
    process.env.RUNTIME_CRASH_REPORT_PATH ?? "reports/phase0-runtime-crash.json"
  );
  const gateway = await prepareGateway();
  const repo = await mkdtemp(join(tmpdir(), "codeweaver-crash-repo-"));
  const dataDir = await mkdtemp(join(tmpdir(), "codeweaver-crash-data-"));
  await writeFile(join(repo, "app.ts"), "export const value = 1;\n", "utf8");
  await writeFile(join(repo, "spec.md"), "# Spec\nNothing to change; this run only has to crash.\n", "utf8");
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
    hermesBaseUrl: `http://127.0.0.1:${gateway.port}`,
    hermesApiKey: gateway.apiKey,
    hermesTimeoutMs: 30_000
  });
  await runtime.ready();
  const server = await createApiServer(runtime.deps).listen(0);

  try {
    const client = httpClient(server.url, repo);
    const results = [await executeRuntimeCrash(client, { killRuntime: () => gateway.kill() })];

    await writeTestReport(reportPath, results);
    console.log(`Phase 0 runtime-crash report: ${reportPath}`);
    for (const item of results) {
      console.log(`${item.id} ${item.status}: ${item.title}`);
      for (const line of item.evidence) console.log(`  ${line}`);
      if (item.notes) console.log(`  notes: ${item.notes}`);
    }
    if (results.some(item => item.status === "failed")) process.exitCode = 1;
  } finally {
    await server.close();
    await runtime.close();
    await rm(repo, { recursive: true, force: true });
    await rm(dataDir, { recursive: true, force: true });
    await gateway.restore();
  }
}

/** The real client: everything goes over HTTP, the way the console drives it. */
function httpClient(baseUrl: string, workspace: string): CrashClient {
  return {
    start: async input => {
      const response = await fetch(`${baseUrl}/api/v1/tasks/${input.taskId}/executions`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          projectId: "probe-runtime-crash",
          prompt: input.prompt,
          worktreePath: workspace,
          documents: ["spec.md"],
          selectedFiles: ["app.ts"],
          architectureVersion: "1.0",
          policy: {
            snapshotId: "snap-runtime-crash",
            projectId: "probe-runtime-crash",
            taskId: input.taskId,
            executionId: "",
            worktreeId: "wt-runtime-crash",
            policyVersion: "1.0.0",
            grantedCapabilities: ["*"],
            allowedPaths: [workspace],
            expiresAt: "2099-01-01T00:00:00.000Z"
          },
          template: { name: "coding-agent", version: "1.4.2", contentHash: "e".repeat(64) },
          modelConfig: {},
          correlationId: input.taskId,
          timeoutSeconds: 300,
          // The crashed runtime is the pinned Hermes gateway: it runs out of process, so killing it
          // is a runtime crash and not the worker crash of P0-021.
          runtimeId: "hermes-local"
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
    journal: async executionId => {
      const response = await fetch(`${baseUrl}/api/v1/executions/${encodeURIComponent(executionId)}/logs`);
      const body = (await response.json()) as { entries?: CrashJournalEntry[] };
      return body.entries ?? [];
    },
    traceStatus: async executionId => {
      const response = await fetch(`${baseUrl}/api/v1/executions/${encodeURIComponent(executionId)}/trace`);
      return response.status;
    },
    taskExecutions: async taskId => {
      const response = await fetch(`${baseUrl}/api/v1/tasks/${encodeURIComponent(taskId)}/trace`);
      const body = (await response.json()) as { executions?: Array<{ executionId: string }> };
      return (body.executions ?? []).map(execution => execution.executionId);
    },
    retry: async executionId => {
      const response = await fetch(`${baseUrl}/api/v1/executions/${encodeURIComponent(executionId)}/retry`, {
        method: "POST"
      });
      const body = (await response.json().catch(() => ({}))) as { message?: string };
      return { status: response.status, ...(typeof body.message === "string" ? { message: body.message } : {}) };
    },
    runtimeHealthy: async runtimeId => {
      const response = await fetch(`${baseUrl}/api/v1/runtimes/${encodeURIComponent(runtimeId)}/health-check`, {
        method: "POST"
      });
      if (!response.ok) return false;
      const body = (await response.json()) as { healthy?: boolean };
      return body.healthy === true;
    }
  };
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : undefined;
if (invokedPath === import.meta.url) {
  await main();
}
