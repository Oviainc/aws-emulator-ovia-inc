Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: a27e8c942e6626601cfec60cefd89de119a9570b

# clientToken is recorded only after the EventBridge await, so two concurrent executions under one token each trigger and each publish a notification

_Filed: 2026-10-28T17:16:38Z_
_Operation: ExecuteScheduledReport | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `a27e8c942e6626601cfec60cefd89de119a9570b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ExecuteScheduledReport` reads the execution-idempotency record under one lock, DROPS that lock to await the EventBridge delivery, and only writes the record under a second lock taken after the delivery. Two concurrent calls presenting the same `clientToken` for the same report therefore both find no prior record, both compute `executionTriggered = true`, both call `deliver_execution_notification`, and both write `lastExecutionAt`/`healthStatus` — so one idempotency token yields two executions and TWO notifications on the account's default event bus, instead of the one the handler documents.

The lock must indeed be dropped across the await, so the fix is to RESERVE the token under the first lock (the way the sibling create does) rather than to hold the guard: `CreateScheduledReport` solves exactly this racing-duplicate case and says so explicitly, while this handler's own post-await comment only accounts for a concurrent DELETE of the report and not for a concurrent duplicate token.

## Evidence

The documented guarantee — `src/ops/execute_scheduled_report.rs:39-46`:
```
//! * A `clientToken` makes the call idempotent for
//!   `crate::state::EXECUTION_IDEMPOTENCY_WINDOW` (the minute the model documents): a
//!   retry presenting the same token for the same report answers the first call's
//!   outcome and triggers nothing.
```

The read happens under the first guard, which is then dropped — `src/ops/execute_scheduled_report.rs:229-304`:
```
// Everything this request can decide from the store alone is decided here, under
// one guard that is DROPPED before the delivery below: the delivery is an await,
// and a lock is not held across one.
let (record, configured_health) = {
    let mut store = state.lock();
    ...
    store.expire_execution_tokens(now);
    if let Some(prior) = idempotency_key
        .as_ref()
        .and_then(|key| store.scheduled_report_executions.get(key))
    { ... return Ok(...); }
```

The delivery, and then the write under a NEW guard — `src/ops/execute_scheduled_report.rs:323-355`:
```
let health_status = if execution_triggered
    && !crate::state::deliver_execution_notification(ctx, &record, now).await
...
// The writes, under a guard taken AFTER the delivery: the store was unlocked
// while the bus was reached, so the report is looked up again rather than
// assumed to still be there — a `DeleteScheduledReport` in between removes it,
// and this execution must not write it back.
let mut store = state.lock();
...
if let Some(key) = idempotency_key {
    store.scheduled_report_executions.insert(key, ...);
}
```
Nothing between the read at line 291 and the insert at line 347 marks the token as taken, so a second concurrent request passes the same check.

The sibling handler that DOES close this race — `src/ops/create_scheduled_report.rs:253-263`:
```
// * the report the token started IS BEING CREATED — a duplicate racing the first
//   call, inside `crate::state::SCHEDULED_REPORT_CREATION_WINDOW` of it. Several
//   callers sharing one token therefore produce exactly one report and one
//   success, and the losers are told a creation is in progress;
...
// Only a call that got as far as creating records its token: a refused request
// created nothing, so its token stays free. The lookup and the insert below both
// happen under one lock.
```

The notification this duplicates is the service's cross-service event — `src/state.rs:1895-1931` (`deliver_execution_notification`, one `PutEvents` entry on `DEFAULT_EVENT_BUS`).

Test coverage: `tests/all/scheduled_report_execution_events.rs:300-396` (`nothing_is_delivered_without_a_triggered_execution`) asserts that "a retry under the same client token" leaves exactly 1 notification on the bus — but it issues the two calls SEQUENTIALLY (`let first = ok_call(..).await; let retried = ok_call(..).await;`), so no test exercises two calls in flight at once.

## Steps to Reproduce

1. Create a dashboard and a scheduled report whose resources resolve, with an EventBridge emulator mounted so notifications are accepted.
2. Issue two ExecuteScheduledReport requests concurrently (e.g. join two futures) with the identical body `{"arn": <report arn>, "clientToken": "same-token-0001"}`.
3. Both answers come back with `executionTriggered: true` instead of the second replaying the first call's outcome.
4. Inspect the default event bus: TWO `BCM Dashboards Scheduled Report Execution` notifications were published for the single idempotent request, contradicting the handler's documented "a retry presenting the same token for the same report answers the first call's outcome and triggers nothing".
5. Contrast with CreateScheduledReport, where two concurrent calls sharing one clientToken produce exactly one report and one success because its lookup and insert happen under one lock.
