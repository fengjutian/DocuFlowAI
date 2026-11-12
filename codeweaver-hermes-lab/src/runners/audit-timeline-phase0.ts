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
 * P0-022: the audit timeline, on the Pi runtime and a real model.
 *
 * The checklist asks for a "correlated event trace"; design 15 asks for "从审计事件还原完整执行
 * 过程". The trace endpoint (P0-024) already answers "which records belong together" — task, run,
 * tool, change, commit, test. What it cannot answer is "what happened, and in what order", because
 * a chain is a set of links and a timeline is a sequence.
 *
 * The probe runs the task twice on the same task id, so the timeline has to hold more than one
 * execution and put them in the recorded order. The yardstick is the journal itself: the timeline
 * must hold every recorded event of every execution exactly once — a timeline that quietly dropped
 * half a run would otherwise look complete.
 *
 * The claims are:
 *
 *  - journal-covered: the timeline holds every recorded event, exactly once, and nothing else.
 *  - ordered: the timeline is ordered by the recorded time, and each execution keeps its own
 *    journal order inside it.
 *  - correlated: both executions of the task are on one timeline, tied to the same task.
 *  - process: the stages an execution went through are reconstructed from the recorded events.
 *  - evidence: the commit and the passing verdict are readable *from the timeline itself* — the
 *    point of an audit timeline is that the record is enough on its own.
 *  - task-scope: asking for the task returns the whole tree, not one execution.
 *  - not-found: an execution that was never recorded is refused instead of answered with an empty
 *    timeline.
 */

export interface AuditTimelineItemView {
  executionId: string;
  sequence: number;
  kind: string;
  category: string;
  recordedAt: string;
  taskId?: string;
  parentExecutionId?: string;
  payload: Record<string, unknown>;
}

export interface AuditTimelineExecutionView {
  executionId: string;
  taskIds: string[];
  parentExecutionId?: string;
  childExecutionIds: string[];
  runtimeId?: string;
  eventCount: number;
  firstRecordedAt: string;
  lastRecordedAt: string;
  gaps: number[];
  phases: string[];
  timestampsMonotonic: boolean;
}

export interface AuditTimelineView {
  items: AuditTimelineItemView[];
  executions: AuditTimelineExecutionView[];
  taskIds: string[];
  categories: Record<string, number>;
  gaps: Array<{ executionId: string; sequences: number[] }>;
  truncated: boolean;
}

export interface AuditTimelineClient {
  /** Starts the task and returns its execution id. */
  start(input: { taskId: string; prompt: string; allowedTools: string[] }): Promise<string>;
  /** The execution's own state: the status, the recorded terminal event and the event count. */
  execution(executionId: string): Promise<{
    status: string;
    terminalEventType?: string;
    eventCount: number;
  }>;
  /** The raw journal of one execution — the yardstick the timeline is measured against. */
  logs(executionId: string): Promise<{ count: number; entries: Array<{ sequence: number }> }>;
  timeline(query: { taskId?: string; executionId?: string }): Promise<AuditTimelineView>;
  /** The status the timeline answers with for an execution id, 404 included. */
  timelineStatus(executionId: string): Promise<number>;
}

export interface AuditTimelineOptions {
  now?: () => Date;
  runTimeoutMs?: number;
}

export type AuditTimelineLinkName =
  | "journal-covered"
  | "ordered"
  | "correlated"
  | "process"
  | "evidence"
  | "task-scope"
  | "not-found";

export interface AuditTimelineLink {
  name: AuditTimelineLinkName;
  ok: boolean;
  detail: string;
}

export interface AuditTimelineJudgement {
  links: AuditTimelineLink[];
  ok: boolean;
}

/**
 * Judges the timeline.
 *
 * Every link is judged on its own, so a failure names what is missing instead of saying "the
 * timeline is incomplete". The yardstick is the journal: the timeline is measured against the
 * events the server actually recorded.
 */
export function judgeAuditTimeline(input: {
  taskId: string;
  firstExecutionId: string;
  secondExecutionId: string;
  timeline: AuditTimelineView;
  taskTimeline: AuditTimelineView;
  journalCounts: Record<string, number>;
  missingTimelineStatus?: number | undefined;
}): AuditTimelineJudgement {
  const links: AuditTimelineLink[] = [];
  const timeline = input.timeline;

  // Every recorded event, exactly once. Both directions matter: an event that is missing and an
  // event that was invented look the same to a reader who only sees the timeline.
  const seen = new Set<string>();
  let duplicates = 0;
  for (const item of timeline.items) {
    const key = `${item.executionId}:${item.sequence}`;
    if (seen.has(key)) duplicates += 1;
    seen.add(key);
  }
  const journalTotal = Object.values(input.journalCounts).reduce((sum, count) => sum + count, 0);
  const covered =
    duplicates === 0 &&
    timeline.items.length === journalTotal &&
    Object.entries(input.journalCounts).every(([executionId, count]) => {
      const held = timeline.items.filter(item => item.executionId === executionId).length;
      return held === count;
    });
  links.push({
    name: "journal-covered",
    ok: covered,
    detail: covered
      ? `the timeline holds all ${journalTotal} recorded events, once each`
      : `the timeline holds ${timeline.items.length} events (${duplicates} repeated) against ${journalTotal} recorded`
  });

  // The global order is the recorded time; inside an execution the journal order is never broken.
  let timeOrdered = true;
  let firstBreak: { previous: AuditTimelineItemView; current: AuditTimelineItemView } | undefined;
  for (let index = 1; index < timeline.items.length; index += 1) {
    if (timeline.items[index]!.recordedAt < timeline.items[index - 1]!.recordedAt) {
      timeOrdered = false;
      if (!firstBreak) {
        firstBreak = { previous: timeline.items[index - 1]!, current: timeline.items[index]! };
      }
    }
  }
  let sequenceOrdered = true;
  for (const execution of timeline.executions) {
    const sequences = timeline.items
      .filter(item => item.executionId === execution.executionId)
      .map(item => item.sequence);
    for (let index = 1; index < sequences.length; index += 1) {
      if (sequences[index]! <= sequences[index - 1]!) sequenceOrdered = false;
    }
  }
  const skews = timeline.executions
    .filter(execution => !execution.timestampsMonotonic)
    .map(execution => execution.executionId);
  links.push({
    name: "ordered",
    ok: timeOrdered && sequenceOrdered,
    detail:
      timeOrdered && sequenceOrdered
        ? "the timeline is ordered by time, and every execution keeps its own journal order"
        : `the recorded order is broken (time ordered: ${timeOrdered}, journal order kept: ${sequenceOrdered}${
            firstBreak
              ? `; first break: ${firstBreak.previous.executionId}#${firstBreak.previous.sequence} ${firstBreak.previous.category} at ${firstBreak.previous.recordedAt} then ${firstBreak.current.executionId}#${firstBreak.current.sequence} ${firstBreak.current.category} at ${firstBreak.current.recordedAt}`
              : ""
          }${skews.length > 0 ? `; executions whose own timestamps disagree with their order: ${skews.join(",")}` : ""})`
  });

  // Correlation: the executions of one task are on one timeline and carry the task.
  const first = timeline.executions.find(item => item.executionId === input.firstExecutionId);
  const second = timeline.executions.find(item => item.executionId === input.secondExecutionId);
  const correlated =
    first !== undefined &&
    second !== undefined &&
    first.taskIds.includes(input.taskId) &&
    second.taskIds.includes(input.taskId) &&
    first.lastRecordedAt <= second.firstRecordedAt;
  links.push({
    name: "correlated",
    ok: correlated,
    detail: correlated
      ? `both executions of ${input.taskId} are on the timeline, in the recorded order`
      : `the timeline holds ${timeline.executions.length} execution(s); the task's runs are not both correlated`
  });

  // The process: the stages the events reconstruct.
  const requiredPhases = ["started", "tool", "change", "evaluation", "completed"];
  const phases = first?.phases ?? [];
  const missingPhases = requiredPhases.filter(phase => !phases.includes(phase));
  links.push({
    name: "process",
    ok: missingPhases.length === 0,
    detail:
      missingPhases.length === 0
        ? `the first run reconstructs as ${phases.join(" → ")}`
        : `the first run is missing the stage(s) ${missingPhases.join(", ")} (it has ${phases.join(", ") || "nothing"})`
  });

  // The evidence has to be readable from the timeline alone — otherwise the timeline is a list of
  // event names and not a record of what happened.
  const firstItems = timeline.items.filter(item => item.executionId === input.firstExecutionId);
  const change = firstItems.find(item => item.category === "change.applied");
  const commitSha = change?.payload?.["gitCommitSha"];
  const evaluation = firstItems.find(item => item.category === "evaluation.completed");
  const verdict = evaluation?.payload?.["verdict"];
  const hasEvidence = typeof commitSha === "string" && commitSha !== "" && verdict === "passed";
  links.push({
    name: "evidence",
    ok: hasEvidence,
    detail: hasEvidence
      ? `the timeline carries commit ${String(commitSha).slice(0, 12)} and the verdict ${String(verdict)}`
      : `the timeline carries no commit (${String(commitSha ?? "none")}) or no passing verdict (${String(verdict ?? "none")})`
  });

  // The task scope: asking for the task returns the whole tree.
  const taskExecutionIds = input.taskTimeline.executions.map(item => item.executionId);
  const taskScopeOk =
    taskExecutionIds.includes(input.firstExecutionId) &&
    taskExecutionIds.includes(input.secondExecutionId);
  links.push({
    name: "task-scope",
    ok: taskScopeOk,
    detail: taskScopeOk
      ? `the task's timeline holds ${taskExecutionIds.length} execution(s)`
      : `the task's timeline holds ${taskExecutionIds.join(",") || "nothing"}`
  });

  // An id that was never an execution has to be refused. An empty timeline for a mistyped id reads
  // exactly like a real but silent run.
  const missingStatus = input.missingTimelineStatus;
  links.push({
    name: "not-found",
    ok: missingStatus === 404,
    detail:
      missingStatus === 404
        ? "an execution that was never recorded is refused"
        : `an execution that was never recorded answered ${missingStatus ?? "?"} instead of 404`
  });

  return { links, ok: links.every(link => link.ok) };
}

/**
 * P0-022: one task, run twice, read back as one timeline.
 */
export async function executeAuditTimeline(
  client: AuditTimelineClient,
  options: AuditTimelineOptions = {}
): Promise<TestResult> {
  const now = options.now ?? (() => new Date());
  const startedAt = now().toISOString();
  const evidence: string[] = [];
  const failures: string[] = [];

  const taskId = "task-p0-022";

  const firstExecutionId = await client.start({
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
  evidence.push(`firstExecutionId=${firstExecutionId}`);
  const first = await waitForTerminal(client, firstExecutionId, options.runTimeoutMs ?? 300_000);
  evidence.push(`firstStatus=${first.status}`);
  evidence.push(`firstTerminalEvent=${first.terminalEventType ?? "none"}`);
  evidence.push(`firstJournalEvents=${first.eventCount}`);

  // The same task, run again. Two executions of one task are what makes the timeline a correlation
  // rather than a list: the reader has to see both runs, in the order they happened.
  const secondExecutionId = await client.start({
    taskId,
    prompt: [
      "Work in the repository you are given. Do exactly these three steps, using the tools:",
      "1. Call read_file with path 'app2.ts' and look at what it contains.",
      "2. Call request_apply_diff with a unified diff that replaces the single line",
      "   'export const value = 1;' with 'export const value = 3;'. The diff must use the",
      "   a/app2.ts and b/app2.ts prefixes and one hunk, so that git apply accepts it.",
      "3. Call evaluate_changes to run the acceptance checks of this task."
    ].join(" "),
    allowedTools: ["read_file", "request_diff", "request_apply_diff", "evaluate_changes"]
  });
  evidence.push(`secondExecutionId=${secondExecutionId}`);
  const second = await waitForTerminal(client, secondExecutionId, options.runTimeoutMs ?? 300_000);
  evidence.push(`secondStatus=${second.status}`);
  evidence.push(`secondTerminalEvent=${second.terminalEventType ?? "none"}`);
  evidence.push(`secondJournalEvents=${second.eventCount}`);

  const timeline = await client.timeline({});
  const taskTimeline = await client.timeline({ taskId });
  const journalCounts: Record<string, number> = {};
  for (const executionId of [firstExecutionId, secondExecutionId]) {
    const logs = await client.logs(executionId);
    journalCounts[executionId] = logs.count;
  }
  const missingTimelineStatus = await client.timelineStatus("exec-never-recorded-probe");
  evidence.push(`timelineItems=${timeline.items.length}`);
  evidence.push(`timelineExecutions=${timeline.executions.map(item => item.executionId).join(",") || "none"}`);
  evidence.push(`taskTimelineExecutions=${taskTimeline.executions.map(item => item.executionId).join(",") || "none"}`);
  evidence.push(`journalCounts=${Object.entries(journalCounts).map(([id, count]) => `${id}=${count}`).join(",")}`);
  evidence.push(`missingTimelineStatus=${missingTimelineStatus}`);
  for (const execution of timeline.executions) {
    evidence.push(
      `execution=${execution.executionId} events=${execution.eventCount} phases=${execution.phases.join(">") || "none"} timestampsMonotonic=${execution.timestampsMonotonic} gaps=${execution.gaps.join(",") || "none"}`
    );
  }

  const judgement = judgeAuditTimeline({
    taskId,
    firstExecutionId,
    secondExecutionId,
    timeline,
    taskTimeline,
    journalCounts,
    missingTimelineStatus
  });
  for (const link of judgement.links) {
    evidence.push(`${link.name}:${link.ok ? "ok" : "missing"} ${link.detail}`);
  }

  if (first.status !== "succeeded") failures.push(`the first run ended as '${first.status}'`);
  if (second.status !== "succeeded") failures.push(`the second run ended as '${second.status}'`);
  for (const link of judgement.links) {
    if (!link.ok) failures.push(`the ${link.name} link does not hold: ${link.detail}`);
  }

  return {
    id: "P0-022",
    title: "审计时间线：从审计事件还原完整执行过程",
    status: failures.length === 0 ? "passed" : "failed",
    startedAt,
    finishedAt: now().toISOString(),
    evidence,
    ...(failures.length > 0 ? { notes: failures.join("; ") } : {})
  };
}

/**
 * Waits until the run is over *in the record*, not in the runtime's own claim.
 *
 * The runtime says "succeeded" before the journal has caught up: the terminal event is written
 * through the coordinator's queue, so a probe that measures the moment the runtime reports success
 * measures a moving target — and a timeline read too early is short by exactly the entries that
 * had not been written yet. The probe waits for the recorded terminal event and for the event count
 * to stop moving.
 */
async function waitForTerminal(
  client: AuditTimelineClient,
  executionId: string,
  timeoutMs: number
): Promise<{ status: string; terminalEventType?: string; eventCount: number }> {
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
  return { status, ...(terminalEventType ? { terminalEventType } : {}), eventCount };
}

function sleep(ms: number): Promise<void> {
  return new Promise(resolveSleep => setTimeout(resolveSleep, ms));
}

function git(args: string[], cwd: string): string {
  const result = spawnSync("git", args, { cwd, encoding: "utf8" });
  if (result.status !== 0) {
    throw new Error(`git ${args.join(" ")} failed: ${result.stderr}`);
  }
  return result.stdout;
}

async function main(): Promise<void> {
  const reportPath = resolve(
    process.env.AUDIT_TIMELINE_REPORT_PATH ?? "reports/phase0-audit-timeline.json"
  );
  const model = process.env.PI_PROBE_MODEL ?? "MiniMax-M3";
  if (!process.env.MINIMAX_CN_API_KEY) {
    throw new Error("MINIMAX_CN_API_KEY is not set; nothing to probe.");
  }

  const repo = await mkdtemp(join(tmpdir(), "codeweaver-audit-timeline-repo-"));
  const dataDir = await mkdtemp(join(tmpdir(), "codeweaver-audit-timeline-data-"));
  await writeFile(join(repo, "app.ts"), "export const value = 1;\n", "utf8");
  await writeFile(join(repo, "app2.ts"), "export const value = 1;\n", "utf8");
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
  await writeFile(
    join(repo, "check2.mjs"),
    [
      "import { readFileSync } from 'node:fs';",
      "const content = readFileSync(new URL('./app2.ts', import.meta.url), 'utf8');",
      "if (!content.includes('value = 3')) {",
      "  console.error('app2.ts still holds the old value');",
      "  process.exit(1);",
      "}",
      "console.log('app2.ts holds value = 3');",
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

  const results = [await executeAuditTimeline(client)];

  await writeTestReport(reportPath, results);
  console.log(`Phase 0 audit timeline report: ${reportPath}`);
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
function httpClient(baseUrl: string, workspace: string): AuditTimelineClient {
  return {
    start: async input => {
      const response = await fetch(`${baseUrl}/api/v1/tasks/${input.taskId}/executions`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          projectId: "probe-audit-timeline",
          prompt: input.prompt,
          worktreePath: workspace,
          documents: ["spec.md"],
          selectedFiles: ["app.ts", "app2.ts"],
          architectureVersion: "1.0",
          policy: {
            snapshotId: "snap-audit-timeline",
            projectId: "probe-audit-timeline",
            taskId: input.taskId,
            executionId: "",
            worktreeId: "wt-audit-timeline",
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
              name: "the edited file holds the new value",
              command: "node",
              args: ["check.mjs", "check2.mjs"],
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
      return (await response.json()) as { count: number; entries: Array<{ sequence: number }> };
    },
    timeline: async query => {
      const params = new URLSearchParams();
      if (query.taskId) params.set("taskId", query.taskId);
      if (query.executionId) params.set("executionId", query.executionId);
      const suffix = params.toString();
      const response = await fetch(`${baseUrl}/api/v1/audit/timeline${suffix ? `?${suffix}` : ""}`);
      if (response.status !== 200) {
        throw new Error(`the timeline was refused: ${response.status} ${await response.text()}`);
      }
      return (await response.json()) as AuditTimelineView;
    },
    timelineStatus: async executionId => {
      const response = await fetch(
        `${baseUrl}/api/v1/audit/timeline?executionId=${encodeURIComponent(executionId)}`
      );
      return response.status;
    }
  };
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : undefined;
if (invokedPath === import.meta.url) {
  await main();
}
