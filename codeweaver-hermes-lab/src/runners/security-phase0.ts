import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { writeTestReport } from "../reporting/report-writer.js";
import { executeBoundaryProbes, type BoundaryProvider } from "../security/boundary-provider.js";
import { DockerBoundaryProvider } from "../security/docker-boundary.js";
import { Wsl2BoundaryProvider } from "../security/wsl2-boundary.js";
import {
  DEFAULT_SECURITY_IMAGE,
  DEFAULT_WSL_DISTRO,
  type BoundaryBackend,
  type BoundarySpec
} from "../security/boundary-spec.js";
import type { TestResult } from "../types/hermes.js";

/**
 * Proves that the declared boundary enforces its filesystem and network limits.
 *
 * This report uses `SEC-B*` ids and must not be merged into the 24-item Phase 0 report: it is
 * boundary evidence, and the checklist items it supports still need an agent-level probe.
 *
 * The mechanism is chosen by name, and the report says which one produced the evidence. The
 * project does not depend on a Docker daemon any more (doc/HERMES_NATIVE_PLAN.md); WSL2 is a
 * Hyper-V virtual machine and is the mechanism this lab can drive without administrator rights.
 */
export async function executeSecurityPhase0(options: {
  provider: BoundaryProvider;
  spec: BoundarySpec;
  timeoutMs?: number;
}): Promise<TestResult[]> {
  return executeBoundaryProbes(options.provider, {
    spec: options.spec,
    timeoutMs: options.timeoutMs ?? 120_000
  });
}

async function main(): Promise<void> {
  const reportPath = resolve(process.env.HERMES_SECURITY_REPORT_PATH ?? "reports/phase0-security-boundary.json");
  const backend = (process.env.HERMES_SECURITY_BACKEND ?? "wsl2") as BoundaryBackend;
  const spec: BoundarySpec = {
    backend,
    image:
      backend === "wsl2"
        ? (process.env.HERMES_SECURITY_DISTRO ?? DEFAULT_WSL_DISTRO)
        : (process.env.HERMES_SECURITY_IMAGE ?? DEFAULT_SECURITY_IMAGE),
    workspaceDir: resolve(process.env.HERMES_SECURITY_WORKSPACE ?? "fixtures/security-lab/workspace"),
    restrictedDir: resolve(process.env.HERMES_SECURITY_RESTRICTED ?? "fixtures/security-lab/restricted"),
    outsideDir: resolve(process.env.HERMES_SECURITY_OUTSIDE ?? "fixtures/security-lab/outside"),
    memoryLimitMb: positiveNumber("HERMES_SECURITY_MEMORY_MB", 512),
    cpus: positiveNumber("HERMES_SECURITY_CPUS", 1),
    pidsLimit: Math.trunc(positiveNumber("HERMES_SECURITY_PIDS", 128))
  };

  const provider: BoundaryProvider =
    backend === "docker"
      ? new DockerBoundaryProvider()
      : new Wsl2BoundaryProvider({
          ...(process.env.HERMES_SECURITY_DISTRO ? { distro: process.env.HERMES_SECURITY_DISTRO } : {})
        });

  const results = await executeSecurityPhase0({
    provider,
    spec,
    timeoutMs: positiveNumber("HERMES_SECURITY_TIMEOUT_MS", 120_000)
  });

  await writeTestReport(reportPath, results);
  console.log(`Security boundary report: ${reportPath}`);
  for (const item of results) console.log(`${item.id} ${item.status}: ${item.title}`);
  if (results.some(item => item.status === "failed")) process.exitCode = 1;
}

function positiveNumber(name: string, fallback: number): number {
  const value = Number(process.env[name] ?? fallback);
  if (!Number.isFinite(value) || value <= 0) throw new Error(`${name} must be a positive number`);
  return value;
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : undefined;
if (invokedPath === import.meta.url) {
  await main();
}
