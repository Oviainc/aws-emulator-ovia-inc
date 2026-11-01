Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 9fa26429e9fb75f482c46ca7a1dcc182c128efe3

# EnableBaseline answers ConflictException for an already-DEPLOYED pair, where its sibling EnableControl answers ValidationException

_Filed: 2026-10-29T10:35:18Z_
_Operation: EnableBaseline | Class: error-response_

> _Note: this issue was identified against the code base at commit `9fa26429e9fb75f482c46ca7a1dcc182c128efe3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both `EnableBaseline` and `EnableControl` enforce the same cardinality rule in this crate — one baseline/control per target — and both model `ConflictException` (409) and `ValidationException` (400). `EnableControl` splits the refusal in two along the state of the existing enablement: a repeat while the first enablement is still `UNDER_CHANGE` is 409 `ConflictException` (it would race a change in flight, which is the inconsistent state the modelled error describes), and a repeat once that enablement is DEPLOYED is 400 `ValidationException` ("the request asks for something that already exists"). `EnableBaseline` makes no such distinction: it answers 409 `ConflictException` for both, including for a pair whose `ENABLE_BASELINE` operation has long since become due and whose enablement reports `SUCCEEDED`.

So one and the same condition — "the pair you named is already enabled and fully deployed" — is classified two different ways by two sibling handlers of this crate, and the 409 answer contradicts the trigger the model states for the error it uses: `ConflictException` is documented as "Updating or deleting the resource can cause an inconsistent state", and a second enable of a settled pair neither updates nor deletes anything (the handler writes nothing and there is no in-flight change to leave half applied). `EnableControl`'s own comment spells that reading out; `EnableBaseline`'s comment gives no reason for collapsing the two cases.

A client cannot tell from the status code whether it must wait (a transient 409 while a deployment completes) or stop (a permanent 400 because the resource exists) for baselines, although it can for controls.

## Evidence

`EnableBaseline` collapses both cases into 409 — services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/enable_baseline.rs:281-300:
```
        // One baseline per target: the pair is what identifies an enablement, so a second
        // enable of the same pair would leave two `EnabledBaseline` resources describing
        // one target. ...
        if let Some(existing) =
            store.enabled_baseline_on_target(baseline, &input.target_identifier)
        {
            return Err(EnableBaselineErrorModeled::ConflictException(
                ConflictException {
                    message: format!(
                        "baseline {} is already enabled on target {} as {existing}",
                        input.baseline_identifier, input.target_identifier
                    ),
                },
            )
            .into());
        }
```

The sibling handler for exactly the same rule splits them — src/ops/enable_control.rs:296-327:
```
        // A control is enabled at most once per target. A repeat while the first
        // enablement is still deploying would race it, which is the inconsistent state
        // `ConflictException` reports; once it is deployed the request asks for something
        // that already exists. ...
        if let Some(existing) = store.enabled_control_on_target(control, &input.target_identifier)
        {
            let existing_arn = existing.arn.clone();
            if existing.status == crate::state::EnablementStatus::UnderChange {
                return Err(EnableControlErrorModeled::ConflictException(
...
            }
            return Err(EnableControlErrorModeled::ValidationException(
                ValidationException {
                    message: format!(
                        "control {} is already enabled on target {} as {existing_arn}",
                        input.control_identifier, input.target_identifier
                    ),
```

The modelled trigger the 409 answer contradicts (`smithy_query(query="shape", shape="ConflictException")`):
```
"ConflictException": {"errorKind": "client", "httpError": 409, "doc": "<p>Updating or deleting the resource can cause an inconsistent state.</p>"}
```
versus `ValidationException` (400): "The input does not satisfy the constraints specified by an Amazon Web Services service."

Test coverage pinning the divergence, with no reason stated for it:
- controls, both cases, split 409/400 — tests/all/one_enabled_control_per_target.rs:11-18:
```
//! The rule under test, asserted over a TABLE of inputs rather than one case: a control is
//! identified by the catalog entry its identifier resolves to, so one control on one
//! target is ONE enabled control with ONE operation lifecycle, whichever of the control's
//! published identifier forms each call names it by — a repeat refused while the first
//! enablement is deploying (409 `ConflictException`) or once it is deployed (400
//! `ValidationException`) names the enablement it collided with
```
  and the deployed case at tests/all/one_enabled_control_per_target.rs:154-165 (`tokio::time::sleep(past_the_settle_span())` then `assert_refused_naming(..., 400, "ValidationException", arn)`).
- baselines, the DEPLOYED case asserted as 409 — tests/all/disable_baseline.rs:402 and :431-440:
```
    tokio::time::sleep(past_the_settle_span()).await;
...
        // The sibling is still enabled, so its pair cannot be enabled a second time...
        let again = enable(&router, &rows[index]).await;
        assert_eq!(
            (again.status, again.error_code.as_deref()),
            (409, Some("ConflictException")),
```
(the enable-side suite's own repeat case, tests/all/baseline_operations.rs:486-495, fires inside the settle span, so it only pins the IN-FLIGHT case, which both handlers already answer 409.)

The `EnableBaseline` handler's module header states no reason for the choice — src/ops/enable_baseline.rs:3-17 describes the catalog checks and the tag rule only.

## Steps to Reproduce

1. Start the emulator and POST /enable-baseline with {"baselineIdentifier": "arn:aws:controltower:us-west-2::baseline/17BSJV3IGJ2QSGA2", "baselineVersion": "1.0", "targetIdentifier": "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-aa11-1a"} — observe 200 with an `arn` and `operationIdentifier`.
2. Wait past the settle span (crate::state::OPERATION_SETTLE_AFTER, 500 ms) and POST /get-enabled-baseline for that `arn` — `statusSummary.status` is SUCCEEDED, so the enablement is deployed and nothing is in flight.
3. POST /enable-baseline again with the identical body — observe 409 ConflictException "baseline ... is already enabled on target ... as ..." (enable_baseline.rs:291-299).
4. POST /enable-control with {"controlIdentifier": "arn:aws:controltower:us-west-2::control/AWS-GR_ENCRYPTED_VOLUMES", "targetIdentifier": "arn:aws:organizations::123456789012:ou/o-exampleorg/ou-bb22-2b"}, wait past the settle span, and repeat the identical call — observe 400 ValidationException "control ... is already enabled on target ... as ..." (enable_control.rs:318-326).
5. Compare: the same condition (the pair is already enabled and deployed, nothing in flight, nothing written) is reported as 409 by one handler and 400 by the other, and the 409 is the status the model reserves for "Updating or deleting the resource can cause an inconsistent state".
