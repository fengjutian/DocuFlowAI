import test from "node:test";
import assert from "node:assert/strict";
import { PassThrough } from "node:stream";
import { AcpFrameReader, serializeFrame } from "../../acp/framing.js";
import { AcpAgentServer } from "../../acp/server.js";
import type { AcpBackend } from "../../acp/protocol.js";

function backend(): AcpBackend {
  return {
    async createSession() { return { sessionId: "s1" }; },
    async prompt() { return "end_turn"; },
    async cancel() { return undefined; }
  };
}

async function waitFor(condition: () => boolean, timeoutMs = 2_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!condition()) {
    if (Date.now() > deadline) throw new Error("timed out");
    await new Promise(resolve => setTimeout(resolve, 5));
  }
}

test("framing: reads newline-delimited frames, including split chunks", () => {
  const reader = new AcpFrameReader();
  assert.deepEqual(reader.push('{"a":1}\n{"b":'), ['{"a":1}']);
  assert.deepEqual(reader.push("2}\n"), ['{"b":2}']);
  assert.equal(reader.framing, "ndjson");
});

test("framing: reads Content-Length frames and counts bytes, not characters", () => {
  const reader = new AcpFrameReader();
  const body = JSON.stringify({ text: "中文消息" });
  const frame = `Content-Length: ${Buffer.byteLength(body, "utf8")}\r\n\r\n${body}`;

  // Split inside the header and inside the multibyte body.
  const first = reader.push(Buffer.from(frame.slice(0, 30), "utf8"));
  const second = reader.push(Buffer.concat([Buffer.from(frame.slice(30), "utf8"), Buffer.from(frame)]));
  assert.deepEqual(first, []);
  assert.equal(second.length, 2);
  assert.deepEqual(JSON.parse(second[0]!), JSON.parse(body));
  assert.equal(reader.framing, "content-length");
});

test("framing: serializes in the requested format", () => {
  assert.equal(serializeFrame({ a: 1 }, "ndjson"), '{"a":1}\n');
  const framed = serializeFrame({ a: 1 }, "content-length");
  assert.match(framed, /^Content-Length: \d+\r\n\r\n\{"a":1\}$/);
});

function harness(): { server: AcpAgentServer; input: PassThrough; frames: Array<Record<string, unknown>> } {
  const input = new PassThrough();
  const output = new PassThrough();
  const frames: Array<Record<string, unknown>> = [];
  const reader = new AcpFrameReader("auto");
  output.on("data", (chunk: Buffer) => {
    for (const frame of reader.push(chunk)) {
      frames.push(JSON.parse(frame) as Record<string, unknown>);
    }
  });
  return { server: new AcpAgentServer(backend(), { input, output }), input, frames };
}

test("acp: answers a Content-Length client in Content-Length framing", async () => {
  const app = harness();
  app.server.start();
  const request = JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: 1 } });
  app.input.write(`Content-Length: ${Buffer.byteLength(request)}\r\n\r\n${request}`);
  await waitFor(() => app.frames.length === 1);
  assert.equal((app.frames[0] as { id: number }).id, 1);
  app.server.stop();
});

test("acp: a newline-delimited client still gets newline-delimited responses", async () => {
  const app = harness();
  app.server.start();
  app.input.write(`${JSON.stringify({ jsonrpc: "2.0", id: 2, method: "initialize", params: {} })}\n`);
  await waitFor(() => app.frames.length === 1);
  assert.equal((app.frames[0] as { id: number }).id, 2);
  app.server.stop();
});
