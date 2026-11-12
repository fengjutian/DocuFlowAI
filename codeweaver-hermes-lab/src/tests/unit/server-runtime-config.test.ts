import test from "node:test";
import assert from "node:assert/strict";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createApiServer } from "../../api/app.js";
import { SpawnGitRunner } from "../../code-engine/git-runner.js";
import { RuntimeConfigError } from "../../persistence/runtime-config.js";
import { createRuntime } from "../../server/runtime.js";

/**
 * The runtime configuration at the level of the server (V0.3.0).
 *
 * These tests are about the claims a user of the product can check: a disabled runtime refuses new
 * work and says why, the configuration survives a restart, and a configuration that cannot be read
 * stops the server instead of quietly running on half of it.
 */

interface Harness {
  url: string;
  repo: string;
  dataDir: string;
  /** Stops the server. The data directory survives, so a restart can be tested over it. */
  stop(): Promise<void>;
  /** Removes the directories the harness created. */
  cleanup(): Promise<void>;
}

async function git(args: string[], cwd: string): Promise<void> {
  const result = await new SpawnGitRunner().run(args, { cwd });
  if (result.code !== 0) throw new Error(`git ${args.join(" ")} failed: ${result.stderr}`);
}

/** A server with a real repository behind it and no runtime the tests would have to reach. */
async function harness(): Promise<Harness> {
  const repo = await mkdtemp(join(tmpdir(), "codeweaver-v030-repo-"));
  await git(["init", "-b", "main"], repo);
  await git(["config", "user.name", "CodeWeaver Test"], repo);
  await git(["config", "user.email", "test@codeweaver.local"], repo);
  await writeFile(join(repo, "app.ts"), "export const value = 1;\n", "utf8");
  await writeFile(join(repo, "spec.md"), "# Spec\nRequirement A\n", "utf8");
  await git(["add", "-A"], repo);
  await git(["commit", "-m", "initial"], repo);

  const dataDir = await mkdtemp(join(tmpdir(), "codeweaver-v030-data-"));
  const runtime = createRuntime({
    host: "127.0.0.1",
    port: 0,
    dataDir,
    worktreeRoot: join(dataDir, "worktrees"),
    repository: repo,
    // Nothing in these tests contacts a runtime; the address only has to be well formed.
    hermesBaseUrl: "http://127.0.0.1:9",
    hermesTimeoutMs: 1_000
  });
  await runtime.ready();
  const server = await createApiServer(runtime.deps).listen(0);
  return {
    url: server.url,
    repo,
    dataDir,
    stop: async () => {
      await server.close();
      await runtime.close();
    },
    cleanup: async () => {
      await rm(repo, { recursive: true, force: true });
      await rm(dataDir, { recursive: true, force: true });
    }
  };
}

function taskBody(repo: string, runtimeId?: string): Record<string, unknown> {
  return {
    projectId: "proj-1",
    prompt: "implement the feature",
    worktreePath: repo,
    documents: ["spec.md"],
    selectedFiles: ["app.ts"],
    architectureVersion: "1.0",
    policy: {
      snapshotId: "snap-1",
      projectId: "proj-1",
      taskId: "task-1",
      executionId: "exec-1",
      worktreeId: "wt-1",
      policyVersion: "1.0.0",
      grantedCapabilities: ["*"],
      allowedPaths: [repo],
      expiresAt: "2099-01-01T00:00:00.000Z"
    },
    template: { name: "coding-agent", version: "1.4.2", contentHash: "e".repeat(64) },
    modelConfig: { model: "hermes-default" },
    correlationId: "corr-1",
    timeoutSeconds: 30,
    allowedTools: ["read_file"],
    ...(runtimeId ? { runtimeId } : {})
  };
}

test("a task naming a disabled runtime is refused with a reason that says it is disabled", async () => {
  const app = await harness();
  try {
    const disabled = await fetch(`${app.url}/api/v1/runtimes/hermes-local/disable`, { method: "POST" });
    assert.equal(disabled.status, 200);

    const response = await fetch(`${app.url}/api/v1/tasks/task-1/executions`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(taskBody(app.repo, "hermes-local"))
    });
    const body = await response.text();
    assert.equal(response.status, 409);
    // "Unknown runtime" would send the operator looking for a runtime that is right there.
    assert.match(body, /disabled/);
    assert.doesNotMatch(body, /Unknown runtime/);
  } finally {
    await app.stop();
    await app.cleanup();
  }
});

test("the configuration survives a restart of the server", async () => {
  const app = await harness();
  try {
    const disabled = await fetch(`${app.url}/api/v1/runtimes/hermes-local/disable`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ actor: "console" })
    });
    assert.equal(disabled.status, 200);
    const patched = await fetch(`${app.url}/api/v1/runtimes/hermes-local`, {
      method: "PATCH",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ priority: 12, tags: ["drained"] })
    });
    assert.equal(patched.status, 200);
  } finally {
    // The data directory stays: the restart below runs over it.
    await app.stop();
  }

  // A new process over the same data directory: this is what a restart looks like.
  const runtime = createRuntime({
    host: "127.0.0.1",
    port: 0,
    dataDir: app.dataDir,
    worktreeRoot: join(app.dataDir, "worktrees"),
    repository: app.repo,
    hermesBaseUrl: "http://127.0.0.1:9",
    hermesTimeoutMs: 1_000
  });
  await runtime.ready();
  const server = await createApiServer(runtime.deps).listen(0);
  try {
    const list = await fetch(`${server.url}/api/v1/runtimes`);
    const body = await list.json() as {
      runtimes: Array<{ id: string; enabled: boolean; priority: number; tags: string[] }>;
    };
    const hermes = body.runtimes.find(item => item.id === "hermes-local")!;
    assert.equal(hermes.enabled, false);
    assert.equal(hermes.priority, 12);
    assert.deepEqual(hermes.tags, ["drained"]);

    // The operation record survived as well.
    const history = await fetch(`${server.url}/api/v1/runtimes/config/history`);
    const historyBody = await history.json() as { history: Array<{ action: string; actor: string }> };
    assert.ok(historyBody.history.some(entry => entry.action === "disable" && entry.actor === "console"));
    assert.ok(historyBody.history.some(entry => entry.action === "restore"));
  } finally {
    await server.close();
    await runtime.close();
    await app.cleanup();
  }
});

test("a configuration that cannot be read stops the server with a diagnostic", async () => {
  const dataDir = await mkdtemp(join(tmpdir(), "codeweaver-v030-broken-"));
  const repo = await mkdtemp(join(tmpdir(), "codeweaver-v030-repo-"));
  try {
    await mkdir(join(dataDir, "config"), { recursive: true });
    const path = join(dataDir, "config", "runtimes.json");
    await writeFile(path, "{ not json at all", "utf8");

    const runtime = createRuntime({
      host: "127.0.0.1",
      port: 0,
      dataDir,
      worktreeRoot: join(dataDir, "worktrees"),
      repository: repo,
      hermesBaseUrl: "http://127.0.0.1:9",
      hermesTimeoutMs: 1_000
    });

    // The server refuses to come up: it does not route work according to a state nobody chose.
    await assert.rejects(
      () => runtime.ready(),
      (error: unknown) => {
        assert.ok(error instanceof RuntimeConfigError);
        assert.equal(error.path, path);
        assert.match(error.message, /runtimes\.json/);
        return true;
      }
    );
  } finally {
    await rm(dataDir, { recursive: true, force: true });
    await rm(repo, { recursive: true, force: true });
  }
});

test("the configuration endpoint reports the configuration and the operation record", async () => {
  const app = await harness();
  try {
    await fetch(`${app.url}/api/v1/runtimes/hermes-local`, {
      method: "PATCH",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ priority: 3, tags: ["primary"] })
    });
    const setDefault = await fetch(`${app.url}/api/v1/runtimes/default`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ runtimeId: "hermes-local", actor: "console" })
    });
    assert.equal(setDefault.status, 200);

    const config = await fetch(`${app.url}/api/v1/runtimes/config`);
    const body = await config.json() as {
      defaultRuntimeId: string | null;
      runtimes: Array<{ id: string; priority: number; tags: string[]; isDefault: boolean }>;
    };
    assert.equal(body.defaultRuntimeId, "hermes-local");
    const hermes = body.runtimes.find(item => item.id === "hermes-local")!;
    assert.equal(hermes.priority, 3);
    assert.deepEqual(hermes.tags, ["primary"]);
    assert.equal(hermes.isDefault, true);

    const history = await fetch(`${app.url}/api/v1/runtimes/config/history`);
    const historyBody = await history.json() as { history: Array<{ action: string }> };
    assert.ok(historyBody.history.some(entry => entry.action === "default"));
  } finally {
    await app.stop();
    await app.cleanup();
  }
});

test("a configuration change that names no field is refused", async () => {
  const app = await harness();
  try {
    const response = await fetch(`${app.url}/api/v1/runtimes/hermes-local`, {
      method: "PATCH",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({})
    });
    assert.equal(response.status, 400);
  } finally {
    await app.stop();
    await app.cleanup();
  }
});

test("a wrongly typed priority is refused", async () => {
  const app = await harness();
  try {
    const response = await fetch(`${app.url}/api/v1/runtimes/hermes-local`, {
      method: "PATCH",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ priority: "high" })
    });
    assert.equal(response.status, 400);
  } finally {
    await app.stop();
    await app.cleanup();
  }
});
