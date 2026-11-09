import { randomUUID } from "node:crypto";
import { mkdir } from "node:fs/promises";
import { isAbsolute, join, relative, resolve } from "node:path";
import { GitCommandError, gitOrThrow, type GitRunner } from "./git-runner.js";

export interface Worktree {
  path: string;
  baseSha: string;
  createdAt: string;
}

export class BaseShaMismatchError extends Error {
  constructor(expected: string, actual: string) {
    super(`Worktree HEAD ${actual} does not match the pinned base SHA ${expected}`);
    this.name = "BaseShaMismatchError";
  }
}

/**
 * Mode B isolation: every execution gets its own worktree pinned to a base SHA, so an agent that
 * writes files directly can never touch the shared checkout (design 7.2 / AT-102).
 */
export class WorktreeManager {
  constructor(
    private readonly git: GitRunner,
    private readonly root: string,
    private readonly now: () => Date = () => new Date()
  ) {}

  async create(baseSha: string, label?: string): Promise<Worktree> {
    const resolved = await this.resolveCommit(baseSha);
    const directory = join(this.root, `wt-${label ? `${sanitize(label)}-` : ""}${randomUUID().slice(0, 8)}`);
    assertInside(this.root, directory);
    await mkdir(this.root, { recursive: true });

    await gitOrThrow(this.git, ["worktree", "add", "--detach", directory, resolved]);
    const head = await this.head(directory);
    if (head.toLowerCase() !== resolved.toLowerCase()) {
      throw new BaseShaMismatchError(resolved, head);
    }
    return { path: directory, baseSha: resolved, createdAt: this.now().toISOString() };
  }

  async remove(worktreePath: string): Promise<void> {
    assertInside(this.root, worktreePath);
    const result = await this.git.run(["worktree", "remove", "--force", worktreePath]);
    // A worktree that is already gone is not an error worth failing the caller over.
    if (result.code !== 0 && !/not a working tree|does not exist/i.test(result.stderr)) {
      throw new GitCommandError(["worktree", "remove", "--force", worktreePath], result);
    }
  }

  async list(): Promise<string[]> {
    const result = await gitOrThrow(this.git, ["worktree", "list", "--porcelain"]);
    return result.stdout
      .split(/\r?\n/)
      .filter(line => line.startsWith("worktree "))
      .map(line => line.slice("worktree ".length).trim())
      .filter(path => path && path !== resolve(this.root) && isInside(this.root, path));
  }

  async head(worktreePath: string): Promise<string> {
    const result = await gitOrThrow(this.git, ["rev-parse", "HEAD"], { cwd: worktreePath });
    return result.stdout.trim();
  }

  /** AT-105: refuse to apply anything when the worktree drifted from the snapshot's base SHA. */
  async assertBaseSha(worktreePath: string, expected: string): Promise<void> {
    const actual = await this.head(worktreePath);
    const want = expected.toLowerCase();
    if (!actual.toLowerCase().startsWith(want)) throw new BaseShaMismatchError(expected, actual);
  }

  private async resolveCommit(ref: string): Promise<string> {
    if (!/^[0-9a-f]{4,64}$/i.test(ref)) {
      throw new Error(`Base SHA must be a hexadecimal commit id, received '${ref}'`);
    }
    const result = await gitOrThrow(this.git, ["rev-parse", "--verify", `${ref}^{commit}`]);
    return result.stdout.trim();
  }
}

export function isInside(root: string, candidate: string): boolean {
  const rel = relative(resolve(root), resolve(candidate));
  if (rel === "") return true;
  if (isAbsolute(rel)) return false;
  return !rel.split(/[\\/]/).includes("..");
}

function assertInside(root: string, candidate: string): void {
  if (!isInside(root, candidate)) {
    throw new Error(`Path '${candidate}' is outside the worktree root '${root}'`);
  }
}

function sanitize(label: string): string {
  return label.replace(/[^a-zA-Z0-9._-]/g, "-").slice(0, 32);
}
