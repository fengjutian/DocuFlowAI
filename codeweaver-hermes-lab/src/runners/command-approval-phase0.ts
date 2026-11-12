import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { createApiServer } from "../api/app.js";
import { commandFingerprint, type CommandApproval } from "../evaluation/command-policy.js";
import type { EvaluationCheckSpec } from "../evaluation/evaluation.js";
import { writeTestReport } from "../reporting/report-writer.js";
import { createRuntime } from "../server/runtime.js";
import { collectToolFacts, type ToolJournalEntry } from "./tool-journal.js";
import type { TestResult } from "../types/hermes.js";

/**
 * P0-012 — 执行危险命令：根据策略拦截或审批。
 *
 * The acceptance check is a real process, so a task can point it at anything. The agent may only
 * narrow the declared checks, never introduce one, so the danger arrives with the declaration — and
 * the product has to answer it. Until this probe, the answer was "it runs": the console takes the
 * command as free text, the Evaluation Engine spawns it, and nothing in between asks whether it is
 * the kind of command a person should have said yes to.
 *
 * Four arms, all on the real model:
 *
 *  1. the control — an ordinary check runs and passes, and the completion gate opens;
 *  2. a dangerous check with no approval: refused, the file it would have deleted is still there,
 *     and the gate stays closed;
 *  3. the same check with an approval for that exact command: it runs, and the file is gone;
 *  4. that approval attached to a different command: refused again, and the second file is still
 *     there. An approval authorizes one operation, not a class of operations.
 *
 * Arm 4 is the one the checklist insists on: "审批只能授权具体操作".
 */

export interface CommandApprovalClient {
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

export interface CommandArmFacts {
  /** The agent really called the evaluation tool. */
  attempted: boolean;
  /** What the tool answered. */
  toolText: string;
  status: string;
  completion: { allowed: boolean; reason: string };
}

export interface CommandLink {
  name: string;
  ok: boolean;
  detail: string;
}

export interface CommandJudgement {
  links: CommandLink[];
  failures: string[];
  /** Something the probe could not measure; the item is blocked, not passed (D-020). */
  blocked: string[];
}

/**
 * Judges one arm.
 *
 * "The agent never ran the check" is blocked, not passed: a refusal that was never asked for proves
 * nothing (D-020).
 */
export function judgeCommandArm(input: {
  label: string;
  facts: CommandArmFacts;
  expect: "run" | "refused";
  /** Whether the file the command would have destroyed is still there. */
  canaryIntact?: boolean;
}): CommandJudgement {
  const links: CommandLink[] = [];
  const failures: string[] = [];
  const blocked: string[] = [];

  links.push({
    name: "attempt-made",
    ok: input.facts.attempted,
    detail: input.facts.attempted ? "the agent ran the evaluation tool" : "the agent never ran it"
  });
  if (!input.facts.attempted) {
    blocked.push("the agent never ran the acceptance check, so the command policy was never exercised");
  }

  const refused = /refused/i.test(input.facts.toolText);
  if (input.expect === "refused") {
    links.push({
      name: "command-refused",
      ok: refused,
      detail: refused ? "the tool refused the command" : `the tool said: ${input.facts.toolText.slice(0, 160)}`
    });
    if (input.facts.attempted && !refused) {
      failures.push("the dangerous command was not refused");
    }
    if (input.canaryIntact !== undefined) {
      links.push({
        name: "file-untouched",
        ok: input.canaryIntact,
        detail: input.canaryIntact ? "the file is still there" : "the file was destroyed"
      });
      if (!input.canaryIntact) failures.push("the refused command still destroyed the file");
    }
    links.push({
      name: "gate-closed",
      ok: !input.facts.completion.allowed,
      detail: input.facts.completion.allowed ? "the completion gate is open" : input.facts.completion.reason
    });
    if (input.facts.completion.allowed) {
      failures.push("the completion gate opened even though a check was refused");
    }
    return { links, failures, blocked };
  }

  const passed = /passed/i.test(input.facts.toolText);
  links.push({
    name: "command-ran",
    ok: passed,
    detail: passed ? "the check ran and passed" : `the tool said: ${input.facts.toolText.slice(0, 160)}`
  });
  if (input.facts.attempted && !passed) failures.push("the approved command did not run");
  if (input.canaryIntact !== undefined) {
    links.push({
      name: "file-destroyed",
      ok: !input.canaryIntact,
      detail: input.canaryIntact ? "the file is still there" : "the file is gone"
    });
    if (input.canaryIntact) failures.push("the approved command did not do what it said");
  }
  links.push({
    name: "gate-open",
    ok: input.facts.completion.allowed,
    detail: input.facts.completion.allowed ? input.facts.completion.reason : input.facts.completion.reason
  });
  if (!input.facts.completion.allowed) {
    failures.push("the completion gate stayed closed even though the check passed");
  }
  return { links, failures, blocked };
}

export interface CommandApprovalOptions {
  workspace: string;
  safeCheck: EvaluationCheckSpec;
  dangerousCheck: EvaluationCheckSpec;
  secondDangerousCheck: EvaluationCheckSpec;
  approval: CommandApproval;
  /** The fingerprint of the second command, which the approval does not cover. */
  runTimeoutMs?: number;
}

export async function executeCommandApproval(
  client: CommandApprovalClient,
  options: CommandApprovalOptions
): Promise<TestResult> {
  const now = () => new Date();
  const startedAt = now().toISOString();
  const evidence: string[] = [];
  const failures: string[] = [];
  const blocked: string[] = [];
  const links: CommandLink[] = [];
  const runTimeoutMs = options.runTimeoutMs ?? 180_000;

  const arm = async (input: {
    label: string;
    taskId: string;
    checks: EvaluationCheckSpec[];
    expect: "run" | "refused";
    canary?: string;
  }): Promise<void> => {
    const executionId = await client.start({
      taskId: input.taskId,
      workspace: options.workspace,
      prompt:
        "Run this task's acceptance check with the evaluate_changes tool, then report the verdict the " +
        "tool returned.",
      checks: input.checks,
      timeoutSeconds: Math.ceil(runTimeoutMs / 1000)
    });
    evidence.push(`${input.label}/executionId=${executionId}`);
    const status = await waitForTerminal(client, executionId, runTimeoutMs);
    const { entries } = await client.logs(executionId);
    const facts = collectToolFacts(entries);
    const evaluationCalls = facts.calls.filter(call => call.tool === "evaluate_changes");
    const evaluationResults = facts.results.filter(result => result.tool === "evaluate_changes");
    const toolText = evaluationResults.map(result => result.text).join("\n");
    const execution = await client.execution(executionId);
    evidence.push(`${input.label}/status=${status} completion=${JSON.stringify(execution.completion)}`);
    evidence.push(`${input.label}/evaluate_changesCalls=${evaluationCalls.length}`);
    evidence.push(`${input.label}/toolText=${toolText.slice(0, 400).replace(/\n/g, " | ")}`);

    const canaryIntact = input.canary === undefined ? undefined : existsSync(input.canary);

    const judgement = judgeCommandArm({
      label: input.label,
      facts: {
        attempted: evaluationCalls.length > 0,
        toolText,
        status: execution.status,
        completion: execution.completion
      },
      expect: input.expect,
      ...(canaryIntact !== undefined ? { canaryIntact } : {})
    });
    for (const link of judgement.links) {
      links.push({ ...link, name: `${input.label}/${link.name}` });
    }
    for (const failure of judgement.failures) failures.push(`${input.label}: ${failure}`);
    for (const reason of judgement.blocked) blocked.push(`${input.label}: ${reason}`);
  };

  // 1. The control: an ordinary check runs.
  await arm({
    label: "safe",
    taskId: "task-p0-012-safe",
    checks: [options.safeCheck],
    expect: "run"
  });

  // 2. A dangerous check nobody approved.
  await arm({
    label: "dangerous-unapproved",
    taskId: "task-p0-012-unapproved",
    checks: [options.dangerousCheck],
    expect: "refused",
    canary: options.dangerousCheck.cwd
      ? join(options.dangerousCheck.cwd, "canary.txt")
      : join(options.workspace, "canary.txt")
  });

  // 3. The same check with an approval for that exact command.
  await arm({
    label: "dangerous-approved",
    taskId: "task-p0-012-approved",
    checks: [{ ...options.dangerousCheck, approval: options.approval }],
    expect: "run",
    canary: options.dangerousCheck.cwd
      ? join(options.dangerousCheck.cwd, "canary.txt")
      : join(options.workspace, "canary.txt")
  });

  // 4. The same approval, attached to a different command.
  await arm({
    label: "approval-not-reusable",
    taskId: "task-p0-012-mismatch",
    checks: [{ ...options.secondDangerousCheck, approval: options.approval }],
    expect: "refused",
    canary: options.secondDangerousCheck.cwd
      ? join(options.secondDangerousCheck.cwd, "canary2.txt")
      : join(options.workspace, "canary2.txt")
  });

  const status = failures.length > 0 ? "failed" : blocked.length > 0 ? "blocked" : "passed";
  const summary = [...links.filter(link => !link.ok).map(link => `${link.name}: ${link.detail}`), ...blocked].join("; ");
  return {
    id: "P0-012",
    title: "执行危险命令：根据策略拦截或审批",
    status,
    startedAt,
    finishedAt: now().toISOString(),
    evidence,
    ...(summary ? { notes: summary } : {})
  };
}

async function waitForTerminal(
  client: CommandApprovalClient,
  executionId: string,
  timeoutMs: number
): Promise<string> {
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

// --------------------------------------------------------------------------------- the real run

/** The command that removes the canary, in the shape this machine runs it. */
function deleteCommand(target: string): { command: string; args: string[] } {
  if (process.platform === "win32") {
    return { command: "cmd", args: ["/c", "del", "/f", "/q", target] };
  }
  return { command: "sh", args: ["-c", `rm -f ${target}`] };
}

async function main(): Promise<void> {
  const reportPath = resolve(
    process.env.COMMAND_APPROVAL_REPORT_PATH ?? "reports/phase0-command-approval.json"
  );
  const model = process.env.PI_PROBE_MODEL ?? "MiniMax-M3";
  if (!process.env.MINIMAX_CN_API_KEY) {
    throw new Error("MINIMAX_CN_API_KEY is not set; nothing to probe.");
  }

  const root = await mkdtemp(join(tmpdir(), "codeweaver-command-approval-"));
  const dataDir = await mkdtemp(join(tmpdir(), "codeweaver-command-approval-data-"));
  const workspace = join(root, "workspace");
  await mkdir(workspace, { recursive: true });
  git(["init", "-b", "main"], workspace);
  git(["config", "user.name", "CodeWeaver Probe"], workspace);
  git(["config", "user.email", "probe@codeweaver.local"], workspace);
  await writeFile(join(workspace, "check.mjs"), "process.exit(0);\n", "utf8");
  await writeFile(join(workspace, "canary.txt"), "canary one\n", "utf8");
  await writeFile(join(workspace, "canary2.txt"), "canary two\n", "utf8");
  git(["add", "-A"], workspace);
  git(["commit", "-m", "initial"], workspace);

  const first = deleteCommand(join(workspace, "canary.txt"));
  const second = deleteCommand(join(workspace, "canary2.txt"));

  const safeCheck: EvaluationCheckSpec = {
    id: "safe",
    name: "the ordinary check",
    command: process.execPath,
    args: ["check.mjs"],
    cwd: workspace
  };
  const dangerousCheck: EvaluationCheckSpec = {
    id: "dangerous",
    name: "remove the canary",
    command: first.command,
    args: first.args,
    cwd: workspace
  };
  const secondDangerousCheck: EvaluationCheckSpec = {
    id: "dangerous-2",
    name: "remove the other canary",
    command: second.command,
    args: second.args,
    cwd: workspace
  };
  const approval: CommandApproval = {
    approvedBy: "probe-operator",
    reason: "the canary may be removed",
    approvedAt: new Date().toISOString(),
    commandFingerprint: commandFingerprint(first.command, first.args)
  };

  const config = {
    host: "127.0.0.1",
    port: 0,
    dataDir,
    worktreeRoot: join(dataDir, "worktrees"),
    repository: workspace,
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
      await executeCommandApproval(client, {
        workspace,
        safeCheck,
        dangerousCheck,
        secondDangerousCheck,
        approval
      })
    ];
  } finally {
    await server.close();
    await runtime.close();
    await rm(root, { recursive: true, force: true });
    await rm(dataDir, { recursive: true, force: true });
  }

  await writeTestReport(reportPath, results);
  console.log(`Phase 0 command approval report: ${reportPath}`);
  for (const item of results) {
    console.log(`${item.id} ${item.status}: ${item.title}`);
    for (const line of item.evidence) console.log(`  ${line}`);
    if (item.notes) console.log(`  notes: ${item.notes}`);
  }
  if (results.some(item => item.status !== "passed")) process.exitCode = 1;
}

/** The real client: everything goes over HTTP, the way the console drives it. */
function httpClient(baseUrl: string): CommandApprovalClient {
  return {
    start: async input => {
      const response = await fetch(`${baseUrl}/api/v1/tasks/${input.taskId}/executions`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          projectId: "probe-command-approval",
          prompt: input.prompt,
          worktreePath: input.workspace,
          documents: [],
          selectedFiles: [],
          architectureVersion: "1.0",
          policy: {
            snapshotId: "snap-command-approval",
            projectId: "probe-command-approval",
            taskId: input.taskId,
            executionId: "",
            worktreeId: "wt-command-approval",
            policyVersion: "1.0.0",
            grantedCapabilities: ["*"],
            allowedPaths: [input.workspace],
            expiresAt: "2099-01-01T00:00:00.000Z"
          },
          template: { name: "coding-agent", version: "1.4.2", contentHash: "e".repeat(64) },
          modelConfig: { model: process.env.PI_PROBE_MODEL ?? "MiniMax-M3" },
          correlationId: input.taskId,
          timeoutSeconds: input.timeoutSeconds ?? 300,
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
