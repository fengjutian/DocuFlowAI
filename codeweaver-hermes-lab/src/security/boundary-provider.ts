import { spawn } from "node:child_process";
import { BOUNDARY_PROBES, evaluateBoundaryProbe, type ProbeOutcome } from "./boundary-probes.js";
import { validateBoundarySpec, type BoundaryBackend, type BoundarySpec } from "./boundary-spec.js";
import type { TestResult } from "../types/hermes.js";

/**
 * A boundary provider: the thing that actually puts a process on the other side of a boundary.
 *
 * The SEC-B* probes are backend independent - they hand a shell script to the boundary and look
 * at what came back - so the evidence does not change when the mechanism does. That is the point
 * of the abstraction: the project stopped depending on a Docker daemon (doc/HERMES_NATIVE_PLAN.md),
 * and the probes that carry the evidence have to survive that change without being rewritten.
 *
 * A provider that cannot establish its boundary reports why. It never runs the probe outside the
 * boundary instead: an unisolated run that answers "denied" for the wrong reason is exactly the
 * kind of false evidence this series exists to prevent.
 */
export interface BoundaryProvider {
  readonly backend: BoundaryBackend;
  /** Whether the boundary can be established on this machine, and why not when it cannot. */
  available(): Promise<{ ok: boolean; reason: string }>;
  /** Describes the boundary for the evidence line, e.g. "Ubuntu (WSL2 2.7.3.0)". */
  describe(): Promise<string>;
  /** Runs one probe script inside the boundary. */
  run(spec: BoundarySpec, script: string, timeoutMs: number): Promise<ProbeOutcome>;
}

export interface BoundaryProbeRunOptions {
  spec: BoundarySpec;
  timeoutMs?: number;
}

/**
 * A provider that can hand the *product* a command which runs at the boundary.
 *
 * The agent arm needs the acceptance check to run at the boundary - not the probe - so that the
 * agent's own tool call goes through the same machinery a user's acceptance check goes through.
 * The control arm needs the same script to run *without* the boundary, in the same environment
 * otherwise, so that the difference between the two arms is the boundary and nothing else.
 */
export interface CheckPlanningProvider extends BoundaryProvider {
  planCheck(
    spec: BoundarySpec,
    script: string,
    options: { mode: "inside" | "outside" }
  ): Promise<{ command: string; args: string[] }>;
}

/**
 * Runs the SEC-B* series against one provider.
 *
 * The result uses `SEC-B*` ids and must not be merged into the 24-item Phase 0 report: it is
 * boundary evidence, and the checklist items it supports still need an agent-level probe.
 */
export async function executeBoundaryProbes(
  provider: BoundaryProvider,
  options: BoundaryProbeRunOptions
): Promise<TestResult[]> {
  const timeoutMs = options.timeoutMs ?? 120_000;
  const violations = validateBoundarySpec(options.spec);
  if (violations.length > 0) {
    return blockedAll(`Boundary specification is not usable as evidence: ${violations.join("; ")}`);
  }

  const availability = await provider.available();
  if (!availability.ok) {
    return blockedAll(`Boundary runtime unavailable: ${availability.reason}`);
  }

  let described: string;
  try {
    described = await provider.describe();
  } catch (error) {
    return blockedAll(`Boundary runtime unavailable: ${errorMessage(error)}`);
  }

  const results: TestResult[] = [];
  for (const probe of BOUNDARY_PROBES) {
    const startedAt = new Date().toISOString();
    const outcome = await provider.run(options.spec, probe.script, timeoutMs);
    const verdict = evaluateBoundaryProbe(probe, outcome);
    results.push({
      id: probe.id,
      title: probe.title,
      status: verdict.status,
      startedAt,
      finishedAt: new Date().toISOString(),
      evidence: [
        `backend=${provider.backend}`,
        `boundary=${described}`,
        ...(probe.mapsTo.length > 0 ? [`supports=${probe.mapsTo.join(",")}`] : []),
        ...verdict.evidence
      ],
      ...(verdict.notes ? { notes: verdict.notes } : {})
    });
  }
  return results;
}

export function blockedAll(note: string): TestResult[] {
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

/**
 * Spawns a process without a shell and captures its output.
 *
 * No shell means the probe script stays one argv entry and cannot be re-interpreted by the shell
 * on the way in - a boundary probe whose arguments are re-parsed is not measuring the boundary.
 */
export function spawnCapture(
  command: string,
  args: string[],
  timeoutMs: number,
  options: { input?: string; cwd?: string } = {}
): Promise<ProbeOutcome> {
  return new Promise(resolveOutcome => {
    const stdoutChunks: Buffer[] = [];
    const stderrChunks: Buffer[] = [];
    let settled = false;
    let timedOut = false;
    const child = spawn(command, args, {
      stdio: ["pipe", "pipe", "pipe"],
      windowsHide: true,
      ...(options.cwd ? { cwd: options.cwd } : {})
    });
    const timer = setTimeout(() => {
      timedOut = true;
      child.kill("SIGKILL");
    }, timeoutMs);

    if (options.input !== undefined) {
      child.stdin?.end(options.input);
    } else {
      child.stdin?.end();
    }

    child.stdout.on("data", chunk => stdoutChunks.push(Buffer.from(chunk)));
    child.stderr.on("data", chunk => stderrChunks.push(Buffer.from(chunk)));
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
      let stderr = decodeOutput(Buffer.concat(stderrChunks));
      if (error) stderr += stderr ? ` ${error}` : error;
      resolveOutcome({
        exitCode,
        stdout: decodeOutput(Buffer.concat(stdoutChunks)),
        stderr,
        timedOut
      });
    }
  });
}

/**
 * Decodes a captured stream.
 *
 * The Windows build of `wsl` writes UTF-16LE, so reading its output as UTF-8 produces text with
 * NUL bytes between the characters - "W S L   H r,g:". An evidence line that reads like that is
 * unreadable, and an unreadable boundary description is not evidence.
 */
export function decodeOutput(buffer: Buffer): string {
  if (buffer.length === 0) return "";
  let nuls = 0;
  for (const byte of buffer) if (byte === 0) nuls += 1;
  // A UTF-16LE stream is mostly NUL bytes; a UTF-8 text stream never is.
  if (nuls > 0 && nuls * 10 >= buffer.length) {
    return buffer.toString("utf16le").replace(/\u0000/g, "");
  }
  return buffer.toString("utf8").replace(/\u0000/g, "");
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
