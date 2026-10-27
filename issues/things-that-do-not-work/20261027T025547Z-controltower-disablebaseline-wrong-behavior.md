Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4e0be56b24c0fc13c424c1a1b18bb5f0120957ed

# DisableBaseline's teardown is the one pending transition read back off the evictable operation log, so an evicted DISABLE record resurrects the enablement

_Filed: 2026-10-27T02:55:47Z_
_Operation: DisableBaseline | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4e0be56b24c0fc13c424c1a1b18bb5f0120957ed`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other asynchronous transition in this crate is parked ON THE RESOURCE with its own `due_at`, precisely so that applying it does not depend on the operation record still being in the bounded log: `PendingLandingZoneDeletion` (DeleteLandingZone), `PendingEnabledControlUpdate` (UpdateEnabledControl/ResetEnabledControl) and `PendingEnabledBaselineUpdate` (UpdateEnabledBaseline) all carry that field and all three document the same reason.

The teardown `DisableBaseline` starts is the exception: `EnabledBaseline` has no pending-teardown field, and `settle` discovers the teardown by SCANNING the baseline operation log for a `DISABLE_BASELINE` record that has reached `Succeeded` (`src/state.rs:1222-1230`). The same log is bounded and self-evicting (`enforce_log_bound`, `src/state.rs:1923-1937`): when every record in it is still in flight, it evicts the oldest in-flight record. If the in-flight `DISABLE_BASELINE` record for an enablement is the record evicted, nothing in the store remembers that a teardown is owed:

* the enablement is never removed from `enabled_baselines` and its tags are never dropped (`src/state.rs:1231-1236` never sees it);
* the status loop then re-derives the enablement's status from the log and, finding no operation in flight, reports it as `SUCCEEDED` again (`src/state.rs:1275-1281`).

So a baseline the caller disabled comes back as a live, enabled, SUCCEEDED resource that `GetEnabledBaseline`/`ListEnabledBaselines` keep reporting, its tags included, and the pair stays blocked for `EnableBaseline`'s one-baseline-per-target rule under an ARN the caller already tore down. The fix is the one this crate already applies three times: park the teardown on the `EnabledBaseline` with its own `due_at` and have `settle` apply it from there, rather than rediscovering it in a log that may have dropped the record. This is not a request to change `MAX_BASELINE_OPERATIONS` — it is the crate's own stated rule being applied in three of four places.

## Evidence

Teardown derived from the evictable log — services/service-com-amazonaws-controltower-awscontroltowerapis/src/state.rs:1219-1232:
```rust
// A DISABLE_BASELINE that has reached its terminal status has finished tearing
// its `EnabledBaseline` down, so the resource is gone whether or not anybody
// called; its operation record stays, and still resolves.
let torn_down: Vec<String> = self
    .baseline_operations
    .values()
    .filter(|operation| {
        operation.operation_type == BaselineOperationType::DisableBaseline
            && operation.status == BaselineOperationStatus::Succeeded
    })
    .map(|operation| operation.enabled_baseline_arn.clone())
    .collect();
for arn in torn_down {
    self.enabled_baselines.remove(&arn);
```

The rule the crate states for every OTHER pending transition — state.rs:629-632 (`PendingLandingZoneDeletion`):
```rust
/// The instant the clock owes this removal — the same instant that operation becomes
/// due, held here so applying it never depends on the operation's record still being
/// in the (bounded) log. Stored, never slept on.
pub due_at: SystemTime,
```
verbatim again at state.rs:744-747 (`PendingEnabledControlUpdate`) and state.rs:943-946 (`PendingEnabledBaselineUpdate`): "held here so applying it never depends on the operation's record still being in the (bounded) log". `settle` makes the same point for the landing zone at state.rs:1094-1097: "`enforce_log_bound` evicts the oldest in-flight record when every record is in flight, and a missing record must not be read as \"due\"".

The eviction that can take the DISABLE record — state.rs:1917-1933:
```rust
/// Evicts the oldest TERMINAL record first, so an operation a caller is still polling is
/// never dropped out from under it; only when every record is in flight does the oldest
/// overall go.
fn enforce_log_bound<R: OperationLogRecord>(log: &mut HashMap<String, R>, max_records: usize) {
    while log.len() > max_records {
```

The resurrection — state.rs:1272-1281:
```rust
if enabled_baseline.status == EnablementStatus::Failed {
    continue;
}
enabled_baseline.status = if baseline_operation_in_flight(baseline_operations, arn)
    .is_some()
{
    EnablementStatus::UnderChange
} else {
    EnablementStatus::Succeeded
};
```

Handler side — src/ops/disable_baseline.rs:221-231 records the operation and marks the resource `UNDER_CHANGE`, but parks nothing on the resource:
```rust
let operation_identifier = store.record_baseline_operation(
    crate::state::BaselineOperationType::DisableBaseline,
    &arn,
    now,
);
// The resource stays until the teardown is due — `settle` removes it then — so a
// caller polling straight after this call still sees it UNDER_CHANGE.
if let Some(enabled_baseline) = store.enabled_baselines.get_mut(&arn) {
    enabled_baseline.status = crate::state::EnablementStatus::UnderChange;
```

Test coverage: `tests/all/disable_baseline.rs` (`every_enablement_tears_down_through_its_own_idempotent_operation`, `a_teardown_removes_only_its_own_enablement`) exercises the teardown only on a small log, so no test covers the log at its retention bound.

## Steps to Reproduce

1. Enable a baseline on an OU and note the returned EnabledBaseline ARN (EnableBaseline)
2. Call DisableBaseline on that ARN and note the returned operationIdentifier
3. Within the same settle window (before the disable becomes due), drive more than MAX_BASELINE_OPERATIONS (1024) further baseline operations (EnableBaseline/UpdateEnabledBaseline on other enablements) so that every record in `baseline_operations` is still IN_PROGRESS and `enforce_log_bound` evicts the oldest in-flight record — the DISABLE_BASELINE record of step 2
4. Wait past OPERATION_SETTLE_AFTER and call GetEnabledBaseline (or ListEnabledBaselines) with the ARN from step 1
5. Observe the enablement is still reported, with statusSummary.status = SUCCEEDED, instead of having been torn down; ListTagsForResource on the same ARN still reports its tags, and EnableBaseline for the same baseline/target pair is still refused with ConflictException naming it
6. Compare with the same sequence against a landing zone (DeleteLandingZone), where the removal is parked on the resource and is applied regardless of the DELETE record's eviction
