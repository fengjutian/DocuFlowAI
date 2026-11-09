# HTTP API (design 12) and live events (design 13)

`createApiServer(deps).listen(port)` returns `{ url, close }`. The layer is deliberately thin: it
authenticates, validates, delegates and maps errors; decisions stay in the services.

## Endpoints

| Method | Path | Behavior |
|---|---|---|
| GET | `/api/v1/runtimes` | registered runtimes (no adapter internals) |
| POST | `/api/v1/runtimes/{id}/health-check` | 200 healthy / 503 unhealthy |
| POST | `/api/v1/tasks/{taskId}/executions` | `startTask`: snapshots + run, 201 |
| GET | `/api/v1/executions/{id}` | status, checkpoint, pending approvals |
| GET | `/api/v1/executions/{id}/events?since=|Last-Event-ID` | journal replay as unified events |
| POST | `/api/v1/executions/{id}/cancel` | 202, audited |
| POST | `/api/v1/executions/{id}/retry` | recovery `rebuild`; 409 when recovery decides otherwise |
| POST | `/api/v1/executions/{id}/resume` | 501 unless the runtime can actually resume |
| GET | `/api/v1/executions/{id}/approvals` | pending runtime permission requests |
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
  "approvedSkills": ["deploy@1.0.0"]
}
```

A denied skill returns **403 `skill_denied`** and no execution is started; a skill needing approval but
absent from `approvedSkills` is also 403.

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

- Approval resolution names the execution in the path
  (`/executions/{id}/approvals/{requestId}`) instead of design 12.3's `/approvals/{id}/approve`,
  because a request id alone is not enough to route to a runtime session.
- `retry` and `resume` both go through `coordinator.recover()`; the plan decides. Resume returns 501
  when the runtime cannot resume rather than silently doing a retry.
- No request-scoped idempotency key yet: two `POST /executions` calls create two runs.
