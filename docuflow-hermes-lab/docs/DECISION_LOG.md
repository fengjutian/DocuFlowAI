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
