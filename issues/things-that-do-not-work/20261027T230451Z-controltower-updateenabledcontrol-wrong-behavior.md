Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c56dd21dd1ebda0a2d29e841f8875a573d0c3ba4

# UpdateEnabledControl leaves an UNDER_CHANGE control reporting IN_SYNC drift, unlike its sibling reset

_Filed: 2026-10-27T23:04:51Z_
_Operation: UpdateEnabledControl | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c56dd21dd1ebda0a2d29e841f8875a573d0c3ba4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

While an `UPDATE_ENABLED_CONTROL` is in flight the control is reported `UNDER_CHANGE` but its three drift dimensions keep the values they had before the call (normally `IN_SYNC`), so `GetEnabledControl`/`ListEnabledControls` answer "the configuration is changing" and "the control matches the configuration we deployed" in the same body.

This crate states the opposite rule in three places and implements it on the sibling path: `ResetEnabledControl` puts all three drift dimensions back to `UNKNOWN` when it starts its change, because "a control cannot be assessed against a configuration still being deployed". `UpdateEnabledControl` reaches the state through `start_control_change`, which sets `status = UnderChange` and parks the new parameters but never touches `drift_status`, `inheritance_drift_status` or `resource_drift_status`. Result: the same transient state is reported two different ways depending on which call produced it.

The fix is to drive the drift dimensions to `UNKNOWN` in the shared `start_control_change` span (the one place both calls go through) rather than only in `record_enabled_control_reset`; `settle` already moves `Unknown` back to `InSync` when the operation becomes due, so the settled answer — the only thing existing tests assert for an update — does not change.

## Evidence

The rule, in the crate's own words:

* `src/state.rs:1283-1286` (inside `settle`): "// Drift is only assessable once the configuration is deployed: while the control is `UNDER_CHANGE` the service \"is not able to check the drift status\" (`UNKNOWN`), and the deployment that completes here is of the configuration the service expects, so nothing has diverged from it yet."
* `src/state.rs:1677-1680` (doc of `record_enabled_control_reset`): "What the re-deployment settles is drift: a control cannot be assessed against a configuration still being deployed, so the three drift dimensions go back to `UNKNOWN` here and [`settle`] makes them `IN_SYNC` once the clock has made the operation due."
* `tests/all/enabled_control_lifecycle.rs:412`: "/// RULE: an enablement is reported non-terminal (`UNDER_CHANGE`, drift `UNKNOWN`) until" — and `tests/all/enabled_control_reset.rs:343-345` pins `(Some("UNKNOWN"), Some("UNKNOWN"), Some("UNKNOWN"))` for a reset in flight, "expected UNDER_CHANGE with UNKNOWN drift and the reset as the …".

The sibling applies it (`src/state.rs:1697-1701`):
```
        let control = self.enabled_controls.get_mut(arn)?;
        control.drift_status = DriftStatus::Unknown;
        control.inheritance_drift_status = DriftStatus::Unknown;
        control.resource_drift_status = DriftStatus::Unknown;
```

The update path skips it (`src/state.rs:1779-1790`, the body of `start_control_change`, which `record_enabled_control_update` at `src/state.rs:1656-1668` delegates to):
```
        let control = self.enabled_controls.get_mut(arn)?;
        // The control's configuration is changing until the operation is due, so a caller
        // reading straight after the call sees the transient status rather than the
        // configuration it asked for.
        control.status = EnablementStatus::UnderChange;
        control.last_operation_identifier = operation_identifier.clone();
        control.due_at = now + OPERATION_SETTLE_AFTER;
        control.pending_update = Some(PendingEnabledControlUpdate { … });
```
No drift field is written, so the previously assessed `IN_SYNC` survives the whole in-flight window.

Handler entry point: `src/ops/update_enabled_control.rs:300-301` — `let Some(operation_identifier) = store.record_enabled_control_update(&control.arn, requested, now)`.

Test coverage: `tests/all/enabled_control_update.rs:230-244` asserts only `status`/`lastOperationIdentifier`/`parameters` while the update is in flight and `:278-298` asserts `Some("IN_SYNC")` only AFTER the settle span, so no existing test pins the in-flight drift value.

## Steps to Reproduce

1. EnableControl a catalogued control on an organizational-unit target and wait past `crate::state::OPERATION_SETTLE_AFTER`.
2. GetEnabledControl: `statusSummary.status` = `SUCCEEDED`, `driftStatusSummary.driftStatus` = `IN_SYNC`.
3. Call UpdateEnabledControl with a parameter list that differs from the configured one.
4. Immediately call GetEnabledControl: `statusSummary.status` = `UNDER_CHANGE` while `driftStatusSummary.driftStatus` (and `types.inheritance.status` / `types.resource.status`) are still `IN_SYNC`.
5. Repeat the same window with ResetEnabledControl instead of UpdateEnabledControl: the drift dimensions are `UNKNOWN` (as `tests/all/enabled_control_reset.rs:343` pins), proving the two paths disagree about the same transient state.
