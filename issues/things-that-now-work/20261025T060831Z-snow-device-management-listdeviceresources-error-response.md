Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 30ecb1282bd7333a88bd61f51b7f1fc433035291

# ListDeviceResources resolves the device AFTER the nextToken, so an unknown managedDeviceId plus a bad token answers 400 instead of the 404 the handler promises

_Filed: 2026-10-25T06:08:31Z_
_Operation: ListDeviceResources | Class: error-response_

> _Note: this issue was identified against the code base at commit `30ecb1282bd7333a88bd61f51b7f1fc433035291`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListDeviceResources`'s handler states that the addressed device id decides the answer ahead of everything else the request carries, but the code does not do that: the `nextToken` decode (which returns the modelled `ValidationException`, HTTP 400) runs BEFORE the device lookup that produces the modelled `ResourceNotFoundException` (HTTP 404). A request addressed to a `managedDeviceId` that names no device of the account in the signed Region, carrying a `nextToken` this service never issued, is therefore answered 400 `ValidationException` about the token — i.e. the caller is told its page cursor is wrong rather than that the device it addressed does not exist, and the refusal class now depends on which page was asked of a device that is not there. The crate's own comment (and the suite's own comment) say the opposite: the id refusal is the answer "whatever page or filter was asked of it".

The sibling `ListExecutions` is ordered the same way (token decode at `src/ops/list_executions.rs:180-195`, task 404 at `:198-203`) but makes no such claim in its comments, so the contradicted statement is the one quoted below.

## Evidence

Handler claim, `services/service-com-amazonaws-snowdevicemanagement-snowdevicemanagement/src/ops/list_device_resources.rs:193-195`:

```
        // The addressed device is RESOLVED first: an id that names no device of this
        // account in the signed Region is the modelled `ResourceNotFoundException`,
        // whatever page or filter was asked of it.
        let Some(device) = store.device_in(&region, &input.managed_device_id) else {
```

Code that runs BEFORE it, same file `:175-190`:

```
        let resume_at = match input.next_token.as_deref() {
            None => None,
            Some(token) => match crate::state::decode_token(token) {
                Some(key) => Some(key),
                None => {
                    return Err(ListDeviceResourcesErrorModeled::ValidationException(
                        ValidationException {
                            message: format!(
                                "nextToken {token:?} is not a token returned by ListDeviceResources"
                            ),
```

The same claim in the suite that pins the 404, `tests/all/device_resources.rs:506-507`: "// The table rows: the shape of the unknown id × what was asked of it. The id is / // resolved ahead of the filter and the page, so every row is the same refusal." — and `:513-518` shows the rows only vary `type` and `maxResults` (`(None, None), (Some(EC2_INSTANCE_TYPE), None), (Some("AWS::S3::Bucket"), None), (None, Some(1))`), never a `nextToken`, so the combination is uncovered; `tests/all/device_resources.rs:559-590` (`a_token_this_service_never_issued_is_refused`) sends unissued tokens only to devices that DO exist. Spec side: `ListDeviceResources` declares both `ResourceNotFoundException (HTTP 404)` and `ValidationException (HTTP 400)` (OPERATIONS.md:78), so either code is renderable and only the ordering decides.

## Steps to Reproduce

1. Start the emulator with a managed-device inventory activated in the signed Region (the one state seam that populates `devices`), so the account/region gate lifts.
2. Send `GET /managed-device/smd-0000000000000099/resources?nextToken=zz` — an id that names no device of the account in that Region, plus a token this service never issued.
3. Observe HTTP 400 `ValidationException` naming `nextToken`.
4. Expected per the handler's own comment at `src/ops/list_device_resources.rs:193-195` and the suite comment at `tests/all/device_resources.rs:506-507`: HTTP 404 `ResourceNotFoundException` for the unknown device, "whatever page or filter was asked of it".
5. Compare with `GET /managed-device/smd-0000000000000099/resources?maxResults=1` (no token), which does answer 404 — so the refusal class changes with the page parameter.
