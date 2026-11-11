import test from "node:test";
import assert from "node:assert/strict";
import { resolve } from "node:path";
import { executeSecurityPhase0 } from "../../runners/security-phase0.js";
import type { ProbeOutcome } from "../../security/boundary-probes.js";
import type { BoundarySpec } from "../../security/boundary-spec.js";

function spec(overrides: Partial<BoundarySpec> = {}): BoundarySpec {
  return {
    backend: "docker",
    image: "alpine:3.20",
    workspaceDir: resolve("fixtures/security-lab/workspace"),
    restrictedDir: resolve("fixtures/security-lab/restricted"),
    outsideDir: resolve("fixtures/security-lab/outside"),
    memoryLimitMb: 512,
    cpus: 1,
    pidsLimit: 128,
    ...overrides
  };
}

function ok(stdout: string): ProbeOutcome {
  return { exitCode: 0, stdout, stderr: "", timedOut: false };
}

function denied(): ProbeOutcome {
  return { exitCode: 1, stdout: "", stderr: "No such file or directory", timedOut: false };
}

function scriptOf(args: string[]): string {
  return args.at(-1) ?? "";
}

test("passes every boundary probe when the sandbox enforces its limits", async () => {
  const results = await executeSecurityPhase0({
    spec: spec(),
    checkRuntime: async () => "27.0.0",
    runProbe: async args => {
      const script = scriptOf(args);
      if (script === "cat /workspace/README.md") return ok("Allowed workspace fixture.\\n");
      if (script.startsWith("printf CODEWEAVER_WRITE_PROBE")) return ok("CODEWEAVER_WRITE_PROBE");
      return denied();
    }
  });

  assert.equal(results.length, 6);
  assert.deepEqual([...new Set(results.map(item => item.status))], ["passed"]);
  assert.ok(results[0]!.evidence.some(item => /runtime=27\.0\.0/.test(item)));
  assert.ok(results[0]!.evidence.some(item => /supports=P0-009/.test(item)));
});

test("fails the restricted-read probe when the sandbox exposes the restricted file", async () => {
  const results = await executeSecurityPhase0({
    spec: spec(),
    checkRuntime: async () => "27.0.0",
    runProbe: async args => {
      const script = scriptOf(args);
      if (script === "cat /workspace/README.md") return ok("Allowed workspace fixture.\\n");
      if (script === "cat /restricted/secret.txt") return ok("SYNTHETIC_SECRET_DO_NOT_READ");
      return denied();
    }
  });

  const restricted = results.find(item => item.id === "SEC-B02");
  assert.equal(restricted?.status, "failed");
  assert.match(restricted?.notes ?? "", /BOUNDARY BREACH/);
});

test("blocks every probe when the container runtime is unavailable", async () => {
  const results = await executeSecurityPhase0({
    spec: spec(),
    checkRuntime: async () => { throw new Error("open //./pipe/docker: The system cannot find the file"); },
    runProbe: async () => denied()
  });

  assert.equal(results.length, 6);
  assert.deepEqual([...new Set(results.map(item => item.status))], ["blocked"]);
  assert.ok(results.every(item => /Container runtime unavailable/.test(item.notes ?? "")));
});

test("blocks every probe when the boundary specification is not usable as evidence", async () => {
  const results = await executeSecurityPhase0({
    spec: spec({ backend: "none" }),
    checkRuntime: async () => "27.0.0",
    runProbe: async () => ok("")
  });

  assert.deepEqual([...new Set(results.map(item => item.status))], ["blocked"]);
  assert.ok(results.every(item => /backend must be 'docker'/.test(item.notes ?? "")));
});
