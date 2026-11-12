import test from "node:test";
import assert from "node:assert/strict";
import {
  executeAccounting,
  judgeAccountingArm,
  judgeAccountingLedger,
  readReportedUsage,
  type AccountingClient,
  type AccountingJournalEntry,
  type AccountingSummary,
  type AccountingUsageRecord,
  type ReportedUsage
} from "../../runners/accounting-phase0.js";

/**
 * The probe's judgement, tested the way it is meant to work: mostly on the ways it has to say no.
 *
 * A checker that only ever sees good data proves nothing. Every case below breaks one thing — the
 * record is missing, the tokens disagree, the model was invented, the duration is somebody else's,
 * the total cannot be explained, the summary is not the sum, an unknown id is answered with an
 * empty record, the ledger forgets the run after a restart — and the judgement has to refuse it.
 */

const PI_EXECUTION = "exec-pi";
const HERMES_EXECUTION = "exec-hermes";
const STARTED_AT = "2026-11-12T00:00:00.000Z";
const FINISHED_AT = "2026-11-12T00:00:05.000Z";
const SPAN_MS = 5_000;

const PI_PAYLOAD = {
  messages: [
    { role: "user", content: "read app.ts" },
    {
      role: "assistant",
      model: "MiniMax-M3",
      usage: { input: 100, output: 20, cacheRead: 30, cacheWrite: 0, totalTokens: 150 }
    },
    {
      role: "assistant",
      model: "MiniMax-M3",
      usage: { input: 200, output: 40, cacheRead: 60, cacheWrite: 0, totalTokens: 300 }
    }
  ]
};

const HERMES_PAYLOAD = {
  output: "app.ts holds a constant.",
  usage: { input_tokens: 100, output_tokens: 20, total_tokens: 120 },
  runtime: { model: "" }
};

function journal(terminalPayload: Record<string, unknown>): AccountingJournalEntry[] {
  return [
    { sequence: 1, kind: "audit", category: "execution.input", recordedAt: STARTED_AT, payload: {} },
    { sequence: 2, kind: "runtime.event", category: "execution.started", recordedAt: STARTED_AT, payload: {} },
    { sequence: 3, kind: "runtime.event", category: "execution.completed", recordedAt: FINISHED_AT, payload: terminalPayload }
  ];
}

function piRecord(): AccountingUsageRecord {
  return {
    id: "use_pi",
    executionId: PI_EXECUTION,
    model: "MiniMax-M3",
    promptTokens: 300,
    completionTokens: 60,
    cachedTokens: 90,
    cacheWriteTokens: 0,
    totalTokens: 450,
    durationMs: SPAN_MS,
    startedAt: STARTED_AT,
    finishedAt: FINISHED_AT
  };
}

function hermesRecord(): AccountingUsageRecord {
  return {
    id: "use_hermes",
    executionId: HERMES_EXECUTION,
    model: "unknown",
    promptTokens: 100,
    completionTokens: 20,
    totalTokens: 120,
    durationMs: SPAN_MS,
    startedAt: STARTED_AT,
    finishedAt: FINISHED_AT
  };
}

type Break =
  | "none"
  | "no-record"
  | "wrong-tokens"
  | "invented-model"
  | "wrong-duration"
  | "unexplained-total"
  | "bad-summary"
  | "not-found-200"
  | "forgotten-after-restart";

function summaryOf(records: AccountingUsageRecord[]): AccountingSummary {
  const byModel = new Map<string, { model: string; executions: number; totalTokens: number }>();
  let promptTokens = 0;
  let completionTokens = 0;
  let cachedTokens = 0;
  let cacheWriteTokens = 0;
  let totalTokens = 0;
  let durationMs = 0;
  for (const record of records) {
    promptTokens += record.promptTokens;
    completionTokens += record.completionTokens;
    cachedTokens += record.cachedTokens ?? 0;
    cacheWriteTokens += record.cacheWriteTokens ?? 0;
    totalTokens += record.totalTokens;
    durationMs += record.durationMs;
    const entry = byModel.get(record.model) ?? { model: record.model, executions: 0, totalTokens: 0 };
    entry.executions += 1;
    entry.totalTokens += record.totalTokens;
    byModel.set(record.model, entry);
  }
  return {
    executions: records.length,
    promptTokens,
    completionTokens,
    cachedTokens,
    cacheWriteTokens,
    totalTokens,
    durationMs,
    byModel: [...byModel.values()]
  };
}

function fakeClient(brk: Break = "none"): AccountingClient {
  const pi = piRecord();
  const hermes = hermesRecord();
  if (brk === "wrong-tokens") pi.totalTokens = 999;
  if (brk === "invented-model") hermes.model = "gpt-4o";
  if (brk === "wrong-duration") pi.durationMs = 1234;
  if (brk === "unexplained-total") pi.totalTokens = 1_000;

  const records = brk === "no-record" ? [hermes] : [pi, hermes];

  const build = (restarted: boolean): AccountingClient => ({
    run: async input =>
      input.runtimeId === "pi-local" ? PI_EXECUTION : HERMES_EXECUTION,
    execution: async () => ({
      status: "succeeded",
      terminalEventType: "execution.completed",
      eventCount: 3
    }),
    usage: async executionId => {
      if (executionId === PI_EXECUTION) {
        if (restarted && brk === "forgotten-after-restart") return { status: 404 };
        // The defect this probe exists for: the run happened and the ledger has nothing for it.
        if (brk === "no-record") return { status: 404 };
        return { status: 200, record: pi };
      }
      if (executionId === HERMES_EXECUTION) return { status: 200, record: hermes };
      if (brk === "not-found-200") {
        // An id that was never an execution, answered with a record that says nothing happened.
        return { status: 200, record: { ...pi, id: "use_empty", executionId } };
      }
      return { status: 404 };
    },
    ledger: async () => {
      const summary = summaryOf(records);
      if (brk === "bad-summary") summary.totalTokens = 42;
      return { summary, records };
    },
    logs: async executionId =>
      executionId === PI_EXECUTION
        ? { count: 3, entries: journal(PI_PAYLOAD) }
        : { count: 3, entries: journal(HERMES_PAYLOAD) },
    restart: async () => build(true)
  });

  return build(false);
}

function linksOf(links: { name: string; ok: boolean }[], name: string): boolean {
  return links.find(link => link.name === name)?.ok ?? false;
}

test("a faithful ledger passes every link", async () => {
  const result = await executeAccounting(fakeClient(), { now: () => new Date("2026-11-12T00:00:00.000Z") });

  assert.equal(result.status, "passed", result.notes ?? "");
  assert.equal(result.id, "P0-023");
  assert.ok(result.evidence.some(line => line.startsWith("pi-local/recorded:ok")));
  assert.ok(result.evidence.some(line => line.startsWith("hermes-local/recorded:ok")));
});

test("a run the ledger never recorded is not accounted for", async () => {
  const result = await executeAccounting(fakeClient("no-record"));

  assert.equal(result.status, "failed");
  assert.ok(result.evidence.some(line => line.startsWith("pi-local/recorded:missing")));
});

test("tokens that disagree with the runtime are refused", async () => {
  const result = await executeAccounting(fakeClient("wrong-tokens"));

  assert.equal(result.status, "failed");
  assert.ok(result.evidence.some(line => line.startsWith("pi-local/tokens-agree:missing")));
  assert.ok(result.evidence.some(line => line.startsWith("pi-local/tokens-explain:missing")));
});

test("a model the runtime never named is not invented", async () => {
  const result = await executeAccounting(fakeClient("invented-model"));

  assert.equal(result.status, "failed");
  assert.ok(result.evidence.some(line => line.startsWith("hermes-local/model-unnamed:missing")));
});

test("a duration that is not the journal's span is refused", async () => {
  const result = await executeAccounting(fakeClient("wrong-duration"));

  assert.equal(result.status, "failed");
  assert.ok(result.evidence.some(line => line.startsWith("pi-local/timing-agree:missing")));
});

test("a total its parts do not explain is refused", async () => {
  const result = await executeAccounting(fakeClient("unexplained-total"));

  assert.equal(result.status, "failed");
  assert.ok(result.evidence.some(line => line.startsWith("pi-local/tokens-explain:missing")));
});

test("a summary that is not the sum of the records is refused", async () => {
  const result = await executeAccounting(fakeClient("bad-summary"));

  assert.equal(result.status, "failed");
  assert.ok(result.evidence.some(line => line.startsWith("summary-consistent:missing")));
});

test("an execution that was never run is refused, not answered with an empty record", async () => {
  const result = await executeAccounting(fakeClient("not-found-200"));

  assert.equal(result.status, "failed");
  assert.ok(result.evidence.some(line => line.startsWith("not-found:missing")));
});

test("a ledger that forgets the run after a restart is refused", async () => {
  const result = await executeAccounting(fakeClient("forgotten-after-restart"));

  assert.equal(result.status, "failed");
  assert.ok(result.evidence.some(line => line.startsWith("durable:missing")));
});

test("the judge refuses an arm with no record at all", () => {
  const judgement = judgeAccountingArm({
    runtimeId: "pi-local",
    executionId: PI_EXECUTION,
    usageStatus: 404,
    span: { startedAt: STARTED_AT, finishedAt: FINISHED_AT, durationMs: SPAN_MS }
  });

  assert.equal(judgement.ok, false);
  assert.equal(linksOf(judgement.links, "recorded"), false);
  assert.equal(linksOf(judgement.links, "tokens-agree"), false);
  assert.equal(linksOf(judgement.links, "timing-agree"), false);
});

test("the judge refuses a summary that does not match the records", () => {
  const records = [piRecord()];
  const judgement = judgeAccountingLedger({
    summary: { ...summaryOf(records), totalTokens: 1 },
    records,
    expected: [{ executionId: PI_EXECUTION, totalTokens: 450, model: "MiniMax-M3" }],
    missingUsageStatus: 404,
    durable: { status: 200, record: piRecord() }
  });

  assert.equal(judgement.ok, false);
  assert.equal(linksOf(judgement.links, "summary-consistent"), false);
});

test("the judge refuses a ledger that was never measured for durability", () => {
  const records = [piRecord()];
  const judgement = judgeAccountingLedger({
    summary: summaryOf(records),
    records,
    expected: [{ executionId: PI_EXECUTION, totalTokens: 450, model: "MiniMax-M3" }],
    missingUsageStatus: 404
  });

  assert.equal(judgement.ok, false);
  assert.equal(linksOf(judgement.links, "durable"), false);
});

test("the yardstick reads the primary runtime's shape by summing the assistant messages", () => {
  const reported = readReportedUsage(PI_PAYLOAD, "pi");

  assert.equal(reported.promptTokens, 300);
  assert.equal(reported.completionTokens, 60);
  assert.equal(reported.cachedTokens, 90);
  assert.equal(reported.totalTokens, 450);
  assert.equal(reported.model, "MiniMax-M3");
});

test("the yardstick reports no model when the runtime named none", () => {
  const reported = readReportedUsage(HERMES_PAYLOAD, "hermes");

  assert.equal(reported.promptTokens, 100);
  assert.equal(reported.completionTokens, 20);
  assert.equal(reported.totalTokens, 120);
  assert.equal(reported.model, undefined);
  assert.equal(reported.cachedTokens, undefined);
});

test("the yardstick and the ledger agree on what the primary runtime spent", () => {
  const reported: ReportedUsage = readReportedUsage(PI_PAYLOAD, "pi");
  const record = piRecord();

  assert.equal(record.promptTokens, reported.promptTokens);
  assert.equal(record.completionTokens, reported.completionTokens);
  assert.equal(record.cachedTokens, reported.cachedTokens);
  assert.equal(record.totalTokens, reported.totalTokens);
});
