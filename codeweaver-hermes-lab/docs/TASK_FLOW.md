# Task flow: startTask → change → trace

`TaskExecutionService` (`src/tasks/`) is the single entry point that enforces the order:

```text
capture context snapshot  → persist (immutable)
reserve policy + prompt ids
start the execution       → coordinator (adapter + journal)
persist policy snapshot   → now carries execution id
persist prompt snapshot   → now carries execution id
journal: context.snapshot / policy.snapshot / prompt.snapshot
```

## Why the two snapshots are written after `start`

Design 11.6 and 11.8 give both tables an `execution_id`, and an execution id only exists once the
runtime has created the run. The ids are reserved before `start` so the task input can reference
them, and the records are written immediately after — still before any event handling matters.

## Applying a change

```ts
await service.applyProposedChange(executionId, patch, { commitMessage: "feat: ..." })
```

Runs the Code Engine pipeline (path scope → base SHA → risk → policy → approval → check → apply),
then records the result in three places:

1. the immutable change store;
2. the journal as `change.applied`, which makes `buildCheckpoint()` report it in
   `appliedChangeRecordIds` so a rebuild never re-applies it;
3. the trace (commit → execution → context/policy/prompt), via `trace()` / `traceCommit()`.

`awaiting_approval` and `denied` outcomes write nothing to the change store.

## What is still missing

- No caller yet runs the agent loop between `startTask` and `applyProposedChange`; the patch comes
  from the caller (in production, from the agent's `request_diff` tool call).
- Prompt snapshots store hashes; the encrypted prompt body belongs to the API layer's storage.
- No idempotency key on `startTask`: calling it twice creates two runs, which is correct for a retry
  but must be guarded by the caller.
