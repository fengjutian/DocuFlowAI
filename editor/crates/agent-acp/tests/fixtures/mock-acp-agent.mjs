// A mock ACP agent endpoint for the editor's contract tests.
//
// It reuses the real `AcpAgentServer` from the agent endpoint, so the framing,
// the dispatch and the `session/update` shapes under test are the ones the
// editor will meet in production. Only the backend is replaced with a scripted
// one, so the test needs no Hermes runtime.
//
// Usage: node mock-acp-agent.js <scenario>
//   scenarios: echo | tool | permission | slow | diff | context
//
// The repository root is taken from CODEWEAVER_REPO_ROOT, or derived from this
// file's location.

import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, resolve } from "node:path";

const scenario = process.argv[2] ?? "echo";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = process.env.CODEWEAVER_REPO_ROOT
  ? resolve(process.env.CODEWEAVER_REPO_ROOT)
  : resolve(here, "../../../../..");

// On Windows an absolute path is not a valid ES module specifier; it has to
// be a file:// URL.
const { AcpAgentServer } = await import(
  pathToFileURL(resolve(repoRoot, "codeweaver-hermes-lab/dist/acp/server.js")).href
);

/**
 * A scripted backend. The permission scenario needs the server itself to ask
 * the client a question, so the backend receives a handle to it.
 */
function scriptedBackend(serverHandle) {
  return {
    async createSession() {
      return { sessionId: "sess-mock-1" };
    },
    async prompt(sessionId, text, emit) {
      if (scenario === "tool") {
        emit({
          sessionUpdate: "tool_call",
          toolCallId: "call-1",
          title: "read_file",
          status: "in_progress"
        });
        emit({ sessionUpdate: "agent_message_chunk", content: { type: "text", text: "reading " } });
        emit({ sessionUpdate: "tool_call_update", toolCallId: "call-1", status: "completed" });
        return "end_turn";
      }
      if (scenario === "permission") {
        emit({
          sessionUpdate: "tool_call",
          toolCallId: "call-9",
          title: "apply_diff",
          status: "in_progress"
        });
        emit({ sessionUpdate: "agent_message_chunk", content: { type: "text", text: "need permission" } });
        try {
          const result = await serverHandle().request("session/request_permission", {
            sessionId,
            toolCall: {
              toolCallId: "call-9",
              title: "apply_diff",
              kind: "other",
              status: "pending"
            },
            options: [
              { optionId: "allow", name: "Allow", kind: "allow_once" },
              { optionId: "reject", name: "Reject", kind: "reject_once" }
            ]
          });
          const optionId = result?.outcome?.optionId ?? "none";
          emit({
            sessionUpdate: "agent_message_chunk",
            content: { type: "text", text: `decision: ${optionId}` }
          });
        } catch {
          emit({
            sessionUpdate: "agent_message_chunk",
            content: { type: "text", text: "decision: cancelled" }
          });
        }
        return "end_turn";
      }
      if (scenario === "slow") {
        emit({ sessionUpdate: "agent_message_chunk", content: { type: "text", text: "first " } });
        // Long enough that the editor can cancel the turn before it ends.
        await new Promise(done => setTimeout(done, 60_000));
        return "cancelled";
      }
      if (scenario === "diff") {
        // The agent proposes a change. It travels as the tool call's content,
        // so the editor can show it and let the user decide; nothing is
        // written here.
        const patch = [
          "--- a/fixture.txt",
          "+++ b/fixture.txt",
          "@@ -1,2 +1,3 @@",
          " line one",
          "-line two",
          "+line two changed",
          "+line three"
        ].join("\n");
        emit({
          sessionUpdate: "tool_call",
          toolCallId: "call-diff",
          title: "request_diff",
          kind: "edit",
          status: "in_progress",
          content: [{ type: "diff", path: "fixture.txt", patch }]
        });
        emit({ sessionUpdate: "agent_message_chunk", content: { type: "text", text: "proposed a change" } });
        emit({ sessionUpdate: "tool_call_update", toolCallId: "call-diff", status: "completed" });
        return "end_turn";
      }
      if (scenario === "context") {
        // The endpoint renders the prompt's blocks: the instruction and the
        // material the user attached.
        emit({ sessionUpdate: "agent_message_chunk", content: { type: "text", text: `echo: ${text}` } });
        return "end_turn";
      }
      emit({ sessionUpdate: "agent_message_chunk", content: { type: "text", text: `echo: ${text}` } });
      return "end_turn";
    },
    async cancel() {
      // The mock has nothing to cancel; the editor only needs the call to
      // reach the endpoint without an error.
    }
  };
}

let server;
server = new AcpAgentServer(scriptedBackend(() => server), {
  input: process.stdin,
  output: process.stdout
});
server.start();

process.on("SIGINT", () => process.exit(0));
process.on("SIGTERM", () => process.exit(0));
