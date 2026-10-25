Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 3a80585caf3e9f62e646ed534a139af0bb6604b7

# Validate UpdateEnabledBaseline's baselineVersion and parameter keys instead of applying any pattern-shaped version

_Filed: 2026-10-24T07:41:10Z_
_Operation: UpdateEnabledBaseline | Class: validation_

> _Note: this issue was identified against the code base at commit `3a80585caf3e9f62e646ed534a139af0bb6604b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateEnabledBaseline applies whatever `baselineVersion` and `parameters` the request carries to the stored enablement, with no check against the baseline catalog or the baseline's declared parameters, so requests real AWS rejects with ValidationException answer 200, log a SUCCEEDED UPDATE_ENABLED_BASELINE operation, and leave the `EnabledBaseline` reporting a version/parameter set that cannot exist in AWS.

Expected (real AWS): `baselineVersion` must be a version the enabled baseline actually offers (e.g. `4.0` for AWSControlTowerBaseline); an arbitrary value such as `10.10.1000` or `0.0` is refused with ValidationException (400), and the enablement keeps its current version. Each `parameters` key must be a parameter the baseline declares (e.g. `IdentityCenterEnabledForBaseline`); an undeclared key is refused. A request that neither changes the version nor the parameters, or that asks for a version the baseline no longer supports for that target, is likewise refused rather than silently applied.

Actual (emulator): the handler resolves the enablement, refuses an overlapping in-flight operation, and then persists the requested version and parameter list verbatim (`record_enabled_baseline_update`), with the comment "the requested version is persisted as given rather than checked against a catalog this service cannot yet consult". Once the clock makes the update due, `settle` writes that version and those parameters onto the enablement, so GetEnabledBaseline reports them as the applied configuration.

## Evidence

Smithy model: UpdateEnabledBaseline (POST /update-enabled-baseline) declares ValidationException (400) and ResourceNotFoundException (404); `baselineVersion` is `@required` (len 1..10, pattern ^\d+(?:\.\d+){0,2}$) — the pattern bounds the SHAPE of a version only, not the set of versions a baseline offers, and the existence of `ListBaselines`/`GetBaseline` plus `BaselineArn` (pattern ^arn:[a-z-]+:controltower:...:baseline/[A-Z0-9]{16}$) establishes baselines (and therefore their versions and declared parameters) as a fixed, service-owned catalog. `EnabledBaselineDetails$baselineVersion`/`$parameters` are reported back from the stored value, so an unchecked value is observable.

Handler: services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/update_enabled_baseline.rs lines 186-244 — ARN resolution, in-flight conflict check, then `store.record_enabled_baseline_update(&arn, &input.baseline_version, parameters, now)` with no catalog or parameter-name validation (see the comment at lines 216-223); src/state.rs:922-948 parks the values and src/state.rs:626-647 applies them verbatim at settle time.

Tests: tests/all/enabled_baseline_update.rs `a_version_outside_the_modelled_bounds_is_refused` only covers the FRONT DOOR's length/pattern refusals, and `an_update_applies_the_requested_version_and_parameters` asserts arbitrary versions/parameter keys round-trip. No test asserts an unsupported baseline version or an undeclared parameter key is refused. The backlog's EnableBaseline validation issue covers the enable path only; there is no UpdateEnabledBaseline validation issue.

## Steps to Reproduce

1. POST /enable-baseline with a baseline/target pair and baselineVersion "4.0"; wait past the settle span so the enablement is SUCCEEDED.
2. POST /update-enabled-baseline with {"enabledBaselineIdentifier":"<the arn>","baselineVersion":"10.10.1000"} — the emulator answers 200 with an operationIdentifier; AWS refuses the unsupported baseline version.
3. POST /get-baseline-operation with that identifier after the settle span — operationType UPDATE_ENABLED_BASELINE, status SUCCEEDED.
4. POST /get-enabled-baseline with the enablement ARN — the emulator reports baselineVersion 10.10.1000 as the applied version, state AWS would never produce.
5. POST /update-enabled-baseline with {"baselineVersion":"4.0","parameters":[{"key":"NoSuchParameter","value":true}]} — emulator answers 200 and, once due, GetEnabledBaseline reports the undeclared parameter as applied; AWS refuses a parameter key the baseline does not declare.
