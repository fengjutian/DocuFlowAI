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