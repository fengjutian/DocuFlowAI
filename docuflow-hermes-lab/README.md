# DocuFlow AI · Hermes Agent Runtime Lab

Phase 0 validation scaffold for evaluating Hermes as a pluggable Agent Runtime.

## Scope

This repository provides:
- Typed HTTP client for Hermes' documented run-oriented API.
- SSE event reader for `/v1/runs/{id}/events`.
- Generic JSON-RPC request tracking and a newline-delimited stdio process client for TUI Gateway validation.
- The DocuFlow `AgentRuntimeAdapter` contract, a Hermes adapter over the HTTP client, and a runtime
  registry with aggregated health checks.
- A Policy Engine permission evaluator (design section 8) that turns a policy snapshot into
  allow / deny / require_approval, and an approval round-trip in the adapter.
- Sandbox boundary probes that prove the container enforces filesystem and network limits.
- A Code Engine change pipeline: worktrees bound to a base SHA, diff risk analysis, policy and
  approval gates, atomic patch application, and immutable change records.
- An append-only execution journal with checkpoints and a recovery planner (compensate / resume /
  rebuild) for gaps and crashes.
- Immutable context / policy / prompt snapshots and change records, with traceability queries
  (commit → agent run, execution → everything, task → executions).
- Context snapshot capture with a deterministic, content-addressed hash and drift verification.
- A `startTask` entry point that ties snapshots, execution, change application and tracing together.
- Unit tests that run without a Hermes server.
- Opt-in integration tests against a locally configured Hermes server.
- Security-test fixture layout and a structured JSON result format.

**Important:** this is a validation harness, not a production adapter. The exact API payloads,
event names, approval semantics, resume behavior and Gateway method schemas must be verified
against the pinned Hermes release. Do not treat a successful HTTP response as proof of safe
filesystem isolation.

The upstream contract facts already confirmed from official documentation are recorded in
[`docs/UPSTREAM_CONTRACT.md`](docs/UPSTREAM_CONTRACT.md). Phase 0 is pinned by
[`hermes.lock.json`](hermes.lock.json); run `npm run check:hermes` before accepting evidence.

## Requirements

- Node.js 22+
- npm
- A separately installed, version-pinned Hermes Agent
- A dedicated disposable test environment for security tests

## Quick start

```bash
cp .env.example .env
npm install
npm run test:unit
```

Set `HERMES_HTTP_BASE_URL` and optional `HERMES_API_KEY`, then run:

```bash
npm run test:integration
npm run validate:http
```

`validate:http` writes a machine-readable report to `reports/phase0-http.json` by default.
Use `HERMES_REPORT_PATH`, `HERMES_EVENT_TIMEOUT_MS` and `HERMES_MAX_EVENTS` to adjust
the report destination and SSE capture limits. Missing server configuration is recorded as
`blocked`; verified protocol failures are recorded as `failed` and return a non-zero exit code.

After pinning the Hermes Gateway command and confirming its method schema, configure
`HERMES_GATEWAY_COMMAND`, `HERMES_GATEWAY_METHOD` and optional JSON parameters, then run:

```bash
npm run validate:gateway
```

`HERMES_GATEWAY_ARGS` must be a JSON array of strings and `HERMES_GATEWAY_PARAMS` must be valid
JSON. Set `HERMES_GATEWAY_FRAMING` to `ndjson` or `content-length` after confirming the pinned
Hermes protocol. The runner does not invoke a shell, which keeps command arguments explicit and
avoids shell expansion. Incoming frame size is bounded by `HERMES_GATEWAY_MAX_MESSAGE_BYTES`, and
the report defaults to `reports/phase0-gateway.json`.

Merge the HTTP and Gateway evidence into the complete 24-item checklist with:

```bash
npm run report:phase0
```

The consolidated report is written to `reports/phase0.json`. Missing checks remain explicitly
`not_run`. Override the input list with `PHASE0_REPORT_INPUTS` (a JSON array of paths) and the
destination with `PHASE0_REPORT_PATH`; when multiple reports contain the same check, the last
input wins.

The integration test creates a run and checks that a run identifier is returned. It is
skipped when `HERMES_HTTP_BASE_URL` is not configured. Never point security tests at a
production workspace.

## Suggested Phase 0 execution order

1. Pin Hermes version and record `hermes --version`.
2. Run unit tests.
3. Validate HTTP `/v1/capabilities`, `/v1/models`, `/v1/runs`.
4. Validate SSE event delivery and terminal event behavior.
5. Configure the stdio client and validate TUI Gateway method schemas against the pinned version.
6. Run security tests only inside a disposable OS/container boundary.
7. Verify cancellation, process crash, reconnect, resume and duplicate-run behavior.
8. Export a signed-off Phase 0 report before starting the production Adapter.
9. Run `npm run validate:adapter` to prove the Phase 1 adapter path end to end.

## Phase 1 adapter skeleton

`src/types/adapter.ts` is DocuFlow's stable contract (design V1.0 section 5). `HermesAdapter`
implements it on top of the HTTP client and covers the Phase 1 items that do not require a live
server to be designed: execution lifecycle, event normalization, registration and health checks.

```text
start()      POST /v1/runs, then emit execution.started with the external run id
subscribe()  one shared SSE pump per execution; events are buffered and replayed to late
             subscribers, deduplicated by the SSE id field, and numbered per execution
getStatus()  maps Hermes run status onto AgentExecutionStatus
cancel()     POST /v1/runs/{id}/stop, but only when cancellation is declared verified
resume()     always unsupported_capability: the HTTP API has no documented resume
terminate()  aborts the local pump and drops the execution state
```

Two rules keep this honest:

1. **Capabilities are declarations, not guesses.** `getCapabilities()` returns `streaming` only when
   the server advertises `run_events_sse`, and returns false for cancellation, resume, tool events,
   permission events, sub-agents and skills until Phase 0 evidence exists. Pass
   `verifiedCapabilities: { cancellation: true }` only after P0-018 passes.
2. **Unknown events are forwarded, never dropped.** Unmapped Hermes event names become
   `runtime.unknown` with `payload.hermesEvent` and `payload.raw` preserved.

`AgentRuntimeRegistry` holds runtime registrations and produces an aggregated health report; a
runtime that throws or exceeds the health timeout is reported unhealthy rather than blocking the
caller. See [`docs/ADAPTER_CONTRACT.md`](docs/ADAPTER_CONTRACT.md) for the full mapping.

## Permission decisions

`src/policy/permission-policy.ts` is the executable form of design 8.1. A decision is made from the
operation, the policy snapshot and the current time — never from the prompt or the runtime's own
approval UI (D-003).

| Level | Operations | Default |
|---|---|---|
| P0 | `read_file`, `search_code`, `query_gitnexus` | allow |
| P1 | `generate_diff` | allow |
| P1 | `modify_worktree` | allow only inside `allowedPaths` and the worktree; no paths means require approval |
| P2 | `delete_file`, `install_dependency`, `modify_public_api`, `modify_database_schema` | require approval unless `autoApprovedOperations` names it |
| P3 | `git_merge`, `deploy_production` | require approval |
| P3 | `access_production_data` | deny |

Unknown operations, unexpired-but-ungranted operations and expired snapshots are all denied.

The adapter consumes this through `permissionHandler` (or an explicit `respondToPermission()` call):
a `permission.requested` event is forwarded, the execution moves to `waiting_approval`, and nothing
is sent back until a decision exists. With no handler the request stays pending, which is the
fail-closed behavior the upstream contract requires.

### Adapter smoke test

```bash
npm run validate:adapter
```

Runs the Phase 1 acceptance items that do not need a human: AT-001 registration + health, AT-002 task
submission, AT-003 runtime event reception, AT-004 terminal status recognition. It writes
`reports/phase1-adapter.json`; without `HERMES_HTTP_BASE_URL` every check is recorded as `blocked`,
not passed. AT-005 (cancellation) is deliberately not run here because cancellation is gated on
P0-018 evidence.

## Security boundary probes

```bash
HERMES_SECURITY_LAB_CONFIRM=I_UNDERSTAND_THE_RISK npm run validate:security
```

Runs six probes inside a container built from the declared spec: allowed workspace read, restricted
directory absent, outside directory absent, workspace write, path traversal denied, network egress
denied. The report goes to `reports/phase0-security-boundary.json` with `SEC-B*` ids.

Rules that keep the evidence meaningful:

- Only `workspace/` is mounted. `restricted/` and `outside/` are deliberately **absent**, so their
  probes fail with "no such file" — a read-only mount would still be readable by root in-container.
- Network is `--network none`, capabilities are dropped, `no-new-privileges` is set, and memory, CPU
  and PID limits come from the spec.
- Without the confirmation flag nothing runs; that state is recorded as `blocked`.
- A missing Docker daemon is `blocked`, a denied probe that *succeeds* is `failed` and flagged as a
  boundary breach. Never convert either into `passed`.

`SEC-B*` results support but do not replace the Phase 0 checklist: P0-009, P0-010, P0-013 and P0-017
still need an agent-level probe before they can be signed off, because boundary enforcement and agent
behavior are different claims.

## Code change pipeline

`src/code-engine/` enforces design 7.3: the agent proposes a patch, Code Engine validates and applies
it into an isolated worktree. Nothing writes to the shared checkout.

```text
propose()   parse the diff, hash it, classify risk (blocking vs approval)
apply()     path scope → base SHA (AT-105) → policy → approval → git apply --check → git apply
            → optional commit → immutable change record
```

| Outcome | Meaning |
|---|---|
| `denied` | unsafe path, scope escape, base SHA drift, policy deny, or the patch does not apply |
| `awaiting_approval` | dependency/secret/migration/deletion/binary/oversized change without an approval record |
| `applied` | written (and optionally committed) with a change record |

A rejected patch never leaves partial writes: `git apply --check` runs first, and the worktree is
only touched after it passes. See [`docs/CODE_ENGINE.md`](docs/CODE_ENGINE.md).

## Execution journal and recovery

Every execution can be recorded in an append-only journal (`src/journal/`), one JSONL file per
execution, with runtime events and DocuFlow audit facts in one timeline. From it:

```text
buildCheckpoint()  last sequence, terminal state, applied change records, unresolved approvals,
                   snapshot ids, base SHA, and any gaps
planRecovery()     stop | compensate | resume | rebuild — a decision, not a retry
```

A gap means compensate first (re-fetch and `mergeCompensated()`, which refuses conflicting content).
`resume` requires both an advertised capability and a live session; Hermes over HTTP has neither, so
recovery rebuilds with the checkpoint instead of pretending. See
[`docs/RECOVERY.md`](docs/RECOVERY.md).

`ExecutionCoordinator` wires it together: `start()` journals every event through one serialized chain,
`recordAuditFact()` puts change records and policy decisions on the same timeline, and `recover()`
executes the plan — including a rebuild that injects `[DOCUFLOW CHECKPOINT]` and records
`recovery.rebuild` against the new execution.

## Snapshots and traceability

`src/persistence/` stores context, policy and prompt snapshots plus change records as one immutable
JSON file each: an existing id is a conflict, never an overwrite, and prompt snapshots keep hashes
instead of plaintext. `TraceabilityQuery` answers the design-15 questions:

```ts
byCommit(sha)        // commit → agent run → context/policy/prompt/change
byExecution(id)      // one execution's full evidence bundle
byTask(taskId)       // every execution that worked on a task
```

Context snapshots have no execution id, so they are resolved through the prompt snapshot that
consumed them. See [`docs/PERSISTENCE.md`](docs/PERSISTENCE.md).

## Context snapshots

`captureContextSnapshot()` reads the worktree itself: git base SHA, content hashes of the authoritative
documents, and hashes of every selected file, folded into a canonical `context_hash`. Two captures of
the same bytes produce the same hash; one changed byte produces a different one.

```ts
verifyContextSnapshot(record, current)
// → { matches: false, actualHash, differences: ["document changed: spec.md"] }
```

Every path is resolved inside the worktree and re-read before hashing, so a caller cannot assert a
file is unchanged. See [`docs/CONTEXT.md`](docs/CONTEXT.md).

## Task flow

`TaskExecutionService` is the one entry point that knows the order:

```ts
const { execution } = await service.startTask(request)       // context + policy + prompt snapshots
const outcome = await service.applyProposedChange(execution.executionId, patch)
await service.trace(execution.executionId)                   // context, policy, prompt, changes
await service.traceCommit(outcome.record.gitCommitSha)       // commit → agent run
```

An applied change lands in the immutable change store, the journal (`change.applied`, so checkpoints
know it must not be re-applied) and the trace. `awaiting_approval` and `denied` write nothing.
See [`docs/TASK_FLOW.md`](docs/TASK_FLOW.md).

## Test status vocabulary

- `passed`: assertion verified by executable evidence.
- `failed`: assertion contradicted by evidence.
- `blocked`: environment or protocol prevented verification.
- `not_run`: not attempted.

Never convert `blocked` to `passed`. Security boundary tests require evidence from the OS/container
layer, not merely a Hermes permission prompt.

## Directory map

```text
src/
  adapters/           AgentRuntimeAdapter implementation, event normalizer, runtime registry
  clients/http/       Hermes HTTP API client + SSE
  clients/gateway/    JSON-RPC primitives + stdio process transport
  code-engine/        Worktrees, diff analysis, change pipeline, change records
  context/            Context snapshot capture and drift verification
  journal/            Append-only execution journal, checkpoints, recovery planner
  orchestrator/       ExecutionCoordinator: adapter + journal + recovery wiring
  persistence/        Immutable snapshot/change stores + traceability queries
  policy/             Permission evaluation from a policy snapshot
  security/           Sandbox boundary specification and probes
  tasks/              startTask / applyProposedChange / trace entry point
  tests/unit/         Offline tests
  tests/integration/  Opt-in live-server tests
  reporting/          Machine-readable test result writer
  types/              Shared protocol types + the DocuFlow adapter contract
fixtures/security-lab/
  workspace/          Allowed test workspace
  restricted/         Must remain inaccessible to the agent
  outside/            Must remain inaccessible to the agent
reports/              Generated local reports (gitignored)
```
