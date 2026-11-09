# Runtime candidate: Pi (`earendil-works/pi`)

Pi is the second runtime evaluated for DocuFlow's `AgentRuntimeAdapter` contract. Hermes is the
remote-runtime reference; Pi is the in-process candidate. This document records what was verified,
what the difference buys us, and what is still missing before either is signed off.

- Repository: <https://github.com/earendil-works/pi> (MIT, TypeScript)
- Packages used: `@earendil-works/pi-agent-core`, `@earendil-works/pi-ai` (both pinned to `1.5.0`)
- Evidence date: 2026-11-09, against the installed package — not against documentation

## Why Pi was looked at

The Hermes adapter had to declare `cancellation` and `sessionResume` as unavailable (D-005), because
the HTTP API exposes neither. The reason is structural: a remote runtime can only offer what its
protocol carries. Pi runs in the same process as DocuFlow and its primitives are the ones the
adapter contract already asks for.

## What was verified against the runtime

`scripts/probe-pi-agent.mjs` drives a real `pi-agent-core` agent through Pi's own faux provider —
no model and no network, but the actual runtime. All five probes pass:

| Probe | Result |
|---|---|
| Event stream | `agent_start`, `turn_start`, `message_start`, `message_update`, `message_end`, `turn_end`, `agent_end` |
| Tool execution | Arguments reach `execute(toolCallId, params, signal, onUpdate)` |
| Policy interception | `beforeToolCall` returning `{ block: true }` prevents execution; the reason reaches the model as an error tool result |
| Cancellation | `agent.abort()` fires the run's `AbortSignal`; the run ends with `This operation was aborted` |
| Resume | The transcript survives across prompts (3 → 5 messages) |

`src/adapters/pi-adapter.ts` implements the DocuFlow contract over those primitives, and
`src/tests/unit/pi-adapter.test.ts` (7 tests) exercises the same real agent:

- a run streams `execution.started` / `message.delta` / `execution.completed`, and a late subscriber
  is replayed the buffered history
- a permitted tool runs and reports `tool.started` / `tool.completed`
- a denied tool **never executes**, and the decision is auditable as `permission.requested`
- a tool outside `allowedTools` is blocked even when the policy handler approves
- with no policy handler, every tool call is blocked (fail closed)
- `cancel()` produces exactly one terminal event

## What this changes for DocuFlow

| Contract member | Hermes over HTTP | Pi in-process |
|---|---|---|
| `subscribe()` | SSE stream, ids and reconnect handling | native event subscription, buffered and replayed |
| `cancel()` | `unsupported_capability` (D-005) | `AbortSignal` on the running turn |
| `resume()` | `unsupported_capability` | the transcript continues on the same agent |
| Policy enforcement | outside the runtime: sandbox + tool gating | inside the runtime: `beforeToolCall` denial |
| Install | 90 MB tag tarball, Python 3.14, install stamp | `npm install` of two packages |

The policy row is the important one. D-003 requires enforcement outside the model, and D-020 requires
a *denial* rather than a *refusal*. A `beforeToolCall` block is a denial produced by DocuFlow's own
policy code, before the tool runs — the evidence the earlier design could only ask the sandbox for.

## What is still missing

- **No real model run yet.** Every result above comes from Pi's faux provider. A run against a live
  model is required before any capability is claimed for production use, and MiniMax is the intended
  provider (`pi-ai` speaks the OpenAI-compatible wire, so MiniMax's OpenAI-compatible endpoint
  applies — no Anthropic-wire adapter needed).
- **The 24-item Phase 0 checklist has not been re-pointed at Pi.** The checklist is runtime-neutral
  in wording, but its evidence is currently Hermes-shaped.
- **Security probes.** The container boundary probes need a running Docker daemon; the agent-level
  probes need a live model. Neither has run against Pi.
- **Upstream stability.** Pi 1.5.0 is a fast-moving release train. The version is pinned exactly in
  `package.json`, and `package-lock.json` is committed, but the API surface must be re-checked on
  every upgrade.

## What is not changing

DocuFlow's own layers are runtime-neutral and stay as they are: the policy engine, the Code Engine
and worktree isolation, the journal and recovery planner, the evaluation gate, the skill registry,
the API, and the ACP endpoint that Zed spawns. Pi has no ACP package, which is not a problem:
DocuFlow is the ACP agent, and only the runtime behind it changes.
