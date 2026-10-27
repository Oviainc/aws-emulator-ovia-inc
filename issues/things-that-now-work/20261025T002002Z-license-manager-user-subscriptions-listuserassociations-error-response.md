Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 9d4ea5e78ce612465906f9a67fc5f9d068061c14

# The instance-authorization refusal is not hoisted for ListUserAssociations, contradicting its own handler doc and a stale justification in service.rs

_Filed: 2026-10-25T00:20:02Z_
_Operation: ListUserAssociations | Class: error-response_

> _Note: this issue was identified against the code base at commit `9d4ea5e78ce612465906f9a67fc5f9d068061c14`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListUserAssociations` runs the same instance-authorization predicate `AssociateUser` runs, and its handler comment claims the refusal is given "whatever else it says — exactly as `AssociateUser` is refused identically however else the request is wrong". That parity does not hold: the pre-validation hoist in `crate::service::EmulatedService::override_operation` matches ONLY the `AssociateUser` URI, so for `ListUserAssociations` the framework's deserialize -> validate -> handle walk refuses a malformed request before the handler's gate can run. A `ListUserAssociations` call naming an instance the account's fleet does not hold AND omitting the `@required` `IdentityProvider` answers `ValidationException`, while the equivalent `AssociateUser` call answers `UnauthorizedException` — the exact asymmetry the `AssociateUser` suite was written to rule out.

The reason the hoist was not extended is a comment in `service.rs` that is no longer true: it states that `DisassociateUser` "answers the scaffold's 501 for every request, so there is nothing yet for a gate in front of it to be consistent with". `DisassociateUser` is fully implemented (it resolves and tears down a stored association), and `ListUserAssociations` — the other operation that takes an `InstanceId` and gates on it — is not mentioned at all. The helper the hoist needs already exists in this crate and already reads `InstanceId` straight off the raw JSON body (`authorizes_user_association`), which is the member `ListUserAssociations` also declares `@required`, so nothing new has to be invented.

## Evidence

The parity claim — `src/ops/list_user_associations.rs:169-177`:
```
// THE AUTHORIZATION GATE, asked before anything else about the request is examined,
// with the same predicate `AssociateUser` and the pre-handler gate in
// `crate::service` ask: an instance the account's EC2 fleet does not hold cannot be
// shown to be the caller's, so the request fails closed whatever else it says —
// exactly as `AssociateUser` is refused identically however else the request is wrong.
if !state.authorizes_instance(&input.instance_id) {
    return Err(crate::scaffolded::Scaffolded::unauthorized());
}
```

The hoist that does not cover it — `src/service.rs:155-163`:
```
// Only `AssociateUser` is gated. `DisassociateUser` answers the scaffold's 501 for
// every request, so there is nothing yet for a gate in front of it to be
// consistent with; its own issue owns it.
if req.method() == "POST" && request_path(req.uri()) == ASSOCIATE_USER_URI {
```
and the constant it matches, `src/service.rs:51`: `const ASSOCIATE_USER_URI: &str = "/user/AssociateUser";`

The justification is stale — `src/ops/disassociate_user.rs:185-208` is a real implementation, not a 501:
```
let arn = state
    .resolve_instance_user(...)
    .ok_or_else(crate::scaffolded::Scaffolded::unauthorized)?
...
Ok(DisassociateUserOutput {
    instance_user_summary: crate::state::instance_user_summary(&record),
})
```

The helper already present — `src/service.rs:111-120`:
```
let body: serde_json::Value = serde_json::from_slice(req.body().bytes()?).ok()?;
let instance_id = body.get("InstanceId")?.as_str()?;
...
let authorized = state.lock().authorizes_instance(instance_id);
```

Test coverage: `tests/all/refusals.rs:773-826` (`associate_user_is_authorized_before_its_shape_is_validated`) pins the hoisted behaviour for `AssociateUser` only — "an unauthorized call is refused identically whatever else the request gets wrong" (file header :23-24). No suite asserts the same for `ListUserAssociations`, which is why the drift went unnoticed.

## Steps to Reproduce

1. POST /user/AssociateUser with `{"Username": "u", "InstanceId": "i-00000000000000000", "IdentityProvider": {...}, "Tags": {<51 entries>}}` — refused with `UnauthorizedException` (HTTP 400), because the hoisted gate ran before validation.
2. POST /user/ListUserAssociations with `{"InstanceId": "i-00000000000000000"}` (the same unheld instance, `IdentityProvider` omitted although it is `@required`) — refused with `ValidationException`, not the `UnauthorizedException` the handler comment says this request gets "whatever else it says".
3. Read `src/service.rs:155-157`, which justifies gating only `AssociateUser` by claiming `DisassociateUser` still answers 501, and compare with `src/ops/disassociate_user.rs:155-209`, which implements the operation.
