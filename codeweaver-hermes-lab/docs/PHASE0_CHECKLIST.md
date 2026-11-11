# Phase 0 Checklist

The table below is the **baseline**: what the 24 items are and what evidence each one requires.
The `Initial status` column is the state the project started from and is not kept in sync.

The live status is the aggregated report — `reports/phase0.json`, rebuilt with
`npm run report:phase0` — and it is the only place the counts come from. `reports/phase0.json`
carries one entry per item with its evidence; the console does not show it yet.

| ID | Verification | Initial status | Evidence required |
|---|---|---|---|
| P0-001 | HTTP create run | not_run | Run ID + response |
| P0-002 | HTTP SSE event stream | not_run | Captured event sequence |
| P0-003 | TUI Gateway session/prompt | not_run | JSON-RPC transcript |
| P0-004 | ACP editor connection | not_run | ACP transcript + Zed version |
| P0-005 | Single-step task | not_run | Output + terminal status |
| P0-006 | Multi-tool task | not_run | Ordered tool events |
| P0-007 | Multi-turn session | not_run | Session continuity evidence |
| P0-008 | Subagent delegation | not_run | Parent/child execution IDs |
| P0-009 | Allowed file read | not_run | File access log |
| P0-010 | Restricted file denied | not_run | OS/container denial evidence |
| P0-011 | Unauthorized write blocked | not_run | Before/after hashes |
| P0-012 | Dangerous command blocked/approved | not_run | Policy decision + process evidence |
| P0-013 | Unauthorized network blocked | not_run | Network policy evidence |
| P0-014 | Parallel Agent runs | not_run | Independent run IDs |
| P0-015 | Worktree isolation | not_run | Cross-worktree read/write checks |
| P0-016 | Sandbox resource limits | not_run | CPU/memory/process measurements |
| P0-017 | Host path protection | not_run | Mount and access evidence |
| P0-018 | Cancellation | not_run | Cancel request + terminal state |
| P0-019 | Client disconnect | not_run | No duplicate execution evidence |
| P0-020 | Runtime crash | not_run | Crash and recovery logs |
| P0-021 | Session resume/rebuild | not_run | Recovered context + run identity |
| P0-022 | Audit timeline | not_run | Correlated event trace |
| P0-023 | Token/model/time accounting | not_run | Usage fields and timing |
| P0-024 | End-to-end traceability | not_run | Task→Run→Tool→Change→Commit→Test |
