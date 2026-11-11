# Usage and cost accounting (AT-206)

`UsageLedger` records, per execution, the model, prompt/completion/total tokens and wall-clock
duration.

## Extraction

Runtime payloads are inconsistent, so `recordFromEvent()` accepts:

- nested `usage` / `token_usage`, or flat fields on the event;
- `prompt_tokens` / `input_tokens` / `tokens_in`;
- `completion_tokens` / `output_tokens` / `tokens_out`;
- `total_tokens` / `tokens_total`, otherwise the sum of prompt and completion.

If no total can be determined, **nothing is recorded**. A zero-token record would look like a free run
and silently understate cost.

`durationMs` comes from the caller: the server tracks `execution.started` timestamps and computes the
difference at the terminal event. The runtime event carries counters, not a trustworthy duration.

## Aggregation

`summary()` returns totals plus `byModel` (executions and tokens per model), which is what a
dashboard or a per-model budget check needs.

## API

| Method | Path | Result |
|---|---|---|
| GET | `/api/v1/executions/{id}/usage` | this execution's record, 404 if none |
| GET | `/api/v1/usage` | overall summary and all records |

Both return **501** when usage accounting is not wired into the server, rather than an empty success.

## Limits

- One usage row per execution, persisted under `<data>/usage/` and reloaded by `UsageLedger.load()`;
  a second terminal event for the same execution updates memory without rewriting the record.
- No price table: tokens and time are recorded, currency is not.
- Streaming runs that report usage per chunk are not summed; only the terminal event is used.
