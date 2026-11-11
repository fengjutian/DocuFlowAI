import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { writeTestReport } from "../../reporting/report-writer.js";

test("writes a structured report with status totals", async () => {
  const directory = await mkdtemp(join(tmpdir(), "codeweaver-report-"));
  const path = join(directory, "nested", "phase0.json");
  try {
    await writeTestReport(path, [
      {
        id: "P0-001",
        title: "HTTP create run",
        status: "passed",
        startedAt: "2026-11-09T00:00:00.000Z",
        finishedAt: "2026-11-09T00:00:01.000Z",
        evidence: ["run:r1"]
      },
      {
        id: "P0-002",
        title: "HTTP SSE event stream",
        status: "blocked",
        startedAt: "2026-11-09T00:00:01.000Z",
        finishedAt: "2026-11-09T00:00:02.000Z",
        evidence: [],
        notes: "No live server"
      }
    ]);

    const report = JSON.parse(await readFile(path, "utf8")) as {
      schemaVersion: string;
      summary: Record<string, number>;
      results: unknown[];
    };
    assert.equal(report.schemaVersion, "1.0");
    assert.deepEqual(report.summary, { total: 2, passed: 1, failed: 0, blocked: 1, notRun: 0 });
    assert.equal(report.results.length, 2);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
