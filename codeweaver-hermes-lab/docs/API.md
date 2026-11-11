# HTTP API (design 12) and live events (design 13)

`createApiServer(deps).listen(port)` returns `{ url, close }`. The layer is deliberately thin: it
authenticates, validates, delegates and maps errors; decisions stay in the services.

## Endpoints

| Method | Path | Behavior |
|---|---|---|
| GET | `/api/v1/runtimes` | registered runtimes (no adapter internals) |
| GET | `/api/v1/runtimes/{id}` | one runtime plus its health entry |
| GET | `/api/v1/runtimes/selection` | what the scheduler would pick (`?runtimeType=`, `?skipHealthCheck=`) |
| POST | `/api/v1/runtimes/{id}/health-check` | 200 healthy / 503 unhealthy |
| POST | `/api/v1/tasks/{taskId}/executions` | `startTask`: snapshots + run, 201 |
| GET | `/api/v1/executions/{id}` | status, checkpoint, pending approvals, completion gate |
| GET | `/api/v1/executions/{id}/events?since=|Last-Event-ID` | journal replay as unified events |
| GET | `/api/v1/executions/{id}/logs` | the raw journal entries |
| POST | `/api/v1/executions/{id}/changes` | `{patch, commitMessage?, approval?, allowedPaths?}` → apply outcome |
| POST | `/api/v1/executions/{id}/evaluation` | `{checks: [...]}` → evaluation result |
| GET | `/api/v1/executions/{id}/trace` | snapshots, change records and evaluations of the execution |
| POST | `/api/v1/executions/{id}/subagents` | spawn a child execution (501 without a subagent manager) |
| GET | `/api/v1/executions/{id}/subagents` | children, lineage and root execution |
| POST | `/api/v1/executions/{id}/cancel` | 202, audited |
| POST | `/api/v1/executions/{id}/retry` | recovery `rebuild`; 409 when recovery decides otherwise |
| POST | `/api/v1/executions/{id}/resume` | 501 unless the runtime can actually resume |
| GET | `/api/v1/executions/{id}/approvals` | pending runtime permission requests |
| POST | `/api/v1/approvals/{requestId}/approve` | approve a pending request (design 12.3) |
| POST | `/api/v1/approvals/{requestId}/reject` | reject a pending request (design 12.3) |
| POST | `/api/v1/executions/{id}/approvals/{requestId}` | `{decision: "approve"\|"deny", reason?}` |
| GET | `/api/v1/trace/commits/{sha}` | commit → change record → execution trace |
| GET | `/api/v1/usage` | usage summary and all records |
| GET | `/api/v1/executions/{id}/usage` | one execution's usage, 404 if none |
| GET | `/api/v1/skills` | registered skill versions with `deprecated` flags |
| POST | `/api/v1/skills` | register a manifest (400 invalid, 409 duplicate) |
| GET | `/api/v1/executions/{id}/skills` | skill bindings for the execution |
| GET | `/api/v1/events` | SSE stream, optional `?executionId=` |

`POST /api/v1/tasks/{taskId}/executions` also accepts:

```json
{
  "skills": [{ "skillId": "react-dev", "version": "1.0.0" }],
  "approvedSkills": ["deploy@1.0.0"],
  "runtimeId": "pi-local",
  "evaluationChecks": [
    { "id": "build", "name": "Build", "command": "npm", "args": ["run", "build"] }
  ]
}
```

A denied skill returns **403 `skill_denied`** and no execution is started; a skill needing approval but
absent from `approvedSkills` is also 403.

`runtimeId` names the runtime that runs the task. It is honoured exactly or refused — an unknown id
is an error, never a silent fallback to another runtime. `auto` asks the scheduler to choose
(D-047).

`evaluationChecks` are the task's acceptance checks. They are declared by the caller, not by the
model: the runtime may run them, but it may not write them. A task without declared checks has no
evidence and its completion gate stays closed (D-049).

## Auth

When `token` is configured, every request needs `Authorization: Bearer <token>`; otherwise the
server is open, which is only acceptable on loopback in a lab.

## Live events

The envelope is exactly design 13:

```json
{ "id": "...", "type": "agent.execution.started", "projectId": "...", "executionId": "...",
  "sequence": 3, "timestamp": "...", "data": { } }
```

- Transport is SSE today (`text/event-stream`, `id:` per frame, `retry: 3000`, heartbeat comments).
  WebSocket would carry the same envelope; the client contract does not change.
- The bus is notification only. A reconnecting client replays from the journal with `since` or
  `Last-Event-ID`, which is why the socket is never the source of truth.
- Recent messages are not proof of success; only a terminal status plus evaluation evidence is.

## Deliberate deviations

- Approval resolution follows design 12.3 (`/approvals/{requestId}/approve|reject`). The server finds
  the owning execution from the request id, because the runtime that raised the request is the one
  that has to answer it (D-046). The execution-scoped path
  (`/executions/{id}/approvals/{requestId}`) is still accepted and routes to the same handler.
- `retry` and `resume` both go through `coordinator.recover()`; the plan decides. Resume returns 501
  when the runtime cannot resume rather than silently doing a retry.
- No request-scoped idempotency key yet: two `POST /executions` calls create two runs.
