Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 3c3c87ac9166712e1ad44c6d25ef542a1ff1b247

# UpdateEnabledControl never asks whether the enablement is a mandatory control the service applied itself, the check both its siblings make

_Filed: 2026-10-29T14:00:55Z_
_Operation: UpdateEnabledControl | Class: validation_

> _Note: this issue was identified against the code base at commit `3c3c87ac9166712e1ad44c6d25ef542a1ff1b247`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`EnableControl` and `DisableControl` both refuse a request that names a MANDATORY catalog control (`CatalogControl::enablable == false`) — the controls AWS Control Tower applies to a governed target itself through `apply_governance_controls`, which land in `enabled_controls` with ARNs `ListEnabledControls` hands callers. `UpdateEnabledControl` resolves the same catalog entry (it needs it for the parameter check) but never looks at `enablable`, so a request to reconfigure a control the service applied passes the mandatory gate entirely.

Today the request is still refused, but only INCIDENTALLY and for the wrong reason: a mandatory catalog entry declares no parameters, so a non-empty `parameters` list trips the undeclared-key refusal and an empty list trips the "must differ from the configured parameters" refusal (`same_control_configuration(&[], &[])` is true). The caller is therefore told "parameters is the configuration enabled control <arn> already carries; supply parameters that differ from the configured ones" instead of being told the control is not a caller's to configure — and the moment any mandatory entry of `CONTROL_CATALOG` declares a parameter (the catalog already publishes a `parameters` list on one entry), the same path would ACCEPT a caller's reconfiguration of a control AWS Control Tower applies itself and park it on the stored enablement. The crate has one helper for exactly this question and this handler is the one call on an enablement that does not reach it.

## Evidence

The rule, stated once in this crate and declared to bind every such call — services/service-com-amazonaws-controltower-awscontroltowerapis/src/state.rs:3307-3318:
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
Sibling 1 — src/ops/enable_control.rs:234-246:
```
        if !control.enablable {
            return Err(EnableControlErrorModeled::ValidationException(
                ValidationException {
                    message: format!(
                        "control {} is a mandatory control, which AWS Control Tower applies \
                         itself; only strongly recommended and elective controls may be \
                         enabled, ...
```
Sibling 2 — src/ops/disable_control.rs:351-371:
```
        // A MANDATORY control is one AWS Control Tower applies to every target it governs
        // itself rather than one a caller enabled, so it is not a caller's to turn off
        // either: what removes it is the target ceasing to be governed. Read off the
        // catalog entry this enablement's control resolves to — the same property
        // `EnableControl` refuses a mandatory `controlIdentifier` by ...
        if let Some(mandatory) = crate::state::resolve_control(&enabled.control_identifier)
            .filter(|control| !control.enablable)
```
This handler — src/ops/update_enabled_control.rs:216-239 resolves the very same catalog entry and checks only the parameter list; `enablable` appears nowhere in the file:
```
        let Some(catalogued) = crate::state::resolve_control(&control.control_identifier) else {
...
        if let Some(message) =
            crate::state::unacceptable_control_parameters_message(catalogued, &requested)
```
and the only thing that ends up refusing the call is the unrelated same-configuration rule, src/ops/update_enabled_control.rs:282-295:
```
        if control.status == crate::state::EnablementStatus::Succeeded
            && crate::state::same_control_configuration(&control.parameters, &requested)
        {
            return Err(UpdateEnabledControlErrorModeled::ValidationException(
                ValidationException {
                    message: format!(
                        "parameters is the configuration enabled control {} already carries; ...
```
These enablements really exist in the store — src/state.rs:2156 (`apply_governance_controls`):
```
        for control in CONTROL_CATALOG.iter().filter(|control| !control.enablable) {
```
Test coverage: tests/all/governed_target_mandatory_controls.rs:455-458 states the rule only for enable and disable ("so it is not a caller's to enable OR to disable: every such request is refused with 400 `ValidationException`, whichever way it names the enablement") and its case list (lines 488-518) exercises `/enable-control` and `/disable-control` only — no test pins `UpdateEnabledControl`'s present refusal reason for a mandatory control.

## Steps to Reproduce

1. POST /enable-baseline with the governance baseline (`AWSControlTowerBaseline`, `baseline/17BSJV3IGJ2QSGA2`) on an organizational unit ARN, so `apply_governance_controls` writes an EnabledControl for every mandatory catalog entry on that target.
2. Sleep past the settle span, then POST /list-enabled-controls with that `targetIdentifier` and note the `arn` reported for a mandatory control (e.g. `AWS-GR_CLOUDTRAIL_ENABLED`).
3. POST /update-enabled-control with `{"enabledControlIdentifier": "<that arn>", "parameters": []}`.
4. Observe 400 ValidationException whose message is "parameters is the configuration enabled control ... already carries" — the handler never says, as EnableControl and DisableControl do for the same control, that a mandatory control is not a caller's to configure; nothing in the handler consults `CatalogControl::enablable`/`is_mandatory_control`.
