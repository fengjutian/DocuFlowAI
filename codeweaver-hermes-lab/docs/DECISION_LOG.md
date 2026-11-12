# Phase 0 Decision Log

## D-001: Keep CodeWeaver as the control plane
Status: proposed
Decision: Hermes is an external runtime adapter. CodeWeaver retains orchestration, task state,
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
Context: an adapter that claims cancellation or resume before evidence exists makes CodeWeaver plan
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
`[CODEWEAVER CHECKPOINT]` block listing applied change records with an explicit instruction not to
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
`toCodeWeaverEvent` unchanged. Approval resolution takes the execution id in the path, because a request
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
`CODEWEAVER_DATA_DIR` (snapshots at the root, journal under `journal/`). Auth is opt-in via
`CODEWEAVER_API_TOKEN`; without it the server is loopback-only by default, which the startup banner says.

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

## D-032: Zed is the client; CodeWeaver sits in the ACP path
Status: accepted
Context: Zed is the project's client base, and design 4.4 says ACP is editor-facing and must not become
the task-scheduling protocol.
Decision: CodeWeaver exposes an ACP agent over stdio (`Zed → ACP → CodeWeaver → Hermes`). It bridges
sessions and prompts to CodeWeaver executions and maps runtime events to `session/update`, while
scheduling, policy and audit stay in CodeWeaver's own layer. Each prompt starts a new execution because
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

## D-036: ACP framing follows the client, byte-counted
Status: accepted
Context: ACP over stdio is newline-delimited, but some clients speak LSP-style `Content-Length` framing;
assuming one format breaks the other.
Decision: the reader detects the framing from the first frame and the writer answers in the same one.
`Content-Length` is measured in bytes and the reader works on raw Buffers, so a multibyte prompt split
across chunks is not corrupted by UTF-8 decoding at chunk boundaries.

## D-037: Zed source is provisioned in place, identified by a tree digest
Status: accepted
Context: the project needs Zed's source to build features on, but the clone stalls on this network and
the supplied archive has no `.git`, so neither a submodule nor a commit pin is available.
Decision: the full tree (1.29.0, 4,404 files, 91.5 MB) lives at `client/zed`, gitignored, and
`client/PIN.json` records the version, toolchain, file count, size and a SHA-256 digest over
`relative-path\0size`. Re-provisioning is a local copy (`fetch-zed.ps1 -FromPath`) or the pinned tarball.
A real fork, if needed, is a repository fork plus a submodule — not a committed copy.

## D-038: Phase 0 evidence comes before Phase 1+ implementation
Status: accepted
Context: on 2026-11-09 the Phase 0 report showed 0 of 24 checks passed, 3 blocked and 21 not run,
because the pinned runtime was never installed — while the last eleven commits had all moved to the
Zed/ACP client line and Phase 1/2/3 subsystems (Code Engine, journal, evaluation, skills, subagents,
scheduler, API) were already implemented. Every one of those subsystems rests on the unverified
assumption that the mock server behaves like the real runtime.
Decision: the client line is frozen and Phase 0 evidence is the only active work. The install path is
now real: Hermes 0.23.0 is installed from the pinned tag, and P0-001 (HTTP create run), P0-002 (SSE
event stream) and P0-003 (TUI Gateway session) pass against it. Downstream work resumes only when the
checklist is signed off, not before. The deviation itself is recorded here rather than deleted: the
mistake was letting unverified downstream code imply a verified upstream.

## D-039: The pinned runtime identifies itself through `install-stamp.json`
Status: accepted
Context: the 0.23.0 release tarball ships `version = "0.0.0"` in pyproject.toml, so
`importlib.metadata` reports 0.0.0 and the old version check could only ever say "wrong version".
Upstream's own resolution order is install stamp, then live git, then unknown, and the stamp is
written at build time by `scripts/write_install_stamp.py`.
Decision: the install writes the stamp with the locked commit and base version, and
`check:hermes` reads the stamp first and falls back to package metadata. A reported 0.0.0 is
reported as the release placeholder it is, with the command that fixes it, instead of a version
mismatch. The install directory can be overridden with `HERMES_INSTALL_DIR`.

## D-040: The pinned release requires a Python 3.14 interpreter
Status: accepted
Context: every runtime dependency in 0.23.0 is declared as `python_version >= '3.14'`. On Python 3.11
pip installs the package and none of its dependencies, and the failure surfaces later as
`ModuleNotFoundError: ruamel`, which reads like a broken package rather than a wrong interpreter.
Decision: the install creates the virtual environment with Python 3.14 and the API server's
`aiohttp` adapter dependency is installed explicitly. The interpreter path is what `HERMES_PYTHON`
must point at.

## D-041: The pinned release is only installable from the GitHub tag tarball
Status: accepted
Context: PyPI publishes `hermes-agent` only up to 0.19.0, so the pinned 0.23.0 exists solely as a
GitHub tag. On this network `git clone` of the repository stalls (its .git alone is ~1 GB), and a
single-stream download from codeload measured ~1 MB/min, which is ~90 minutes for the 90 MB tarball.
A GitHub proxy that supports byte ranges measured ~1.6 MB/s.
Decision: the install script resolves the tag to the locked commit, downloads the tarball with
parallel ranged chunks when the host honours `Range` and falls back to a single stream when it does
not, then verifies the artifact by size and gzip integrity before extracting. The deployment script
(`scripts/install-hermes.ps1`) is a Phase 0 deliverable in its own right: it used to be missing.

## D-042: The runtime candidate is not fixed by the first adapter
Status: accepted
Context: Hermes was chosen as a *remote* runtime, so its adapter speaks HTTP and SSE. Pi
(`earendil-works/pi`, MIT, `pi-agent-core` 1.5.0) is an in-process TypeScript harness whose
primitives map one-to-one onto CodeWeaver's adapter contract. Measured on 2026-11-09 against the
installed package: a native lifecycle event stream (`agent_start`, `turn_start`, `message_start`,
`message_update`, `message_end`, `turn_end`, `agent_end`), tool interception that blocks execution
before the tool runs, `AbortSignal` cancellation, and a transcript that survives across prompts.
Hermes' HTTP surface offers none of these natively, which is why D-005 had to declare cancellation
and resume as unavailable capabilities.
Decision: Pi is evaluated as the primary runtime candidate while Hermes remains the remote-runtime
reference. The adapter contract is the seam, so CodeWeaver's own layers — policy, code engine,
journal, evaluation, skills, API and the ACP endpoint — are unchanged by this choice. Nothing is
retired before the 24-item checklist is signed off against the chosen runtime. The tool hook also
closes a gap the earlier design could only approximate: D-003 and D-020 require enforcement outside
the model, and a `beforeToolCall` denial is enforcement, because the tool never executes.

## D-043: A runtime claim must come from the runtime, not from a test double
Status: accepted
Context: the Pi adapter's first version claimed a tool was executed while the tool list was empty,
because the test used a hand-written fake agent. The mock hid the defect that the real runtime
rejected the model ("Unknown provider: unknown") and never reached the tool.
Decision: the Pi adapter's tests drive a real `pi-agent-core` agent through Pi's own faux provider —
no model, no network, but the actual runtime. The same rule applies to the standalone probe
(`scripts/probe-pi-agent.mjs`): a statement about a runtime's behaviour is only evidence if the
runtime produced it.

## D-044: An optional runtime dependency must be installed before the first turn
Status: accepted
Context: on 2026-11-09 a real Hermes run stayed `running` for over four minutes with only
`: keepalive` comments on the SSE stream, while the same MiniMax key answered a direct request in
about two seconds. The cause was not the model, the provider or the SSE parser: `py-spy` showed
every agent turn parked in `agent/anthropic_adapter.py::_get_anthropic_sdk` -> `pm.ensure_import("anthropic")`,
with the runtime's `hermes-download_*` worker threads blocked in an SSL read. The Anthropic SDK is
an optional extra that Hermes resolves lazily on the first agent turn, so a missing SDK turns every
turn into a package download instead of a fast failure. On this network that download stalls.
Decision: the deployment script installs the pinned `anthropic==0.125.0` extra up front, next to
`aiohttp`. A runtime that resolves optional dependencies during a request is a deployment hazard,
not a configuration detail, so it belongs in the install script rather than in a runbook.

## D-045: The event name lives in the payload, not in the SSE frame name
Status: accepted
Context: the 0.23.0 API server sends no SSE `event:` field. The frame name stays at the SSE default
`message` and the runtime event name travels inside the JSON payload as `data.event`. The Phase 0
runner compared the terminal-event names against the frame name, so a `run.completed` event that
had really arrived was reported as a missing terminal event and P0-002 was failed against a healthy
runtime. The adapter already resolved the name correctly; the runner had a second, weaker copy of
the rule.
Decision: one resolver, `resolveEventName()`, decides where the event name lives, and both the
adapter and the Phase 0 runner use it. The mock server can emit either shape so the contract test
covers the real one. The rule generalises: when one layer already knows how to read a runtime frame,
a second implementation of the same rule is a defect waiting to happen.

## D-046: With several runtimes, "the adapter" belongs to the execution
Status: accepted
Context: the coordinator, the API layer and the ACP backend all held a single adapter reference.
That was correct while only one runtime was registered, and it becomes wrong the moment a second
one exists: a permission response, a cancellation or a status query must reach the runtime that is
actually running the task. Sending it to the default runtime would answer on behalf of a runtime
that never saw the execution, and the audit trail would name the wrong runtime.
Decision: the coordinator records the adapter that started each execution and exposes it as
`adapterFor(executionId)`; the API layer and the ACP backend route every per-execution call
through it. The default adapter remains the fallback for an unknown execution, because answering
from the default runtime is better than guessing. `getAdapter()` survives only for the calls that
genuinely have no execution context.

## D-047: A named runtime is honoured or refused, never silently redirected
Status: accepted
Context: a task request may name the runtime it wants. Running it on a different runtime than the
caller asked for would change which policy snapshot, which tools and which audit trail apply, so
the request would no longer be the thing that was asked for.
Decision: an explicit `runtimeId` is resolved to exactly that registered runtime or the request is
refused; an unknown id is an error, not a fallback. `auto` is the one exception: it is the explicit
request for the scheduler to choose. A task that does not name a runtime uses the server default.

## D-048: CodeWeaver's own tools are their own enforcement point
Status: accepted
Context: the Pi adapter gates every tool call with the CodeWeaver permission handler, because the
handler is where the policy decision belongs (D-003). That is correct for a tool the runtime
brought with it, and wrong for CodeWeaver's own tools: `read_file` knows the resolved path and
`request_apply_diff` goes through the Code Engine, which checks the path scope, the base SHA and
the policy before it writes. A second gate in front of them would either duplicate the decision
with less information or refuse a tool that was never unsafe.
Decision: the adapter lists the tools whose own implementation is the enforcement point
(`selfEnforcedTools`) and does not gate them a second time. Every other tool still needs the
permission handler, and a policy that cannot answer on its own stays pending for a human rather
than being approved on the runtime's behalf (D-020).

## D-049: The tool surface belongs to one execution, and the checks belong to the task
Status: accepted
Context: the tool surface was written as a set of functions over a worktree and a policy snapshot,
but it had no caller, so `applyProposedChange` and `evaluateChange` were reachable only from the
HTTP API and never from a runtime. Binding them needed two decisions that the design left open.
Decision: a tool set is built per execution, from that execution's worktree, its policy snapshot and
the acceptance checks its task declared; the runtime only sees the tools the execution is allowed
to use. The acceptance checks are declared by the caller when the task starts, never by the model:
a runtime that could name its own command would be grading itself. The model may narrow the
declared set, not extend it, and a task without declared checks has no evidence — the completion
gate stays closed (AT-204).

## D-050: The agent-level security probes run against Pi, in a disposable lab, without a container
Status: accepted
Context: the four agent-level probes were blocked because they need a runtime that really tries to
break the boundary, and the only runtime wired into that runner was the pinned Hermes behind HTTP.
The container boundary that was meant to host them cannot be produced on this machine, and waiting
for a Docker daemon would leave the probes unmoved while the enforcement point they test is already
exercisable: Pi runs the policy gate in its `beforeToolCall` hook, so a denial there happens before
the tool executes rather than after the fact.
Decision: the probes run against Pi with a real MiniMax China model, against a disposable lab
directory under the OS temp directory — a workspace the agent may use, a restricted file that really
holds the secret, an outside directory that really exists — which is deleted afterwards. The tools
are deliberately capable and unguarded, because a tool that policed itself would only prove that the
tool is careful; what has to hold is CodeWeaver's gate. The probes check the disk after each run as
well as the transcript, so a write that slipped through cannot be hidden by a quiet transcript. This
evidence is about the policy gate, not about the host: the container boundary stays a separate
series, and P0-017 remains unsigned until a Docker daemon is available.

## D-051: A leak is content a tool returned, not a string the model repeated
Status: accepted
Context: the probes judged the whole transcript for the forbidden marker. On the write probe that
produced a false breach: the model proposed the sentinel as the content of the write it was asked to
perform, the arguments were recorded in the transcript, and the marker was found there — although
the file was never written and nothing the boundary protects had been obtained. A false breach is
worse than a missed one, because it condemns a working boundary.
Decision: a leak is forbidden content that came back in a tool result. The request side of the
transcript — the arguments, the permission request that carried them, the model's own words — is a
proposal, not a breach. The allow probe follows the same rule in reverse: the expected content has
to come back from a tool, so a model cannot sign off P0-009 by stating the answer from memory.

## D-052: A cancellation is reported when the runtime confirms it, not when it is requested
Status: accepted
Context: the adapter reported the cancelled state the moment the cancel request arrived. That made
the product's own claim the only evidence: the event said "cancelled" because CodeWeaver asked, not
because the runtime stopped, and a runtime that ignored the request would have produced exactly the
same record.
Decision: the terminal event waits for the runtime. Pi ends an aborted turn with
`stopReason: "aborted"`, so the cancelled event carries that reason together with
`runtimeConfirmed`, and the audit timeline says who stopped the turn and why. The wait is bounded: a
runtime that never answers produces a cancellation marked `runtimeConfirmed: false` rather than an
execution that hangs. The execution passes through the `cancelling` state, which is what the API's
202 already promised.

## D-053: The console shows what the runtime said, and folds the stream noise instead of hiding it
Status: accepted
Context: once a task could actually run on Pi, the console turned out to be the weakest link in the
chain. It rendered every event as a full card labelled by type alone, so a single run produced 216
rows of which 131 were Pi's streaming lifecycle events (`turn_start`, `message_update`, `message_end`
and so on) — the eight rows that carried meaning were buried. The assistant's answer was never
shown at all: it only existed inside an event payload, one "原始数据" click away. And the runtime's
own event name was invisible for Pi, because the lookup only knew Hermes' `payload.hermesEvent`.
A console that cannot show the answer is an audit log, not a control plane.
Decision: three rules. First, the assistant's text is a first-class thing the console shows: the
last completed assistant message appears verbatim in its own card, and message deltas and tool
results are rendered inline rather than hidden behind a JSON dump. Second, an event the contract
does not model is folded into one compact line that still shows the runtime's original name
(`payload.hermesEvent` for Hermes, `payload.runtimeEvent` for Pi) — nothing is dropped, but the
stream noise does not get a card per chunk. Third, the server names its charset: `application/json;
charset=utf-8`, because a client that falls back to Latin-1 turns every Chinese character in the
agent's answer into `?`, and the console is the one place where that answer is read.

## D-054: A recorded execution stays describable after the process that ran it is gone
Status: accepted
Context: restarting the server emptied the coordinator's live sessions, and the console immediately
lost every execution it had ever run: `GET /executions/{id}` threw "Unknown execution" and the page
showed a red error, while the journal — durable, on disk — still held every event of the run. The
timeline was still readable next to it, which made the failure look arbitrary. Two things had been
assumed rather than checked: that the runtime id could be recovered from the registry, and that the
status could be recovered from the adapter. Neither survives a restart: the registry would name the
server's *default* runtime, which may not be the one that ran the task, and the adapter knows
nothing about an execution it did not start.
Decision: the journal is the record, and the API reads from it. The runtime id is written into the
journal with the execution (`execution.runtime`), so a reader can still name the runtime that
produced a timeline; the status of a recorded execution is derived from its terminal event
(`execution.completed` → `succeeded`, `execution.failed` → `failed`, `execution.cancelled` →
`cancelled`) instead of an event name where a status belongs. The API also says whether the
execution is still live in this process, and refuses to cancel one that is not — a refusal with a
reason beats a failure from inside the adapter. What the server cannot prove, it does not claim: an
execution recorded before this rule has no journaled runtime, so its runtime stays unknown rather
than being guessed from the default.

## D-055: The recovery strategy is the runtime's answer, and the rebuild runs where the task ran
Status: accepted
Context: the checklist asks for a session that can be recovered to be recovered, or a safe rebuild
— and it says the four failures (client disconnect, gateway stop, agent crash, worker crash) must
not share one strategy. The code had a plan for all of them and no way to reach it: the option
that asks whether the session survived was never wired, so the plan always fell through to
"rebuild", and the rebuild had no input to rebuild from — the original request only ever existed
in the worker's memory. Worse, the rebuild would have run on the server's *default* runtime,
because the coordinator no longer knew which runtime ran the task. A rebuild that quietly moves
the task to another runtime is worse than no rebuild: the caller asked for Pi and got Hermes.
Decision: three rules. First, the runtime that owns the execution answers whether its session
survived (`isSessionResumable` on the adapter); an adapter that cannot answer has no session to
offer, and the answer is "no". For Pi the answer is structural rather than a flag: the transcript
lives on the agent instance inside the worker, so a worker that died took the session with it. The
plan then says "rebuild" — and it says why. Second, the two facts a rebuild needs are journaled
with the execution, not kept in memory: which runtime ran it (`execution.runtime`) and what it was
asked (`execution.input`). The coordinator writes them, because the coordinator is what starts
executions — the first attempt and the rebuild alike. Third, a rebuild continues on the runtime
that ran the original task, looked up by that journaled id; if the runtime cannot be named, the
rebuild is refused rather than redirected. The probe stages the failure the design calls out — a
worker that dies mid-turn — and checks the decision, the runtime, the checkpoint and the fact that
the rebuilt execution actually ran.

## D-056: The runtime is chosen explicitly, and the stack starts it before the API server
Status: accepted
Context: the lab stack always started the mock Hermes, so the pinned runtime was reachable only
through the Phase 0 runners, and the console could not be pointed at it. Two things made "just
connect to whatever is on 8642" wrong. First, the mode has to be explicit: a stack that silently
attached to a real runtime would report the real runtime's answers under the mock's name, and the
difference between "the UI works" and "the pinned runtime works" is exactly what this lab exists
to prove. Second, the order matters: the API server reads the runtime's capabilities at boot, so
starting the API first reports an unreachable runtime for a runtime that was merely still
starting.
Decision: the runtime is a mode, not a guess — `CODEWEAVER_RUNTIME=mock|hermes` in the server and
`--runtime mock|hermes` in `scripts/dev.py`. In `hermes` mode the stack starts the runtime, waits
for it to answer `/v1/capabilities`, and only then starts the API server. The bearer token comes
from the Hermes profile's own `.env`, never from a command line: a credential on a command line
ends up in shell history and in the process list. The stack stops the runtime on exit only if it
was the one that brought it up; a runtime that was already running belongs to whoever started it.
The runtime's own lifecycle lives in `scripts/hermes.ps1`, which records the supervisor PID,
resolves the listener separately (the gateway forks, so the process that answers on the port is
not the process that was started), and refuses to stop a gateway it did not start unless `-Force`
is given.

## D-057: A path that was never exercised is not a path that works
Status: accepted
Context: `scripts/install-hermes.ps1` looked for `hermes.lock.json` two directories above itself,
which was correct while the lab was named `docuflow-hermes-lab` and wrong after the rename. The
script had therefore been failing at its first line — "lock file not found" — and nobody noticed,
because the runtime it installs was already installed from an earlier session. A script that
cannot run is not a script that is merely unused.
Decision: the lock file is resolved relative to the lab root, the same place the version check
reads it from, and the failure is stated rather than tolerated. The general rule this records: a
script in this repository is evidence only when it has been run. The lifecycle script added in
D-056 was exercised end to end — start, status, stop, the refusal to stop a foreign gateway, and
a full task through the real runtime — before any of it was described as working.

## D-058: A background runtime's output is captured through a file, not a pipe
Status: accepted
Context: `scripts/dev.py` started the real Hermes runtime by running `hermes.ps1 start` with
`subprocess.run(..., stdout=PIPE)`. The stack hung there: Hermes was up and healthy, the API
server never started, and the log stopped at the runtime step. The cause is the pipe's lifetime,
not the script's: the gateway that `hermes.ps1` starts inherits the script's stdout, and a pipe
stays open for as long as any process holds it. `subprocess.run` waits for EOF, and the gateway
outlives the script by design, so the pipe never closed. The same trap is why a "the child
exited but the parent is still waiting" symptom reads as a hang in the wrong place.
Decision: the lifecycle script's output is captured through a file, which the caller reads
afterwards. A file has no reader-blocking semantics, so the call returns when the script returns
regardless of what the runtime inherited. The general rule: a process that is meant to outlive its
launcher must never hold the launcher's stdout pipe. The teardown path is verified separately,
because a stack killed with `taskkill /F` never reaches its `finally` block at all — so
`dev.py --check-runtime` walks the three states the stack depends on (down, up, down) and reports
which one it reached.

## D-059: On Windows the stack is stopped by a file, not by a signal
Status: accepted
Context: the dev stack's teardown never ran when the stack was stopped from another process. The
symptom was precise: the API server and the frontend kept their ports, the runtime kept running,
and the console output stopped at "running" without ever printing the teardown line. The cause is
that `Popen.terminate()` on Windows calls `TerminateProcess` — it does not raise a signal, so
`SIGTERM` is never delivered and the handler never runs. A `SIGTERM` handler therefore looks
correct and is never called; only `Ctrl+C` in the console itself reaches the process as SIGINT.
Two smaller faults compounded it: `psutil.wait_procs` raises `TimeoutExpired` for a process that
refuses to die, and that exception sat inside the `finally` block, so one stuck process skipped
every cleanup step after it.
Decision: the running stack watches a stop file (`.dev-logs/stop.request`). Writing the file asks
the stack to stop, and the stack cleans up after itself — releasing both ports, and stopping the
Hermes runtime if it was the one that brought it up. `dev.py --stop` writes that file first and
falls back to killing recorded PIDs only when no live instance answers. Every step of the
teardown is independently guarded, because one failure must not skip the rest. The general rule: a
cleanup path that is only reachable from the console is a cleanup path nobody exercises; it needs
a mechanism that another process can trigger, and it needs a test that triggers it.

## D-060: The tool surface belongs to Pi; Hermes does not get an equivalent
Status: accepted
Context: D-042 chose Pi as the primary runtime candidate and kept Hermes as the remote-runtime
reference. The tool surface (`read_file`, `request_diff`, `request_apply_diff`,
`evaluate_changes`) is CodeWeaver's own, and it is already proven end to end on Pi against a real
MiniMax M3 model — along with the four agent-level security probes, cancellation and session
recovery. Hermes 0.23.0 does support `mcp_servers`, so exposing the same tools to it was
technically reachable. It was nonetheless proposed as the largest remaining capability gap, and
that proposal reopened a decision D-042 had already made. The reasoning behind it was that
"multi-runtime" means every runtime can do the same work — but the runtime that carries the
product is Pi, and the capabilities the tool surface buys (a denial before the tool runs,
`AbortSignal` cancellation, a transcript that survives) are native to Pi and absent from the
Hermes HTTP surface, which is exactly why D-005 declares them unavailable there.
Decision: the tool surface is provided on Pi only; Hermes does not get an equivalent MCP
exposure. Hermes is kept as the remote-runtime reference — it is the only instance of the
"remote HTTP + SSE" shape, and its adapter is the evidence that the adapter contract holds in
that shape (run creation, SSE event stream, terminal recognition, approval round-trip). That
evidence is not weakened by not exposing tools to it. The consequence for the work queue: the
remaining checklist items are signed off against Pi, and the list is stated as "Pi signed off,
Hermes referenced" rather than "both runtimes must be completed". A reference runtime is not a
half-built product path, and treating it as one is what put an unnecessary MCP integration on the
critical path.

The decision was checked against the running product rather than against the documents: with the
dev stack in `--runtime pi` mode the console lists `pi-local` (`runtimeType=native`,
`version=MiniMax-M3`, `provider=minimax-cn`) beside the reference runtime, and a real task on
`pi-local` had the model call `read_file` itself — the tool returned the actual file and the model
answered from it. The tool surface is live inside a real execution on the runtime the product
runs on, which is what the choice was supposed to buy.

## D-061: A skill is its body; a bound skill is a delivered skill
Status: accepted
Context: the skill layer looked finished and was not. The registry validated manifests, the
authorization checked permissions and runtimes against the policy snapshot, the binding store
recorded an immutable binding per execution, and the console had a skill page — but
`SkillManifest` carried no body at all. The console computed a SHA-256 over the Markdown the user
typed and sent the digest without the text; the server had nothing to check the digest against and
nothing to deliver. The skill's instructions existed only in the console's text box, and the model
never saw them. The same gap made the checksum meaningless: it could not detect an edit, because
there was no text to compare against.
Two further defects sat behind it. The authorization context named a fixed runtime id
(`hermes-local`) instead of the runtime the task actually ran on, so a skill declared for `pi-local`
was refused on a Pi run and a `hermes-local` skill was allowed there. And the registry was held in
memory only, so every restart emptied the skill library: a skill registered yesterday was gone
today, which makes the whole surface a demo.
Decision: the manifest carries the body. `content` is required, the checksum is the SHA-256 of the
body, and the registry refuses a manifest whose checksum does not match its content — the
immutability the versions were supposed to have becomes checkable instead of asserted. Delivery
has exactly one channel, the task prompt: the Hermes HTTP surface accepts a single `input` string
and no system prompt, so a second channel would have meant two behaviours for one feature. The
body is appended after the user's prompt, the prompt snapshot hashes the composed text, and every
delivery is journaled as `skill.delivered` with its checksum and mode — a binding without a
delivery is a skill that was authorized and then dropped, and that has to be visible. The
authorization context is the runtime the task will run under, resolved before the skills are
checked; when it cannot be resolved, a runtime-specific skill is denied rather than assumed.
The registry is persisted, so a registered skill survives the restart that would otherwise have
emptied it.
Evidence: the probe runs the same task twice on the real model — once without the skill, once with
it — and the token it looks for exists only inside the skill body, so the control run cannot
produce it by accident. Against a real MiniMax M3 on `pi-local` the control answer did not contain
the token and the instructed answer did (`controlOutputHasToken=false`,
`instructedOutputHasToken=true`), the delivery was recorded as `prompt_inline`, and the two runs
had different prompt hashes. Report: `reports/phase0-pi-skill-delivery.json`. The probe's own
verdict logic is covered by unit tests, including the case where the control run produces the
token anyway, which fails the probe rather than crediting it.
Consequence: a skill is no longer a permission declaration with nothing to say. It is a body the
model reads, and the console can show what that body is.
## D-062: A delegated child runs on the runtime its parent ran on
Status: accepted
Context: the delegation endpoint was wired, the manager existed and the console had no way to see
the result. The first real run of the P0-008 probe found the defect that had been hiding behind
that: `SubagentManager.spawn` called `coordinator.start(childInput)` without an adapter, so the
child landed on the server's default runtime. With Pi as the default in the probe and Hermes
registered beside it, the child was routed to Hermes and refused with "Hermes run creation
failed" — a 502 that looked like a runtime outage and was nothing of the kind. The parent, which
the caller had deliberately placed on `pi-local`, had its child executed somewhere else, under a
different policy, a different tool surface and a different model. That is the failure mode D-047
was written to prevent, one level down: a named runtime is honoured or refused, and delegation is
not a way around it. D-055 had already settled the same question for recovery ("the rebuild runs
where the task ran"); delegation is the same fact wearing different clothes.
Decision: the child is started on the adapter that owns the parent execution. The manager reads it
from the coordinator rather than taking it from the caller, so the runtime cannot be chosen twice
or disagreed about. The spawn audit fact records the runtime id, so a reader can see where the
child ran without reconstructing it.
Evidence: with the fix, the P0-008 probe passes on the real model. The parent ran on `pi-local`
and produced output; the spawn returned 201; the child ran on `pi-local` as well, in an isolated
worktree, and reached `succeeded`; the lineage resolved from the child to the parent and from the
parent to the root in both directions; the parent's `subagent.spawned` fact and the child's
`subagent.parent` fact are both in the journal. Report:
`reports/phase0-pi-subagent.json`. The console gained a delegation page that shows the children
and the lineage of a chosen parent, so the structure is visible without reading the journal.
Consequence: P0-008 is signed off against Pi, and the Phase 0 count is 10 of 24. The rule worth
keeping: delegation inherits the runtime, it never chooses one.
## D-063: A runtime is scored by what it did; a claim is not a capability
Status: accepted
Context: the scheduler had three strategies and the console had a runtime picker, and both were
invisible as claims: the picker is "one runtime, chosen by hand", and nothing said how the two
registered runtimes compared. There was no capability score at all — `skillEffectiveness()` scores
skills, and the only capability numbers in the system were the runtime's own declaration.
`getCapabilities()` answers "what does this runtime claim?", and a claim is the one thing a
runtime controls, so it cannot be evidence about the runtime.
The gap had a second half that was harder to notice: two runtimes could be registered and one
could be picked, but nothing had ever proven that the second runtime was reachable, that naming it
put the task on it, or that naming a runtime that does not exist was refused rather than quietly
redirected. D-047 said the last one was the rule; there was no evidence that the rule held when two
real runtimes were registered at once.
Decision: a runtime's quality is a number computed from what the journal recorded, and the
declaration is an input rather than the answer. Three dimensions of the score come straight from
the decision log: reliability counts only decided outcomes and a failure is a failure (D-020);
cancellation counts only cancellations the runtime itself confirmed (D-052); capability
realization counts only capabilities that were declared, were put in play by the task, and were
demonstrated in the records (D-043, D-008). A dimension with no evidence is dropped and its weight
is redistributed, so a runtime is never punished for a measurement nobody took — and a runtime with
no evidence at all has no score, which is "unmeasured", not "bad".
The "put in play by the task" part is the subtle one. Scoring the observed set alone would credit a
runtime for capabilities nobody asked of it and punish it for ones the model chose not to use, so
the caller states what the workload exercised: tools were bound, a skill was bound, a subagent was
spawned, a stop was requested, a run was resumed. What the runtime then did with that chance is
what the records say.
The console shows the score, the dimensions with their stated reasons, and — because a score
computed from one measured dimension is a number that reads too well — how many of the five
dimensions actually had evidence. The known caveat is written down rather than hidden: a runtime
with only reliability measured scores 100, and the coverage line is what stops that from being
read as a verdict.
Evidence: the probe runs against both real runtimes at once — the pinned Hermes 0.23.0 gateway over
HTTP and Pi in-process on a real MiniMax M3. All six checks passed. `round_robin` alternated
`hermes-local > pi-local > hermes-local`; `least_loaded` picked `hermes-local` while the load was
equal and switched to `pi-local` once `hermes-local` was marked busy; `first_healthy` picked
`hermes-local`. The capability filter runs before the balancing (D-025): requiring
`toolEvents=true` excluded `hermes-local`, which declares only `streaming`, and selected
`pi-local`. Naming `pi-local` ran on `pi-local` and succeeded in 4.0 s with the requested token in
the output; naming `hermes-local` ran on `hermes-local` and succeeded in 7.1 s; naming
`no-such-runtime` was refused with `Unknown runtime no-such-runtime` and no execution was created.
The two runtimes then scored from the journal: `hermes-local` 12 417 tokens per successful
execution at 6 648 ms average, `pi-local` 208 tokens at 3 706 ms — a real, reproducible difference
in cost and latency that no declaration could have shown. Report:
`reports/phase0-multi-runtime-policy.json`. The probe's verdict logic has unit tests, including the
cases where the server runs the task on a runtime nobody named, accepts a runtime that does not
exist, or where round robin has nothing to alternate to — a probe that cannot fail is not
evidence.
Consequence: the runtime registry page carries a score that is computed from records and shows
what it was computed from. The score does not join the 24-item Phase 0 list: it is a new kind of
evidence with its own id and its own report, the same way skill delivery was kept out of the
denominator.
## D-064: A checklist item is signed off by what the agent did, not by what the server logged
Status: accepted
Context: the Phase 0 list has 24 items and 12 were still unrun. Two of them are about the agent's
own behaviour rather than about the server's plumbing: P0-005 "single-step task" and P0-006
"multi-tool task". The lab already had a tool-chain script, but a script is not a checklist item:
it printed its own booleans, it was not in the report set the aggregate reads, and nothing in it
was written to be wrong on purpose.
The interesting part is what "correct" and "in order" have to mean before they can be checked.
"Correct" is the hard one: an answer can be wrong, and a check written as "the answer is
reasonable" checks nothing. The only answers worth asserting on are ones that could not have been
produced without the work being done.
Decision: P0-005 asks the model to read a file and reply with its content, and the file holds a
nonce that exists nowhere else — not in the prompt, not in the task text. The check is therefore
two-sided: the runtime must have emitted exactly one kind of tool call (`read_file`), and the
answer must contain the nonce. A model that guessed, a server that echoed the instruction and a
run that never touched the file all fail the same check. The first version of this probe put the
nonce into the prompt as well, which would have made the check pass for a model that never read
anything; that was wrong and was removed before the probe was run.
P0-006 claims the *order* of the tool calls, so the evidence is the ordered list of tool events,
and the order is only half of it: the change has to be in the file, a change record has to exist,
an independent command has to have passed, and the completion gate has to be open. A chain that
runs the tools and produces nothing is not a finished task.
The verdict logic is unit tested, and most of those tests are the ones that have to fail: a file
that was never read, an answer without the nonce, tools in the wrong order, a missing tool, a
change that never landed, a missing change record, an acceptance check that did not pass, and a
completion gate that stayed shut.
One correction belongs on the record: the first real run of P0-005 failed, and the model was
right. The verdict counted a `tool.started` and its `tool.completed` as two tool calls, so a
correct single read looked like two reads. The probe's verdict was wrong, not the agent. The check
now asks what the checklist item asks — one step, one kind of tool — rather than pinning a number
the model has no reason to respect.
Evidence: both items pass on the real model through the HTTP API. P0-005: the execution reached
`succeeded`, the journal holds one `read_file` call (`started` at sequence 19, `completed` at 20),
and the answer carried the file's nonce. P0-006: the tool events ran `read_file` then
`request_apply_diff` then `evaluate_changes`, the file holds the new value, one change record was
written, the acceptance check returned `passed` and the completion gate opened. Report:
`reports/phase0-pi-agent-execution.json`. Unlike skill delivery and the multi-runtime policy,
these are checklist items, so the report joins the Phase 0 aggregate and the count moves from 10
of 24 to 12 of 24.
Consequence: the checklist can carry items that mean "the agent did the thing". The rule worth
keeping: an answer that could have been produced without the work is not evidence, and neither is
a tool call that proves nothing landed.
## D-065: A second turn is the same session; a new execution with the same text is not
Status: accepted
Context: P0-007 asks for "session continuity evidence", which is the part that is easy to fake. A
second execution launched with the same text produces a second turn that shares nothing with the
first — it starts from an empty transcript — so it would look like continuity while carrying
none. And the mechanism did not exist to begin with: the adapter contract had no way to send a
follow-up to a finished execution, the coordinator had no session to continue, and the console had
no way to ask.
Decision: continuation is an optional adapter capability — `followUp?(executionId, prompt)` — and
it is answered by the runtime that owns the execution, never by the server's default runtime. Pi
implements it on the same agent instance, so the transcript survives the turn boundary. Hermes
does not implement it, and the API answers 501 rather than quietly starting a fresh run that only
looks like a continuation; that is D-047 one level down. The coordinator also requires a live
session: an execution recorded by an earlier server process answers 409, because the transcript
lived in the process that is gone. Every accepted follow-up writes a `turn.started` audit fact,
so the journal shows where one turn ends and the next begins — without a boundary, "the second
turn" is not even a thing that can be located.
The judgement is two-sided on purpose. The first turn is asked to remember two strings and to
answer with nothing but "READY"; the follow-up asks for one of them. The second turn must carry
the string that appeared only in the first prompt, and must not carry the one it was never asked
for — an answer that dumps the whole context is a failure, not a stronger pass, because dumping
the context is not answering the question. The second turn's text is cut at the turn boundary, so
a model that repeats its previous answer cannot pass by accident.
Evidence: the probe passes on the real model through the HTTP API. The first turn ended
`agent.execution.completed`; the follow-up was accepted with 202; the second turn ended
`agent.execution.completed`; the journal carries the turn marker; the second answer was 32
characters long and carried the string it was asked for and not the one it was not. Report:
`reports/phase0-pi-multi-turn.json`. The verdict logic is unit tested, and most of those tests
are the ones that have to fail: a refused follow-up, a second turn without the string, a second
turn that dumps the context, a journal without a turn boundary, and a second turn that never
completed. The HTTP surface has four more: 202 when the runtime can continue, 501 when it cannot,
400 on an empty prompt, 409 when the execution is not live in this process.
Consequence: P0-007 is signed off against Pi and the Phase 0 count moves from 12 of 24 to 13 of
24. The console gained a "继续这一轮" card on the execution detail page, so a follow-up is
something a user does rather than something the journal proves happened. The rule worth keeping:
a follow-up continues a session, and never starts a new one that only looks like it does.
## D-066: The trace is the chain, and it can be walked from both ends
Status: accepted
Context: P0-024 asks for end-to-end traceability, and design 15 draws the chain as Task → Run →
Tool → Change → Commit → Test. The traceability query answered part of it: a commit resolved back
to the execution it came from, and an execution resolved to its context, prompt and policy
snapshots and its evaluations. Three links were missing, and the way they were missing was
instructive.
The task and the run were not absent from the records — they were absent from the *query*. The
journal had `execution.input` (the task) and `execution.runtime` (the runtime) written as audit
facts from the first attempt onwards, but nothing read them back into the trace. The tool calls
were worse: they were in the journal as runtime events, and the trace did not look at the journal
at all. It read four record stores, and the timeline was not one of them.
The tool link brought a second gap with it. Design 15 asks what a tool call was answered with —
the permission decision. That decision was computed and enforced, but never recorded: the journal
had `permission.requested` and nothing else, and the checkpoint had a branch for
`permission.resolved` that no code ever wrote. So the records could show that the runtime asked
and could not show what CodeWeaver answered, which is the difference between an enforced denial
and a request nobody settled.
Decision: the decision becomes a runtime event. `permission.resolved` joins the event union, and
the adapter emits it when the answer arrives — inline from the policy handler or later through the
API, whichever path answered. The request and its answer then sit on the same timeline, and the
checkpoint's unresolved-permission bookkeeping starts working for the first time. The Pi adapter
also carries the tool call id on both the request and the decision, so the pairing is exact
rather than a guess from the tool name.
The chain itself is read from the journal, not from a second bookkeeping structure. A record that
is maintained next to the timeline can disagree with it, and a trace that disagrees with the
timeline is worse than no trace: it looks like evidence. The journal is the only place the
timeline exists, so it is the only place the task, the run and the tool calls come from.
The chain is judged link by link, and the two round trips are links in their own right: the task
must list the execution it produced, and the commit must resolve back to the same execution the
chain started from. A chain that can only be read in one direction is a log, not traceability.
The task query also became reachable: `GET /api/v1/tasks/{taskId}/trace` answers design 15's
"从任务查询所有 Hermes 执行记录", and it finds executions that no record store links yet, because
the journal knows which task a run belonged to even when the run died before its first snapshot.
Evidence: the probe passes on the real model through the HTTP API. One run read `app.ts`, applied
a diff and had the acceptance checks executed; every link was then read back out of the server's
records. The task `task-p0-024` names the execution; the execution names the runtime `pi-local`;
the tool calls are `read_file > request_apply_diff > evaluate_changes`, in that order; one change
record exists; the change landed in commit `109fa849d47c988645c5766870c2d9e66bb730ad`; the
evaluation verdict is `passed`; the task lists the execution; and the commit resolves back to that
same execution. Report: `reports/phase0-pi-traceability.json`. The verdict logic is unit tested,
and most of those tests are the ones that have to fail: a trace that names the wrong task, a trace
that names no runtime, a missing tool call, tools in the wrong order, no change record, a change
that never became a commit, an evaluation that did not pass, a task that does not list the
execution, and a commit that resolves somewhere else.
Consequence: P0-024 is signed off against Pi and the Phase 0 count moves from 13 of 24 to 14 of
24. The console's traceability page now shows the chain as a chain — one row per link, with a
missing link shown as missing rather than dropped — and it can be entered from a commit, an
execution or a task. The rule worth keeping: a chain is only evidence if it can be walked from
both ends.

## D-067: An execution that was never recorded is refused, not answered with an empty chain
Status: accepted
Context: D-066 made the console's traceability page show a chain link by link, and made a missing
link visible as missing rather than dropped. That rule is right inside a chain. It was applied one
level too far, and the browser check showed it.
`GET /api/v1/executions/{id}/trace` answered every id with 200 and a chain of six empty links. A
mistyped execution id therefore looked exactly like a real run that happened to produce no tool
calls, no change, no commit and no evaluation — every link "无记录 / not_run", every status green
in the sense that nothing failed. An auditor reading that page cannot tell a typo from an empty
run, and an empty run is not a neutral fact: it is the difference between "this ran" and "this
never happened".
The rule that was already in the codebase said the opposite. `GET /api/v1/executions/{id}` holds
that "a real execution always journals at least `execution.started`; an empty journal means
unknown", and refuses the id with 404. The trace endpoint answered a different question without
saying so.
Decision: the trace endpoint applies the same rule as the execution endpoint. An execution whose
timeline is empty never existed as far as the API is concerned, so the trace is a 404 with the
server's own words, not a chain of holes. The check goes through the coordinator rather than
straight to the journal, because the coordinator flushes the pending writes of a live execution
first — a running execution has not written its journal yet, and it must not be refused as one
that never existed.
The task endpoint stays a 200 with an empty list, and that is a deliberate difference: a task
that produced no execution is a real state (the run died before it wrote anything), while a
mistyped execution id is not a state at all. The console therefore says "没有执行记录" for the
task and shows the refusal for the execution.
Two smaller things came out of the same check. The console collected the change record for a
commit lookup and then never passed it to the card that renders it, so the commit entry showed
the chain but not the change it was about; it is passed now. And a task lookup that found nothing
rendered nothing at all — no card, no heading, as if the query had never happened — so it gets an
explicit empty state.
Evidence: the probe now has a ninth link, `not-found`, and it is judged on the real server: an id
that was never recorded must be answered with 404. The run passed on the real model through the
HTTP API; `missingTraceStatus=404` and all nine links hold. Report:
`reports/phase0-pi-traceability.json`. The verdict logic is unit tested, and the new case is one
of the ones that has to fail: a server that answers 200 with an empty chain for an id that was
never recorded breaks the probe. The HTTP surface has one more: the trace of an unknown execution
is a 404 with `execution_not_found`, while the trace of a recorded one is a 200. The console was
checked in the browser on all three entrances — commit, execution and task — plus both wrong
answers: a mistyped execution id shows the refusal, and a task with no executions shows the empty
state.
Consequence: P0-024 stays signed off, and the rule it rests on gets its second half: a chain is
evidence only if a missing link means "this link was empty", never "this was never here".

## D-068: The console's default task has to be a real task
Status: accepted
Context: the traceability page was verified in the browser, and the verification went through the
console's own "新建执行" dialog — which is where the product's default path lives. The first run
of it produced a successful execution on `hermes-local` with ten events and the answer "hello
world". Nothing was wrong with the model, the runtime or the task. The dialog's defaults simply
described a different product than the one the project is building, in four separate ways.
The allowed tools were `read_file,search_code`. The Pi adapter refuses a tool that is not on that
list before it runs ("tool request_apply_diff is not in the allowed tool list"), so the three
change tools — `request_diff`, `request_apply_diff`, `evaluate_changes` — were all blocked, and
`search_code` was on the list although CodeWeaver does not provide it. A task created through the
console could read and could not change anything, which is the opposite of what the product is.
The granted capabilities were `read_file,search_code` as well. The policy table grants
`modify_worktree` at P1, and the Code Engine checks it before it writes, so even a tool that got
through the list would have been refused ("Operation 'modify_worktree' is not granted by
snapshot"). The allowed paths were empty. A P1 write has to land inside a path prefix the
snapshot authorizes, and an empty list authorizes nothing ("Path ... is outside every authorized
path in the snapshot"). The runtime was "自动选择". The scheduler picks by load and, at a tie, by
registration order, and `hermes-local` is registered first — and in the Pi development stack
`hermes-local` is the mock, which answers "hello world" to anything and looks successful without
having done anything.
So the default path of the product was a path that does nothing, and it looked like success. That
is the same failure as a mock counted as evidence: a wrong answer that no one can tell from a
right one.
Decision: the defaults describe the product's own path. The allowed tools are the four
CodeWeaver tools. The granted capabilities are the ones the path needs — reading, diffing and
writing inside the worktree — and not the wildcard, because the wildcard also grants the P2 and
P3 operations that are supposed to ask a human. The allowed paths fall back to the worktree the
task was pointed at: the user already named that scope, and the advanced field can narrow it. The
runtime defaults to `pi-local` when it is registered, because that is the primary runtime
(D-042, D-060); "自动选择" stays as the fallback for a stack without it, and the picker still
offers every runtime, so nothing is hidden and nothing is silently redirected (D-047).
Evidence: the same dialog, with the advanced section untouched, now runs a real task. The task
`TASK-DEFAULT-02` ran on `pi-local`; the tool calls were `read_file > request_diff >
request_apply_diff > evaluate_changes`, all completed; the change landed in commit
`c332a9a26e14d634e3f562b61966095aaeeecfdc`; the acceptance check reported `passed: 1/1 checks
passed`; and the repository's `app.ts` went from `export const value = 1;` to
`export const value = 2;`. The run before it, with the old defaults, was `hermes-local` with the
answer "hello world" and a terminal state of succeeded. The rule worth keeping: a default is
not a placeholder — it is the path most people will take, so it has to be the real one, and it
has to fail loudly when it cannot be.

## D-069: The live stream and the replay are two readings of one journal
Status: accepted
Context: design 13 makes the server the recorder and the socket a notification channel: the client
reconnects with an `executionId` and a `since`, and the server replays what it missed. The promise
was in the design and nowhere in the code — the three pieces that had to agree did not.
The live stream and the replay spoke two different sequence spaces. A live frame carried the
runtime's own event sequence; a replayed frame carried the journal position. `since` was a
journal position, so the number a client was told to resume from was a number it could not compare
against the frames it had already received. The console's execution page merged the history and
the live stream by sequence, which under that rule could not tell a replayed frame from a live one.
The live stream also announced events before they were recorded. An event that was published
before its journal write, to a client that then dropped, was in neither the replay nor the live
stream: the client could never learn that it had happened. And the server read the journal before
it subscribed, so every event published between those two steps fell into the same hole.
Decision: the journal is the only sequence space, and the live stream carries what was recorded.
The runtime event and the CodeWeaver audit fact are written to the journal first and published
afterwards, as the journal entry itself — so a live frame's `sequence` is the journal position and
its `id` is `${executionId}:${journalPosition}`. Design 13's own event table lists
`code.diff.applied` and `evaluation.completed` next to the agent events, so the audit facts go on
the same stream; the replay and the live stream are then the same thing read two ways, and a
client has exactly one number to compare. The server subscribes before it replays: the frames
that arrive while the replay is written wait in a buffer and are flushed afterwards, minus the
frames the replay already carried, so no event is lost and none is delivered twice.
`GET /api/v1/executions` was added to the same rule — the journal is what the server knows, so
the execution list is read from the journal rather than from a second bookkeeping structure.
Evidence: the probe `pi-client-disconnect-phase0` runs a real task on the real model, opens the
event stream, drops the connection in the middle of the run, waits for the execution to finish on
its own, and comes back with `since`. The journal is the yardstick: the client's timeline is
compared against the entries the server actually recorded, so a stream that quietly dropped half
the run cannot pass. The last run received 9 frames before the disconnect, resumed from 9,
received 98 more, and the journal held 107 entries — 9 + 98 = 107, every entry held exactly once,
one execution, the change landed in commit `a5016c371a4e41e190e0c0584cf7029676dbed65` and the
acceptance check passed. The five links are `continues`, `complete`, `no-duplicate`,
`single-execution` and `evidence`, and a run that finished before the client could disconnect is
`blocked`, not `passed`: the probe never got to test the thing (D-020).
Two things had to be true of the verdict itself. The `complete` link used to pass on an empty
journal — nothing to miss is nothing missing — so it now also demands that the server recorded
something. The `no-duplicate` link counted the deduplicated timeline, which cannot hold a repeat
by construction; it now counts the raw deliveries, because a client that keys on the frame id
hides a repeated frame and only the probe can see it. The probe was then broken on purpose twice,
because a probe that cannot fail is not evidence. Replaying one position too many
(`sequence >= since`) made the client receive event 9 twice and the `no-duplicate` link fail.
Switching the replay off made the client never receive 114 of the 118 recorded entries and the
`complete` link fail. Both runs exited non-zero, and both were restored before the signed-off run.
The server-side order has its own test, `src/tests/unit/event-reconnect.test.ts`, whose two race
cases fail on the old order (the client receives `[1]` where it should receive `[1,2]`).
The console was checked in the browser, with a real disconnect rather than a simulated one: the API
process was killed while the console was open. The footer said `重连中` instead of `实时`, the API
was started again, and the console reconnected on its own — the backoff ladder is 1s, 2s, 4s, 8s —
and the execution list came back intact, because it is read from the server's journal rather than
from what the socket happened to carry. The detail page of an execution with 146 journal entries
showed all 146, numbered `#1` to `#146` with nothing repeated in between: the history and the live
stream are the same sequence space, so merging them cannot double an entry.
Consequence: P0-019 is signed off. The rule it rests on: the live stream and the replay are two
readings of one journal, so a client that comes back gets exactly what it missed — and exactly
once.

## D-070: A crashed runtime ends the execution; it does not get another turn
Status: accepted
Context: design 14.2 lists eight ways a run can go wrong and says they cannot all be answered with
the same recovery move. Three of them are now pinned down by evidence, and this is the third.
P0-021 pinned "the CodeWeaver worker crashed": the session is gone with the process, so the answer
is rebuild from the checkpoint. P0-019 pinned "the client disconnected": the execution is fine, so
the answer is replay from the journal. This entry pins "the runtime process crashed": the worker is
still alive, the runtime is gone, and the execution has to end in a failure that says why.

The obvious way to test it would have proved the wrong thing. Pi runs in-process inside CodeWeaver,
so killing Pi kills the worker — that is P0-021's scenario wearing a different hat. A runtime crash
needs a runtime that lives in another process, so the probe uses the pinned Hermes gateway
(`scripts/hermes.ps1`), speaking HTTP and SSE. It starts the gateway, runs a real task through it,
kills the gateway process while the model is still answering, and then asks the questions an
operator would ask.

Two of those questions came back wrong, and both were real defects. The first was the recovery
path: `recover()` asked the runtime what it could do before deciding anything, so once the runtime
was dead the answer came back 502 "Hermes capabilities request failed" — a runtime that could not
be reached, reported in place of the fact that mattered, which is that this execution had already
ended. The 502 was true and useless. The decision now reads the execution's own checkpoint first: a
terminal execution gets a `stop` plan without asking the runtime anything. The runtime is consulted
only when its answer could still change the decision.

The second was the execution list. It derived the status from the coordinator's live session, and
that session is only removed when the coordinator is terminated — so an execution that had finished
in this process stayed `running` forever, and the console's list filled up with work that was
already done still showing as "执行中". The journal's terminal event is the fact; `isLive` is now
only the fallback for an execution that has no terminal event yet.

Decision: a crashed runtime ends its execution in `failed`, with the failure and its reason
recorded, and it never gets another turn. The journal keeps the whole story of the run, including
the crash, and stays readable after the failure. Recovery for a terminal execution is `stop`, decided
from the execution's own record, and the runtime is not asked at all.

Evidence: the probe `runtime-crash-phase0` runs a real task on the real model through the pinned
gateway and kills that gateway mid-run. Six links are asserted: `crash-was-real` (the process was
alive before the kill and gone after it), `crash-detected` (the execution ended as `failed`),
`crash-recorded` (the journal records the failure with the reason `stream_error`), `audit-kept`
(the failed execution's record is still readable — 20 journal entries, trace 200), `no-silent-rerun`
(the recovery attempt refused to re-run the model, 409, and the task still has exactly one
execution), and `runtime-unhealthy` (the registry reports the crashed runtime as unhealthy). The
last run: `runtimeAliveBeforeKill=true`, `runtimeAliveAfterKill=false`, 17 events before the crash
with the model's output already in them, `status=failed`, `journalEvents=20`, `traceStatus=200`,
`retryStatus=409` with `Recovery decided 'stop': Execution is already terminal (execution.failed)`,
`runtimeHealthy=false`, one execution on the task.

The probe was made to fail before it was trusted. Run against the unrepaired coordinator, the same
probe exited non-zero on `no-silent-rerun` — "the recovery attempt answered 502 instead of
refusing" — so the defect was caught by the probe rather than by inspection. The verdict logic has
13 unit tests, most of them the cases that have to fail: a run that never crashed, a run that ended
`running`, a journal with no reason, a trace that cannot be read, a retry that answers 200, a task
that gained a second execution, a registry that still calls the runtime healthy, and a model that
never produced any output — that last one is `blocked`, not `passed`, because a probe that never got
to the crash has not tested it (D-020).

The console was checked in the browser against the real gateway. A long task ran to completion and
the list showed "已成功"; a second task was killed two seconds after it started and the list showed
"已失败" with 85 events, while the execution that had finished in the same process no longer showed
as "执行中" — that was the second defect, visible in the product and not only in a test. The crashed
execution's detail page still scrolls to the end of its 85-entry timeline: a failed run keeps its
evidence.

The probe also owns what it kills. It records whether the gateway was running before it started and
restores that state afterwards — a gateway it started is stopped again, a gateway it found running
is brought back. A probe that leaves the machine in a different state than it found it has changed
the subject it was measuring.

Consequence: P0-020 is signed off. The rule it rests on: a crashed runtime ends the execution in
`failed` with the crash recorded, the audit trail survives, and the model is not asked again behind
the operator's back. The recovery decision reads the execution's own record; the runtime is only
asked when it could still change the answer.

## D-071: The audit timeline reconstructs a run from the recorded events
Status: accepted
Context: design 15 lists what a traceability layer must answer, and one of the answers was missing:
"从审计事件还原完整执行过程". P0-024 delivered the others — from a task, from a commit, from an
execution, the chain of records that belong together. But a chain is a set of links, not a sequence.
"Which records belong to this run" and "what happened, and in what order" are different questions,
and only the second one reconstructs a process. The journal held every event, but nothing could read
them back as a timeline: the execution page showed the live stream of one execution, the trace
showed the links of one execution, and nothing crossed executions at all.

Decision: the audit timeline is that entry point, and it is read from the journal — the same
append-only record the live stream replays (D-069), so a timeline can never disagree with what a
client was told while it was listening. Its order has two parts, because two orders exist: inside
one execution the order is the journal position and is never second-guessed; across executions the
order is the recorded time. The merge interleaves whole executions by time and never reorders an
execution's own events, so a timestamp that disagrees with the recorded order cannot scramble a run.
A hole in the journal is reported as a gap, and an execution whose timestamps disagree with its own
order is reported as such — the timeline says what it knows and where it is unsure.

Evidence: the probe `audit-timeline-phase0` runs the same task twice on the real model, so the
timeline has to hold more than one execution and put them in the recorded order. The yardstick is
the journal itself: the timeline must hold every recorded event of every execution exactly once —
a timeline that quietly dropped half a run would otherwise look complete. Seven links are
asserted: `journal-covered` (the timeline holds all 271 recorded events, once each, against the
counts the server's own journal endpoint reports), `ordered` (ordered by the recorded time, and
every execution keeps its own journal order), `correlated` (both runs of the task are on one
timeline and carry the task), `process` (the stages reconstruct as started → tool → change →
evaluation → completed), `evidence` (the commit and the passing verdict are readable from the
timeline itself — an audit timeline that only carried event names would not be a record of what
happened), `task-scope` (asking for the task returns the whole tree, not one execution) and
`not-found` (an execution that was never recorded is refused with a 404 instead of answered with
an empty timeline). The verdict logic has 14 unit tests, most of them the cases that have to fail:
a dropped event, an invented event, a repeated event, events running backwards in time, an
execution's own events reordered, one of the task's runs missing, a run that does not name the
task, stages that cannot be reconstructed, a timeline without the commit, a timeline without a
passing verdict, a task timeline that holds only one run, and an unknown execution answered with
200.

Two defects fell out of it, and the second one was the probe's own.

The first was the journal's. `recordedAt` meant two different things: for a runtime event it was
the time the runtime emitted the event, for a CodeWeaver fact it was the time the fact was
written. The two disagree — the runtime's first event is emitted before the facts that precede it
in the journal are written — so the journal's timestamps contradicted its own order, and a
timeline built on them could not be ordered. The fix gives the field one meaning: `recordedAt` is
now when the entry was written, which is the same clock the facts use and the same order the
journal position carries. The runtime's own time is not lost — it moves into the payload as
`eventTimestamp`, where it belongs. The two unit tests for this fail on the old implementation and
pass on the new one.

The second was the probe's. It waited for the runtime to say "succeeded" and then measured — but
the runtime says it before the journal has caught up, because the terminal event is written through
the coordinator's queue. The probe measured a moving target and the coverage link failed by
exactly the two entries that had not been written yet. A probe that measures a moving target is
not evidence of anything, so it now waits for the recorded terminal event and for the event count
to stop moving. The rule it follows is the one the rest of the system already follows: the
journal's terminal event is the fact, not the runtime's claim.

The console was checked in the browser against a real run. The new 「审计时间线」page reads the
timeline from the server, shows every execution with the stages it went through, and reports the
runs whose recorded timestamps disagree with their own order — which is what the executions
recorded before the fix look like. The run recorded after the fix shows no such warning, and its
99 events are ordered.

Consequence: P0-022 is signed off. The rule it rests on: a timeline is read from the journal and
nothing else, one execution keeps its journal order, the recorded time only interleaves
executions, and what the journal cannot vouch for is reported rather than smoothed away.
## D-072: One rule, one implementation: the ledger and the runtime score read the counters the same way
Status: accepted
Context: P0-023 asks for one thing — "能关联 Token、模型和执行时间" — and the lab already had the
ledger that answers it (`GET /api/v1/usage`, `GET /api/v1/executions/{id}/usage`), plus a runtime
score that read token counts of its own. Two readers. The ledger's reader knew the gateway's shape
(`usage.{input_tokens,output_tokens,total_tokens}`); the runtime score's reader knew the primary
runtime's shape (one set of counters per assistant message) and took only the first assistant
message. So the runtime the product actually runs on was never accounted for at all: the run the user
had just watched cost the model real tokens, and the cost page showed nothing for it. A cost page
that shows nothing for the runtime that did the work is not a cost page.

Decision: three rules, and the first one is the reason the other two exist.

1. One rule, one implementation. `src/accounting/token-usage.ts` is the only reader of a runtime's
   counters; the ledger and the runtime score both use it. Two readers are how a payload ends up
   accounted for in one place and not the other — and the runtime that was missed was the primary
   one. The primary runtime reports one set of counters per assistant message, because every
   assistant message is one model request, so a run that asked the model five times spent the sum of
   five sets. The first implementation read one of them and called it the run's cost.
2. The usage record is written from the journal, not from the live event stream. The journal is
   where a run's history actually is, and it is the only thing that survives a restart. The duration
   is the journal's own start→terminal span, so the number in the cost page and the span in the audit
   timeline are one number rather than two measurements of the same run. `onRecorded` is awaited, so
   "the run is over in the journal" and "the run is accounted for" are the same moment rather than a
   race with a write still in flight.
3. The model in the record is what the runtime said. The primary runtime names its model and the
   record carries that name; the gateway names none, and the record says `unknown` — a made-up model
   is worse than a missing one. The parts must also explain the total: prompt + completion +
   cached + cache-write = total, which is why the cache-write count was added to the record (the
   primary runtime's total includes it, and a total its parts cannot explain is a number nobody can
   check).

Evidence: the probe `accounting-phase0` runs both runtimes — the primary one in process against the
real model, and the remote one through the pinned gateway — and reads the cost back out of the
ledger. The yardstick is deliberately not the product's extractor: the probe reads the raw payload
keys itself, because a checker that reads a payload the same way the writer does cannot tell a
faithful transcription from a shared mistake. Thirteen links are asserted. On the primary runtime:
`recorded` (the ledger holds a record — the link that failed before the fix), `tokens-agree` (530
prompt + 105 completion + 764 cached = 1399, the numbers the runtime reported),
`model-named` (MiniMax-M3, the model the runtime said), `timing-agree` (2824ms is the journal's own
span), `tokens-explain`. On the gateway: the same links, except `model-unnamed` — the gateway named
no model, so the record says `unknown` instead of guessing — and 12404 + 8 = 12412 tokens over
7262ms. Then the ledger as a whole: `summary-consistent` (13811 tokens is the sum of the two
records), `model-groups`, `not-found` (an execution that was never run is refused with a 404, not
answered with an empty record that reads like a free run) and `durable` (the probe restarts the
server over the same data directory and both records are still there, with the same numbers). The
verdict logic has 15 unit tests, most of them the cases that have to fail: a run the ledger never
recorded, tokens that disagree with the runtime, a model the runtime never named, a duration that is
somebody else's, a total its parts do not explain, a summary that is not the sum, an unknown id
answered with 200, and a ledger that forgets the run after a restart.

Two things fell out of it, and the second one was the probe's own. The first was the product's: the
two readers described above, which left the primary runtime uncosted. The second was a probe that
asked the remote runtime to look at a file, so the gateway's agent went exploring and the arm did
not finish inside its budget — and the probe reported that as a failure, which is what it should
do. The fix was to ask the remote runtime for a word instead of a reading (a turn that answers in
seconds) and to give each arm its own budget, because a remote runtime answers slower than an
in-process one. A probe that cannot finish its scenario reports failed; it never reports passed.

The console's 「用量统计」page shows the cached tokens next to the prompt tokens, because the runtime
counts them separately and a total that cannot be explained by its parts is a number nobody can
check.

Consequence: P0-023 is signed off. The rule it rests on: one reader for the counters, the record
written from the journal, and every number in it — model included — exactly what the runtime said.
## D-073: A delegated child runs under a policy of its own, and two agents can run at the same time
Status: accepted
Context: P0-014 asks for "两个 Agent 同时执行" and the acceptance criterion is "各自独立运行". Two
claims hide in that sentence, and the second is the one a system can fake: a system that runs the two
tasks one after the other has run two agents, and it has never had to keep them apart. The lab could
answer "two agents ran" with two serialized runs and mean it. So the item is about concurrency *and*
separation, and a probe has to check both.

It also had a gap nobody had looked at. Delegation was signed off (P0-008) on the evidence that a
child is created, runs on the parent's runtime and is linked back to its parent. Nobody checked
whether the child could *do* anything — and it could not. A delegated child is started by the
subagent manager, not by the task service, so it has no session there, and the task service resolved
a policy snapshot per session. The child's policy travelled in its request and was journaled with it,
but nothing read it back: the child's tool surface had no policy to ask for, and every tool call
failed with "No policy snapshot is linked to execution …". A child that cannot read a file is not a
subagent; it is a second agent that answers from nothing.

Decision: two rules.

1. A delegated child's policy is its parent's capabilities, scoped to the child's own workspace. The
   capabilities are inherited because a delegation is the parent handing work over under the same
   policy; the scope is the child's own because a child that could act in the parent's workspace
   would not be isolated from it, and the tool surface is where that isolation is enforced. The
   policy is resolved from the journal — the child's request is there, and this is the same rule the
   runtime id and the task input already follow (D-055). Resolving it from the request rather than
   from a new registration means there is no window in which the child runs without a policy.
2. Parallelism is judged by overlap, and separation is judged by a nonce. Each workspace holds a
   string that exists nowhere else — not in the prompt, not in the other workspace, not in the
   model's head. Each agent has to read its own file and answer with its own string. If two runs
   shared a session, a context or a transcript, the answers would cross, and a crossed answer is not
   a cosmetic failure: it means one agent's work was attributed to the other.

Evidence: the probe `parallel-agents-phase0` runs two forms of real concurrency on the real model,
and judges five links per arm. Two top-level tasks, each in its own workspace, the second started
while the first was still running: the first started at 02:30:03.101 and the second at 02:30:03.295
against the first's finish at 02:30:05.381, and the probe read the first journal immediately after
the second run was created and saw it still running — two runs that never coexisted would not
satisfy the criterion, and the timestamps alone are not enough either. One parent delegating to two
children at the same time, each child in its own isolated worktree: both children were created
(201/201) with different worktrees, the second started at 02:30:18.980 against the first's finish at
02:30:22.616. Then the same three checks on both arms: `own-journal` (each journal holds only its
own run — 32/30 entries for the top-level runs, 29/26 for the children — the same count on the
shared audit timeline, so no event was lost, invented or attributed to the other run, and each run
recorded exactly one `execution.completed`), `own-answer` (each run answered with its own nonce:
NONCE-ALPHA-7f3a91 / NONCE-BETA-9c2e04 for the top-level runs, NONCE-CHILD-ALPHA-51c7de /
NONCE-CHILD-BETA-83fa20 for the children, the second pair only possible after the policy fix), and
`separate-usage` (the ledger holds one record per run — 1100 and 1090 tokens, 1114 and 1090). The
judgement has 11 unit tests, mostly the cases that have to fail: runs that never coexisted, an
overlap that was not observed, two executions sharing an id, a run that did not succeed, a crossed
answer, a run that ended twice, a timeline that lost or invented an event, a ledger that kept one
of the two runs, and a ledger that attributed one run's cost to the other.

The policy fix has two unit tests of its own, and the first one fails on the old implementation:
a delegated child resolves the policy it was given, scoped to its own workspace, and an execution
with no journaled request has no policy to read.

Consequence: P0-014 is signed off. The rule it rests on: two agents that run at the same time have
to be apart in every way the record can show — their journals, their answers, their cost — and a
delegated child runs under a policy of its own, inherited in capability and scoped in path.

## D-074: An execution's scope is its own worktree, not the directory it happens to run in
Status: accepted
Context: P0-015 asks for "不同 Worktree 文件访问" with the acceptance criterion "不发生跨工作区读取或修改".
The lab had a boundary, and the boundary was a directory: every read was checked against the
execution's working directory, every write against the same directory plus the policy's allowed
paths. That holds while worktrees sit next to each other. It stops holding the moment one sits
inside another — and one does. The lab's worktree root lives under the data directory, and the data
directory lives inside the checkout a task may be running in; a user can put a worktree inside their
own checkout with one git command. Both are ordinary, and both defeat a directory.

The probe found it before it was looking for it. A local experiment — two worktrees of one
repository, the second created inside the first, an agent in the first holding every capability a
policy can grant — read the other worktree's file and got its contents, and then applied a patch
that rewrote it. Both succeeded, and for the same reason both times: the boundary said "inside the
directory", and the file was inside the directory. The tool's own description had promised more
("Paths outside the worktree or outside the policy's allowed paths are denied"), and the read path
never asked the policy about paths at all.

Decision: two rules.

1. The scope of an execution is a worktree, and the rule is one line: the worktree that owns a path
   must be the execution's own worktree. The owner is the deepest worktree that contains the path,
   because a worktree inside a checkout is contained by the checkout as well, and the narrower one is
   the one that really owns the file. A path no worktree owns is not a cross-worktree access — the
   directory boundary still governs it — and an execution running in a subdirectory of a checkout
   belongs to that checkout, so a task in a subdirectory does not lose its own files.
2. The read path and the write path ask that question through the same function. A boundary the two
   sides answer differently is not a boundary; one of them will be wrong. When the scope cannot be
   established — git cannot list the worktrees — the read is refused rather than allowed: a scope
   that cannot be verified is not a scope with nothing in it.

Evidence: the probe `worktree-isolation-phase0` runs the real model on Pi and asks it four times to
cross a worktree, judging five links. The control comes first: the agent reads a file in its own
worktree and the answer carries the nonce that only exists there (NONCE-OWN-4b7e12) — without it, a
tool that refused everything would pass every other ring. Then a sibling worktree
(`outside_worktree`), then a worktree nested inside the agent's own (`outside_worktree_scope` — the
case that used to succeed), and then a change to a file in that nested worktree: the model made
eight calls, proposed the patch with `request_apply_diff`, and was refused; the other worktree's file
is still byte-identical (NONCE-NESTED-5c28f6) and its HEAD has not moved. A fifth ring has no model
in it at all: the Code Engine is handed a patch that aims at the nested worktree and refuses it by
name ("is inside worktree '…', which is not this execution's worktree '…'"), so the evidence does
not depend on how the model behaves. The judgement has 11 unit tests, mostly the cases that have to
fail: a refusal that never happened is blocked rather than passed (D-020), a call that was not
refused, the other worktree's content in the answer, the other worktree's content in the journal, the
other worktree's file changed, the other worktree given a commit, and an own-scope read that was
never attempted.

The rule itself has 8 unit tests, and the two that matter fail on the old implementation: reading a
file that belongs to another worktree, and applying a patch that targets one. Both were checked to
fail with the scope check switched off and to pass with it on.

Consequence: P0-015 is signed off. The rule it rests on: an execution may touch its own worktree
and nothing else, and "its own worktree" is an identity, not a directory.

## D-075: A dangerous command runs only because somebody approved that exact command
Status: accepted
Context: P0-012 asks for "执行危险命令" with the acceptance criterion "根据策略拦截或审批". The lab had
an approval mechanism, and it was not on this path. The Code Engine asks for a human decision
before it applies a risky change — a dependency, a secret, a deletion — and the API takes that
decision as an approval record. But the acceptance checks had no policy at all: the console takes a
command as free text, the Evaluation Engine spawns it as a real process, and the agent picks which
declared checks to run. "rm -rf", "del /s", "git push --force", "npm publish" — all of it ran,
because nobody between the console and the process asked whether this was the kind of command a
person should have said yes to.

The checklist says more than "block it": "审批只能授权具体操作，不能自动授予整个容器的无限权限",
and the emphasis above the table says the runtime's own approval is not the policy engine. Both cut
against a fix shaped like "dangerous commands are allowed if a flag is set".

Decision: three rules.

1. The command line is classified. Destructive programs (rm, del, rmdir, shred, mkfs, format,
   dd), privilege and permission changes (sudo, chown, icacls, chmod 777), process and system
   control (kill, taskkill, shutdown, systemctl), publishing (npm/cargo/twine publish, git push,
   git clean -fdx, docker system prune) and a download piped into a shell are dangerous. A shell
   (cmd /c, sh -c, powershell -Command) is dangerous because the policy cannot read what is behind
   it — the command line it hides is inspected as well, so "cmd /c del /f /q x" is caught by both
   rules.
2. A dangerous command runs only with an approval that names it. The approval carries the
   fingerprint of the command it was given for, so it cannot be moved to another command: approving
   "remove the build directory" is not approving "remove everything". A refused command is blocked
   before the process starts — not started and then judged afterwards.
3. The refusal is the product's own decision, and the agent is told why. The check's result
   carries the refusal and its reasons, the tool tells the model what happened (a verdict without
   a reason looks like a flaky check), and the completion gate stays closed, so a task whose only
   evidence was a refused command cannot be called done.

What this is not: a sandbox. The classifier reads the command line it is given, so a check that
hides its work inside a script file is beyond it — that is what the runtime isolation items
(P0-016, P0-017) are for. The promise here is narrower and honest: the dangerous commands people
actually type do not run unless somebody said yes to that exact line.

Evidence: the probe `command-approval-phase0` runs the real model on Pi and asks it four times to
run an acceptance check. The control comes first: an ordinary check ("node check.mjs") ran and
passed, and the completion gate opened ("All 1 check(s) passed"). Then the dangerous check with no
approval: the tool answered "refused: 'cmd /c del /f /q …' is classified as dangerous and no
approval was given for it ('cmd' runs a command line the policy cannot read; 'del' destroys data).
The command was not run.", the canary file was still there, and the gate stayed closed ("A check
could not run: blocked: remove the canary"). Then the same check with an approval for that exact
command: it ran, the check passed, the gate opened and the canary was gone. Then that same approval
attached to a different command: refused again — "the approval was given for a different command
(approved 749b1ef4d1e2, this command is 64b724b6cce2)" — and the second canary was still there. An
approval authorizes one operation, not a class of operations.

The judgement has 7 unit tests, mostly the cases that have to fail: a refusal that was never asked
for is blocked rather than passed (D-020), a dangerous command that was not refused, a refused
command that still destroyed the file, the completion gate staying open after a refusal, the safe
check not running, and the approved command not doing what it said. The classifier has 10 unit
tests, and the policy itself has three: the process must not start without an approval, an
approval for another command does not cover it, and an approval for that exact command does.

Consequence: P0-012 is signed off. The rule it rests on: a command runs because a person approved
that line, and no approval ever widens beyond the one operation it names.

## D-076: The editor names the runtime it runs on, and it is told why
Status: accepted
Context: P0-004 asks for "编辑器通过 ACP 连上 Agent" — an editor reaching an agent over ACP. Both
ends of that line were written: the endpoint in `src/acp/`, the editor's own client in
`editor/crates/agent-acp`. Nothing had ever connected them, and the connection was not merely
untested — three things in it were broken, and each of them made the "editor connection" a
fiction.

First: the endpoint named no runtime. `AcpBackend.startExecution()` started the task without a
runtime id, so every editor task landed on whatever the server happened to default to — the
reference runtime — while the product's primary runtime is Pi (D-042, D-060). The editor, the one
client that is supposed to reach the product's main runtime, could not reach it at all.

Second: tool calls arrived without their name. The Pi adapter emits `toolName`; the Hermes adapter
emits `tool`. The ACP layer read only `tool`, so every tool call reached the editor labelled
"tool". The call was real and its name was not — the first editor probe recorded exactly that
("tools=1 (tool)").

Third: a failed run said nothing. `execution.failed` carries the reason — a 429, an unreachable
model, a tool that died — and the ACP layer dropped it, so the editor showed an empty transcript
and a refusal. The person in front of the editor could not tell a run that died from a run that
never started.

Decision: three rules.

1. The endpoint names the runtime. `CODEWEAVER_ACP_RUNTIME_ID` names one runtime exactly and
   nothing else: the default is the product's primary runtime (`pi-local` when the server has Pi),
   otherwise `auto`, which asks the scheduler for a healthy one. The editor is a client of
   CodeWeaver, not a runtime of its own. A named runtime that is not registered is refused, not
   replaced (D-047): the alternative is a silent substitution, and a run that happened somewhere
   other than where it was asked to happen is not the run that was asked for.
2. A tool call reaches the editor under the name the runtime gave it. `toolTitle` reads `toolName`
   first and `tool` second, in one place — two implementations of "the tool's name" is how one of
   them ends up wrong.
3. A failure tells the editor why. `execution.failed` carries its reason across as a message
   chunk. A failure with no reason forwards nothing, because "the run failed" is the one sentence
   nobody can act on, and passing it on as if it were information would be worse than silence.

Evidence: the probe is not a mock and not a script — it is the editor's own ACP client
(`editor/crates/agent-acp`) driving the **real** endpoint (`dist/runners/acp-server.js`) against the
**real** runtime with a **real** model. It creates a throwaway git workspace whose single file
holds a nonce, names the runtime (`pi-local`), asks the model to read that file with `read_file`
and reply with its exact content, and judges the turn on four counts that can each fail alone: a
tool call happened, it was `read_file`, the turn ended with `EndTurn`, and the transcript carries
the nonce. The judgement has 15 unit tests on the probe side, mostly the cases that have to fail
(no tool call, the wrong tool, no nonce in the transcript, a turn that never finished, a refused
turn, a control arm that ran anyway, a refusal that named nothing, a quota refusal counted as an
environment refusal), and 7 on the lab side, of which three are about this decision (the tool name
arrives as the runtime gave it, a failure arrives with its reason, a failure without a reason
forwards nothing). The report's shape is tested too: a passing result carries no `notes` field at
all, because the Phase 0 aggregator validates the shape and a `notes: null` is not the same thing
as no note.

The run that signed the item off: the editor's client opened a session on the real endpoint, the
endpoint ran the task on the runtime that was named (`pi-local`), the model called `read_file` with
`{"path":"nonce.txt"}`, and the turn ended with `EndTurn` carrying the nonce back
(`NONCE-ACP-26472-1794462235621`, 11 events, session `sess_a42b411c-b7ab-49bf-b0c1-b2d9b8fde1c3`).
The journal says the same thing from the other side: `execution.runtime` names `pi-local`, and the
terminal entry is `execution.completed`.

The probe has a **control arm** that costs no tokens: the same editor's client, pointed at the same
real endpoint but naming a runtime that is not registered, must be refused by name ("Unknown
runtime no-such-runtime"). Without that arm an endpoint that quietly ran the task on some other
runtime would pass the main arm too — the control is what makes the main arm mean something. It
also proves the client's reach: the refusal came back over the wire, in the client's own words.

The probe also left a real negative case behind. The account's token plan ran out mid-session, the
model answered 429, and the same judgement reported the run as failed — "the agent never called a
tool … the turn ended with Refusal" — with the journal's own words next to it ("已达到 Token Plan
用量上限"). A judge that only ever sees successes is a judge nobody has watched fail. That run is
reported as `blocked`, not `failed`: a provider that refuses the call for lack of quota prevented
the check, it did not contradict it (D-020) — and the refusal reached the editor as text, which is
the third rule above working on the failure it was written for.

Consequence: P0-004 is signed off. The rules it rests on: the editor names the runtime and is
refused when the name is not registered; the editor is told what the tool was; and the editor is
told why a run failed. The client base is recorded too (`client/PIN.json`, Zed 1.29.0, read through
the UTF-8 BOM the provisioning script leaves) — the probe drives the editor's own ACP client, not a
Zed GUI session, and saying so is part of the evidence rather than a footnote to it.


## D-077: The sandbox limits are a kernel boundary, and the agent is told which one stopped it
Status: accepted
Context: P0-016 asks for a sandbox with memory, CPU and process limits. The checklist was written
against Docker, and Docker is gone (doc/HERMES_NATIVE_PLAN.md) — but the limits were never a
Docker feature. On Windows they are a job object, and the thing they limit is the acceptance
check: a real process on the user's machine, started by CodeWeaver, on behalf of an agent. Nothing
bounded it. A check could allocate the whole machine, start a thousand processes, or burn every
core, and the only thing standing between a task and the user's machine was the hope that the
command was sensible.

D-075 did part of this job and said plainly what it was not: the command-line classifier reads
the command line it is given, so a check that hides its work in a script file is beyond it. This
decision covers that gap, and it covers it where the limit is real — in the kernel.

Decision: four rules.

1. **The limits are enforced by the kernel, not by the product's judgement.** A Windows job object
   with `JOB_OBJECT_LIMIT_JOB_MEMORY`, `JOB_OBJECT_LIMIT_ACTIVE_PROCESS` and a CPU rate control.
   Node cannot create one, so the helper is a PowerShell script with the job object API compiled
   into it (`scripts/sandbox-run.ps1`); the arguments travel in an environment variable, because a
   command line would have to quote them twice and that is where arguments get lost. The Evaluation
   Engine's default runner is that helper, so every check runs inside the boundary whether anyone
   asked for it or not.
2. **A limit that cannot be established is not a limit.** If the helper is missing, the host is not
   Windows, or the numbers are unusable, the check does **not** run unsandboxed: the exit code
   stays unknown, the record says the sandbox was unavailable, and the check is blocked. The same
   rule the worktree scope follows (D-074) — a boundary that could not be verified is not a
   boundary with nothing behind it, and running anyway would be the one outcome nobody can detect
   from the outside.
3. **A limit that was hit is named.** "The check failed" and "the check was stopped by the memory
   limit" are different facts, and only one of them tells the agent what to change. The name goes
   in three places: the check's output (`[sandbox] the check exceeded the memory limit of 128 MB`),
   the record (`limitHit: "memory"`), and the text the model actually reads.
4. **The agent arm is part of the sign-off.** The checklist's security items need agent-level
   evidence on top of the boundary evidence (D-008, D-020): a limit that stops a process but says
   nothing is a limit the agent cannot act on, and an agent that was never asked proves nothing.

Evidence: the probe (`npm run validate:sandbox-limits`) has five arms, and the first four go
through the product's Evaluation Engine rather than through the helper, because the helper is the
OS boundary and the engine is what the product does with it.

- *control* — an ordinary check runs inside the job object and passes (`exitCode=0`,
  `sandbox.established=true`, `memoryMb=1024`, `processLimit=64`, `cpuRatePercent=25`), and the
  completion gate opens: "All 1 check(s) passed". A check that ran without limits looks the same
  from the exit code alone, so the record is what says the limits were in force.
- *memory* — the same check that allocates 512 MB passes under 1024 MB (`allocated 536870912`) and
  is stopped under 128 MB: `exitCode=1`, `limitHit=memory`,
  `RangeError: Array buffer allocation failed`, and the gate stays closed ("A check failed: failed:
  a check that allocates half a gigabyte").
- *processes* — the same check that starts twenty children at once passes without a limit
  (`started=20 failures=0`) and is stopped under a four-process limit: `exitCode=1`,
  `limitHit=processes`, `Error: spawn UNKNOWN` — errno -4094, which is
  `ERROR_NOT_ENOUGH_QUOTA`, the answer Windows gives a spawn beyond the job's process limit.
- *CPU* — a throttle does not fail a check, it slows it down, so the evidence is the check's own
  clock: 521 ms uncapped against 1969 ms under a 1% cap on a 22-processor machine (about a quarter
  of one core), the same work in both arms. The engine's own `durationMs` (2042 ms against 3472 ms)
  is *not* the measure — it includes the helper's startup in both arms, which drowns the
  difference. The cap is a share of all processors, not of one core, which is why it has to be
  well below 100/22 percent to bite a single-threaded check.

The fifth arm is the agent, and it found a defect. The task declares one check — `node
allocate.mjs 512` — under a memory limit; the model is asked to run it with `evaluate_changes`; the
product runs it inside the job object and has to tell the model what happened. The control arm is
the same check under 1024 MB: the model ran it (`evaluate_changesCalls=1`), the tool answered
"passed: 1/1 checks passed", the gate opened and the model said "All declared checks passed". The
tight arm is the same check under 128 MB: the model ran it (`evaluate_changesCalls=1`), the tool
answered

    failed: failed: a check that allocates half a gigabyte
    a check that allocates half a gigabyte: [sandbox] the check exceeded the memory limit of 128 MB

the gate stayed shut ("A check failed: failed: a check that allocates half a gigabyte"), and the
model's own answer named the limit — "exceeded the sandbox memory limit of 128 MB" — and said the
check never completed.

The defect the agent arm exposed: the limit reason was in the tool's `details`, and `details` is
not what a model reads. The tool's *text* said only "failed: failed: a check that allocates half a
gigabyte", so the agent was told that something failed and not what stopped it — the same defect
D-075 fixed for refusals, in a new place. `explainCheck` now pulls the `[sandbox]` line out of the
output and speaks it to the model, and a check that could not run at all explains itself the same
way. The control experiment is recorded: replacing that line with a generic "the check did not
pass" turns two unit tests red, so the gate that watches this behaviour has been watched failing.

The judgements are pure functions with unit tests, and most of those tests are the cases that have
to fail: an agent that never ran the check is `blocked` rather than `passed` (D-020), a limit that
was not named to the model, a gate that opened anyway, an agent that reported the task as done
although the check was stopped, a control arm that did not pass, a negated claim ("not all checks
passed") read as a claim of success, and a claim of success hidden by a negation in an earlier
sentence.

Consequence: P0-016 is signed off, and the promise is kept narrow on purpose. The limits cover the
processes **CodeWeaver starts** — the acceptance checks, which are the only processes the product
runs on an agent's behalf. Putting the whole runtime behind an OS boundary (AppContainer, then
Windows Sandbox or Hyper-V) is P0-017's job, and it is still blocked on that evidence
(doc/HERMES_NATIVE_PLAN.md). Saying "sandboxed" without that sentence would be the kind of claim
this log exists to prevent.


