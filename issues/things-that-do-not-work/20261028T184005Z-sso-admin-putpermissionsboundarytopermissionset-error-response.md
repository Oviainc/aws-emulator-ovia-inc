Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: af40c6e1c3301a14f83f727add020b300628421a

# The PermissionsBoundary choice refusal is a fixed constant that always says "but not both", although its own doc says it is BUILT from the members the request got wrong

_Filed: 2026-10-28T18:40:05Z_
_Operation: PutPermissionsBoundaryToPermissionSet | Class: error-response_

> _Note: this issue was identified against the code base at commit `af40c6e1c3301a14f83f727add020b300628421a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutPermissionsBoundaryToPermissionSet` refuses two different malformed bodies through ONE fixed string: a `PermissionsBoundary` that names BOTH `CustomerManagedPolicyReference` and `ManagedPolicyArn`, and one that names NEITHER. `PermissionsBoundaryRecord::from_shape` collapses both into `None` (`_ => None`), and the handler maps that single `None` onto `crate::state::PERMISSIONS_BOUNDARY_CHOICE_MESSAGE`.

The documentation that sits directly above that constant states a rule the constant breaks: it says the message is "BUILT from the members the request got wrong" and that "the caller has to fix the request from this text alone". The item under that sentence is a `pub const` — nothing about it is built from the request — and its text ends with "but not both", which is a false statement about the request for the NEITHER case: a caller that sent `{"PermissionsBoundary": {}}` (both members are optional, so the boundary validator passes it) is told it named both policy kinds and must drop one, when in fact it named none and must add one.

This is the only conditional refusal of this crate that does not name the member the request got wrong. Every sibling conditional refusal — the ones for the same class of rule the model cannot express — is a FUNCTION built from the request (`grant_type_mismatch_message`, `grant_member_required_message`, `authentication_method_type_mismatch_message`, `provisioning_status_unknown_message`, `duplicate_attribute_key_message`), each one naming the member and the value that broke the rule. The boundary refusal is written as a constant instead, so the two cases it serves cannot be told apart from the body.

No existing test pins the message text: `tests/all/permissions_boundary.rs` asserts the status and the error code only, so distinguishing the two cases leaves every existing test unchanged.

## Evidence

The doc claim and the constant it sits above — `services/service-com-amazonaws-ssoadmin-swbexternalservice/src/state.rs:1785-1793`:
```
/// The refusal a `PermissionsBoundary` that names both policy kinds, or neither, earns —
/// as the message of the modelled `ValidationException` (HTTP 400).
///
/// BUILT from the members the request got wrong, which is what the shape's documented
/// choice ("Specify either `CustomerManagedPolicyReference` … or `ManagedPolicyArn`") is
/// about; the caller has to fix the request from this text alone.
pub const PERMISSIONS_BOUNDARY_CHOICE_MESSAGE: &str =
    "Specify either CustomerManagedPolicyReference or ManagedPolicyArn in \
     PermissionsBoundary, but not both.";
```
It is a `const`, so nothing is built from the request, and the single text is used for BOTH cases because the resolution collapses them — `src/state.rs:1738-1757`:
```
pub fn from_shape(boundary: &PermissionsBoundary) -> Option<Self> {
    match (
        &boundary.customer_managed_policy_reference,
        &boundary.managed_policy_arn,
    ) {
        (Some(reference), None) => { ... }
        (None, Some(managed_policy_arn)) => ...
        _ => None,
    }
}
```
and the handler answers that one `None` with that one string — `src/ops/put_permissions_boundary_to_permission_set.rs:157-173`:
```
// The CHOICE `PermissionsBoundary` documents, checked FIRST: it is a property of
// the request body alone ...
let boundary = crate::state::PermissionsBoundaryRecord::from_shape(
    &input.permissions_boundary,
)
.ok_or_else(|| {
    PutPermissionsBoundaryToPermissionSetErrorModeled::ValidationException(
        ValidationException {
            message: Some(crate::state::PERMISSIONS_BOUNDARY_CHOICE_MESSAGE.to_string()),
```
The sibling rule-of-the-same-class refusals in this same crate ARE built from the request — `src/state.rs:942-948`:
```
/// together. Built from the request: it names both halves, since the caller has to learn
/// which of the two to change.
pub fn grant_type_mismatch_message(declared: &str, carried: &str) -> String {
    format!(
        "Grant carries a {carried} grant, which does not match the GrantType {declared} \
         the request declares"
    )
}
```
and `src/state.rs:961-963`:
```
pub fn grant_member_required_message(member: &str, grant_type: &str) -> String {
    format!("{member} is required when the grant type is {grant_type}.")
}
```
The NEITHER case is reachable and is exercised today — `tests/all/permissions_boundary.rs:461-465`:
```
        RefusedBoundary {
            label: "neither kind",
            sent: json!({}),
        },
```
and that test asserts only the status and the code, never the text — `tests/all/permissions_boundary.rs:479-489`:
```
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400, got {status} {body}",
            case.label
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {:?}: expected ValidationException, got {body}",
            case.label
        );
```
For reference, the model sentence the crate is implementing (`smithy_query(query="doc",shape="PermissionsBoundary")`): "Specify either `CustomerManagedPolicyReference` to use the name and path of a customer managed policy, or `ManagedPolicyArn` to use the ARN of an Amazon Web Services managed policy." — it states the choice, not the "but not both" half alone.

## Steps to Reproduce

1. POST / with `X-Amz-Target: SWBExternalService.ListInstances` and read the account's `InstanceArn`; enable the permission-set surface on it with `SWBExternalService.UpdateInstance` `{"InstanceArn":"<instance>","PermissionSetsEnabled":true}`.
2. POST / with `X-Amz-Target: SWBExternalService.CreatePermissionSet` and `{"InstanceArn":"<instance>","Name":"Boundaries"}`; keep the minted `PermissionSetArn`.
3. POST / with `X-Amz-Target: SWBExternalService.PutPermissionsBoundaryToPermissionSet` and `{"InstanceArn":"<instance>","PermissionSetArn":"<ps>","PermissionsBoundary":{"ManagedPolicyArn":"arn:aws:iam::aws:policy/ReadOnlyAccess","CustomerManagedPolicyReference":{"Name":"Both"}}}` — 400 ValidationException, Message "Specify either CustomerManagedPolicyReference or ManagedPolicyArn in PermissionsBoundary, but not both." (correct for this request).
4. POST the same target with `{"InstanceArn":"<instance>","PermissionSetArn":"<ps>","PermissionsBoundary":{}}` — 400 ValidationException with the IDENTICAL Message, telling a caller that named NEITHER member that it must not name both, and naming no member for it to add.
5. Compare with the crate's sibling conditional refusal: POST `SWBExternalService.PutApplicationGrant` with a `GrantType`/`Grant` mismatch, or with `AuthorizationCodeGrant` missing `RedirectUris` — those messages name the member and value the request got wrong, which is what `state.rs:1788-1790` claims this one does too.
