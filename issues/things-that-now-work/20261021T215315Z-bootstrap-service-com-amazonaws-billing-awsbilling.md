Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 9fd9433686ca60725b6b64638320351edab7ca3b

# Bootstrap the AWS Billing (awsbilling) emulator

_Filed: 2026-10-21T21:53:15Z_
_Operation: service-com-amazonaws-billing-awsbilling | Class: stub_

> _Note: this issue was identified against the code base at commit `9fd9433686ca60725b6b64638320351edab7ca3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 23 handlers in `src/ops/` is an unmodified scaffold answering HTTP 501 `NotImplemented` and holding no state, so no client workflow works at all. Implement a FIRST, SMALL slice: the stateful lifecycle of the service's primary resource, the **billing view** (`arn:aws:billing::<account>:billingview/<id>`), over the awsJson1_0 wire (`POST /` with `X-Amz-Target: AWSBilling.<OperationName>`).

Happy path to implement, in order (5 operations):

1. **CreateBillingView** — input `name` (required, 1..128, `^[ a-zA-Z0-9_\+=\.\-@]+$`), `sourceViews` (required, list of 1..10 billing view ARNs), optional `description`, `dataFilterExpression`, `resourceTags`, and `clientToken` (header `X-Amzn-Client-Token`, @idempotencyToken). Must mint a NEW ARN matching the modeled billing-view ARN pattern, persist the record (name, description, sourceViews, dataFilterExpression, tags, createdAt=now), and return `arn` + `createdAt`. Repeating the call with the same `clientToken` must return the SAME arn and create no second view (@idempotent).
2. **GetBillingView** — input `arn` (required). Must look the record up in state and return `billingView` (BillingViewElement) populated FROM the stored record: `arn`, `name`, `description`, `billingViewType` (`CUSTOM` for views created through CreateBillingView), `ownerAccountId` (the 12-digit caller account), `dataFilterExpression`, `createdAt`, `updatedAt`, `sourceViewCount` (= stored sourceViews.len()), `derivedViewCount`, `healthStatus` (`HEALTHY`). Unknown/never-created ARN must be `ResourceNotFoundException` (HTTP 404), not 200 and not 501.
3. **ListBillingViews** — must return `billingViews` as a list of BillingViewListElement built from every persisted view (`arn`, `name`, `description`, `ownerAccountId`, `billingViewType`, `healthStatus`). Honour at minimum the `arns` filter (0..10) and `billingViewTypes`; honour `maxResults` (1..100) by truncating and emitting an opaque `nextToken` that the next call resumes from (the op is @paginated inputToken=nextToken/outputToken=nextToken/items=billingViews). A newly created view MUST appear here.
4. **UpdateBillingView** — input `arn` (required) plus optional `name`, `description`, `dataFilterExpression`. Must mutate the persisted record in place (only the members present in the request), set `updatedAt=now`, and return `arn` + `updatedAt`. A following GetBillingView must show the new values. Unknown arn → `ResourceNotFoundException` (404).
5. **DeleteBillingView** — input `arn` (required) and `force` (boolean, default false). Must remove the record from state and return `arn`. A following GetBillingView must fail with `ResourceNotFoundException` (404) and the view must be gone from ListBillingViews. Deleting an already-deleted arn → `ResourceNotFoundException`/`ValidationException` as modeled, never a hollow 200.

Every one of these five must compute its response from the persisted state described below. No canned bodies, no echoing the request back, and no minting an ARN that is never stored — a create followed by a get/list must return exactly what was created, and a delete must be observable by the next caller.

Shared requirements for the slice:
- Validate the modeled constraints before mutating: ARN pattern, `name` length/pattern, `description` length/pattern, `sourceViews` length 1..10, `maxResults` range 1..100 → `ValidationException` (HTTP 400) in the awsJson1_0 error shape (`__type` = `ValidationException`), not a 500 or a panic.
- Errors must serialize with the modeled HTTP statuses: ValidationException 400, ResourceNotFoundException 404, ConflictException 409, AccessDeniedException 403, ThrottlingException 429, InternalServerException 500.

## State Model

In `ComAmazonawsBillingAwsbillingState` (`src/state.rs`), reached only via `crate::state::service_state(&*self.engine)` (never a fresh engine):

- `billing_views: BTreeMap<String /* billing view ARN */, BillingView>` — ordered by ARN so ListBillingViews/pagination are deterministic.
- `struct BillingView { arn: String, name: String, description: Option<String>, billing_view_type: BillingViewType /* CUSTOM | PRIMARY | BILLING_GROUP | CUSTOM_SOURCE */, owner_account_id: String /* 12 digits */, source_account_id: Option<String>, source_views: Vec<String>, data_filter_expression: Option<Expression>, created_at: SystemTime, updated_at: Option<SystemTime>, view_definition_last_updated_at: Option<SystemTime>, health_status: BillingViewHealthStatus /* HEALTHY */, tags: BTreeMap<String, String> }`
  - `source_view_count` is derived as `source_views.len()`; `derived_view_count` is derived by counting views whose `source_views` contain this ARN — neither is stored as a stale copy.
- `idempotency_tokens: HashMap<String /* clientToken */, String /* arn */>` — so a replayed CreateBillingView returns the first ARN instead of creating a duplicate.
- `next_view_id: u64` (or a UUID source) used ONLY to build the ARN that is then stored in `billing_views`.

Out of scope for this first slice (they stay 501 and are filed as individual gap issues later): AssociateSourceViews/DisassociateSourceViews, ListSourceViewsForBillingView, ListBillingViewSegments, Tag/Untag/ListTagsForResource, GetResourcePolicy, Get/UpdateBillingPreferences, GetCredits, GetCreditAllocationHistory, RedeemCredits, and all the Business/Enterprise support charge operations.

Done = a realistic client script (AWS SDK/CLI against the emulator) runs create → get → list → update → get → delete → get-fails end to end with correct, stateful response bodies — not merely 2xx.

## Evidence

`services/service-com-amazonaws-billing-awsbilling/OPERATIONS.md` lists all 23 operations with the input/output members cited above: CreateBillingView (required `name`, `sourceViews`; output `arn`, `createdAt`; @idempotent with `clientToken` @idempotencyToken), GetBillingView (@readonly, required `arn`, output `billingView` BillingViewElement, declares ResourceNotFoundException 404), ListBillingViews (@paginated inputToken=nextToken/outputToken=nextToken/items=billingViews, `maxResults` range 1..100, `arns` 0..10), UpdateBillingView (required `arn`, output `arn`+`updatedAt`), DeleteBillingView (required `arn`, `force` default false, output `arn`). Field lists for BillingViewElement / BillingViewListElement / ResourceTag come from `smithy_query shape` on the model at `services/service-com-amazonaws-billing-awsbilling/smithy-model/com-amazonaws-billing-awsbilling.json`. Wire protocol awsJson1_0 with `X-Amz-Target: AWSBilling.<Op>` is from OPERATIONS.md's "Wire protocol" section. All 23 files under `src/ops/` still carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker (grep over that directory matches every operation file, e.g. `create_billing_view.rs:4`, `get_billing_view.rs:4`, `list_billing_views.rs:4`, `update_billing_view.rs:4`, `delete_billing_view.rs:4`), so the service holds zero state and answers 501 everywhere; no tests assert any of this behavior.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI Billing client at it.
2. CreateBillingView with name="demo-view", sourceViews=["arn:aws:billing::123456789012:billingview/primary"], description="first view", clientToken="tok-1" — expect 200 with a billing-view `arn` and `createdAt`; today: HTTP 501 NotImplemented.
3. Repeat the identical CreateBillingView with clientToken="tok-1" — expect the SAME `arn` returned and no duplicate view.
4. GetBillingView with the returned arn — expect `billingView.name` == "demo-view", `description` == "first view", `sourceViewCount` == 1, `billingViewType` == CUSTOM, `createdAt` equal to the create response.
5. ListBillingViews with no filters — expect the new view present in `billingViews` with matching arn/name; then call with maxResults=1 and follow `nextToken` and expect each view exactly once.
6. UpdateBillingView with the arn and name="demo-view-2" — expect 200 with `arn` + `updatedAt`; GetBillingView must then report name "demo-view-2" and the new `updatedAt`.
7. GetBillingView with arn "arn:aws:billing::123456789012:billingview/does-not-exist" — expect ResourceNotFoundException (HTTP 404).
8. CreateBillingView with name="bad/name!" or sourceViews=[] — expect ValidationException (HTTP 400) and no view created.
9. DeleteBillingView with the arn — expect 200 echoing `arn`; then GetBillingView on it must return ResourceNotFoundException (404) and ListBillingViews must no longer include it.
