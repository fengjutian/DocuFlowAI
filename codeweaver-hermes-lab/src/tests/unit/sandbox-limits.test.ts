import test from "node:test";
import assert from "node:assert/strict";
import {
  classifyLimitHit,
  normalizeSandboxLimits,
  validateSandboxLimits,
  type SandboxRunResult
} from "../../evaluation/sandbox-limits.js";

function run(overrides: Partial<SandboxRunResult> = {}): SandboxRunResult {
  return {
    started: true,
    timedOut: false,
    exitCode: 0,
    exitCodeKnown: true,
    peakJobMemoryBytes: 0,
    jobMemoryLimitBytes: 0,
    processLimit: 0,
    limitViolation: "",
    error: "",
    stdout: "",
    stderr: "",
    ...overrides
  };
}

const limits = normalizeSandboxLimits({ memoryMb: 128, cpuRatePercent: 2, processLimit: 4 });

test("rejects limits that cannot be applied", () => {
  const violations = validateSandboxLimits({ memoryMb: 0, cpuRatePercent: 101, processLimit: 1.5 });
  assert.ok(violations.some(item => /memoryMb/.test(item)));
  assert.ok(violations.some(item => /cpuRatePercent/.test(item)));
  assert.ok(violations.some(item => /processLimit/.test(item)));
  assert.deepEqual(validateSandboxLimits({ memoryMb: 128, cpuRatePercent: 2, processLimit: 4 }), []);
});

test("the limits a task does not name keep the default", () => {
  const resolved = normalizeSandboxLimits({ memoryMb: 256 });
  assert.equal(resolved.memoryMb, 256);
  assert.equal(resolved.cpuRatePercent, 25);
  assert.equal(resolved.processLimit, 64);
});

test("names the limit the kernel recorded", () => {
  const memory = classifyLimitHit(run({ limitViolation: "job_memory" }), limits);
  assert.equal(memory?.hit, "memory");
  assert.match(memory!.reason, /128 MB/);

  const processes = classifyLimitHit(run({ limitViolation: "active_process" }), limits);
  assert.equal(processes?.hit, "processes");
  assert.match(processes!.reason, /4 processes/);

  const cpu = classifyLimitHit(run({ limitViolation: "cpu_rate" }), limits);
  assert.equal(cpu?.hit, "cpu");
  assert.match(cpu!.reason, /2%/);
});

test("names the limit the child ran into on its own", () => {
  // A limit that stops a *child* leaves nothing in the job's violation record: the only trace is
  // the words of the process that could not allocate.
  const memory = classifyLimitHit(run({ exitCode: 1, stderr: "RangeError: Array buffer allocation failed" }), limits);
  assert.equal(memory?.hit, "memory");

  // Windows answers a spawn beyond the job's process limit with ERROR_NOT_ENOUGH_QUOTA (1816).
  const processes = classifyLimitHit(run({ exitCode: 1, stderr: "Error: spawn UNKNOWN { errno: -4094 }" }), limits);
  assert.equal(processes?.hit, "processes");
});

test("a check that failed for its own reasons names no limit", () => {
  const verdict = classifyLimitHit(run({ exitCode: 1, stderr: "assertion failed: 1 !== 2" }), limits);
  assert.equal(verdict, undefined);
});

test("a run that took too long is a timeout, not a silent failure", () => {
  const verdict = classifyLimitHit(run({ timedOut: true, exitCode: null, exitCodeKnown: false }), limits);
  assert.equal(verdict?.hit, "timeout");
});

test("an empty result names no limit", () => {
  assert.equal(classifyLimitHit(run(), limits), undefined);
});
