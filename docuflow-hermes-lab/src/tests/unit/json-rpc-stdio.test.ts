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

test("rejects a JSON-RPC request after its timeout", async () => {
  const client = new JsonRpcStdioClient({ command: process.execPath, args: ["-e", echoServer] });
  client.start();
  try {
    await assert.rejects(client.request("slow", undefined, 20), /timed out/);
  } finally {
    await client.stop();
  }
});
