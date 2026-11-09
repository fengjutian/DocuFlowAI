import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { ImmutableRecordStore, RecordConflictError } from "../../persistence/record-store.js";
import { assertContextSnapshot, type ContextSnapshotRecord } from "../../persistence/records.js";

function contextSnapshot(overrides: Partial<ContextSnapshotRecord> = {}): ContextSnapshotRecord {
  return {
    id: "ctx-1",
    projectId: "proj-1",
    taskId: "task-1",
    gitBaseSha: "a".repeat(40),
    documentVersions: { "spec.md": "3" },
    architectureVersion: "1.0",
    selectedFiles: ["src/index.ts"],
    contextHash: "b".repeat(64),
    createdAt: "2026-11-09T00:00:00.000Z",
    ...overrides
  };
}

test("persists records across store instances", async () => {
  const root = await mkdtemp(join(tmpdir(), "docuflow-records-"));
  try {
    const writer = new ImmutableRecordStore<ContextSnapshotRecord>(root, "contexts", assertContextSnapshot);
    await writer.put(contextSnapshot());
    await writer.put(contextSnapshot({ id: "ctx-2", taskId: "task-2" }));

    const reader = new ImmutableRecordStore<ContextSnapshotRecord>(root, "contexts");
    assert.equal((await reader.get("ctx-1"))?.taskId, "task-1");
    assert.deepEqual((await reader.list()).map(record => record.id), ["ctx-1", "ctx-2"]);
    assert.equal(await reader.get("missing"), undefined);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("refuses to overwrite an existing record", async () => {
  const root = await mkdtemp(join(tmpdir(), "docuflow-records-"));
  try {
    const store = new ImmutableRecordStore<ContextSnapshotRecord>(root, "contexts", assertContextSnapshot);
    await store.put(contextSnapshot());

    await assert.rejects(store.put(contextSnapshot({ taskId: "task-2" })), (error: unknown) => {
      assert.ok(error instanceof RecordConflictError);
      assert.match(error.message, /immutable/);
      return true;
    });
    assert.equal((await store.get("ctx-1"))?.taskId, "task-1");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("rejects ids that cannot be file names before touching the disk", async () => {
  const store = new ImmutableRecordStore<ContextSnapshotRecord>("unused", "contexts");

  await assert.rejects(store.put(contextSnapshot({ id: "../escape" })), /not safe as a file name/);
  await assert.rejects(store.get("a/b"), /not safe as a file name/);
});

test("runs the entity validator so malformed evidence never lands", async () => {
  const root = await mkdtemp(join(tmpdir(), "docuflow-records-"));
  try {
    const store = new ImmutableRecordStore<ContextSnapshotRecord>(root, "contexts", assertContextSnapshot);

    await assert.rejects(store.put(contextSnapshot({ gitBaseSha: "not-a-sha" })), /gitBaseSha/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("returns an empty list for a collection that was never written", async () => {
  const root = await mkdtemp(join(tmpdir(), "docuflow-records-"));
  try {
    const store = new ImmutableRecordStore<ContextSnapshotRecord>(root, "missing");
    assert.deepEqual(await store.list(), []);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
