Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0a6367a2f72726ece298ef6fb266120e608ee126

# The control teardown is the one parked transition left on the resource and re-decided from the evictable operation log, so an evicted FAILED record silently removes the enablement

_Filed: 2026-10-29T11:08:50Z_
_Operation: DisableControl | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0a6367a2f72726ece298ef6fb266120e608ee126`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every asynchronous transition in this crate is parked on the RESOURCE and TAKEN the moment the clock makes it due, so the decision is made exactly once and never depends on the bounded, self-evicting operation log. `DisableControl`'s teardown (parked by `record_enabled_control_disable`) is the single exception:

* `settle`'s control-teardown pass never clears `control.pending_deletion` — it only reads it — so after the parked removal becomes due the teardown stays parked on the control for the rest of the process's life;
* the only thing that stops that still-parked, already-due teardown from removing the control on the NEXT settle is a lookup of its operation record in `self.control_operations`, a store bounded at `MAX_CONTROL_OPERATIONS = 1024` whose retention rule evicts "the oldest TERMINAL record first". A FAILED record is terminal, so it is a prime eviction candidate; once it is gone, `operations.get(&pending.operation_identifier)` answers `None`, `is_some_and(..Failed)` is `false`, and the negation makes the control — and its tags — be removed by a teardown the crate has already decided removed nothing.

Two observable defects follow from the same un-taken pending transition:

1. STATE LOSS: an `EnabledControl` that a failed `DISABLE_CONTROL` left in place (plus its entry in `tags`) is destroyed later, by log retention alone, with no call having asked for it. The crate's own comment immediately above that code forbids exactly this reading.
2. A control whose teardown failed keeps `pending_deletion = Some(..)` forever, and `withdraw_governance_controls` skips every control with `pending_deletion.is_some()` — so ending that target's governance never tears that control down, leaving a service-applied mandatory control on an ungoverned target permanently.

The fix is the one the five sibling transitions already use: take the parked teardown when it becomes due and decide the failure case once, as `record_enabled_baseline_disable`'s counterpart in `settle` does (a FIXED issue, `20261027T025547Z-controltower-disablebaseline-wrong-behavior`, moved the baseline teardown off the log for this very reason).

## Evidence

The control-teardown pass states the rule and then breaks it — `src/state.rs:1326-1354`:
```
        // its operation record stays, and still resolves. The due instant is the pending
        // teardown's OWN, not read back off the operation log, for the same reason a
        // landing zone's decommissioning carries its own: `enforce_log_bound` evicts the
        // oldest in-flight record when every record is in flight, and a missing record
        // must not be read as "due" and remove a resource the clock does not owe yet. A
        // teardown whose own record FAILED removed nothing, so the enablement stays —
        // the same reading the pending update below gives a failed operation.
        let operations = &self.control_operations;
        let torn_down: Vec<String> = self
            .enabled_controls
            .iter()
            .filter(|(_, control)| {
                control.pending_deletion.as_ref().is_some_and(|pending| {
                    now >= pending.due_at
                        && !operations
                            .get(&pending.operation_identifier)
                            .is_some_and(|operation| {
                                operation.status == ControlOperationStatus::Failed
                            })
                })
            })
            .map(|(arn, _)| arn.clone())
            .collect();
        for arn in torn_down {
            self.enabled_controls.remove(&arn);
            // A tag entry never outlives the resource it tags, which is what keeps the tag
            // map bounded by the resource stores.
            self.tags.remove(&arn);
        }
```
`pending_deletion` is read through `as_ref()` and never taken, so the same already-due teardown is re-decided on every later settle, against the log.

The FIVE sibling transitions all TAKE the parked transition when it becomes due, so none is re-decided:
* control update — `src/state.rs:1365-1370`: `let due = control.pending_update.take_if(|pending| now >= pending.due_at); let Some(pending) = due else { continue; };`
* baseline teardown — `src/state.rs:1433-1439`: `let due = enabled_baseline.pending_deletion.take_if(|pending| now >= pending.due_at); let Some(pending) = due else { // Not due yet leaves the parked teardown where it is, unchanged. continue; };`
* landing zone decommissioning — `src/state.rs:1246-1252`: `let due = landing_zone.pending_deletion.take_if(|pending| now >= pending.due_at);`
* landing zone configuration — `src/state.rs:1278-1284`: `let due = landing_zone.pending_configuration.take_if(|pending| now >= pending.due_at);`
* baseline update — `src/state.rs:1463-1470`: `let Some(pending) = enabled_baseline.pending_update.take() else { continue; }; if now < pending.due_at { enabled_baseline.pending_update = Some(pending); continue; }`

The log that decides the failure case is bounded and evicts terminal records first — `src/state.rs:149` `pub const MAX_CONTROL_OPERATIONS: usize = 1024;` and `src/state.rs:2563-2582`:
```
/// Drop records until `log` holds at most `max_records`.
///
/// Evicts the oldest TERMINAL record first, so an operation a caller is still polling is
/// never dropped out from under it; only when every record is in flight does the oldest
/// overall go.
```

The second symptom — `src/state.rs:2239-2248`:
```
        let mut applied: Vec<String> = self
            .enabled_controls
            .values()
            .filter(|enabled| {
                enabled.target_identifier == target_identifier
                    && enabled.pending_deletion.is_none()
                    && is_mandatory_control(&enabled.control_identifier)
            })
```
so a control left carrying a stale `pending_deletion` is never withdrawn with its target's governance.

The parked teardown is written by the handler under review — `src/ops/disable_control.rs:389` `let Some(operation_identifier) = store.record_enabled_control_disable(&arn, now) else {` → `src/state.rs:1893-1896`:
```
        control.pending_deletion = Some(PendingEnabledControlDeletion {
            operation_identifier: operation_identifier.clone(),
            due_at,
        });
```

Test coverage: the two sibling resources have a dedicated suite for exactly this hazard — `tests/all/landing_zone_pending_transitions.rs:170` (`fn evict_from_the_log`), `:210-219` (`fn fail_the_operation`, which seeds `LandingZoneOperationStatus::Failed` because "Nothing in this crate fails a landing-zone operation yet") and `tests/all/enabled_baseline_pending_transitions.rs:320` (`a_teardown_whose_record_was_evicted_still_removes_the_enablement_and_its_tags`). There is NO equivalent suite for enabled controls, and `grep` finds no test in `tests/all/` that seeds `ControlOperationStatus::Failed` or inspects `pending_deletion`, so nothing pins the current reading.

## Steps to Reproduce

1. POST /enable-baseline for the governance baseline on an OU (or POST /enable-control for an elective control) and wait past OPERATION_SETTLE_AFTER (500 ms) so the enablement is SUCCEEDED.
2. POST /disable-control with that enablement's `enabledControlIdentifier`; note the returned `operationIdentifier`.
3. Seed that control-operation record as FAILED in the store, the way `tests/all/landing_zone_pending_transitions.rs:210` does for a landing zone (`record.status = ControlOperationStatus::Failed; record.end_time = Some(record.due_at);`) — nothing in this crate fails a control operation yet.
4. Wait past the settle span and POST /get-enabled-control: the control is still there, as the comment at src/state.rs:1330-1332 promises ("A teardown whose own record FAILED removed nothing, so the enablement stays").
5. Now push that FAILED record out of the bounded control-operation log, the way `evict_from_the_log` does for the sibling logs: append control operations until `control_operations` passes MAX_CONTROL_OPERATIONS = 1024 — the FAILED record is terminal, so `enforce_log_bound` evicts it first.
6. POST /get-enabled-control again: the control is now gone (404 ResourceNotFoundException) and GET /tags/{arn} answers 404 too — a resource and its tags destroyed by log retention, by a teardown the service had already decided removed nothing.
7. Variant for the second symptom: instead of evicting, POST /disable-baseline for the governance baseline of that target. The mandatory control whose teardown failed is skipped by `withdraw_governance_controls` (its `pending_deletion` is still Some), so /list-enabled-controls keeps reporting it on a target nothing governs any more.
