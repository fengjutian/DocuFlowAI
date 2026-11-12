import { spawnCapture } from "./boundary-provider.js";
import type { CheckPlanningProvider } from "./boundary-provider.js";
import type { ProbeOutcome } from "./boundary-probes.js";
import { DEFAULT_WSL_DISTRO, type BoundarySpec } from "./boundary-spec.js";

/**
 * The WSL2 boundary: a Hyper-V lightweight virtual machine with its own kernel.
 *
 * The plan (doc/HERMES_NATIVE_PLAN.md) allows a Hyper-V virtual machine as the fallback when
 * AppContainer cannot be used. WSL2 is exactly that: a real virtual machine, not a container, so
 * the guest kernel enforces the boundary rather than a filter the product maintains.
 *
 * The shape of the boundary is:
 *
 *  1. a **private mount namespace** (`unshare -m --propagation private`) - the host drives are
 *     unmounted inside it, so the guest sees its own filesystem and nothing of the host;
 *  2. **only the workspace is mounted back in**, over drvfs, which is the one host directory the
 *     boundary grants - the same "only the workspace is mounted" rule the Docker probes used;
 *  3. a **network namespace** (`unshare --net`) with no interface, so there is no egress at all;
 *  4. the probe runs as an **unprivileged user**. This is the load-bearing part: a root process
 *     inside the guest can mount a host drive again (drvfs is part of the WSL kernel), and it can
 *     read the host's raw disks through /dev/sd*, which the kernel exposes read-write to root.
 *     An unprivileged user can do neither, so the boundary holds for the process that runs the
 *     work - not only for the guest as a whole.
 */
export interface Wsl2BoundaryOptions {
  /** The WSL distribution that hosts the boundary. */
  distro?: string;
  /** The unprivileged user the probe runs as inside the guest. */
  sandboxUser?: string;
  /** The wsl launcher. */
  wslCommand?: string;
  /** Where the probe script is written inside the guest. */
  scriptPath?: string;
}

const DEFAULT_SANDBOX_USER = "cwlab";

export class Wsl2BoundaryProvider implements CheckPlanningProvider {
  readonly backend = "wsl2" as const;

  constructor(private readonly options: Wsl2BoundaryOptions = {}) {}

  private get distro(): string {
    return this.options.distro ?? DEFAULT_WSL_DISTRO;
  }

  private get user(): string {
    return this.options.sandboxUser ?? DEFAULT_SANDBOX_USER;
  }

  private get wsl(): string {
    return this.options.wslCommand ?? "wsl";
  }

  private get scriptPath(): string {
    return this.options.scriptPath ?? "/run/cw/probe.sh";
  }

  async available(): Promise<{ ok: boolean; reason: string }> {
    const listed = await spawnCapture(this.wsl, ["-l", "-q"], 30_000);
    if (listed.exitCode !== 0) {
      return { ok: false, reason: `wsl is not usable on this machine: ${listed.stderr.trim() || "no list returned"}` };
    }
    const distros = listed.stdout
      .split(/\r?\n/)
      .map(line => line.replace(/\u0000/g, "").trim())
      .filter(Boolean);
    if (!distros.includes(this.distro)) {
      return {
        ok: false,
        reason: `the distribution '${this.distro}' is not installed (found: ${distros.join(", ") || "none"})`
      };
    }
    const probe = await this.guest(["--", "echo", "cw-ok"], 60_000);
    if (probe.exitCode !== 0 || !probe.stdout.includes("cw-ok")) {
      return { ok: false, reason: `the distribution '${this.distro}' did not answer: ${probe.stderr.trim() || "no output"}` };
    }
    return { ok: true, reason: `WSL2 distribution '${this.distro}' is available` };
  }

  /** Describes the boundary for the evidence line: the version and the guest kernel. */
  async describe(): Promise<string> {
    const version = await spawnCapture(this.wsl, ["--version"], 30_000);
    const first = version.stdout.split(/\r?\n/).map(line => line.trim()).find(line => line.length > 0) ?? "WSL2";
    const kernel = await this.guest(["--", "uname", "-r"], 60_000);
    return `${this.distro} (${first}, kernel ${kernel.stdout.trim() || "unknown"})`;
  }

  async run(spec: BoundarySpec, script: string, timeoutMs: number): Promise<ProbeOutcome> {
    await this.stage(script);
    const outcome = await this.guest(["--", "bash", "-c", this.boundaryCommand(spec)], timeoutMs);
    return outcome;
  }

  /**
   * Prepares a command the product can spawn, which runs a script at the boundary.
   *
   * The script is staged in the guest first, because the command line is not a place to put a
   * script: the product spawns it as one argv entry, and a script that travelled inside the
   * command line would be re-parsed by the shell on the way in.
   */
  async planCheck(
    workspaceDir: string,
    script: string,
    options: { mode: "inside" | "outside" }
  ): Promise<{ command: string; args: string[] }> {
    await this.stage(script);
    const inner =
      options.mode === "inside"
        ? this.boundaryCommand(workspaceDir)
        : ["cd /", `exec su -s /bin/sh ${this.user} -c 'sh ${this.scriptPath}'`].join("; ");
    return {
      command: this.wsl,
      args: ["-d", this.distro, "--user", "root", "--", "bash", "-c", inner]
    };
  }

  /** Writes the probe script into the guest. */
  private async stage(script: string): Promise<void> {
    const prepare = await this.guest(
      [
        "--",
        "bash",
        "-c",
        `mkdir -p "$(dirname ${this.scriptPath})" && cat > ${this.scriptPath} && chmod 755 ${this.scriptPath} && ` +
          `(id -u ${this.user} >/dev/null 2>&1 || useradd -m -s /bin/sh ${this.user})`
      ],
      60_000,
      script
    );
    if (prepare.exitCode !== 0) {
      throw new Error(
        `the probe script could not be placed in the guest: ${prepare.stderr.trim() || `exit ${prepare.exitCode}`}`
      );
    }
  }

  /** The command that runs the staged script inside the boundary. */
  private boundaryCommand(workspaceDir: string): string {
    return `unshare -m --propagation private --net bash -c "${this.boundarySetup(workspaceDir)}"`;
  }

  /** What the boundary does, in the order it has to happen. */
  private boundarySetup(workspaceDir: string): string {
    const workspace = workspaceDir.replace(/'/g, "'\\''");
    return [
      "umount -l /mnt/c /mnt/d /mnt/wsl /mnt/wslg 2>/dev/null || true",
      "mkdir -p /workspace",
      // noexec is load-bearing. WSL2 runs Windows binaries through a kernel handler that cannot
      // be removed from inside the guest, so the boundary's protection is that no Windows
      // executable is reachable: the host drives are unmounted, and the one directory that is
      // mounted back is mounted without the execute bit, so a Windows binary inside the
      // workspace cannot be launched either.
      `mount -t drvfs -o noexec '${workspace}' /workspace || exit 70`,
      "cd /workspace",
      `exec su -s /bin/sh ${this.user} -c 'sh ${this.scriptPath}'`
    ].join("; ");
  }

  /** Runs a command inside the guest as root. */
  private async guest(args: string[], timeoutMs: number, input?: string): Promise<ProbeOutcome> {
    return spawnCapture(this.wsl, ["-d", this.distro, "--user", "root", ...args], timeoutMs, {
      ...(input !== undefined ? { input } : {})
    });
  }
}
