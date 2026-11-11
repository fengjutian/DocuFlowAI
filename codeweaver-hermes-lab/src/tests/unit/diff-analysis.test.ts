import test from "node:test";
import assert from "node:assert/strict";
import { extractPatchFiles, extractRemovedFiles, hashPatch, proposeChange } from "../../code-engine/diff-analysis.js";

const simplePatch = [
  "diff --git a/src/index.ts b/src/index.ts",
  "--- a/src/index.ts",
  "+++ b/src/index.ts",
  "@@ -1 +1 @@",
  "-export const value = 1;",
  "+export const value = 2;",
  ""
].join("\n");

test("extracts file paths from a unified diff", () => {
  assert.deepEqual(extractPatchFiles(simplePatch), ["src/index.ts"]);
});

test("hashes the same patch to the same value", () => {
  assert.equal(hashPatch(simplePatch), hashPatch(simplePatch));
  assert.notEqual(hashPatch(simplePatch), hashPatch(`${simplePatch}\n`));
});

test("accepts a plain source change without requiring approval", () => {
  const proposal = proposeChange(simplePatch);

  assert.deepEqual(proposal.files, ["src/index.ts"]);
  assert.equal(proposal.blocked, false);
  assert.equal(proposal.requiresApproval, false);
  assert.equal(proposal.diffHash.length, 64);
});

test("blocks a patch that escapes the worktree with '..'", () => {
  const patch = [
    "diff --git a/../../etc/passwd b/../../etc/passwd",
    "--- a/../../etc/passwd",
    "+++ b/../../etc/passwd",
    "@@ -1 +1 @@",
    "-root",
    "+owned",
    ""
  ].join("\n");
  const proposal = proposeChange(patch);

  assert.equal(proposal.blocked, true);
  assert.ok(proposal.risks.some(risk => risk.code === "path_escape" && risk.blocked));
});

test("blocks absolute paths and writes into git metadata", () => {
  const absolute = proposeChange("--- /etc/passwd\n+++ /etc/passwd\n@@ -1 +1 @@\n-a\n+b\n");
  assert.ok(absolute.risks.some(risk => risk.code === "absolute_path" && risk.blocked));

  const gitMeta = proposeChange(
    "diff --git a/.git/config b/.git/config\n--- a/.git/config\n+++ b/.git/config\n@@ -1 +1 @@\n-a\n+b\n"
  );
  assert.ok(gitMeta.risks.some(risk => risk.code === "git_metadata" && risk.blocked));
});

test("requires approval for dependency, secret, migration and deletion changes", () => {
  const dependency = proposeChange(
    "diff --git a/package.json b/package.json\n--- a/package.json\n+++ b/package.json\n@@ -1 +1 @@\n-a\n+b\n"
  );
  assert.equal(dependency.requiresApproval, true);
  assert.ok(dependency.risks.some(risk => risk.code === "dependency_change"));

  const secret = proposeChange(
    "diff --git a/.env b/.env\n--- a/.env\n+++ b/.env\n@@ -1 +1 @@\n-A=1\n+A=2\n"
  );
  assert.ok(secret.risks.some(risk => risk.code === "secret_file"));

  const migration = proposeChange(
    "diff --git a/migrations/001_init.sql b/migrations/001_init.sql\n--- a/migrations/001_init.sql\n+++ b/migrations/001_init.sql\n@@ -1 +1 @@\n-a\n+b\n"
  );
  assert.ok(migration.risks.some(risk => risk.code === "migration"));
});

test("flags deletions, binary patches and oversized diffs", () => {
  const deletion = proposeChange(
    "diff --git a/old.ts b/old.ts\ndeleted file mode 100644\n--- a/old.ts\n+++ /dev/null\n@@ -1 +0,0 @@\n-gone\n"
  );
  assert.ok(deletion.risks.some(risk => risk.code === "deletion"));

  const binary = proposeChange("diff --git a/logo.png b/logo.png\nBinary files a/logo.png and b/logo.png differ\n");
  assert.ok(binary.risks.some(risk => risk.code === "binary_patch"));

  const large = proposeChange(simplePatch, { maxPatchChars: 10 });
  assert.ok(large.risks.some(risk => risk.code === "large_patch"));
});

test("flags a removal that carries no deletion marker", () => {
  // `git apply` deletes the file here, so the risk analysis has to see it too: design 8 puts
  // "delete file" at P2, which means human approval, not a silent apply.
  const removal = proposeChange(
    "diff --git a/src/keep.ts b/src/keep.ts\n--- a/src/keep.ts\n+++ /dev/null\n@@ -1 +0,0 @@\n-gone\n"
  );
  assert.ok(
    removal.risks.some(risk => risk.code === "deletion" && risk.blocked === false),
    "the removal needs approval"
  );
  assert.equal(removal.requiresApproval, true);
  assert.equal(removal.blocked, false);
  assert.deepEqual(extractRemovedFiles(
    "diff --git a/src/keep.ts b/src/keep.ts\n--- a/src/keep.ts\n+++ /dev/null\n@@ -1 +0,0 @@\n-gone\n"
  ), ["src/keep.ts"]);
});

test("a new file is not a removal", () => {
  const creation = proposeChange(
    "diff --git a/doc/new.md b/doc/new.md\nnew file mode 100644\n--- /dev/null\n+++ b/doc/new.md\n@@ -0,0 +1 @@\n+hello\n"
  );
  assert.deepEqual(extractRemovedFiles(
    "diff --git a/doc/new.md b/doc/new.md\nnew file mode 100644\n--- /dev/null\n+++ b/doc/new.md\n@@ -0,0 +1 @@\n+hello\n"
  ), []);
  assert.ok(!creation.risks.some(risk => risk.code === "deletion"));
  assert.equal(creation.requiresApproval, false);
});

test("blocks an empty patch because it authorizes nothing", () => {
  const proposal = proposeChange("");

  assert.equal(proposal.blocked, true);
  assert.ok(proposal.risks.some(risk => risk.code === "no_files"));
});
