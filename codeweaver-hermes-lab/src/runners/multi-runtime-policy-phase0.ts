import { spawnSync } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import type { AgentRuntimeRegistry } from "../adapters/runtime-registry.js";
import { RuntimeScheduler, type SchedulingStrategy } from "../adapters/runtime-scheduler.js";
import { collectRuntimeScores } from "../adapters/runtime-score-report.js";
import { createApiServer } from "../api/app.js";
import { writeTestReport } from "../reporting/report-writer.js";
import { createRuntime } from "../server/runtime.js";
import type { TestResult } from "../types/hermes.js";

/**
 * Multi-runtime execution policy, proven against two real runtimes.
 *
 * The scheduler existed and the console had a runtime picker, but the picker is "pick one runtime by
 * hand". What was missing was the evidence for the two claims the product makes:
 *
 *  - several runtimes coexist and the scheduler really distributes work between them, in the three
 *    strategies the scheduler implements, with the capability filter applied before the balancing
 *    (D-025);
 *  - a named runtime is honoured and an unknown one is refused rather than redirected (D-047).
 *
 * Both runtimes in this probe are real: the pinned Hermes 0.23.0 gateway over HTTP, and Pi in this
 * process against a real MiniMax M3 model. Nothing here is mocked, and a runtime that refuses to
 * run is reported as a failure rather than quietly replaced.
 */

/** The word the model is asked for. Whether it appears is recorded, not required. */
const EXPECTED_TOKEN = "CODEWEAVER_MULTI_RUNTIME_OK";

const PROMPT = `Reply with exactly the word: ${EXPECTED_TOKEN}. Do not use any tools.`;

const TERMINAL_STATUSES = new Set(["succeeded", "failed", "cancelled", "interrupted"]);

const STRATEGIES: SchedulingStrategy[] = ["round_robin", "least_loaded", "first_healthy"];

export interface MultiRuntimeProbeOptions {
  prompt?: string;
  /** How long a real run may take before the probe gives up on it. */
  runTimeoutMs?: number;
  now?: () => Date;
}

interface ExecutionOutcome {
  executionId: string;
  /** The runtime the server says ran it. */
  runtimeId?: string;
  status: string;
  output: string;
  outputHasToken: boolean;
  durationMs: number;
}

/**
 * Runs the probe against an already-running API server.
 *
 * The probe is written against the HTTP surface rather than the internals on purpose: the claims
 * are about what a user of the product gets, and the HTTP surface is what the console uses.
 */
export async function executeMultiRuntimePolicyProbe(
  baseUrl: string,
  registry: AgentRuntimeRegistry,
  options: MultiRuntimeProbeOptions = {}
): Promise<TestResult[]> {
  const now = options.now ?? (() => new Date());
  const prompt = options.prompt ?? PROMPT;
  const runTimeoutMs = options.runTimeoutMs ?? 180_000;
  const results: TestResult[] = [];

  // --- 1. The three strategies, on the live registry -----------------------------------------
  {
    const startedAt = now().toISOString();
    const evidence: string[] = [];
    const failures: string[] = [];
    try {
      for (const strategy of STRATEGIES) {
        const scheduler = new RuntimeScheduler(registry, { strategy });
        const picks: string[] = [];
        for (let index = 0; index < 3; index += 1) {
          const selection = await scheduler.select({ requiredCapabilities: { streaming: true } });
          picks.push(selection.runtimeId);
        }
        evidence.push(`${strategy}=${picks.join(">")}`);
        // Round robin is the only strategy that is *supposed* to alternate; the others may keep
        // picking the first candidate when the load is equal.
        if (strategy === "round_robin" && new Set(picks).size < 2) {
          failures.push(`round_robin never left ${picks[0]}`);
        }
        // The load-aware strategy has to notice a busy runtime.
        if (strategy === "least_loaded") {
          scheduler.markBusy(picks[0]!);
          const afterBusy = await scheduler.select({ requiredCapabilities: { streaming: true } });
          evidence.push(`least_loaded(after marking ${picks[0]!} busy)=${afterBusy.runtimeId}`);
          if (afterBusy.runtimeId === picks[0]) {
            failures.push(`least_loaded kept picking the busy runtime ${picks[0]}`);
          }
        }
      }
    } catch (error) {
      failures.push(error instanceof Error ? error.message : String(error));
    }
    results.push({
      id: "MULTI-RUNTIME-SCHEDULER",
      title: "多运行时并存：三种调度策略各自选中真实运行时",
      status: failures.length === 0 ? "passed" : "failed",
      startedAt,
      finishedAt: now().toISOString(),
      evidence,
      ...(failures.length > 0 ? { notes: failures.join("; ") } : {})
    });
  }

  // --- 2. The capability filter runs before the balancing (D-025) ------------------------------
  {
    const startedAt = now().toISOString();
    const evidence: string[] = [];
    const failures: string[] = [];
    try {
      const scheduler = new RuntimeScheduler(registry, { strategy: "least_loaded" });
      const streaming = await scheduler.select({ requiredCapabilities: { streaming: true } });
      evidence.push(`required(streaming)=${streaming.runtimeId}`);
      const toolEvents = await scheduler.select({ requiredCapabilities: { toolEvents: true } });
      evidence.push(`required(toolEvents)=${toolEvents.runtimeId}`);
      const hermes = await registry.capabilities("hermes-local");
      const pi = await registry.capabilities("pi-local");
      evidence.push(
        `hermes-local declares toolEvents=${String(hermes.toolEvents)}, pi-local declares toolEvents=${String(pi.toolEvents)}`
      );
      if (hermes.toolEvents) {
        failures.push("hermes-local declares toolEvents, so the filter proves nothing");
      }
      if (toolEvents.runtimeId !== "pi-local") {
        failures.push(`the toolEvents filter selected ${toolEvents.runtimeId} instead of pi-local`);
      }
    } catch (error) {
      failures.push(error instanceof Error ? error.message : String(error));
    }
    results.push({
      id: "MULTI-RUNTIME-CAPABILITY-FILTER",
      title: "能力过滤先于均衡（D-025）",
      status: failures.length === 0 ? "passed" : "failed",
      startedAt,
      finishedAt: now().toISOString(),
      evidence,
      ...(failures.length > 0 ? { notes: failures.join("; ") } : {})
    });
  }

  // --- 3. A named runtime is honoured (D-047) ----------------------------------------------------
  const piRun = await runTask(baseUrl, "pi-local", prompt, runTimeoutMs);
  {
    const evidence = [
      `executionId=${piRun.executionId}`,
      `runtimeId=${piRun.runtimeId ?? "unknown"}`,
      `status=${piRun.status}`,
      `durationMs=${piRun.durationMs}`,
      `outputChars=${piRun.output.length}`,
      `outputHasToken=${String(piRun.outputHasToken)}`
    ];
    const failures: string[] = [];
    if (piRun.runtimeId !== "pi-local") {
      failures.push(`naming pi-local ran on ${piRun.runtimeId ?? "an unknown runtime"}`);
    }
    if (piRun.status !== "succeeded") {
      failures.push(`the Pi run ended as '${piRun.status}'`);
    }
    results.push({
      id: "MULTI-RUNTIME-D047-HONOURED",
      title: "点名 pi-local 就跑在 pi-local（D-047）",
      status: failures.length === 0 ? "passed" : "failed",
      startedAt: new Date(Date.now() - piRun.durationMs).toISOString(),
      finishedAt: now().toISOString(),
      evidence,
      ...(failures.length > 0 ? { notes: failures.join("; ") } : {})
    });
  }

  // --- 4. An unknown runtime is refused, not redirected -----------------------------------------
  {
    const startedAt = now().toISOString();
    const evidence: string[] = [];
    const failures: string[] = [];
    try {
      const response = await postExecution(baseUrl, "no-such-runtime", prompt);
      const body = await response.text();
      evidence.push(`status=${response.status}`);
      evidence.push(`body=${body.slice(0, 300)}`);
      if (response.ok) {
        failures.push("the server accepted a task naming a runtime that does not exist");
      }
      if (!body.includes("no-such-runtime")) {
        failures.push("the refusal did not name the runtime that was asked for");
      }
    } catch (error) {
      failures.push(error instanceof Error ? error.message : String(error));
    }
    results.push({
      id: "MULTI-RUNTIME-D047-REFUSED",
      title: "点名不存在的运行时被明确拒绝而不是回退（D-047）",
      status: failures.length === 0 ? "passed" : "failed",
      startedAt,
      finishedAt: now().toISOString(),
      evidence,
      ...(failures.length > 0 ? { notes: failures.join("; ") } : {})
    });
  }

  // --- 5. The remote runtime really runs a task -------------------------------------------------
  const hermesRun = await runTask(baseUrl, "hermes-local", prompt, runTimeoutMs);
  {
    const evidence = [
      `executionId=${hermesRun.executionId}`,
      `runtimeId=${hermesRun.runtimeId ?? "unknown"}`,
      `status=${hermesRun.status}`,
      `durationMs=${hermesRun.durationMs}`,
      `outputChars=${hermesRun.output.length}`,
      `outputHasToken=${String(hermesRun.outputHasToken)}`
    ];
    const failures: string[] = [];
    if (hermesRun.runtimeId !== "hermes-local") {
      failures.push(`naming hermes-local ran on ${hermesRun.runtimeId ?? "an unknown runtime"}`);
    }
    if (hermesRun.status !== "succeeded") {
      failures.push(`the Hermes run ended as '${hermesRun.status}'`);
    }
    results.push({
      id: "MULTI-RUNTIME-HERMES-EXECUTION",
      title: "远程 Hermes 运行时真实跑完一次任务",
      status: failures.length === 0 ? "passed" : "failed",
      startedAt: new Date(Date.now() - hermesRun.durationMs).toISOString(),
      finishedAt: now().toISOString(),
      evidence,
      ...(failures.length > 0 ? { notes: failures.join("; ") } : {})
    });
  }

  return results;
}

/**
 * Starts a task on a named runtime and waits for it to finish.
 *
 * The runtime id in the answer is the server's own report, taken from the execution detail rather
 * than from the request: a request that names a runtime proves nothing unless the server says the
 * run happened there.
 */
async function runTask(
  baseUrl: string,
  runtimeId: string,
  prompt: string,
  timeoutMs: number
): Promise<ExecutionOutcome> {
  const startedAt = Date.now();
  const response = await postExecution(baseUrl, runtimeId, prompt);
  if (response.status !== 201) {
    const body = await response.text();
    return {
      executionId: "none",
      status: `refused(${response.status})`,
      output: body,
      outputHasToken: body.includes(EXPECTED_TOKEN),
      durationMs: Date.now() - startedAt
    };
  }
  const created = (await response.json()) as { execution: { executionId: string } };
  const executionId = created.execution.executionId;

  const deadline = Date.now() + timeoutMs;
  let status = "unknown";
  let reportedRuntime: string | undefined;
  while (Date.now() < deadline) {
    const detail = await fetch(`${baseUrl}/api/v1/executions/${encodeURIComponent(executionId)}`);
    const body = (await detail.json()) as { status?: string; runtimeId?: string };
    status = String(body.status ?? "unknown");
    if (body.runtimeId) reportedRuntime = body.runtimeId;
    if (TERMINAL_STATUSES.has(status)) break;
    await sleep(300);
  }

  const output = await readOutput(baseUrl, executionId);
  return {
    executionId,
    ...(reportedRuntime ? { runtimeId: reportedRuntime } : {}),
    status,
    output,
    outputHasToken: output.includes(EXPECTED_TOKEN),
    durationMs: Date.now() - startedAt
  };
}

/** The assistant text of an execution, taken from its event stream. */
async function readOutput(baseUrl: string, executionId: string): Promise<string> {
  const response = await fetch(`${baseUrl}/api/v1/executions/${encodeURIComponent(executionId)}/events`);
  const body = (await response.json()) as {
    events?: Array<{ type?: string; data?: Record<string, unknown> }>;
  };
  return (body.events ?? [])
    .filter(event => event.type === "agent.message.delta")
    .map(event => String(event.data?.["delta"] ?? ""))
    .join("");
}

async function postExecution(
  baseUrl: string,
  runtimeId: string,
  prompt: string
): Promise<Response> {
  return fetch(`${baseUrl}/api/v1/tasks/task-multi-runtime/executions`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      projectId: "probe-multi-runtime",
      prompt,
      worktreePath: process.env.MULTI_RUNTIME_WORKTREE ?? process.cwd(),
      documents: ["README.md"],
      selectedFiles: ["README.md"],
      architectureVersion: "1.0",
      policy: {
        snapshotId: "snap-multi-runtime",
        projectId: "probe-multi-runtime",
        taskId: "task-multi-runtime",
        executionId: "",
        worktreeId: "wt-multi-runtime",
        policyVersion: "1.0.0",
        grantedCapabilities: ["*"],
        allowedPaths: [process.env.MULTI_RUNTIME_WORKTREE ?? process.cwd()],
        expiresAt: "2099-01-01T00:00:00.000Z"
      },
      template: { name: "coding-agent", version: "1.4.2", contentHash: "e".repeat(64) },
      modelConfig: { model: process.env.PI_PROBE_MODEL ?? "MiniMax-M3" },
      correlationId: "probe-multi-runtime",
      timeoutSeconds: 300,
      allowedTools: [],
      runtimeId
    })
  });
}


function sleep(ms: number): Promise<void> {
  return new Promise(resolveSleep => setTimeout(resolveSleep, ms));
}

/**
 * The credentials, read from the pinned runtime's own profile.
 *
 * The Hermes profile's `.env` is where the pinned runtime keeps its keys, and it is the only place
 * this probe looks: nothing is written to the repository and nothing is passed on a command line.
 * Values are never printed — the probe reports which variables it found, not what they are.
 */
async function loadProfileEnv(): Promise<Record<string, string>> {
  const labDir = resolve(fileURLToPath(new URL("..", import.meta.url)), "..");
  const lock = JSON.parse(await readFile(join(labDir, "hermes.lock.json"), "utf8")) as { version?: string };
  const version = lock.version?.trim();
  if (!version) return {};
  const profile = join(process.env["LOCALAPPDATA"] ?? "", "hermes", `hermes-agent-${version}`, "profile", ".env");
  let content: string;
  try {
    content = await readFile(profile, "utf8");
  } catch {
    return {};
  }
  const values: Record<string, string> = {};
  for (const line of content.split(/\r?\n/)) {
    const trimmed = line.trim();
    if (trimmed === "" || trimmed.startsWith("#") || !trimmed.includes("=")) continue;
    const [name, value] = trimmed.split("=", 2);
    if (name && value !== undefined) values[name.trim()] = value.trim();
  }
  return values;
}

function git(args: string[], cwd: string): void {
  const result = spawnSync("git", args, { cwd, encoding: "utf8" });
  if (result.status !== 0) {
    throw new Error(`git ${args.join(" ")} failed: ${result.stderr}`);
  }
}

async function main(): Promise<void> {
  const reportPath = resolve(
    process.env.MULTI_RUNTIME_POLICY_REPORT_PATH ?? "reports/phase0-multi-runtime-policy.json"
  );
  const model = process.env.PI_PROBE_MODEL ?? "MiniMax-M3";

  const profile = await loadProfileEnv();
  const minmaxKey = process.env.MINIMAX_CN_API_KEY ?? profile["MINIMAX_CN_API_KEY"];
  const hermesKey = process.env.HERMES_API_KEY ?? profile["API_SERVER_KEY"];
  if (!minmaxKey) {
    throw new Error("MINIMAX_CN_API_KEY is not set and the Hermes profile does not carry one; nothing to probe.");
  }
  // Pi's provider reads the key from the environment; the probe sets it in-process and never logs it.
  process.env.MINIMAX_CN_API_KEY = minmaxKey;
  const hermesBaseUrl = process.env.HERMES_HTTP_BASE_URL ?? "http://127.0.0.1:8642";

  const repo = await mkdtemp(join(tmpdir(), "codeweaver-multi-runtime-repo-"));
  const dataDir = await mkdtemp(join(tmpdir(), "codeweaver-multi-runtime-data-"));
  git(["init", "-b", "main"], repo);
  git(["config", "user.name", "CodeWeaver Probe"], repo);
  git(["config", "user.email", "probe@codeweaver.local"], repo);
  await writeFile(join(repo, "README.md"), "# Multi-runtime policy probe\n", "utf8");
  git(["add", "-A"], repo);
  git(["commit", "-m", "initial"], repo);
  process.env["MULTI_RUNTIME_WORKTREE"] = repo;

  const runtime = createRuntime({
    host: "127.0.0.1",
    port: 0,
    dataDir,
    worktreeRoot: join(dataDir, "worktrees"),
    repository: repo,
    hermesBaseUrl,
    ...(hermesKey ? { hermesApiKey: hermesKey } : {}),
    hermesTimeoutMs: 30_000,
    pi: { provider: "minimax-cn", model }
  });
  await runtime.ready();
  const server = await createApiServer(runtime.deps).listen(0);

  const health = await runtime.deps.registry.healthCheckAll();
  console.log(`runtime health: ${health.runtimes.map(item => `${item.id}=${item.healthy ? "healthy" : "unhealthy"}`).join(", ")}`);
  console.log(`hermes gateway: ${hermesBaseUrl}`);

  const results = await executeMultiRuntimePolicyProbe(server.url, runtime.deps.registry);
  const scoreReport = await collectRuntimeScores({
    registry: runtime.deps.registry,
    journal: runtime.deps.journal
  });

  const scoreResult: TestResult = {
    id: "MULTI-RUNTIME-SCORE",
    title: "两个运行时按实测记录得到可比较的能力评分",
    status: scoreReport.runtimes.length >= 2 ? "passed" : "failed",
    startedAt: new Date().toISOString(),
    finishedAt: new Date().toISOString(),
    evidence: [
      `executionsInJournal=${scoreReport.executionsInJournal}`,
      ...scoreReport.comparison.lines,
      ...scoreReport.runtimes.flatMap(item => [
        `${item.runtimeId}: score=${item.score === null ? "n/a" : item.score} unmeasured=${item.unmeasuredDimensions.join(",") || "none"}`,
        `${item.runtimeId}: declared=${item.metrics.declaredCapabilities.join(",") || "none"} observed=${item.metrics.observedCapabilities.join(",") || "none"}`,
        `${item.runtimeId}: declaredNotObserved=${item.metrics.declaredNotObserved.join(",") || "none"} observedNotDeclared=${item.metrics.observedNotDeclared.join(",") || "none"}`
      ])
    ],
    ...(scoreReport.runtimes.length < 2 ? { notes: "fewer than two runtimes were scored" } : {})
  };

  const all = [...results, scoreResult];
  await writeTestReport(reportPath, all);
  console.log(`\nMulti-runtime policy report: ${reportPath}`);
  for (const item of all) {
    console.log(`${item.id} ${item.status}: ${item.title}`);
    for (const line of item.evidence) console.log(`  ${line}`);
    if (item.notes) console.log(`  notes: ${item.notes}`);
  }
  if (all.some(item => item.status === "failed")) process.exitCode = 1;

  await server.close();
  await runtime.close();
  await rm(repo, { recursive: true, force: true });
  await rm(dataDir, { recursive: true, force: true });
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : undefined;
if (invokedPath === import.meta.url) {
  await main();
}
