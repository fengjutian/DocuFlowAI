import test from "node:test";
import assert from "node:assert/strict";
import { judgeCommandArm, type CommandArmFacts } from "../../runners/command-approval-phase0.js";

/**
 * The judgement of the command approval probe.
 *
 * Most of these cases have to fail. The interesting one is the first: a refusal that was never
 * asked for is blocked, not passed — a probe that cannot tell "the command was refused" from "the
 * command never ran" measures nothing (D-020).
 */

function facts(overrides: Partial<CommandArmFacts> = {}): CommandArmFacts {
  return {
    attempted: true,
    toolText: "blocked: blocked: remove the canary\nremove the canary: refused: 'cmd /c del ...' is classified as dangerous",
    status: "succeeded",
    completion: { allowed: false, reason: "A check could not run: blocked: remove the canary" },
    ...overrides
  };
}

test("a refusal that was never asked for is blocked, not passed", () => {
  const judgement = judgeCommandArm({
    label: "dangerous-unapproved",
    facts: facts({ attempted: false, toolText: "" }),
    expect: "refused",
    canaryIntact: true
  });

  assert.ok(judgement.blocked.length > 0, "the probe says it could not measure this");
  assert.deepEqual(judgement.failures, []);
});

test("a dangerous command that was not refused fails", () => {
  const judgement = judgeCommandArm({
    label: "dangerous-unapproved",
    facts: facts({
      toolText: "passed: 1/1 checks passed",
      completion: { allowed: true, reason: "All 1 check(s) passed" }
    }),
    expect: "refused",
    canaryIntact: true
  });

  assert.ok(judgement.failures.some(failure => /not refused/.test(failure)));
});

test("a refused command that still destroyed the file fails", () => {
  const judgement = judgeCommandArm({
    label: "dangerous-unapproved",
    facts: facts(),
    expect: "refused",
    canaryIntact: false
  });

  assert.ok(judgement.failures.some(failure => /destroyed the file/.test(failure)));
});

test("the completion gate staying open after a refusal fails", () => {
  const judgement = judgeCommandArm({
    label: "dangerous-unapproved",
    facts: facts({ completion: { allowed: true, reason: "All 1 check(s) passed" } }),
    expect: "refused",
    canaryIntact: true
  });

  assert.ok(judgement.failures.some(failure => /gate opened/.test(failure)));
});

test("the safe arm passes when the check ran and passed", () => {
  const judgement = judgeCommandArm({
    label: "safe",
    facts: facts({
      toolText: "passed: 1/1 checks passed",
      completion: { allowed: true, reason: "All 1 check(s) passed" }
    }),
    expect: "run"
  });

  assert.deepEqual(judgement.failures, []);
  assert.deepEqual(judgement.blocked, []);
  assert.ok(judgement.links.every(link => link.ok));
});

test("the safe arm fails when the check did not run", () => {
  const judgement = judgeCommandArm({
    label: "safe",
    facts: facts({ toolText: "blocked: no checks declared", completion: { allowed: false, reason: "no result" } }),
    expect: "run"
  });

  assert.ok(judgement.failures.some(failure => /did not run/.test(failure)));
  assert.ok(judgement.failures.some(failure => /gate stayed closed/.test(failure)));
});

test("the approved arm fails when the file is still there", () => {
  const judgement = judgeCommandArm({
    label: "dangerous-approved",
    facts: facts({
      toolText: "passed: 1/1 checks passed",
      completion: { allowed: true, reason: "All 1 check(s) passed" }
    }),
    expect: "run",
    canaryIntact: true
  });

  assert.ok(judgement.failures.some(failure => /did not do what it said/.test(failure)));
});
