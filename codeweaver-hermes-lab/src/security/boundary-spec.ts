import { existsSync, statSync } from "node:fs";
import { isAbsolute, relative, resolve } from "node:path";

/**
 * Description of the isolation boundary the security probes run inside.
 *
 * A boundary is only evidence if it is real. `backend: "none"` exists so a caller can state that
 * nothing was isolated, and validating such a spec always fails: the runner must not report
 * "restricted file denied" when there was nothing to deny it.
 *
 * The backends are the native ones, not Docker: the project decided not to depend on a Docker
 * daemon (doc/HERMES_NATIVE_PLAN.md), and the SEC-B* evidence has to say which mechanism enforced
 * it. Docker is kept as one backend among others because the probes themselves are backend
 * independent - they ask the boundary to run a shell script and look at the answer.
 */
export type BoundaryBackend =
  | "docker"
  | "appcontainer"
  | "wsl2"
  | "windows-sandbox"
  | "hyper-v"
  | "none";

export interface BoundarySpec {
  backend: BoundaryBackend;
  /** The container image (docker) or the distribution name (wsl2). */
  image: string;
  workspaceDir: string;
  restrictedDir: string;
  outsideDir: string;
  memoryLimitMb: number;
  cpus: number;
  pidsLimit: number;
}

export const DEFAULT_SECURITY_IMAGE = "alpine:3.20";
export const DEFAULT_WSL_DISTRO = "Ubuntu";

/**
 * The backends this lab can actually drive today.
 *
 * The type above is wider on purpose - AppContainer and Windows Sandbox are mechanisms this
 * project may use later - but a backend that has no provider behind it is not a mechanism, it
 * is a name. Listing one here would let a spec claim an isolation nobody established.
 */
export const SUPPORTED_BACKENDS: readonly BoundaryBackend[] = ["wsl2", "docker"];

/** Violations that make the spec unusable as evidence. An empty array means it can run. */
export function validateBoundarySpec(spec: BoundarySpec): string[] {
  const violations: string[] = [];
  if (!SUPPORTED_BACKENDS.includes(spec.backend)) {
    violations.push(
      `backend must be one of ${SUPPORTED_BACKENDS.join(", ")}: a process boundary cannot deny filesystem access on its own`
    );
  }
  // The image is the container image for docker and the distribution for wsl2; both name what
  // runs the probe. An empty name means nothing runs, which is not evidence.
  if (!spec.image.trim()) violations.push("image must not be empty");

  for (const [name, dir] of [
    ["workspaceDir", spec.workspaceDir],
    ["restrictedDir", spec.restrictedDir],
    ["outsideDir", spec.outsideDir]
  ] as const) {
    if (!isAbsolute(dir)) violations.push(`${name} must be an absolute path`);
    if (!existsSync(dir)) {
      violations.push(`${name} does not exist: ${dir}`);
    } else if (!statSync(dir).isDirectory()) {
      violations.push(`${name} is not a directory: ${dir}`);
    }
  }

  if (!Number.isFinite(spec.memoryLimitMb) || spec.memoryLimitMb <= 0) {
    violations.push("memoryLimitMb must be a positive number");
  }
  if (!Number.isFinite(spec.cpus) || spec.cpus <= 0) violations.push("cpus must be a positive number");
  if (!Number.isInteger(spec.pidsLimit) || spec.pidsLimit <= 0) violations.push("pidsLimit must be a positive integer");

  if (violations.length === 0) {
    const dirs = [spec.workspaceDir, spec.restrictedDir, spec.outsideDir].map(dir => resolve(dir));
    for (let i = 0; i < dirs.length; i += 1) {
      for (let j = i + 1; j < dirs.length; j += 1) {
        if (overlaps(dirs[i]!, dirs[j]!)) {
          violations.push(`fixture directories overlap: ${dirs[i]} and ${dirs[j]}`);
        }
      }
    }
  }
  return violations;
}

/**
 * Docker arguments for one probe. Only the workspace is mounted; `restricted` and `outside` are
 * absent from the container on purpose, so their probes fail with "no such file" rather than
 * relying on a read-only mount that root inside a container could still read.
 */
export function buildDockerArgs(spec: BoundarySpec, script: string): string[] {
  return [
    "run",
    "--rm",
    "--network",
    "none",
    "--cap-drop",
    "ALL",
    "--security-opt",
    "no-new-privileges",
    "--memory",
    `${spec.memoryLimitMb}m`,
    "--cpus",
    String(spec.cpus),
    "--pids-limit",
    String(spec.pidsLimit),
    "-v",
    `${toDockerPath(spec.workspaceDir)}:/workspace:rw`,
    "-w",
    "/workspace",
    spec.image,
    "sh",
    "-c",
    script
  ];
}

/** Docker Desktop accepts forward slashes with a drive letter; backslashes break the -v syntax. */
export function toDockerPath(path: string): string {
  return resolve(path).replace(/\\/g, "/");
}

function overlaps(a: string, b: string): boolean {
  return isWithin(a, b) || isWithin(b, a);
}

function isWithin(child: string, parent: string): boolean {
  const rel = relative(parent, child);
  if (rel === "") return true;
  if (isAbsolute(rel)) return false;
  return !rel.split(/[\\/]/).includes("..");
}
