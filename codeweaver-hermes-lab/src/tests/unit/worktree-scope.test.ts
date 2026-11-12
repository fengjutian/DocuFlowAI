import test from "node:test";
import assert from "node:assert/strict";
import { resolve } from "node:path";
import {
  canonicalPath,
  owningWorktree,
  scopeViolation,
  scopeWorktree,
  type WorktreeScope
} from "../../policy/worktree-scope.js";

/**
 * The scope rule, on its own.
 *
 * A directory boundary is not a worktree boundary: another worktree of the same repository can sit
 * inside the directory an execution runs in. These tests pin the rule that keeps the two apart —
 * the worktree that owns a path has to be the execution's own worktree — and they pin the cases
 * where it must NOT fire, because a scope that refuses everything is not isolation, it is a
 * broken tool.
 */

const repo = resolve("repo");
const checkout = resolve(repo);
const own = resolve(repo, "wt-a");
const sibling = resolve(repo, "wt-b");
const nested = resolve(repo, "wt-a", "nested-wt");

function scope(overrides: Partial<WorktreeScope> = {}): WorktreeScope {
  return {
    own,
    worktrees: [checkout, own, sibling, nested],
    ...overrides
  };
}

test("a path inside the execution's own worktree is in scope", () => {
  assert.equal(scopeViolation(resolve(own, "src", "index.ts"), scope()), undefined);
});

test("a path inside a sibling worktree is out of scope", () => {
  const violation = scopeViolation(resolve(sibling, "secret.txt"), scope());
  assert.ok(violation, "a sibling worktree is not this execution's worktree");
  assert.match(violation, /not this execution's worktree/);
});

test("a path inside a worktree nested in the execution's own worktree is out of scope", () => {
  const violation = scopeViolation(resolve(nested, "secret.txt"), scope());
  assert.ok(violation, "a nested worktree is a different workspace, however it is nested");
  assert.match(violation, /not this execution's worktree/);
});

test("the deepest worktree that contains a path is the one that owns it", () => {
  assert.equal(owningWorktree(resolve(nested, "secret.txt"), scope().worktrees), nested);
  assert.equal(owningWorktree(resolve(own, "src", "index.ts"), scope().worktrees), own);
  assert.equal(owningWorktree(resolve(checkout, "README.md"), scope().worktrees), checkout);
});

test("an execution running in a subdirectory of a checkout belongs to that checkout", () => {
  const subdirectory = resolve(checkout, "packages", "app");
  assert.equal(scopeWorktree(scope({ own: subdirectory })), checkout);
  assert.equal(
    scopeViolation(resolve(subdirectory, "index.ts"), scope({ own: subdirectory })),
    undefined,
    "a task in a subdirectory of a checkout may read its own files"
  );
});

test("a path no worktree owns is not a cross-worktree access", () => {
  assert.equal(scopeViolation(resolve("elsewhere", "file.txt"), scope()), undefined);
  assert.equal(owningWorktree(resolve("elsewhere", "file.txt"), scope().worktrees), undefined);
});

test("the execution's own worktree is the one that holds its directory", () => {
  assert.equal(scopeWorktree(scope()), own);
  assert.equal(scopeWorktree(scope({ own: nested })), nested);
});

test("canonicalPath resolves what exists and keeps what does not", async () => {
  const missing = resolve("definitely", "missing", "file.txt");
  assert.equal(await canonicalPath(missing), missing);
  assert.ok((await canonicalPath(repo)).length > 0);
});
