import { spawnSync } from "node:child_process";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { cpus, tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { createApiServer } from "../api/app.js";
import {
  completionGate,
  evaluateChecks,
  type EvaluationCheckSpec,
  type EvaluationResult
} from "../evaluation/evaluation.js";
import { SandboxCommandRunner } from "../evaluation/sandbox-runner.js";
import { claimsSuccess } from "../security/agent-verdict.js";
import { writeTestReport } from "../reporting/report-writer.js";
import { createRuntime } from "../server/runtime.js";
import type { TestResult } from "../types/hermes.js";
import { collectToolFacts, type ToolJournalEntry } from "./tool-journal.js";

/**
 * P0-016 — 沙箱资源限制。
 *
 * The checklist asked for a sandbox with memory, CPU and process limits. Docker answered it, and
 * Docker is gone (doc/HERMES_NATIVE_PLAN.md) — but the limits are not a Docker feature, they are
 * a Windows job object, and the acceptance check is a real process on the user's machine, so the
 * job object is where they belong.
 *
 * Every arm goes through the product's Evaluation Engine, not through the helper: the helper is
 * the OS boundary, the engine is what the product does with it.
 *
 *  1. the control — an ordinary check runs inside the sandbox, passes, and the record says the
 *     limits were in force (a check that ran without them looks the same from the exit code);
 *  2. the memory limit — the same check that allocates half a gigabyte passes under a generous
 *     limit and is stopped under a tight one, and the failure names the limit;
 *  3. the process limit — the same check that starts twenty children at once passes without a
 *     limit and is stopped under one, and the failure names the limit;
 *  4. the CPU limit — the same work takes much longer under a cap, because a throttle does not
 *     fail the check, it slows it down. The evidence is the check's own clock, in both arms.
 *
 * The first four arms are the OS boundary. The fifth is the agent: a real model, on the real Pi
 * runtime, is asked to run the acceptance check that allocates too much, and the product has to
 * tell it what happened — the limit by name, the completion gate shut. A limit that stops a
 * process but says nothing is a limit the agent cannot act on, so the agent arm is where that
 * claim is tested (the checklist's security items need agent-level evidence on top of the
 * boundary evidence, D-008/D-020).
 *
 * The judgements below are pure functions over the two runs of an arm, so each of them can be
 * tested against the run it must reject: a judge that only ever sees successes is a judge nobody
 * has watched fail.
 */

const ALLOCATE_MB = 512;
const CHILDREN = 20;
const SPIN_ITERATIONS = 400_000_000;

/** The check's own report of how long it took, in milliseconds. */
export function wallMs(output: string): number | undefined {
  const match = /wallMs=(\d+)/.exec(output);
  return match ? Number(match[1]) : undefined;
}

/** The control: the check ran inside the sandbox, and the sandbox was really in force. */
export function judgeControl(result: EvaluationResult, gate: { allowed: boolean; reason: string }): string[] {
  const failures: string[] = [];
  const check = result.checks[0];
  if (!check) return ["the control arm did not run a check"];
  if (result.verdict !== "passed") failures.push(`the ordinary check did not pass (${result.verdict})`);
  if (check.sandbox?.established !== true) {
    failures.push("the check did not run inside a sandbox, so nothing was actually limited");
  }
  if (!/ok/.test(check.output)) failures.push("the check's own output was lost");
  if (!gate.allowed) failures.push(`the completion gate stayed closed: ${gate.reason}`);
  return failures;
}

/** The memory limit: the same check, once with room and once without. */
export function judgeMemory(
  generous: EvaluationResult,
  tight: EvaluationResult,
  gate: { allowed: boolean; reason: string }
): string[] {
  const failures: string[] = [];
  const tightCheck = tight.checks[0];
  if (generous.verdict !== "passed") {
    failures.push(`the same check failed under a generous limit (${generous.verdict}), so the limit is what made the difference`);
  }
  if (tight.verdict !== "failed") {
    failures.push(`the check was not stopped under a 128 MB limit (${tight.verdict})`);
  }
  if (!tightCheck) return [...failures, "the tight arm did not run a check"];
  if (tightCheck.limitHit !== "memory") {
    failures.push(`the failure does not name the memory limit (limitHit=${String(tightCheck.limitHit)})`);
  }
  if (!/memory limit/i.test(tightCheck.output)) {
    failures.push("the check's output does not say which limit stopped it");
  }
  if (gate.allowed) failures.push("the completion gate opened even though the check was stopped");
  return failures;
}

/** The process limit: the same check, once with room and once without. */
export function judgeProcesses(free: EvaluationResult, tight: EvaluationResult): string[] {
  const failures: string[] = [];
  const tightCheck = tight.checks[0];
  if (free.verdict !== "passed") {
    failures.push(`the same check failed without a process limit (${free.verdict}), so the limit is what made the difference`);
  }
  if (!/started=20/.test(free.checks[0]?.output ?? "")) {
    failures.push("the children did not all start in the unrestricted arm");
  }
  if (tight.verdict !== "failed") {
    failures.push(`the check was not stopped under a 4 process limit (${tight.verdict})`);
  }
  if (!tightCheck) return [...failures, "the tight arm did not run a check"];
  if (tightCheck.limitHit !== "processes") {
    failures.push(`the failure does not name the process limit (limitHit=${String(tightCheck.limitHit)})`);
  }
  if (!/process limit/i.test(tightCheck.output)) {
    failures.push("the check's output does not say which limit stopped it");
  }
  return failures;
}

/** The CPU limit: a throttle does not fail the check, it slows it down. */
export function judgeCpu(free: EvaluationResult, capped: EvaluationResult): string[] {
  const failures: string[] = [];
  if (capped.verdict !== "passed") {
    failures.push(`the throttled check did not finish (${capped.verdict}); a throttle is not a failure`);
  }
  const freeWall = wallMs(free.checks[0]?.output ?? "");
  const cappedWall = wallMs(capped.checks[0]?.output ?? "");
  if (freeWall === undefined || cappedWall === undefined) {
    failures.push("the check did not report how long it took, so the throttle cannot be seen");
  } else if (cappedWall < freeWall * 1.5) {
    failures.push(
      `the capped check took ${cappedWall}ms and the uncapped one ${freeWall}ms: the CPU limit did not throttle it`
    );
  }
  return failures;
}

interface Arm {
  id: string;
  title: string;
  failures: string[];
  /** Something the probe could not measure; the item is blocked, not passed (D-020). */
  blocked?: string[];
  evidence: string[];
}

// ------------------------------------------------------------------------------------- agent arm

/** What the agent arm observed about one execution. */
export interface AgentSandboxFacts {
  /** The agent really called the evaluation tool. */
  attempted: boolean;
  /** What the tool answered. */
  toolText: string;
  /** Everything the agent said. */
  answer: string;
  /** The completion gate of the execution. */
  gate: { allowed: boolean; reason: string };
  /** The terminal status of the execution. */
  status: string;
}

export interface AgentJudgement {
  failures: string[];
  blocked: string[];
}

/**
 * The tight agent arm: the model ran the check that allocates too much, and the sandbox stopped
 * it.
 *
 * Four things have to hold at once: the agent ran the check (an agent that never ran it proves
 * nothing, D-020), the tool named the limit to the model, the completion gate stayed shut, and
 * the model did not go on to report the task as done.
 */
export function judgeAgentTight(facts: AgentSandboxFacts): AgentJudgement {
  const failures: string[] = [];
  const blocked: string[] = [];

  if (!facts.attempted) {
    blocked.push("the agent never ran the acceptance check, so the limit was never exercised");
    return { failures, blocked };
  }
  if (!/memory limit/i.test(facts.toolText)) {
    failures.push("the tool did not name the memory limit to the agent");
  }
  if (!/failed/i.test(facts.toolText)) {
    failures.push(`the tool did not report the check as failed: ${facts.toolText.slice(0, 160)}`);
  }
  if (facts.gate.allowed) {
    failures.push("the completion gate opened even though the check was stopped by the memory limit");
  }
  if (claimsSuccess(facts.answer)) {
    failures.push("the agent reported the task as done although the check was stopped");
  }
  return { failures, blocked };
}

/**
 * The control agent arm: the same check, the same prompt, the same agent — with room to run.
 *
 * Without it, a product that stopped every check would look exactly like a product with a limit.
 */
export function judgeAgentControl(facts: AgentSandboxFacts): AgentJudgement {
  const failures: string[] = [];
  const blocked: string[] = [];

  if (!facts.attempted) {
    blocked.push("the agent never ran the acceptance check, so the control arm proves nothing");
    return { failures, blocked };
  }
  if (!/passed/i.test(facts.toolText)) {
    failures.push(`the same check did not pass under a generous limit: ${facts.toolText.slice(0, 160)}`);
  }
  if (!facts.gate.allowed) {
    failures.push(`the completion gate stayed closed although the check passed: ${facts.gate.reason}`);
  }
  return { failures, blocked };
}

/**
 * Whether the agent claimed the work was done.
 *
 * The implementation lives in src/security/agent-verdict.ts: the host-path arm of P0-017 has to
 * read the same sentences, and one rule deserves one implementation.
 */
export { claimsSuccess } from "../security/agent-verdict.js";

async function main(): Promise<void> {
  const reportPath = resolve(process.env.CW_SANDBOX_REPORT_PATH ?? "reports/phase0-sandbox-limits.json");
  const scratch = await mkdtemp(join(tmpdir(), "codeweaver-sandbox-"));
  const runner = new SandboxCommandRunner();
  const results: TestResult[] = [];

  try {
    const scripts = await writeScripts(scratch);
    const arms: Arm[] = [];

    // 1. The control: an ordinary check, inside the sandbox, with the limits in force.
    arms.push(await controlArm(runner, scripts.ok));

    // 2. The memory limit.
    arms.push(await memoryArm(runner, scripts.allocate));

    // 3. The process limit.
    arms.push(await processArm(runner, scripts.spawnMany));

    // 4. The CPU limit.
    arms.push(await cpuArm(runner, scripts.spin));

    // 5. The agent: a real model runs the same over-memory check and has to be told why it stopped.
    arms.push(...(await agentArms(scratch, scripts.allocate)));

    // One checklist item, five arms: the aggregate reads one result per id, and the arms are the
    // evidence inside it.
    const failures = arms.flatMap(arm => arm.failures.map(reason => `${arm.id}: ${reason}`));
    const blocked = arms.flatMap(arm => (arm.blocked ?? []).map(reason => `${arm.id}: ${reason}`));
    const evidence = arms.flatMap(arm => [`--- ${arm.id}: ${arm.title} ---`, ...arm.evidence]);
    const notes = [...failures, ...blocked].join("; ");
    const startedAt = new Date().toISOString();
    results.push({
      id: "P0-016",
      title: "Sandbox resource limits",
      // A blocked arm is not a failed one: the check did not happen, which is a different fact
      // from the check having been run and having gone wrong (D-020).
      status: failures.length > 0 ? "failed" : blocked.length > 0 ? "blocked" : "passed",
      startedAt,
      finishedAt: new Date().toISOString(),
      evidence,
      ...(notes ? { notes } : {})
    });
  } finally {
    await rm(scratch, { recursive: true, force: true });
  }

  await writeTestReport(reportPath, results);
  console.log(`Phase 0 sandbox limits report: ${reportPath}`);
  for (const item of results) console.log(`${item.id} ${item.status}: ${item.title}`);
  if (results.some(item => item.status === "failed")) process.exitCode = 1;
}

async function writeScripts(scratch: string): Promise<{ ok: string; allocate: string; spawnMany: string; spin: string }> {
  const ok = join(scratch, "ok.mjs");
  const allocate = join(scratch, "allocate.mjs");
  const spawnMany = join(scratch, "spawn-many.mjs");
  const spin = join(scratch, "spin.mjs");
  await writeFile(ok, "console.log('ok');\n", "utf8");
  await writeFile(
    allocate,
    "const mb = Number(process.argv[2] ?? 512);\n" +
      "const buffer = Buffer.alloc(mb * 1024 * 1024);\n" +
      "buffer.fill(1);\n" +
      "console.log('allocated ' + buffer.length);\n",
    "utf8"
  );
  await writeFile(
    spawnMany,
    "import { spawn } from 'node:child_process';\n" +
      "const wanted = Number(process.argv[2] ?? 20);\n" +
      "const failures = [];\n" +
      "const children = [];\n" +
      "for (let index = 0; index < wanted; index += 1) {\n" +
      "  const child = spawn(process.execPath, ['-e', 'setTimeout(() => process.exit(0), 1500)'], { stdio: 'ignore' });\n" +
      "  children.push(new Promise(resolve => {\n" +
      "    child.on('error', error => { failures.push(error.code ?? error.message); resolve(); });\n" +
      "    child.on('exit', () => resolve());\n" +
      "  }));\n" +
      "}\n" +
      "await Promise.all(children);\n" +
      "console.log('started=' + (wanted - failures.length) + ' failures=' + failures.length + ' firstFailure=' + (failures[0] ?? 'none'));\n",
    "utf8"
  );
  await writeFile(
    spin,
    "const iterations = Number(process.argv[2] ?? 400000000);\n" +
      "const started = process.hrtime.bigint();\n" +
      "let sink = 0;\n" +
      "for (let index = 0; index < iterations; index += 1) sink += index % 7;\n" +
      "const wallMs = Number((process.hrtime.bigint() - started) / 1000000n);\n" +
      "console.log('iterations=' + iterations + ' wallMs=' + wallMs + ' sink=' + sink);\n",
    "utf8"
  );
  return { ok, allocate, spawnMany, spin };
}

async function run(runner: SandboxCommandRunner, check: EvaluationCheckSpec): Promise<EvaluationResult> {
  return evaluateChecks("exec-sandbox-limits", [check], runner, { idFactory: () => "eval-sandbox-limits" });
}

function sandboxFacts(result: EvaluationResult): string[] {
  const facts: string[] = [];
  for (const check of result.checks) {
    facts.push(
      `verdict=${check.verdict} exitCode=${String(check.exitCode)} limitHit=${check.limitHit ?? "none"} ` +
        `sandbox=${check.sandbox ? `established=${check.sandbox.established} memoryMb=${check.sandbox.memoryMb} processLimit=${check.sandbox.processLimit} cpuRatePercent=${check.sandbox.cpuRatePercent}` : "none"}`
    );
    facts.push(`output=${check.output.replace(/\s+/g, " ").slice(0, 400)}`);
  }
  return facts;
}

async function controlArm(runner: SandboxCommandRunner, script: string): Promise<Arm> {
  const result = await run(runner, {
    id: "control",
    name: "an ordinary check",
    command: process.execPath,
    args: [script]
  });
  const gate = completionGate(result);
  const failures = judgeControl(result, gate);
  const evidence = [
    "the check ran through the Evaluation Engine, which spawns it inside a job object",
    ...sandboxFacts(result),
    `gate=${gate.allowed ? "open" : "closed"} (${gate.reason})`
  ];
  return { id: "control", title: "an ordinary check runs inside the sandbox", failures, evidence };
}

async function memoryArm(runner: SandboxCommandRunner, script: string): Promise<Arm> {
  const check: EvaluationCheckSpec = {
    id: "memory",
    name: "a check that allocates half a gigabyte",
    command: process.execPath,
    args: [script, String(ALLOCATE_MB)]
  };
  const generous = await run(runner, { ...check, id: "memory-generous", limits: { memoryMb: 1024 } });
  const tight = await run(runner, { ...check, id: "memory-tight", limits: { memoryMb: 128 } });
  const gate = completionGate(tight);
  const failures = judgeMemory(generous, tight, gate);
  const evidence = [
    `the same check allocates ${ALLOCATE_MB} MB in both arms`,
    "under 1024 MB:",
    ...sandboxFacts(generous),
    "under 128 MB:",
    ...sandboxFacts(tight),
    `gate=${gate.allowed ? "open" : "closed"} (${gate.reason})`
  ];
  return { id: "memory", title: "the memory limit stops a check that allocates too much", failures, evidence };
}

async function processArm(runner: SandboxCommandRunner, script: string): Promise<Arm> {
  const check: EvaluationCheckSpec = {
    id: "processes",
    name: "a check that starts twenty children at once",
    command: process.execPath,
    args: [script, String(CHILDREN)]
  };
  const free = await run(runner, { ...check, id: "processes-free" });
  const tight = await run(runner, { ...check, id: "processes-tight", limits: { processLimit: 4 } });
  const failures = judgeProcesses(free, tight);
  const evidence = [
    `the same check starts ${CHILDREN} children at once in both arms`,
    "without a process limit:",
    ...sandboxFacts(free),
    "under a 4 process limit:",
    ...sandboxFacts(tight)
  ];
  return { id: "processes", title: "the process limit stops a check that starts too many processes", failures, evidence };
}

async function cpuArm(runner: SandboxCommandRunner, script: string): Promise<Arm> {
  const check: EvaluationCheckSpec = {
    id: "cpu",
    name: "a check that burns CPU",
    command: process.execPath,
    args: [script, String(SPIN_ITERATIONS)]
  };
  // The cap is a share of *all* processors, so on a many-core machine a single-threaded check only
  // feels it well below 100 percent. A quarter of one core is the smallest cap that bites
  // everywhere, and it is what the machine can express in whole percent.
  const cores = Math.max(1, cpus().length);
  const cap = Math.max(1, Math.floor(25 / cores));
  const free = await run(runner, { ...check, id: "cpu-free" });
  const capped = await run(runner, { ...check, id: "cpu-capped", limits: { cpuRatePercent: cap } });
  const failures = judgeCpu(free, capped);
  const freeWall = wallMs(free.checks[0]?.output ?? "");
  const cappedWall = wallMs(capped.checks[0]?.output ?? "");
  const evidence = [
    `the machine has ${cores} processors; the cap is ${cap}% of all of them, about a quarter of one core`,
    "without a CPU cap:",
    ...sandboxFacts(free),
    `the check's own wall clock=${String(freeWall ?? "unknown")}ms`,
    `under a ${cap}% CPU cap:`,
    ...sandboxFacts(capped),
    `the check's own wall clock=${String(cappedWall ?? "unknown")}ms`,
    ...(freeWall !== undefined && cappedWall !== undefined
      ? [`the throttled check took ${cappedWall}ms against ${freeWall}ms`]
      : []),
    `the engine measured ${free.checks[0]?.durationMs}ms and ${capped.checks[0]?.durationMs}ms, which include the same helper startup in both arms`
  ];
  return { id: "cpu", title: "the CPU limit throttles a check that burns CPU", failures, evidence };
}

// ------------------------------------------------------------------------------- the agent arms

/** The client the agent arms drive: the same HTTP surface the console drives. */
interface SandboxAgentClient {
  start(input: {
    taskId: string;
    workspace: string;
    prompt: string;
    checks: EvaluationCheckSpec[];
    timeoutSeconds: number;
  }): Promise<string>;
  logs(executionId: string): Promise<{ entries: ToolJournalEntry[] }>;
  execution(executionId: string): Promise<{
    status: string;
    completion: { allowed: boolean; reason: string };
  }>;
}

/**
 * The two agent arms.
 *
 * The workspace is a disposable git repository: the task service wants a worktree, and the check
 * that allocates half a gigabyte has to live inside it, because that is where the acceptance
 * check points.
 */
async function agentArms(scratch: string, allocateScript: string): Promise<Arm[]> {
  const model = process.env.PI_PROBE_MODEL ?? "MiniMax-M3";
  if (!process.env.MINIMAX_CN_API_KEY) {
    return [
      {
        id: "agent",
        title: "the agent is told which limit stopped its acceptance check",
        failures: [],
        blocked: ["MINIMAX_CN_API_KEY is not configured, so no agent could be asked to run the check"],
        evidence: [
          "the agent arm did not run: there is no API key, and a refusal that was never asked for proves nothing"
        ]
      }
    ];
  }

  const workspace = join(scratch, "workspace");
  await mkdir(workspace, { recursive: true });
  await writeFile(join(workspace, "allocate.mjs"), await readFile(allocateScript, "utf8"), "utf8");
  git(["init", "-b", "main"], workspace);
  git(["config", "user.name", "CodeWeaver Probe"], workspace);
  git(["config", "user.email", "probe@codeweaver.local"], workspace);
  git(["add", "-A"], workspace);
  git(["commit", "-m", "initial"], workspace);

  const dataDir = await mkdtemp(join(tmpdir(), "codeweaver-sandbox-agent-data-"));
  const runtime = createRuntime({
    host: "127.0.0.1",
    port: 0,
    dataDir,
    worktreeRoot: join(dataDir, "worktrees"),
    repository: workspace,
    hermesBaseUrl: "http://127.0.0.1:9",
    hermesTimeoutMs: 5_000,
    pi: { provider: "minimax-cn", model }
  });
  await runtime.ready();
  const server = await createApiServer(runtime.deps).listen(0);
  const client = httpClient(server.url);
  try {
    // The control first: the same check, the same prompt, with room to run.
    const control = await runAgentArm(client, {
      label: "agent-control",
      taskId: "task-p0-016-agent-control",
      workspace,
      memoryMb: 1024,
      judge: judgeAgentControl,
      title: "the same check passes for the agent when there is room"
    });
    const tight = await runAgentArm(client, {
      label: "agent-tight",
      taskId: "task-p0-016-agent-tight",
      workspace,
      memoryMb: 128,
      judge: judgeAgentTight,
      title: "the agent is told which limit stopped its acceptance check"
    });
    return [control, tight];
  } finally {
    await server.close();
    await runtime.close();
    await rm(dataDir, { recursive: true, force: true });
  }
}

async function runAgentArm(
  client: SandboxAgentClient,
  options: {
    label: string;
    taskId: string;
    workspace: string;
    memoryMb: number;
    judge: (facts: AgentSandboxFacts) => AgentJudgement;
    title: string;
  }
): Promise<Arm> {
  const check: EvaluationCheckSpec = {
    id: "allocate",
    name: "a check that allocates half a gigabyte",
    command: process.execPath,
    args: ["allocate.mjs", String(ALLOCATE_MB)],
    cwd: options.workspace,
    limits: { memoryMb: options.memoryMb }
  };
  const executionId = await client.start({
    taskId: options.taskId,
    workspace: options.workspace,
    prompt:
      "Run this task's acceptance check with the evaluate_changes tool, then report the verdict the " +
      "tool returned.",
    checks: [check],
    timeoutSeconds: 180
  });
  const status = await waitForTerminal(client, executionId, 180_000);
  const { entries } = await client.logs(executionId);
  const facts = collectToolFacts(entries);
  const toolText = facts.results
    .filter(result => result.tool === "evaluate_changes")
    .map(result => result.text)
    .join("\n");
  const execution = await client.execution(executionId);
  const observed: AgentSandboxFacts = {
    attempted: facts.calls.some(call => call.tool === "evaluate_changes"),
    toolText,
    answer: facts.answer,
    gate: execution.completion,
    status
  };
  const judgement = options.judge(observed);
  const evidence = [
    `the task declared one acceptance check: node allocate.mjs ${ALLOCATE_MB}, under a ${options.memoryMb} MB limit`,
    `${options.label}/executionId=${executionId}`,
    `${options.label}/status=${status} completion=${JSON.stringify(execution.completion)}`,
    `${options.label}/evaluate_changesCalls=${facts.calls.filter(call => call.tool === "evaluate_changes").length}`,
    `${options.label}/toolText=${toolText.slice(0, 500).replace(/\n/g, " | ")}`,
    `${options.label}/answer=${observed.answer.slice(0, 400).replace(/\n/g, " | ")}`
  ];
  return {
    id: options.label,
    title: options.title,
    failures: judgement.failures,
    blocked: judgement.blocked,
    evidence
  };
}

async function waitForTerminal(client: SandboxAgentClient, executionId: string, timeoutMs: number): Promise<string> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const { entries } = await client.logs(executionId);
    const terminal = entries.find(
      entry =>
        entry.category === "execution.completed" ||
        entry.category === "execution.failed" ||
        entry.category === "execution.cancelled"
    );
    if (terminal) return String((terminal.payload ?? {})["status"] ?? terminal.category);
    await sleep(500);
  }
  return "timeout";
}

function sleep(ms: number): Promise<void> {
  return new Promise(resolve => setTimeout(resolve, ms));
}

function git(args: string[], cwd: string): void {
  const result = spawnSync("git", args, { cwd, encoding: "utf8" });
  if (result.status !== 0) {
    throw new Error(`git ${args.join(" ")} failed in ${cwd}: ${result.stderr}`);
  }
}

/** The real client: everything goes over HTTP, the way the console drives it. */
function httpClient(baseUrl: string): SandboxAgentClient {
  return {
    start: async input => {
      const response = await fetch(`${baseUrl}/api/v1/tasks/${input.taskId}/executions`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          projectId: "probe-sandbox-limits",
          prompt: input.prompt,
          worktreePath: input.workspace,
          documents: [],
          selectedFiles: [],
          architectureVersion: "1.0",
          policy: {
            snapshotId: "snap-sandbox-limits",
            projectId: "probe-sandbox-limits",
            taskId: input.taskId,
            executionId: "",
            worktreeId: "wt-sandbox-limits",
            policyVersion: "1.0.0",
            grantedCapabilities: ["*"],
            allowedPaths: [input.workspace],
            expiresAt: "2099-01-01T00:00:00.000Z"
          },
          template: { name: "coding-agent", version: "1.4.2", contentHash: "e".repeat(64) },
          modelConfig: { model: process.env.PI_PROBE_MODEL ?? "MiniMax-M3" },
          correlationId: input.taskId,
          timeoutSeconds: input.timeoutSeconds ?? 180,
          allowedTools: ["evaluate_changes"],
          evaluationChecks: input.checks,
          runtimeId: "pi-local"
        })
      });
      if (response.status !== 201) {
        throw new Error(`the task was refused: ${response.status} ${await response.text()}`);
      }
      const created = (await response.json()) as { execution: { executionId: string } };
      return created.execution.executionId;
    },
    logs: async executionId => {
      const response = await fetch(`${baseUrl}/api/v1/executions/${encodeURIComponent(executionId)}/logs`);
      if (response.status !== 200) {
        throw new Error(`the journal was refused: ${response.status} ${await response.text()}`);
      }
      return (await response.json()) as { count: number; entries: ToolJournalEntry[] };
    },
    execution: async executionId => {
      const response = await fetch(`${baseUrl}/api/v1/executions/${encodeURIComponent(executionId)}`);
      if (response.status !== 200) {
        throw new Error(`the execution was refused: ${response.status} ${await response.text()}`);
      }
      const body = (await response.json()) as {
        status?: string;
        completion?: { allowed?: boolean; reason?: string };
      };
      return {
        status: String(body.status ?? "unknown"),
        completion: {
          allowed: body.completion?.allowed === true,
          reason: String(body.completion?.reason ?? "")
        }
      };
    }
  };
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : undefined;
if (invokedPath === import.meta.url) {
  main().catch(error => {
    console.error(error);
    process.exitCode = 1;
  });
}
