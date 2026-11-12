import { spawnSync } from "node:child_process";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { createApiServer } from "../api/app.js";
import { writeTestReport } from "../reporting/report-writer.js";
import { createRuntime } from "../server/runtime.js";
import type { TestResult } from "../types/hermes.js";

/**
 * P0-014: two agents at the same time, each running on its own.
 *
 * The checklist asks for "两个 Agent 同时执行" with the acceptance criterion "各自独立运行". Those
 * are two claims, and the second one is the one that is easy to fake: a system that runs the two
 * tasks one after the other has run two agents, but it has never run two agents *at the same time*,
 * and it has never had to keep them apart. A run that looks parallel but is serialized proves
 * nothing about interference.
 *
 * So the probe runs two forms of concurrency, both on the real model:
 *
 *  - two top-level tasks, each in its own workspace, started while the first is still running;
 *  - one parent execution that delegates to two children at the same time, each child in its own
 *    isolated worktree.
 *
 * The yardstick for "independent" is a nonce. Each workspace holds a file with a string that exists
 * nowhere else — not in the prompt, not in the other workspace, not in the model's head. Each agent
 * has to read its own file and answer with its own string. If two runs shared a session, a context
 * or a transcript, the answers would cross, and a crossed answer is not a cosmetic failure: it means
 * one agent's work was attributed to the other.
 *
 * The links, judged per arm:
 *
 *  - two-runs: two distinct executions, both finished on their own.
 *  - overlapped: the second run started before the first finished, and the probe saw the first one
 *    still running at that moment. Two runs that never coexisted do not satisfy the criterion.
 *  - own-journal: each journal holds only its own execution's events, and each run has exactly one
 *    terminal event of its own.
 *  - own-answer: each answer carries its own nonce and not the other's.
 *  - separate-usage: the ledger keeps one record per run, so the two runs are also two rows of cost.
 *
 * The two arms answer different questions and both are needed. The first asks whether the *server*
 * can hold two runs at once; the second asks whether the *product* can put two agents to work at
 * once. A system that only does the first is a server with a concurrency bug; one that only does the
 * second is a demo.
 */

export interface ParallelJournalEntry {
  sequence: number;
  kind: string;
  category: string;
  recordedAt: string;
  /** Which execution the entry belongs to. The journal endpoint reports it for every entry. */
  executionId?: string;
  payload?: Record<string, unknown>;
}

export interface ParallelUsageRecord {
  id: string;
  executionId: string;
  model: string;
  promptTokens: number;
  completionTokens: number;
  totalTokens: number;
  durationMs: number;
}

/** The slice of the system this probe drives; the real one talks to a server over the wire. */
export interface ParallelAgentsClient {
  /** Starts a top-level task in its own workspace and returns the execution id. */
  start(input: {
    taskId: string;
    workspace: string;
    prompt: string;
    /** The file the task is about, so the context snapshot names it. */
    file: string;
    timeoutSeconds?: number;
  }): Promise<string>;
  /** The execution's own state: the status, the recorded terminal event and the event count. */
  execution(executionId: string): Promise<{
    status: string;
    terminalEventType?: string;
    eventCount: number;
  }>;
  /** The raw journal of one execution — the yardstick the run is measured against. */
  logs(executionId: string): Promise<{ count: number; entries: ParallelJournalEntry[] }>;
  /** The shared audit timeline: the surface where two runs could get crossed. */
  timeline(): Promise<{ items: Array<{ executionId: string; sequence: number }> }>;
  /** The usage record of one execution. */
  usage(executionId: string): Promise<{ status: number; record?: ParallelUsageRecord }>;
  /** Delegates a child execution from a parent execution. */
  spawnChild(input: {
    parentExecutionId: string;
    prompt: string;
    policySnapshotId: string;
    contextSnapshotId: string;
    runId: string;
    label: string;
    baseSha: string;
  }): Promise<{
    status: number;
    childExecutionId?: string;
    workingDirectory?: string;
    error?: string;
  }>;
}

export interface ParallelArmFacts {
  executionId: string;
  status: string;
  startedAt?: string;
  finishedAt?: string;
  durationMs?: number;
  /** How many terminal events this run recorded. Exactly one is a run; two is an ambiguous one. */
  terminalCount: number;
  terminalCategory?: string;
  /** The assistant's own text, as it was streamed. */
  answer: string;
  usage?: ParallelUsageRecord;
  /** How many entries this run's own journal holds. */
  journalCount: number;
  /** How many entries the shared timeline holds for this execution. */
  timelineCount?: number;
}

export interface ParallelLink {
  name: string;
  ok: boolean;
  detail: string;
}

export interface ParallelJudgement {
  links: ParallelLink[];
  ok: boolean;
}

export interface ParallelOptions {
  now?: () => Date;
  /** How long one run may take. */
  runTimeoutMs?: number;
  /** The workspace of the first top-level run, and the second one's. */
  workspaces?: { first: string; second: string };
  /** The workspace the parent and its children work in. */
  parentWorkspace?: string;
  /** The commit the children's isolated worktrees are created at. */
  baseSha?: string;
  /** The nonce each run has to find. */
  nonces?: { first: string; second: string; childFirst: string; childSecond: string };
}

const TERMINAL_CATEGORIES = new Set(["execution.completed", "execution.failed", "execution.cancelled"]);

/** The prompt both agents get: read the file, then answer with what is in it. */
export function readAndAnswerPrompt(fileName: string): string {
  return [
    "Work in the repository you are given. Do exactly this, using the tools:",
    `1. Call read_file with path '${fileName}' and look at what it contains.`,
    "2. Reply with exactly the contents of that file, and nothing else."
  ].join(" ");
}

/**
 * Judges one arm: two runs that had to happen at the same time and had to stay apart.
 */
export function judgeParallelArm(input: {
  label: string;
  first: ParallelArmFacts;
  second: ParallelArmFacts;
  firstNonce: string;
  secondNonce: string;
  /** Whether the probe saw the first run still in flight when the second one started. */
  observedOverlap: boolean;
}): ParallelJudgement {
  const links: ParallelLink[] = [];
  const first = input.first;
  const second = input.second;

  // Two runs, two executions, two outcomes of their own.
  const twoRuns =
    first.executionId !== second.executionId &&
    first.executionId !== "" &&
    second.executionId !== "" &&
    first.status === "succeeded" &&
    second.status === "succeeded";
  links.push({
    name: "two-runs",
    ok: twoRuns,
    detail: twoRuns
      ? `two distinct executions (${first.executionId}, ${second.executionId}), both succeeded`
      : `the runs are ${first.executionId || "none"} ('${first.status}') and ${second.executionId || "none"} ('${second.status}')`
  });

  // They had to coexist. The timestamps say the spans overlap; the observation says the first one was
  // still running when the second started — two runs that never coexisted are not a parallel run.
  const spansOverlap =
    first.startedAt !== undefined &&
    first.finishedAt !== undefined &&
    second.startedAt !== undefined &&
    second.startedAt < first.finishedAt;
  const overlapped = spansOverlap && input.observedOverlap;
  links.push({
    name: "overlapped",
    ok: overlapped,
    detail: overlapped
      ? `the second run started at ${second.startedAt}, before the first one finished at ${first.finishedAt}, and the first one was still running when it started`
      : `the spans do not overlap (first ${first.startedAt ?? "?"} → ${first.finishedAt ?? "?"}, second starts ${second.startedAt ?? "?"}), observed in flight at the second start: ${String(input.observedOverlap)}`
  });

  // Each journal holds its own run and nothing else, and each run ends exactly once. The shared
  // timeline is where two runs could get crossed, so the count it holds for each execution has to be
  // the count that execution's own journal holds — no event lost, none invented, none attributed to
  // the other run.
  const ownJournal =
    first.terminalCount === 1 &&
    second.terminalCount === 1 &&
    first.terminalCategory === "execution.completed" &&
    second.terminalCategory === "execution.completed" &&
    first.timelineCount === first.journalCount &&
    second.timelineCount === second.journalCount;
  links.push({
    name: "own-journal",
    ok: ownJournal,
    detail: ownJournal
      ? `each journal holds only its own run (${first.journalCount} and ${second.journalCount} entries, the same on the shared timeline), and each run recorded exactly one 'execution.completed'`
      : `the terminal events are ${first.terminalCount}×'${first.terminalCategory ?? "none"}' / ${second.terminalCount}×'${second.terminalCategory ?? "none"}', and the timeline holds ${first.timelineCount ?? "?"} / ${second.timelineCount ?? "?"} entries against journals of ${first.journalCount} / ${second.journalCount}`
  });

  // Each answer carries its own nonce, and not the other's. This is the part that cannot be faked by
  // a system that serializes the two runs: a crossed answer means the contexts crossed.
  const firstOwn = first.answer.includes(input.firstNonce) && !first.answer.includes(input.secondNonce);
  const secondOwn = second.answer.includes(input.secondNonce) && !second.answer.includes(input.firstNonce);
  const ownAnswer = firstOwn && secondOwn;
  links.push({
    name: "own-answer",
    ok: ownAnswer,
    detail: ownAnswer
      ? `each run answered with its own nonce (${input.firstNonce} / ${input.secondNonce})`
      : `the answers do not match their own runs: the first answer holds ${
          first.answer.includes(input.firstNonce) ? "its own" : "not its own"
        } nonce and ${first.answer.includes(input.secondNonce) ? "the other's" : "not the other's"}, the second answer holds ${
          second.answer.includes(input.secondNonce) ? "its own" : "not its own"
        } nonce and ${second.answer.includes(input.firstNonce) ? "the other's" : "not the other's"}`
  });

  // The ledger keeps the two runs apart too: two executions, two rows of cost.
  const separateUsage =
    first.usage !== undefined &&
    second.usage !== undefined &&
    first.usage.executionId === first.executionId &&
    second.usage.executionId === second.executionId &&
    first.usage.id !== second.usage.id;
  links.push({
    name: "separate-usage",
    ok: separateUsage,
    detail: separateUsage
      ? `the ledger holds one record per run (${first.usage!.totalTokens} and ${second.usage!.totalTokens} tokens)`
      : `the ledger holds ${first.usage ? "one" : "no"} record for the first run and ${second.usage ? "one" : "no"} for the second`
  });

  return { links, ok: links.every(link => link.ok) };
}

/**
 * P0-014: two arms of real concurrency.
 */
export async function executeParallelAgents(
  client: ParallelAgentsClient,
  options: ParallelOptions = {}
): Promise<TestResult> {
  const now = options.now ?? (() => new Date());
  const startedAt = now().toISOString();
  const evidence: string[] = [];
  const failures: string[] = [];
  const links: ParallelLink[] = [];
  const runTimeoutMs = options.runTimeoutMs ?? 300_000;

  const workspaces = options.workspaces ?? { first: "", second: "" };
  const nonces = options.nonces ?? {
    first: "NONCE-FIRST",
    second: "NONCE-SECOND",
    childFirst: "NONCE-CHILD-FIRST",
    childSecond: "NONCE-CHILD-SECOND"
  };

  // ---------------------------------------------------------------- the two top-level runs
  {
    const firstId = await client.start({
      taskId: "task-p0-014-top-first",
      workspace: workspaces.first,
      prompt: readAndAnswerPrompt("secret.txt"),
      file: "secret.txt",
      timeoutSeconds: Math.ceil(runTimeoutMs / 1000)
    });
    evidence.push(`top/firstExecutionId=${firstId}`);
    await waitForStarted(client, firstId, runTimeoutMs);

    // The second run starts while the first one is still running. Whether that is true is not assumed:
    // the first journal is read immediately after the second run is created.
    const secondId = await client.start({
      taskId: "task-p0-014-top-second",
      workspace: workspaces.second,
      prompt: readAndAnswerPrompt("secret.txt"),
      file: "secret.txt",
      timeoutSeconds: Math.ceil(runTimeoutMs / 1000)
    });
    evidence.push(`top/secondExecutionId=${secondId}`);
    const observedOverlap = !(await hasTerminal(client, firstId));
    evidence.push(`top/firstStillRunningWhenSecondStarted=${observedOverlap}`);
    await waitForStarted(client, secondId, runTimeoutMs);

    await waitForTerminal(client, firstId, runTimeoutMs);
    await waitForTerminal(client, secondId, runTimeoutMs);
    const counts = await timelineCounts(client);
    const first = await collectFacts(client, firstId, counts);
    const second = await collectFacts(client, secondId, counts);
    evidence.push(`top/first=${describe(first)}`);
    evidence.push(`top/second=${describe(second)}`);

    const judgement = judgeParallelArm({
      label: "top",
      first,
      second,
      firstNonce: nonces.first,
      secondNonce: nonces.second,
      observedOverlap
    });
    for (const link of judgement.links) links.push({ ...link, name: `top/${link.name}` });
  }

  // ------------------------------------------------------------- the two delegated children
  if (options.parentWorkspace !== undefined && options.baseSha !== undefined) {
    const parentId = await client.start({
      taskId: "task-p0-014-parent",
      workspace: options.parentWorkspace,
      prompt: readAndAnswerPrompt("alpha.txt"),
      file: "alpha.txt",
      timeoutSeconds: Math.ceil(runTimeoutMs / 1000)
    });
    evidence.push(`sub/parentExecutionId=${parentId}`);
    const parentStatus = await waitForTerminal(client, parentId, runTimeoutMs);
    evidence.push(`sub/parentStatus=${parentStatus}`);
    if (parentStatus !== "succeeded") {
      failures.push(`the parent run ended as '${parentStatus}'`);
    }

    // The children run under the policy the parent ran under: a snapshot is looked up by id, and the
    // children's own worktrees are what the tool surface binds to.
    const parentEntries = (await client.logs(parentId)).entries;
    const policySnapshotId = parentEntries.find(entry => entry.category === "policy.snapshot")?.payload?.[
      "policySnapshotId"
    ];
    const contextSnapshotId = parentEntries.find(entry => entry.category === "context.snapshot")?.payload?.[
      "contextSnapshotId"
    ];
    const childContextSnapshotId =
      typeof contextSnapshotId === "string" && contextSnapshotId !== ""
        ? contextSnapshotId
        : "ctx-parallel-agents-child";
    if (typeof policySnapshotId !== "string" || policySnapshotId === "") {
      failures.push("the parent recorded no policy snapshot, so the children could not be delegated under one");
    } else {
      evidence.push(`sub/policySnapshotId=${policySnapshotId}`);
      const [firstChild, secondChild] = await Promise.all([
        client.spawnChild({
          parentExecutionId: parentId,
          prompt: readAndAnswerPrompt("alpha.txt"),
          policySnapshotId,
          contextSnapshotId: childContextSnapshotId,
          runId: `run_child_p0_014_first_${Date.now()}`,
          label: "p0-014-first",
          baseSha: options.baseSha
        }),
        client.spawnChild({
          parentExecutionId: parentId,
          prompt: readAndAnswerPrompt("beta.txt"),
          policySnapshotId,
          contextSnapshotId: childContextSnapshotId,
          runId: `run_child_p0_014_second_${Date.now()}`,
          label: "p0-014-second",
          baseSha: options.baseSha
        })
      ]);
      evidence.push(`sub/firstChild=${firstChild.status} ${firstChild.childExecutionId ?? firstChild.error ?? ""}`);
      evidence.push(`sub/secondChild=${secondChild.status} ${secondChild.childExecutionId ?? secondChild.error ?? ""}`);
      if (firstChild.status !== 201 || !firstChild.childExecutionId) {
        failures.push(`the first child was refused: ${firstChild.error ?? firstChild.status}`);
      }
      if (secondChild.status !== 201 || !secondChild.childExecutionId) {
        failures.push(`the second child was refused: ${secondChild.error ?? secondChild.status}`);
      }
      if (firstChild.childExecutionId && secondChild.childExecutionId) {
        await waitForStarted(client, firstChild.childExecutionId, runTimeoutMs);
        await waitForStarted(client, secondChild.childExecutionId, runTimeoutMs);
        const observedOverlap = !(await hasTerminal(client, firstChild.childExecutionId));
        evidence.push(`sub/firstChildStillRunningWhenSecondStarted=${observedOverlap}`);
        await waitForTerminal(client, firstChild.childExecutionId, runTimeoutMs);
        await waitForTerminal(client, secondChild.childExecutionId, runTimeoutMs);
        const counts = await timelineCounts(client);
        const first = await collectFacts(client, firstChild.childExecutionId, counts);
        const second = await collectFacts(client, secondChild.childExecutionId, counts);
        evidence.push(`sub/first=${describe(first)}`);
        evidence.push(`sub/second=${describe(second)}`);
        evidence.push(`sub/worktreesDiffer=${firstChild.workingDirectory !== secondChild.workingDirectory}`);

        const judgement = judgeParallelArm({
          label: "sub",
          first,
          second,
          firstNonce: nonces.childFirst,
          secondNonce: nonces.childSecond,
          observedOverlap
        });
        for (const link of judgement.links) links.push({ ...link, name: `sub/${link.name}` });
      }
    }
  }

  for (const link of links) {
    evidence.push(`${link.name}:${link.ok ? "ok" : "missing"} ${link.detail}`);
    if (!link.ok) failures.push(`the ${link.name} link does not hold: ${link.detail}`);
  }

  return {
    id: "P0-014",
    title: "并行 Agent 运行：两个 Agent 同时执行，各自独立运行",
    status: failures.length === 0 ? "passed" : "failed",
    startedAt,
    finishedAt: now().toISOString(),
    evidence,
    ...(failures.length > 0 ? { notes: failures.join("; ") } : {})
  };
}

/** Everything the judge needs to know about one run, read from the journal and the ledger. */
async function collectFacts(
  client: ParallelAgentsClient,
  executionId: string,
  timelineCounts: Map<string, number>
): Promise<ParallelArmFacts> {
  const detail = await client.execution(executionId);
  const logs = await client.logs(executionId);
  const entries = logs.entries;
  const runtimeEvents = entries.filter(entry => entry.kind === "runtime.event");
  const started = runtimeEvents.find(entry => entry.category === "execution.started");
  const terminals = runtimeEvents.filter(entry => TERMINAL_CATEGORIES.has(entry.category));
  const terminal = terminals.at(-1);
  const startedAt = started?.recordedAt;
  const finishedAt = terminal?.recordedAt;
  const usage = await client.usage(executionId);

  return {
    executionId,
    status: detail.status,
    ...(startedAt ? { startedAt } : {}),
    ...(finishedAt ? { finishedAt } : {}),
    ...(startedAt && finishedAt
      ? { durationMs: Math.max(0, Date.parse(finishedAt) - Date.parse(startedAt)) }
      : {}),
    terminalCount: terminals.length,
    ...(terminal ? { terminalCategory: terminal.category } : {}),
    answer: answerText(entries),
    ...(usage.record ? { usage: usage.record } : {}),
    journalCount: logs.count,
    ...(timelineCounts.has(executionId) ? { timelineCount: timelineCounts.get(executionId)! } : {})
  };
}

/** How many entries the shared timeline holds for each execution. */
async function timelineCounts(client: ParallelAgentsClient): Promise<Map<string, number>> {
  const counts = new Map<string, number>();
  const timeline = await client.timeline();
  for (const item of timeline.items) {
    counts.set(item.executionId, (counts.get(item.executionId) ?? 0) + 1);
  }
  return counts;
}

/** The assistant's own words, as they were streamed. */
function answerText(entries: ParallelJournalEntry[]): string {
  return entries
    .filter(entry => entry.kind === "runtime.event" && entry.category === "message.delta")
    .map(entry => {
      const delta = entry.payload?.["delta"];
      return typeof delta === "string" ? delta : "";
    })
    .join("");
}

/** Waits until the run is recorded as started — it is live from that moment on. */
async function waitForStarted(
  client: ParallelAgentsClient,
  executionId: string,
  timeoutMs: number
): Promise<boolean> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const entries = (await client.logs(executionId)).entries;
    if (entries.some(entry => entry.category === "execution.started")) return true;
    if (entries.some(entry => TERMINAL_CATEGORIES.has(entry.category))) return false;
    await sleep(150);
  }
  return false;
}

/** Whether the run already reached a terminal state. */
async function hasTerminal(client: ParallelAgentsClient, executionId: string): Promise<boolean> {
  const entries = (await client.logs(executionId)).entries;
  return entries.some(entry => TERMINAL_CATEGORIES.has(entry.category));
}

/**
 * Waits until the run is over *in the record*, not in the runtime's own claim.
 *
 * The runtime says "succeeded" before the journal has caught up, so a probe that measured the moment
 * the runtime reported success would measure a moving target.
 */
async function waitForTerminal(
  client: ParallelAgentsClient,
  executionId: string,
  timeoutMs: number
): Promise<string> {
  const deadline = Date.now() + timeoutMs;
  let status = "unknown";
  let terminalEventType: string | undefined;
  let eventCount = -1;
  let lastCount = -1;
  let stableReads = 0;
  while (Date.now() < deadline) {
    const detail = await client.execution(executionId);
    status = detail.status;
    terminalEventType = detail.terminalEventType;
    eventCount = detail.eventCount;
    if (terminalEventType && eventCount === lastCount) {
      stableReads += 1;
      if (stableReads >= 2) break;
    } else {
      stableReads = 0;
    }
    lastCount = eventCount;
    await sleep(300);
  }
  return status;
}

function describe(facts: ParallelArmFacts): string {
  return [
    `id=${facts.executionId}`,
    `status=${facts.status}`,
    `eventsTerminal=${facts.terminalCount}×${facts.terminalCategory ?? "none"}`,
    `span=${facts.startedAt ?? "?"} → ${facts.finishedAt ?? "?"}`,
    `durationMs=${facts.durationMs ?? "?"}`,
    `answerChars=${facts.answer.length}`,
    `usage=${facts.usage ? `${facts.usage.totalTokens} tokens / ${facts.usage.durationMs}ms` : "none"}`,
    `journal=${facts.journalCount} timeline=${facts.timelineCount ?? "?"}`
  ].join(" ");
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

// --------------------------------------------------------------------------------- the real run

async function main(): Promise<void> {
  const reportPath = resolve(
    process.env.PARALLEL_AGENTS_REPORT_PATH ?? "reports/phase0-parallel-agents.json"
  );
  const model = process.env.PI_PROBE_MODEL ?? "MiniMax-M3";
  if (!process.env.MINIMAX_CN_API_KEY) {
    throw new Error("MINIMAX_CN_API_KEY is not set; nothing to probe.");
  }

  const nonces = {
    first: "NONCE-ALPHA-7f3a91",
    second: "NONCE-BETA-9c2e04",
    childFirst: "NONCE-CHILD-ALPHA-51c7de",
    childSecond: "NONCE-CHILD-BETA-83fa20"
  };

  // Three workspaces: two for the top-level runs, one for the parent and its children.
  const root = await mkdtemp(join(tmpdir(), "codeweaver-parallel-agents-"));
  const dataDir = await mkdtemp(join(tmpdir(), "codeweaver-parallel-agents-data-"));
  const firstWorkspace = join(root, "first");
  const secondWorkspace = join(root, "second");
  const parentWorkspace = join(root, "parent");
  await makeWorkspace(firstWorkspace, { "secret.txt": `${nonces.first}\n` });
  await makeWorkspace(secondWorkspace, { "secret.txt": `${nonces.second}\n` });
  await makeWorkspace(parentWorkspace, {
    "alpha.txt": `${nonces.childFirst}\n`,
    "beta.txt": `${nonces.childSecond}\n`
  });

  const config = {
    host: "127.0.0.1",
    port: 0,
    dataDir,
    worktreeRoot: join(dataDir, "worktrees"),
    repository: parentWorkspace,
    // Hermes is registered so the server has a default runtime; this probe never talks to it.
    hermesBaseUrl: "http://127.0.0.1:9",
    hermesTimeoutMs: 5_000,
    pi: { provider: "minimax-cn", model }
  };

  const runtime = createRuntime(config);
  await runtime.ready();
  const server = await createApiServer(runtime.deps).listen(0);
  const client = httpClient(server.url);

  let results: TestResult[];
  try {
    results = [
      await executeParallelAgents(client, {
        workspaces: { first: firstWorkspace, second: secondWorkspace },
        parentWorkspace,
        baseSha: gitHead(parentWorkspace),
        nonces
      })
    ];
  } finally {
    await server.close();
    await runtime.close();
    await rm(root, { recursive: true, force: true });
    await rm(dataDir, { recursive: true, force: true });
  }

  await writeTestReport(reportPath, results);
  console.log(`Phase 0 parallel agents report: ${reportPath}`);
  for (const item of results) {
    console.log(`${item.id} ${item.status}: ${item.title}`);
    for (const line of item.evidence) console.log(`  ${line}`);
    if (item.notes) console.log(`  notes: ${item.notes}`);
  }
  if (results.some(item => item.status === "failed")) process.exitCode = 1;
}

/** A workspace with the committed files the agents have to read. */
async function makeWorkspace(path: string, files: Record<string, string>): Promise<void> {
  await mkdir(path, { recursive: true });
  for (const [name, content] of Object.entries(files)) {
    await writeFile(join(path, name), content, "utf8");
  }
  git(["init", "-b", "main"], path);
  git(["config", "user.name", "CodeWeaver Probe"], path);
  git(["config", "user.email", "probe@codeweaver.local"], path);
  git(["add", "-A"], path);
  git(["commit", "-m", "initial"], path);
}

function gitHead(path: string): string {
  const result = spawnSync("git", ["rev-parse", "HEAD"], { cwd: path, encoding: "utf8" });
  if (result.status !== 0) {
    throw new Error(`git rev-parse HEAD failed: ${result.stderr}`);
  }
  return result.stdout.trim();
}

/** The real client: everything goes over HTTP, the way the console drives it. */
function httpClient(baseUrl: string): ParallelAgentsClient {
  return {
    start: async input => {
      const response = await fetch(`${baseUrl}/api/v1/tasks/${input.taskId}/executions`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          projectId: "probe-parallel-agents",
          prompt: input.prompt,
          worktreePath: input.workspace,
          documents: [],
          selectedFiles: [input.file],
          architectureVersion: "1.0",
          policy: {
            snapshotId: "snap-parallel-agents",
            projectId: "probe-parallel-agents",
            taskId: input.taskId,
            executionId: "",
            worktreeId: "wt-parallel-agents",
            policyVersion: "1.0.0",
            grantedCapabilities: ["*"],
            allowedPaths: [input.workspace],
            expiresAt: "2099-01-01T00:00:00.000Z"
          },
          template: { name: "coding-agent", version: "1.4.2", contentHash: "e".repeat(64) },
          modelConfig: { model: process.env.PI_PROBE_MODEL ?? "MiniMax-M3" },
          correlationId: input.taskId,
          timeoutSeconds: input.timeoutSeconds ?? 300,
          allowedTools: ["read_file"],
          evaluationChecks: [
            {
              id: "check",
              name: "the file is still there",
              command: "node",
              args: ["-e", "process.exit(0)"],
              cwd: input.workspace
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
    execution: async executionId => {
      const response = await fetch(`${baseUrl}/api/v1/executions/${encodeURIComponent(executionId)}`);
      const body = (await response.json()) as {
        status?: string;
        checkpoint?: { terminalEventType?: string; eventCount?: number };
      };
      return {
        status: String(body.status ?? "unknown"),
        ...(body.checkpoint?.terminalEventType
          ? { terminalEventType: body.checkpoint.terminalEventType }
          : {}),
        eventCount: Number(body.checkpoint?.eventCount ?? -1)
      };
    },
    logs: async executionId => {
      const response = await fetch(`${baseUrl}/api/v1/executions/${encodeURIComponent(executionId)}/logs`);
      if (response.status !== 200) {
        throw new Error(`the journal was refused: ${response.status} ${await response.text()}`);
      }
      return (await response.json()) as { count: number; entries: ParallelJournalEntry[] };
    },
    timeline: async () => {
      const response = await fetch(`${baseUrl}/api/v1/audit/timeline`);
      if (response.status !== 200) {
        throw new Error(`the timeline was refused: ${response.status} ${await response.text()}`);
      }
      return (await response.json()) as { items: Array<{ executionId: string; sequence: number }> };
    },
    usage: async executionId => {
      const response = await fetch(`${baseUrl}/api/v1/executions/${encodeURIComponent(executionId)}/usage`);
      if (response.status === 404) return { status: 404 };
      if (response.status !== 200) {
        throw new Error(`the usage endpoint refused: ${response.status} ${await response.text()}`);
      }
      const body = (await response.json()) as { usage: ParallelUsageRecord };
      return { status: 200, record: body.usage };
    },
    spawnChild: async input => {
      const response = await fetch(
        `${baseUrl}/api/v1/executions/${encodeURIComponent(input.parentExecutionId)}/subagents`,
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            projectId: "probe-parallel-agents",
            taskId: "task-p0-014-child",
            runId: input.runId,
            prompt: input.prompt,
            contextSnapshotId: input.contextSnapshotId,
            policySnapshotId: input.policySnapshotId,
            worktreeId: "wt-parallel-agents-child",
            allowedTools: ["read_file"],
            timeoutSeconds: 300,
            correlationId: "task-p0-014-child",
            worktree: "isolated",
            baseSha: input.baseSha,
            label: input.label
          })
        }
      );
      const text = await response.text();
      if (response.status !== 201) {
        let message = text;
        try {
          const parsed = JSON.parse(text) as { error?: string; message?: string };
          message = parsed.message ?? parsed.error ?? text;
        } catch {
          // The server's own text; keep it.
        }
        return { status: response.status, error: message };
      }
      const body = JSON.parse(text) as {
        execution?: { executionId?: string };
        workingDirectory?: string;
      };
      return {
        status: response.status,
        ...(body.execution?.executionId ? { childExecutionId: body.execution.executionId } : {}),
        ...(body.workingDirectory ? { workingDirectory: body.workingDirectory } : {})
      };
    }
  };
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : undefined;
if (invokedPath === import.meta.url) {
  await main();
}
