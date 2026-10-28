Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: eba09938c5757f116b88f59710a766eb7f22de4f

# DisableControl's control/target path skips the organizational-unit check its siblings apply to targetIdentifier

_Filed: 2026-10-28T16:14:15Z_
_Operation: DisableControl | Class: validation_

> _Note: this issue was identified against the code base at commit `eba09938c5757f116b88f59710a766eb7f22de4f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When `DisableControl` is called with the `controlIdentifier`/`targetIdentifier` pair instead of `enabledControlIdentifier`, the handler validates `controlIdentifier` exactly the way `EnableControl` does (a control ARN naming no catalog control gets 404, anything that is not a control reference gets a 400 ValidationException) but applies NO check at all to `targetIdentifier`: the value is passed straight to `enabled_control_on_target`, where it is compared as text against stored targets. A `targetIdentifier` that could never be a control target — a landing zone ARN, an S3 bucket ARN, an Organizations ROOT or ACCOUNT ARN, anything satisfying the member's `Arn` pattern — therefore comes back as `ResourceNotFoundException` (404, "control X is not enabled on target Y") rather than the `ValidationException` (400) the crate answers for the very same malformed value on every other path that treats `targetIdentifier` as a reference.

The crate already owns this check as a single shared helper, `state::non_organizational_unit_target_message`, whose doc calls itself "The one place that refusal's wording is built"; `EnableControl` and `EnableBaseline` both call it before touching the store. `DisableControl` is the only handler that resolves an enablement through a `targetIdentifier` and does not. The result is that one request can be answered as "your input is malformed" or "that resource does not exist" purely according to which member the caller used to name the same enablement, which is the distinction this handler's own comment says it is making for `controlIdentifier` ("the same reading `EnableControl` gives this member").

## Evidence

The pair path takes `targetIdentifier` unchecked and goes straight to the store — `src/ops/disable_control.rs:259-276` and `:312-323`:
```
            None => {
                let (Some(control_identifier), Some(target_identifier)) =
                    (&input.control_identifier, &input.target_identifier)
                else {
...
                let Some(enabled) = store.enabled_control_on_target(control, target_identifier)
                else {
                    return Err(DisableControlErrorModeled::ResourceNotFoundException(
                        ResourceNotFoundException {
                            message: format!(
                                "control {control_identifier} is not enabled on target \
                                 {target_identifier}"
                            ),
```
The same handler consciously mirrors `EnableControl` for the OTHER member of the pair — `src/ops/disable_control.rs:278-281`:
```
                // A control ARN that names no catalog control is a reference to a resource
                // that does not exist; anything else is not a control reference at all,
                // which is a malformed input rather than a missing resource — the same
                // reading `EnableControl` gives this member.
```
The sibling that creates the enablement DOES check the target — `src/ops/enable_control.rs:248-255`:
```
        if let Some(message) =
            crate::state::non_organizational_unit_target_message(&input.target_identifier)
        {
            return Err(
                EnableControlErrorModeled::ValidationException(ValidationException { message })
                    .into(),
            );
        }
```
and so does `EnableBaseline` — `src/ops/enable_baseline.rs:230-237` calls the same helper.

The helper the disable path skips, `src/state.rs:3061-3074`:
```
/// Why `target` cannot have a baseline enabled on it, or `None` when it is an
/// organizational unit ARN.
///
/// The one place that refusal's wording is built. The message is built from the request
/// and from the rule, so it tells the caller what kind of target it may name.
pub fn non_organizational_unit_target_message(target: &str) -> Option<String> {
    if is_organizational_unit_arn(target) {
        return None;
    }
    Some(format!(
        "targetIdentifier {target:?} is not the ARN of an organizational unit; only \
         organizational units are supported as targets, named as \
         arn:aws:organizations::<account>:ou/<organization>/<organizational unit>"
    ))
}
```

Test coverage: `tests/all/disable_control.rs:573-625` is the table of handler-refused malformed requests, and every row's `targetIdentifier` is a well-formed OU ARN (`ou("d001")`, `first_target`, `second_target`); the 404 table at `:627-697` likewise only uses OU targets. No test pins the answer for a non-OU `targetIdentifier` in the pair path, so nothing in the crate justifies the divergence.

## Steps to Reproduce

1. POST /enable-control with a catalogued enablable control and an OU `targetIdentifier`; wait past the settle span.
2. POST /disable-control with `{"controlIdentifier": "<the control ARN>", "targetIdentifier": "arn:aws:controltower:us-west-2:123456789012:landingzone/ABCDEFGHIJKLMNOP"}` — a value that satisfies the member's length/pattern but is not an organizational unit ARN.
3. Observe HTTP 404 `ResourceNotFoundException` ("control ... is not enabled on target ...").
4. Send the same `targetIdentifier` to POST /enable-control (or /enable-baseline) and observe HTTP 400 `ValidationException` from `non_organizational_unit_target_message` for the identical value.
