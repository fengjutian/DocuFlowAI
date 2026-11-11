import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  SkillBindingError,
  SkillBindingStore,
  type SkillBindingEvent
} from "../../skills/skill-bindings.js";
import { ImmutableRecordStore } from "../../persistence/record-store.js";
import {
  SkillConflictError,
  SkillNotFoundError,
  SkillRegistry,
  type SkillManifestRecord
} from "../../skills/skill-registry.js";
import {
  MAX_SKILL_CONTENT_BYTES,
  SkillValidationError,
  compareVersions,
  skillContentChecksum,
  type SkillManifest
} from "../../skills/skill.js";
import type { PolicySnapshot } from "../../policy/permission-policy.js";

const CONTENT = "# React development\n\nPrefer function components and hooks.\n";

function manifest(overrides: Partial<SkillManifest> = {}): SkillManifest {
  return {
    skillId: "react-dev",
    version: "1.0.0",
    name: "React development",
    kind: "engineering",
    permissions: ["read_file", "search_code", "modify_worktree"],
    allowedRuntimes: ["hermes-local"],
    riskLevel: "medium",
    approvalPolicy: "auto",
    content: CONTENT,
    checksum: skillContentChecksum(CONTENT),
    createdAt: "2026-11-09T00:00:00.000Z",
    ...overrides
  };
}

function policy(overrides: Partial<PolicySnapshot> = {}): PolicySnapshot {
  return {
    snapshotId: "pol-1",
    projectId: "proj-1",
    taskId: "task-1",
    executionId: "exec-1",
    worktreeId: "wt-1",
    policyVersion: "1.0.0",
    grantedCapabilities: ["read_file", "search_code", "modify_worktree"],
    allowedPaths: ["/tmp/wt"],
    expiresAt: "2099-01-01T00:00:00.000Z",
    ...overrides
  };
}

test("orders versions and resolves the newest non-deprecated one", async () => {
  const registry = new SkillRegistry();
  await registry.register(manifest({ version: "1.0.0" }));
  await registry.register(manifest({ version: "1.10.0" }));
  await registry.register(manifest({ version: "1.2.0" }));

  assert.deepEqual(registry.listVersions("react-dev"), ["1.10.0", "1.2.0", "1.0.0"]);
  assert.equal(registry.resolve("react-dev").version, "1.10.0");

  registry.deprecate("react-dev", "1.10.0");
  assert.equal(registry.resolve("react-dev").version, "1.2.0");
  assert.equal(registry.isDeprecated("react-dev", "1.10.0"), true);
});

test("treats a pre-release as older than the release", () => {
  assert.ok(compareVersions("2.0.0-rc.1", "2.0.0") < 0);
  assert.ok(compareVersions("2.0.0", "1.99.99") > 0);
  assert.equal(compareVersions("1.2.3", "1.2.3"), 0);
});

test("refuses to overwrite a published version", async () => {
  const registry = new SkillRegistry();
  await registry.register(manifest());

  await assert.rejects(
    () => registry.register(manifest({ description: "edited" })),
    (error: unknown) => error instanceof SkillConflictError
  );
  assert.throws(() => registry.resolve("missing"), (error: unknown) => error instanceof SkillNotFoundError);
});

test("rejects manifests with undeclared permissions or malformed evidence", async () => {
  const registry = new SkillRegistry();

  await assert.rejects(
    () => registry.register(manifest({ permissions: ["root_everything" as never] })),
    (error: unknown) => error instanceof SkillValidationError
  );
  await assert.rejects(() => registry.register(manifest({ checksum: "not-a-hash" })), /checksum/);
  await assert.rejects(() => registry.register(manifest({ version: "v1" })), /semantic version/);
  await assert.rejects(() => registry.register(manifest({ allowedRuntimes: [] })), /allowedRuntimes/);
});

test("the checksum has to match the skill body", async () => {
  const registry = new SkillRegistry();

  // A body that was edited after the checksum was computed is exactly the silent edit that
  // immutable versions exist to prevent, so it is refused.
  await assert.rejects(
    () => registry.register(manifest({ content: `${CONTENT}\nsneaky line\n` })),
    /does not match the skill content/
  );
  await assert.rejects(() => registry.register(manifest({ content: "   " })), /content/);
  await assert.rejects(
    () => registry.register(manifest({ content: "x".repeat(MAX_SKILL_CONTENT_BYTES + 1) })),
    /byte limit/
  );
});

test("keeps the registered skills on disk and reads them back after a restart", async () => {
  const root = await mkdtemp(join(tmpdir(), "codeweaver-skills-"));
  try {
    const store = new ImmutableRecordStore<SkillManifestRecord>(root, "skills");
    const first = new SkillRegistry(store);
    await first.register(manifest());
    await first.register(manifest({ skillId: "deploy", version: "2.0.0", permissions: [] }));

    const second = new SkillRegistry(store);
    await second.load();
    assert.equal(second.list().length, 2);
    // The body comes back with the manifest: a skill the server cannot read is a skill it cannot
    // deliver.
    assert.equal(second.resolve("react-dev").content, CONTENT);
    assert.equal(second.resolve("deploy").version, "2.0.0");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("authorizes against the execution policy and runtime", async () => {
  const registry = new SkillRegistry();
  const skill = manifest();
  await registry.register(skill);

  assert.equal(registry.authorize(skill, { runtimeId: "hermes-local", policy: policy() }).decision, "allow");

  const missing = registry.authorize(skill, {
    runtimeId: "hermes-local",
    policy: policy({ grantedCapabilities: ["read_file"] })
  });
  assert.equal(missing.decision, "deny");
  assert.match(missing.reasons[0]!, /modify_worktree|search_code/);

  const wrongRuntime = registry.authorize(skill, { runtimeId: "hermes-ci", policy: policy() });
  assert.equal(wrongRuntime.decision, "deny");
  assert.match(wrongRuntime.reasons[0]!, /may not run/);
});

test("denies a runtime-specific skill when the runtime of the task is unknown", async () => {
  const registry = new SkillRegistry();
  const skill = manifest();
  await registry.register(skill);

  const result = registry.authorize(skill, { runtimeId: undefined, policy: policy() });
  assert.equal(result.decision, "deny");
  assert.match(result.reasons[0]!, /runtime of this task is unknown/);

  // A skill that names every runtime is still allowed when the runtime is unknown.
  const wildcard = manifest({ skillId: "anywhere", allowedRuntimes: ["*"] });
  await registry.register(wildcard);
  assert.equal(
    registry.authorize(wildcard, { runtimeId: undefined, policy: policy() }).decision,
    "allow"
  );
});

test("requires approval for high-risk, deprecated or explicitly gated skills", async () => {
  const registry = new SkillRegistry();
  const risky = manifest({ skillId: "deploy", riskLevel: "high", permissions: [] });
  await registry.register(risky);
  assert.equal(
    registry.authorize(risky, { runtimeId: "hermes-local", policy: policy({ grantedCapabilities: ["*"] }) }).decision,
    "require_approval"
  );

  const gated = manifest({ skillId: "db-migrate", approvalPolicy: "require_approval", permissions: [] });
  await registry.register(gated);
  assert.equal(
    registry.authorize(gated, { runtimeId: "hermes-local", policy: policy({ grantedCapabilities: ["*"] }) }).decision,
    "require_approval"
  );

  const old = manifest({ version: "0.9.0" });
  await registry.register(old);
  registry.deprecate("react-dev", "0.9.0");
  assert.equal(
    registry.authorize(old, { runtimeId: "hermes-local", policy: policy() }).decision,
    "require_approval"
  );
});

test("denies authorization when the policy snapshot expired", async () => {
  const registry = new SkillRegistry();
  const skill = manifest();
  await registry.register(skill);

  const result = registry.authorize(skill, {
    runtimeId: "hermes-local",
    policy: policy({ expiresAt: "2000-01-01T00:00:00.000Z" })
  });

  assert.equal(result.decision, "deny");
  assert.match(result.reasons[0]!, /expired/);
});

test("binds allowed skills immutably and refuses denied ones", async () => {
  const registry = new SkillRegistry();
  const store = new SkillBindingStore();
  const skill = manifest();
  await registry.register(skill);
  const allowed = registry.authorize(skill, { runtimeId: "hermes-local", policy: policy() });

  const binding = await store.bind({
    executionId: "exec-1",
    manifest: skill,
    policyId: "pol-1",
    authorization: allowed,
    idFactory: () => "bind-1"
  });
  assert.equal(binding.skillVersion, "1.0.0");
  assert.equal(Object.isFrozen(binding), true);

  const again = await store.bind({
    executionId: "exec-1",
    manifest: skill,
    policyId: "pol-1",
    authorization: allowed
  });
  assert.equal(again.id, "bind-1");
  assert.equal(store.forExecution("exec-1").length, 1);

  await assert.rejects(
    store.bind({
      executionId: "exec-2",
      manifest: skill,
      policyId: "pol-1",
      authorization: { decision: "deny", reasons: ["nope"] }
    }),
    (error: unknown) => error instanceof SkillBindingError
  );

  const disabled = await store.setEnabled("exec-1", "react-dev", "1.0.0", false);
  assert.equal(disabled.enabled, false);
  assert.equal(store.forExecution("exec-1")[0]!.enabled, false);
});

test("restores bindings from the event log after a restart", async () => {
  const root = await mkdtemp(join(tmpdir(), "codeweaver-bindings-"));
  try {
    const events = new ImmutableRecordStore<SkillBindingEvent>(root, "bindings");
    const skill = manifest();
    const first = new SkillBindingStore(events);
    await first.bind({
      executionId: "exec-1",
      manifest: skill,
      policyId: "pol-1",
      authorization: { decision: "allow", reasons: [] }
    });
    await first.setEnabled("exec-1", "react-dev", "1.0.0", false);

    const second = new SkillBindingStore(events);
    await second.load();
    const restored = second.forExecution("exec-1");
    assert.equal(restored.length, 1);
    assert.equal(restored[0]!.enabled, false);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
