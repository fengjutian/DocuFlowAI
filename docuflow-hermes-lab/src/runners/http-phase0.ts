import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { HermesHttpClient } from "../clients/http/hermes-http-client.js";
import { writeTestReport } from "../reporting/report-writer.js";
import type { HermesCapabilities, HermesRun, HermesRunCreateRequest, HermesSseEvent, TestResult } from "../types/hermes.js";

export interface HttpPhase0Client {
  capabilities(): Promise<HermesCapabilities>;
  createRun(payload: HermesRunCreateRequest): Promise<HermesRun>;
  getRun(runId: string): Promise<HermesRun>;
  streamRunEvents(runId: string, signal?: AbortSignal): AsyncGenerator<HermesSseEvent>;
}

export interface HttpPhase0Options {
  input?: string;
  model?: string;
  eventTimeoutMs?: number;
  maxEvents?: number;
  now?: () => Date;
}

export async function executeHttpPhase0(
  client: HttpPhase0Client,
  options: HttpPhase0Options = {}
): Promise<TestResult[]> {
  const now = options.now ?? (() => new Date());
  const results: TestResult[] = [];
  const createStarted = now().toISOString();
  let run: HermesRun;
  try {
    const capabilities = await client.capabilities();
    if (capabilities.features?.run_submission !== true) {
      throw new Error("Hermes capabilities do not advertise run_submission");
    }
    if (capabilities.features.run_events_sse !== true) {
      throw new Error("Hermes capabilities do not advertise run_events_sse");
    }
    run = await client.createRun({
      input: options.input ?? "Reply with exactly: DOCUFLOW_PHASE0_OK",
      ...(options.model ? { model: options.model } : {})
    });
    if (!run.run_id) throw new Error("Hermes create-run response did not contain run_id");
    const fetched = await client.getRun(run.run_id);
    if (fetched.run_id !== run.run_id) throw new Error("Hermes get-run response returned a different run_id");
    results.push(result("P0-001", "HTTP create run", "passed", createStarted, now(), [
      `platform=${String(capabilities.platform ?? "unknown")}`,
      `run_id=${run.run_id}`,
      `status=${String(fetched.status ?? "unknown")}`
    ]));
  } catch (error) {
    results.push(result("P0-001", "HTTP create run", "failed", createStarted, now(), [], errorMessage(error)));
    results.push(result("P0-002", "HTTP SSE event stream", "blocked", now().toISOString(), now(), [],
      "SSE verification requires a successfully created run"));
    return results;
  }

  const eventStarted = now().toISOString();
  const events: HermesSseEvent[] = [];
  const eventTimeoutMs = options.eventTimeoutMs ?? 10_000;
  const signal = AbortSignal.timeout(eventTimeoutMs);
  try {
    for await (const event of client.streamRunEvents(run.run_id, signal)) {
      events.push(event);
      if (events.length >= (options.maxEvents ?? 100)) break;
    }
    results.push(result(
      "P0-002",
      "HTTP SSE event stream",
      events.length > 0 ? "passed" : "failed",
      eventStarted,
      now(),
      events.map(event => JSON.stringify(event)),
      events.length > 0 ? undefined : "The SSE stream ended without any events"
    ));
  } catch (error) {
    results.push(result(
      "P0-002",
      "HTTP SSE event stream",
      events.length > 0 ? "passed" : "failed",
      eventStarted,
      now(),
      events.map(event => JSON.stringify(event)),
      events.length > 0
        ? `Event capture ended before stream completion: ${errorMessage(error)}`
        : errorMessage(error)
    ));
  }
  return results;
}

async function main(): Promise<void> {
  const baseUrl = process.env.HERMES_HTTP_BASE_URL;
  const reportPath = resolve(process.env.HERMES_REPORT_PATH ?? "reports/phase0-http.json");
  let results: TestResult[];
  if (!baseUrl) {
    const timestamp = new Date().toISOString();
    results = [
      result("P0-001", "HTTP create run", "blocked", timestamp, new Date(), [], "HERMES_HTTP_BASE_URL is not configured"),
      result("P0-002", "HTTP SSE event stream", "blocked", timestamp, new Date(), [], "HERMES_HTTP_BASE_URL is not configured")
    ];
  } else {
    const client = new HermesHttpClient({
      baseUrl,
      ...(process.env.HERMES_API_KEY ? { apiKey: process.env.HERMES_API_KEY } : {}),
      timeoutMs: numberFromEnv("HERMES_TIMEOUT_MS", 30_000)
    });
    results = await executeHttpPhase0(client, {
      ...(process.env.HERMES_MODEL ? { model: process.env.HERMES_MODEL } : {}),
      eventTimeoutMs: numberFromEnv("HERMES_EVENT_TIMEOUT_MS", 10_000),
      maxEvents: numberFromEnv("HERMES_MAX_EVENTS", 100)
    });
  }
  await writeTestReport(reportPath, results);
  console.log(`Phase 0 HTTP report: ${reportPath}`);
  for (const item of results) console.log(`${item.id} ${item.status}: ${item.title}`);
  if (results.some(item => item.status === "failed")) process.exitCode = 1;
}

function result(
  id: string,
  title: string,
  status: TestResult["status"],
  startedAt: string,
  finishedAt: Date,
  evidence: string[],
  notes?: string
): TestResult {
  return { id, title, status, startedAt, finishedAt: finishedAt.toISOString(), evidence, ...(notes ? { notes } : {}) };
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function numberFromEnv(name: string, fallback: number): number {
  const value = Number(process.env[name] ?? fallback);
  if (!Number.isFinite(value) || value <= 0) throw new Error(`${name} must be a positive number`);
  return value;
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : undefined;
if (invokedPath === import.meta.url) {
  await main();
}
