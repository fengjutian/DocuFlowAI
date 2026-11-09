# Zed integration (ACP)

Topology: **Zed → ACP → DocuFlow → Hermes**. Zed is the client; DocuFlow is the ACP agent; Hermes
runs behind DocuFlow's adapter. ACP carries editor interaction only and never becomes the task
scheduling protocol (design 4.4).

## Run the endpoint

```bash
HERMES_HTTP_BASE_URL=http://127.0.0.1:8642 npm run acp:serve
```

The process speaks JSON-RPC 2.0 over stdio, one JSON object per line. It answers `initialize`,
`session/new`, `session/prompt`, and treats `session/cancel` as a notification.

## Zed settings

```json
{
  "agent_servers": {
    "docuflow": {
      "command": "node",
      "args": ["D:/github/DocuFlowAI/docuflow-hermes-lab/dist/runners/acp-server.js"],
      "env": {
        "HERMES_HTTP_BASE_URL": "http://127.0.0.1:8642",
        "DOCUFLOW_DATA_DIR": "D:/github/DocuFlowAI/docuflow-hermes-lab/.docuflow-data"
      }
    }
  }
}
```

Restart Zed after editing settings; the agent appears as an available external agent.

## What the bridge does

| ACP | DocuFlow |
|---|---|
| `session/new { cwd }` | remembers the workspace |
| `session/prompt { sessionId, prompt }` | starts a DocuFlow execution with `workingDirectory = cwd`, journals it |
| `agent_message_chunk` | `message.delta` / `message.completed` events |
| `tool_call` / `tool_call_update` | `tool.started` / `tool.completed` / `tool.failed` |
| `stopReason` | `end_turn` (completed), `cancelled` (cancelled), `refusal` (failed) |
| `session/cancel` | `coordinator.cancel(executionId)`; if the runtime cannot cancel, the local subscription is terminated, `execution.cancel_unsupported` is journaled, and the turn still ends `cancelled` |

Each prompt starts a new execution because the Hermes HTTP adapter cannot continue a finished run
(D-015). The session keeps the workspace; the execution ids stay journaled.

## Tests

| Coverage | Test |
|---|---|
| Transport, dispatch, parse errors, cancel | `acp-server.test.ts` |
| Prompt → execution → updates → stopReason | `acp-server.test.ts` |
| Permission round-trip and fail-closed answers | `acp-server.test.ts` |
| Full editor path over real HTTP against a mock Hermes | `acp-e2e-mock.test.ts` |

## Scope and gaps (read before relying on it)

- **Snapshots are captured.** Prompts go through `TaskExecutionService`, so the editor run gets a
  context snapshot (git base SHA of `cwd`), a policy snapshot scoped to `cwd`, a prompt snapshot and the
  matching `*.snapshot` audit entries. A `cwd` that is not a git repository cannot be snapshotted: the
  run degrades to journal-only, warns on stderr, and records `context.snapshot.skipped` with the reason
  so the missing snapshot is visible to an auditor instead of silently absent.
- **Policy is recorded, not enforced here.** The snapshot grants read/search/generate-diff and limits
  paths to `cwd`; real enforcement still happens in the Code Engine and the sandbox.
- **Framing is newline-delimited only.** If a Zed release negotiates Content-Length framing, the reader
  must be extended.
- **Approvals are bridged.** A Hermes `permission.requested` event becomes an ACP
  `session/request_permission` request with `allow` / `reject` options; choosing allow calls
  `respondToPermission(approve)`, anything else (including a closed editor or a timeout) denies so the
  runtime never waits forever. Without a client handler the request stays pending, same as the API path.
