Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: d0b7c7f06678596bb74c7aa80db04e65e20adb2f

# The tagging/policy plane resolves a foreign-account ARN against the store instead of applying the crate's account-boundary gate

_Filed: 2026-11-01T12:33:34Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `d0b7c7f06678596bb74c7aa80db04e65e20adb2f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TagResource` locks the store and looks `input.resource_arn` up in `billing_views` with no account check, so a well-formed ARN whose embedded account id belongs to another account is answered with `ResourceNotFoundException` (404) rather than the modeled `AccessDeniedException` (403) the operation declares. The same omission is in the two readers beside it (`ListTagsForResource`, `GetResourcePolicy`), which also resolve the ARN against `billing_views` directly.

This contradicts the rule stated on the helper the rest of the crate uses: `state::is_caller_account_arn`'s doc says EVERY operation that identifies a billing view by ARN checks it before the lookup, and `GetBillingView`, `UpdateBillingView`, `DeleteBillingView`, `DisassociateSourceViews` and `ListBillingViews` all do exactly that as their first act. The tagging plane identifies a billing view by ARN (it looks the ARN up in `billing_views` and reports `resourceType: "billingview"` in its refusals) and skips the gate, so the account boundary of this service is enforced on some ARN-addressed operations and not others.

## Evidence

The rule, stated on the helper — services/service-com-amazonaws-billing-awsbilling/src/state.rs:996-1003:
```
/// Whether `arn` is scoped to the caller's own account.
///
/// Every operation that identifies a billing view by ARN checks this BEFORE it
/// looks the ARN up, so the answer to a foreign-account ARN never depends on
/// whether a view exists there.
pub fn is_caller_account_arn(arn: &str) -> bool {
    arn_account_id(arn) == Some(caller_account_id())
}
```
A sibling that obeys it — services/service-com-amazonaws-billing-awsbilling/src/ops/delete_billing_view.rs:158-171:
```
        // The account boundary comes first: a well-formed ARN outside the
        // caller's own account is refused before the store is consulted, so the
        // answer never depends on what another account holds.
        if !crate::state::is_caller_account_arn(&input.arn) {
            return Err(DeleteBillingViewErrorModeled::AccessDeniedException(
                crate::state::cross_account_access_denied(
```
The handler under review, which never calls it (`grep is_caller_account_arn` over the crate lists only update/get/disassociate/delete/list_billing_views) — services/service-com-amazonaws-billing-awsbilling/src/ops/tag_resource.rs:168-203:
```
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(...)?;
        let mut state = state.lock();
...
        let Some(view) = state.billing_views.get_mut(&input.resource_arn) else {
            return Err(TagResourceErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: crate::state::RESOURCE_NOT_FOUND_MESSAGE.to_string(),
                    resource_id: input.resource_arn.clone(),
                    resource_type: crate::state::BILLING_VIEW_RESOURCE_TYPE.to_string(),
```
The model gives the operation the 403 it never answers — OPERATIONS.md:163: "**Errors:** `AccessDeniedException (HTTP 403)`, `InternalServerException (HTTP 500)`, `ResourceNotFoundException (HTTP 404)`, …".
Test coverage: services/service-com-amazonaws-billing-awsbilling/tests/all/rejection_fidelity.rs:334-341 claims to table "Every implemented operation that identifies a billing view by ARN" but lists only GetBillingView/UpdateBillingView/DeleteBillingView/ListBillingViews, so the tagging plane's boundary is untested.

## Steps to Reproduce

1. Boot the emulator and point an awsJson1_0 `billing` client at it.
2. Call GetBillingView(arn="arn:aws:billing::111122223333:billingview/primary") — observe 403 AccessDeniedException (pinned by tests/all/rejection_fidelity.rs:343-357).
3. Call TagResource(resourceArn="arn:aws:billing::111122223333:billingview/primary", resourceTags=[{key:"team", value:"finance"}]) with the same foreign ARN.
4. Observe 404 ResourceNotFoundException instead of the modeled 403 AccessDeniedException; ListTagsForResource and GetResourcePolicy answer the same way for the same ARN.
