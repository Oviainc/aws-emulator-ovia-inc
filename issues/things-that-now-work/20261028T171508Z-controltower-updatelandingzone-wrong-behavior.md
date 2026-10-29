Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 68e4a9e742a6835063083b9abc795c05ac8a482e

# Tie a landing zone's parked configuration change to its operation record, as every sibling pending transition in this crate is

_Filed: 2026-10-28T17:15:08Z_
_Operation: UpdateLandingZone | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `68e4a9e742a6835063083b9abc795c05ac8a482e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every asynchronous change in this crate is modelled the same way: the call appends a record to an operation log and PARKS the transition on the resource, and `settle` materialises the parked transition once the clock makes it due — but only when the operation that started it did not FAIL. That second half is implemented for the two enablement resources and skipped for the landing zone.

The landing zone's parked transitions are the odd ones out in two connected ways:

1. `PendingLandingZoneConfiguration` (written by `UpdateLandingZone` and `ResetLandingZone`) carries NO `operation_identifier`, unlike `PendingLandingZoneDeletion`, `PendingEnabledControlUpdate`, `PendingEnabledControlDeletion`, `PendingEnabledBaselineUpdate` and `PendingEnabledBaselineDeletion`, every one of which holds the identifier with the stated purpose that "applying it never depends on the operation's record still being in the (bounded) log". Because there is no identifier, `settle` cannot ask whether the deployment failed, and it applies the queued version/manifest/remediationTypes/drift status unconditionally (state.rs:1193-1204) — where the control path (state.rs:1276-1284) and the baseline path (state.rs:1376-1387) explicitly keep the old configuration for a FAILED operation.

2. The landing-zone decommissioning path has the identifier but still does not consult it: `settle` removes the landing zone and its tags on `now >= pending.due_at` alone (state.rs:1171-1187), where both sibling teardowns refuse to remove the resource when their own record FAILED ("A teardown whose own record FAILED removed nothing, so the enablement stays", state.rs:1236-1250 and state.rs:1333-1351).

This is not hypothetical scaffolding that nobody meant to write: `settle` already carries the matching guard for the landing zone's STATUS, with a comment that names exactly the operations that will be able to fail it — "it is here so the later UpdateLandingZone/ResetLandingZone slice, which can fail a landing zone, does not have its status silently overwritten by a reader" (state.rs:1209-1213). So the crate already defends the reported status against a failed landing-zone operation while leaving the configuration deployment and the decommissioning to be applied as if the operation had succeeded. The moment anything sets `LandingZoneOperationStatus::Failed`, a failed UPDATE will still overwrite the deployed manifest and a failed DELETE will still destroy the landing zone and its tags, which is the state loss the sibling guards exist to prevent.

Related, same root cause: the in-flight check the three mutating landing-zone handlers share reads the EVICTABLE log (`has_operation_in_flight` -> `any_operation_in_flight`, state.rs:1440-1441) rather than the transition parked on the resource, which is the rule this crate states everywhere else for precisely this reason (state.rs:949-956, state.rs:1848-1858). `settle`'s landing-zone status derivation consults `pending_deletion` but not `pending_configuration` (state.rs:1220-1221), so a parked configuration deployment is invisible to both the status and the conflict check; `UpdateLandingZone` would then accept a second update and overwrite the still-owed `pending_configuration` (update_landing_zone.rs:292). The fix for all of these is in one place — give the parked configuration its operation identifier and have `settle`/`has_operation_in_flight` read the parked transitions, the way the control and baseline paths already do.

The defect lives in `crate::state::settle` and `has_operation_in_flight`; `UpdateLandingZone` is the first operation whose handler parks a landing-zone configuration and calls them (`ResetLandingZone` and `DeleteLandingZone` reach the same code).

## Evidence

The landing zone's parked configuration applied with no check of its operation, `src/state.rs:1189-1204`:
```
        // A configuration an UPDATE started deploying becomes the deployed one once the
        // clock has made that deployment due — the same instant the operation record
        // reaches its terminal status — whether or not anybody called. Until then a
        // reporting call reports what is actually deployed.
        for landing_zone in self.landing_zones.values_mut() {
            let due = landing_zone
                .pending_configuration
                .take_if(|pending| now >= pending.due_at);
            if let Some(pending) = due {
                landing_zone.version = pending.version;
                landing_zone.latest_available_version =
                    latest_available_version(&landing_zone.version, CATALOG_LANDING_ZONE_VERSION);
                landing_zone.manifest = pending.manifest;
                landing_zone.remediation_types = pending.remediation_types;
                landing_zone.drift_status = pending.drift_status;
            }
        }
```

The sibling control path, which DOES check, `src/state.rs:1273-1284`:
```
            let Some(pending) = due else {
                continue;
            };
            if control_operations
                .get(&pending.operation_identifier)
                .is_some_and(|operation| operation.status == ControlOperationStatus::Failed)
            {
                // A failed update applied nothing, so the control keeps the parameters it
                // had.
                continue;
            }
            control.parameters = pending.parameters;
```

The sibling baseline path, same guard, `src/state.rs:1376-1387`:
```
            if baseline_operations
                .get(&pending.operation_identifier)
                .is_some_and(|operation| operation.status == BaselineOperationStatus::Failed)
            {
                // A failed update applied nothing, so the enablement keeps the version
                // and parameters it had.
                continue;
            }
            enabled_baseline.baseline_version = pending.baseline_version;
```

The landing zone's decommissioning, which has the identifier but ignores its status, `src/state.rs:1171-1187`:
```
        let decommissioned: Vec<String> = self
            .landing_zones
            .iter()
            .filter(|(_, landing_zone)| {
                landing_zone
                    .pending_deletion
                    .as_ref()
                    .is_some_and(|pending| now >= pending.due_at)
            })
            .map(|(arn, _)| arn.clone())
            .collect();
        for arn in decommissioned {
            self.landing_zones.remove(&arn);
            ...
            self.tags.remove(&arn);
        }
```
against the control teardown, `src/state.rs:1242-1250`:
```
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
```

The crate's own comment admitting a landing-zone operation will be able to fail, `src/state.rs:1209-1213`:
```
            // A FAILED landing zone is not re-derived from the log. Nothing in this
            // slice can set `Failed` yet, so the branch is unreachable today; it is here
            // so the later UpdateLandingZone/ResetLandingZone slice, which can fail a
            // landing zone, does not have its status silently overwritten by a reader.
```

The parked configuration carries no operation identifier, `src/state.rs:613-624`:
```
pub struct PendingLandingZoneConfiguration {
    pub version: String,
    pub manifest: ::aws_smithy_types::Document,
    pub remediation_types: Option<Vec<String>>,
    /// The drift status the deployed landing zone will carry once this configuration is
    /// deployed.
    pub drift_status: Option<LandingZoneDriftStatus>,
    /// The instant the clock owes this deployment. Stored, never slept on: a reader
    /// materialises the transition once it is due.
    pub due_at: SystemTime,
}
```
while every other parked transition carries one for the stated reason, e.g. `src/state.rs:777-783`:
```
pub struct PendingEnabledControlUpdate {
    /// The `UPDATE_ENABLED_CONTROL` operation whose completion applies this change.
    pub operation_identifier: String,
    /// The instant the clock owes this change its application — the same instant that
    /// operation becomes due, held here so applying it never depends on the operation's
    /// record still being in the (bounded) log. Stored, never slept on.
    pub due_at: SystemTime,
```

In-flight detection read off the evictable log instead of the parked transition, `src/state.rs:1437-1441`:
```
    /// Whether any operation against `arn` is still `IN_PROGRESS`. Call [`settle`] first.
    pub fn has_operation_in_flight(&self, arn: &str) -> bool {
        any_operation_in_flight(&self.landing_zone_operations, arn)
    }
```
called by `src/ops/update_landing_zone.rs:240`, `src/ops/reset_landing_zone.rs:203` and `src/ops/delete_landing_zone.rs:219`, against the rule the crate states for the other resources, `src/state.rs:1851-1858`:
```
    /// Read off the RESOURCE — the status [`settle`] derived from the transitions parked on
    /// it — rather than off the operation log, for the same reason
    /// [`deletion_in_flight_on`] reads a landing zone's parked removal:
    /// [`enforce_log_bound`] can evict a record while the transition it started is still
    /// owed, and a change in flight must not stop being reported because its record was
    /// dropped.
```
and `src/state.rs:1220-1221`, where the landing zone's status consults only one of its two parked transitions:
```
            let in_flight = any_operation_in_flight(operations, arn)
                || landing_zone.pending_deletion.is_some();
```

The parked configuration is created at `src/ops/update_landing_zone.rs:292-309` (and `src/ops/reset_landing_zone.rs:233-239`) with no operation identifier, even though the identifier is minted three lines later (`update_landing_zone.rs:314-315`).

Test coverage today exercises only the succeeding paths — `tests/all/landing_zone_update.rs`, `tests/all/landing_zone_reset.rs`, `tests/all/landing_zone_decommission.rs`, `tests/all/enabled_baseline_pending_transitions.rs` — so nothing pins the failed-operation behaviour either way, and the fix keeps every one of them passing.

## Steps to Reproduce

1. Read `src/state.rs:1193-1204`: the landing zone's `pending_configuration` is applied on `now >= pending.due_at` alone — there is no `operation_identifier` on the struct (`src/state.rs:613-624`) for `settle` to resolve, so the deployment cannot be conditioned on the operation's terminal status.
2. Compare `src/state.rs:1276-1284` (control) and `src/state.rs:1376-1387` (baseline): both resolve `pending.operation_identifier` and skip applying the change when that record is `Failed`.
3. Compare `src/state.rs:1171-1187` (landing zone removal, no status check) with `src/state.rs:1242-1250` (control teardown) and `src/state.rs:1345-1351` (baseline teardown), which both keep the resource when their own record FAILED.
4. Read `src/state.rs:1209-1213`: the crate already guards the landing zone's STATUS against a FAILED landing-zone operation and names `UpdateLandingZone`/`ResetLandingZone` as the slice that can produce one — so a failed update/delete would overwrite the deployed manifest, or delete the landing zone and its tags, while the status guard says the failure must survive a reader.
5. Read `src/state.rs:1220-1221` and `src/state.rs:1440-1441`: a parked `pending_configuration` is invisible to both the derived status and `has_operation_in_flight`, so `UpdateLandingZone` (`src/ops/update_landing_zone.rs:240,292`) decides "nothing is in flight" from the evictable log and overwrites a still-owed configuration — the rule `src/state.rs:1851-1858` states for every other pending transition.
