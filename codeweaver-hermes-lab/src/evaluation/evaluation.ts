import { randomUUID } from "node:crypto";
import { SpawnCommandRunner, type CommandRunner } from "./command-runner.js";

/**
 * Evaluation Engine (design 14.1 EVALUATION_FAILED / AT-204).
 *
 * A change is only "done" when an independent check passed. No checks means no evidence, so an empty
 * check list is `blocked`, never `passed`.
 */
export type EvaluationVerdict = "passed" | "failed" | "blocked";

export interface EvaluationCheckSpec {
  id: string;
  name: string;
  command: string;
  args: string[];
  cwd?: string;
  timeoutMs?: number;
  expectExitCode?: number;
}

export interface EvaluationCheckResult {
  id: string;
  name: string;
  verdict: EvaluationVerdict;
  exitCode: number | null;
  durationMs: number;
  output: string;
}

export interface EvaluationResult {
  id: string;
  executionId: string;
  changeRecordId?: string;
  commitSha?: string;
  checks: EvaluationCheckResult[];
  verdict: EvaluationVerdict;
  startedAt: string;
  finishedAt: string;
  summary: string;
}

export interface EvaluateOptions {
  changeRecordId?: string;
  commitSha?: string;
  now?: () => Date;
  idFactory?: () => string;
}

export async function evaluateChecks(
  executionId: string,
  checks: readonly EvaluationCheckSpec[],
  runner: CommandRunner = new SpawnCommandRunner(),
  options: EvaluateOptions = {}
): Promise<EvaluationResult> {
  const now = options.now ?? (() => new Date());
  const startedAt = now().toISOString();
  const results: EvaluationCheckResult[] = [];

  for (const check of checks) {
    const checkStarted = Date.now();
    const expected = check.expectExitCode ?? 0;
    const outcome = await runner.run(check.command, check.args, {
      ...(check.cwd ? { cwd: check.cwd } : {}),
      ...(check.timeoutMs ? { timeoutMs: check.timeoutMs } : {})
    });
    const verdict: EvaluationVerdict = outcome.timedOut || outcome.code === null
      ? "blocked"
      : outcome.code === expected
        ? "passed"
        : "failed";
    results.push({
      id: check.id,
      name: check.name,
      verdict,
      exitCode: outcome.code,
      durationMs: Date.now() - checkStarted,
      output: truncate(`${outcome.stdout}\n${outcome.stderr}`)
    });
  }

  const verdict = overallVerdict(results);
  return {
    id: (options.idFactory ?? (() => `eval_${randomUUID()}`))(),
    executionId,
    ...(options.changeRecordId ? { changeRecordId: options.changeRecordId } : {}),
    ...(options.commitSha ? { commitSha: options.commitSha } : {}),
    checks: results,
    verdict,
    startedAt,
    finishedAt: now().toISOString(),
    summary: summarize(results, verdict)
  };
}

export interface CompletionGate {
  allowed: boolean;
  reason: string;
}

/**
 * AT-204: a failed (or unproven) evaluation must not let the caller mark the task complete.
 * Callers that ignore this gate are the bug, so the reason always names what is missing.
 */
export function completionGate(result: EvaluationResult | undefined): CompletionGate {
  if (!result) return { allowed: false, reason: "No evaluation result exists for this change" };
  if (result.checks.length === 0) {
    return { allowed: false, reason: "Evaluation produced no checks; absence of evidence is not success" };
  }
  if (result.verdict === "passed") {
    return { allowed: true, reason: `All ${result.checks.length} check(s) passed` };
  }
  if (result.verdict === "blocked") {
    return { allowed: false, reason: `A check could not run: ${result.summary}` };
  }
  return { allowed: false, reason: `A check failed: ${result.summary}` };
}

function overallVerdict(results: readonly EvaluationCheckResult[]): EvaluationVerdict {
  if (results.length === 0) return "blocked";
  if (results.some(check => check.verdict === "blocked")) return "blocked";
  if (results.some(check => check.verdict === "failed")) return "failed";
  return "passed";
}

function summarize(results: readonly EvaluationCheckResult[], verdict: EvaluationVerdict): string {
  if (results.length === 0) return "no checks were configured";
  const failed = results.filter(check => check.verdict === "failed").map(check => check.name);
  const blocked = results.filter(check => check.verdict === "blocked").map(check => check.name);
  if (verdict === "failed") return `failed: ${failed.join(", ")}`;
  if (verdict === "blocked") return `blocked: ${blocked.join(", ")}`;
  return `${results.length}/${results.length} checks passed`;
}

function truncate(value: string, limit = 1_000): string {
  const trimmed = value.trim();
  return trimmed.length <= limit ? trimmed : `${trimmed.slice(0, limit)}...`;
}
