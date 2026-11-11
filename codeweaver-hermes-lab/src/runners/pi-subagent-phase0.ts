import { spawnSync } from "node:child_process";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { createApiServer } from "../api/app.js";
import { writeTestReport } from "../reporting/report-writer.js";
import { createRuntime } from "../server/runtime.js";
import type { JournalStore } from "../journal/journal.js";
import type { TestResult } from "../types/hermes.js";

/**
 * P0-008: subagent delegation, against the Pi runtime and a real model.
 *
 * A delegated child is only worth having if four things are true at the same time: the child really
 * ran, it ran on the same runtime as its parent, both sides of the relation are recorded in the
 * journal, and the lineage resolves to the root of the task tree. Each of those is checked against
 * what the server actually did — not against what the delegation request asked for.
 *
 * The parent runs on the real model, because a delegation probe on a mock proves nothing about the
 * runtime that ships. The child is delegated through the real HTTP endpoint, so the console and the
 * probe see the same thing.
 *
 * A refusal is not a failure of the design, it is a fact: if the server never creates the child,
 * the probe reports `blocked` and says why instead of inventing a child that never existed (D-020).
 */

const TERMINAL_STATUSES = new Set(["succeeded", "failed", "cancelled", "interrupted"]);

/** The parent task: short, so the probe spends its time on the delegation and not on the answer. */
const DEFAULT_PROMPT =
  "You are the parent execution of a subagent delegation probe. " +
  "In one sentence, explain what a subagent delegation is.";

/** The child task: the shortest possible real task, so the child reaches a terminal state fast. */
const DEFAULT_CHILD_PROMPT = "Reply with exactly one word: CHILD_OK";

/** What the server answered to a delegation request. */
export interface SpawnOutcome {
  /** The HTTP status of the delegation request. */
  status: number;
  /** The error code the server refused with, when it refused. */
  errorCode?: string;
  /** The child execution, when the server created one. */
  childExecutionId?: string;
  worktreeMode?: string;
  workingDirectory?: string;
  rootExecutionId?: string;
  /** The lineage the server recorded for the child. */
  lineage?: string[];
  /** The server's own words, when it refused. */
  error?: string;
}

/** The children and the lineage the server records for one execution. */
export interface SubagentLineage {
  children: string[];
  lineage: string[];
  rootExecutionId: string;
}

/** The slice of the system this probe drives; the real one talks to a server over the wire. */
export interface PiSubagentClient {
  /** Starts the parent execution on the real runtime and returns its id. */
  startParent(): Promise<string>;
  /** Waits until the runtime is producing output; false when it never did. */
  waitForOutput(executionId: string, timeoutMs: number): Promise<boolean>;
  /** The status the runtime reports for an execution. */
  status(executionId: string): Promise<string>;
  /** Waits for a terminal state and returns it. */
  waitForTerminal(executionId: string, timeoutMs: number): Promise<string>;
  /** The runtime the server names for an execution. */
  runtimeId(executionId: string): Promise<string | undefined>;
  /** The runtimes the server has registered. */
  runtimes(): Promise<Array<{ id: string; runtimeType: string }>>;
  /** Delegates a child execution from the parent execution. */
  spawnChild(parentExecutionId: string): Promise<SpawnOutcome>;
  /** A journaled fact of an execution, by category. */
  fact(executionId: string, category: string): Promise<Record<string, unknown> | undefined>;
  /** The children and the lineage the server records for an execution. */
  lineage(executionId: string): Promise<SubagentLineage>;
}

export interface PiSubagentOptions {
  prompt?: string;
  childPrompt?: string;
  /** How long to wait for the parent to produce output. */
  startTimeoutMs?: number;
  /** How long to wait for the child to finish. */
  terminalTimeoutMs?: number;
  now?: () => Date;
}

export async function executePiSubagentPhase0(
  client: PiSubagentClient,
  options: PiSubagentOptions = {}
): Promise<TestResult[]> {
  const now = options.now ?? (() => new Date());
  const startedAt = now().toISOString();
  const startTimeoutMs = options.startTimeoutMs ?? 120_000;
  const terminalTimeoutMs = options.terminalTimeoutMs ?? 180_000;
  const evidence: string[] = [];

  const parentId = await client.startParent();
  evidence.push(`parentExecutionId=${parentId}`);

  // The parent has to be a real execution on the real runtime. A parent that never produced output
  // means the model did not run, so there is nothing to delegate from.
  const streaming = await client.waitForOutput(parentId, startTimeoutMs);
  evidence.push(`parentProducedOutput=${streaming}`);
  if (!streaming) {
    return [
      {
        id: "P0-008",
        title: "Subagent delegation",
        status: "blocked",
        startedAt,
        finishedAt: now().toISOString(),
        evidence,
        notes:
          "The real model never produced output on the parent execution, so there was no real parent to delegate from"
      }
    ];
  }

  const parentRuntimeId = await client.runtimeId(parentId);
  evidence.push(`parentRuntimeId=${String(parentRuntimeId)}`);

  // The registered runtimes are part of the evidence: a delegation that fails while the parent's
  // runtime is registered is not a delegation that had nowhere to go.
  const registered = await client.runtimes();
  evidence.push(`registeredRuntimes=${registered.map(item => item.id).join(",")}`);

  const spawn = await client.spawnChild(parentId);
  evidence.push(`spawnStatus=${spawn.status}`);
  evidence.push(`childExecutionId=${spawn.childExecutionId ?? "none"}`);
  if (spawn.worktreeMode) evidence.push(`worktreeMode=${spawn.worktreeMode}`);
  if (spawn.workingDirectory) evidence.push(`childWorkingDirectory=${spawn.workingDirectory}`);
  if (spawn.rootExecutionId) evidence.push(`spawnRootExecutionId=${spawn.rootExecutionId}`);
  if (spawn.lineage) evidence.push(`spawnLineage=${JSON.stringify(spawn.lineage)}`);
  if (spawn.errorCode) evidence.push(`spawnErrorCode=${spawn.errorCode}`);
  if (spawn.error) evidence.push(`spawnError=${spawn.error}`);

  const childId = spawn.childExecutionId;
  if (spawn.status !== 201 || !childId) {
    // No child means no delegation: the probe reports what the server said instead of a verdict
    // about a child that does not exist. The parent's own lineage is still read, so the report
    // shows that the delegation endpoint answers for a parent without children.
    const parentTree = await readLineage(client, parentId);
    if (parentTree) {
      evidence.push(`parentChildren=${JSON.stringify(parentTree.children)}`);
      evidence.push(`parentLineage=${JSON.stringify(parentTree.lineage)}`);
      evidence.push(`parentRootExecutionId=${parentTree.rootExecutionId}`);
    }
    return [
      {
        id: "P0-008",
        title: "Subagent delegation",
        status: "blocked",
        startedAt,
        finishedAt: now().toISOString(),
        evidence,
        notes: `The server did not create a child execution: HTTP ${spawn.status}${
          spawn.errorCode ? ` (${spawn.errorCode})` : ""
        }${spawn.error ? ` — ${spawn.error}` : ""}; the parent ran on '${String(parentRuntimeId)}'`
      }
    ];
  }

  const failures: string[] = [];

  // 1. The child really ran, and it finished.
  const childStatus = await client.waitForTerminal(childId, terminalTimeoutMs);
  evidence.push(`childStatus=${childStatus}`);
  if (!TERMINAL_STATUSES.has(childStatus)) {
    failures.push(`the child execution never reached a terminal state (last status '${childStatus}')`);
  }

  // 2. The child ran on the same runtime as its parent. A child on another runtime is a different
  //    task tree wearing the same lineage.
  const childRuntimeId = await client.runtimeId(childId);
  evidence.push(`childRuntimeId=${String(childRuntimeId)}`);
  if (childRuntimeId !== parentRuntimeId) {
    failures.push(
      `the child ran on '${String(childRuntimeId)}' instead of the parent's '${String(parentRuntimeId)}'`
    );
  }

  // 3. Both sides of the relation are in the journal: the parent knows it spawned, the child knows
  //    who its parent is.
  const spawned = await client.fact(parentId, "subagent.spawned");
  evidence.push(`parentSubagentSpawned=${JSON.stringify(spawned ?? null)}`);
  if (!spawned) {
    failures.push("the parent's journal has no 'subagent.spawned' fact");
  } else if (spawned["childExecutionId"] !== childId) {
    failures.push(
      `the parent's 'subagent.spawned' fact names child '${String(spawned["childExecutionId"])}' instead of '${childId}'`
    );
  }

  const parentLink = await client.fact(childId, "subagent.parent");
  evidence.push(`childSubagentParent=${JSON.stringify(parentLink ?? null)}`);
  if (!parentLink) {
    failures.push("the child's journal has no 'subagent.parent' fact");
  } else if (parentLink["parentExecutionId"] !== parentId) {
    failures.push(
      `the child's 'subagent.parent' fact names parent '${String(parentLink["parentExecutionId"])}' instead of '${parentId}'`
    );
  }

  // 4. The lineage resolves: the child's chain reaches the parent, the parent lists the child, and
  //    the root of the tree is the parent.
  const childTree = await client.lineage(childId);
  evidence.push(`childLineage=${JSON.stringify(childTree.lineage)}`);
  evidence.push(`childRootExecutionId=${childTree.rootExecutionId}`);
  if (!childTree.lineage.includes(parentId)) {
    failures.push(
      `the child's lineage does not reach the parent ('${childTree.lineage.join(" -> ")}')`
    );
  }
  if (childTree.rootExecutionId !== parentId) {
    failures.push(
      `the lineage resolves to root '${childTree.rootExecutionId}' instead of the parent '${parentId}'`
    );
  }

  const parentTree = await client.lineage(parentId);
  evidence.push(`parentChildren=${JSON.stringify(parentTree.children)}`);
  evidence.push(`parentLineage=${JSON.stringify(parentTree.lineage)}`);
  evidence.push(`parentRootExecutionId=${parentTree.rootExecutionId}`);
  if (!parentTree.children.includes(childId)) {
    failures.push(`the parent does not list the child '${childId}' among its children`);
  }
  if (parentTree.rootExecutionId !== parentId) {
    failures.push(
      `the parent's own root is '${parentTree.rootExecutionId}' instead of '${parentId}'`
    );
  }

  return [
    {
      id: "P0-008",
      title: "Subagent delegation",
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
    process.env.PI_SUBAGENT_REPORT_PATH ?? "reports/phase0-pi-subagent.json"
  );
  const model = process.env.PI_PROBE_MODEL ?? "MiniMax-M3";
  if (!process.env.MINIMAX_CN_API_KEY) {
    throw new Error("MINIMAX_CN_API_KEY is not set; nothing to probe.");
  }

  const repo = await mkdtemp(join(tmpdir(), "codeweaver-pi-subagent-repo-"));
  const dataDir = await mkdtemp(join(tmpdir(), "codeweaver-pi-subagent-data-"));
  git(["init", "-b", "main"], repo);
  git(["config", "user.name", "CodeWeaver Probe"], repo);
  git(["config", "user.email", "probe@codeweaver.local"], repo);
  await writeFile(join(repo, "README.md"), "# Subagent probe\n", "utf8");
  git(["add", "-A"], repo);
  git(["commit", "-m", "initial"], repo);
  const baseSha = git(["rev-parse", "HEAD"], repo).trim();

  const runtime = await createRuntime({
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
  const url = server.url;

  const startBody = {
    projectId: "probe-pi-subagent",
    prompt: DEFAULT_PROMPT,
    worktreePath: repo,
    documents: ["README.md"],
    selectedFiles: ["README.md"],
    architectureVersion: "1.0",
    policy: {
      snapshotId: "snap-subagent",
      projectId: "probe-pi-subagent",
      taskId: "task-pi-subagent",
      executionId: "",
      worktreeId: "wt-subagent",
      policyVersion: "1.0.0",
      grantedCapabilities: ["*"],
      allowedPaths: [repo],
      expiresAt: "2099-01-01T00:00:00.000Z"
    },
    template: { name: "coding-agent", version: "1.4.2", contentHash: "e".repeat(64) },
    modelConfig: { model },
    correlationId: "probe-pi-subagent",
    timeoutSeconds: 600,
    allowedTools: [],
    runtimeId: "pi-local"
  };

  const readEvents = async (executionId: string): Promise<Array<{ type?: string }>> => {
    const response = await fetch(`${url}/api/v1/executions/${executionId}/events`);
    const body = await response.json();
    return (body.events ?? []) as Array<{ type?: string }>;
  };

  const client: PiSubagentClient = {
    startParent: async () => {
      const response = await fetch(`${url}/api/v1/tasks/task-pi-subagent/executions`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(startBody)
      });
      if (response.status !== 201) {
        throw new Error(`the parent task was refused: ${response.status} ${await response.text()}`);
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
    status: async executionId => {
      const response = await fetch(`${url}/api/v1/executions/${executionId}`);
      if (response.status !== 200) return "unknown";
      const body = await response.json();
      return String(body.status ?? "unknown");
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
    },
    runtimeId: async executionId => {
      const response = await fetch(`${url}/api/v1/executions/${executionId}`);
      if (response.status !== 200) return undefined;
      const body = await response.json();
      return typeof body.runtimeId === "string" ? body.runtimeId : undefined;
    },
    runtimes: async () => {
      const response = await fetch(`${url}/api/v1/runtimes`);
      if (response.status !== 200) return [];
      const body = await response.json();
      return ((body.runtimes ?? []) as Array<{ id?: string; runtimeType?: string }>)
        .filter(item => typeof item.id === "string")
        .map(item => ({ id: item.id as string, runtimeType: item.runtimeType ?? "unknown" }));
    },
    spawnChild: async parentExecutionId => {
      const response = await fetch(`${url}/api/v1/executions/${parentExecutionId}/subagents`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          projectId: "probe-pi-subagent",
          taskId: "task-pi-subagent-child",
          runId: `run_child_${Date.now()}`,
          prompt: DEFAULT_CHILD_PROMPT,
          contextSnapshotId: "ctx-subagent-child",
          policySnapshotId: "pol-subagent-child",
          worktreeId: "wt-subagent-child",
          allowedTools: [],
          timeoutSeconds: 300,
          correlationId: "probe-pi-subagent-child",
          worktree: "isolated",
          baseSha,
          label: "probe-child"
        })
      });
      const text = await response.text();
      if (response.status !== 201) {
        let message = text;
        let code: string | undefined;
        try {
          const parsed = JSON.parse(text) as { error?: string; message?: string };
          message = parsed.message ?? parsed.error ?? text;
          code = parsed.error;
        } catch {
          // A non-JSON body is the server's own text; keep it.
        }
        return {
          status: response.status,
          ...(code ? { errorCode: code } : {}),
          error: message
        };
      }
      const body = JSON.parse(text) as {
        execution?: { executionId?: string };
        worktreeMode?: string;
        workingDirectory?: string;
        rootExecutionId?: string;
        lineage?: string[];
      };
      return {
        status: response.status,
        ...(body.execution?.executionId ? { childExecutionId: body.execution.executionId } : {}),
        ...(body.worktreeMode ? { worktreeMode: body.worktreeMode } : {}),
        ...(body.workingDirectory ? { workingDirectory: body.workingDirectory } : {}),
        ...(body.rootExecutionId ? { rootExecutionId: body.rootExecutionId } : {}),
        ...(body.lineage ? { lineage: body.lineage } : {})
      };
    },
    fact: async (executionId, category) => {
      const journal = runtime.deps.journal as JournalStore;
      const entries = await journal.read(executionId);
      return entries.find(entry => entry.category === category)?.payload;
    },
    lineage: async executionId => {
      const response = await fetch(`${url}/api/v1/executions/${executionId}/subagents`);
      if (response.status !== 200) {
        throw new Error(`the lineage request failed: ${response.status} ${await response.text()}`);
      }
      const body = await response.json();
      return {
        children: (body.children ?? []) as string[],
        lineage: (body.lineage ?? []) as string[],
        rootExecutionId: String(body.rootExecutionId ?? "")
      };
    }
  };

  const results = await executePiSubagentPhase0(client);
  await writeTestReport(reportPath, results);
  console.log(`Phase 0 subagent report: ${reportPath}`);
  for (const item of results) {
    console.log(`${item.id} ${item.status}: ${item.title}`);
    for (const line of item.evidence) console.log(`  ${line}`);
    if (item.notes) console.log(`  notes: ${item.notes}`);
  }
  if (results.some(item => item.status === "failed" || item.status === "blocked")) process.exitCode = 1;

  await server.close();
  await runtime.close();
  await rm(repo, { recursive: true, force: true });
  await rm(dataDir, { recursive: true, force: true });
}

function sleep(ms: number): Promise<void> {
  return new Promise(resolveSleep => setTimeout(resolveSleep, ms));
}

/** The lineage the server records for an execution, or nothing when it cannot answer. */
async function readLineage(
  client: PiSubagentClient,
  executionId: string
): Promise<SubagentLineage | undefined> {
  try {
    return await client.lineage(executionId);
  } catch {
    return undefined;
  }
}

function git(args: string[], cwd: string): string {
  const result = spawnSync("git", args, { cwd, encoding: "utf8" });
  if (result.status !== 0) {
    throw new Error(`git ${args.join(" ")} failed: ${result.stderr}`);
  }
  return result.stdout;
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : undefined;
if (invokedPath === import.meta.url) {
  await main();
}
