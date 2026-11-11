import { spawn } from "node:child_process";

export interface CommandOutcome {
  code: number | null;
  stdout: string;
  stderr: string;
  timedOut: boolean;
}

export interface CommandRunner {
  run(command: string, args: string[], options?: { cwd?: string; timeoutMs?: number }): Promise<CommandOutcome>;
}

/** Evidence has to come from a real process, so tests and checks run without a shell wrapper. */
export class SpawnCommandRunner implements CommandRunner {
  run(
    command: string,
    args: string[],
    options: { cwd?: string; timeoutMs?: number } = {}
  ): Promise<CommandOutcome> {
    const timeoutMs = options.timeoutMs ?? 60_000;
    return new Promise(resolveOutcome => {
      let stdout = "";
      let stderr = "";
      let settled = false;
      let timedOut = false;
      const child = spawn(command, args, {
        cwd: options.cwd,
        stdio: ["ignore", "pipe", "pipe"],
        windowsHide: true
      });
      const timer = setTimeout(() => {
        timedOut = true;
        child.kill("SIGKILL");
      }, timeoutMs);

      child.stdout.on("data", chunk => { stdout += String(chunk); });
      child.stderr.on("data", chunk => { stderr += String(chunk); });
      child.on("error", error => finish(null, error.message));
      child.on("close", code => finish(code, undefined));

      function finish(code: number | null, error: string | undefined): void {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        resolveOutcome({
          code,
          stdout,
          stderr: error ? `${stderr}${error}` : stderr,
          timedOut
        });
      }
    });
  }
}
