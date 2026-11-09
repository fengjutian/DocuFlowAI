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
