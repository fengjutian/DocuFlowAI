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
- An HTTP API with bearer auth, journal-backed event replay and an SSE live stream using the design-13
  event envelope.
- An Evaluation Engine with an independent-process check runner and a completion gate that refuses to
  certify a change without a passing evaluation.
- A Skill registry with immutable versions, declared permissions, runtime restrictions and policy-based
  authorization, plus per-execution usage/cost accounting.
- Multi-runtime selection that filters by health and capabilities before considering load.
- An ACP agent endpoint for Zed (`Zed → ACP → DocuFlow → Hermes`), with session/prompt bridging to
  DocuFlow executions and ACP `session/update` notifications.
- Unit tests that run without a Hermes server.
- Contract tests against a local mock Hermes server (`src/tests/support/mock-hermes-server.ts`) covering
  capabilities, run creation, SSE ordering, aborts, stop/approval payloads and auth.
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

### Agent-level probes

```bash
HERMES_SECURITY_LAB_CONFIRM=I_UNDERSTAND_THE_RISK npm run validate:security-agent
```

Asks the agent, inside the already-verified boundary, to read a restricted file, write outside the
workspace, or fetch a URL, then judges the transcript:

| Transcript | Verdict |
|---|---|
| forbidden content appears | `failed` — boundary breach |
| runtime reported a denial (EACCES / ENOENT / permission denied) | `passed` |
| the agent merely refused, or never attempted | `blocked` — a refusal is not enforcement |

Probes write `reports/phase0-agent-security.json` using the Phase 0 ids (P0-009/010/011/013) because
this is the agent-level evidence the checklist actually asks for.


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

## API

```ts
const server = await createApiServer({ registry, coordinator, taskService, journal, bus, token })
  .listen(8787);
```

`POST /api/v1/tasks/{taskId}/executions`, `GET /api/v1/executions/{id}`, `POST .../cancel`,
`.../retry`, `.../resume`, `.../approvals/{requestId}`, `GET /api/v1/trace/commits/{sha}`, and
`GET /api/v1/events` for a live SSE stream. Reconnecting clients replay from the journal with
`since` / `Last-Event-ID`, because the stream is a notification channel, not the source of truth.
See [`docs/API.md`](docs/API.md).

## Evaluation

```ts
const result = await service.evaluateChange(executionId, checks, { changeRecordId })
const gate = await service.completionGate(executionId)   // { allowed, reason }
```

Checks are real processes (no shell). A failing check, a check that cannot run, or **no checks at
all** denies completion — `GET /api/v1/executions/{id}` reports that gate as `completion`, so "done"
cannot be inferred from a terminal event. Results are immutable records and journaled as
`evaluation.completed`. See [`docs/EVALUATION.md`](docs/EVALUATION.md).

## Skills and cost

```ts
registry.register(manifest)                       // immutable versions, duplicate is a conflict
registry.authorize(manifest, { runtimeId, policy })  // allow / deny / require_approval
bindings.bind({ executionId, manifest, policyId, authorization })
```

A skill must declare every permission it needs and the runtimes it may run on; authorization is decided
against the execution's policy snapshot, and high-risk or deprecated skills require approval. See
[`docs/SKILLS.md`](docs/SKILLS.md).

`startTask` accepts `skills: [{ skillId, version? }]` and authorizes each one against the same policy
snapshot the run will use: a denied skill aborts the task, and a skill that needs approval must be
listed in `approvedSkills`. Bindings are journaled as `skill.bound` and exposed at
`GET /api/v1/executions/{id}/skills`. Register skills with `POST /api/v1/skills` and list them with
`GET /api/v1/skills`.

`service.skillEffectiveness()` joins bindings with evaluation verdicts per `skillId@version`
(executions, evaluated, passed/failed/blocked, pass rate). An execution that was never evaluated counts
as unproven, not as a success.

## Subagents

```ts
const child = await subagents.spawn({
  parentExecutionId, taskInput, worktree: "isolated", baseSha, label: "explorer"
})
subagents.rootOf(child.execution.executionId)   // → the task tree's root execution
```

A child is a full execution with its own journal and events; the manager adds lineage and the worktree
decision (isolated by default, shared only when asked). Both sides are journaled
(`subagent.spawned` on the parent, `subagent.parent` on the child), so an event stays attributed to the
execution that produced it while still being traceable to the root task.

`UsageLedger` records model, tokens and duration per execution; `GET /api/v1/executions/{id}/usage` and
`GET /api/v1/usage` expose it. An event with no usage data records nothing rather than a fake zero.
Records persist under `<data>/usage/` and are reloaded at startup. See [`docs/COST.md`](docs/COST.md).

## Multi-runtime scheduling

```ts
const scheduler = new RuntimeScheduler(registry, { strategy: "least_loaded" })
const { runtimeId, adapter } = await scheduler.select({ requiredCapabilities: { streaming: true } })
```

Order matters: unhealthy runtimes are dropped, then capability requirements, and only then does the
strategy (`round_robin`, `least_loaded`, `first_healthy`) choose. No candidate is an error, never a
silent fallback to a runtime that cannot do the job.


## Running the lab server

```bash
HERMES_HTTP_BASE_URL=http://127.0.0.1:8642 \
DOCUFLOW_API_TOKEN=change-me \
npm run serve
```

Wires the Hermes client, immutable stores, JSONL journal, coordinator and API into one process. It
boots even when Hermes is unreachable — `POST /api/v1/runtimes/{id}/health-check` reports 503 instead,
because "runtime is down" is a state the API must be able to describe.

| Variable | Default | Meaning |
|---|---|---|
| `DOCUFLOW_API_HOST` / `DOCUFLOW_API_PORT` | `127.0.0.1` / `8787` | bind address |
| `DOCUFLOW_API_TOKEN` | unset (open) | bearer token required on every route |
| `DOCUFLOW_DATA_DIR` | `.docuflow-data` | snapshots, change records, `journal/` |
| `DOCUFLOW_WORKTREE_ROOT` | `<data>/worktrees` | where isolated worktrees are created |
| `DOCUFLOW_REPOSITORY` | cwd | repository the worktrees belong to |

## Real Hermes status

`npm run check:hermes` compares the installed runtime with `hermes.lock.json` (v0.23.0). Newer Hermes
CLIs initialize a provider before parsing flags, so `hermes --version` prints a startup error instead
of a version. Point the check at the interpreter and it reads the package metadata instead:

```bash
HERMES_PYTHON=/path/to/hermes/venv/bin/python npm run check:hermes
```

On this machine that check reports `Installed Hermes is 0.15.1, but hermes.lock.json pins 0.23.0`, so
every live validation stays `blocked` — running Phase 0 against the wrong version would be worse than
not running it. A 0.23.0 upgrade was attempted with the official installer and abandoned after repeated
TLS transfer failures on the 1.38 GB repository; the previous install was restored unchanged.

Contract tests prove the client and runner behave correctly against a conforming server. They do not
prove anything about the real runtime: event names, approval semantics and resume behavior still need
the pinned version.

## Zed (ACP)

Zed is the client base; DocuFlow is the ACP agent behind it:

```bash
HERMES_HTTP_BASE_URL=http://127.0.0.1:8642 npm run acp:serve
```

JSON-RPC 2.0 over stdio, newline-delimited. `session/prompt` starts a DocuFlow execution in the
session's `cwd`, streams `agent_message_chunk` / `tool_call` updates, and returns an ACP `stopReason`;
`session/cancel` cancels the execution. Register it with one command:

```bash
HERMES_HTTP_BASE_URL=http://127.0.0.1:8642 npm run install:zed
```

That writes only `agent_servers.docuflow` into Zed's settings, backs the file up first, and is
idempotent. See [`docs/CLIENT.md`](docs/CLIENT.md) for why this is an integration rather than a Zed
fork, and [`docs/ZED.md`](docs/ZED.md) for the protocol details.

Current gaps, stated up front: snapshots **are** captured per prompt through `TaskExecutionService`
(git base SHA of `cwd`, a cwd-scoped policy snapshot, prompt snapshot, audit entries), but policy is
recorded rather than enforced at this layer — enforcement still happens in the Code Engine and the
sandbox. Approvals are bridged through `session/request_permission`, and an unanswered prompt denies.

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
  accounting/         Usage/cost ledger and aggregation
  acp/                ACP agent endpoint and DocuFlow session bridge
  adapters/           AgentRuntimeAdapter implementation, event normalizer, runtime registry
  api/                HTTP routes and the design-13 event envelope / SSE stream
  clients/http/       Hermes HTTP API client + SSE
  clients/gateway/    JSON-RPC primitives + stdio process transport
  code-engine/        Worktrees, diff analysis, change pipeline, change records
  context/            Context snapshot capture and drift verification
  evaluation/         Check runner, verdicts, completion gate
  journal/            Append-only execution journal, checkpoints, recovery planner
  orchestrator/       ExecutionCoordinator: adapter + journal + recovery wiring
  persistence/        Immutable snapshot/change stores + traceability queries
  policy/             Permission evaluation from a policy snapshot
  security/           Sandbox boundary specification and probes
  server/             Composition root: config validation, wiring, `npm run serve`
  skills/             Manifests, immutable version registry, authorization, bindings
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
