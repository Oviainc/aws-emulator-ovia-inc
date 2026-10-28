Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 87b02e9e0446b34956525b9761a3171af51589ac

# An out-of-range startTime/endTime is silently coerced to the opposite extreme instead of being refused as the sibling window readers refuse it

_Filed: 2026-10-28T13:43:25Z_
_Operation: GetRelationalDatabaseLogEvents | Class: validation_

> _Note: this issue was identified against the code base at commit `87b02e9e0446b34956525b9761a3171af51589ac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetRelationalDatabaseLogEvents` converts both window bounds to epoch milliseconds with `unwrap_or(i64::MIN)` for `startTime` and `unwrap_or(i64::MAX)` for `endTime`. A modelled `timestamp` arrives on this protocol as (possibly fractional) epoch SECONDS, so a client can send an instant whose millisecond conversion overflows (e.g. 1e18). When that happens the handler does not refuse: it replaces the bound with the extreme at the OPPOSITE end of the caller's intent, so a request whose window starts far in the future is served as if the window had no lower bound at all and the answer carries every line of the stream. The failure is turned into a success with wrong contents.

The crate's two other handlers that make the same conversion refuse it instead, with a message naming the member — `GetBucketMetricData` and `GetCostEstimate` both route it through a local `epoch_millis` that maps the conversion error onto the modelled `InvalidInputException`. (The ordering check `inverted_window_refusal` cannot catch this: it compares the instants exactly and, for a request that carries only `startTime`, is not reached at all.)

## Evidence

The coercion — services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/get_relational_database_log_events.rs:208-213:
```
        // Epoch milliseconds, the unit the stored log holds its instants in:
        // "Specified in the Unix time format."
        let from_ms = input
            .start_time
            .map(|start| start.to_millis().unwrap_or(i64::MIN));
        let to_ms = input.end_time.map(|end| end.to_millis().unwrap_or(i64::MAX));
```
and what those values then do — src/state.rs:7957-7958 (`log_event_page`):
```
            .filter(|line| from_ms.is_none_or(|from_ms| line.created_at_ms >= from_ms))
            .filter(|line| to_ms.is_none_or(|to_ms| line.created_at_ms < to_ms))
```
The sibling rule for the same conversion — src/ops/get_bucket_metric_data.rs:443-452:
```
fn epoch_millis(
    at: ::aws_smithy_types::DateTime,
    member: &str,
) -> Result<i64, GetBucketMetricDataError> {
    at.to_millis().map_err(|_| {
        invalid(format!(
            "{member} must name an instant within the range of epoch milliseconds"
        ))
    })
}
```
and the same helper again in src/ops/get_cost_estimate.rs:735.

Test coverage: tests/all/inverted_time_window.rs drives this operation only over ORDERING cases (bounds between -200 and 1.7e9, lines 185-228), and tests/all/relational_database_log_events.rs pins the in-range windows — no case sends a bound outside the epoch-millisecond range, so nothing pins the coercion.

## Steps to Reproduce

1. CreateRelationalDatabase "db1" and let it settle to available, so its error log holds the creation/availability lines.
2. Call GetRelationalDatabaseLogEvents {relationalDatabaseName: "db1", logStreamName: "error", startTime: 1e18} (no endTime).
3. Observe HTTP 200 with the stream's lines reported: `start_time.to_millis()` overflowed, so `from_ms` became i64::MIN and the lower bound the caller asked for was dropped (src/ops/get_relational_database_log_events.rs:210-212).
4. Call GetBucketMetricData with the same out-of-range startTime -> 400 InvalidInputException "startTime must name an instant within the range of epoch milliseconds" (src/ops/get_bucket_metric_data.rs:443-452): the two handlers answer the same input differently.
