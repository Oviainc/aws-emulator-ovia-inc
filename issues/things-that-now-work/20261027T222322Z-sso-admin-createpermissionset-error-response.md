Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 65fd8797851bb5c5937d1ea0792f5c718c73df47

# An InstanceArn this account holds no instance for earns two different Message texts, although the crate holds one helper for "the refusal" and states that one ARN must read the same whichever operation a client calls

_Filed: 2026-10-27T22:23:22Z_
_Operation: CreatePermissionSet | Class: error-response_

> _Note: this issue was identified against the code base at commit `65fd8797851bb5c5937d1ea0792f5c718c73df47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::instance_not_found_message` is documented as THE refusal text for one condition — "an `InstanceArn` this account holds no instance for" — and 12 handlers use it, on both the `AccessDeniedException` and the `ResourceNotFoundException` channel. The other 37 handlers hand-roll a DIFFERENT sentence for the identical condition, so the same unknown instance ARN reads two ways depending on which operation a client called.

The divergence is not a channel distinction: `CreateApplication` and `CreatePermissionSet` both answer the modelled `ResourceNotFoundException` for an `InstanceArn` nothing is held under, and they answer it with different text. `TagResource`, `UntagResource`, `ListTagsForResource`, `ListApplications` and `ListApplicationAssignmentsForPrincipal` likewise use the helper on the `ResourceNotFoundException` channel, while every operation of the permission-set surface (and `UpdateInstance`, `CreateTrustedTokenIssuer`, `ListTrustedTokenIssuers`, and the three ABAC operations) inlines the other sentence.

This contradicts the rule the crate states for exactly this refusal in `add_region.rs`: the missing instance is reported "under the same message, so one ARN that resolves to nothing reads the same whichever of them a client calls". A client that matches on the detail text — the only member these error shapes carry — sees one of two sentences depending on the operation, and a future change to the helper reaches only 12 of the 49 sites.

## Evidence

The crate holds ONE helper, documented as the refusal for the condition — `src/state.rs:245-253`:
```
/// The refusal an `InstanceArn` this account holds no instance for earns, as the message
/// of the modelled `AccessDeniedException` — the only not-found channel
/// `DescribeInstance`/`DeleteInstance` declare ...
///
/// Built from the request: it names the ARN that resolved to nothing.
pub fn instance_not_found_message(instance_arn: &str) -> String {
    format!("No IAM Identity Center instance {instance_arn} exists in this account")
}
```

The rule that one ARN must read the same everywhere — `src/ops/add_region.rs:154-168`:
```
/// The modelled refusal for an `InstanceArn` this account holds no instance for.
///
/// This operation's model declares no `ResourceNotFoundException`, so a missing instance
/// is reported on the channel it does declare — the `AccessDeniedException` (HTTP 403)
/// `DescribeInstance`/`DeleteInstance` answer for the same ARN, under the same message, so
/// one ARN that resolves to nothing reads the same whichever of them a client calls.
fn instance_not_found(instance_arn: &str) -> AddRegionError {
    AddRegionErrorModeled::AccessDeniedException(AccessDeniedException {
        message: Some(crate::state::instance_not_found_message(instance_arn)),
```

Both sides of the inconsistency, on the SAME error channel and the SAME condition:

`src/ops/create_application.rs:229-239` (uses the helper):
```
// The instance the application will live under must exist.
let instance = store.instance(&input.instance_arn).ok_or_else(|| {
    CreateApplicationErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
        message: Some(crate::state::instance_not_found_message(
            &input.instance_arn,
        )),
```

`src/ops/create_permission_set.rs:207-219` (hand-rolls another sentence):
```
// The instance the permission set will live under must exist: a syntactically
// valid ARN for an instance that does not is a missing resource, not a bad
// request.
let instance_arn = store
    .instance(&input.instance_arn)
    ...
            CreatePermissionSetErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(format!(
                        "No instance found with the ARN {}",
                        input.instance_arn
                    )),
```

`grep -n "No instance found with the ARN" src/ops` reports 37 handlers (attach/detach managed policy and customer-managed reference, every `Delete*`/`Describe*`/`Get*`/`Put*`/`List*` of the permission-set and account-assignment surface, `UpdateInstance:304`, `UpdatePermissionSet:223`, `CreateAccountAssignment:237`, `CreateTrustedTokenIssuer:218`, `ListTrustedTokenIssuers:215`, and the three `*InstanceAccessControlAttributeConfiguration` handlers); `grep -n instance_not_found_message src/ops` reports the other 12.

Test coverage: `tests/all/validation_message_wording.rs` pins this service's refusal wording, and `tests/all/discovered_instance_surface.rs` / `tests/all/account_instance_restriction.rs` exercise unknown-instance refusals across the surface — none asserts that the two texts must differ.

## Steps to Reproduce

1. Start the emulator. Pick a pattern-valid `InstanceArn` nothing is held under, e.g. `arn:aws:sso:::instance/ssoins-1111111111111111`.
2. POST / with `X-Amz-Target: SWBExternalService.CreateApplication` and body `{"InstanceArn":"arn:aws:sso:::instance/ssoins-1111111111111111","ApplicationProviderArn":"arn:aws:sso::aws:applicationProvider/custom","Name":"A"}` — 404 `ResourceNotFoundException` whose `Message` is "No IAM Identity Center instance arn:aws:sso:::instance/ssoins-1111111111111111 exists in this account".
3. POST / with `X-Amz-Target: SWBExternalService.CreatePermissionSet` and body `{"InstanceArn":"arn:aws:sso:::instance/ssoins-1111111111111111","Name":"A"}` — 404 `ResourceNotFoundException` whose `Message` is "No instance found with the ARN arn:aws:sso:::instance/ssoins-1111111111111111".
4. Compare with `ListApplications` (helper text) and `ListPermissionSets` (inline text) for the same ARN: the same condition on the same channel reads two different ways.
5. POST / with `X-Amz-Target: SWBExternalService.AddRegion` and `{"InstanceArn":"arn:aws:sso:::instance/ssoins-1111111111111111","RegionName":"us-west-2"}` — 403 `AccessDeniedException` with the helper text, the text `add_region.rs` says every operation must agree on.
