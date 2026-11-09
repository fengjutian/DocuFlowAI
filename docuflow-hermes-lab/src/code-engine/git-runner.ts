import { spawn } from "node:child_process";

export interface CommandResult {
  code: number;
  stdout: string;
  stderr: string;
}

export interface GitRunOptions {
  cwd?: string;
  timeoutMs?: number;
  /** Written to the child's stdin, so patches never have to touch the filesystem. */
  stdin?: string;
}

/** Injectable so the change pipeline can be tested without a repository on disk. */
export interface GitRunner {
  run(args: string[], options?: GitRunOptions): Promise<CommandResult>;
}

export class GitCommandError extends Error {
  constructor(
    public readonly args: string[],
    public readonly result: CommandResult
  ) {
    super(`git ${args.join(" ")} failed with exit code ${result.code}: ${result.stderr.trim()}`);
    this.name = "GitCommandError";
  }
}

/** Spawns git without a shell so patch paths and commit messages stay single argv entries. */
export class SpawnGitRunner implements GitRunner {
  constructor(private readonly gitBinary = "git") {}

  run(args: string[], options: GitRunOptions = {}): Promise<CommandResult> {
    const timeoutMs = options.timeoutMs ?? 60_000;
    return new Promise(resolveResult => {
      let stdout = "";
      let stderr = "";
      let settled = false;
      const child = spawn(this.gitBinary, args, {
        cwd: options.cwd,
        stdio: ["pipe", "pipe", "pipe"],
        windowsHide: true
      });
      const timer = setTimeout(() => {
        child.kill("SIGKILL");
      }, timeoutMs);
      child.stdin.on("error", () => undefined);
      child.stdin.end(options.stdin ?? "");
      child.stdout.on("data", chunk => { stdout += String(chunk); });
      child.stderr.on("data", chunk => { stderr += String(chunk); });
      child.on("error", error => {
        finish(null, error.message);
      });
      child.on("close", code => {
        finish(code, undefined);
      });

      function finish(code: number | null, spawnError: string | undefined): void {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        resolveResult({ code: code ?? 1, stdout, stderr: spawnError ? `${stderr}${spawnError}` : stderr });
      }
    });
  }
}

export async function gitOrThrow(
  git: GitRunner,
  args: string[],
  options?: GitRunOptions
): Promise<CommandResult> {
  const result = await git.run(args, options);
  if (result.code !== 0) throw new GitCommandError(args, result);
  return result;
}
