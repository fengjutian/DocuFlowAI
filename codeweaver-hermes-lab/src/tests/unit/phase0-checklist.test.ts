import test from "node:test";
import assert from "node:assert/strict";
import { mergePhase0Results, phase0Checks } from "../../reporting/phase0-checklist.js";
import type { TestResult } from "../../types/hermes.js";

function result(id: string, status: TestResult["status"], notes: string): TestResult {
  return {
    id,
    title: "input title",
    status,
    startedAt: "2026-11-09T00:00:00.000Z",
    finishedAt: "2026-11-09T00:00:01.000Z",
    evidence: [],
    notes
  };
}

test("fills every missing Phase 0 check as not_run", () => {
  const merged = mergePhase0Results([result("P0-001", "passed", "verified")], new Date("2026-11-09T00:00:00.000Z"));

  assert.equal(merged.length, 24);
  assert.equal(phase0Checks.length, 24);
  assert.equal(merged[0]!.status, "passed");
  assert.equal(merged[1]!.status, "not_run");
  assert.deepEqual(merged.map(item => item.id), phase0Checks.map(([id]) => id));
});

test("uses the last supplied result when reports overlap", () => {
  const merged = mergePhase0Results([
    result("P0-003", "failed", "old"),
    result("P0-003", "passed", "new")
  ]);

  assert.equal(merged[2]!.status, "passed");
  assert.equal(merged[2]!.notes, "new");
});

test("rejects results outside the Phase 0 checklist", () => {
  assert.throws(() => mergePhase0Results([result("P0-999", "passed", "unknown")]), /Unknown/);
});
