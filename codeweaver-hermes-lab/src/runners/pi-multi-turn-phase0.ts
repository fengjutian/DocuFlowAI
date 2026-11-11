import { randomBytes } from "node:crypto";
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
 * P0-007: a multi-turn session keeps the context it was given.
 *
 * The item asks for "session continuity evidence", which is the part that is easy to fake. A second
 * execution that happens to be launched with the same text is not a second turn: it starts with an
 * empty transcript. The evidence has to come from a runtime that kept the transcript, so the
 * follow-up goes to the same session through the same execution.
 *
 * Two nonces, not one. The first turn is asked to remember A and B and to answer with nothing but
 * "READY"; the follow-up asks for B only. An answer that carries B and not A cannot be a copy of
 * the previous turn, and it cannot be a lucky guess: both strings appear exactly once in the whole
 * conversation, in the first prompt.
 */

export interface PiPhase0Event {
  type: string;
  sequence?: number;
  data?: Record<string, unknown>;
}

export interface PiMultiTurnClient {
  start(input: { taskId: string; prompt: string }): Promise<string>;
  followUp(executionId: string, prompt: string): Promise<{ accepted: boolean; status: number; body: string }>;
  events(executionId: string): Promise<PiPhase0Event[]>;
}

export interface PiMultiTurnOptions {
  /** The first nonce, remembered but not asked for. */
  nonceA?: string;
  /** The second nonce, asked for by the follow-up. */
  nonceB?: string;
  now?: () => Date;
  runTimeoutMs?: number;
}

export function defaultNonces(): { a: string; b: string } {
  return {
    a: `CODEWEAVER_P0_007_A_${randomBytes(6).toString("hex").toUpperCase()}`,
    b: `CODEWEAVER_P0_007_B_${randomBytes(6).toString("hex").toUpperCase()}`
  };
}

const TERMINAL_EVENTS = new Set([
  "agent.execution.completed",
  "agent.execution.failed",
  "agent.execution.cancelled"
]);

export async function executeMultiTurnSession(
  client: PiMultiTurnClient,
  options: PiMultiTurnOptions = {}
): Promise<TestResult> {
  const now = options.now ?? (() => new Date());
  const nonces = options.nonceA && options.nonceB
    ? { a: options.nonceA, b: options.nonceB }
    : defaultNonces();
  const startedAt = now().toISOString();
  const evidence: string[] = [];
  const failures: string[] = [];

  const executionId = await client.start({
    taskId: "task-p0-007",
    prompt: [
      `Remember these two strings exactly:`,
      `A = ${nonces.a}`,
      `B = ${nonces.b}`,
      `Do not output them now. Reply with exactly: READY`
    ].join("\n")
  });
  evidence.push(`executionId=${executionId}`);
  evidence.push(`nonceA=${nonces.a}`);
  evidence.push(`nonceB=${nonces.b}`);

  const firstTurn = await waitForTurn(client, executionId, options.runTimeoutMs ?? 180_000, 1);
  evidence.push(`firstTurnTerminal=${firstTurn.terminal?.type ?? "none"}`);
  if (firstTurn.terminal?.type !== "agent.execution.completed") {
    failures.push(`the first turn ended as '${firstTurn.terminal?.type ?? "none"}'`);
  }

  const followUp = await client.followUp(
    executionId,
    `Now output the value of B only, exactly as it was given. Do not output anything else.`
  );
  evidence.push(`followUpStatus=${followUp.status}`);
  if (!followUp.accepted) {
    failures.push(`the follow-up was refused: ${followUp.status} ${followUp.body}`);
    return {
      id: "P0-007",
      title: "多轮任务执行：会话保留上下文",
      status: "failed",
      startedAt,
      finishedAt: now().toISOString(),
      evidence,
      notes: failures.join("; ")
    };
  }

  const secondTurn = await waitForTurn(client, executionId, options.runTimeoutMs ?? 180_000, 2);
  evidence.push(`secondTurnTerminal=${secondTurn.terminal?.type ?? "none"}`);
  evidence.push(`turnMarker=${String(secondTurn.hasTurnMarker)}`);
  evidence.push(`secondTurnChars=${secondTurn.text.length}`);
  evidence.push(`secondTurnHasB=${String(secondTurn.text.includes(nonces.b))}`);
  evidence.push(`secondTurnHasA=${String(secondTurn.text.includes(nonces.a))}`);

  if (secondTurn.terminal?.type !== "agent.execution.completed") {
    failures.push(`the second turn ended as '${secondTurn.terminal?.type ?? "none"}'`);
  }
  if (!secondTurn.hasTurnMarker) {
    failures.push("the journal has no turn boundary between the two turns");
  }
  // B is only in the first prompt, so an answer that carries it came out of the retained transcript.
  if (!secondTurn.text.includes(nonces.b)) {
    failures.push("the follow-up did not answer with the string the first turn was given");
  }
  // A was never asked for. An answer that carries it too is the model dumping its context rather
  // than answering the question, which is not evidence of continuity either.
  if (secondTurn.text.includes(nonces.a)) {
    failures.push("the follow-up repeated the string it was not asked for");
  }

  return {
    id: "P0-007",
    title: "多轮任务执行：会话保留上下文",
    status: failures.length === 0 ? "passed" : "failed",
    startedAt,
    finishedAt: now().toISOString(),
    evidence,
    ...(failures.length > 0 ? { notes: failures.join("; ") } : {})
  };
}

interface TurnView {
  /** The terminal event of the turn, if it arrived. */
  terminal?: PiPhase0Event;
  /** The assistant text of this turn. */
  text: string;
  /** Whether the journal marks the start of a follow-up turn. */
  hasTurnMarker: boolean;
}

/**
 * Waits for the n-th terminal event of an execution and returns that turn's answer.
 *
 * The text is cut at the turn boundary, so the answer belongs to this turn and not to the earlier
 * one: a model that repeats its previous answer must not be able to pass by accident.
 */
async function waitForTurn(
  client: PiMultiTurnClient,
  executionId: string,
  timeoutMs: number,
  turn: number
): Promise<TurnView> {
  const deadline = Date.now() + timeoutMs;
  let view: TurnView = { text: "", hasTurnMarker: false };
  while (Date.now() < deadline) {
    const events = await client.events(executionId);
    const markerIndex = events.findIndex(event => event.type === "codeweaver.turn.started");
    const terminals = events.filter(event => TERMINAL_EVENTS.has(event.type));
    const terminal = terminals[turn - 1];
    const scoped = markerIndex >= 0 ? events.slice(markerIndex + 1) : events;
    view = {
      ...(terminal ? { terminal } : {}),
      text: scoped
        .filter(event => event.type === "agent.message.delta")
        .map(event => String(event.data?.["delta"] ?? ""))
        .join(""),
      hasTurnMarker: markerIndex >= 0
    };
    if (terminal && scoped.length > 0) return view;
    await sleep(300);
  }
  return view;
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
    process.env.PI_MULTI_TURN_REPORT_PATH ?? "reports/phase0-pi-multi-turn.json"
  );
  const model = process.env.PI_PROBE_MODEL ?? "MiniMax-M3";
  if (!process.env.MINIMAX_CN_API_KEY) {
    throw new Error("MINIMAX_CN_API_KEY is not set; nothing to probe.");
  }

  const repo = await mkdtemp(join(tmpdir(), "codeweaver-pi-multi-turn-repo-"));
  const dataDir = await mkdtemp(join(tmpdir(), "codeweaver-pi-multi-turn-data-"));
  await writeFile(join(repo, "README.md"), "# Multi-turn probe\n", "utf8");
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

  const client: PiMultiTurnClient = {
    start: async input => {
      const response = await fetch(`${server.url}/api/v1/tasks/${input.taskId}/executions`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          projectId: "probe-pi-multi-turn",
          prompt: input.prompt,
          worktreePath: repo,
          documents: ["README.md"],
          selectedFiles: ["README.md"],
          architectureVersion: "1.0",
          policy: {
            snapshotId: "snap-multi-turn",
            projectId: "probe-pi-multi-turn",
            taskId: input.taskId,
            executionId: "",
            worktreeId: "wt-multi-turn",
            policyVersion: "1.0.0",
            grantedCapabilities: ["*"],
            allowedPaths: [repo],
            expiresAt: "2099-01-01T00:00:00.000Z"
          },
          template: { name: "coding-agent", version: "1.4.2", contentHash: "e".repeat(64) },
          modelConfig: { model },
          correlationId: input.taskId,
          timeoutSeconds: 300,
          allowedTools: [],
          runtimeId: "pi-local"
        })
      });
      if (response.status !== 201) {
        throw new Error(`the task was refused: ${response.status} ${await response.text()}`);
      }
      const created = (await response.json()) as { execution: { executionId: string } };
      return created.execution.executionId;
    },
    followUp: async (executionId, prompt) => {
      const response = await fetch(
        `${server.url}/api/v1/executions/${encodeURIComponent(executionId)}/follow-up`,
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ prompt })
        }
      );
      return {
        accepted: response.status === 202,
        status: response.status,
        body: (await response.text()).slice(0, 300)
      };
    },
    events: async executionId => {
      const response = await fetch(
        `${server.url}/api/v1/executions/${encodeURIComponent(executionId)}/events`
      );
      const body = (await response.json()) as { events?: PiPhase0Event[] };
      return body.events ?? [];
    }
  };

  const results = [await executeMultiTurnSession(client)];
  await writeTestReport(reportPath, results);
  console.log(`Phase 0 multi-turn report: ${reportPath}`);
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

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : undefined;
if (invokedPath === import.meta.url) {
  await main();
}
