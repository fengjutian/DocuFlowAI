import test from "node:test";
import assert from "node:assert/strict";
import { SpawnCommandRunner, type CommandRunner } from "../../evaluation/command-runner.js";
import { completionGate, evaluateChecks, type EvaluationCheckSpec } from "../../evaluation/evaluation.js";

function fakeRunner(exitCode: number | null, overrides: { timedOut?: boolean } = {}): CommandRunner {
  return {
    async run() {
      return { code: exitCode, stdout: "out", stderr: "err", timedOut: overrides.timedOut ?? false };
    }
  };
}

const check = (id: string): EvaluationCheckSpec => ({
  id,
  name: `check-${id}`,
  command: "node",
  args: ["-e", "process.exit(0)"]
});

test("passes when every check exits with the expected code", async () => {
  const result = await evaluateChecks("exec-1", [check("a"), check("b")], fakeRunner(0), {
    now: () => new Date("2026-11-09T00:00:00.000Z"),
    idFactory: () => "eval-1"
  });

  assert.equal(result.verdict, "passed");
  assert.equal(result.id, "eval-1");
  assert.equal(result.checks.length, 2);
  assert.match(result.summary, /2\/2 checks passed/);
  assert.deepEqual(completionGate(result), { allowed: true, reason: "All 2 check(s) passed" });
});

test("fails when any check exits non-zero", async () => {
  const result = await evaluateChecks("exec-1", [check("a")], fakeRunner(1));
  const gate = completionGate(result);

  assert.equal(result.verdict, "failed");
  assert.equal(gate.allowed, false);
  assert.match(gate.reason, /check failed|A check failed/);
});

test("blocks when a check could not run", async () => {
  const timedOut = await evaluateChecks("exec-1", [check("a")], fakeRunner(null, { timedOut: true }));
  assert.equal(timedOut.verdict, "blocked");

  const spawnFailed = await evaluateChecks("exec-1", [check("a")], fakeRunner(null));
  assert.equal(spawnFailed.verdict, "blocked");
  assert.match(completionGate(spawnFailed).reason, /could not run/);
});

test("treats an empty check list as unproven, not as success", async () => {
  const result = await evaluateChecks("exec-1", [], fakeRunner(0));

  assert.equal(result.verdict, "blocked");
  const gate = completionGate(result);
  assert.equal(gate.allowed, false);
  assert.match(gate.reason, /no checks|absence of evidence/i);
});

test("denies completion when no evaluation exists", () => {
  const gate = completionGate(undefined);

  assert.equal(gate.allowed, false);
  assert.match(gate.reason, /No evaluation result/);
});

test("runs real commands without a shell and captures exit codes", async () => {
  const runner = new SpawnCommandRunner();
  const passing = await evaluateChecks("exec-1", [{
    id: "real",
    name: "real command",
    command: process.execPath,
    args: ["-e", "console.log('ok')"]
  }], runner);
  assert.equal(passing.verdict, "passed");
  assert.match(passing.checks[0]!.output, /ok/);

  const failing = await evaluateChecks("exec-1", [{
    id: "real-fail",
    name: "failing command",
    command: process.execPath,
    args: ["-e", "process.exit(3)"]
  }], runner);
  assert.equal(failing.verdict, "failed");
  assert.equal(failing.checks[0]!.exitCode, 3);
});
