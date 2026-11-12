import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import {
  classifyLimitHit,
  normalizeSandboxLimits,
  validateSandboxLimits,
  type SandboxLimitHit,
  type SandboxLimits,
  type ResolvedSandboxLimits,
  type SandboxRunResult
} from "./sandbox-limits.js";
import type { CommandOutcome, CommandRunOptions, CommandRunner } from "./command-runner.js";

/** What the sandbox did with one check. */
export interface SandboxOutcome {
  /** Whether the limits were actually in force. A check that ran without them is not sandboxed. */
  established: boolean;
  /** The limits that were in force. */
  limits: ResolvedSandboxLimits;
  /** The limit that stopped the work, when one did. */
  limitHit?: SandboxLimitHit;
  /** The sentence that names the limit. */
  reason?: string;
  /** The most memory the job committed, according to the kernel. */
  peakJobMemoryBytes?: number;
  /** Why the sandbox could not be established, when it could not. */
  error?: string;
}

/** The helper that applies the limits; it is a script because the job object API is a Windows API. */
const DEFAULT_SCRIPT = fileURLToPath(new URL("../../scripts/sandbox-run.ps1", import.meta.url));

/**
 * Runs a check inside a Windows job object with resource limits.
 *
 * The helper is a PowerShell script with the job object API compiled into it, because Node has no
 * way to create one. The arguments travel in an environment variable rather than on the command
 * line: a command line would have to quote them twice, and that is where arguments get lost.
 *
 * When the sandbox cannot be established the check does not run unsandboxed. The outcome says the
 * sandbox was unavailable, the exit code stays unknown, and the Evaluation Engine blocks the
 * check: a limit that could not be applied is not a limit (the same rule the worktree scope
 * follows).
 */
export class SandboxCommandRunner implements CommandRunner {
  constructor(
    private readonly options: { scriptPath?: string; limits?: SandboxLimits; powershell?: string } = {}
  ) {}

  async run(
    command: string,
    args: string[],
    options: CommandRunOptions = {}
  ): Promise<CommandOutcome> {
    const limits = normalizeSandboxLimits(options.limits ?? this.options.limits);
    const violations = validateSandboxLimits(limits);
    if (violations.length > 0) {
      return unavailable(limits, `the sandbox limits are unusable: ${violations.join("; ")}`);
    }
    if (process.platform !== "win32") {
      return unavailable(limits, `the sandbox is only available on Windows (this is ${process.platform})`);
    }
    const script = this.options.scriptPath ?? DEFAULT_SCRIPT;
    if (!existsSync(script)) {
      return unavailable(limits, `the sandbox helper is missing: ${script}`);
    }

    const timeoutMs = options.timeoutMs ?? 60_000;
    const result = await this.invoke(script, command, args, limits, timeoutMs, options.cwd);
    if (!result || result.error) {
      return unavailable(limits, result?.error ?? "the sandbox helper did not answer");
    }
    if (!result.started) {
      return unavailable(limits, "the sandbox helper did not start the process");
    }

    const verdict = classifyLimitHit(result, limits);
    return {
      code: result.exitCodeKnown ? result.exitCode : null,
      stdout: result.stdout ?? "",
      stderr: result.stderr ?? "",
      timedOut: result.timedOut === true,
      sandbox: {
        established: true,
        limits,
        ...(verdict ? { limitHit: verdict.hit, reason: verdict.reason } : {}),
        ...(result.peakJobMemoryBytes !== undefined
          ? { peakJobMemoryBytes: result.peakJobMemoryBytes }
          : {})
      }
    };
  }

  private invoke(
    script: string,
    command: string,
    args: string[],
    limits: SandboxLimits,
    timeoutMs: number,
    cwd?: string
  ): Promise<SandboxRunResult | undefined> {
    const powershell = this.options.powershell ?? "powershell";
    const helperArgs = [
      "-ExecutionPolicy",
      "Bypass",
      "-NoProfile",
      "-File",
      script,
      "-Command",
      command,
      "-MemoryMb",
      String(limits.memoryMb ?? 0),
      "-CpuRatePercent",
      String(limits.cpuRatePercent ?? 0),
      "-ProcessLimit",
      String(limits.processLimit ?? 0),
      "-TimeoutMs",
      String(timeoutMs)
    ];

    return new Promise(resolve => {
      let stdout = "";
      let stderr = "";
      let settled = false;
      const child = spawn(powershell, helperArgs, {
        cwd,
        stdio: ["ignore", "pipe", "pipe"],
        windowsHide: true,
        env: { ...process.env, CW_SANDBOX_ARGS: JSON.stringify(args) }
      });
      const timer = setTimeout(() => {
        child.kill("SIGKILL");
      }, timeoutMs + 30_000);

      child.stdout.on("data", chunk => { stdout += String(chunk); });
      child.stderr.on("data", chunk => { stderr += String(chunk); });
      child.on("error", error => finish(undefined, error.message));
      child.on("close", () => {
        const line = stdout.trim().split(/\r?\n/).filter(Boolean).pop() ?? "";
        try {
          finish(JSON.parse(line) as SandboxRunResult, "");
        } catch {
          finish(undefined, `the sandbox helper did not answer with a result: ${stderr.trim() || stdout.trim()}`);
        }
      });

      function finish(result: SandboxRunResult | undefined, error: string): void {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        resolve(result ? { ...result, error: result.error || error } : undefined);
      }
    });
  }
}

function unavailable(limits: ResolvedSandboxLimits, error: string): CommandOutcome {
  return {
    code: null,
    stdout: "",
    stderr: error,
    timedOut: false,
    sandbox: { established: false, limits, error }
  };
}
