Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 0711e0a570adac04732f4f058a2849d70bc38321

# UpdateLandingZone decides the in-flight conflict before the version-downgrade refusal, inverting the precedence both sibling update handlers apply

_Filed: 2026-10-29T17:26:20Z_
_Operation: UpdateLandingZone | Class: error-response_

> _Note: this issue was identified against the code base at commit `0711e0a570adac04732f4f058a2849d70bc38321`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateLandingZone implements two refusals that this crate's other two "update an existing resource" handlers order the other way round: (1) the request's own `version` must be one the landing zone can reach from where it is (400 ValidationException, the forward-only rule), and (2) a landing zone with an operation still in flight is a conflict (409 ConflictException). The handler checks the conflict FIRST, so one and the same never-acceptable request — an update to a version older than the one the landing zone is on or on its way to — is answered 409 ConflictException while a CREATE/UPDATE/RESET/DELETE is still settling, and 400 ValidationException a moment later once the clock has made that operation due. Neither answer writes anything, so this is purely which modelled error the caller is told.

The ordering also defeats the stated purpose of the downgrade check's own comment. The handler says the comparison is made against the version an in-flight deployment is on its way to "so the answer does not depend on whether the clock has made that deployment due yet", and `LandingZone::configured_version()` exists only to supply that pending version — but the conflict check above it returns first whenever a deployment is in flight, so `configured_version()`'s pending branch is unreachable on the wire, which `state.rs` itself records ("DEFENSIVE on the wire path: `UpdateLandingZone` refuses a landing zone with an operation in flight with `ConflictException` before it gets here"). The answer therefore DOES depend on whether the clock has made the deployment due: 409 before, 400 after.

Nothing about the landing zone path forces this order. The value the downgrade check needs (`store.landing_zones.get(&arn).configured_version()`) is read out of the same locked store, through a shared borrow, immediately after the conflict check, so it can be settled first exactly as `UpdateEnabledControl` and (since its own fix) `UpdateEnabledBaseline` settle their request-level refusals before the conflict.

## Evidence

Conflict FIRST — services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/update_landing_zone.rs:238-250:
```
        // Updating a landing zone whose own asynchronous operation has not finished would
        // leave it in an inconsistent state, which is what `ConflictException` reports.
        if store.has_operation_in_flight(&arn) {
            return Err(UpdateLandingZoneErrorModeled::ConflictException(
                ConflictException {
                    message: format!(
                        "landing zone {arn} has an operation in progress; updating it now \
                         would leave it in an inconsistent state"
                    ),
```
and the request's own version refusal only AFTER it — update_landing_zone.rs:252-274:
```
        // A landing zone moves FORWARD only. The catalog check above says the requested
        // version EXISTS; this one says the landing zone can get there from where it is:
        // ... Ordered against the version an in-flight
        // deployment is on its way to, when there is one, so the answer does not depend on
        // whether the clock has made that deployment due yet. Decided through a SHARED
        // borrow, before anything takes a mutable one: a refused update writes nothing.
        let refusal = store
            .landing_zones
            .get(&arn)
            .and_then(|landing_zone| {
                crate::state::version_downgrade_message(
                    landing_zone.configured_version(),
                    &input.version,
                )
            });
        if let Some(message) = refusal {
            return Err(
                UpdateLandingZoneErrorModeled::ValidationException(ValidationException { message })
                    .into(),
            );
```
The crate records that the pending-version branch this comment relies on is unreachable on the wire because of that order — src/state.rs:868-880:
```
    /// A further update is ordered against THIS rather than against `version`, so a
    /// version already on its way to being deployed cannot be walked backwards by a
    /// second call that arrives before the first is due. DEFENSIVE on the wire path:
    /// `UpdateLandingZone` refuses a landing zone with an operation in flight with
    /// `ConflictException` before it gets here, and a deployment the clock has made due
    /// has already been applied by [`settle`], so the pending branch is what keeps the
    /// order right if either of those ever stops holding.
```
The sibling order — request first, then conflict — src/ops/update_enabled_control.rs:232-239 and :261-275:
```
        if let Some(message) =
            crate::state::unacceptable_control_parameters_message(catalogued, &requested)
        {
            return Err(UpdateEnabledControlErrorModeled::ValidationException(
                ValidationException { message },
            )
            .into());
        }
```
```
        // An operation still changing this control owns it: a change starting now would
        // apply on top of one half applied, which is the inconsistent state
        // `ConflictException` reports.
        if control.status == crate::state::EnablementStatus::UnderChange {
```
and the same order in src/ops/update_enabled_baseline.rs:199-206 (its header states the principle) with the version/parameter checks at :226-256 ahead of the conflict at :263-274:
```
        // WHAT the request asks for is settled before the enablement's own state is
        // looked at, the order the sibling `UpdateEnabledControl` and the enable handlers
        // apply: ... so an update cannot move an enablement to a
        // version its baseline does not offer ... and a request carrying such a value is answered with
        // the same `ValidationException` whether or not a change is still in flight.
```

Test coverage, which pins neither precedence over the wire (so a reorder changes no existing test): tests/all/landing_zone_version_order.rs:138-181 asserts the in-flight ordering rule by calling the helper directly against the store rather than over the wire —
```
    assert!(
        version_downgrade_message(landing_zone.configured_version(), oldest).is_some(),
        "expected a move back to {oldest:?} to be refused while {newest:?} is being \
         deployed, got no refusal"
    );
```
while the wire-level test only runs against a SETTLED landing zone — tests/all/landing_zone_version_order.rs:203-225:
```
        let arn =
            create_settled_landing_zone(&router, case, deployed_version, &deployed_manifest).await;
        ...
        assert_eq!(
            (answer.status, answer.error_code.as_deref()),
            (400, Some("ValidationException")),
```

## Steps to Reproduce

1. POST /create-landingzone with {"version":"3.0","manifest":{...a valid manifest...}} and wait past crate::state::OPERATION_SETTLE_AFTER so the landing zone is ACTIVE on 3.0.
2. POST /update-landingzone with {"landingZoneIdentifier":"<arn>","version":"3.3"} and observe 200; the UPDATE is now in flight for OPERATION_SETTLE_AFTER.
3. Immediately (within that span) POST /update-landingzone with {"landingZoneIdentifier":"<arn>","version":"2.0"} — a move backwards that the service will never accept. Observe 409 ConflictException ("landing zone ... has an operation in progress") from update_landing_zone.rs:240-250, because the downgrade check at :260-274 is never reached.
4. Wait past the settle span and repeat the identical request. Observe 400 ValidationException ("version \"2.0\" is older than the landing zone's version \"3.3\"; a landing zone can only be updated to the same or a newer version") — the same malformed request, a different modelled error and status.
5. Contrast with the siblings: POST /enable-control, then immediately POST /update-enabled-control with a parameters key the control does not declare — 400 ValidationException is answered even while the ENABLE_CONTROL operation is in flight (update_enabled_control.rs:232-239); likewise /update-enabled-baseline with an unoffered baselineVersion answers 400 while a baseline operation is in flight (update_enabled_baseline.rs:226-234).
