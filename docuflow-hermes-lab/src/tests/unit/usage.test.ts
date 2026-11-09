import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { UsageLedger, type UsageRecord } from "../../accounting/usage.js";
import { ImmutableRecordStore } from "../../persistence/record-store.js";
import type { AgentRuntimeEvent } from "../../types/adapter.js";

function event(payload: Record<string, unknown>): AgentRuntimeEvent {
  return {
    eventId: "exec-1:5",
    executionId: "exec-1",
    projectId: "proj-1",
    taskId: "task-1",
    sequence: 5,
    eventType: "execution.completed",
    timestamp: "2026-11-09T00:00:10.000Z",
    payload
  };
}

function usageRecord(overrides: Partial<UsageRecord> = {}): UsageRecord {
  return {
    id: "use_1",
    executionId: "exec-1",
    model: "a",
    promptTokens: 10,
    completionTokens: 5,
    totalTokens: 15,
    durationMs: 100,
    ...overrides
  };
}

test("extracts nested usage and derives the total", async () => {
  const ledger = new UsageLedger();
  const record = await ledger.recordFromEvent(
    event({ model: "hermes-small", usage: { prompt_tokens: 100, completion_tokens: 40 } }),
    { durationMs: 10_000, startedAt: "2026-11-09T00:00:00.000Z" }
  );

  assert.ok(record);
  assert.equal(record.model, "hermes-small");
  assert.equal(record.promptTokens, 100);
  assert.equal(record.completionTokens, 40);
  assert.equal(record.totalTokens, 140);
  assert.equal(record.durationMs, 10_000);
});

test("accepts flat and alternative token field names", async () => {
  const ledger = new UsageLedger();

  const flat = await ledger.recordFromEvent(event({ input_tokens: 7, output_tokens: 3, total_tokens: 10 }), {
    durationMs: 1000
  });
  assert.equal(flat?.totalTokens, 10);

  const aliased = await ledger.recordFromEvent(event({ usage: { tokens_in: 5, tokens_out: 2, model: "m2" } }), {
    durationMs: 500
  });
  assert.equal(aliased?.model, "m2");
  assert.equal(aliased?.totalTokens, 7);
});

test("records nothing when the runtime reported no usage", async () => {
  const ledger = new UsageLedger();
  const record = await ledger.recordFromEvent(event({ output: "done" }), { durationMs: 1000 });

  assert.equal(record, undefined);
  assert.equal(ledger.list().length, 0);
});

test("aggregates totals and groups by model", async () => {
  const ledger = new UsageLedger();
  await ledger.record(usageRecord({ id: "u1", executionId: "exec-1", model: "a", totalTokens: 15 }));
  await ledger.record(usageRecord({ id: "u2", executionId: "exec-2", model: "b", totalTokens: 30, durationMs: 200 }));
  await ledger.record(usageRecord({ id: "u3", executionId: "exec-3", model: "a", totalTokens: 10, durationMs: 50 }));

  const summary = ledger.summary();
  assert.equal(summary.executions, 3);
  assert.equal(summary.totalTokens, 55);
  assert.equal(summary.durationMs, 350);
  assert.deepEqual(summary.byModel, [
    { model: "b", executions: 1, totalTokens: 30 },
    { model: "a", executions: 2, totalTokens: 25 }
  ]);
  assert.equal(ledger.forExecution("exec-2")?.totalTokens, 30);
});

test("rejects negative token counts instead of recording nonsense", async () => {
  const ledger = new UsageLedger();

  await assert.rejects(ledger.record(usageRecord({
    id: "bad",
    promptTokens: -1,
    completionTokens: 0,
    totalTokens: -1
  })), /non-negative/);
});

test("restores usage from the store after a restart", async () => {
  const root = await mkdtemp(join(tmpdir(), "docuflow-usage-"));
  try {
    const store = new ImmutableRecordStore<UsageRecord>(root, "usage");
    const first = new UsageLedger(store);
    await first.recordFromEvent(event({ model: "m1", usage: { prompt_tokens: 4, completion_tokens: 6 } }), {
      durationMs: 1234
    });

    const second = new UsageLedger(store);
    await second.load();
    const restored = second.forExecution("exec-1");
    assert.equal(restored?.totalTokens, 10);
    assert.equal(restored?.durationMs, 1234);
    assert.equal(second.summary().totalTokens, 10);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("does not write a second record for the same execution", async () => {
  const root = await mkdtemp(join(tmpdir(), "docuflow-usage-"));
  try {
    const store = new ImmutableRecordStore<UsageRecord>(root, "usage");
    const ledger = new UsageLedger(store);
    await ledger.recordFromEvent(event({ usage: { total_tokens: 5 } }), { durationMs: 10 });
    await ledger.recordFromEvent(event({ usage: { total_tokens: 9 } }), { durationMs: 20 });

    assert.equal((await store.list()).length, 1);
    assert.equal(ledger.forExecution("exec-1")?.totalTokens, 9);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
