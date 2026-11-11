import test from "node:test";
import assert from "node:assert/strict";
import {
  CAPABILITY_KEYS,
  compareRuntimeScores,
  extractTokenUsage,
  observationsFromJournal,
  scoreRuntime,
  type RuntimeCapabilityEvidence,
  type RuntimeExecutionObservation
} from "../../adapters/runtime-scoring.js";
import type { JournalEntry } from "../../journal/journal.js";
import { NO_CAPABILITIES, type AgentCapability } from "../../types/adapter.js";

/**
 * The scoring algorithm is pure data in / data out, so every case here is a hand-built record.
 * No model, no network, no runtime: a test that needed a runtime would be testing the runtime.
 */

const NO_CAPABILITIES_PARTIAL: Partial<AgentCapability> = {};

function observation(overrides: Partial<RuntimeExecutionObservation> = {}): RuntimeExecutionObservation {
  return {
    executionId: "exec-1",
    runtimeId: "rt",
    terminalStatus: "succeeded",
    startedAt: "2026-11-11T00:00:00.000Z",
    finishedAt: "2026-11-11T00:00:10.000Z",
    durationMs: 10_000,
    cancelRequested: false,
    cancelConfirmedByRuntime: false,
    toolEventCount: 0,
    permissionEventCount: 0,
    messageDeltaCount: 0,
    messageCompletedCount: 1,
    resumeObserved: false,
    skillBound: false,
    subAgentObserved: false,
    ...overrides
  };
}

function evidence(overrides: Partial<RuntimeCapabilityEvidence> = {}): RuntimeCapabilityEvidence {
  return {
    declared: { ...NO_CAPABILITIES },
    exercised: { ...NO_CAPABILITIES_PARTIAL },
    observed: { ...NO_CAPABILITIES_PARTIAL },
    ...overrides
  };
}

function score(
  executions: readonly RuntimeExecutionObservation[],
  capabilityEvidence: RuntimeCapabilityEvidence = evidence(),
  extra: { tokenBudgetPerSuccessfulExecution?: number } = {}
) {
  return scoreRuntime({
    runtimeId: "rt",
    runtimeType: "native",
    capabilityEvidence,
    executions,
    ...extra
  });
}

test("an execution with no records scores nothing rather than zero", () => {
  const result = score([]);

  assert.equal(result.score, null);
  assert.deepEqual(result.unmeasuredDimensions.sort(), [
    "cancellationHonesty",
    "capabilityRealization",
    "costEfficiency",
    "evaluationOutcome",
    "reliability"
  ]);
  assert.equal(result.metrics.executions, 0);
  assert.equal(result.metrics.successRate, null);
});

test("reliability counts only completed outcomes and treats failures as failures", () => {
  const result = score([
    observation({ executionId: "a", terminalStatus: "succeeded" }),
    observation({ executionId: "b", terminalStatus: "succeeded" }),
    observation({ executionId: "c", terminalStatus: "failed" })
  ]);

  assert.equal(result.metrics.succeeded, 2);
  assert.equal(result.metrics.failed, 1);
  assert.equal(result.metrics.successRate, 0.667);
  const reliability = result.dimensions.find(dimension => dimension.key === "reliability");
  assert.equal(reliability?.score, 0.667);
  assert.equal(reliability?.measured, true);
});

test("cancelled and unfinished runs stay out of the reliability denominator", () => {
  const result = score([
    observation({ executionId: "a", terminalStatus: "succeeded" }),
    observation({ executionId: "b", terminalStatus: "cancelled", cancelRequested: true }),
    observation({ executionId: "c", terminalStatus: "unknown" })
  ]);

  // One success out of one decided run: a stop we asked for is not the runtime's choice.
  assert.equal(result.metrics.successRate, 1);
  assert.equal(result.metrics.cancelled, 1);
  assert.equal(result.metrics.unfinished, 1);
});

test("only a runtime-confirmed cancellation counts as a cancellation (D-052)", () => {
  const confirmed = score([
    observation({ executionId: "a", cancelRequested: true, cancelConfirmedByRuntime: true }),
    observation({ executionId: "b", cancelRequested: true, cancelConfirmedByRuntime: false })
  ]);

  assert.equal(confirmed.metrics.cancelRequests, 2);
  assert.equal(confirmed.metrics.cancelConfirmed, 1);
  assert.equal(confirmed.metrics.cancelConfirmationRate, 0.5);
  const dimension = confirmed.dimensions.find(item => item.key === "cancellationHonesty");
  assert.equal(dimension?.score, 0.5);
  assert.equal(dimension?.measured, true);
});

test("a runtime that was never asked to stop has no cancellation score, not a zero", () => {
  const result = score([observation({ terminalStatus: "succeeded" })]);

  const dimension = result.dimensions.find(item => item.key === "cancellationHonesty");
  assert.equal(dimension?.measured, false);
  assert.equal(dimension?.score, null);
  assert.ok(result.unmeasuredDimensions.includes("cancellationHonesty"));
});

test("a capability the task never exercised is not held against the runtime", () => {
  const result = score(
    [observation()],
    evidence({
      declared: { ...NO_CAPABILITIES, streaming: true, toolEvents: true },
      // The task granted no tools, so toolEvents had no chance to show up.
      exercised: { streaming: true },
      observed: { streaming: true }
    })
  );

  const dimension = result.dimensions.find(item => item.key === "capabilityRealization");
  assert.equal(dimension?.measured, true);
  assert.equal(dimension?.score, 1);
  assert.deepEqual(result.metrics.realizedCapabilities, ["streaming"]);
});

test("a declared capability the runtime never demonstrated counts against it (D-043)", () => {
  const result = score(
    [observation()],
    evidence({
      declared: { ...NO_CAPABILITIES, streaming: true, permissionEvents: true },
      exercised: { streaming: true, permissionEvents: true },
      observed: { streaming: true }
    })
  );

  const dimension = result.dimensions.find(item => item.key === "capabilityRealization");
  assert.equal(dimension?.score, 0.5);
  assert.deepEqual(result.metrics.declaredNotObserved, ["permissionEvents"]);
  assert.deepEqual(result.metrics.realizedCapabilities, ["streaming"]);
});

test("a capability the runtime showed but never declared is reported", () => {
  const result = score(
    [observation({ toolEventCount: 2 })],
    evidence({
      declared: { ...NO_CAPABILITIES, streaming: true },
      exercised: { streaming: true, toolEvents: true },
      observed: { streaming: true, toolEvents: true }
    })
  );

  assert.deepEqual(result.metrics.observedNotDeclared, ["toolEvents"]);
  assert.equal(result.metrics.toolEventsObserved, true);
  assert.equal(result.metrics.toolEventCount, 2);
});

test("a blocked evaluation is not a pass, and an unevaluated run is not a failure (D-020)", () => {
  const result = score([
    observation({ executionId: "a", evaluationVerdict: "passed" }),
    observation({ executionId: "b", evaluationVerdict: "blocked" }),
    observation({ executionId: "c" })
  ]);

  assert.equal(result.metrics.evaluations, 2);
  assert.equal(result.metrics.evaluationsPassed, 1);
  assert.equal(result.metrics.evaluationPassRate, 0.5);
  const dimension = result.dimensions.find(item => item.key === "evaluationOutcome");
  assert.equal(dimension?.score, 0.5);
});

test("the cost dimension is unmeasured without a budget and never negative with one", () => {
  const executions = [
    observation({ executionId: "a", terminalStatus: "succeeded", totalTokens: 1_000 }),
    observation({ executionId: "b", terminalStatus: "succeeded", totalTokens: 3_000 })
  ];

  const withoutBudget = score(executions);
  assert.equal(withoutBudget.dimensions.find(item => item.key === "costEfficiency")?.measured, false);
  assert.equal(withoutBudget.metrics.tokensPerSuccessfulExecution, 2_000);

  const withinBudget = score(executions, evidence(), { tokenBudgetPerSuccessfulExecution: 4_000 });
  assert.equal(withinBudget.dimensions.find(item => item.key === "costEfficiency")?.score, 0.5);

  const overBudget = score(executions, evidence(), { tokenBudgetPerSuccessfulExecution: 1_000 });
  assert.equal(overBudget.dimensions.find(item => item.key === "costEfficiency")?.score, 0);
});

test("unmeasured dimensions drop out and the remaining weight is redistributed", () => {
  const result = score([observation({ terminalStatus: "succeeded" })]);

  const measured = result.dimensions.filter(dimension => dimension.measured);
  assert.equal(measured.length, 1);
  assert.equal(measured[0]?.effectiveWeight, 1);
  const totalWeight = result.dimensions.reduce((sum, dimension) => sum + dimension.effectiveWeight, 0);
  assert.equal(Math.round(totalWeight * 1000) / 1000, 1);
  assert.equal(result.score, 100);
});

test("a perfect runtime scores 100 and a failing one scores far below it", () => {
  const perfect = score(
    [
      observation({
        terminalStatus: "succeeded",
        cancelRequested: true,
        cancelConfirmedByRuntime: true,
        toolEventCount: 1,
        evaluationVerdict: "passed"
      })
    ],
    evidence({
      declared: { ...NO_CAPABILITIES, streaming: true },
      exercised: { streaming: true },
      observed: { streaming: true }
    }),
    { tokenBudgetPerSuccessfulExecution: 100_000 }
  );
  assert.equal(perfect.score, 100);

  const failing = score(
    [observation({ terminalStatus: "failed", evaluationVerdict: "failed" })],
    evidence({
      declared: { ...NO_CAPABILITIES, streaming: true },
      exercised: { streaming: true },
      observed: {}
    })
  );
  assert.ok((failing.score ?? 0) < 40, `expected a low score, got ${String(failing.score)}`);
});

test("metrics report tool events, permission events, duration and tokens", () => {
  const result = score([
    observation({
      executionId: "a",
      toolEventCount: 3,
      permissionEventCount: 1,
      messageDeltaCount: 12,
      durationMs: 4_000,
      totalTokens: 120
    }),
    observation({
      executionId: "b",
      toolEventCount: 1,
      messageDeltaCount: 4,
      durationMs: 2_000,
      totalTokens: 80
    })
  ]);

  assert.equal(result.metrics.toolEventsObserved, true);
  assert.equal(result.metrics.toolEventCount, 4);
  assert.equal(result.metrics.executionsWithToolEvents, 2);
  assert.equal(result.metrics.permissionEventsObserved, true);
  assert.equal(result.metrics.permissionEventCount, 1);
  assert.equal(result.metrics.messageDeltas, 16);
  assert.equal(result.metrics.averageDurationMs, 3_000);
  assert.equal(result.metrics.totalTokens, 200);
  assert.equal(result.metrics.averageTokensPerExecution, 100);
});

test("the comparison ranks by score and shows cost and latency as columns", () => {
  const good = scoreRuntime({
    runtimeId: "pi-local",
    runtimeType: "native",
    capabilityEvidence: evidence({
      declared: { ...NO_CAPABILITIES, streaming: true },
      exercised: { streaming: true },
      observed: { streaming: true }
    }),
    executions: [observation({ terminalStatus: "succeeded", totalTokens: 100, durationMs: 1_000 })]
  });
  const bad = scoreRuntime({
    runtimeId: "hermes-local",
    runtimeType: "hermes",
    capabilityEvidence: evidence(),
    executions: [observation({ terminalStatus: "failed" })]
  });

  const comparison = compareRuntimeScores([bad, good]);

  assert.deepEqual(comparison.rows.map(row => row.runtimeId), ["pi-local", "hermes-local"]);
  assert.equal(comparison.rows[0]?.averageDurationMs, 1_000);
  assert.equal(comparison.rows[0]?.tokensPerSuccessfulExecution, 100);
  assert.ok(comparison.lines.length >= 3);
  assert.ok(comparison.lines[1]!.includes("pi-local"));
});

test("journal entries become observations: terminal state, tools, permissions and tokens", () => {
  const entries: JournalEntry[] = [
    {
      executionId: "exec-9",
      sequence: 1,
      kind: "audit",
      category: "execution.runtime",
      recordedAt: "2026-11-11T00:00:00.000Z",
      payload: { runtimeId: "pi-local" }
    },
    {
      executionId: "exec-9",
      sequence: 2,
      kind: "runtime.event",
      eventSequence: 1,
      category: "execution.started",
      recordedAt: "2026-11-11T00:00:01.000Z",
      payload: {}
    },
    {
      executionId: "exec-9",
      sequence: 3,
      kind: "runtime.event",
      eventSequence: 2,
      category: "message.delta",
      recordedAt: "2026-11-11T00:00:02.000Z",
      payload: { delta: "hi" }
    },
    {
      executionId: "exec-9",
      sequence: 4,
      kind: "runtime.event",
      eventSequence: 3,
      category: "tool.started",
      recordedAt: "2026-11-11T00:00:03.000Z",
      payload: { toolName: "read_file" }
    },
    {
      executionId: "exec-9",
      sequence: 5,
      kind: "runtime.event",
      eventSequence: 4,
      category: "permission.requested",
      recordedAt: "2026-11-11T00:00:04.000Z",
      payload: { requestId: "perm-1" }
    },
    {
      executionId: "exec-9",
      sequence: 6,
      kind: "audit",
      category: "execution.cancel_requested",
      recordedAt: "2026-11-11T00:00:05.000Z",
      payload: {}
    },
    {
      executionId: "exec-9",
      sequence: 7,
      kind: "runtime.event",
      eventSequence: 5,
      category: "execution.cancelled",
      recordedAt: "2026-11-11T00:00:06.000Z",
      payload: { runtimeConfirmed: true, runtimeStopReason: "aborted" }
    },
    {
      executionId: "exec-9",
      sequence: 8,
      kind: "audit",
      category: "evaluation.completed",
      recordedAt: "2026-11-11T00:00:07.000Z",
      payload: { evaluationId: "eval-1", verdict: "passed", checks: 1 }
    }
  ];

  const observations = observationsFromJournal(entries);

  assert.equal(observations.length, 1);
  const record = observations[0]!;
  assert.equal(record.runtimeId, "pi-local");
  assert.equal(record.terminalStatus, "cancelled");
  assert.equal(record.cancelRequested, true);
  assert.equal(record.cancelConfirmedByRuntime, true);
  assert.equal(record.runtimeStopReason, "aborted");
  assert.equal(record.toolEventCount, 1);
  assert.equal(record.permissionEventCount, 1);
  assert.equal(record.messageDeltaCount, 1);
  assert.equal(record.evaluationVerdict, "passed");
  assert.equal(record.durationMs, 5_000);
});

test("a cancellation the runtime never confirmed is not counted as confirmed (D-052)", () => {
  const entries: JournalEntry[] = [
    {
      executionId: "exec-10",
      sequence: 1,
      kind: "audit",
      category: "execution.cancel_requested",
      recordedAt: "2026-11-11T00:00:00.000Z",
      payload: {}
    },
    {
      executionId: "exec-10",
      sequence: 2,
      kind: "runtime.event",
      eventSequence: 1,
      category: "execution.cancelled",
      recordedAt: "2026-11-11T00:00:01.000Z",
      payload: { runtimeConfirmed: false, runtimeStopReason: "unconfirmed" }
    }
  ];

  const record = observationsFromJournal(entries)[0]!;

  assert.equal(record.cancelRequested, true);
  assert.equal(record.cancelConfirmedByRuntime, false);
});

test("token usage is read from the shapes the runtimes actually report", () => {
  const hermes = extractTokenUsage({
    usage: { input_tokens: 12_405, output_tokens: 8, total_tokens: 12_413 }
  });
  assert.deepEqual(hermes, { promptTokens: 12_405, completionTokens: 8, totalTokens: 12_413 });

  const pi = extractTokenUsage({
    messages: [{ role: "assistant", usage: { input: 100, output: 20, totalTokens: 120 } }]
  });
  assert.deepEqual(pi, { promptTokens: 100, completionTokens: 20, totalTokens: 120 });

  // No usage is no record, not a zero that would look like a free run.
  assert.deepEqual(extractTokenUsage({}), {});
});

test("every capability key is covered by the declared set", () => {
  assert.deepEqual([...CAPABILITY_KEYS].sort(), [
    "cancellation",
    "permissionEvents",
    "sessionResume",
    "skills",
    "streaming",
    "subAgents",
    "toolEvents"
  ]);
});
