# Agent Adapter Contract (Phase 1 skeleton)

Source of truth for the interface itself: design document V1.0 section 5.1 and 5.2.
Implementation: `src/types/adapter.ts`, `src/adapters/hermes-adapter.ts`,
`src/adapters/hermes-event-normalizer.ts`, `src/adapters/runtime-registry.ts`.

The interface is DocuFlow's contract, not Hermes'. A Hermes release upgrade may change the wire
protocol, the event names and the capability set; it must not change this interface.

## Capability declarations

| Capability | How HermesAdapter answers | Gate |
|---|---|---|
| streaming | `features.run_events_sse === true` from `/v1/capabilities` | upstream fact |
| cancellation | option `verifiedCapabilities.cancellation` | Phase 0 P0-018 |
| sessionResume | always `false` | no resume in the HTTP API |
| toolEvents | `false` until captured tool-event evidence | Phase 0 P0-006 |
| permissionEvents | `false` until approval evidence | Phase 0 P0-012 |
| subAgents | `false` until delegation evidence | Phase 0 P0-008 |
| skills | `false` until skill evidence | Phase 0 P0-024 |

`cancel()` refuses with `unsupported_capability` while `cancellation` is false, so a caller cannot
believe a run stopped when the stop path was never verified. Enable it by passing
`verifiedCapabilities: { cancellation: true }` after P0-018 passes; do not flip the default.

## Event mapping

`payload.hermesEvent` always carries the original runtime event name. Unmapped names become
`runtime.unknown` with `payload.raw`, per design 5.2.6.

| DocuFlow event | Hermes event names |
|---|---|
| execution.started | `run.started`, `run.queued`, `execution.started` |
| message.delta | `message.delta`, `run.message.delta`, `assistant.delta`, `run.output.delta` |
| message.completed | `message.completed`, `run.message.completed`, `assistant.message` |
| tool.started | `tool.started`, `tool_call.started`, `run.tool.started` |
| tool.progress | `tool.progress`, `tool_call.progress`, `run.tool.progress` |
| tool.completed | `tool.completed`, `tool_call.completed`, `tool_call.finished`, `run.tool.completed` |
| tool.failed | `tool.failed`, `tool_call.failed`, `run.tool.failed` |
| permission.requested | `permission.requested`, `approval.requested`, `run.approval.requested`, `approval` |
| execution.waiting | `execution.waiting`, `run.waiting`, `run.input_required` |
| execution.completed | `run.completed`, `run.succeeded`, `execution.completed` |
| execution.failed | `run.failed`, `run.error`, `run.interrupted`, `execution.failed` |
| execution.cancelled | `run.cancelled`, `run.canceled`, `execution.cancelled` |
| runtime.unknown | anything else |

`run.interrupted` maps to `execution.failed` with `payload.hermesEvent = "run.interrupted"`, because
the V1.0 event union has no interrupted terminal event while `AgentExecutionStatus` does. Consumers
can distinguish it from a plain failure through the original event name.

Every alias above is either documented in `docs/UPSTREAM_CONTRACT.md` or must be justified by a
captured transcript before it is added.

## Ordering, deduplication, buffering

- `sequence` increments per execution across both synthetic and runtime events; consumers sort on it
  instead of trusting SSE arrival order (design 5.2.5).
- `externalEventId` carries the SSE `id`; a redelivered id is ignored (design 5.2.2).
- Events are buffered per execution and replayed to subscribers that join late, which is the local
  half of event compensation (AT-008). Durable persistence remains the Audit Engine's job.
- The SSE pump is created lazily on the first `subscribe()` and shared by all subscribers.

## Failure behavior

| Situation | Result |
|---|---|
| `/v1/runs` fails | `runtime_unreachable` from `start()` |
| response has no `run_id` | `protocol_error` from `start()` |
| stream ends with no terminal event | synthetic `execution.failed`, `payload.reason = "stream_closed"` |
| stream throws | synthetic `execution.failed`, `payload.reason = "stream_error"` |
| unknown execution id | `execution_not_found` |
| second active execution for one run id | `invalid_state` |
| timeout | `input.timeoutSeconds`, combined with the local abort signal |

## Not implemented yet

- Durable event persistence and audit timeline (Phase 3).
- Resume via checkpoint injection (Recovery Engine, Phase 3).
- Approval round-trip: `permission.requested` is forwarded but the adapter does not answer it. The
  Policy Engine must own that decision before `permissionEvents` can be declared.
- TUI Gateway transport as an alternative adapter backend; the client exists, the adapter does not.
