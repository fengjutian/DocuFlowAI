import { spawnCapture } from "./boundary-provider.js";
import type { BoundaryProvider } from "./boundary-provider.js";
import type { ProbeOutcome } from "./boundary-probes.js";
import { buildDockerArgs, type BoundarySpec } from "./boundary-spec.js";

/**
 * The Docker boundary.
 *
 * The project stopped depending on a Docker daemon (doc/HERMES_NATIVE_PLAN.md), but the provider
 * stays: the probes are backend independent, and a machine that still has Docker should be able
 * to produce the same evidence. What matters for the evidence is the `backend=` line in the
 * report, not the mechanism behind it.
 */
export class DockerBoundaryProvider implements BoundaryProvider {
  readonly backend = "docker" as const;

  constructor(private readonly options: { dockerCommand?: string } = {}) {}

  private get docker(): string {
    return this.options.dockerCommand ?? "docker";
  }

  async available(): Promise<{ ok: boolean; reason: string }> {
    const version = await spawnCapture(this.docker, ["version", "--format", "{{.Server.Version}}"], 30_000);
    if (version.exitCode !== 0 || !version.stdout.trim()) {
      return { ok: false, reason: `docker daemon is not reachable: ${version.stderr.trim() || "no version returned"}` };
    }
    return { ok: true, reason: `docker daemon ${version.stdout.trim()}` };
  }

  async describe(): Promise<string> {
    const version = await spawnCapture(this.docker, ["version", "--format", "{{.Server.Version}}"], 30_000);
    return `docker ${version.stdout.trim() || "unknown"}`;
  }

  async run(spec: BoundarySpec, script: string, timeoutMs: number): Promise<ProbeOutcome> {
    return spawnCapture(this.docker, buildDockerArgs(spec, script), timeoutMs);
  }
}
