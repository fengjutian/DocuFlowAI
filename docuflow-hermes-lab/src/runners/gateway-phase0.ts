import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { JsonRpcStdioClient } from "../clients/gateway/json-rpc-stdio.js";
import { writeTestReport } from "../reporting/report-writer.js";
import type { TestResult } from "../types/hermes.js";

export interface GatewayPhase0Client {
  request(method: string, params?: unknown, timeoutMs?: number): Promise<unknown>;
}

export interface GatewayPhase0Options {
  method: string;
  params?: unknown;
  timeoutMs?: number;
  now?: () => Date;
}

export async function executeGatewayPhase0(
  client: GatewayPhase0Client,
  options: GatewayPhase0Options
): Promise<TestResult> {
  const now = options.now ?? (() => new Date());
  const startedAt = now().toISOString();
  try {
    const response = await client.request(options.method, options.params, options.timeoutMs);
    return makeResult("passed", startedAt, now(), [
      `method=${options.method}`,
      `response=${JSON.stringify(response)}`
    ]);
  } catch (error) {
    return makeResult("failed", startedAt, now(), [`method=${options.method}`], errorMessage(error));
  }
}

async function main(): Promise<void> {
  const reportPath = resolve(process.env.HERMES_GATEWAY_REPORT_PATH ?? "reports/phase0-gateway.json");
  const command = process.env.HERMES_GATEWAY_COMMAND;
  const method = process.env.HERMES_GATEWAY_METHOD;
  let result: TestResult;

  if (!command || !method) {
    const now = new Date();
    const missing = [
      ...(!command ? ["HERMES_GATEWAY_COMMAND"] : []),
      ...(!method ? ["HERMES_GATEWAY_METHOD"] : [])
    ];
    result = makeResult("blocked", now.toISOString(), now, [], `${missing.join(" and ")} not configured`);
  } else {
    const client = new JsonRpcStdioClient({
      command,
      args: parseStringArray("HERMES_GATEWAY_ARGS", process.env.HERMES_GATEWAY_ARGS ?? "[]"),
      requestTimeoutMs: positiveNumber("HERMES_GATEWAY_TIMEOUT_MS", 30_000),
      onStderr: chunk => process.stderr.write(chunk)
    });
    try {
      client.start();
      result = await executeGatewayPhase0(client, {
        method,
        ...(process.env.HERMES_GATEWAY_PARAMS
          ? { params: parseJson("HERMES_GATEWAY_PARAMS", process.env.HERMES_GATEWAY_PARAMS) }
          : {}),
        timeoutMs: positiveNumber("HERMES_GATEWAY_TIMEOUT_MS", 30_000)
      });
    } finally {
      await client.stop();
    }
  }

  await writeTestReport(reportPath, [result]);
  console.log(`Phase 0 Gateway report: ${reportPath}`);
  console.log(`${result.id} ${result.status}: ${result.title}`);
  if (result.status === "failed") process.exitCode = 1;
}

function makeResult(
  status: TestResult["status"],
  startedAt: string,
  finishedAt: Date,
  evidence: string[],
  notes?: string
): TestResult {
  return {
    id: "P0-003",
    title: "TUI Gateway session/prompt",
    status,
    startedAt,
    finishedAt: finishedAt.toISOString(),
    evidence,
    ...(notes ? { notes } : {})
  };
}

function parseStringArray(name: string, value: string): string[] {
  const parsed = parseJson(name, value);
  if (!Array.isArray(parsed) || !parsed.every(item => typeof item === "string")) {
    throw new Error(`${name} must be a JSON array of strings`);
  }
  return parsed;
}

function parseJson(name: string, value: string): unknown {
  try {
    return JSON.parse(value);
  } catch (error) {
    throw new Error(`${name} must contain valid JSON`, { cause: error });
  }
}

function positiveNumber(name: string, fallback: number): number {
  const value = Number(process.env[name] ?? fallback);
  if (!Number.isFinite(value) || value <= 0) throw new Error(`${name} must be a positive number`);
  return value;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : undefined;
if (invokedPath === import.meta.url) {
  await main();
}
