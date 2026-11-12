import type { EvaluationVerdict } from "../evaluation/evaluation.js";
import type { JournalEntry } from "../journal/journal.js";
import type { AgentCapability, AgentRuntimeType } from "../types/adapter.js";
import { extractTokenUsage } from "../accounting/token-usage.js";

/**
 * Runtime capability scoring (multi-model execution policy).
 *
 * `getCapabilities()` answers "what does this runtime claim?". It never answered "how did it
 * actually behave", so two runtimes could not be compared on anything but their declarations.
 * This module closes that gap: it turns recorded behaviour into one comparable number.
 *
 * The rules the number obeys come from the decision log:
 *
 *  - D-020: a refusal is not a pass. A failed or blocked execution never counts as a success,
 *    and an execution with no evaluation is "run but unproven", never a pass.
 *  - D-052: a cancellation is reported when the *runtime* confirms it, not when CodeWeaver asks.
 *    The confirmation ratio therefore counts only runtime-originated cancellations.
 *  - D-043: a runtime claim must come from the runtime. The declared capability set is an input
 *    that the caller reads from the runtime itself; the observed set is derived from records.
 *  - D-008: boundary evidence is not agent evidence. Only what the runtime actually emitted
 *    (journal entries) counts as observed capability.
 *
 * Every number below comes from a record: the journal, the usage ledger, the evaluation store or
 * the runtime's own capability declaration. Nothing here is hardcoded per runtime, and the whole
 * scoring function is pure data in / data out so it can be unit tested without a model.
 */

export const CAPABILITY_KEYS = [
  "streaming",
  "cancellation",
  "sessionResume",
  "toolEvents",
  "permissionEvents",
  "subAgents",
  "skills"
] as const;

export type CapabilityKey = (typeof CAPABILITY_KEYS)[number];

/** The terminal states an execution can end in. `unknown` means "no terminal event was recorded". */
export type RuntimeTerminalStatus = "succeeded" | "failed" | "cancelled" | "interrupted" | "unknown";

/**
 * One execution, as the records describe it.
 *
 * This is the unit the scoring aggregates. It is deliberately flat: the probe builds it from the
 * journal and the stores, the tests build it by hand, and the scorer never talks to I/O.
 */
export interface RuntimeExecutionObservation {
  executionId: string;
  /** The runtime that ran it, as recorded by the journal's `execution.runtime` fact. */
  runtimeId: string;
  terminalStatus: RuntimeTerminalStatus;
  startedAt?: string;
  finishedAt?: string;
  durationMs?: number;
  /** CodeWeaver asked this execution to stop. */
  cancelRequested: boolean;
  /** The runtime itself ended the turn (D-052). */
  cancelConfirmedByRuntime: boolean;
  /** The runtime's own account of how the turn ended, when it gave one. */
  runtimeStopReason?: string;
  toolEventCount: number;
  permissionEventCount: number;
  messageDeltaCount: number;
  messageCompletedCount: number;
  /** A second `execution.started` for the same execution id means the run was resumed. */
  resumeObserved: boolean;
  skillBound: boolean;
  subAgentObserved: boolean;
  promptTokens?: number;
  completionTokens?: number;
  totalTokens?: number;
  /** The latest evaluation verdict recorded for this execution, if it was evaluated. */
  evaluationVerdict?: EvaluationVerdict;
}

/** What the runtime declares, and what the records show. */
export interface RuntimeCapabilityEvidence {
  /** The runtime's own declaration. Read from the runtime, never from a test double (D-043). */
  declared: AgentCapability;
  /**
   * Capabilities the workload gave this runtime an opportunity to exercise.
   *
   * This is a property of the *task*, not of the runtime: a runtime cannot be credited or blamed
   * for a capability the task never asked of it. The caller supplies it from what it granted.
   */
  exercised: Partial<AgentCapability>;
  /** Capabilities the runtime actually demonstrated, derived from recorded events. */
  observed: Partial<AgentCapability>;
}

export interface RuntimeScoreInput {
  runtimeId: string;
  runtimeType: AgentRuntimeType;
  version?: string;
  capabilityEvidence: RuntimeCapabilityEvidence;
  executions: readonly RuntimeExecutionObservation[];
  /**
   * How many tokens one successful execution may cost before the cost dimension scores zero.
   *
   * Absent means "no budget was set", and the cost dimension is then unmeasured rather than
   * scored as zero: a missing target is not a failure (D-020 in reverse).
   */
  tokenBudgetPerSuccessfulExecution?: number;
}

export interface RuntimeScoreDimension {
  key: string;
  /** The dimension's nominal weight. */
  weight: number;
  /** The weight it actually carried, after unmeasured dimensions were dropped. */
  effectiveWeight: number;
  measured: boolean;
  /** 0..1, or null when the dimension had no evidence. */
  score: number | null;
  /** Why this dimension is weighted the way it is, and what was measured. */
  detail: string;
  raw: Record<string, unknown>;
}

export interface RuntimeMetrics {
  executions: number;
  succeeded: number;
  failed: number;
  cancelled: number;
  interrupted: number;
  /** Executions with no terminal event recorded yet. */
  unfinished: number;
  successRate: number | null;
  cancelRequests: number;
  cancelConfirmed: number;
  cancelConfirmationRate: number | null;
  toolEventsObserved: boolean;
  toolEventCount: number;
  executionsWithToolEvents: number;
  permissionEventsObserved: boolean;
  permissionEventCount: number;
  executionsWithPermissionEvents: number;
  messageDeltas: number;
  averageDurationMs: number | null;
  totalTokens: number;
  averageTokensPerExecution: number | null;
  tokensPerSuccessfulExecution: number | null;
  evaluations: number;
  evaluationsPassed: number;
  /** `passed / evaluated`. A `blocked` evaluation is not a pass (D-020). */
  evaluationPassRate: number | null;
  declaredCapabilities: string[];
  observedCapabilities: string[];
  /** Declared true, but the records show no evidence of it. */
  declaredNotObserved: string[];
  /** Demonstrated in the records, but never declared by the runtime. */
  observedNotDeclared: string[];
  /** Declared true, exercised by the task, and demonstrated. */
  realizedCapabilities: string[];
}

export interface RuntimeScore {
  runtimeId: string;
  runtimeType: AgentRuntimeType;
  version?: string;
  /** 0..100, or null when no dimension had any evidence at all. */
  score: number | null;
  dimensions: RuntimeScoreDimension[];
  metrics: RuntimeMetrics;
  /** Dimensions that had no evidence and therefore carried no weight. */
  unmeasuredDimensions: string[];
}

/**
 * The weights, and why each one is what it is.
 *
 * A runtime that cannot finish work is worthless, however fast or however capable it claims to
 * be, so reliability carries the largest weight. Capability realization comes next: a declaration
 * the runtime never demonstrated is a claim, not a capability (D-043). Cancellation honesty is
 * third: a runtime that does not confirm a stop leaves CodeWeaver unable to say whether the turn
 * really ended (D-052), which is a correctness problem, not a feature gap. Evaluation outcome
 * measures the strongest external signal there is — an independent command saying the work passed
 * (AT-204) — and a blocked check is not a pass (D-020). Cost is last: it matters, but it must
 * never outrank correctness.
 */
export const DIMENSION_WEIGHTS: Readonly<Record<string, number>> = {
  reliability: 0.35,
  cancellationHonesty: 0.2,
  capabilityRealization: 0.25,
  evaluationOutcome: 0.15,
  costEfficiency: 0.05
};

const TERMINAL_BY_EVENT: Readonly<Record<string, RuntimeTerminalStatus>> = {
  "execution.completed": "succeeded",
  "execution.failed": "failed",
  "execution.cancelled": "cancelled"
};

/**
 * Scores one runtime from its records.
 *
 * The score is the weighted mean of the dimensions that had evidence. A dimension without evidence
 * is dropped and its weight is redistributed over the others, so a runtime is never punished for
 * a measurement nobody took. If no dimension had evidence the score is `null` — an unproven
 * runtime is not a bad runtime, it is an unmeasured one.
 */
export function scoreRuntime(input: RuntimeScoreInput): RuntimeScore {
  const executions = input.executions;
  const succeeded = count(executions, "succeeded");
  const failed = count(executions, "failed");
  const cancelled = count(executions, "cancelled");
  const interrupted = count(executions, "interrupted");
  const unfinished = count(executions, "unknown");

  const cancelRequests = executions.filter(item => item.cancelRequested).length;
  const cancelConfirmed = executions.filter(item => item.cancelConfirmedByRuntime).length;
  const evaluations = executions.filter(item => item.evaluationVerdict !== undefined);
  const evaluationsPassed = evaluations.filter(item => item.evaluationVerdict === "passed").length;
  const totalTokens = executions.reduce((sum, item) => sum + (item.totalTokens ?? 0), 0);
  const succeededTokens = executions
    .filter(item => item.terminalStatus === "succeeded")
    .reduce((sum, item) => sum + (item.totalTokens ?? 0), 0);
  const durations = executions
    .map(item => item.durationMs)
    .filter((value): value is number => typeof value === "number" && Number.isFinite(value) && value >= 0);

  const declaredTrue = CAPABILITY_KEYS.filter(key => input.capabilityEvidence.declared[key]);
  const observedTrue = CAPABILITY_KEYS.filter(key => input.capabilityEvidence.observed[key]);
  const exercisedTrue = CAPABILITY_KEYS.filter(key => input.capabilityEvidence.exercised[key]);
  const considered = declaredTrue.filter(key => exercisedTrue.includes(key));
  const realized = considered.filter(key => observedTrue.includes(key));

  const metrics: RuntimeMetrics = {
    executions: executions.length,
    succeeded,
    failed,
    cancelled,
    interrupted,
    unfinished,
    successRate: rate(succeeded, succeeded + failed + interrupted),
    cancelRequests,
    cancelConfirmed,
    cancelConfirmationRate: rate(cancelConfirmed, cancelRequests),
    toolEventsObserved: executions.some(item => item.toolEventCount > 0),
    toolEventCount: executions.reduce((sum, item) => sum + item.toolEventCount, 0),
    executionsWithToolEvents: executions.filter(item => item.toolEventCount > 0).length,
    permissionEventsObserved: executions.some(item => item.permissionEventCount > 0),
    permissionEventCount: executions.reduce((sum, item) => sum + item.permissionEventCount, 0),
    executionsWithPermissionEvents: executions.filter(item => item.permissionEventCount > 0).length,
    messageDeltas: executions.reduce((sum, item) => sum + item.messageDeltaCount, 0),
    averageDurationMs: durations.length === 0
      ? null
      : Math.round(durations.reduce((sum, value) => sum + value, 0) / durations.length),
    totalTokens,
    averageTokensPerExecution: executions.length === 0 ? null : Math.round(totalTokens / executions.length),
    tokensPerSuccessfulExecution: succeeded === 0 ? null : Math.round(succeededTokens / succeeded),
    evaluations: evaluations.length,
    evaluationsPassed,
    evaluationPassRate: rate(evaluationsPassed, evaluations.length),
    declaredCapabilities: declaredTrue,
    observedCapabilities: observedTrue,
    declaredNotObserved: declaredTrue.filter(key => !observedTrue.includes(key)),
    observedNotDeclared: observedTrue.filter(key => !declaredTrue.includes(key)),
    realizedCapabilities: realized
  };

  const dimensions: RuntimeScoreDimension[] = [];

  const reliabilityDenominator = succeeded + failed + interrupted;
  dimensions.push({
    key: "reliability",
    weight: DIMENSION_WEIGHTS["reliability"]!,
    effectiveWeight: 0,
    measured: reliabilityDenominator > 0,
    score: rate(succeeded, reliabilityDenominator),
    detail:
      "succeeded / (succeeded + failed + interrupted). Cancelled and unfinished runs are excluded: " +
      "a stop CodeWeaver asked for is not the runtime's choice, and a run with no terminal event " +
      "is not a failure (D-020).",
    raw: { succeeded, failed, interrupted, denominator: reliabilityDenominator }
  });

  dimensions.push({
    key: "cancellationHonesty",
    weight: DIMENSION_WEIGHTS["cancellationHonesty"]!,
    effectiveWeight: 0,
    measured: cancelRequests > 0,
    score: rate(cancelConfirmed, cancelRequests),
    detail:
      "runtime-confirmed cancellations / requested cancellations. Only a cancellation the runtime " +
      "itself ended counts (D-052); a request CodeWeaver sent is not a confirmation.",
    raw: { requested: cancelRequests, confirmed: cancelConfirmed }
  });

  dimensions.push({
    key: "capabilityRealization",
    weight: DIMENSION_WEIGHTS["capabilityRealization"]!,
    effectiveWeight: 0,
    measured: considered.length > 0,
    score: rate(realized.length, considered.length),
    detail:
      "declared-and-exercised capabilities that the records show / declared-and-exercised " +
      "capabilities. A declaration the runtime never demonstrated is a claim, not a capability " +
      "(D-043); a capability the task never asked of it is not held against the runtime.",
    raw: { declared: declaredTrue, exercised: exercisedTrue, realized: realized }
  });

  dimensions.push({
    key: "evaluationOutcome",
    weight: DIMENSION_WEIGHTS["evaluationOutcome"]!,
    effectiveWeight: 0,
    measured: evaluations.length > 0,
    score: rate(evaluationsPassed, evaluations.length),
    detail:
      "passed / evaluated. A blocked check is not a pass (D-020), and an execution that was never " +
      "evaluated is 'run but unproven', so it is left out of both sides of the ratio.",
    raw: { evaluated: evaluations.length, passed: evaluationsPassed }
  });

  const budget = input.tokenBudgetPerSuccessfulExecution;
  const tokensPerSuccess = metrics.tokensPerSuccessfulExecution;
  const costMeasured =
    typeof budget === "number" &&
    Number.isFinite(budget) &&
    budget > 0 &&
    typeof tokensPerSuccess === "number" &&
    tokensPerSuccess > 0;
  dimensions.push({
    key: "costEfficiency",
    weight: DIMENSION_WEIGHTS["costEfficiency"]!,
    effectiveWeight: 0,
    measured: costMeasured,
    score: costMeasured ? clamp01(1 - tokensPerSuccess! / budget!) : null,
    detail:
      "1 - tokens per successful execution / budget. Unmeasured when no budget was set, because a " +
      "missing target is not a failure. Cost carries the smallest weight on purpose: it must never " +
      "outrank correctness.",
    raw: { tokensPerSuccessfulExecution: tokensPerSuccess ?? null, budget: budget ?? null }
  });

  const measuredWeight = dimensions
    .filter(dimension => dimension.measured)
    .reduce((sum, dimension) => sum + dimension.weight, 0);
  for (const dimension of dimensions) {
    dimension.effectiveWeight = dimension.measured && measuredWeight > 0
      ? dimension.weight / measuredWeight
      : 0;
  }

  const score = measuredWeight === 0
    ? null
    : Math.round(
        dimensions.reduce(
          (sum, dimension) =>
            sum + (dimension.measured && dimension.score !== null
              ? dimension.effectiveWeight * dimension.score
              : 0),
          0
        ) * 100
      );

  return {
    runtimeId: input.runtimeId,
    runtimeType: input.runtimeType,
    ...(input.version ? { version: input.version } : {}),
    score,
    dimensions,
    metrics,
    unmeasuredDimensions: dimensions.filter(dimension => !dimension.measured).map(dimension => dimension.key)
  };
}

export interface RuntimeComparisonRow {
  runtimeId: string;
  runtimeType: AgentRuntimeType;
  version?: string;
  score: number | null;
  successRate: number | null;
  cancelConfirmationRate: number | null;
  capabilityRealization: number | null;
  evaluationPassRate: number | null;
  averageDurationMs: number | null;
  tokensPerSuccessfulExecution: number | null;
  declaredCapabilities: string[];
  observedCapabilities: string[];
}

export interface RuntimeComparison {
  /** Highest score first; runtimes without a score come last. */
  rows: RuntimeComparisonRow[];
  /** The same comparison, rendered for a console or a report. */
  lines: string[];
}

/**
 * Compares scored runtimes side by side.
 *
 * Cost and latency are shown as columns rather than folded into the score: they depend on the
 * task, so they belong in the comparison and not in a per-runtime quality number.
 */
export function compareRuntimeScores(scores: readonly RuntimeScore[]): RuntimeComparison {
  const rows: RuntimeComparisonRow[] = scores
    .map(score => ({
      runtimeId: score.runtimeId,
      runtimeType: score.runtimeType,
      ...(score.version ? { version: score.version } : {}),
      score: score.score,
      successRate: score.metrics.successRate,
      cancelConfirmationRate: score.metrics.cancelConfirmationRate,
      capabilityRealization: dimensionValue(score, "capabilityRealization"),
      evaluationPassRate: score.metrics.evaluationPassRate,
      averageDurationMs: score.metrics.averageDurationMs,
      tokensPerSuccessfulExecution: score.metrics.tokensPerSuccessfulExecution,
      declaredCapabilities: score.metrics.declaredCapabilities,
      observedCapabilities: score.metrics.observedCapabilities
    }))
    .sort((a, b) => (b.score ?? -1) - (a.score ?? -1) || a.runtimeId.localeCompare(b.runtimeId));

  const lines: string[] = [];
  lines.push(
    "runtimeId | type | score | successRate | cancelConfirm | capabilityRealization | evalPass | avgMs | tokens/success | declared | observed"
  );
  for (const row of rows) {
    lines.push(
      [
        row.runtimeId,
        row.runtimeType,
        row.score === null ? "n/a" : String(row.score),
        formatRate(row.successRate),
        formatRate(row.cancelConfirmationRate),
        formatRate(row.capabilityRealization),
        formatRate(row.evaluationPassRate),
        row.averageDurationMs === null ? "n/a" : String(row.averageDurationMs),
        row.tokensPerSuccessfulExecution === null ? "n/a" : String(row.tokensPerSuccessfulExecution),
        row.declaredCapabilities.join(",") || "-",
        row.observedCapabilities.join(",") || "-"
      ].join(" | ")
    );
  }
  return { rows, lines };
}

/**
 * Derives one observation per execution from journal entries.
 *
 * The journal is the durable record of what the runtime emitted, so it is the primary source:
 * terminal state, tool and permission events, streaming, cancellation and token usage all come
 * out of the recorded events. The evaluation verdict is read from the journal's
 * `evaluation.completed` fact, which is written by the same service that stores the result.
 */
export function observationsFromJournal(entries: readonly JournalEntry[]): RuntimeExecutionObservation[] {
  const grouped = new Map<string, JournalEntry[]>();
  for (const entry of entries) {
    const bucket = grouped.get(entry.executionId);
    if (bucket) bucket.push(entry);
    else grouped.set(entry.executionId, [entry]);
  }
  return [...grouped.entries()]
    .map(([executionId, bucket]) => observationFromJournal(executionId, bucket))
    .sort((a, b) => a.executionId.localeCompare(b.executionId));
}

function observationFromJournal(executionId: string, entries: readonly JournalEntry[]): RuntimeExecutionObservation {
  const sorted = [...entries].sort((a, b) => a.sequence - b.sequence);
  const runtimeEvents = sorted.filter(entry => entry.kind === "runtime.event");
  const auditFacts = sorted.filter(entry => entry.kind === "audit");

  const runtimeId = auditFacts.find(entry => entry.category === "execution.runtime")?.payload["runtimeId"];
  const started = runtimeEvents.find(entry => entry.category === "execution.started");
  const terminal = [...runtimeEvents]
    .reverse()
    .find(entry => TERMINAL_BY_EVENT[entry.category] !== undefined);
  const cancelled = runtimeEvents.find(entry => entry.category === "execution.cancelled");

  // A cancellation counts as confirmed when the runtime produced the cancelled event itself.
  // Pi's adapter additionally reports `runtimeConfirmed: false` when the runtime never answered,
  // and that is exactly the case D-052 says must not be reported as a cancellation.
  const cancelConfirmedByRuntime = cancelled !== undefined && cancelled.payload["runtimeConfirmed"] !== false;
  const stopReason = cancelled?.payload["runtimeStopReason"] ?? cancelled?.payload["turn_exit_reason"];

  const startedAt = started?.recordedAt;
  const finishedAt = terminal?.recordedAt;
  const durationMs =
    startedAt && finishedAt ? Math.max(0, Date.parse(finishedAt) - Date.parse(startedAt)) : undefined;

  const usage = extractTokenUsage(terminal?.payload ?? {});
  const verdict = evaluationVerdictFromJournal(auditFacts);
  const startedCount = runtimeEvents.filter(entry => entry.category === "execution.started").length;

  const observation: RuntimeExecutionObservation = {
    executionId,
    runtimeId: typeof runtimeId === "string" && runtimeId !== "" ? runtimeId : "unknown",
    terminalStatus: terminal ? TERMINAL_BY_EVENT[terminal.category]! : "unknown",
    ...(startedAt ? { startedAt } : {}),
    ...(finishedAt ? { finishedAt } : {}),
    ...(durationMs !== undefined ? { durationMs } : {}),
    cancelRequested: auditFacts.some(entry => entry.category === "execution.cancel_requested"),
    cancelConfirmedByRuntime,
    ...(typeof stopReason === "string" && stopReason !== "" ? { runtimeStopReason: stopReason } : {}),
    toolEventCount: runtimeEvents.filter(entry => entry.category.startsWith("tool.")).length,
    permissionEventCount: runtimeEvents.filter(entry => entry.category === "permission.requested").length,
    messageDeltaCount: runtimeEvents.filter(entry => entry.category === "message.delta").length,
    messageCompletedCount: runtimeEvents.filter(entry => entry.category === "message.completed").length,
    resumeObserved: startedCount > 1,
    skillBound: auditFacts.some(entry => entry.category === "skill.bound"),
    subAgentObserved: auditFacts.some(entry => entry.category === "subagent.spawned"),
    ...(usage.promptTokens !== undefined ? { promptTokens: usage.promptTokens } : {}),
    ...(usage.completionTokens !== undefined ? { completionTokens: usage.completionTokens } : {}),
    ...(usage.totalTokens !== undefined ? { totalTokens: usage.totalTokens } : {}),
    ...(verdict ? { evaluationVerdict: verdict } : {})
  };
  return observation;
}

/**
 * Token usage out of a terminal event payload.
 *
 * The accounting ledger reads the same payload with the same reader (`accounting/token-usage.ts`).
 * Two readers is how a payload ends up accounted for in one place and not the other, and the
 * runtime that was missed was the primary one.
 */
export { extractTokenUsage };

function evaluationVerdictFromJournal(auditFacts: readonly JournalEntry[]): EvaluationVerdict | undefined {
  const fact = [...auditFacts]
    .reverse()
    .find(entry => entry.category === "evaluation.completed");
  const verdict = fact?.payload["verdict"];
  return verdict === "passed" || verdict === "failed" || verdict === "blocked" ? verdict : undefined;
}

function dimensionValue(score: RuntimeScore, key: string): number | null {
  return score.dimensions.find(dimension => dimension.key === key)?.score ?? null;
}

function count(executions: readonly RuntimeExecutionObservation[], status: RuntimeTerminalStatus): number {
  return executions.filter(item => item.terminalStatus === status).length;
}

function rate(numerator: number, denominator: number): number | null {
  if (denominator <= 0) return null;
  return Math.round((numerator / denominator) * 1000) / 1000;
}

function clamp01(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return Math.min(1, Math.max(0, value));
}

function formatRate(value: number | null): string {
  return value === null ? "n/a" : value.toFixed(3);
}
