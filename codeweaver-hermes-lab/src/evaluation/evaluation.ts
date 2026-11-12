import { randomUUID } from "node:crypto";
import type { CommandRunner } from "./command-runner.js";
import { approvalCovers, classifyCommand, type CommandApproval } from "./command-policy.js";
import { SandboxCommandRunner } from "./sandbox-runner.js";
import type { SandboxLimits } from "./sandbox-limits.js";

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
  /**
   * A human decision about this exact command.
   *
   * A dangerous command runs only with one: the approval carries the fingerprint of the command it
   * was given for, so it cannot be moved to another command.
   */
  approval?: CommandApproval;
  /**
   * The resource limits this check runs under.
   *
   * A check is a real process on the user's machine, so it runs inside a Windows job object with
   * a memory, CPU and process limit. The task may ask for tighter or looser limits; the ones it
   * does not name keep the default.
   */
  limits?: SandboxLimits;
}

export interface EvaluationCheckResult {
  id: string;
  name: string;
  verdict: EvaluationVerdict;
  exitCode: number | null;
  durationMs: number;
  output: string;
  /** Set when the check was refused before it ran. */
  refusal?: { code: "dangerous_without_approval" | "approval_mismatch"; reasons: string[] };
  /** The approval that let the command run, when one did. */
  approval?: CommandApproval;
  /** Which resource limit stopped the check, when one did. */
  limitHit?: "memory" | "processes" | "cpu" | "timeout";
  /**
   * The sandbox the check ran under.
   *
   * A check that ran without limits and a check that ran inside them look the same from the exit
   * code alone, so the record says which one it was.
   */
  sandbox?: {
    established: boolean;
    memoryMb: number;
    cpuRatePercent: number;
    processLimit: number;
    peakJobMemoryBytes?: number;
  };
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
  runner: CommandRunner = new SandboxCommandRunner(),
  options: EvaluateOptions = {}
): Promise<EvaluationResult> {
  const now = options.now ?? (() => new Date());
  const startedAt = now().toISOString();
  const results: EvaluationCheckResult[] = [];

  for (const check of checks) {
    const checkStarted = Date.now();
    const expected = check.expectExitCode ?? 0;

    // A dangerous command does not run because it was written down. It runs because a human
    // approved that exact command — and the approval is bound to it by its fingerprint.
    const classification = classifyCommand(check.command, check.args);
    if (classification.dangerous && !approvalCovers(check.approval, check.command, check.args)) {
      const mismatch = check.approval !== undefined;
      const code = mismatch ? "approval_mismatch" : "dangerous_without_approval";
      const explanation = mismatch
        ? `the approval was given for a different command (approved ${check.approval!.commandFingerprint.slice(0, 12)}, this command is ${classification.fingerprint.slice(0, 12)})`
        : `no approval was given for it (${classification.reasons.join("; ")})`;
      results.push({
        id: check.id,
        name: check.name,
        verdict: "blocked",
        exitCode: null,
        durationMs: Date.now() - checkStarted,
        output:
          `refused: '${[check.command, ...check.args].join(" ")}' is classified as dangerous and ${explanation}. ` +
          `The command was not run.`,
        refusal: { code, reasons: classification.reasons },
        ...(check.approval ? { approval: check.approval } : {})
      });
      continue;
    }

    const outcome = await runner.run(check.command, check.args, {
      ...(check.cwd ? { cwd: check.cwd } : {}),
      ...(check.timeoutMs ? { timeoutMs: check.timeoutMs } : {}),
      ...(check.limits ? { limits: check.limits } : {})
    });
    const verdict: EvaluationVerdict = outcome.timedOut || outcome.code === null
      ? "blocked"
      : outcome.code === expected
        ? "passed"
        : "failed";
    // A check that was stopped by a resource limit failed, and the reason says which limit: a
    // check that died because it ran out of memory is a different fact from a check that failed,
    // and only one of them tells the agent what to change.
    const limitHit = outcome.sandbox?.limitHit;
    const limitNote = limitHit
      ? `\n[sandbox] ${outcome.sandbox?.reason ?? "the check exceeded a resource limit"}`
      : outcome.sandbox && !outcome.sandbox.established
        ? `\n[sandbox] the check did not run inside a sandbox: ${outcome.sandbox.error ?? "the sandbox was unavailable"}`
        : "";
    results.push({
      id: check.id,
      name: check.name,
      verdict,
      exitCode: outcome.code,
      durationMs: Date.now() - checkStarted,
      output: truncate(`${outcome.stdout}\n${outcome.stderr}${limitNote}`),
      ...(limitHit ? { limitHit } : {}),
      ...(outcome.sandbox
        ? {
            sandbox: {
              established: outcome.sandbox.established,
              memoryMb: outcome.sandbox.limits.memoryMb,
              cpuRatePercent: outcome.sandbox.limits.cpuRatePercent,
              processLimit: outcome.sandbox.limits.processLimit,
              ...(outcome.sandbox.peakJobMemoryBytes !== undefined
                ? { peakJobMemoryBytes: outcome.sandbox.peakJobMemoryBytes }
                : {})
            }
          }
        : {}),
      ...(check.approval ? { approval: check.approval } : {})
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
