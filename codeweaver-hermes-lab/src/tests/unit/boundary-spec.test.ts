import test from "node:test";
import assert from "node:assert/strict";
import { resolve } from "node:path";
import {
  buildDockerArgs,
  toDockerPath,
  validateBoundarySpec,
  type BoundarySpec
} from "../../security/boundary-spec.js";

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

test("accepts a docker boundary over the fixture directories", () => {
  assert.deepEqual(validateBoundarySpec(spec()), []);
});

test("rejects a spec that claims isolation without a container backend", () => {
  const violations = validateBoundarySpec(spec({ backend: "none" }));

  assert.ok(violations.some(item => /backend must be 'docker'/.test(item)));
});

test("rejects overlapping or missing fixture directories", () => {
  const overlapping = validateBoundarySpec(spec({ restrictedDir: resolve("fixtures/security-lab/workspace") }));
  assert.ok(overlapping.some(item => /overlap/.test(item)));

  const missing = validateBoundarySpec(spec({ workspaceDir: resolve("fixtures/does-not-exist") }));
  assert.ok(missing.some(item => /does not exist/.test(item)));
});

test("rejects unusable resource limits", () => {
  const violations = validateBoundarySpec(spec({ memoryLimitMb: 0, cpus: -1, pidsLimit: 1.5 }));

  assert.ok(violations.some(item => /memoryLimitMb/.test(item)));
  assert.ok(violations.some(item => /cpus/.test(item)));
  assert.ok(violations.some(item => /pidsLimit/.test(item)));
});

test("mounts only the workspace and drops network and capabilities", () => {
  const args = buildDockerArgs(spec(), "cat /workspace/README.md");
  const mounts = args.filter(arg => arg.startsWith("-v")).length;
  const mountValue = args[args.indexOf("-v") + 1]!;

  assert.equal(mounts, 1);
  assert.match(mountValue, /\/workspace:rw$/);
  assert.ok(args.includes("--network"));
  assert.equal(args[args.indexOf("--network") + 1], "none");
  assert.ok(args.includes("--cap-drop"));
  assert.ok(args.includes("no-new-privileges"));
  assert.equal(args.at(-1), "cat /workspace/README.md");
  assert.equal(args.at(-2), "-c");
});

test("passes the probe script as one argv element instead of a shell string", () => {
  const script = "cat /workspace/README.md; echo injected";
  const args = buildDockerArgs(spec(), script);

  assert.equal(args.filter(arg => arg === script).length, 1);
});

test("converts backslashes to forward slashes for docker mounts", () => {
  const converted = toDockerPath(resolve("fixtures", "security-lab", "workspace"));

  assert.equal(converted.includes("\\"), false);
  assert.ok(converted.endsWith("/fixtures/security-lab/workspace"));
});
