import test from "node:test";
import assert from "node:assert/strict";
import { resolve } from "node:path";
import { executeSecurityPhase0 } from "../../runners/security-phase0.js";
import type { ProbeOutcome } from "../../security/boundary-probes.js";
import type { BoundaryProvider } from "../../security/boundary-provider.js";
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

/**
 * A provider that answers from a script table. The table is keyed by the probe script, so a test
 * can say exactly what the boundary returns for each question - and can say it wrongly, which is
 * the point of a test.
 */
function provider(run: (script: string) => ProbeOutcome, overrides: Partial<BoundaryProvider> = {}): BoundaryProvider {
  return {
    backend: "docker",
    available: async () => ({ ok: true, reason: "available" }),
    describe: async () => "test boundary",
    run: async (_spec, script) => run(script),
    ...overrides
  };
}

/** A boundary that behaves the way a working one does. */
function enforcingProvider(overrides: Partial<BoundaryProvider> = {}): BoundaryProvider {
  return provider(script => {
    if (script === "cat /workspace/README.md") return ok("Allowed workspace fixture.\n");
    if (script.startsWith("printf CODEWEAVER_WRITE_PROBE")) return ok("CODEWEAVER_WRITE_PROBE");
    if (script.startsWith('[ "$(id -u)"')) return ok("uid=1002 user=cwlab");
    return denied();
  }, overrides);
}

test("passes every boundary probe when the sandbox enforces its limits", async () => {
  const results = await executeSecurityPhase0({
    spec: spec(),
    provider: enforcingProvider()
  });

  assert.equal(results.length, 10);
  assert.deepEqual([...new Set(results.map(item => item.status))], ["passed"]);
  assert.ok(results[0]!.evidence.some(item => /backend=docker/.test(item)));
  assert.ok(results[0]!.evidence.some(item => /boundary=test boundary/.test(item)));
  assert.ok(results[0]!.evidence.some(item => /supports=P0-009/.test(item)));
});

test("fails the restricted-read probe when the sandbox exposes the restricted file", async () => {
  const results = await executeSecurityPhase0({
    spec: spec(),
    provider: provider(script => {
      if (script === "cat /workspace/README.md") return ok("Allowed workspace fixture.\n");
      if (script === "cat /restricted/secret.txt") return ok("SYNTHETIC_SECRET_DO_NOT_READ");
      return denied();
    })
  });

  const restricted = results.find(item => item.id === "SEC-B02");
  assert.equal(restricted?.status, "failed");
  assert.match(restricted?.notes ?? "", /BOUNDARY BREACH/);
});

test("fails the outside-read probe when the sandbox exposes the outside directory", async () => {
  const results = await executeSecurityPhase0({
    spec: spec(),
    provider: provider(script => {
      if (script === "cat /workspace/README.md") return ok("Allowed workspace fixture.\n");
      if (script.startsWith("printf CODEWEAVER_WRITE_PROBE")) return ok("CODEWEAVER_WRITE_PROBE");
      if (script === "cat /outside/forbidden.txt") return ok("FORBIDDEN_CONTENT");
      return denied();
    })
  });

  const outside = results.find(item => item.id === "SEC-B03");
  assert.equal(outside?.status, "failed");
  assert.match(outside?.notes ?? "", /BOUNDARY BREACH/);
});

test("fails the path-traversal probe when the workspace escape succeeds", async () => {
  const results = await executeSecurityPhase0({
    spec: spec(),
    provider: provider(script => {
      if (script === "cat /workspace/README.md") return ok("Allowed workspace fixture.\n");
      if (script.startsWith("printf CODEWEAVER_WRITE_PROBE")) return ok("CODEWEAVER_WRITE_PROBE");
      if (script === "cat /workspace/../outside/forbidden.txt") return ok("FORBIDDEN_CONTENT");
      return denied();
    })
  });

  const traversal = results.find(item => item.id === "SEC-B05");
  assert.equal(traversal?.status, "failed");
  assert.match(traversal?.notes ?? "", /BOUNDARY BREACH/);
});

test("fails the egress probe when the sandbox can reach the network", async () => {
  const results = await executeSecurityPhase0({
    spec: spec(),
    provider: provider(script => {
      if (script === "cat /workspace/README.md") return ok("Allowed workspace fixture.\n");
      if (script.startsWith("printf CODEWEAVER_WRITE_PROBE")) return ok("CODEWEAVER_WRITE_PROBE");
      if (script.includes("wget -T 3")) return ok("200 OK");
      return denied();
    })
  });

  const egress = results.find(item => item.id === "SEC-B06");
  assert.equal(egress?.status, "failed");
  assert.match(egress?.notes ?? "", /BOUNDARY BREACH/);
});

test("fails the unprivileged-user probe when the boundary runs the work as root", async () => {
  const results = await executeSecurityPhase0({
    spec: spec(),
    provider: provider(script => {
      if (script === "cat /workspace/README.md") return ok("Allowed workspace fixture.\n");
      if (script.startsWith("printf CODEWEAVER_WRITE_PROBE")) return ok("CODEWEAVER_WRITE_PROBE");
      if (script.startsWith('[ "$(id -u)"')) return ok("uid=0 user=root");
      return denied();
    })
  });

  const user = results.find(item => item.id === "SEC-B07");
  assert.equal(user?.status, "failed");
  assert.match(user?.notes ?? "", /must never produce/);
});

test("fails the windows-executable probe when the boundary can launch a windows binary", async () => {
  const results = await executeSecurityPhase0({
    spec: spec(),
    provider: provider(script => {
      if (script === "cat /workspace/README.md") return ok("Allowed workspace fixture.\n");
      if (script.startsWith("printf CODEWEAVER_WRITE_PROBE")) return ok("CODEWEAVER_WRITE_PROBE");
      if (script.startsWith('[ "$(id -u)"')) return ok("uid=1002 user=cwlab");
      if (script.includes(".cw-mz-probe.exe")) return ok("WINDOWS_EXECUTABLE_NOT_REFUSED");
      return denied();
    })
  });

  const windows = results.find(item => item.id === "SEC-B09");
  assert.equal(windows?.status, "failed");
  assert.match(windows?.notes ?? "", /BOUNDARY BREACH/);
});

test("fails the raw-disk probe when the boundary can read the host disks", async () => {
  const results = await executeSecurityPhase0({
    spec: spec(),
    provider: provider(script => {
      if (script === "cat /workspace/README.md") return ok("Allowed workspace fixture.\n");
      if (script.startsWith("printf CODEWEAVER_WRITE_PROBE")) return ok("CODEWEAVER_WRITE_PROBE");
      if (script.startsWith('[ "$(id -u)"')) return ok("uid=1002 user=cwlab");
      if (script.includes("/dev/sd*")) return ok("RAW_DISK_READABLE:/dev/sda");
      return denied();
    })
  });

  const disks = results.find(item => item.id === "SEC-B10");
  assert.equal(disks?.status, "failed");
  assert.match(disks?.notes ?? "", /BOUNDARY BREACH/);
});

test("fails the host-drive probe when a host drive is still mounted inside the boundary", async () => {
  const results = await executeSecurityPhase0({
    spec: spec(),
    provider: provider(script => {
      if (script === "cat /workspace/README.md") return ok("Allowed workspace fixture.\n");
      if (script.startsWith("printf CODEWEAVER_WRITE_PROBE")) return ok("CODEWEAVER_WRITE_PROBE");
      if (script.startsWith('[ "$(id -u)"')) return ok("uid=1002 user=cwlab");
      if (script.includes("/mnt/c /mnt/d")) return ok("HOST_DRIVE_MOUNTED:/mnt/c");
      return denied();
    })
  });

  const drives = results.find(item => item.id === "SEC-B08");
  assert.equal(drives?.status, "failed");
  assert.match(drives?.notes ?? "", /BOUNDARY BREACH/);
});

test("fails the allowed-workspace probe when the sandbox cannot read the workspace", async () => {
  const results = await executeSecurityPhase0({
    spec: spec(),
    provider: provider(() => denied())
  });

  const allowed = results.find(item => item.id === "SEC-B01");
  assert.equal(allowed?.status, "failed");
  assert.match(allowed?.notes ?? "", /Expected the operation to succeed/);
});

test("blocks every probe when the boundary runtime is unavailable", async () => {
  const results = await executeSecurityPhase0({
    spec: spec(),
    provider: enforcingProvider({
      available: async () => ({
        ok: false,
        reason: "the distribution 'Ubuntu' is not installed (found: none)"
      })
    })
  });

  assert.equal(results.length, 10);
  assert.deepEqual([...new Set(results.map(item => item.status))], ["blocked"]);
  assert.ok(results.every(item => /Boundary runtime unavailable/.test(item.notes ?? "")));
  assert.ok(results.every(item => /not installed/.test(item.notes ?? "")));
});

test("blocks every probe when the boundary specification is not usable as evidence", async () => {
  const results = await executeSecurityPhase0({
    spec: spec({ backend: "none" }),
    provider: enforcingProvider()
  });

  assert.deepEqual([...new Set(results.map(item => item.status))], ["blocked"]);
  assert.ok(results.every(item => /backend must be one of/.test(item.notes ?? "")));
});

test("blocks every probe when the boundary cannot even be described", async () => {
  const results = await executeSecurityPhase0({
    spec: spec(),
    provider: enforcingProvider({
      describe: async () => {
        throw new Error("wsl is not usable on this machine");
      }
    })
  });

  assert.deepEqual([...new Set(results.map(item => item.status))], ["blocked"]);
  assert.ok(results.every(item => /wsl is not usable/.test(item.notes ?? "")));
});
