import { spawnCapture } from "../security/boundary-provider.js";
import type { CheckPlanningProvider } from "../security/boundary-provider.js";
import type { CommandOutcome, CommandRunner, CommandRunOptions } from "./command-runner.js";

/**
 * Runs an acceptance check inside the host path boundary.
 *
 * The check is a real process, and this runner is what puts it behind the
 * boundary: the work happens in a guest where the host's directories are not
 * mounted, where the network has no route, and where the process is not root.
 * The command the task declared is the same command it would run on the host -
 * only the place it runs in is different, which is the whole point.
 *
 * Two rules keep this honest:
 *
 *  - **A boundary that cannot be established is not a boundary.** When the
 *    guest cannot be reached the check does not run on the host instead: it is
 *    reported as not run, so the caller can block it (the same rule the
 *    sandbox limits follow, D-074).
 *  - **The record says where the check ran.** A check that ran behind the
 *    boundary and a check that ran on the host look the same from the exit
 *    code alone, so the outcome carries the boundary it ran in.
 */
export interface BoundaryRunnerOptions {
  /** The boundary the check runs in. */
  provider: CheckPlanningProvider;
  /** How long the check may take. */
  timeoutMs?: number;
}

/** What the boundary did with the check. */
export interface BoundaryOutcome {
  /** Whether the check really ran inside the boundary. */
  established: boolean;
  /** Which boundary it was, in words a reader can check. */
  description: string;
  /** Why the boundary could not be established, when it could not. */
  error?: string;
}

export class BoundaryCommandRunner implements CommandRunner {
  constructor(private readonly options: BoundaryRunnerOptions) {}

  async run(
    command: string,
    args: string[],
    options: CommandRunOptions = {}
  ): Promise<CommandOutcome> {
    const workspace = options.cwd;
    if (!workspace) {
      return notEstablished(
        "the check has no working directory, so there is no workspace to mount inside the boundary"
      );
    }

    const script = checkScript(command, args);
    let plan: { command: string; args: string[] };
    try {
      plan = await this.options.provider.planCheck(workspace, script, { mode: "inside" });
    } catch (error) {
      return notEstablished(
        `the boundary could not be established: ${error instanceof Error ? error.message : String(error)}`
      );
    }

    const timeoutMs = options.timeoutMs ?? this.options.timeoutMs ?? 120_000;
    const outcome = await spawnCapture(plan.command, plan.args, timeoutMs);
    let description = `${this.options.provider.backend} boundary`;
    try {
      description = await this.options.provider.describe();
    } catch {
      // The description is a nicety; the check still ran (or did not) and the
      // exit code says which.
    }
    return {
      code: outcome.exitCode,
      stdout: outcome.stdout,
      stderr: outcome.stderr,
      timedOut: outcome.timedOut,
      boundary: { established: true, description }
    };
  }
}

/**
 * The script that runs the declared command inside the boundary.
 *
 * The command is quoted rather than pasted: a check that declares an argument
 * with a space in it must not become two arguments on the way in.
 */
export function checkScript(command: string, args: string[]): string {
  const parts = [command, ...args].map(quote);
  return ["cd /workspace", `exec ${parts.join(" ")}`, ""].join("\n");
}

/** One shell word. */
function quote(value: string): string {
  return `'${value.replace(/'/g, "'\\''")}'`;
}

function notEstablished(error: string): CommandOutcome {
  return {
    code: null,
    stdout: "",
    stderr: error,
    timedOut: false,
    boundary: { established: false, description: "no boundary", error }
  };
}
