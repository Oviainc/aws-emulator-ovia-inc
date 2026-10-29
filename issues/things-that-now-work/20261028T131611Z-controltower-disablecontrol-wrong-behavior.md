Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 930ad079f132337074cf049827d554fb10a3d200

# DisableControl leaves the torn-down control reporting UNDER_CHANGE with a stale IN_SYNC drift assessment

_Filed: 2026-10-28T13:16:11Z_
_Operation: DisableControl | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `930ad079f132337074cf049827d554fb10a3d200`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

While a `DISABLE_CONTROL` teardown is in flight the enabled control stays in the store and is still reported by `GetEnabledControl`/`ListEnabledControls` as `UNDER_CHANGE` — but its three drift dimensions keep the values they had before the call (normally `IN_SYNC`), so one response body says "this control is being changed" and "the control matches the configuration Control Tower expected" at the same time.

This crate states the opposite rule in its own words and implements it on every sibling mutating control path. `settle` says drift is unassessable while the control is `UNDER_CHANGE`, and `start_control_change` — the one span `UpdateEnabledControl` and `ResetEnabledControl` both reach the store through — drives all three dimensions to `DriftStatus::Unknown` when it marks the control `UNDER_CHANGE`. `record_enabled_control_disable`, which `DisableControl` calls, writes `status`, `last_operation_identifier`, `due_at` and `pending_deletion` but touches no drift field, so the previously assessed `IN_SYNC` survives the whole teardown window. `settle` cannot repair it either: its drift fix-up only runs on the `UnderChange -> Succeeded` transition and only promotes `Unknown` to `InSync`, never the reverse.

The result is that the same transient `UNDER_CHANGE` state is reported two different ways depending on which call produced it — a reset/update in flight reports `UNKNOWN`, a disable in flight reports `IN_SYNC`. The fix is to drop the drift assessment where the teardown is parked (`record_enabled_control_disable`), the way the shared change span already does; no existing test asserts the drift value during a teardown, and the control is removed when the teardown becomes due, so no settled response changes.

## Evidence

The rule, in the crate's own words — `src/state.rs:1283-1286`, inside `settle`:
```
                // Drift is only assessable once the configuration is deployed: while the
                // control is `UNDER_CHANGE` the service "is not able to check the drift
                // status` (`UNKNOWN`), and the deployment that completes here is of the
                // configuration the service expects, so nothing has diverged from it yet.
```
(the quoted phrase is the model's own: `DriftStatusSummary$driftStatus` documents "`UNKNOWN`: Amazon Web Services Control Tower is not able to check the drift status for the enabled control.")

The same rule as a test-level RULE statement — `tests/all/enabled_control_lifecycle.rs:412-414`:
```
/// RULE: an enablement is reported non-terminal (`UNDER_CHANGE`, drift `UNKNOWN`) until
/// the clock makes it due and `SUCCEEDED`/`IN_SYNC` after that — and a read never moves
/// it, so two reads of the same instant agree.
```
and `tests/all/enabled_control_lifecycle.rs:16`: "//! * an enablement is `UNDER_CHANGE` with `UNKNOWN` drift until the clock makes it due,".

The sibling path applies it — `src/state.rs:1777-1791` (`start_control_change`, which `record_enabled_control_update` at `src/state.rs:1656-1668` and `record_enabled_control_reset` at `src/state.rs:1690-1695` both delegate to):
```
        let control = self.enabled_controls.get_mut(arn)?;
        control.status = EnablementStatus::UnderChange;
        control.last_operation_identifier = operation_identifier.clone();
        control.due_at = now + OPERATION_SETTLE_AFTER;
        control.pending_update = Some(PendingEnabledControlUpdate { ... });
        control.drift_status = DriftStatus::Unknown;
        control.inheritance_drift_status = DriftStatus::Unknown;
        control.resource_drift_status = DriftStatus::Unknown;
```

The teardown path skips it — `src/state.rs:1725-1737` (`record_enabled_control_disable`):
```
        let control = self.enabled_controls.get_mut(arn)?;
        // ONE instant, held in both places that need it: the status the control reports
        // while the teardown runs is owed at the same moment as the removal itself, so the
        // two cannot drift apart.
        let due_at = now + OPERATION_SETTLE_AFTER;
        control.status = EnablementStatus::UnderChange;
        control.last_operation_identifier = operation_identifier.clone();
        control.due_at = due_at;
        control.pending_deletion = Some(PendingEnabledControlDeletion {
            operation_identifier: operation_identifier.clone(),
            due_at,
        });
        Some(operation_identifier)
```
No drift field is written.

`settle` cannot repair it — `src/state.rs:1280-1296` only runs the drift fix-up inside `if control.status == EnablementStatus::UnderChange && now >= control.due_at`, and only in the `Unknown -> InSync` direction; a control whose `pending_deletion` is due is removed earlier in the same pass (`src/state.rs:1231-1252`), so it never reaches that branch.

Handler entry point — `src/ops/disable_control.rs:363`:
```
        let Some(operation_identifier) = store.record_enabled_control_disable(&arn, now) else {
```
under the handler's own comment at `src/ops/disable_control.rs:359-362`: "the enablement stays in the store, `UNDER_CHANGE`, until the clock makes that record due and `settle` removes it."

Test coverage: `tests/all/disable_control.rs:427-435` reads the enablement back inside the teardown window and asserts only `(status(&reported), last_operation(&reported))` == `(Some("UNDER_CHANGE"), Some(identifier))` — the drift value in that window is pinned by no test, while `tests/all/enabled_control_reset.rs:341-345` and `tests/all/in_flight_configuration_change_drift.rs:556-566` pin `(Some("UNDER_CHANGE"), (Some("UNKNOWN"), Some("UNKNOWN"), Some("UNKNOWN")))` for the sibling reset/update windows, proving the two paths disagree about the same transient state.

## Steps to Reproduce

1. EnableControl a catalogued enablable control (e.g. `AWS-GR_ENCRYPTED_VOLUMES`) on an organizational-unit target, then wait past `crate::state::OPERATION_SETTLE_AFTER`.
2. GetEnabledControl: `statusSummary.status` = `SUCCEEDED`, `driftStatusSummary.driftStatus` = `IN_SYNC`, `types.inheritance.status` = `IN_SYNC`, `types.resource.status` = `IN_SYNC`.
3. Call DisableControl with that `enabledControlIdentifier` and immediately call GetEnabledControl again (inside the settle span).
4. Observed: `statusSummary.status` = `UNDER_CHANGE` while all three drift dimensions still read `IN_SYNC`.
5. Repeat the same window with ResetEnabledControl (or UpdateEnabledControl) instead of DisableControl: all three dimensions read `UNKNOWN`, as `tests/all/enabled_control_reset.rs:341-345` pins — the two in-flight windows report the same `UNDER_CHANGE` state differently.
