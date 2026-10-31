Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: a66b2a2f28102a010a545393b9f4e71052de9805

# DisableControl classifies an unpublished controlIdentifier two ways: 404 on the pair path, 400 when it accompanies enabledControlIdentifier

_Filed: 2026-10-28T22:55:48Z_
_Operation: DisableControl | Class: error-response_

> _Note: this issue was identified against the code base at commit `a66b2a2f28102a010a545393b9f4e71052de9805`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DisableControl` reads `controlIdentifier` on two paths, and the handler states ONE rule for that member: a control ARN that names no catalogued control is a reference to a resource that does not exist (404 `ResourceNotFoundException`), while a value that is not a control reference at all is a malformed input (400 `ValidationException`). The pair path (`controlIdentifier` + `targetIdentifier`) implements exactly that split. The cross-check path — the one taken when `enabledControlIdentifier` is also sent — collapses both cases into one 400 `ValidationException` whose message claims a MISMATCH, so a control ARN of the right shape naming no catalogued control (e.g. `arn:aws:controltower:us-west-2::control/AWS-GR_NO_SUCH_CONTROL`) is answered 400 "...which controlIdentifier ... does not name" instead of the 404 the same handler gives the same value two branches further down. The two readings of one member cannot both be right, and the handler's own comment says which one is.

The redundant `named.is_none()` disjunct is what merges the two cases: the stored `control_identifier` always resolves (`EnableControl` refuses an unpublished control before persisting), so `named != resolve_control(&enabled.control_identifier)` already covers every unresolvable value, and the dedicated "names no catalog control" refusal the pair path builds is never reached from this path.

## Evidence

The rule, stated in this handler for this member — services/service-com-amazonaws-controltower-awscontroltowerapis/src/ops/disable_control.rs:278-294:

```
// A control ARN that names no catalog control is a reference to a resource
// that does not exist; anything else is not a control reference at all,
// which is a malformed input rather than a missing resource — the same
// reading `EnableControl` gives this member.
let control = match crate::state::resolve_control(control_identifier) {
    Some(control) => control,
    None if crate::state::is_control_arn(control_identifier) => {
        return Err(DisableControlErrorModeled::ResourceNotFoundException(
            ResourceNotFoundException {
                message: format!(
                    "no control matches controlIdentifier \
                     {control_identifier}"
```

The other path in the same handler, breaking it — disable_control.rs:220-236:

```
if let Some(control_identifier) = &input.control_identifier {
    let named = crate::state::resolve_control(control_identifier);
    if named.is_none()
        || named != crate::state::resolve_control(&enabled.control_identifier)
    {
        return Err(DisableControlErrorModeled::ValidationException(
            ValidationException {
                message: format!(
                    "enabledControlIdentifier {identifier} is the \
                     enablement of control {}, which controlIdentifier \
                     {control_identifier} does not name",
```

The sibling handler the comment appeals to gives the same split as the pair path — src/ops/enable_control.rs:199-228 (`None if crate::state::is_control_arn(&input.control_identifier) => ... ResourceNotFoundException` / `None => ... ValidationException`).

Test coverage: `tests/all/disable_control.rs` pins the pair path's 404 for exactly this value class — line 684-689, `("a pair naming a control ARN the catalog does not publish", json!({ "controlIdentifier": control("AWS-GR_NO_SUCH_CONTROL"), "targetIdentifier": first_target }))` in the `missing` table asserted at line 705 as `404, "ResourceNotFoundException"`. The `malformed` (400) table covers only `("an ARN with a controlIdentifier that is no control reference at all", ...)` at line 597-603, i.e. a NON-reference; no test pins the cross-check path for a control ARN naming no catalogued control, so the divergent 400 is unpinned.

## Steps to Reproduce

1. POST /enable-control with `controlIdentifier` = `arn:aws:controltower:us-west-2::control/AWS-GR_ENCRYPTED_VOLUMES` and `targetIdentifier` = `arn:aws:organizations::123456789012:ou/o-exampleorg/ou-9988-8x`; keep the returned `arn` (the enabled-control ARN).
2. Wait past the settle span so the enablement is SUCCEEDED (otherwise the call answers 409).
3. POST /disable-control with `enabledControlIdentifier` = that ARN and `controlIdentifier` = `arn:aws:controltower:us-west-2::control/AWS-GR_NO_SUCH_CONTROL` (a well-formed control ARN the catalog does not publish). Observed: 400 ValidationException, message "...is the enablement of control AWS-GR_ENCRYPTED_VOLUMES, which controlIdentifier ... does not name".
4. POST /disable-control with the same `controlIdentifier` but WITHOUT `enabledControlIdentifier`, plus `targetIdentifier` = the OU above. Observed: 404 ResourceNotFoundException, message "no control matches controlIdentifier ..." — the classification the handler's own comment states for that member.
