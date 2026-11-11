import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  InMemoryJournalStore,
  JsonlJournalStore,
  JournalConflictError,
  detectGaps,
  mergeCompensated,
  type JournalEntry
} from "../../journal/journal.js";

function entry(sequence: number, overrides: Partial<JournalEntry> = {}): JournalEntry {
  return {
    executionId: "exec-1",
    sequence,
    kind: "runtime.event",
    category: "message.delta",
    recordedAt: "2026-11-09T00:00:00.000Z",
    payload: { text: `chunk-${sequence}` },
    ...overrides
  };
}

test("appends entries and reads them in sequence order", async () => {
  const store = new InMemoryJournalStore();
  await store.append(entry(2));
  await store.append(entry(1));

  const read = await store.read("exec-1");
  assert.deepEqual(read.map(item => item.sequence), [1, 2]);
  assert.deepEqual(await store.listExecutions(), ["exec-1"]);
});

test("rejects a duplicate sequence instead of overwriting history", async () => {
  const store = new InMemoryJournalStore();
  await store.append(entry(1));

  await assert.rejects(store.append(entry(1)), (error: unknown) => {
    assert.ok(error instanceof JournalConflictError);
    assert.match(error.message, /already exists/);
    return true;
  });
  assert.equal((await store.read("exec-1")).length, 1);
});

test("rejects malformed entries before they reach the journal", async () => {
  const store = new InMemoryJournalStore();

  await assert.rejects(store.append(entry(0)), /positive integer/);
  await assert.rejects(store.append(entry(1, { executionId: " " })), /executionId/);
  await assert.rejects(store.append(entry(1, { category: "" })), /category/);
  await assert.rejects(store.append(entry(1, { recordedAt: "not-a-date" })), /not a timestamp/);
  await assert.rejects(
    store.append(entry(1, { kind: "bogus" as JournalEntry["kind"] })),
    /Unknown journal entry kind/
  );
});

test("detects missing sequences without inventing them", () => {
  assert.deepEqual(detectGaps([entry(1), entry(2)]), []);
  assert.deepEqual(detectGaps([entry(1), entry(4)]), [2, 3]);
  assert.deepEqual(detectGaps([]), []);
});

test("merges compensated events idempotently and refuses conflicting duplicates", () => {
  const existing = [entry(1), entry(3)];
  const incoming = [entry(2), entry(3)];

  const merged = mergeCompensated(existing, incoming);
  assert.deepEqual(merged.map(item => item.sequence), [1, 2, 3]);

  assert.throws(
    () => mergeCompensated(existing, [entry(1, { category: "tool.started" })]),
    JournalConflictError
  );
});

test("persists across store instances on disk", async () => {
  const root = await mkdtemp(join(tmpdir(), "codeweaver-journal-"));
  try {
    const writer = new JsonlJournalStore(root);
    await writer.append(entry(1));
    await writer.append(entry(2, { kind: "audit", category: "change.applied", payload: { changeRecordId: "chg_1" } }));

    const reader = new JsonlJournalStore(root);
    const read = await reader.read("exec-1");
    assert.deepEqual(read.map(item => item.sequence), [1, 2]);
    assert.equal(read[1]!.kind, "audit");
    assert.deepEqual(await reader.listExecutions(), ["exec-1"]);

    await assert.rejects(reader.append(entry(2)), JournalConflictError);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("reports a corrupted journal line instead of dropping it", async () => {
  const root = await mkdtemp(join(tmpdir(), "codeweaver-journal-"));
  try {
    await writeFile(join(root, "exec-1.jsonl"), `${JSON.stringify(entry(1))}\n{not json}\n`, "utf8");
    const store = new JsonlJournalStore(root);

    await assert.rejects(store.read("exec-1"), /malformed JSON at line 2/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("refuses an execution id that cannot be a safe file name", async () => {
  const store = new JsonlJournalStore("unused");

  await assert.rejects(store.read("../escape"), /not safe as a file name/);
});
