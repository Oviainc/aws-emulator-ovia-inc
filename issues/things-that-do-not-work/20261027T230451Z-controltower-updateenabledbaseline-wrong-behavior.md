Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c56dd21dd1ebda0a2d29e841f8875a573d0c3ba4

# UpdateEnabledBaseline keeps the inheritance drift assessment while the new configuration is still being applied

_Filed: 2026-10-27T23:04:51Z_
_Operation: UpdateEnabledBaseline | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c56dd21dd1ebda0a2d29e841f8875a573d0c3ba4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

While an `UPDATE_ENABLED_BASELINE` is in flight the enablement is reported `UNDER_CHANGE` but its `driftStatusSummary` keeps the assessment made for the PREVIOUS configuration, so `GetEnabledBaseline`/`ListEnabledBaselines` report a drift verdict for a configuration that is not deployed yet.

The crate states that drift is a property of a deployed configuration and implements exactly that on the sibling path: `ResetEnabledBaseline` sets `inheritance_drift_status = None` (unassessed) when it starts its change, so the summary member disappears until `settle` assesses it again. `UpdateEnabledBaseline` — whose change is equally in flight and equally un-deployed — leaves the field alone, so the stale assessment keeps being reported.

The fix is to clear `inheritance_drift_status` in `record_enabled_baseline_update` the way `record_enabled_baseline_reset` already does; `settle` re-assesses it to `IN_SYNC` once the operation becomes due (`src/state.rs:1400-1412`), so the settled response is unchanged and the existing update tests (which assert nothing about drift) keep passing.

## Evidence

The rule, in the crate's own words:

* `src/state.rs:955-961` (doc of `EnabledBaseline::inheritance_drift_status`): "…and drift is a property of a DEPLOYED configuration, so it is assessed only once the enablement's operation has become due — [`settle`] materialises it then, the same way it materialises an enabled control's drift statuses."
* `src/state.rs:2138-2141` (doc of `record_enabled_baseline_reset`): "What the re-application settles is inheritance drift — an enablement cannot be assessed against a configuration still being applied, so the drift status goes back to UNASSESSED here and [`settle`] assesses it again once the clock has made the operation due."
* `tests/all/enabled_baseline_reset.rs:346-349` pins the sibling: `drift_summary(&in_flight)` must be absent — "case {:?}: expected no driftStatusSummary while the configuration is being re-applied, got …".

The sibling applies it (`src/state.rs:2162-2170`):
```
        let enabled_baseline = self.enabled_baselines.get_mut(arn)?;
        enabled_baseline.status = EnablementStatus::UnderChange;
        enabled_baseline.last_operation_identifier = operation_identifier.clone();
        enabled_baseline.due_at = now + OPERATION_SETTLE_AFTER;
        enabled_baseline.inheritance_drift_status = None;
```

The update path skips it (`src/state.rs:2071-2087`, `record_enabled_baseline_update`):
```
        let enabled_baseline = self.enabled_baselines.get_mut(arn)?;
        let due_at = now + OPERATION_SETTLE_AFTER;
        enabled_baseline.status = EnablementStatus::UnderChange;
        enabled_baseline.last_operation_identifier = operation_identifier.clone();
        enabled_baseline.due_at = due_at;
        enabled_baseline.pending_update = Some(PendingEnabledBaselineUpdate { … });
```
`inheritance_drift_status` is never written, and the reporting path serialises whatever it holds (`src/ops/get_enabled_baseline.rs:211-219`: `drift_status_summary: enabled_baseline.inheritance_drift_status.map(|status| …)`).

Handler entry point: `src/ops/update_enabled_baseline.rs:271-272` — `store.record_enabled_baseline_update(&arn, &input.baseline_version, parameters, now)`.

Test coverage: `tests/all/enabled_baseline_update.rs` contains no `drift` assertion at all (grep for "drift" in that file returns no matches), so nothing pins the current in-flight behaviour.

## Steps to Reproduce

1. EnableBaseline a catalogued baseline whose inheritance drift AWS assesses (`state::assesses_inheritance_drift` true) on an organizational-unit target, and wait past `crate::state::OPERATION_SETTLE_AFTER`.
2. GetEnabledBaseline: `statusSummary.status` = `SUCCEEDED` and `driftStatusSummary.types.inheritance.status` = `IN_SYNC`.
3. Call UpdateEnabledBaseline with another `baselineVersion` the baseline offers.
4. Immediately call GetEnabledBaseline: `statusSummary.status` = `UNDER_CHANGE` while `driftStatusSummary` is still present and still reports `IN_SYNC` for a configuration that has not been applied.
5. Repeat the same window with ResetEnabledBaseline instead: `driftStatusSummary` is absent (as `tests/all/enabled_baseline_reset.rs:346` pins), proving the two paths disagree about the same transient state.
