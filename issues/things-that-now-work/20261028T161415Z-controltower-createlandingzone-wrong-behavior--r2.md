Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: eba09938c5757f116b88f59710a766eb7f22de4f

# CreateLandingZone reports driftStatus IN_SYNC for a manifest that is not deployed yet

_Filed: 2026-10-28T16:14:15Z_
_Operation: CreateLandingZone | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `eba09938c5757f116b88f59710a766eb7f22de4f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateLandingZone` starts an asynchronous CREATE: the handler stores the landing zone as `PROCESSING` and `settle` only makes it `ACTIVE` once the clock makes the CREATE operation due. In the same stored record the handler asserts `drift_status: Some(InSync)` immediately, so `GetLandingZone` called while the create is in flight answers `status: "PROCESSING"` together with `driftStatus: { "status": "IN_SYNC" }` — a drift assessment against a configuration this service has not deployed yet.

That contradicts the rule this crate applies everywhere else that a configuration starts being deployed, including on the landing zone itself:

* `UpdateLandingZone`, the sibling call on the SAME resource, does not report the new `IN_SYNC` straight away: it parks it inside `pending_configuration` so that a reporting call keeps reporting the DEPLOYED configuration's drift until the clock makes the deployment due, and only then does `settle` copy `pending.drift_status` onto the landing zone.
* `EnableControl` stores all three drift dimensions as `UNKNOWN` for exactly this reason ("Drift is not assessable until the configuration is deployed"), and `settle` moves them to `IN_SYNC` once the enablement is due.
* `record_enablement` (the EnableBaseline path) stores `inheritance_drift_status: None` for exactly this reason ("Drift is a property of a DEPLOYED configuration, so nothing is assessed while the ENABLE_BASELINE operation is still in flight").

So the first-ever deployment of a control and of a baseline both withhold the assessment while the operation is in flight, and an UPDATE of the landing zone withholds it too; only the landing zone's CREATE publishes it before anything is deployed. `LandingZoneDetail$driftStatus` is an optional member (the crate already models the stored value as `Option<LandingZoneDriftStatus>` and `get_landing_zone.rs` maps `None` to an absent member), so "not assessed yet" is representable on this path exactly as it is on the other two. No test pins the in-flight value: `tests/all/landing_zone_lifecycle.rs` only asserts `PROCESSING` while the create is in flight, and the drift assertions in `tests/all/landing_zone_update.rs` / `landing_zone_reset.rs` are both taken after the operation has settled.

## Evidence

The create publishes the assessment while the landing zone is still PROCESSING — `src/ops/create_landing_zone.rs:279-284`:
```
                // The landing zone is PROCESSING until its CREATE operation is due; the
                // status is derived from the log by `settle`, and seeded to match here.
                status: crate::state::LandingZoneStatus::Processing,
                // Nothing has diverged from a manifest the service has only just been
                // given, so a new landing zone is in sync.
                drift_status: Some(crate::state::LandingZoneDriftStatus::InSync),
```

The sibling update of the SAME resource withholds it until the deployment is due — `src/ops/update_landing_zone.rs:292-301`:
```
        landing_zone.pending_configuration = Some(crate::state::PendingLandingZoneConfiguration {
            version: input.version,
            // The deployed configuration will be exactly the manifest this call supplied,
            // so it will be in sync with it; an update that supplies no manifest changes
            // nothing the manifest describes, so it carries the drift status forward.
            drift_status: if input.manifest.is_some() {
                Some(crate::state::LandingZoneDriftStatus::InSync)
            } else {
                landing_zone.drift_status
            },
```
and `src/state.rs:1189-1203` is where that parked status is finally reported: "A configuration an UPDATE started deploying becomes the deployed one once the clock has made that deployment due ... Until then a reporting call reports what is actually deployed." followed by `landing_zone.drift_status = pending.drift_status;`.

The first-ever deployment of the other two resources withholds it — `src/ops/enable_control.rs:376-380`:
```
                // Drift is not assessable until the configuration is deployed; `settle`
                // moves these to `IN_SYNC` once the enablement is due.
                drift_status: crate::state::DriftStatus::Unknown,
                inheritance_drift_status: crate::state::DriftStatus::Unknown,
                resource_drift_status: crate::state::DriftStatus::Unknown,
```
and `src/state.rs:1956-1959`:
```
                // Drift is a property of a DEPLOYED configuration, so nothing is assessed
                // while the ENABLE_BASELINE operation is still in flight; `settle`
                // materialises it once that operation is due.
                inheritance_drift_status: None,
```

The member is reported straight out of the stored value, so the create's value is what a caller sees — `src/ops/get_landing_zone.rs:199-205`:
```
                status: Some(landing_zone.status.as_wire().to_string()),
                latest_available_version: Some(landing_zone.latest_available_version),
                drift_status: landing_zone
                    .drift_status
                    .map(|status| LandingZoneDriftStatusSummary {
                        status: Some(status.as_wire().to_string()),
                    }),
```

Test coverage: `tests/all/landing_zone_lifecycle.rs:312-326` asserts only the status while the create is in flight ("get-landingzone while the create is in flight: expected PROCESSING"); `tests/all/landing_zone_update.rs:376-381` and `tests/all/landing_zone_reset.rs:284-289` assert `IN_SYNC` only after the operation has settled; `tests/all/in_flight_configuration_change_drift.rs:10-18` states the rule for the other two resources ("drift is a property of a DEPLOYED configuration, so an enablement whose configuration a call has started applying reports no drift assessment at all until the clock makes that operation due").

## Steps to Reproduce

1. POST /create-landingzone with a valid `version` and manifest; note the returned `arn`.
2. Immediately (within OPERATION_SETTLE_AFTER = 500ms) POST /get-landingzone with that `landingZoneIdentifier`.
3. Observe the response carries `landingZone.status = "PROCESSING"` together with `landingZone.driftStatus = { "status": "IN_SYNC" }` — a drift assessment for a configuration the service has not deployed yet.
4. Compare with POST /enable-control (or /enable-baseline) followed immediately by /get-enabled-control (or /get-enabled-baseline): the control reports `UNKNOWN` in all three drift dimensions and the baseline reports no `driftStatusSummary` at all until the operation is due.
5. Compare with /update-landingzone followed immediately by /get-landingzone: the new `IN_SYNC` is withheld until the queued deployment becomes due.
