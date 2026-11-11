import test from "node:test";
import assert from "node:assert/strict";
import { summarizeSkillEffectiveness } from "../../skills/skill-effectiveness.js";
import type { SkillBinding } from "../../skills/skill-bindings.js";
import type { EvaluationResult } from "../../evaluation/evaluation.js";

function binding(executionId: string, version = "1.0.0"): SkillBinding {
  return {
    id: `bind-${executionId}`,
    executionId,
    skillId: "react-dev",
    skillVersion: version,
    policyId: "pol-1",
    enabled: true,
    createdAt: "2026-11-09T00:00:00.000Z"
  };
}

function evaluation(
  executionId: string,
  verdict: EvaluationResult["verdict"],
  finishedAt = "2026-11-09T01:00:00.000Z"
): EvaluationResult {
  return {
    id: `eval-${executionId}-${finishedAt}`,
    executionId,
    checks: [],
    verdict,
    startedAt: "2026-11-09T00:00:00.000Z",
    finishedAt,
    summary: `${verdict} summary`
  };
}

test("aggregates verdicts per skill version", () => {
  const summary = summarizeSkillEffectiveness(
    [binding("exec-1"), binding("exec-2"), binding("exec-3"), binding("exec-4", "2.0.0")],
    [
      evaluation("exec-1", "passed"),
      evaluation("exec-2", "failed"),
      evaluation("exec-3", "blocked"),
      evaluation("exec-4", "passed")
    ]
  );

  const v1 = summary.find(item => item.skillVersion === "1.0.0")!;
  assert.equal(v1.executions, 3);
  assert.equal(v1.evaluated, 3);
  assert.equal(v1.passed, 1);
  assert.equal(v1.failed, 1);
  assert.equal(v1.blocked, 1);
  assert.equal(v1.passRate, 0.333);
  assert.equal(v1.lastEvaluatedAt, "2026-11-09T01:00:00.000Z");

  const v2 = summary.find(item => item.skillVersion === "2.0.0")!;
  assert.equal(v2.passRate, 1);
});

test("counts an execution with no evaluation as unproven, not successful", () => {
  const [entry] = summarizeSkillEffectiveness([binding("exec-1")], []);

  assert.equal(entry!.executions, 1);
  assert.equal(entry!.evaluated, 0);
  assert.equal(entry!.passRate, null);
  assert.equal(entry!.lastEvaluatedAt, undefined);
});

test("uses only the latest evaluation per execution", () => {
  const [entry] = summarizeSkillEffectiveness([binding("exec-1")], [
    evaluation("exec-1", "failed", "2026-11-09T01:00:00.000Z"),
    evaluation("exec-1", "passed", "2026-11-09T02:00:00.000Z")
  ]);

  assert.equal(entry!.evaluated, 1);
  assert.equal(entry!.passed, 1);
  assert.equal(entry!.failed, 0);
  assert.equal(entry!.passRate, 1);
});

test("returns nothing when no skills were bound", () => {
  assert.deepEqual(summarizeSkillEffectiveness([], [evaluation("exec-1", "passed")]), []);
});
