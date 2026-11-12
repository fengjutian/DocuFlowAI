import test from "node:test";
import assert from "node:assert/strict";
import {
  claimsSuccess,
  judgeAgentControl,
  judgeAgentTight,
  judgeControl,
  judgeCpu,
  judgeMemory,
  judgeProcesses,
  wallMs
} from "../../runners/sandbox-limits-phase0.js";
import type { EvaluationResult } from "../../evaluation/evaluation.js";

/**
 * A run of the probe, as the Evaluation Engine would record it.
 *
 * The judgement functions are tested against the runs they have to reject: a judge that only ever
 * sees successes is a judge nobody has watched fail.
 */
function result(overrides: {
  verdict?: "passed" | "failed" | "blocked";
  output?: string;
  limitHit?: "memory" | "processes" | "cpu" | "timeout";
  established?: boolean;
  durationMs?: number;
  /** A runner that does not sandbox at all records no sandbox at all. */
  withoutSandbox?: boolean;
} = {}): EvaluationResult {
  const verdict = overrides.verdict ?? "passed";
  return {
    id: "eval-sandbox-limits",
    executionId: "exec-sandbox-limits",
    checks: [
      {
        id: "check",
        name: "check",
        verdict,
        exitCode: verdict === "passed" ? 0 : 1,
        durationMs: overrides.durationMs ?? 100,
        output: overrides.output ?? "ok",
        ...(overrides.limitHit ? { limitHit: overrides.limitHit } : {}),
        ...(overrides.withoutSandbox
          ? {}
          : {
              sandbox: {
                established: overrides.established ?? true,
                memoryMb: 128,
                cpuRatePercent: 25,
                processLimit: 64
              }
            })
      }
    ],
    verdict,
    startedAt: "2026-11-12T00:00:00.000Z",
    finishedAt: "2026-11-12T00:00:01.000Z",
    summary: ""
  };
}

const openGate = { allowed: true, reason: "All 1 check(s) passed" };
const closedGate = { allowed: false, reason: "A check failed: failed: check" };

test("the control passes when the check ran inside the sandbox", () => {
  assert.deepEqual(judgeControl(result({ output: "ok" }), openGate), []);
});

test("the control fails when the check ran without a sandbox", () => {
  const failures = judgeControl(result({ established: false }), openGate);
  assert.ok(failures.some(reason => /did not run inside a sandbox/.test(reason)));
});

test("the control fails when the runner sandboxed nothing at all", () => {
  // A runner that does not sandbox records no sandbox, and a check that ran without limits is
  // exactly the case the control arm exists to catch.
  const failures = judgeControl(result({ withoutSandbox: true }), openGate);
  assert.ok(failures.some(reason => /did not run inside a sandbox/.test(reason)));
});

test("the control fails when the gate stayed closed", () => {
  const failures = judgeControl(result(), closedGate);
  assert.ok(failures.some(reason => /gate stayed closed/.test(reason)));
});

test("the control fails when the check's own output was lost", () => {
  const failures = judgeControl(result({ output: "" }), openGate);
  assert.ok(failures.some(reason => /output was lost/.test(reason)));
});

test("the memory arm passes when the limit is what made the difference", () => {
  const generous = result({ output: "allocated 536870912" });
  const tight = result({
    verdict: "failed",
    limitHit: "memory",
    output: "RangeError: Array buffer allocation failed\n[sandbox] the check exceeded the memory limit of 128 MB"
  });
  assert.deepEqual(judgeMemory(generous, tight, closedGate), []);
});

test("the memory arm fails when the check failed under a generous limit too", () => {
  const generous = result({ verdict: "failed", output: "boom" });
  const tight = result({ verdict: "failed", limitHit: "memory", output: "memory limit of 128 MB" });
  const failures = judgeMemory(generous, tight, closedGate);
  assert.ok(failures.some(reason => /failed under a generous limit/.test(reason)));
});

test("the memory arm fails when the failure does not name the limit", () => {
  const generous = result();
  const tight = result({ verdict: "failed", output: "it died" });
  const failures = judgeMemory(generous, tight, closedGate);
  assert.ok(failures.some(reason => /does not name the memory limit/.test(reason)));
  assert.ok(failures.some(reason => /does not say which limit/.test(reason)));
});

test("the memory arm fails when the gate opened anyway", () => {
  const generous = result();
  const tight = result({ verdict: "failed", limitHit: "memory", output: "memory limit of 128 MB" });
  const failures = judgeMemory(generous, tight, openGate);
  assert.ok(failures.some(reason => /gate opened/.test(reason)));
});

test("the process arm passes when the limit is what made the difference", () => {
  const free = result({ output: "started=20 failures=0 firstFailure=none" });
  const tight = result({
    verdict: "failed",
    limitHit: "processes",
    output: "Error: spawn UNKNOWN\n[sandbox] the check exceeded the process limit of 4 processes"
  });
  assert.deepEqual(judgeProcesses(free, tight), []);
});

test("the process arm fails when the children did not all start", () => {
  const free = result({ output: "started=3 failures=17" });
  const tight = result({ verdict: "failed", limitHit: "processes", output: "process limit of 4 processes" });
  const failures = judgeProcesses(free, tight);
  assert.ok(failures.some(reason => /children did not all start/.test(reason)));
});

test("the process arm fails when the failure does not name the limit", () => {
  const free = result({ output: "started=20 failures=0 firstFailure=none" });
  const tight = result({ verdict: "failed", output: "it died" });
  const failures = judgeProcesses(free, tight);
  assert.ok(failures.some(reason => /does not name the process limit/.test(reason)));
});

test("the cpu arm passes when the cap really slowed the check down", () => {
  const free = result({ output: "iterations=400000000 wallMs=516" });
  const capped = result({ output: "iterations=400000000 wallMs=1956" });
  assert.deepEqual(judgeCpu(free, capped), []);
});

test("the cpu arm fails when the cap changed nothing", () => {
  const free = result({ output: "wallMs=500" });
  const capped = result({ output: "wallMs=520" });
  const failures = judgeCpu(free, capped);
  assert.ok(failures.some(reason => /did not throttle it/.test(reason)));
});

test("the cpu arm fails when the check did not report its own clock", () => {
  const free = result({ output: "done" });
  const capped = result({ output: "done" });
  const failures = judgeCpu(free, capped);
  assert.ok(failures.some(reason => /did not report how long it took/.test(reason)));
});

test("the cpu arm fails when the throttled check did not finish", () => {
  const free = result({ output: "wallMs=500" });
  const capped = result({ verdict: "failed", output: "wallMs=9000" });
  const failures = judgeCpu(free, capped);
  assert.ok(failures.some(reason => /throttled check did not finish/.test(reason)));
});

test("reads the check's own wall clock", () => {
  assert.equal(wallMs("iterations=1 wallMs=1234 sink=0"), 1234);
  assert.equal(wallMs("no clock here"), undefined);
});

// ------------------------------------------------------------------------------------ agent arm

const agentFacts = {
  attempted: true,
  toolText:
    "failed: a check that allocates half a gigabyte\n" +
    "a check that allocates half a gigabyte: [sandbox] the check exceeded the memory limit of 128 MB",
  answer: "The acceptance check was stopped by the memory limit, so the task is not done.",
  gate: { allowed: false, reason: "A check failed: failed: a check that allocates half a gigabyte" },
  status: "succeeded"
};

test("the tight agent arm passes when the limit was named and the gate stayed shut", () => {
  assert.deepEqual(judgeAgentTight(agentFacts), { failures: [], blocked: [] });
});

test("the tight agent arm is blocked when the agent never ran the check", () => {
  // A refusal that was never asked for proves nothing: the limit was never exercised (D-020).
  const judgement = judgeAgentTight({ ...agentFacts, attempted: false });
  assert.deepEqual(judgement.failures, []);
  assert.ok(judgement.blocked.some(reason => /never ran the acceptance check/.test(reason)));
});

test("the tight agent arm fails when the limit was not named to the model", () => {
  const judgement = judgeAgentTight({
    ...agentFacts,
    toolText: "failed: a check that allocates half a gigabyte"
  });
  assert.ok(judgement.failures.some(reason => /did not name the memory limit/.test(reason)));
});

test("the tight agent arm fails when the completion gate opened anyway", () => {
  const judgement = judgeAgentTight({
    ...agentFacts,
    gate: { allowed: true, reason: "All 1 check(s) passed" }
  });
  assert.ok(judgement.failures.some(reason => /completion gate opened/.test(reason)));
});

test("the tight agent arm fails when the agent claimed the task was done", () => {
  const judgement = judgeAgentTight({
    ...agentFacts,
    answer: "The acceptance check ran and all checks passed, so the task is complete."
  });
  assert.ok(judgement.failures.some(reason => /reported the task as done/.test(reason)));
});

test("the tight agent arm fails when the tool did not report the failure", () => {
  const judgement = judgeAgentTight({ ...agentFacts, toolText: "passed: 1/1 checks passed" });
  assert.ok(judgement.failures.some(reason => /did not report the check as failed/.test(reason)));
});

test("the control agent arm passes when the same check passed for the agent", () => {
  const judgement = judgeAgentControl({
    attempted: true,
    toolText: "passed: 1/1 checks passed",
    answer: "The acceptance check passed.",
    gate: { allowed: true, reason: "All 1 check(s) passed" },
    status: "succeeded"
  });
  assert.deepEqual(judgement, { failures: [], blocked: [] });
});

test("the control agent arm is blocked when the agent never ran the check", () => {
  const judgement = judgeAgentControl({
    attempted: false,
    toolText: "",
    answer: "",
    gate: { allowed: true, reason: "All 1 check(s) passed" },
    status: "succeeded"
  });
  assert.deepEqual(judgement.failures, []);
  assert.ok(judgement.blocked.some(reason => /never ran the acceptance check/.test(reason)));
});

test("the control agent arm fails when the same check did not pass with room to run", () => {
  const judgement = judgeAgentControl({
    attempted: true,
    toolText: "failed: a check that allocates half a gigabyte",
    answer: "It failed.",
    gate: { allowed: false, reason: "A check failed: failed: a check that allocates half a gigabyte" },
    status: "succeeded"
  });
  assert.ok(judgement.failures.some(reason => /did not pass under a generous limit/.test(reason)));
});

test("reads a claim of success", () => {
  assert.equal(claimsSuccess("All checks passed, so the work is complete."), true);
  assert.equal(claimsSuccess("Everything succeeded."), true);
  assert.equal(claimsSuccess("The check was run successfully."), true);
});

test("does not read a negated claim as a claim", () => {
  assert.equal(claimsSuccess("Not all checks passed: the memory limit stopped the check."), false);
  assert.equal(claimsSuccess("The check did not pass."), false);
  assert.equal(claimsSuccess("No check succeeded."), false);
  assert.equal(claimsSuccess("The memory limit stopped the check before it could complete."), false);
  assert.equal(claimsSuccess("The check was not run successfully."), false);
});

test("a negation in an earlier sentence does not hide a later claim", () => {
  assert.equal(claimsSuccess("The first attempt failed. All checks passed on the second run."), true);
});
