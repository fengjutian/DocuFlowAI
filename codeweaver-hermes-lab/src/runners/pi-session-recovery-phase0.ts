import { spawnSync } from "node:child_process";
import { cp, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { createApiServer } from "../api/app.js";
import { writeTestReport } from "../reporting/report-writer.js";
import { createRuntime } from "../server/runtime.js";
import type { JournalStore } from "../journal/journal.js";
import type { TestResult } from "../types/hermes.js";

/**
 * P0-021: session recovery, against the Pi runtime and a real model.
 *
 * The checklist asks for two things: recover a session that can be recovered, or rebuild safely.
 * Which one applies is decided by the runtime, not by us, and for Pi the answer is decided by
 * where Pi keeps its transcript: on the agent instance, inside this process. A worker that dies
 * takes the session with it, so the only honest recovery is a rebuild — a new execution that
 * carries the checkpoint of the old one, on the same runtime.
 *
 * The probe therefore stages the fourth failure the design lists ("CodeWeaver Worker crash"):
 *
 * - a real task runs on the real model and is still producing output;
 * - the worker dies, and the journal survives it;
 * - the restarted worker has to decide, and the decision has to be a rebuild;
 * - the rebuild has to actually run, on the same runtime, with the checkpoint in its request.
 *
 * A probe that only checked the plan would prove nothing: the rebuild is the part that can fail.
 */

/** The marker the checkpoint block carries; the rebuild's request has to contain it. */
const CHECKPOINT_MARKER = "[CODEWEAVER CHECKPOINT]";

/**
 * A task that keeps the model busy.
 *
 * The worker has to die while the turn is still running, so the task cannot be one the model
 * finishes instantly. The rebuild re-asks the same task, so it also has to finish.
 */
const DEFAULT_PROMPT =
  "List the integers from 1 to 120, one per line, each followed by a short remark. " +
  "Do not stop early.";

export interface RecoveryEvent {
  type: string;
  data?: Record<string, unknown>;
}

export interface RecoveryBody {
  previousExecutionId?: string;
  execution?: { executionId?: string };
  plan?: { action?: string; reason?: string; fromSequence?: number };
  error?: string;
}

/** The slice of the system this probe drives; the real one talks to a server over the wire. */
export interface PiSessionRecoveryClient {
  /** Starts the execution that the crash will interrupt. */
  start(): Promise<string>;
  /** Waits until the runtime is producing output; false when it never did. */
  waitForOutput(executionId: string, timeoutMs: number): Promise<boolean>;
  /** The execution's events, in journal order. */
  events(executionId: string): Promise<RecoveryEvent[]>;
  /**
   * The worker dies: the session is gone, the journal is not.
   *
   * The real implementation replaces the whole runtime; a fake one only has to make the session
   * unreachable.
   */
  crash(): Promise<void>;
  /** The status the restarted worker reports for an execution. */
  status(executionId: string): Promise<string>;
  /** The recovery decision for an execution whose session is gone. */
  recover(executionId: string): Promise<{ status: number; body: RecoveryBody }>;
  /** A journaled audit fact of an execution. */
  fact(executionId: string, category: string): Promise<Record<string, unknown> | undefined>;
  /** Waits for a terminal state and returns it. */
  waitForTerminal(executionId: string, timeoutMs: number): Promise<string>;
}

export interface PiSessionRecoveryOptions {
  prompt?: string;
  /** How long to wait for the model to start producing output. */
  startTimeoutMs?: number;
  /** How long to wait for the rebuilt execution to finish. */
  terminalTimeoutMs?: number;
  now?: () => Date;
}

const TERMINAL_STATUSES = new Set(["succeeded", "failed", "cancelled", "interrupted"]);

export async function executePiSessionRecoveryPhase0(
  client: PiSessionRecoveryClient,
  options: PiSessionRecoveryOptions = {}
): Promise<TestResult[]> {
  const now = options.now ?? (() => new Date());
  const startedAt = now().toISOString();
  const startTimeoutMs = options.startTimeoutMs ?? 120_000;
  const terminalTimeoutMs = options.terminalTimeoutMs ?? 180_000;
  const evidence: string[] = [];

  const executionId = await client.start();
  evidence.push(`executionId=${executionId}`);

  // The crash has to interrupt a running turn. A task that never started leaves nothing to
  // recover, and a task that already finished is not a crash.
  const streaming = await client.waitForOutput(executionId, startTimeoutMs);
  if (!streaming) {
    return [
      {
        id: "P0-021",
        title: "Session resume/rebuild",
        status: "failed",
        startedAt,
        finishedAt: now().toISOString(),
        evidence,
        notes: "The runtime never produced output, so there was no running turn to interrupt"
      }
    ];
  }
  evidence.push("runtimeWasStreaming=true");

  await client.crash();

  // The journal is what survives the worker, so the interrupted execution is still readable.
  const survivingEvents = await client.events(executionId);
  evidence.push(`survivingEvents=${survivingEvents.length}`);
  if (survivingEvents.length === 0) {
    return [
      {
        id: "P0-021",
        title: "Session resume/rebuild",
        status: "failed",
        startedAt,
        finishedAt: now().toISOString(),
        evidence,
        notes: "The journal did not survive the crash, so there was nothing to recover from"
      }
    ];
  }

  const recovery = await client.recover(executionId);
  evidence.push(`recoveryStatus=${recovery.status}`);
  evidence.push(`planAction=${recovery.body.plan?.action ?? "none"}`);
  evidence.push(`planReason=${recovery.body.plan?.reason ?? "none"}`);
  if (recovery.body.error) evidence.push(`recoveryError=${recovery.body.error}`);

  const failures: string[] = [];
  const planAction = recovery.body.plan?.action;

  if (recovery.status !== 202) {
    failures.push(`the recovery request was refused with ${recovery.status}`);
  }
  // Pi's transcript died with the worker, so resuming it is impossible. A plan that says "resume"
  // would be a plan to talk to a session that no longer exists.
  if (planAction !== "rebuild") {
    failures.push(`the recovery plan was '${String(planAction)}' instead of 'rebuild'`);
  }

  const rebuiltId = recovery.body.execution?.executionId;
  if (!rebuiltId) {
    failures.push("the rebuild did not create an execution");
  } else {
    evidence.push(`rebuiltExecutionId=${rebuiltId}`);
    if (rebuiltId === executionId) {
      failures.push("the rebuild reused the interrupted execution instead of creating a new one");
    }

    // The rebuild has to run on the runtime that ran the original task, not on whatever the
    // restarted worker happens to default to.
    const runtimeFact = await client.fact(rebuiltId, "execution.runtime");
    const runtimeId = runtimeFact?.["runtimeId"];
    evidence.push(`rebuiltRuntimeId=${String(runtimeId)}`);
    if (runtimeId !== "pi-local") {
      failures.push(`the rebuild ran on '${String(runtimeId)}' instead of the runtime that ran the original`);
    }

    // The rebuild is a continuation of the same work, so its request has to carry the checkpoint.
    const inputFact = await client.fact(rebuiltId, "execution.input");
    const prompt = inputFact?.["prompt"];
    const hasCheckpoint = typeof prompt === "string" && prompt.includes(CHECKPOINT_MARKER);
    const mentionsOriginal =
      typeof prompt === "string" && prompt.includes(executionId);
    evidence.push(`rebuiltPromptHasCheckpoint=${hasCheckpoint}`);
    evidence.push(`rebuiltPromptNamesOriginal=${mentionsOriginal}`);
    if (!hasCheckpoint) {
      failures.push("the rebuilt request does not carry the checkpoint of the interrupted execution");
    }
    if (!mentionsOriginal) {
      failures.push("the rebuilt request does not name the execution it continues");
    }

    // The plan is not the recovery: the rebuild has to actually run.
    const rebuiltStatus = await client.waitForTerminal(rebuiltId, terminalTimeoutMs);
    evidence.push(`rebuiltStatus=${rebuiltStatus}`);
    if (!TERMINAL_STATUSES.has(rebuiltStatus)) {
      failures.push(`the rebuilt execution never reached a terminal state (last status '${rebuiltStatus}')`);
    }
    const rebuiltEvents = await client.events(rebuiltId);
    evidence.push(`rebuiltEvents=${rebuiltEvents.length}`);
    if (rebuiltEvents.length === 0) {
      failures.push("the rebuilt execution produced no events");
    }
  }

  return [
    {
      id: "P0-021",
      title: "Session resume/rebuild",
      status: failures.length === 0 ? "passed" : "failed",
      startedAt,
      finishedAt: now().toISOString(),
      evidence,
      ...(failures.length > 0 ? { notes: failures.join("; ") } : {})
    }
  ];
}

async function main(): Promise<void> {
  const reportPath = resolve(
    process.env.PI_SESSION_RECOVERY_REPORT_PATH ?? "reports/phase0-pi-session-recovery.json"
  );
  const model = process.env.PI_PROBE_MODEL ?? "MiniMax-M3";
  if (!process.env.MINIMAX_CN_API_KEY) {
    throw new Error("MINIMAX_CN_API_KEY is not set; nothing to probe.");
  }

  const repo = await mkdtemp(join(tmpdir(), "codeweaver-pi-recovery-repo-"));
  const dataDir = await mkdtemp(join(tmpdir(), "codeweaver-pi-recovery-data-"));
  git(["init", "-b", "main"], repo);
  git(["config", "user.name", "CodeWeaver Probe"], repo);
  git(["config", "user.email", "probe@codeweaver.local"], repo);
  await writeFile(join(repo, "README.md"), "# Session recovery probe\n", "utf8");
  git(["add", "-A"], repo);
  git(["commit", "-m", "initial"], repo);

  const makeRuntime = (dir: string) =>
    createRuntime({
      host: "127.0.0.1",
      port: 0,
      dataDir: dir,
      worktreeRoot: join(dir, "worktrees"),
      repository: repo,
      // Hermes is registered so the server has a default runtime; this probe never talks to it.
      hermesBaseUrl: "http://127.0.0.1:9",
      hermesTimeoutMs: 5_000,
      pi: { provider: "minimax-cn", model }
    });

  // The worker that will crash.
  let runtime = await makeRuntime(dataDir);
  await runtime.ready();
  let server = await createApiServer(runtime.deps).listen(0);
  let url = server.url;

  const startBody = {
    projectId: "probe-pi-recovery",
    prompt: DEFAULT_PROMPT,
    worktreePath: repo,
    documents: ["README.md"],
    selectedFiles: ["README.md"],
    architectureVersion: "1.0",
    policy: {
      snapshotId: "snap-recovery",
      projectId: "probe-pi-recovery",
      taskId: "task-pi-recovery",
      executionId: "",
      worktreeId: "wt-recovery",
      policyVersion: "1.0.0",
      grantedCapabilities: ["*"],
      allowedPaths: [repo],
      expiresAt: "2099-01-01T00:00:00.000Z"
    },
    template: { name: "coding-agent", version: "1.4.2", contentHash: "e".repeat(64) },
    modelConfig: { model },
    correlationId: "probe-pi-recovery",
    timeoutSeconds: 600,
    allowedTools: [],
    runtimeId: "pi-local"
  };

  const readEvents = async (executionId: string): Promise<RecoveryEvent[]> => {
    const response = await fetch(`${url}/api/v1/executions/${executionId}/events`);
    const body = await response.json();
    return (body.events ?? []) as RecoveryEvent[];
  };

  const client: PiSessionRecoveryClient = {
    start: async () => {
      const response = await fetch(`${url}/api/v1/tasks/task-pi-recovery/executions`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(startBody)
      });
      if (response.status !== 201) {
        throw new Error(`the task was refused: ${response.status} ${await response.text()}`);
      }
      const created = await response.json();
      return String(created.execution.executionId);
    },
    waitForOutput: async (executionId, timeoutMs) => {
      const deadline = Date.now() + timeoutMs;
      while (Date.now() < deadline) {
        const events = await readEvents(executionId);
        if (events.some(event => event.type === "agent.message.delta")) return true;
        const status = await client.status(executionId);
        if (TERMINAL_STATUSES.has(status)) return false;
        await sleep(200);
      }
      return false;
    },
    events: readEvents,
    crash: async () => {
      // The worker's disk at the moment it died: the journal, with everything recorded so far.
      const crashedDir = await mkdtemp(join(tmpdir(), "codeweaver-pi-recovery-crashed-"));
      await cp(dataDir, crashedDir, { recursive: true });
      await server.close();
      await runtime.close();

      // The restarted worker: same records, no session, no memory of the task.
      runtime = await makeRuntime(crashedDir);
      await runtime.ready();
      server = await createApiServer(runtime.deps).listen(0);
      url = server.url;
    },
    status: async executionId => {
      const response = await fetch(`${url}/api/v1/executions/${executionId}`);
      if (response.status !== 200) return "unknown";
      const body = await response.json();
      return String(body.status ?? "unknown");
    },
    recover: async executionId => {
      const response = await fetch(`${url}/api/v1/executions/${executionId}/retry`, { method: "POST" });
      const body = (await response.json()) as RecoveryBody;
      return { status: response.status, body };
    },
    fact: async (executionId, category) => {
      const journal = runtime.deps.journal as JournalStore;
      const entries = await journal.read(executionId);
      return entries.find(entry => entry.category === category)?.payload;
    },
    waitForTerminal: async (executionId, timeoutMs) => {
      const deadline = Date.now() + timeoutMs;
      let status = "unknown";
      while (Date.now() < deadline) {
        status = await client.status(executionId);
        if (TERMINAL_STATUSES.has(status)) return status;
        await sleep(500);
      }
      return status;
    }
  };

  const results = await executePiSessionRecoveryPhase0(client);
  await writeTestReport(reportPath, results);
  console.log(`Phase 0 session recovery report: ${reportPath}`);
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

function sleep(ms: number): Promise<void> {
  return new Promise(resolveSleep => setTimeout(resolveSleep, ms));
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
