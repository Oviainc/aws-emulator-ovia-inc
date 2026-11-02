Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ca72e8d5fc2000c02a01dcf678a8fed8074b0f2e

# CreateBillingView persists sourceViews entries that name no stored view, the exact association AssociateSourceViews refuses with 404

_Filed: 2026-11-02T06:28:36Z_
_Operation: CreateBillingView | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ca72e8d5fc2000c02a01dcf678a8fed8074b0f2e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateBillingView` copies `input.source_views` straight into the persisted `BillingView` record without ever consulting `billing_views`, so a view is created and stored drawing from ARNs that no billing view exists under. Its sibling writer of the same field, `AssociateSourceViews`, refuses precisely that input with the modeled `ResourceNotFoundException` (404), stating the reason in a comment: an association to a view that is not held "would point at nothing".

The result is a store the crate's own reads contradict each other over: `ListSourceViewsForBillingView` on the created view happily lists the dangling ARN (it projects `view.source_views` verbatim), while `GetBillingView` and `ListSourceViewsForBillingView` on that very ARN answer 404 `ResourceNotFoundException`, and `AssociateSourceViews` would refuse adding the identical source to the identical view. `GetBillingView` also reports a `sourceViewCount` derived from `source_views.len()`, so the count advertises sources that resolve to nothing.

This is the same defect class as the already-filed forced-delete dangling-association issue (`20261101T123334Z-billing-deletebillingview-wrong-behavior`), but on the create path, where no delete is involved: the dangling association is written by the request itself. `CreateBillingView` declares `ResourceNotFoundException` and `sourceViews` is its only ARN-bearing input member, so the handler never answers a modeled error whose documented trigger it is the only possible source of.

## Evidence

The handler stores the member unchecked — services/service-com-amazonaws-billing-awsbilling/src/ops/create_billing_view.rs:251-278 (nothing between the lock at :183 and the insert at :278 reads `billing_views` except the idempotency replay and the resident-count ceiling):
```
        let arn = state.mint_view_arn();
        let view = crate::state::BillingView {
            arn: arn.clone(),
            name: input.name,
            description: input.description,
            billing_view_type: crate::state::BILLING_VIEW_TYPE_CUSTOM.to_string(),
            owner_account_id: ::engine::emulator_account_id().to_string(),
            source_account_id: None,
            source_views: input.source_views,
...
        state.billing_views.insert(arn.clone(), view);
```
The sibling writer of the same field, which refuses exactly this — services/service-com-amazonaws-billing-awsbilling/src/ops/associate_source_views.rs:241-247:
```
        // Every source must name a view this account holds, or the association
        // would point at nothing.
        for source in &input.source_views {
            if !state.billing_views.contains_key(source) {
                return Err(not_found(source, "sourceViews"));
            }
        }
```
with services/service-com-amazonaws-billing-awsbilling/src/ops/associate_source_views.rs:155-162:
```
fn not_found(arn: &str, member: &str) -> AssociateSourceViewsError {
    AssociateSourceViewsErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
        message: format!("{member}: billing view {arn} does not exist"),
```
The modeled error the create path never answers, from its own generated contract header — services/service-com-amazonaws-billing-awsbilling/src/ops/create_billing_view.rs:89: "//! * `ResourceNotFoundException` (HTTP 404) — The specified ARN in the request doesn't exist." (same list in OPERATIONS.md:39), and the only ARN-bearing input member is services/service-com-amazonaws-billing-awsbilling/src/ops/create_billing_view.rs:66-67: "//! * `sourceViews` (BillingViewSourceViewsList, len 1..10) **required** — A list of billing views used as the data source for the custom billing view."
The handler's own pre-amble claims nothing is left to check — services/service-com-amazonaws-billing-awsbilling/src/ops/create_billing_view.rs:174-177: "// Every modeled constraint (`name` length/pattern, `description`, `sourceViews` 1..10, `resourceTags` 0..200, the `clientToken` pattern) is refused by the boundary validator before this runs, so reaching here means the input satisfies all of them." — but the boundary validator only checks the list's length and each member's ARN pattern, never existence.
The read that then contradicts the other reads — services/service-com-amazonaws-billing-awsbilling/src/ops/list_source_views_for_billing_view.rs:246-251 projects `view.source_views` verbatim (`source_views: page.to_vec()`), while services/service-com-amazonaws-billing-awsbilling/src/ops/get_billing_view.rs:182-188 answers `ResourceNotFoundException` for that same ARN, and services/service-com-amazonaws-billing-awsbilling/src/ops/get_billing_view.rs:202-203 derives `sourceViewCount` from `view.source_views.len()`.
Test coverage: no test sends a non-existent source view to `CreateBillingView` *as a negative case*; instead the existing harnesses rely on the current acceptance — services/service-com-amazonaws-billing-awsbilling/tests/all/billing_views.rs:140-142 builds `long_source_views` as `arn:aws:billing::{ACCOUNT_ID}:billingview/source-{i}` (ARNs no view is stored under) and the row at :170-179 asserts 200, and services/service-com-amazonaws-billing-awsbilling/tests/all/source_view_disassociation.rs:51 seeds views with `src-{i}` ARNs the same way. The stated reason for those rows is only the COUNT they vary ("how many source views are associated (1 through the modeled maximum of 10)", billing_views.rs:134-138), not the existence of the sources, so the harnesses would need to name real views — which is what `AssociateSourceViews`' own tests already do.

## Steps to Reproduce

1. Boot the emulator with the caller recorded as the organization management account, as tests/all/billing_views.rs:57-64 does (`emulator_with_engine()`), since CreateBillingView refuses otherwise.
2. Call CreateBillingView(name="dangling", sourceViews=["arn:aws:billing::000000000000:billingview/does-not-exist"]) — observe 200 and a minted ARN, with no ResourceNotFoundException.
3. Call ListSourceViewsForBillingView(arn=<minted ARN>) — observe 200 listing "arn:aws:billing::000000000000:billingview/does-not-exist" as a source of the stored view, and GetBillingView(arn=<minted ARN>) reporting sourceViewCount 1.
4. Call GetBillingView(arn="arn:aws:billing::000000000000:billingview/does-not-exist") — observe 404 ResourceNotFoundException: the source the record names resolves to nothing.
5. Call AssociateSourceViews(arn=<minted ARN>, sourceViews=["arn:aws:billing::000000000000:billingview/does-not-exist"]) — observe 404 ResourceNotFoundException ("sourceViews: billing view … does not exist") for the very association step 2 accepted and persisted.
