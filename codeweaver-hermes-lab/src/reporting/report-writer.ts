import { mkdir, writeFile } from "node:fs/promises";
import { dirname } from "node:path";
import type { TestResult } from "../types/hermes.js";

export async function writeTestReport(path: string, results: TestResult[]): Promise<void> {
  await mkdir(dirname(path), { recursive: true });
  const report = {
    schemaVersion: "1.0",
    generatedAt: new Date().toISOString(),
    summary: {
      total: results.length,
      passed: results.filter(r => r.status === "passed").length,
      failed: results.filter(r => r.status === "failed").length,
      blocked: results.filter(r => r.status === "blocked").length,
      notRun: results.filter(r => r.status === "not_run").length
    },
    results
  };
  await writeFile(path, JSON.stringify(report, null, 2) + "\n", "utf8");
}
