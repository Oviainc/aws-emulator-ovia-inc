Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 527cd3726300db8090bc233c982f66d05e13948e

# DisableBaseline leaves the torn-down enablement reporting UNDER_CHANGE with a stale IN_SYNC inheritance drift assessment

_Filed: 2026-10-28T15:37:58Z_
_Operation: DisableBaseline | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `527cd3726300db8090bc233c982f66d05e13948e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

While a `DISABLE_BASELINE` teardown is in flight the `EnabledBaseline` stays in the store and is still reported by `GetEnabledBaseline`/`ListEnabledBaselines` as `UNDER_CHANGE` — but its `inheritance_drift_status` keeps the value it had before the call (`Some(IN_SYNC)` for a baseline the catalog assesses drift on), so one response body says "this enablement is being changed" and "it matches the configuration Control Tower expected" at the same time.

This crate states the opposite rule in its own words and implements it on every sibling path that marks an enablement `UNDER_CHANGE`. `start_enabled_baseline_configuration_change` — the one span `UpdateEnabledBaseline` and `ResetEnabledBaseline` both reach the store through — sets `inheritance_drift_status = None` when it marks the enablement `UNDER_CHANGE`, under a doc that says the assessment "stops being reported the moment the change starts". `record_enabled_baseline_disable`, which `DisableBaseline` calls, writes `status`, `last_operation_identifier`, `due_at` and `pending_deletion` but touches no drift field, so the previously assessed `IN_SYNC` survives the whole teardown window. `settle` cannot repair it either: its drift fix-up only assigns when the enablement is `Succeeded` AND its drift is already `None`, never the reverse, and an enablement whose `pending_deletion` has become due is removed earlier in the same pass.

The result is that the same transient `UNDER_CHANGE` state is reported two different ways depending on which call produced it: an update/reset in flight reports no `driftStatusSummary` at all (pinned by tests), a teardown in flight reports `driftStatusSummary.types.inheritance.status = IN_SYNC`. The fix is to drop the assessment where the teardown is parked (`record_enabled_baseline_disable`), the way the shared configuration-change span already does; no existing test asserts the drift value inside a teardown window, and the enablement is removed once the teardown becomes due, so no settled response changes.

## Evidence

The rule, in the crate's own words — `src/state.rs:2182-2191` (doc of `start_enabled_baseline_configuration_change`):
```
    /// The one span every call that applies a configuration to an already enabled baseline
    /// shares: mark the enablement `UNDER_CHANGE` until `due_at`, record `operation_identifier`
    /// as the operation changing it, and drop the inheritance drift assessment it carries.
    ///
    /// Drift is a property of a DEPLOYED configuration, and the configuration a call like this
    /// starts applying is not deployed until the clock makes the operation due, so the
    /// assessment the previous configuration earned stops being reported the moment the change
    /// starts; [`settle`] assesses the new configuration once it is due.
```
and the sibling path applying it — `src/state.rs:2204-2209` (reached by `record_enabled_baseline_update` at `src/state.rs:2090` and `record_enabled_baseline_reset` at `src/state.rs:2174-2178`):
```
        let enabled_baseline = self.enabled_baselines.get_mut(arn)?;
        enabled_baseline.status = EnablementStatus::UnderChange;
        enabled_baseline.last_operation_identifier = operation_identifier.to_string();
        enabled_baseline.due_at = due_at;
        enabled_baseline.inheritance_drift_status = None;
```

The teardown path skips it — `src/state.rs:2126-2138` (`record_enabled_baseline_disable`):
```
        let enabled_baseline = self.enabled_baselines.get_mut(arn)?;
        // ONE instant, held in both places that need it: the status the enablement reports
        // while the teardown runs is owed at the same moment as the removal itself, so the
        // two cannot drift apart.
        let due_at = now + OPERATION_SETTLE_AFTER;
        enabled_baseline.status = EnablementStatus::UnderChange;
        enabled_baseline.last_operation_identifier = operation_identifier.clone();
        enabled_baseline.due_at = due_at;
        enabled_baseline.pending_deletion = Some(PendingEnabledBaselineDeletion {
            operation_identifier: operation_identifier.clone(),
            due_at,
        });
```
No drift field is written, while its own doc at `src/state.rs:2104-2108` states the enablement "stays in the store, `UNDER_CHANGE`, until that instant" and "is still a resource a reporting call reports".

`settle` cannot withdraw it — `src/state.rs:1411-1419`:
```
            // diverged from that configuration yet. Assessed once and then left alone, so
            // a later reader cannot reset a drift status already reported.
            if enabled_baseline.status == EnablementStatus::Succeeded
                && enabled_baseline.inheritance_drift_status.is_none()
                && assesses_inheritance_drift(&enabled_baseline.baseline_identifier)
            {
                enabled_baseline.inheritance_drift_status =
                    Some(EnabledBaselineDriftStatus::InSync);
            }
```

The same rule as a test-level RULE statement — `tests/all/in_flight_configuration_change_drift.rs:10-16`:
```
//! The RULE under test: drift is a property of a DEPLOYED configuration, so an enablement
//! whose configuration a call has started applying reports no drift assessment at all until
//! the clock makes that operation due — the assessment the configuration it carried before
//! earned stops being reported the moment the change starts ... For an enabled baseline "no
//! assessment" is the absent `driftStatusSummary` member
```
and `tests/all/enabled_baseline_drift.rs:95-99`:
```
/// RULE: a deployed enablement reports an inheritance drift status exactly when its
/// baseline is one AWS Control Tower assesses inheritance drift on, ... an enablement still being
/// deployed reports no drift summary, because drift is a property of a deployed
/// configuration.
```
and `tests/all/enabled_baseline_reset.rs:232-237`: "while that operation is in flight the enablement is `UNDER_CHANGE` ... and no drift assessed".

Handler entry point — `src/ops/disable_baseline.rs:217-222`:
```
        // The teardown starts an asynchronous operation, as every call acting on an
        // enablement does, so the identifier it returns names a record in the baseline
        // operation log; the enablement stays in the store, `UNDER_CHANGE`, until the clock
        // makes the removal parked on it due and `settle` removes it. A caller polling
        // straight after this call therefore still sees it UNDER_CHANGE.
        let Some(operation_identifier) = store.record_enabled_baseline_disable(&arn, now) else {
```

Test coverage: the teardown window's drift value is pinned by no test. `tests/all/enabled_baseline_pending_transitions.rs:272-277` is the one place a read happens inside a teardown window and it asserts only `(status(&details), last_operation(&details)) == (Some("UNDER_CHANGE"), Some(identifier))`; `tests/all/disable_baseline.rs:43-228` never calls `get-enabled-baseline` in that window at all. By contrast `tests/all/enabled_baseline_reset.rs:348` and `tests/all/in_flight_configuration_change_drift.rs:330` pin "no driftStatusSummary while the configuration is being applied" for the update/reset windows, proving the two paths disagree about the same transient state.

## Steps to Reproduce

1. EnableBaseline the `AWSControlTowerBaseline` catalog entry (one whose `detects_inheritance_drift` is true, so a drift status is assessed) on an organizational-unit target, at a version it offers.
2. Wait past `crate::state::OPERATION_SETTLE_AFTER` so the enablement settles.
3. GetEnabledBaseline: `statusSummary.status` = `SUCCEEDED` and `driftStatusSummary` = `{"types":{"inheritance":{"status":"IN_SYNC"}}}`.
4. Call DisableBaseline with that `enabledBaselineIdentifier` and immediately call GetEnabledBaseline again (inside the settle span).
5. Observed: `statusSummary.status` = `UNDER_CHANGE` while `driftStatusSummary` still reports `IN_SYNC`; `ListEnabledBaselines` reports the same pair.
6. Repeat the same window with ResetEnabledBaseline (or UpdateEnabledBaseline) instead of DisableBaseline: `driftStatusSummary` is absent, as `tests/all/enabled_baseline_reset.rs:348` pins — the two in-flight windows report the same `UNDER_CHANGE` state differently.
