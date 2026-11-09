import { spawn } from "node:child_process";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { writeTestReport } from "../reporting/report-writer.js";
import { BOUNDARY_PROBES, evaluateBoundaryProbe, type ProbeOutcome } from "../security/boundary-probes.js";
import {
  DEFAULT_SECURITY_IMAGE,
  buildDockerArgs,
  validateBoundarySpec,
  type BoundarySpec
} from "../security/boundary-spec.js";
import type { TestResult } from "../types/hermes.js";

/**
 * Proves that the declared sandbox enforces its filesystem and network limits.
 *
 * This report uses `SEC-B*` ids and must not be merged into the 24-item Phase 0 report: it is
 * boundary evidence, and the checklist items it supports still need an agent-level probe.
 */
export interface SecurityPhase0Options {
  spec: BoundarySpec;
  checkRuntime: () => Promise<string>;
  runProbe: (args: string[], timeoutMs: number) => Promise<ProbeOutcome>;
  timeoutMs?: number;
}

export async function executeSecurityPhase0(options: SecurityPhase0Options): Promise<TestResult[]> {
  const timeoutMs = options.timeoutMs ?? 120_000;
  const violations = validateBoundarySpec(options.spec);
  if (violations.length > 0) {
    return blockedAll(`Sandbox specification is not usable as evidence: ${violations.join("; ")}`);
  }

  let runtime: string;
  try {
    runtime = await options.checkRuntime();
  } catch (error) {
    return blockedAll(`Container runtime unavailable: ${errorMessage(error)}`);
  }

  const results: TestResult[] = [];
  for (const probe of BOUNDARY_PROBES) {
    const startedAt = new Date().toISOString();
    const outcome = await options.runProbe(buildDockerArgs(options.spec, probe.script), timeoutMs);
    const verdict = evaluateBoundaryProbe(probe, outcome);
    results.push({
      id: probe.id,
      title: probe.title,
      status: verdict.status,
      startedAt,
      finishedAt: new Date().toISOString(),
      evidence: [
        `runtime=${runtime}`,
        `image=${options.spec.image}`,
        ...(probe.mapsTo.length > 0 ? [`supports=${probe.mapsTo.join(",")}`] : []),
        ...verdict.evidence
      ],
      ...(verdict.notes ? { notes: verdict.notes } : {})
    });
  }
  return results;
}

function blockedAll(note: string): TestResult[] {
  const timestamp = new Date().toISOString();
  return BOUNDARY_PROBES.map(probe => ({
    id: probe.id,
    title: probe.title,
    status: "blocked" as const,
    startedAt: timestamp,
    finishedAt: timestamp,
    evidence: [],
    notes: note
  }));
}

async function main(): Promise<void> {
  const reportPath = resolve(process.env.HERMES_SECURITY_REPORT_PATH ?? "reports/phase0-security-boundary.json");
  const confirmation = process.env.HERMES_SECURITY_LAB_CONFIRM;
  let results: TestResult[];

  if (confirmation !== "I_UNDERSTAND_THE_RISK") {
    results = blockedAll(
      "Refusing to run security probes. Set HERMES_SECURITY_LAB_CONFIRM=I_UNDERSTAND_THE_RISK only in a " +
        "disposable VM/container with no production credentials."
    );
  } else {
    const spec: BoundarySpec = {
      backend: "docker",
      image: process.env.HERMES_SECURITY_IMAGE ?? DEFAULT_SECURITY_IMAGE,
      workspaceDir: resolve(process.env.HERMES_SECURITY_WORKSPACE ?? "fixtures/security-lab/workspace"),
      restrictedDir: resolve(process.env.HERMES_SECURITY_RESTRICTED ?? "fixtures/security-lab/restricted"),
      outsideDir: resolve(process.env.HERMES_SECURITY_OUTSIDE ?? "fixtures/security-lab/outside"),
      memoryLimitMb: positiveNumber("HERMES_SECURITY_MEMORY_MB", 512),
      cpus: positiveNumber("HERMES_SECURITY_CPUS", 1),
      pidsLimit: Math.trunc(positiveNumber("HERMES_SECURITY_PIDS", 128))
    };
    results = await executeSecurityPhase0({
      spec,
      timeoutMs: positiveNumber("HERMES_SECURITY_TIMEOUT_MS", 120_000),
      checkRuntime: async () => {
        const outcome = await run("docker", ["version", "--format", "{{.Server.Version}}"], 30_000);
        if (outcome.exitCode !== 0 || !outcome.stdout.trim()) {
          throw new Error(`docker daemon is not reachable: ${outcome.stderr.trim() || "no version returned"}`);
        }
        return outcome.stdout.trim();
      },
      runProbe: (args, timeoutMs) => run("docker", args, timeoutMs)
    });
  }

  await writeTestReport(reportPath, results);
  console.log(`Security boundary report: ${reportPath}`);
  for (const item of results) console.log(`${item.id} ${item.status}: ${item.title}`);
  if (results.some(item => item.status === "failed")) process.exitCode = 1;
}

/** Spawns without a shell so probe scripts stay single argv entries and cannot be re-interpreted. */
function run(command: string, args: string[], timeoutMs: number): Promise<ProbeOutcome> {
  return new Promise(resolveOutcome => {
    let stdout = "";
    let stderr = "";
    let settled = false;
    let timedOut = false;
    const child = spawn(command, args, { stdio: ["ignore", "pipe", "pipe"], windowsHide: true });
    const timer = setTimeout(() => {
      timedOut = true;
      child.kill("SIGKILL");
    }, timeoutMs);

    child.stdout.on("data", chunk => { stdout += String(chunk); });
    child.stderr.on("data", chunk => { stderr += String(chunk); });
    child.on("error", error => {
      finish(null, errorMessage(error));
    });
    child.on("close", code => {
      finish(code, undefined);
    });

    function finish(exitCode: number | null, error: string | undefined): void {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      if (error) stderr += stderr ? ` ${error}` : error;
      resolveOutcome({ exitCode, stdout, stderr, timedOut });
    }
  });
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
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
