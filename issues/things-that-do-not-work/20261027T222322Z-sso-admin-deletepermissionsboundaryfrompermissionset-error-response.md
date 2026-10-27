Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 65fd8797851bb5c5937d1ea0792f5c718c73df47

# DeletePermissionsBoundaryFromPermissionSet answers 404 for an absent boundary, breaking the "the delete answers what the get answers" rule its three sibling deletes state

_Filed: 2026-10-27T22:23:22Z_
_Operation: DeletePermissionsBoundaryFromPermissionSet | Class: error-response_

> _Note: this issue was identified against the code base at commit `65fd8797851bb5c5937d1ea0792f5c718c73df47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states ONE rule for deleting a singleton child of a resource: the answer for a child that is not there is the answer the child's own GET gives. Three delete handlers say so explicitly and follow it. `DeletePermissionsBoundaryFromPermissionSet` is the fourth, and it breaks the rule.

- `DeleteApplicationAccessScope` / `DeleteApplicationAuthenticationMethod`: the GET of an absent scope/method is `ResourceNotFoundException`, so the delete is `ResourceNotFoundException` — stated in the handler as "the answer the get of that scope gives".
- `DeleteInlinePolicyFromPermissionSet`: the GET of an absent inline policy is a plain 200 with the optional output member absent, so the handler reasons that "this permission set holds no inline policy" is a STATE the service reports rather than a missing resource, and makes the delete IDEMPOTENT (empty 200).
- `DeletePermissionsBoundaryFromPermissionSet`: the GET of an absent boundary is ALSO a plain 200 with the optional output member absent — `get_permissions_boundary_for_permission_set.rs` says so in as many words — yet this delete answers 404 `ResourceNotFoundException`.

The reason given for the 404 lives in `state.rs` and is factually false inside this crate: it claims the refusal is "the same answer an absent singleton child earns elsewhere in this service", when the other singleton child of the very same resource (the inline policy) earns an idempotent 200 instead, on exactly the premise ("optional output member on the reader") that holds for the boundary too.

Consequence: a caller writing an unconditional cleanup over a permission set — drop the inline policy, drop the boundary — succeeds on the first and is refused 404 on the second, and a retried `DeletePermissionsBoundaryFromPermissionSet` after a successful one fails where the parallel inline-policy retry succeeds. One of the two handlers is wrong; they cannot both implement the crate's stated rule.

## Evidence

The rule, stated twice in this crate:

`src/ops/delete_inline_policy_from_permission_set.rs:215-225`:
```
// The inline policy is a PROPERTY of the permission set, not a resource of its
// own: its reader (`GetInlinePolicyForPermissionSet`) models `InlinePolicy` as an
// OPTIONAL output member and answers a plain 200 with it absent for a permission
// set carrying none, so "this permission set holds no inline policy" is a state
// the service reports rather than a resource that is missing. Dropping the stored
// document is therefore the whole delete, and it is IDEMPOTENT: the permission set
// carries no inline policy afterwards whether or not it carried one before, so a
// repeated delete answers the same empty 200.
...
record.inline_policy = None;
```

`src/ops/delete_application_access_scope.rs:218-222`:
```
// ... so a `Scope` this application
// carries no access scope for (never put, or already deleted) earns the modelled
// `ResourceNotFoundException` — the answer the get of that scope gives — rather
// than an empty 200 that would tell the caller a scope was deleted when none was.
```
(the same sentence appears at `src/ops/delete_application_authentication_method.rs:217-221`: "earns the modelled `ResourceNotFoundException` — the answer the get of that type gives")

The premise holds for the boundary, by this crate's own reader handler — `src/ops/get_permissions_boundary_for_permission_set.rs:213-220`:
```
// `PermissionsBoundary` is an optional output member: a permission set with no
// boundary attached is reported with it absent.
Ok(GetPermissionsBoundaryForPermissionSetOutput {
    permissions_boundary: record
        .permissions_boundary
        .as_ref()
        .map(crate::state::PermissionsBoundaryRecord::to_shape),
})
```
which is the SAME shape of answer as `src/ops/get_inline_policy_for_permission_set.rs:218-222`:
```
// `InlinePolicy` is an optional output member: a permission set with no inline
// policy attached is reported with it absent.
```

The rule is nonetheless broken — `src/ops/delete_permissions_boundary_from_permission_set.rs:218-237`:
```
// The BOUNDARY is the resource of this call: a permission set carrying none —
// never attached, or already deleted — is NOT a successful delete, so it earns the
// modelled `ResourceNotFoundException` rather than an empty 200 ...
if record.permissions_boundary.take().is_none() {
    return Err(
        DeletePermissionsBoundaryFromPermissionSetErrorModeled::ResourceNotFoundException(
```

And the justification for it asserts a crate-wide consistency that does not exist — `src/state.rs:1767-1778`:
```
/// The refusal a delete of a permissions boundary a permission set does not carry earns, as
/// the message of the modelled `ResourceNotFoundException` (HTTP 404).
///
/// The BOUNDARY is the resource of that call ("Deletes the permissions boundary from a
/// specified PermissionSet"), so a permission set carrying none has nothing for the request
/// to name — the same answer an absent singleton child earns elsewhere in this service.
```
"the same answer an absent singleton child earns elsewhere in this service" is contradicted by `delete_inline_policy_from_permission_set.rs:215-225` quoted above.

Test coverage: `tests/all/permissions_boundary_delete.rs:10-13` pins the 404 ("a call that names no boundary this permission set carries (never attached, or already deleted) is refused with the modelled `ResourceNotFoundException` rather than answered with an empty success") but states no reason of its own — the only reason offered anywhere is the `state.rs:1772` sentence that the crate itself falsifies. `tests/all/inline_policy_delete.rs` covers the opposite, idempotent answer for the sibling child.

## Steps to Reproduce

1. POST / with `X-Amz-Target: SWBExternalService.ListInstances` and read the account's `InstanceArn`; the permission-set surface is withdrawn on an account instance, so enable it with `UpdateInstance` `{"InstanceArn":"<instance>","PermissionSetsEnabled":true}`.
2. POST / with `X-Amz-Target: SWBExternalService.CreatePermissionSet` and `{"InstanceArn":"<instance>","Name":"Boundaries"}`; keep the minted `PermissionSetArn`. Attach neither an inline policy nor a permissions boundary.
3. POST / with `X-Amz-Target: SWBExternalService.GetInlinePolicyForPermissionSet` and `{"InstanceArn":"<instance>","PermissionSetArn":"<ps>"}` — 200, `InlinePolicy` absent. Then `GetPermissionsBoundaryForPermissionSet` with the same body — also 200, `PermissionsBoundary` absent. The two readers answer the same way.
4. POST / with `X-Amz-Target: SWBExternalService.DeleteInlinePolicyFromPermissionSet` and the same body — 200 with an empty body (idempotent, per the handler's own reasoning).
5. POST / with `X-Amz-Target: SWBExternalService.DeletePermissionsBoundaryFromPermissionSet` and the same body — 404 `ResourceNotFoundException` "Could not find permissions boundary for permission set <ps>.", although its reader answered exactly as the inline policy's reader did.
6. Repeat with a boundary attached: `PutPermissionsBoundaryToPermissionSet` then two `DeletePermissionsBoundaryFromPermissionSet` calls — the first is 200, the second 404, where the parallel inline-policy delete is 200 both times.
