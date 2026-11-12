/**
 * Resource limits for the processes CodeWeaver starts on an agent's behalf.
 *
 * The Phase 0 checklist asked for a sandbox with memory, CPU and process limits. Docker was the
 * sandbox that answered it, and Docker is gone (doc/HERMES_NATIVE_PLAN.md) — but the limits are
 * not a Docker feature. On Windows they are a job object, and the three of them mean exactly what
 * they meant in the container: how much memory the whole job may commit, how much of the machine's
 * processors it may use, and how many processes may live in it at the same time.
 *
 * Two rules keep this honest:
 *
 *  1. A limit that was hit is named. A check that dies because it ran out of memory has to say
 *     so, because "the check failed" and "the check was stopped by the sandbox" are different
 *     facts and the agent can only act on one of them.
 *  2. A limit that cannot be established is not a limit. If the sandbox cannot be set up, the
 *     check does not run unsandboxed: it is blocked, the same way an unverifiable worktree scope
 *     is refused (D-074) rather than treated as empty.
 */

/** The limits one sandboxed process group runs under. */
export interface SandboxLimits {
  /** The most memory the whole job may commit, in megabytes. */
  memoryMb?: number;
  /**
   * The share of the machine's processors the job may use, in percent.
   *
   * The unit is percent of *all* processors, not percent of one core: on a 22-core machine a cap
   * of 2% is about half a core, and a single-threaded check only feels it below 100/22 percent.
   */
  cpuRatePercent?: number;
  /** How many processes may live in the job at the same time, the check itself included. */
  processLimit?: number;
}

/** The limits a check runs under, with every number settled. */
export interface ResolvedSandboxLimits {
  memoryMb: number;
  cpuRatePercent: number;
  processLimit: number;
}

/** The limits a check runs under when the task does not name its own. */
export const DEFAULT_SANDBOX_LIMITS: SandboxLimits = {
  memoryMb: 1024,
  cpuRatePercent: 25,
  processLimit: 64
};

/** Which limit stopped the work. */
export type SandboxLimitHit = "memory" | "processes" | "cpu" | "timeout";

/** What the sandbox helper reports back about one run. */
export interface SandboxRunResult {
  /** Whether the process was started at all. */
  started: boolean;
  /** Whether the run was stopped because it took too long. */
  timedOut: boolean;
  /** The process exit code, when the sandbox knows it. */
  exitCode: number | null;
  /** Whether the exit code is the process's own and not a guess. */
  exitCodeKnown: boolean;
  /** The most memory the job committed, according to the kernel. */
  peakJobMemoryBytes: number;
  /** The memory limit that was in force, in bytes. */
  jobMemoryLimitBytes: number;
  /** The process limit that was in force. */
  processLimit: number;
  /** The limits the kernel recorded as violated, for example `job_memory`. */
  limitViolation: string;
  /** Why the sandbox itself could not do its job, if it could not. */
  error: string;
  stdout: string;
  stderr: string;
}

/** A limit that was hit, and the sentence that explains it. */
export interface SandboxLimitVerdict {
  hit: SandboxLimitHit;
  reason: string;
}

/** Violations that make the limits unusable. An empty array means they can be applied. */
export function validateSandboxLimits(limits: SandboxLimits): string[] {
  const violations: string[] = [];
  if (limits.memoryMb !== undefined) {
    if (!Number.isFinite(limits.memoryMb) || limits.memoryMb <= 0) {
      violations.push("memoryMb must be a positive number");
    }
  }
  if (limits.cpuRatePercent !== undefined) {
    if (!Number.isFinite(limits.cpuRatePercent) || limits.cpuRatePercent <= 0 || limits.cpuRatePercent > 100) {
      violations.push("cpuRatePercent must be a positive number of at most 100");
    }
  }
  if (limits.processLimit !== undefined) {
    if (!Number.isInteger(limits.processLimit) || limits.processLimit <= 0) {
      violations.push("processLimit must be a positive integer");
    }
  }
  return violations;
}

/** The limits a check actually runs under: what it asked for, and the default for the rest. */
export function normalizeSandboxLimits(limits?: SandboxLimits): ResolvedSandboxLimits {
  return {
    memoryMb: limits?.memoryMb ?? DEFAULT_SANDBOX_LIMITS.memoryMb ?? 1024,
    cpuRatePercent: limits?.cpuRatePercent ?? DEFAULT_SANDBOX_LIMITS.cpuRatePercent ?? 25,
    processLimit: limits?.processLimit ?? DEFAULT_SANDBOX_LIMITS.processLimit ?? 64
  };
}

/**
 * Which limit stopped the run, if any.
 *
 * The kernel's own record comes first: it is the only source that knows what the sandbox did.
 * The signatures in the child's output are the second source, because a limit that stops a
 * *child* (a spawn that runs out of quota) leaves nothing in the job's violation record.
 */
export function classifyLimitHit(result: SandboxRunResult, limits: ResolvedSandboxLimits): SandboxLimitVerdict | undefined {
  const violation = result.limitViolation.toLowerCase();
  if (violation.includes("job_memory")) {
    return { hit: "memory", reason: memoryReason(limits) };
  }
  if (violation.includes("active_process")) {
    return { hit: "processes", reason: processReason(limits) };
  }
  if (violation.includes("cpu_rate")) {
    return { hit: "cpu", reason: cpuReason(limits) };
  }

  const stderr = result.stderr ?? "";
  // The child that could not get memory says so, in the words of the runtime that refused it.
  if (/array buffer allocation failed|out of memory|not enough memory|insufficient memory/i.test(stderr)) {
    return { hit: "memory", reason: memoryReason(limits) };
  }
  // Windows answers a spawn beyond the job's process limit with ERROR_NOT_ENOUGH_QUOTA (1816),
  // which surfaces as errno -4094 on the child.
  if (/-4094|1816|not enough quota/i.test(stderr)) {
    return { hit: "processes", reason: processReason(limits) };
  }
  if (result.timedOut) {
    return { hit: "timeout", reason: "the check took longer than the sandbox allows" };
  }
  return undefined;
}

function memoryReason(limits: ResolvedSandboxLimits): string {
  return `the check exceeded the memory limit of ${limits.memoryMb} MB`;
}

function processReason(limits: ResolvedSandboxLimits): string {
  return `the check exceeded the process limit of ${limits.processLimit} processes`;
}

function cpuReason(limits: ResolvedSandboxLimits): string {
  return `the check exceeded the CPU limit of ${limits.cpuRatePercent}% of the machine`;
}
