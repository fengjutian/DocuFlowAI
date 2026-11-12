import { mkdtemp, readFile, rm, writeFile, mkdir } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { createApiServer } from "../api/app.js";
import { SpawnGitRunner } from "../code-engine/git-runner.js";
import { writeTestReport } from "../reporting/report-writer.js";
import { createRuntime } from "../server/runtime.js";
import type { TestResult } from "../types/hermes.js";

/**
 * The persistent runtime configuration, proven against a real server (V0.3.0).
 *
 * The claims this probe makes are about what survives a restart and what a disable actually does,
 * so it does the thing a restart is: it stops the server and starts a new one over the same data
 * directory. Everything it checks is read back from the server's own HTTP surface, which is the
 * surface the console uses.
 *
 * No runtime is contacted: the probes are about the configuration, the routing decision and the
 * audit trail, none of which need a model.
 */

const RUNTIME_ID = "hermes-local";

interface Probe {
  id: string;
  title: string;
  run(): Promise<TestResult>;
}

async function git(args: string[], cwd: string): Promise<void> {
  const result = await new SpawnGitRunner().run(args, { cwd });
  if (result.code !== 0) throw new Error(`git ${args.join(" ")} failed: ${result.stderr}`);
}

async function startServer(dataDir: string, repo: string): Promise<{
  url: string;
  stop(): Promise<void>;
}> {
  const runtime = createRuntime({
    host: "127.0.0.1",
    port: 0,
    dataDir,
    worktreeRoot: join(dataDir, "worktrees"),
    repository: repo,
    // Nothing here talks to a runtime; the address only has to be well formed.
    hermesBaseUrl: "http://127.0.0.1:9",
    hermesTimeoutMs: 1_000
  });
  await runtime.ready();
  const server = await createApiServer(runtime.deps).listen(0);
  return {
    url: server.url,
    stop: async () => {
      await server.close();
      await runtime.close();
    }
  };
}

async function post(url: string, path: string, body?: unknown): Promise<Response> {
  return fetch(`${url}${path}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    ...(body === undefined ? {} : { body: JSON.stringify(body) })
  });
}

async function patch(url: string, path: string, body: unknown): Promise<Response> {
  return fetch(`${url}${path}`, {
    method: "PATCH",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body)
  });
}

async function getJson(url: string, path: string): Promise<Record<string, unknown>> {
  const response = await fetch(`${url}${path}`);
  return (await response.json()) as Record<string, unknown>;
}

function taskBody(repo: string, runtimeId?: string): Record<string, unknown> {
  return {
    projectId: "probe-v030",
    prompt: "do the thing",
    worktreePath: repo,
    documents: ["spec.md"],
    selectedFiles: ["app.ts"],
    architectureVersion: "1.0",
    policy: {
      snapshotId: "snap-v030",
      projectId: "probe-v030",
      taskId: "task-v030",
      executionId: "exec-v030",
      worktreeId: "wt-v030",
      policyVersion: "1.0.0",
      grantedCapabilities: ["*"],
      allowedPaths: [repo],
      expiresAt: "2099-01-01T00:00:00.000Z"
    },
    template: { name: "coding-agent", version: "1.4.2", contentHash: "e".repeat(64) },
    modelConfig: { model: "probe" },
    correlationId: "probe-v030",
    timeoutSeconds: 30,
    allowedTools: ["read_file"],
    ...(runtimeId ? { runtimeId } : {})
  };
}

async function main(): Promise<void> {
  const reportPath = resolve(
    process.env.RUNTIME_CONFIG_REPORT_PATH ?? "reports/v030-runtime-config.json"
  );
  const repo = await mkdtemp(join(tmpdir(), "codeweaver-v030-repo-"));
  const dataDir = await mkdtemp(join(tmpdir(), "codeweaver-v030-data-"));
  await git(["init", "-b", "main"], repo);
  await git(["config", "user.name", "CodeWeaver Probe"], repo);
  await git(["config", "user.email", "probe@codeweaver.local"], repo);
  await writeFile(join(repo, "app.ts"), "export const value = 1;\n", "utf8");
  await writeFile(join(repo, "spec.md"), "# Spec\nRequirement A\n", "utf8");
  await git(["add", "-A"], repo);
  await git(["commit", "-m", "initial"], repo);

  const results: TestResult[] = [];
  let server = await startServer(dataDir, repo);

  try {
    // --- 1. A configuration change is written down ------------------------------------------------
    {
      const startedAt = new Date().toISOString();
      const evidence: string[] = [];
      const failures: string[] = [];
      try {
        const disabled = await post(server.url, `/api/v1/runtimes/${RUNTIME_ID}/disable`, { actor: "console" });
        evidence.push(`disable status=${disabled.status}`);
        const updated = await patch(server.url, `/api/v1/runtimes/${RUNTIME_ID}`, {
          priority: 11,
          tags: ["drained", "slow"],
          actor: "console"
        });
        evidence.push(`patch status=${updated.status}`);
        const setDefault = await post(server.url, "/api/v1/runtimes/default", {
          runtimeId: RUNTIME_ID,
          actor: "console"
        });
        evidence.push(`default status=${setDefault.status}`);

        const onDisk = JSON.parse(
          await readFile(join(dataDir, "config", "runtimes.json"), "utf8")
        ) as { runtimes: Record<string, { enabled: boolean; priority: number; tags: string[] }> };
        evidence.push(
          `file: enabled=${onDisk.runtimes[RUNTIME_ID]?.enabled} priority=${onDisk.runtimes[RUNTIME_ID]?.priority} tags=${onDisk.runtimes[RUNTIME_ID]?.tags?.join("|")}`
        );
        if (onDisk.runtimes[RUNTIME_ID]?.enabled !== false) failures.push("the file does not record the disable");
        if (onDisk.runtimes[RUNTIME_ID]?.priority !== 11) failures.push("the file does not record the priority");
        if ((onDisk.runtimes[RUNTIME_ID]?.tags ?? []).join("|") !== "drained|slow") {
          failures.push("the file does not record the tags");
        }
      } catch (error) {
        failures.push(error instanceof Error ? error.message : String(error));
      }
      results.push({
        id: "V030-CONFIG-PERSISTED",
        title: "运行时配置写入磁盘：启用状态、优先级、标签",
        status: failures.length === 0 ? "passed" : "failed",
        startedAt,
        finishedAt: new Date().toISOString(),
        evidence,
        ...(failures.length > 0 ? { notes: failures.join("; ") } : {})
      });
    }

    // --- 2. A disabled runtime refuses new work, with a reason -------------------------------------
    {
      const startedAt = new Date().toISOString();
      const evidence: string[] = [];
      const failures: string[] = [];
      try {
        const response = await post(
          server.url,
          "/api/v1/tasks/task-v030/executions",
          taskBody(repo, RUNTIME_ID)
        );
        const body = await response.text();
        evidence.push(`status=${response.status}`);
        evidence.push(`body=${body.slice(0, 240)}`);
        if (response.status !== 409) failures.push(`the server answered ${response.status} instead of 409`);
        if (!body.includes("disabled")) failures.push("the refusal does not say the runtime is disabled");
      } catch (error) {
        failures.push(error instanceof Error ? error.message : String(error));
      }
      results.push({
        id: "V030-DISABLED-REFUSES-NEW-WORK",
        title: "禁用后新任务被明确拒绝，且理由是「已禁用」而不是「不存在」",
        status: failures.length === 0 ? "passed" : "failed",
        startedAt,
        finishedAt: new Date().toISOString(),
        evidence,
        ...(failures.length > 0 ? { notes: failures.join("; ") } : {})
      });
    }

    // --- 3. The operation record --------------------------------------------------------------------
    {
      const startedAt = new Date().toISOString();
      const evidence: string[] = [];
      const failures: string[] = [];
      try {
        const history = (await getJson(server.url, "/api/v1/runtimes/config/history")) as {
          history: Array<{ action: string; actor: string; runtimeId: string }>;
        };
        evidence.push(
          `history=${history.history.map(entry => `${entry.action}@${entry.actor}:${entry.runtimeId}`).join(", ")}`
        );
        const actions = new Set(history.history.map(entry => entry.action));
        for (const action of ["disable", "update", "default"]) {
          if (!actions.has(action)) failures.push(`the operation record has no '${action}' entry`);
        }
        if (!history.history.some(entry => entry.actor === "console")) {
          failures.push("the operation record does not name the operator");
        }
      } catch (error) {
        failures.push(error instanceof Error ? error.message : String(error));
      }
      results.push({
        id: "V030-OPERATION-RECORD",
        title: "每次配置变更都在操作记录里，且记录了操作者",
        status: failures.length === 0 ? "passed" : "failed",
        startedAt,
        finishedAt: new Date().toISOString(),
        evidence,
        ...(failures.length > 0 ? { notes: failures.join("; ") } : {})
      });
    }

    // --- 4. The configuration survives a restart ----------------------------------------------------
    await server.stop();
    server = await startServer(dataDir, repo);
    {
      const startedAt = new Date().toISOString();
      const evidence: string[] = [];
      const failures: string[] = [];
      try {
        const list = (await getJson(server.url, "/api/v1/runtimes")) as {
          runtimes: Array<{ id: string; enabled: boolean; priority: number; tags: string[]; isDefault: boolean }>;
        };
        const entry = list.runtimes.find(item => item.id === RUNTIME_ID);
        evidence.push(
          `after restart: enabled=${entry?.enabled} priority=${entry?.priority} tags=${entry?.tags?.join("|")} isDefault=${entry?.isDefault}`
        );
        if (entry?.enabled !== false) failures.push("the runtime came back enabled");
        if (entry?.priority !== 11) failures.push("the priority did not survive the restart");
        if ((entry?.tags ?? []).join("|") !== "drained|slow") failures.push("the tags did not survive the restart");
        if (entry?.isDefault !== true) failures.push("the default runtime did not survive the restart");

        // The restore is on the record as well.
        const history = (await getJson(server.url, "/api/v1/runtimes/config/history")) as {
          history: Array<{ action: string }>;
        };
        evidence.push(`history actions=${history.history.map(entry => entry.action).join(",")}`);
        if (!history.history.some(entry => entry.action === "restore")) {
          failures.push("the restart is not on the operation record");
        }
      } catch (error) {
        failures.push(error instanceof Error ? error.message : String(error));
      }
      results.push({
        id: "V030-CONFIG-SURVIVES-RESTART",
        title: "重启后配置原样恢复，恢复本身也有记录",
        status: failures.length === 0 ? "passed" : "failed",
        startedAt,
        finishedAt: new Date().toISOString(),
        evidence,
        ...(failures.length > 0 ? { notes: failures.join("; ") } : {})
      });
    }

    // --- 5. A configuration that cannot be read stops the server -----------------------------------
    {
      const startedAt = new Date().toISOString();
      const evidence: string[] = [];
      const failures: string[] = [];
      try {
        await server.stop();
        await mkdir(join(dataDir, "config"), { recursive: true });
        await writeFile(join(dataDir, "config", "runtimes.json"), "{ not json at all", "utf8");
        let refused = false;
        let diagnostic = "";
        try {
          const broken = await startServer(dataDir, repo);
          await broken.stop();
        } catch (error) {
          refused = true;
          diagnostic = error instanceof Error ? error.message : String(error);
        }
        evidence.push(`refused=${refused}`);
        evidence.push(`diagnostic=${diagnostic.slice(0, 240)}`);
        if (!refused) failures.push("the server started with a configuration it cannot read");
        if (!diagnostic.includes("runtimes.json")) {
          failures.push("the diagnostic does not name the configuration file");
        }
      } catch (error) {
        failures.push(error instanceof Error ? error.message : String(error));
      }
      results.push({
        id: "V030-BROKEN-CONFIG-FAILS-CLOSED",
        title: "配置损坏时服务器拒绝启动，并给出可定位的诊断",
        status: failures.length === 0 ? "passed" : "failed",
        startedAt,
        finishedAt: new Date().toISOString(),
        evidence,
        ...(failures.length > 0 ? { notes: failures.join("; ") } : {})
      });
    }
  } finally {
    await server.stop().catch(() => undefined);
    await rm(repo, { recursive: true, force: true });
    await rm(dataDir, { recursive: true, force: true });
  }

  await writeTestReport(reportPath, results);
  console.log(`\nRuntime configuration report: ${reportPath}`);
  for (const item of results) {
    console.log(`${item.id} ${item.status}: ${item.title}`);
    for (const line of item.evidence) console.log(`  ${line}`);
    if (item.notes) console.log(`  notes: ${item.notes}`);
  }
  if (results.some(item => item.status === "failed")) process.exitCode = 1;
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : undefined;
if (invokedPath === import.meta.url) {
  await main();
}
