import test from "node:test";
import assert from "node:assert/strict";
import { UsageLedger } from "../../accounting/usage.js";
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

test("extracts nested usage and derives the total", () => {
  const ledger = new UsageLedger();
  const record = ledger.recordFromEvent(
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

test("accepts flat and alternative token field names", () => {
  const ledger = new UsageLedger();

  const flat = ledger.recordFromEvent(event({ input_tokens: 7, output_tokens: 3, total_tokens: 10 }), {
    durationMs: 1000
  });
  assert.equal(flat?.totalTokens, 10);

  const aliased = ledger.recordFromEvent(event({ usage: { tokens_in: 5, tokens_out: 2, model: "m2" } }), {
    durationMs: 500
  });
  assert.equal(aliased?.model, "m2");
  assert.equal(aliased?.totalTokens, 7);
});

test("records nothing when the runtime reported no usage", () => {
  const ledger = new UsageLedger();
  const record = ledger.recordFromEvent(event({ output: "done" }), { durationMs: 1000 });

  assert.equal(record, undefined);
  assert.equal(ledger.list().length, 0);
});

test("aggregates totals and groups by model", () => {
  const ledger = new UsageLedger();
  ledger.record({
    id: "u1",
    executionId: "exec-1",
    model: "a",
    promptTokens: 10,
    completionTokens: 5,
    totalTokens: 15,
    durationMs: 100
  });
  ledger.record({
    id: "u2",
    executionId: "exec-2",
    model: "b",
    promptTokens: 20,
    completionTokens: 10,
    totalTokens: 30,
    durationMs: 200
  });
  ledger.record({
    id: "u3",
    executionId: "exec-3",
    model: "a",
    promptTokens: 5,
    completionTokens: 5,
    totalTokens: 10,
    durationMs: 50
  });

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

test("rejects negative token counts instead of recording nonsense", () => {
  const ledger = new UsageLedger();

  assert.throws(() => ledger.record({
    id: "bad",
    executionId: "exec-1",
    model: "a",
    promptTokens: -1,
    completionTokens: 0,
    totalTokens: -1,
    durationMs: 10
  }), /non-negative/);
});
