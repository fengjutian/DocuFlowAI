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
