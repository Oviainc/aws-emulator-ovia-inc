Issue-Severity: HIGH
Issue-Gap-Class: event-propagation
Issue-Commit: 043b7f8056dcef57c0c9baaf75abb2dd363a8bd0

# CreateBillingView's management-account gate is unsatisfiable over the wire: nothing in src/ ever records an organization management account

_Filed: 2026-11-01T14:10:54Z_
_Operation: CreateBillingView | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `043b7f8056dcef57c0c9baaf75abb2dd363a8bd0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateBillingView refuses every request whose caller is not the organization management account, and the standing it judges by is the store field `organization_management_account_id`, which `Default` sets to `None` and which NO code in `src/` ever assigns. The only assignments in the repository are in this crate's own test harness, which reaches into the Rust struct directly.

For any client that speaks only HTTP, the gate therefore answers 400 `ValidationException` to EVERY CreateBillingView request, whatever the body. Because CreateBillingView is the only writer of `billing_views` (apart from the seeded `PRIMARY` view), the whole primary-resource lifecycle this crate's state module says it implements is unreachable from outside the process: no CUSTOM view can be created, so UpdateBillingView, DeleteBillingView, AssociateSourceViews, DisassociateSourceViews, ListSourceViewsForBillingView, TagResource, ListTagsForResource and GetResourcePolicy can only ever answer their refusal arms, and a 200 CreateBillingView response (modelled: `arn` + `createdAt`) is unobservable.

The crate already has the mechanism to learn a cross-service fact: `state::caller_identity_arn` reads the STS service's own store (`use service_com_amazonaws_sts_...::state as sts_state`) to build the cross-account refusal. The organization fact is never read from the emulated Organizations service the field's own doc points at.

## Evidence

The gate — services/service-com-amazonaws-billing-awsbilling/src/ops/create_billing_view.rs:192-203:
```
        let caller = crate::state::caller_account_id();
        if !state.is_organization_management_account(caller) {
            return Err(CreateBillingViewErrorModeled::ValidationException(ValidationException {
                message: format!(
                    "Account {caller} is not the management account of an organization. \
                     Only the management account can create a billing view."
                ),
```
The field it reads, and its own admission of the gap — services/service-com-amazonaws-billing-awsbilling/src/state.rs:551-559:
```
    /// `None` by default: an organization is established through AWS
    /// Organizations, which this emulator models in its own service and whose
    /// state holds no organization until one is created — so the emulated
    /// account is a standalone account and is nobody's organization management
    /// account. ... Set this to the caller's account id to
    /// model the management account of an organization.
    pub organization_management_account_id: Option<String>,
```
The default — services/service-com-amazonaws-billing-awsbilling/src/state.rs:621: `organization_management_account_id: None,`; the reader — state.rs:851-853: `self.organization_management_account_id.as_deref() == Some(account_id)`.

`grep -rn organization_management_account_id services/service-com-amazonaws-billing-awsbilling` returns NO assignment in `src/` other than the `None` default; the only writers are the test harnesses: tests/all/billing_views.rs:62 (`.organization_management_account_id = Some(ACCOUNT_ID.to_string());`, whose comment says "this harness models the caller as the management account of an organization ... the world in which the billing view lifecycle these tests are about is reachable at all") and tests/all/rejection_fidelity.rs:463. So every behavioral test of the lifecycle bypasses the wire, and nothing asserts the gate is satisfiable from outside the process.

The crate's own precedent for reading another service's store — services/service-com-amazonaws-billing-awsbilling/src/state.rs:30 and :1042-1052:
```
use service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_state;
...
    sts_state::service_state(engine)
        ... sts_state::resolve_caller(
```
The model makes a successful create a modelled outcome — OPERATIONS.md:38: "**Output:** `arn` (…, required), `createdAt` (timestamp)".

## Steps to Reproduce

1. Boot the emulator (no test harness state mutation) and point an awsJson1_0 `billing` client at it.
2. Optionally call the emulated Organizations service's CreateOrganization so the caller is an organization management account.
3. Call CreateBillingView with a valid body, e.g. {"name":"my-view","sourceViews":["arn:aws:billing::000000000000:billingview/primary"]}.
4. Observe 400 ValidationException "Account 000000000000 is not the management account of an organization..." — `organization_management_account_id` is still `None`, so no body can ever succeed.
5. Call ListBillingViews: only the seeded PRIMARY view exists, and every CUSTOM-view operation (Update/Delete/Associate/Disassociate/Tag/ListTags/GetResourcePolicy) can only answer its refusal arm.
