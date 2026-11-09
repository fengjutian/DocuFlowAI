# DocuFlow AI · Hermes Agent Runtime Lab

Phase 0 validation scaffold for evaluating Hermes as a pluggable Agent Runtime.

## Scope

This repository provides:
- Typed HTTP client for Hermes' documented run-oriented API.
- SSE event reader for `/v1/runs/{id}/events`.
- Generic JSON-RPC request tracking and a newline-delimited stdio process client for TUI Gateway validation.
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
  clients/http/       Hermes HTTP API client + SSE
  clients/gateway/    JSON-RPC primitives + stdio process transport
  tests/unit/         Offline tests
  tests/integration/  Opt-in live-server tests
  reporting/          Machine-readable test result writer
  types/              Shared protocol types
fixtures/security-lab/
  workspace/          Allowed test workspace
  restricted/         Must remain inaccessible to the agent
  outside/            Must remain inaccessible to the agent
reports/              Generated local reports (gitignored)
```
