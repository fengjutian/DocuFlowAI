import test from "node:test";
import assert from "node:assert/strict";
import { evaluateBoundaryProbe, type BoundaryProbe, type ProbeOutcome } from "../../security/boundary-probes.js";

const allowedRead: BoundaryProbe = {
  id: "SEC-B01",
  title: "Allowed workspace file is readable",
  script: "cat /workspace/README.md",
  expectation: "success",
  marker: "Allowed workspace fixture",
  mapsTo: ["P0-009"]
};

const restrictedRead: BoundaryProbe = {
  id: "SEC-B02",
  title: "Restricted directory is absent from the sandbox",
  script: "cat /restricted/secret.txt",
  expectation: "denied",
  mapsTo: ["P0-010"]
};

function outcome(overrides: Partial<ProbeOutcome> = {}): ProbeOutcome {
  return { exitCode: 0, stdout: "", stderr: "", timedOut: false, ...overrides };
}

test("passes an allowed read that returns the expected marker", () => {
  const verdict = evaluateBoundaryProbe(allowedRead, outcome({ stdout: "Allowed workspace fixture.\\n" }));

  assert.equal(verdict.status, "passed");
});

test("fails an allowed read that produced the wrong content", () => {
  const verdict = evaluateBoundaryProbe(allowedRead, outcome({ stdout: "something else" }));

  assert.equal(verdict.status, "failed");
  assert.match(verdict.notes ?? "", /marker/);
});

test("fails an allowed read that could not run inside the boundary", () => {
  const verdict = evaluateBoundaryProbe(allowedRead, outcome({ exitCode: 1, stderr: "No such file" }));

  assert.equal(verdict.status, "failed");
  assert.ok(verdict.evidence.some(item => /stderr=No such file/.test(item)));
});

test("passes a denied probe when the operation fails", () => {
  const verdict = evaluateBoundaryProbe(restrictedRead, outcome({
    exitCode: 1,
    stderr: "cat: can't open '/restricted/secret.txt': No such file or directory"
  }));

  assert.equal(verdict.status, "passed");
  assert.ok(verdict.evidence.some(item => /No such file/.test(item)));
});

test("fails a denied probe that succeeded, and says the boundary was breached", () => {
  const verdict = evaluateBoundaryProbe(restrictedRead, outcome({ stdout: "SYNTHETIC_SECRET_DO_NOT_READ" }));

  assert.equal(verdict.status, "failed");
  assert.match(verdict.notes ?? "", /BOUNDARY BREACH/);
});

test("treats a timed-out probe as blocked rather than proof of enforcement", () => {
  assert.equal(evaluateBoundaryProbe(restrictedRead, outcome({ exitCode: null, timedOut: true })).status, "blocked");
  assert.equal(evaluateBoundaryProbe(allowedRead, outcome({ exitCode: null, timedOut: true })).status, "blocked");
});

test("treats an unexecutable probe as blocked", () => {
  const verdict = evaluateBoundaryProbe(restrictedRead, outcome({ exitCode: null, stderr: "spawn docker ENOENT" }));

  assert.equal(verdict.status, "blocked");
});
