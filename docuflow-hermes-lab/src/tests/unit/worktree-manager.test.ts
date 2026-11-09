import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { CodeChangePipeline } from "../../code-engine/code-change-pipeline.js";
import { SpawnGitRunner, gitOrThrow } from "../../code-engine/git-runner.js";
import { BaseShaMismatchError, WorktreeManager } from "../../code-engine/worktree-manager.js";
import type { PolicySnapshot } from "../../policy/permission-policy.js";

interface TestRepo {
  dir: string;
  git: SpawnGitRunner;
  baseSha: string;
}

async function makeRepo(): Promise<TestRepo> {
  const dir = await mkdtemp(join(tmpdir(), "docuflow-wt-"));
  const git = new SpawnGitRunner();
  await gitOrThrow(git, ["init", "-b", "main"], { cwd: dir });
  await gitOrThrow(git, ["config", "user.name", "DocuFlow Test"], { cwd: dir });
  await gitOrThrow(git, ["config", "user.email", "test@docuflow.local"], { cwd: dir });
  await writeFile(join(dir, "app.ts"), "export const value = 1;\n", "utf8");
  await gitOrThrow(git, ["add", "-A"], { cwd: dir });
  await gitOrThrow(git, ["commit", "-m", "initial"], { cwd: dir });
  const baseSha = (await gitOrThrow(git, ["rev-parse", "HEAD"], { cwd: dir })).stdout.trim();
  return { dir, git, baseSha };
}

function snapshot(worktreeId: string, allowedPaths: string[]): PolicySnapshot {
  return {
    snapshotId: "snap-1",
    projectId: "proj-1",
    taskId: "task-1",
    executionId: "exec-1",
    worktreeId,
    policyVersion: "1.0.0",
    grantedCapabilities: ["*"],
    allowedPaths,
    expiresAt: "2099-01-01T00:00:00.000Z"
  };
}

test("creates a worktree pinned to the base SHA, enforces it, and removes it", async () => {
  const repo = await makeRepo();
  const manager = new WorktreeManager(repo.git, join(repo.dir, ".worktrees"), { repository: repo.dir });
  try {
    const worktree = await manager.create(repo.baseSha, "task 1");
    assert.equal((await manager.head(worktree.path)).trim(), repo.baseSha);
    await manager.assertBaseSha(worktree.path, repo.baseSha);

    const listed = await Promise.all((await manager.list()).map(entry => realpath(entry)));
    assert.ok(listed.includes(await realpath(worktree.path)));

    await assert.rejects(
      manager.assertBaseSha(worktree.path, "b".repeat(40)),
      (error: unknown) => error instanceof BaseShaMismatchError
    );

    await manager.remove(worktree.path);
    assert.equal((await manager.list()).length, 0);
  } finally {
    await rm(repo.dir, { recursive: true, force: true });
  }
});

test("applies a patch inside a real worktree and records the commit", async () => {
  const repo = await makeRepo();
  const manager = new WorktreeManager(repo.git, join(repo.dir, ".worktrees"), { repository: repo.dir });
  const pipeline = new CodeChangePipeline(repo.git, manager);
  const worktree = await manager.create(repo.baseSha, "e2e");
  try {
    const outcome = await pipeline.apply({
      projectId: "proj-1",
      taskId: "task-1",
      executionId: "exec-1",
      worktreePath: worktree.path,
      expectedBaseSha: repo.baseSha,
      patch: [
        "diff --git a/app.ts b/app.ts",
        "--- a/app.ts",
        "+++ b/app.ts",
        "@@ -1 +1 @@",
        "-export const value = 1;",
        "+export const value = 2;",
        ""
      ].join("\n"),
      policy: { snapshot: snapshot("wt-1", [worktree.path]), allowedPaths: [worktree.path] },
      commit: { message: "feat: bump value" }
    });

    assert.equal(outcome.status, "applied");
    if (outcome.status !== "applied") return;
    assert.equal(outcome.record.operation, "apply");
    assert.equal(outcome.record.gitCommitSha?.length, 40);
    assert.match(await readFile(join(worktree.path, "app.ts"), "utf8"), /value = 2/);
  } finally {
    await rm(repo.dir, { recursive: true, force: true });
  }
});

test("rejects the same patch twice without leaving partial writes", async () => {
  const repo = await makeRepo();
  const manager = new WorktreeManager(repo.git, join(repo.dir, ".worktrees"), { repository: repo.dir });
  const pipeline = new CodeChangePipeline(repo.git, manager);
  const worktree = await manager.create(repo.baseSha, "dup");
  const patch = [
    "diff --git a/app.ts b/app.ts",
    "--- a/app.ts",
    "+++ b/app.ts",
    "@@ -1 +1 @@",
    "-export const value = 1;",
    "+export const value = 2;",
    ""
  ].join("\n");
  try {
    const first = await pipeline.apply({
      projectId: "proj-1",
      taskId: "task-1",
      executionId: "exec-1",
      worktreePath: worktree.path,
      expectedBaseSha: repo.baseSha,
      patch,
      policy: { snapshot: snapshot("wt-1", [worktree.path]), allowedPaths: [worktree.path] }
    });
    assert.equal(first.status, "applied");

    const before = await readFile(join(worktree.path, "app.ts"), "utf8");
    const second = await pipeline.apply({
      projectId: "proj-1",
      taskId: "task-1",
      executionId: "exec-1",
      worktreePath: worktree.path,
      expectedBaseSha: repo.baseSha,
      patch,
      policy: { snapshot: snapshot("wt-1", [worktree.path]), allowedPaths: [worktree.path] }
    });

    assert.equal(second.status, "denied");
    assert.equal(await readFile(join(worktree.path, "app.ts"), "utf8"), before);
  } finally {
    await rm(repo.dir, { recursive: true, force: true });
  }
});
