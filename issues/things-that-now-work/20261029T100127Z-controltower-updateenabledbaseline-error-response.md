Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: cc22e59eeda2de4fd1330e74811c0c72a53dba7a

# UpdateEnabledBaseline decides the in-flight conflict before validating baselineVersion/parameters, the opposite order from its sibling UpdateEnabledControl

_Filed: 2026-10-29T10:01:27Z_
_Operation: UpdateEnabledBaseline | Class: error-response_

> _Note: this issue was identified against the code base at commit `cc22e59eeda2de4fd1330e74811c0c72a53dba7a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The two "update an existing enablement" handlers of this crate implement the same pair of refusals — (1) the request's own configuration must be one the resource's catalog entry offers, and (2) an operation still changing the resource is a conflict — but in opposite orders, so one and the same mistake is answered with two different modelled errors and HTTP statuses depending on which resource kind the caller is updating.

`UpdateEnabledControl` settles the request first: it validates `parameters` against the catalog entry of the control the enablement applies (400 `ValidationException`) and only then refuses an `UNDER_CHANGE` control with 409 `ConflictException`.

`UpdateEnabledBaseline` does the reverse: it takes the lock, resolves the ARN, and refuses with 409 `ConflictException` as soon as any baseline operation is in flight — before it has looked at `baselineVersion` or `parameters` at all. So an `UpdateEnabledBaseline` carrying a `baselineVersion` the enablement's baseline does not offer, or a `parameters` key that baseline does not declare, is reported as a conflict (409) when it arrives while the enable/update/reset is still in flight, and as a bad request (400) a moment later once the clock has made that operation due — for a request whose own members are malformed either way and which the service will never accept.

Nothing about the baseline path forces this order: the stored `baseline_identifier` the version/parameter check needs is read out of the same locked store two statements later (update_enabled_baseline.rs:220-224), so the request-level checks can be settled before the resource-level conflict exactly as the control path does. The crate states that principle itself in the sibling enable handlers ("WHAT the request asks for is settled before any state is touched"). Neither order writes anything, so this is purely which error the caller is told.

## Evidence

The baseline path — conflict FIRST, services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/update_enabled_baseline.rs:199-214:
```
        // An operation still changing this enablement owns it: a change starting now
        // would apply on top of one half applied, which is the inconsistent state
        // `ConflictException` reports. This operation is not `@idempotent`, so a repeat
        // is refused rather than reported as the same operation.
        if let Some(in_flight) = store.baseline_change_in_flight_on(&arn) {
            return Err(UpdateEnabledBaselineErrorModeled::ConflictException(
                ConflictException {
```
and only AFTER it the request's own members are checked — update_enabled_baseline.rs:239-247 and :261-269:
```
        if let Some(message) = crate::state::unsupported_baseline_version_message(
            baseline,
            &input.baseline_version,
        ) {
            return Err(UpdateEnabledBaselineErrorModeled::ValidationException(
                ValidationException { message },
            )
            .into());
        }
```
```
        if let Some(message) = parameters
            .as_deref()
            .and_then(|parameters| crate::state::unacceptable_parameters_message(baseline, parameters))
        {
            return Err(UpdateEnabledBaselineErrorModeled::ValidationException(
                ValidationException { message },
            )
            .into());
        }
```
The value the version check needs is available before the conflict check — update_enabled_baseline.rs:220-224:
```
        let stored_baseline_identifier = store
            .enabled_baselines
            .get(&arn)
            .map(|enabled| enabled.baseline_identifier.clone())
            .unwrap_or_default();
```

The sibling path — validation FIRST, src/ops/update_enabled_control.rs:232-239:
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
and the conflict only after it — src/ops/update_enabled_control.rs:261-275:
```
        // An operation still changing this control owns it: a change starting now would
        // apply on top of one half applied, which is the inconsistent state
        // `ConflictException` reports.
        if control.status == crate::state::EnablementStatus::UnderChange {
            return Err(UpdateEnabledControlErrorModeled::ConflictException(
```
with the same two rules described as one shared rule in the state module — src/state.rs:3357-3360:
```
/// The one place a control's parameter list is checked, so `EnableControl` and
/// `UpdateEnabledControl` refuse the same lists in the same words — the rule
/// [`unacceptable_parameters_message`] applies to a baseline's parameters one level up, so
/// both read it off the same walk.
```
and the crate's stated "request first" principle in the sibling enable handlers — src/ops/enable_baseline.rs:192-198:
```
        // WHAT the request asks for is settled before any state is touched, so a request
        // naming something that could not exist writes nothing: the baseline must be one
        // the service's catalog publishes, at a version that baseline offers, applied to
        // an organizational unit, configured only by parameters that baseline declares.
```

Test coverage, which does not pin the precedence either way (so a reorder changes no existing test): the conflict test sends a VALID version — tests/all/enabled_baseline_update.rs:582:
```
        let refused = update(&router, &arn, "3.0", None).await;
```
and the version/parameter validation tests run against DEPLOYED (settled) enablements, asserting 400 — tests/all/enabled_baseline_update_validation.rs:196-199 and :244-247:
```
    let enablements = deployed_enablements(&router, "uv", |baseline| {
        Some(declared_parameters(baseline))
    })
    .await;
```
```
            let refused = update(&router, arn, &version, None).await;
            assert_refused(&case, &refused, 400, "ValidationException");
```

## Steps to Reproduce

1. POST /enable-baseline with {"baselineIdentifier":"arn:aws:controltower:us-east-1::baseline/17BSJV3IGJ2QSGA2","targetIdentifier":"arn:aws:organizations::123456789012:ou/o-exampleorg/ou-cf11-aaaa","baselineVersion":"1.0"} and note the returned `arn` (the enablement is UNDER_CHANGE for crate::state::OPERATION_SETTLE_AFTER).
2. Immediately (within that settle span) POST /update-enabled-baseline with {"enabledBaselineIdentifier":"<that arn>","baselineVersion":"9.9"} — a version no catalog baseline offers. Observe 409 ConflictException ("baseline operation ... is still in progress"), because update_enabled_baseline.rs:203-214 refuses before the version is looked at.
3. Wait past the settle span and repeat the identical request. Observe 400 ValidationException ("baselineVersion \"9.9\" is not a version baseline AWSControlTowerBaseline offers") from update_enabled_baseline.rs:239-247 — the same malformed request, a different modelled error and status.
4. Do the same against a control: POST /enable-control, then immediately POST /update-enabled-control with a `parameters` key the control does not declare. Observe 400 ValidationException from update_enabled_control.rs:232-239 even while the ENABLE_CONTROL operation is in flight — the sibling answers the request-level mistake first.
