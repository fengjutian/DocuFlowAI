import test from "node:test";
import assert from "node:assert/strict";
import { createServer, type Server } from "node:http";
import { AgentRuntimeRegistry } from "../../adapters/runtime-registry.js";
import { executeMultiRuntimePolicyProbe } from "../../runners/multi-runtime-policy-phase0.js";
import {
  NO_CAPABILITIES,
  type AgentCapability,
  type AgentRuntimeAdapter
} from "../../types/adapter.js";

/**
 * The probe's verdict logic, exercised without a runtime.
 *
 * A probe that always says "passed" proves nothing, so the negative cases matter most: a server
 * that runs the task on the wrong runtime, or that accepts a runtime nobody registered, has to
 * make the probe fail.
 */

function adapterWith(capabilities: AgentCapability): AgentRuntimeAdapter {
  return {
    async getCapabilities() { return capabilities; },
    async healthCheck() { return true; },
    async start() { return { executionId: "exec-1", runtimeType: "native", status: "running" }; },
    async getStatus() { return "succeeded"; },
    async cancel() {},
    async resume() { throw new Error("not used"); },
    async subscribe() { return async () => undefined; },
    async terminate() {}
  };
}

function registry(): AgentRuntimeRegistry {
  const registry = new AgentRuntimeRegistry();
  registry.register({
    id: "hermes-local",
    runtimeType: "hermes",
    adapter: adapterWith({ ...NO_CAPABILITIES, streaming: true })
  });
  registry.register({
    id: "pi-local",
    runtimeType: "native",
    adapter: adapterWith({
      ...NO_CAPABILITIES,
      streaming: true,
      cancellation: true,
      sessionResume: true,
      toolEvents: true,
      permissionEvents: true
    })
  });
  return registry;
}

interface FakeServer {
  url: string;
  close(): Promise<void>;
  /** The runtime the server claims ran each execution. */
  ranOn: Map<string, string>;
}

/**
 * A server that behaves the way the lab server does: it names the runtime that ran the execution,
 * and it refuses a runtime it does not know.
 */
async function fakeServer(options: {
  /** Runs the execution somewhere else than it was asked to; the probe has to notice. */
  misroute?: boolean;
  /** Accepts every runtime id; the probe has to notice that nothing was refused. */
  acceptUnknown?: boolean;
} = {}): Promise<FakeServer> {
  const ranOn = new Map<string, string>();
  let counter = 0;

  const server: Server = createServer((request, response) => {
    const chunks: Buffer[] = [];
    request.on("data", chunk => chunks.push(chunk as Buffer));
    request.on("end", () => {
      const url = new URL(request.url ?? "/", "http://localhost");
      const json = (status: number, body: unknown): void => {
        const payload = JSON.stringify(body);
        response.writeHead(status, { "content-type": "application/json; charset=utf-8" });
        response.end(payload);
      };

      if (request.method === "POST" && url.pathname.endsWith("/executions")) {
        const body = JSON.parse(Buffer.concat(chunks).toString("utf8")) as { runtimeId?: string };
        const requested = String(body.runtimeId ?? "");
        if (requested !== "pi-local" && requested !== "hermes-local" && !options.acceptUnknown) {
          json(500, { error: "internal_error", message: `Unknown runtime ${requested}` });
          return;
        }
        counter += 1;
        const executionId = `exec-${counter}`;
        ranOn.set(executionId, options.misroute ? "somewhere-else" : requested);
        json(201, { execution: { executionId } });
        return;
      }

      const detail = /\/api\/v1\/executions\/([^/]+)$/.exec(url.pathname);
      if (detail) {
        const executionId = decodeURIComponent(detail[1]!);
        json(200, { status: "succeeded", runtimeId: ranOn.get(executionId) ?? "unknown" });
        return;
      }

      const events = /\/api\/v1\/executions\/([^/]+)\/events$/.exec(url.pathname);
      if (events) {
        json(200, {
          events: [{ type: "agent.message.delta", data: { delta: "CODEWEAVER_MULTI_RUNTIME_OK" } }]
        });
        return;
      }

      json(404, { error: "not_found" });
    });
  });

  await new Promise<void>(done => server.listen(0, "127.0.0.1", () => done()));
  const address = server.address();
  const port = typeof address === "object" && address ? address.port : 0;
  return {
    url: `http://127.0.0.1:${port}`,
    ranOn,
    close: () => new Promise<void>((done, fail) => server.close(error => (error ? fail(error) : done())))
  };
}

test("the probe passes when both runtimes are named, run and refused correctly", async () => {
  const server = await fakeServer();
  try {
    const results = await executeMultiRuntimePolicyProbe(server.url, registry());
    const byId = new Map(results.map(item => [item.id, item]));

    assert.equal(byId.get("MULTI-RUNTIME-SCHEDULER")?.status, "passed");
    assert.equal(byId.get("MULTI-RUNTIME-CAPABILITY-FILTER")?.status, "passed");
    assert.equal(byId.get("MULTI-RUNTIME-D047-HONOURED")?.status, "passed");
    assert.equal(byId.get("MULTI-RUNTIME-D047-REFUSED")?.status, "passed");
    assert.equal(byId.get("MULTI-RUNTIME-HERMES-EXECUTION")?.status, "passed");
  } finally {
    await server.close();
  }
});

test("the probe fails when the task runs on a runtime nobody named", async () => {
  const server = await fakeServer({ misroute: true });
  try {
    const results = await executeMultiRuntimePolicyProbe(server.url, registry());
    const honoured = results.find(item => item.id === "MULTI-RUNTIME-D047-HONOURED");
    const hermes = results.find(item => item.id === "MULTI-RUNTIME-HERMES-EXECUTION");

    assert.equal(honoured?.status, "failed");
    assert.match(honoured?.notes ?? "", /somewhere-else/);
    assert.equal(hermes?.status, "failed");
  } finally {
    await server.close();
  }
});

test("the probe fails when the server accepts a runtime that does not exist", async () => {
  const server = await fakeServer({ acceptUnknown: true });
  try {
    const results = await executeMultiRuntimePolicyProbe(server.url, registry());
    const refused = results.find(item => item.id === "MULTI-RUNTIME-D047-REFUSED");

    assert.equal(refused?.status, "failed");
    assert.match(refused?.notes ?? "", /accepted a task naming a runtime/);
  } finally {
    await server.close();
  }
});

test("the scheduler check notices that round robin never alternates", async () => {
  // One runtime only: round robin has nothing to alternate to, and the probe must say so.
  const single = new AgentRuntimeRegistry();
  single.register({
    id: "pi-local",
    runtimeType: "native",
    adapter: adapterWith({ ...NO_CAPABILITIES, streaming: true })
  });
  const server = await fakeServer();
  try {
    const results = await executeMultiRuntimePolicyProbe(server.url, single);
    const scheduler = results.find(item => item.id === "MULTI-RUNTIME-SCHEDULER");

    assert.equal(scheduler?.status, "failed");
    assert.match(scheduler?.notes ?? "", /round_robin never left/);
  } finally {
    await server.close();
  }
});
