import type { EvaluationResult } from "../evaluation/evaluation.js";
import type { SkillBinding } from "./skill-bindings.js";

/**
 * Skill effectiveness (design 10.2 "Skill 执行效果评估").
 *
 * Evidence is joined the only honest way available: an execution is bound to an exact skill version,
 * and an evaluation belongs to an execution. An execution with no evaluation is counted as "run but
 * unproven", never as a success.
 */
export interface SkillEffectiveness {
  skillId: string;
  skillVersion: string;
  executions: number;
  evaluated: number;
  passed: number;
  failed: number;
  blocked: number;
  /** Passed / evaluated, or null when nothing was evaluated. */
  passRate: number | null;
  lastEvaluatedAt?: string;
}

export function summarizeSkillEffectiveness(
  bindings: readonly SkillBinding[],
  evaluations: readonly EvaluationResult[]
): SkillEffectiveness[] {
  // Latest evaluation per execution wins; earlier attempts are history, not additional evidence.
  const latestByExecution = new Map<string, EvaluationResult>();
  for (const evaluation of evaluations) {
    const current = latestByExecution.get(evaluation.executionId);
    if (!current || evaluation.finishedAt >= current.finishedAt) {
      latestByExecution.set(evaluation.executionId, evaluation);
    }
  }

  const bySkill = new Map<string, SkillEffectiveness>();
  for (const binding of bindings) {
    const key = `${binding.skillId}@${binding.skillVersion}`;
    const entry = bySkill.get(key) ?? {
      skillId: binding.skillId,
      skillVersion: binding.skillVersion,
      executions: 0,
      evaluated: 0,
      passed: 0,
      failed: 0,
      blocked: 0,
      passRate: null
    };
    entry.executions += 1;

    const evaluation = latestByExecution.get(binding.executionId);
    if (evaluation) {
      entry.evaluated += 1;
      if (evaluation.verdict === "passed") entry.passed += 1;
      if (evaluation.verdict === "failed") entry.failed += 1;
      if (evaluation.verdict === "blocked") entry.blocked += 1;
      if (!entry.lastEvaluatedAt || evaluation.finishedAt > entry.lastEvaluatedAt) {
        entry.lastEvaluatedAt = evaluation.finishedAt;
      }
    }
    entry.passRate = entry.evaluated === 0 ? null : round(entry.passed / entry.evaluated);
    bySkill.set(key, entry);
  }

  return [...bySkill.values()].sort(
    (a, b) => b.executions - a.executions || a.skillId.localeCompare(b.skillId)
      || a.skillVersion.localeCompare(b.skillVersion)
  );
}

function round(value: number): number {
  return Math.round(value * 1000) / 1000;
}
