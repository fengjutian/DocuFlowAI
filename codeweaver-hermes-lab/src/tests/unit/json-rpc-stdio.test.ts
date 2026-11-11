import test from "node:test";
import assert from "node:assert/strict";
import { JsonRpcStdioClient } from "../../clients/gateway/json-rpc-stdio.js";

const echoServer = `
  const readline = require("node:readline");
  const lines = readline.createInterface({ input: process.stdin });
  lines.on("line", line => {
    const message = JSON.parse(line);
    if (message.method === "slow") return;
    if (message.id !== undefined) {
      process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: message.id, result: message.params }) + "\\n");
      process.stdout.write(JSON.stringify({ jsonrpc: "2.0", method: "observed", params: { method: message.method } }) + "\\n");
    }
  });
`;

const contentLengthEchoServer = `
  let buffer = Buffer.alloc(0);
  process.stdin.on("data", chunk => {
    buffer = Buffer.concat([buffer, chunk]);
    while (true) {
      const headerEnd = buffer.indexOf("\\r\\n\\r\\n");
      if (headerEnd < 0) return;
      const header = buffer.subarray(0, headerEnd).toString("ascii");
      const length = Number(/Content-Length:\\s*(\\d+)/i.exec(header)[1]);
      const bodyStart = headerEnd + 4;
      if (buffer.length < bodyStart + length) return;
      const message = JSON.parse(buffer.subarray(bodyStart, bodyStart + length).toString("utf8"));
      buffer = buffer.subarray(bodyStart + length);
      const response = JSON.stringify({ jsonrpc: "2.0", id: message.id, result: message.params });
      process.stdout.write("Content-Length: " + Buffer.byteLength(response) + "\\r\\n\\r\\n" + response);
    }
  });
`;

const peerServer = `
  const readline = require("node:readline");
  const lines = readline.createInterface({ input: process.stdin });
  let originalId;
  lines.on("line", line => {
    const message = JSON.parse(line);
    if (message.method === "prompt.submit") {
      originalId = message.id;
      process.stdout.write(JSON.stringify({
        jsonrpc: "2.0",
        id: "srq-1",
        method: "approval",
        params: { command: "echo safe" }
      }) + "\\n");
    } else if (message.id === "srq-1") {
      process.stdout.write(JSON.stringify({
        jsonrpc: "2.0",
        id: originalId,
        result: { accepted: message.result.choice }
      }) + "\\n");
    }
  });
`;

test("exchanges JSON-RPC requests and receives notifications over stdio", async () => {
  const notifications: string[] = [];
  let notificationReceived!: () => void;
  const notification = new Promise<void>(resolve => { notificationReceived = resolve; });
  const client = new JsonRpcStdioClient({
    command: process.execPath,
    args: ["-e", echoServer],
    onNotification: message => {
      notifications.push(message.method);
      notificationReceived();
    }
  });
  client.start();
  try {
    const result = await client.request<{ value: number }>("echo", { value: 42 });
    assert.deepEqual(result, { value: 42 });
    await notification;
    assert.deepEqual(notifications, ["observed"]);
  } finally {
    await client.stop();
  }
});

test("exchanges Content-Length framed JSON-RPC messages", async () => {
  const client = new JsonRpcStdioClient({
    command: process.execPath,
    args: ["-e", contentLengthEchoServer],
    framing: "content-length"
  });
  client.start();
  try {
    const result = await client.request<{ text: string }>("echo", { text: "你好 Hermes" });
    assert.deepEqual(result, { text: "你好 Hermes" });
  } finally {
    await client.stop();
  }
});

test("answers server-to-client JSON-RPC requests", async () => {
  const requests: string[] = [];
  const client = new JsonRpcStdioClient({
    command: process.execPath,
    args: ["-e", peerServer],
    onRequest: request => {
      requests.push(request.method);
      return { choice: "once" };
    }
  });
  client.start();
  try {
    const result = await client.request<{ accepted: string }>("prompt.submit", { text: "run" });
    assert.deepEqual(result, { accepted: "once" });
    assert.deepEqual(requests, ["approval"]);
  } finally {
    await client.stop();
  }
});

test("rejects a JSON-RPC request after its timeout", async () => {
  const client = new JsonRpcStdioClient({ command: process.execPath, args: ["-e", echoServer] });
  client.start();
  try {
    await assert.rejects(client.request("slow", undefined, 20), /timed out/);
  } finally {
    await client.stop();
  }
});

test("cancels a pending JSON-RPC request with an AbortSignal", async () => {
  const client = new JsonRpcStdioClient({ command: process.execPath, args: ["-e", echoServer] });
  const controller = new AbortController();
  client.start();
  try {
    const request = client.request("slow", undefined, { timeoutMs: 1_000, signal: controller.signal });
    controller.abort(new Error("cancelled by caller"));
    await assert.rejects(request, /cancelled by caller/);
  } finally {
    await client.stop();
  }
});

test("rejects pending requests when the child process crashes", async () => {
  const crashServer = `
    const readline = require("node:readline");
    readline.createInterface({ input: process.stdin }).once("line", () => process.exit(7));
  `;
  const client = new JsonRpcStdioClient({ command: process.execPath, args: ["-e", crashServer] });
  client.start();
  try {
    await assert.rejects(client.request("crash"), /code 7/);
    await assert.rejects(client.request("after-crash"), /not running/);
  } finally {
    await client.stop();
  }
});
