Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the AWS Pricing Plan Manager emulator

_Filed: 2026-10-21T21:28:14Z_
_Operation: service-com-amazonaws-pricingplanmanager-awspricingplanmanager | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 9 operations in `src/ops/` is an unmodified scaffold carrying `//! SCAFFOLDED, NOT IMPLEMENTED:` and answering HTTP 501, so no client can do anything with this service. Implement a FIRST, SMALL, stateful slice: the subscription lifecycle of the service's single primary resource (`Subscription`). Wire protocol is `restJson1`, every route is `POST /v1/<OperationName>`, the subscription travels as the `@httpPayload` member `subscription`, and the concurrency token travels as the `ETag` response header / `If-Match` request header.

### Happy path to implement (in order)

1. **CreateSubscription** — `POST /v1/CreateSubscription`. Input: `planFamily` (required), `planTier` (required), `usageLevel` (optional), `resourceArns` (required, 1..10), `approvalMode` (`IMMEDIATE`|`MANUAL`), `clientToken` (idempotency token). Must mint a new `arn` (e.g. `arn:aws:pricingplanmanager:<region>:<account>:subscription/<uuid>`), persist the record, set `createdAt`/`updatedAt` to now (ISO-8601 date-time), set `status` = `ACTIVE` for `IMMEDIATE` (the default when unset should be treated as immediate activation) or `PENDING_APPROVAL` for `MANUAL`, and return the stored subscription as the payload plus a freshly computed `eTag` in the `ETag` header. Repeating the same `clientToken` must return the SAME subscription, not a second one.
2. **GetSubscription** — `POST /v1/GetSubscription`. Input: `arn` (required). Must look the record up in state and return exactly what was stored (same arn, planFamily, planTier, usageLevel, status, resourceArns, createdAt, updatedAt) with the current `eTag` header. Unknown arn → `ResourceNotFoundException` (HTTP 404). Read-only: must not mutate state.
3. **UpdateSubscription** — `POST /v1/UpdateSubscription`. Input: `arn` (required), `planTier` (required), `usageLevel` (optional), `ifMatch` header (required), `clientToken`. Must verify `ifMatch` against the stored eTag and return `ConflictException` (HTTP 409) on mismatch; on success mutate the stored `planTier`/`usageLevel`, bump `updatedAt`, mint a NEW eTag, and return the updated record. A following GetSubscription must observe the new tier and the new eTag.
4. **ListSubscriptions** — `POST /v1/ListSubscriptions`. Input: `nextToken`. Output: `subscriptionSummaries` (required) + `nextToken`. Must enumerate the subscriptions actually in state as `SubscriptionSummary` (arn, planFamily, planTier, usageLevel, scheduledChange, status, statusReason, resourceArns, createdAt, updatedAt, and the per-record `eTag` which is a BODY member here, not a header). Paging must be real: a bounded page size with an opaque `nextToken` that resumes where the previous page stopped, and the token omitted on the last page. A deleted/cancelled-and-gone subscription must not appear.
5. **CancelSubscription** — `POST /v1/CancelSubscription`. Input: `arn` (required), `ifMatch` header (required), `clientToken`. Must enforce `If-Match` the same way, then record a `scheduledChange` of `changeType = CANCELLATION` with an `effectiveDate`, bump `updatedAt`, mint a new eTag, and return the record. A following GetSubscription must show that `scheduledChange`.
6. **ApprovePaidSubscription** — `POST /v1/ApprovePaidSubscription` (only needed to close the `MANUAL` branch of step 1). Input: `arn`, `ifMatch` header, `clientToken`. Must move a `PENDING_APPROVAL` record to `ACTIVE`, bump `updatedAt` and the eTag; approving a record that is already `ACTIVE` → `ConflictException` (409).

No canned or echoed responses: every one of these must compute its response from the persisted store. In particular the `arn`, `createdAt`/`updatedAt` timestamps, `status` and `eTag` must come from the stored record, not be synthesized per-request, and a create followed by a get/list must return what was created. Minimum validation to include: missing required members and `resourceArns` outside 1..10 → `ValidationException` (HTTP 400); `arn` length outside 1..2048 → `ValidationException`; unknown `arn` → `ResourceNotFoundException` (404); `ifMatch` not equal to the current eTag → `ConflictException` (409).

## State Model

- `PricingPlanManagerState` (service-global, behind the usual lock):
  - `subscriptions: BTreeMap<String /* subscription arn */, SubscriptionRecord>` — BTreeMap so ListSubscriptions has a stable, resumable order for its `nextToken`.
  - `idempotency: HashMap<String /* clientToken */, String /* arn */>` — makes `@idempotencyToken` replays return the original subscription.
- `SubscriptionRecord`:
  - `arn: String`
  - `plan_family: String`
  - `plan_tier: String`
  - `usage_level: Option<String>`
  - `status: Status` (`PENDING_APPROVAL` | `ACTIVE` | `SYNC_IN_PROGRESS` | `FAILED`)
  - `status_reason: Option<String>`
  - `resource_arns: Vec<String>` (1..10)
  - `scheduled_change: Option<ScheduledChangeRecord>`
  - `created_at: DateTime`, `updated_at: DateTime`
  - `etag: String` — regenerated on every mutation; echoed in the `ETag` header for single-resource ops and in `SubscriptionSummary.eTag` for ListSubscriptions.
- `ScheduledChangeRecord`: `change_type: ScheduledChangeType` (`DOWNGRADE` | `CANCELLATION`), `effective_date: Option<DateTime>`, `plan_tier: Option<String>`, `usage_level: Option<String>`.

The remaining operations (`AssociateResourcesToSubscription`, `DisassociateResourcesFromSubscription`, `CancelSubscriptionChange`) stay at 501 for this slice and will be filed as individual gap issues later — once `SubscriptionRecord` above exists they are small follow-ups over the same store.

## Evidence

`services/service-com-amazonaws-pricingplanmanager-awspricingplanmanager/OPERATIONS.md` lists all 9 operations with routes, input/output members and errors (restJson1, sigv4 name `pricingplanmanager`; `subscription` is `@httpPayload`, `eTag` is `@httpHeader(ETag)`, `ifMatch` is `@httpHeader(If-Match)`; ListSubscriptions is `@paginated(inputToken=nextToken, outputToken=nextToken, items=subscriptionSummaries)` and `@readonly`; GetSubscription is `@readonly`). `grep -rn 'SCAFFOLDED, NOT IMPLEMENTED' src/ops/` matches all 9 handler files (approve_paid_subscription.rs, associate_resources_to_subscription.rs, cancel_subscription.rs, cancel_subscription_change.rs, create_subscription.rs, disassociate_resources_from_subscription.rs, get_subscription.rs, list_subscriptions.rs, update_subscription.rs), each returning `Scaffolded::not_implemented(..)` → HTTP 501. Shape fields and enums are from the Smithy model: `Subscription` (arn, planFamily, planTier, usageLevel, scheduledChange, status, statusReason, resourceArns, createdAt, updatedAt), `SubscriptionSummary` (same plus required `eTag` body member), `Status` = PENDING_APPROVAL|ACTIVE|SYNC_IN_PROGRESS|FAILED, `ApprovalMode` = MANUAL|IMMEDIATE, `ScheduledChange` (changeType, effectiveDate, planTier, usageLevel). Model docs state PENDING_APPROVAL means "created with manual approval and is waiting for an ApprovePaidSubscription call" and that `SubscriptionSummary.eTag` is "the entity tag for concurrency control. Pass this value in the If-Match header when making changes" — grounding the eTag/If-Match handling above. No tests exist for this crate.

## Steps to Reproduce

1. Start the emulator and point an SDK/curl client at the pricingplanmanager endpoint.
2. POST /v1/CreateSubscription with {"planFamily":"CloudFront","planTier":"PRO","resourceArns":["arn:aws:cloudfront::111122223333:distribution/E1"],"approvalMode":"IMMEDIATE","clientToken":"tok-1"} — expect 200, a minted subscription arn, status ACTIVE, createdAt/updatedAt set, and an ETag response header. Today: HTTP 501 NotImplemented.
3. Repeat the identical CreateSubscription call with clientToken "tok-1" — expect the SAME arn returned, and only one subscription in state.
4. POST /v1/GetSubscription with {"arn":"<arn from step 2>"} — expect 200 echoing the stored planFamily/planTier/resourceArns/status and the same ETag.
5. POST /v1/UpdateSubscription with {"arn":"<arn>","planTier":"BUSINESS"} and If-Match set to a stale value — expect ConflictException (409). Retry with the current ETag — expect 200, planTier BUSINESS, a bumped updatedAt and a new ETag.
6. POST /v1/GetSubscription again — expect planTier BUSINESS (the update persisted).
7. POST /v1/ListSubscriptions with no nextToken after creating several subscriptions — expect every stored subscription as a SubscriptionSummary (including its eTag body member), a nextToken when more pages remain, and resuming with that token to return the rest without duplicates.
8. POST /v1/CancelSubscription with {"arn":"<arn>"} and the current If-Match — expect 200 and a scheduledChange of changeType CANCELLATION; a following GetSubscription must show it.
9. POST /v1/CreateSubscription with approvalMode MANUAL — expect status PENDING_APPROVAL; then POST /v1/ApprovePaidSubscription with the returned ETag as If-Match — expect status ACTIVE; call it a second time — expect ConflictException (409).
10. POST /v1/GetSubscription with {"arn":"arn:aws:pricingplanmanager:us-east-1:111122223333:subscription/does-not-exist"} — expect ResourceNotFoundException (404), and POST /v1/CreateSubscription with resourceArns: [] — expect ValidationException (400).
