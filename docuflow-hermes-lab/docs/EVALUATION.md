# Evaluation Engine

Answers the question the agent cannot answer about itself: did the change actually work?

## Checks

A check is an independent process, run without a shell:

```ts
{ id: "unit", name: "npm test", command: "npm", args: ["run", "test:unit"], cwd, timeoutMs }
```

Verdicts:

| Situation | Verdict |
|---|---|
| every check exits with the expected code | `passed` |
| any check exits non-zero (or wrong expected code) | `failed` |
| any check times out or cannot spawn | `blocked` |
| no checks configured | `blocked` |

The last row matters: an empty check list is *absence of evidence*, and absence of evidence is not
success. That is why `evaluateChecks([], …)` never returns `passed`.

## Completion gate (AT-204)

`completionGate(result)`:

| Input | Allowed | Reason |
|---|---|---|
| no result | no | "No evaluation result exists for this change" |
| `blocked` | no | "A check could not run: …" |
| `failed` | no | "A check failed: …" |
| `passed` | yes | "All N check(s) passed" |

`GET /api/v1/executions/{id}` returns this gate as `completion`, so a UI that shows "done" has to
contradict the server explicitly rather than assume success from a terminal event.

## Persistence and trace

Results are immutable records (`evaluations/`), keyed by execution, and `TraceabilityQuery.byExecution`
returns them with the context, policy, prompt and change records. Each run is also journaled as
`evaluation.completed`, so it appears on the execution timeline and in checkpoints.

## Deliberate limits

- Checks are configured by the server/operator, never accepted from an API caller: exposing arbitrary
  command execution over HTTP would be a remote shell.
- No flaky-test retry and no baseline comparison yet; a rerun is a new evaluation record.
- Cost accounting (AT-206) is not implemented.
