// Probe: verify Pi's pi-agent-core against the real installed package.
// Checks the five properties CodeWeaver's AgentRuntimeAdapter needs:
//   1. a native lifecycle event stream
//   2. tool execution
//   3. policy enforcement that actually blocks a tool call (beforeToolCall)
//   4. cancellation
//   5. resume from a transcript
import { Agent } from "@earendil-works/pi-agent-core";
import { createModels } from "@earendil-works/pi-ai";
import { fauxProvider, fauxAssistantMessage, fauxToolCall } from "@earendil-works/pi-ai/providers/faux";

const results = [];
const record = (name, ok, detail) => {
  results.push({ name, ok, detail });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}  ${JSON.stringify(detail)}`);
};

function makeAgent({ responses, tools = [], beforeToolCall, onEvent }) {
  const faux = fauxProvider({ api: "faux-api", provider: "faux", models: [{ id: "faux-model" }] });
  const models = createModels();
  models.setProvider(faux.provider);
  faux.setResponses(responses);
  const agent = new Agent({
    initialState: {
      systemPrompt: "You are a helpful assistant.",
      model: faux.getModel(),
      tools
    },
    streamFn: models.streamSimple.bind(models),
    ...(beforeToolCall ? { beforeToolCall } : {})
  });
  if (onEvent) agent.subscribe(onEvent);
  return { agent, faux, models };
}

// --- 1. lifecycle event stream --------------------------------------------
{
  const events = [];
  const { agent } = makeAgent({
    responses: [fauxAssistantMessage("hello from pi")],
    onEvent: event => events.push(event.type)
  });
  await agent.prompt("hi");
  await agent.waitForIdle();
  const seen = [...new Set(events)];
  record("event-stream", seen.includes("agent_start") && seen.includes("agent_end"), {
    eventTypes: seen,
    total: events.length
  });
}

// --- 2. tool execution ----------------------------------------------------
{
  const calls = [];
  const readFile = {
    name: "read_file",
    label: "Read file",
    description: "Read a file from the workspace",
    parameters: {
      type: "object",
      properties: { path: { type: "string" } },
      required: ["path"]
    },
    execute: async (_toolCallId, args) => {
      calls.push(args);
      return { content: [{ type: "text", text: `contents of ${args.path}` }], details: { ok: true } };
    }
  };
  const { agent } = makeAgent({
    responses: [fauxAssistantMessage([fauxToolCall("read_file", { path: "README.md" })])],
    tools: [readFile]
  });
  await agent.prompt("read README.md");
  await agent.waitForIdle();
  record("tool-execution", calls.length === 1 && calls[0].path === "README.md", { calls });
}

// --- 3. policy enforcement blocks the tool --------------------------------
{
  const calls = [];
  const denied = [];
  const writeFile = {
    name: "write_file",
    label: "Write file",
    description: "Write a file",
    parameters: {
      type: "object",
      properties: { path: { type: "string" }, content: { type: "string" } },
      required: ["path"]
    },
    execute: async args => {
      calls.push(args);
      return { content: [{ type: "text", text: "written" }], details: { ok: true } };
    }
  };
  const { agent } = makeAgent({
    responses: [fauxAssistantMessage([fauxToolCall("write_file", { path: "/etc/passwd", content: "x" })])],
    tools: [writeFile],
    beforeToolCall: async context => {
      denied.push({ name: context.toolCall.name, args: context.args });
      return { block: true, reason: "policy: path outside allowedPaths" };
    }
  });
  await agent.prompt("write /etc/passwd");
  await agent.waitForIdle();
  const transcript = JSON.stringify(agent.state.messages);
  record("policy-block", calls.length === 0 && denied.length === 1 && transcript.includes("policy: path outside allowedPaths"), {
    toolExecuted: calls.length,
    deniedCalls: denied,
    transcriptMentionsReason: transcript.includes("policy: path outside allowedPaths")
  });
}

// --- 4. cancellation ------------------------------------------------------
{
  let aborted = false;
  const slowTool = {
    name: "slow_tool",
    label: "Slow tool",
    description: "Waits until aborted",
    parameters: { type: "object", properties: {}, required: [] },
    execute: async (_toolCallId, _params, signal) => {
      await new Promise((resolve, reject) => {
        const timer = setTimeout(resolve, 10_000);
        signal?.addEventListener("abort", () => {
          aborted = true;
          clearTimeout(timer);
          reject(new Error("aborted"));
        });
      });
      return { content: [{ type: "text", text: "never" }], details: {} };
    }
  };
  const { agent } = makeAgent({
    responses: [
      fauxAssistantMessage([fauxToolCall("slow_tool", {})]),
      fauxAssistantMessage("the tool was interrupted")
    ],
    tools: [slowTool]
  });
  const run = agent.prompt("start the slow tool");
  await new Promise(resolve => setTimeout(resolve, 300));
  agent.abort();
  await run.catch(() => undefined);
  await agent.waitForIdle();
  record("cancellation", aborted === true, {
    abortSignalFired: aborted,
    errorMessage: agent.state.errorMessage ?? null
  });
}

// --- 5. resume from transcript --------------------------------------------
// `continue()` only works when the last message is a user or tool result, so a finished turn is
// extended by prompting again on the same agent: the transcript must survive.
{
  const { agent } = makeAgent({
    responses: [fauxAssistantMessage("first answer"), fauxAssistantMessage("second answer")]
  });
  await agent.prompt("question one");
  await agent.waitForIdle();
  const afterFirst = agent.state.messages.length;
  const firstAnswer = JSON.stringify(agent.state.messages).includes("first answer");
  await agent.prompt("question two");
  await agent.waitForIdle();
  const afterSecond = agent.state.messages.length;
  const stillHasFirst = JSON.stringify(agent.state.messages).includes("first answer");
  record("resume-transcript", afterSecond > afterFirst && firstAnswer && stillHasFirst, {
    messagesAfterFirst: afterFirst,
    messagesAfterSecond: afterSecond,
    transcriptSurvived: stillHasFirst
  });
}

const failed = results.filter(r => !r.ok);
console.log(`\n${results.length - failed.length}/${results.length} probes passed`);
if (failed.length) process.exitCode = 1;
