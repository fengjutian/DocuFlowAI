import { realpath } from "node:fs/promises";
import { resolve } from "node:path";
import { isWithin, samePath } from "./permission-policy.js";

/**
 * The file scope of one execution, expressed in worktrees rather than in directories.
 *
 * A directory boundary is not enough. Two worktrees of the same repository can sit inside one
 * another: a worktree created inside a checkout, or a worktree root that lives under the checkout
 * the execution runs in. An execution that may touch "its own directory" would then reach straight
 * into the other worktree — reading its files and writing its files, which is exactly what the
 * isolation is supposed to prevent.
 *
 * The scope therefore names worktrees, and the rule is one line: the worktree that owns a path must
 * be the execution's own worktree. Both the read path and the write path ask the same question
 * through the same function, so the two can never disagree about what is in scope.
 */
export interface WorktreeScope {
  /** Canonical path of the directory the execution runs in. */
  own: string;
  /**
   * Canonical paths of every worktree of the repository.
   *
   * Canonical means the real path: a Windows temp directory may be reached through a short (8.3)
   * name and through its long name, and the two have to compare equal.
   */
  worktrees: string[];
}

/**
 * The deepest listed worktree that contains `path`, or undefined when no worktree does.
 *
 * "Deepest" matters: a worktree inside a checkout is contained by the checkout as well, and the
 * narrower one is the one that actually owns the file.
 */
export function owningWorktree(path: string, worktrees: readonly string[]): string | undefined {
  let owner: string | undefined;
  for (const worktree of worktrees) {
    if (!isWithin(path, worktree)) continue;
    if (!owner || worktree.length > owner.length) owner = worktree;
  }
  return owner;
}

/**
 * The worktree the execution's own directory belongs to.
 *
 * It is the execution's own worktree when it has one, and otherwise the worktree that holds it —
 * a task may run in a subdirectory of a checkout, and that subdirectory is still inside the
 * checkout's worktree.
 */
export function scopeWorktree(scope: WorktreeScope): string {
  return owningWorktree(scope.own, scope.worktrees) ?? scope.own;
}

/**
 * The reason a path is out of scope, or undefined when it is in scope.
 *
 * A path no worktree owns is not a cross-worktree access; the directory boundary is what governs it.
 */
export function scopeViolation(target: string, scope: WorktreeScope): string | undefined {
  const owner = owningWorktree(target, scope.worktrees);
  if (!owner) return undefined;
  const own = scopeWorktree(scope);
  if (samePath(owner, own)) return undefined;
  return `Path '${target}' is inside worktree '${owner}', which is not this execution's worktree '${own}'`;
}

/**
 * The reason a path is out of scope, judged on the path as asked for and on the path it really is.
 *
 * A Windows temp directory can be reached through a short (8.3) name and through its long name,
 * and a symlink inside a worktree points at whatever it points at. One of the two forms is the one
 * the scope knows about, so both are asked.
 */
export async function scopeViolationFor(target: string, scope: WorktreeScope): Promise<string | undefined> {
  return scopeViolation(target, scope) ?? scopeViolation(await canonicalPath(target), scope);
}

/**
 * Canonical path: the real path when it exists, the resolved path when it does not.
 *
 * Resolving the real path is what makes a symlink inside a worktree point at the worktree it
 * really leads to, instead of the path that was asked for.
 */
export async function canonicalPath(path: string): Promise<string> {
  try {
    return await realpath(path);
  } catch {
    return resolve(path);
  }
}
