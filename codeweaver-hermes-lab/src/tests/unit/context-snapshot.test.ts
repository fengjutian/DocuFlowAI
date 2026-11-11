import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  ContextCaptureError,
  captureContextSnapshot,
  verifyContextSnapshot
} from "../../context/context-snapshot.js";
import type { GitRunner } from "../../code-engine/git-runner.js";
import { ImmutableRecordStore } from "../../persistence/record-store.js";
import { assertContextSnapshot } from "../../persistence/records.js";

const baseSha = "a".repeat(40);

function fakeGit(sha = baseSha): GitRunner {
  return {
    async run() {
      return { code: 0, stdout: `${sha}\n`, stderr: "" };
    }
  };
}

async function makeWorktree(): Promise<string> {
  const root = await mkdtemp(join(tmpdir(), "codeweaver-ctx-"));
  await writeFile(join(root, "spec.md"), "# Spec\nRequirement A\n", "utf8");
  await writeFile(join(root, "arch.md"), "# Architecture\nsingle control plane\n", "utf8");
  await writeFile(join(root, "app.ts"), "export const value = 1;\n", "utf8");
  return root;
}

const common = {
  projectId: "proj-1",
  taskId: "task-1",
  documents: ["spec.md", "arch.md"],
  selectedFiles: ["app.ts"],
  architectureVersion: "1.0",
  now: () => new Date("2026-11-09T00:00:00.000Z")
};

test("captures the design 9.3 fields from the worktree", async () => {
  const worktree = await makeWorktree();
  try {
    const record = await captureContextSnapshot({
      ...common,
      worktreePath: worktree,
      git: fakeGit(),
      idFactory: () => "ctx_1"
    });

    assert.equal(record.id, "ctx_1");
    assert.equal(record.gitBaseSha, baseSha);
    assert.equal(record.architectureVersion, "1.0");
    assert.deepEqual(Object.keys(record.documentVersions), ["arch.md", "spec.md"]);
    assert.match(record.documentVersions["spec.md"]!, /^sha256:[0-9a-f]{64}$/);
    assert.deepEqual(record.selectedFiles, ["app.ts"]);
    assert.match(record.contextHash, /^[0-9a-f]{64}$/);
    assert.equal(record.createdAt, "2026-11-09T00:00:00.000Z");
  } finally {
    await rm(worktree, { recursive: true, force: true });
  }
});

test("produces a stable hash for identical inputs and a different one after a change", async () => {
  const worktree = await makeWorktree();
  try {
    const first = await captureContextSnapshot({
      ...common, worktreePath: worktree, git: fakeGit(), idFactory: () => "ctx_a"
    });
    const second = await captureContextSnapshot({
      ...common, worktreePath: worktree, git: fakeGit(), idFactory: () => "ctx_b"
    });
    assert.equal(first.contextHash, second.contextHash);

    await writeFile(join(worktree, "app.ts"), "export const value = 2;\n", "utf8");
    const changed = await captureContextSnapshot({
      ...common, worktreePath: worktree, git: fakeGit(), idFactory: () => "ctx_c"
    });
    assert.notEqual(changed.contextHash, first.contextHash);
  } finally {
    await rm(worktree, { recursive: true, force: true });
  }
});

test("changes the hash when only the git base SHA changes", async () => {
  const worktree = await makeWorktree();
  try {
    const onBase = await captureContextSnapshot({
      ...common, worktreePath: worktree, git: fakeGit(), idFactory: () => "ctx_a"
    });
    const onOther = await captureContextSnapshot({
      ...common, worktreePath: worktree, git: fakeGit("b".repeat(40)), idFactory: () => "ctx_b"
    });

    assert.notEqual(onBase.contextHash, onOther.contextHash);
    assert.equal(onOther.gitBaseSha, "b".repeat(40));
  } finally {
    await rm(worktree, { recursive: true, force: true });
  }
});

test("refuses to hash a missing document or a path outside the worktree", async () => {
  const worktree = await makeWorktree();
  try {
    await assert.rejects(
      captureContextSnapshot({ ...common, documents: ["missing.md"], worktreePath: worktree, git: fakeGit() }),
      (error: unknown) => error instanceof ContextCaptureError && /Cannot read/.test(error.message)
    );

    await assert.rejects(
      captureContextSnapshot({ ...common, documents: ["../outside.md"], worktreePath: worktree, git: fakeGit() }),
      /escapes the worktree/
    );
  } finally {
    await rm(worktree, { recursive: true, force: true });
  }
});

test("verifies a snapshot against the current worktree", async () => {
  const worktree = await makeWorktree();
  try {
    const record = await captureContextSnapshot({
      ...common, worktreePath: worktree, git: fakeGit(), idFactory: () => "ctx_1"
    });
    const unchanged = await verifyContextSnapshot(record, {
      ...common, worktreePath: worktree, git: fakeGit()
    });
    assert.equal(unchanged.matches, true);
    assert.deepEqual(unchanged.differences, []);

    await writeFile(join(worktree, "spec.md"), "# Spec\nRequirement B\n", "utf8");
    const drifted = await verifyContextSnapshot(record, {
      ...common, worktreePath: worktree, git: fakeGit()
    });
    assert.equal(drifted.matches, false);
    assert.ok(drifted.differences.includes("document changed: spec.md"));
  } finally {
    await rm(worktree, { recursive: true, force: true });
  }
});

test("stores the snapshot immutably through the record store", async () => {
  const worktree = await makeWorktree();
  const storeRoot = await mkdtemp(join(tmpdir(), "codeweaver-ctx-store-"));
  try {
    const record = await captureContextSnapshot({
      ...common, worktreePath: worktree, git: fakeGit(), idFactory: () => "ctx_1"
    });
    const store = new ImmutableRecordStore(storeRoot, "contexts", assertContextSnapshot);
    await store.put(record);

    assert.equal((await store.get("ctx_1"))?.contextHash, record.contextHash);
  } finally {
    await rm(worktree, { recursive: true, force: true });
    await rm(storeRoot, { recursive: true, force: true });
  }
});
