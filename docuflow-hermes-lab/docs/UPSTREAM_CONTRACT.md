# Verified Hermes Upstream Contract

Verified against the official `NousResearch/hermes-agent` documentation on 2026-11-09.
Pin a concrete Hermes release or commit before recording Phase 0 results; upstream `main` is not a
reproducible test target.

## HTTP API

- The API server exposes `POST /v1/runs`, `GET /v1/runs/{run_id}` and
  `GET /v1/runs/{run_id}/events`.
- Run creation and status payloads identify the run with `run_id`, not `id`.
- `GET /v1/capabilities` advertises `run_submission` and `run_events_sse`.
- Run SSE terminal events match terminal status, including `run.completed`, `run.failed`,
  `run.cancelled` and `run.interrupted`.

Official reference:
<https://github.com/NousResearch/hermes-agent/blob/main/website/docs/user-guide/features/api-server.md>

## TUI Gateway

- The TUI Gateway uses peer-to-peer JSON-RPC over newline-delimited stdio frames or WebSocket.
- A normal session flow uses `session.create`, followed by `prompt.submit`.
- Hermes can issue server-to-client JSON-RPC requests such as `approval`, `clarify`, `sudo`,
  `secret`, `vault.*` and `connection`; clients must answer with the same request ID.
- Clients must not automatically approve server requests. The validation runner fails closed when
  no explicit handler is configured.
- The generated OpenRPC and TypeScript contracts in the pinned Hermes source are the authority for
  method parameters and result shapes.

Official references:

- <https://github.com/NousResearch/hermes-agent/blob/main/website/docs/developer-guide/programmatic-integration.md>
- <https://github.com/NousResearch/hermes-agent/blob/main/tui_gateway/AGENTS.md>

## ACP and Zed

`hermes acp` is the supported editor-facing stdio protocol and is used by Zed and other ACP
clients. ACP validation remains after the HTTP and TUI Gateway checks; Zed is not a prerequisite
for those checks.
