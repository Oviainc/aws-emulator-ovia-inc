Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 0a6367a2f72726ece298ef6fb266120e608ee126

# ResetEnabledControl never asks whether the enablement is one AWS Control Tower applied itself, the check both its siblings apply to a mandatory control

_Filed: 2026-10-29T11:08:50Z_
_Operation: ResetEnabledControl | Class: validation_

> _Note: this issue was identified against the code base at commit `0a6367a2f72726ece298ef6fb266120e608ee126`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A MANDATORY catalog control is not a caller's resource in this crate: it is written by the service itself when a target comes under its governance (`apply_governance_controls`), and both sibling calls that act on such a control refuse the request with `ValidationException` — `EnableControl` because "only strongly recommended and elective controls may be enabled", `DisableControl` because "it is not a caller's to turn off either: what removes it is the target ceasing to be governed". The crate even factors the question into one helper, `is_mandatory_control`, documented as "The one place that question is asked of a STORED enablement, so every call that treats a control the service applied differently from one a caller enabled reads the same answer".

`ResetEnabledControl` never asks it. It resolves the enablement's control back to its catalog entry, but only to test `implemented_with_service_control_policy()`, and then re-deploys whatever it found. For the three MANDATORY controls the catalog publishes with `ControlImplementation::ConfigRule` (`AWS-GR_CLOUDTRAIL_ENABLED`, `AWS-GR_CLOUDTRAIL_VALIDATION_ENABLED`, `AWS-GR_CONFIG_ENABLED`) the SCP refusal does not fire, so a caller can reset a control it was refused permission to enable and is refused permission to disable: the service-applied enablement is flipped to `UNDER_CHANGE`, its drift assessment is withdrawn to `UNKNOWN` in all three dimensions, and a `RESET_ENABLED_CONTROL` record is appended against a control the caller never enabled. (The other eight mandatory entries are SCP-backed and are accidentally covered by the SCP refusal, which is why the gap is a narrow residue rather than the whole mandatory set.)

## Evidence

`EnableControl` refuses a mandatory control — `src/ops/enable_control.rs:231-246`:
```
        // Only part of the catalog may be enabled by a caller: the mandatory controls AWS
        // Control Tower applies to every governed account itself are named by a
        // `controlIdentifier` but are not a caller's to enable.
        if !control.enablable {
            return Err(EnableControlErrorModeled::ValidationException(
                ValidationException {
                    message: format!(
                        "control {} is a mandatory control, which AWS Control Tower applies \
                         itself; only strongly recommended and elective controls may be \
                         enabled, with the exception of the Region deny control",
                        control.name
```

`DisableControl` applies the same check to a STORED enablement — `src/ops/disable_control.rs:351-371`:
```
        // A MANDATORY control is one AWS Control Tower applies to every target it governs
        // itself rather than one a caller enabled, so it is not a caller's to turn off
        // either: what removes it is the target ceasing to be governed. Read off the
        // catalog entry this enablement's control resolves to — the same property
        // `EnableControl` refuses a mandatory `controlIdentifier` by — so a request naming
        // one is refused rather than tearing down a control the service applied.
        if let Some(mandatory) = crate::state::resolve_control(&enabled.control_identifier)
            .filter(|control| !control.enablable)
        {
            return Err(DisableControlErrorModeled::ValidationException(
```

The shared helper exists and says it is the one place the question is asked — `src/state.rs:3307-3318`:
```
/// Whether `control_identifier` names a MANDATORY control: one AWS Control Tower applies
/// to every target it governs itself, which is exactly the set no caller may enable
/// ([`CatalogControl::enablable`]).
///
/// The one place that question is asked of a STORED enablement, so every call that treats
/// a control the service applied differently from one a caller enabled reads the same
/// answer.
pub fn is_mandatory_control(control_identifier: &str) -> bool {
    resolve_control(control_identifier).is_some_and(|control| !control.enablable)
}
```

`ResetEnabledControl` resolves the same catalog entry but tests only the implementation, never `enablable` — `src/ops/reset_enabled_control.rs:188-211`:
```
        // The documented restriction on this operation: a reset "does not work for
        // controls implemented with SCPs". ...
        if let Some(catalog) = crate::state::resolve_control(&control.control_identifier)
            .filter(|catalog| catalog.implemented_with_service_control_policy())
        {
            return Err(ResetEnabledControlErrorModeled::ValidationException(
```
and then starts the re-deployment unconditionally at `src/ops/reset_enabled_control.rs:236`: `let Some(operation_identifier) = store.record_enabled_control_reset(&control.arn, now)`.

The residue is non-empty in the catalog — `src/state.rs:3123-3149` publishes mandatory entries with `enablable: false` and `implementation: ControlImplementation::ConfigRule`:
```
    CatalogControl {
        name: "AWS-GR_CLOUDTRAIL_ENABLED",
        catalog_id: "1ysc3nmpx8rvg0kdwt2fqhj7zm",
        enablable: false,
        implementation: ControlImplementation::ConfigRule,
```

Test coverage: the mandatory rule is pinned for enable and disable ONLY — `tests/all/governed_target_mandatory_controls.rs:455-463`: "RULE: a MANDATORY control is one AWS Control Tower applies to every target it governs itself, so it is not a caller's to enable OR to disable: every such request is refused with 400 `ValidationException`, whichever way it names the enablement" — and its request table (`:488-518`) spans `/enable-control` and three spellings of `/disable-control`, with no `/reset-enabled-control` row. No test pins a reset of a service-applied control either way.

## Steps to Reproduce

1. POST /enable-baseline with the governance baseline (`establishes_governance: true`, `AWSControlTowerBaseline`) on an organizational unit target, so the service applies every mandatory control to it.
2. Wait past OPERATION_SETTLE_AFTER (500 ms), then POST /list-enabled-controls with that `targetIdentifier` and pick the entry whose `controlIdentifier` ends in `control/AWS-GR_CLOUDTRAIL_ENABLED` — a mandatory, ConfigRule-implemented control. Note its `arn`.
3. POST /disable-control with `{"enabledControlIdentifier": "<that arn>"}` — refused 400 ValidationException, "control AWS-GR_CLOUDTRAIL_ENABLED is a mandatory control, which AWS Control Tower applies itself".
4. POST /reset-enabled-control with the SAME `{"enabledControlIdentifier": "<that arn>"}` — answered 200 with a fresh `operationIdentifier`.
5. POST /get-enabled-control for that arn: the control AWS Control Tower applied itself now reports `statusSummary.status = "UNDER_CHANGE"` and `UNKNOWN` in all three drift dimensions, and /get-control-operation resolves a `RESET_ENABLED_CONTROL` record against a control the caller was never allowed to enable or disable.
