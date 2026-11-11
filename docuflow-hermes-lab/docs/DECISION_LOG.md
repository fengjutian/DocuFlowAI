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
primitives map one-to-one onto DocuFlow's adapter contract. Measured on 2026-11-09 against the
installed package: a native lifecycle event stream (`agent_start`, `turn_start`, `message_start`,
`message_update`, `message_end`, `turn_end`, `agent_end`), tool interception that blocks execution
before the tool runs, `AbortSignal` cancellation, and a transcript that survives across prompts.
Hermes' HTTP surface offers none of these natively, which is why D-005 had to declare cancellation
and resume as unavailable capabilities.
Decision: Pi is evaluated as the primary runtime candidate while Hermes remains the remote-runtime
reference. The adapter contract is the seam, so DocuFlow's own layers — policy, code engine,
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

## D-048: DocuFlow's own tools are their own enforcement point
Status: accepted
Context: the Pi adapter gates every tool call with the DocuFlow permission handler, because the
handler is where the policy decision belongs (D-003). That is correct for a tool the runtime
brought with it, and wrong for DocuFlow's own tools: `read_file` knows the resolved path and
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
tool is careful; what has to hold is DocuFlow's gate. The probes check the disk after each run as
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
the product's own claim the only evidence: the event said "cancelled" because DocuFlow asked, not
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
