# Phase 0 Decision Log

## D-001: Keep DocuFlow as the control plane
Status: proposed
Decision: Hermes is an external runtime adapter. DocuFlow retains orchestration, task state,
policy enforcement, Code Engine, evaluation, audit and recovery authority.

## D-002: Compare TUI Gateway and HTTP API empirically
Status: proposed
Decision: Validate TUI Gateway as the deep-integration candidate and HTTP API as a viable
run-oriented alternative. ACP is reserved for editor-facing integration such as Zed.

## D-003: Enforce security outside the model
Status: accepted
Decision: OS/container controls are mandatory. Prompt instructions and runtime approval UI
alone do not constitute a security boundary.

## D-004: Add `runtime.unknown` to the event union
Status: accepted
Context: design 5.2 closes the event-type union, but 5.2.6 forbids dropping events the runtime
emits and this contract does not model.
Decision: unmapped runtime events are delivered as `runtime.unknown` with `payload.hermesEvent`
and `payload.raw`. The union stays closed, so consumers handle one explicit "unrecognized" case
instead of parsing arbitrary strings.

## D-005: Declare only Phase-0-proven capabilities
Status: accepted
Context: an adapter that claims cancellation or resume before evidence exists makes DocuFlow plan
work it cannot guarantee.
Decision: `getCapabilities()` claims `streaming` from the server's advertised features and leaves
cancellation, resume, tool events, permission events, sub-agents and skills false until the matching
checklist item passes. `cancel()` fails with `unsupported_capability` while the claim is false
rather than silently issuing a stop the project has never validated.

## D-006: Buffer execution events and replay to late subscribers
Status: accepted
Context: AT-008 requires event compensation after a disconnect, and subscribers may attach after
an execution has already started.
Decision: the adapter keeps a bounded-by-execution in-memory event history, deduplicates by the SSE
id, and replays from sequence 0 on subscribe. Durable persistence stays with the Audit Engine.

## D-007: Permission decisions live in code, behind an explicit handler
Status: accepted
Context: the upstream contract forbids auto-approving server requests, and design 8.2 forbids
enforcing permissions through the prompt.
Decision: `evaluatePermission()` answers from operation, policy snapshot and expiry; the adapter
only transports that decision. Without `permissionHandler` no answer is sent and the request stays
pending. Production data access is denied outright rather than routed to approval, because "默认禁止"
must not be unwound by one click.

## D-008: Boundary evidence is separate from agent evidence
Status: accepted
Context: a sandbox that denies a probe does not prove that Hermes, running inside it, behaves. The
reverse is also true: a compliant model response is not evidence of enforcement (D-003).
Decision: sandbox probes report under `SEC-B*` in `reports/phase0-security-boundary.json` and are
never merged into the 24-item Phase 0 report. `supports=P0-0xx` records which checklist item the
evidence feeds; that item still needs an agent-level probe before sign-off.

## D-009: Mount only the workspace; absence is the boundary
Status: accepted
Context: a read-only bind mount is still readable by root inside the container, so mounting
`restricted/` read-only would not deny reads.
Decision: `restricted/` and `outside/` are not mounted at all; the probe asserts they are absent
(ENOENT). A later UID/GID-based backend may test readable-but-denied semantics, which is a different
claim and needs its own evidence.

## D-010: V1 applies agent changes as patches, never as direct worktree writes
Status: accepted
Context: design 7.2 offers Mode A (controlled tools) and Mode B (agent writes in an isolated
worktree), and recommends A first because B depends on unverified filesystem permissions.
Decision: the pipeline accepts a unified diff, validates it, and applies it with `git apply --check`
followed by `git apply`. Worktree isolation is still created per execution so Mode B can be enabled
later without changing the record format.

## D-011: Change records are frozen; reverts add records
Status: accepted
Context: design 9.5 forbids rewriting history to roll something back.
Decision: `ChangeRecord` is frozen at construction. A rollback produces a new record with
`operation = "revert"` plus `reverts_change_record_id` / `reverts_commit_sha`, and the original is
untouched. A record without a commit cannot be reverted, because there is nothing to point at.

## D-012: The journal is append-only and tolerates gaps
Status: accepted
Context: event compensation must detect missing events, and an audit trail must not be rewritable.
Decision: `append` rejects a duplicate sequence but allows gaps; `detectGaps` reports them and
`mergeCompensated` fills them while rejecting conflicting content. No update or delete API exists.
A corrupted line fails the read with its line number rather than being skipped.

## D-013: Recovery is a four-way decision
Status: accepted
Context: design 14.1/14.2 warns against retrying every failure by calling the model again.
Decision: `planRecovery` returns stop / compensate / resume / rebuild from the checkpoint and the
runtime's declared capabilities. `resume` requires both `sessionResume` and a live session; Hermes
HTTP has neither, so it rebuilds with the checkpoint. Applied change records are carried in the
checkpoint so a rebuild never re-applies a diff that already landed.

## D-014: Journal position and runtime sequence are separate fields
Status: accepted
Context: audit entries and runtime events share one timeline, but only runtime events consume the
runtime's event numbers. Using one counter for both makes an audit write look like a missing event
and creates a write race between the adapter handler and the orchestrator.
Decision: `JournalEntry.sequence` is journal order, assigned by the single writer per execution;
`eventSequence` keeps the runtime number. Gap detection uses `eventSequence` via `detectEventGaps`,
and journal writes are serialized through one promise chain per execution.

## D-015: Rebuild is explicit and must not replay applied changes
Status: accepted
Context: design 14.3 requires injecting the checkpoint into a new execution and not re-applying a
successful diff.
Decision: `ExecutionCoordinator.rebuild()` creates a new run with a new run id, appends a
`[DOCUFLOW CHECKPOINT]` block listing applied change records with an explicit instruction not to
re-apply them, and writes a `recovery.rebuild` audit entry on the new execution that points at the
previous execution id.

## D-016: Snapshot records are immutable files, prompts are stored by hash
Status: accepted
Context: design 11.8 says prompt snapshots must not keep a mutable "current" value, and 15 says the
audit trail must not be rewritable.
Decision: every record is a JSON file created with `wx`; a duplicate id is a conflict. Prompt
snapshots carry template and rendered-prompt hashes only, and `assertPromptSnapshot` rejects a
record that includes plaintext. The `RecordStore` interface is the seam for a future database, and
`list()` scans the directory until then.

## D-017: Context hash is content-addressed and re-derived from disk
Status: accepted
Context: design 9.3 requires a context hash that makes "what did the run actually see" answerable.
Decision: `context_hash` is SHA-256 over canonical JSON (sorted keys, sorted file lists) covering the
base SHA, document hashes, selected-file hashes and the version fields. Paths are resolved inside the
worktree and re-read before hashing; file contents are not duplicated into the record.
`verifyContextSnapshot()` recomputes the hash and lists which documents drifted.

## D-018: Snapshot ids are reserved before start, records written after
Status: accepted
Context: `policy_snapshots` and `prompt_snapshots` both carry an execution id, but the task input must
reference them before the runtime has created the execution.
Decision: reserve both ids first, pass them in the task input, and persist the two records right after
`start()` returns. Records stay immutable; nothing is written with a placeholder execution id.

## D-019: SSE first, WebSocket later, one envelope
Status: accepted
Context: design 13 specifies WebSocket, but a WS server without dependencies is significant work and
the client contract is the envelope plus replay-by-sequence, not the transport.
Decision: serve `GET /api/v1/events` as SSE with `id:` per frame, `retry` and heartbeat comments.
Reconnects replay from the journal using `since` or `Last-Event-ID`. A WebSocket transport can reuse
`toDocuFlowEvent` unchanged. Approval resolution takes the execution id in the path, because a request
id alone does not route to a runtime session.

## D-020: A refusal is not a security pass
Status: accepted
Context: an agent that says "I cannot read /restricted" proves nothing about the filesystem; only the
runtime reporting a denial does.
Decision: an agent-level probe passes only when the transcript contains a denial marker
(EACCES / ENOENT / permission denied) and no forbidden content. A leak is a failure, and a refusal or
absent attempt is `blocked`. These probes (P0-009/010/011/013) are therefore recorded under the Phase 0
ids, separate from the `SEC-B*` boundary evidence.

## D-021: The server boots even when the runtime is down
Status: accepted
Context: refusing to start without a reachable Hermes would make the API unavailable exactly when an
operator needs to see why.
Decision: `createRuntime` requires `HERMES_HTTP_BASE_URL` but does not probe it at startup; the health
endpoint returns 503 and executions fail with `runtime_unreachable` (502). Data lives under
`DOCUFLOW_DATA_DIR` (snapshots at the root, journal under `journal/`). Auth is opt-in via
`DOCUFLOW_API_TOKEN`; without it the server is loopback-only by default, which the startup banner says.

## D-022: No checks is a blocked evaluation, not a pass
Status: accepted
Context: AT-204 says a failed test must not mark a task complete; the same reasoning applies to a task
that was never tested.
Decision: `evaluateChecks` returns `blocked` for an empty check list, and `completionGate` denies
completion unless a result exists and every check passed. Checks are configured server-side and are
never accepted from an API body, because that would expose arbitrary command execution.

## D-023: Skills must declare, and declarations are checked against policy
Status: accepted
Context: design 10.3 warns that a Markdown skill can contain shell commands, file edits and network
calls, so its mere presence is not evidence of safety.
Decision: a manifest declares permissions, allowed runtimes, risk level and approval policy; the
registry rejects unknown permissions and empty runtime lists, and authorization denies when a
permission is missing from the execution's policy snapshot or the snapshot expired. High risk,
`require_approval` and deprecated versions become `require_approval` rather than `allow`.

## D-024: Usage is recorded only when the runtime reported it
Status: accepted
Context: AT-206 needs token and time accounting, but runtimes differ in payload shape and some omit
usage entirely.
Decision: `recordFromEvent` accepts several field names and derives the total when possible; a payload
without any token data produces no record, because a zero would look like a free run. Duration is
computed by the server from `execution.started` to the terminal event, not trusted from the payload.
The ledger persists one record per execution and reloads at startup.

## D-025: Scheduling filters before it balances
Status: accepted
Context: an unbalanced queue is a nuisance; sending work to a runtime that cannot stream or is down is
a failure.
Decision: `RuntimeScheduler` removes unhealthy runtimes, then checks required capabilities, then applies
`round_robin` / `least_loaded` / `first_healthy`. No candidate raises `runtime_unreachable` or
`unsupported_capability` instead of falling back to a runtime that cannot serve the request.

## D-026: Mutable binding state is stored as an append-only event log
Status: accepted
Context: design 11.5 gives bindings an `enabled` flag, but the record store is immutable.
Decision: bindings are `bind` / `enable` / `disable` events under `<data>/bindings/`; `load()` folds
them in creation order. The current state is derived, the history is preserved, and no file is
rewritten.

## D-027: Detect the Hermes version from package metadata when the CLI cannot answer
Status: accepted
Context: Hermes 0.15.1 initializes an LLM provider before parsing flags, so `hermes --version` prints a
startup failure rather than a version, and `check:hermes` could not tell "old runtime" from "broken
command".
Decision: `HERMES_PYTHON` reads `importlib.metadata` (and `HERMES_VERSION_COMMAND` overrides entirely);
the error message names the trap. On this machine the check reports 0.15.1 installed against the pinned
0.23.0, which correctly keeps live validation blocked.

## D-028: Skills are authorized before the run starts, not after
Status: accepted
Context: binding a skill after `start()` would let an execution run with tools nobody approved, and a
"disabled binding" is still a record that looks like permission.
Decision: `startTask` resolves and authorizes every requested skill against the same policy snapshot
first. `deny` aborts; `require_approval` aborts unless the exact `skillId@version` appears in
`approvedSkills`. Accepted skills are bound and journaled as `skill.bound`; nothing is silently
downgraded.

## D-029: A subagent is a full execution with lineage, not a nested session
Status: accepted
Context: design 10 leaves parent/child execution shape open, and nesting sessions would make event
attribution and recovery ambiguous.
Decision: every child goes through the same coordinator with its own execution id, run id and journal.
Lineage is recorded on both sides (`subagent.spawned` / `subagent.parent`) and answered by `rootOf()`.
Worktrees are isolated by default; sharing the parent worktree requires `worktree: "shared"` and no base
SHA, because two agents editing one tree is a deliberate decision.

## D-030: Skill effectiveness is per execution, latest evaluation only
Status: accepted
Context: evaluations attach to executions, skills attach to executions, and a rerun produces a second
evaluation for the same execution.
Decision: join through bindings, count only the latest evaluation per execution, and report executions
without any evaluation as `evaluated: 0` with `passRate: null`. Multiple skills bound to one execution
each receive that execution's verdict, which over-credits only when skills are bound together; call
it out rather than inventing a split.

## D-031: Contract tests use a mock server and say so
Status: accepted
Context: the real runtime is unavailable (0.15.1 installed, 0.23.0 pinned), but the client, SSE parser
and Phase 0 runner still need executable evidence.
Decision: `src/tests/support/mock-hermes-server.ts` serves the documented endpoints, and contract tests
assert run creation, SSE ordering/ids, aborts, stop/approval payloads, auth and capability gating. Test
names start with `contract:` and the docs state explicitly that a green contract suite says nothing
about the pinned runtime's real event names or approval semantics.

## D-032: Zed is the client; DocuFlow sits in the ACP path
Status: accepted
Context: Zed is the project's client base, and design 4.4 says ACP is editor-facing and must not become
the task-scheduling protocol.
Decision: DocuFlow exposes an ACP agent over stdio (`Zed → ACP → DocuFlow → Hermes`). It bridges
sessions and prompts to DocuFlow executions and maps runtime events to `session/update`, while
scheduling, policy and audit stay in DocuFlow's own layer. Each prompt starts a new execution because
the HTTP adapter cannot continue a finished run. The endpoint is explicitly a skeleton: snapshots,
policy enforcement and `session/request_permission` bridging are not wired yet, and the docs list them.

## D-033: ACP permission prompts must fail closed
Status: accepted
Context: an editor can close, hang, or answer late; a runtime permission request that is never answered
blocks the run forever, and silently approving it would defeat the approval.
Decision: the agent sends `session/request_permission` and maps only `allow` to approve. Any other
answer, a missing client handler, a timeout or an ACP error denies and records why. The same
fail-closed rule as `HermesAdapter`: no handler means the request stays pending rather than approved.

## D-034: ACP runs go through the task service, or degrade honestly
Status: accepted
Context: routing editor prompts straight to the coordinator left ACP-driven executions without
context/policy/prompt snapshots, so they were auditable but not evidence-complete.
Decision: when the backend is given a `TaskExecutionService`, every prompt starts through `startTask`
and gets the full snapshot set plus audit entries. A `cwd` that is not a git repository cannot be
snapshotted; that case warns on stderr, journals `context.snapshot.skipped` with the reason, and falls
back to a journal-only execution instead of inventing a base SHA. Policy enforcement remains in the
Code Engine and sandbox — the ACP layer records the snapshot, it does not pretend to enforce it.

## D-035: ACP cancel must end the turn even when the runtime cannot stop
Status: accepted
Context: Hermes over HTTP advertises no verified cancellation, so forwarding `session/cancel` alone
left the editor waiting until the prompt timeout after the user pressed stop.
Decision: `cancel` attempts the runtime stop; on `unsupported_capability` (or any failure) it terminates
the local subscription, journals `execution.cancel_unsupported` with the reason, and settles the turn as
`cancelled`. The editor always gets an immediate answer, and the journal records that the remote run may
still be alive.
