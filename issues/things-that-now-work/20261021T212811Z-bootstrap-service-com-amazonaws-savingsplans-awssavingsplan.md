Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the savingsplans emulator

_Filed: 2026-10-21T21:28:11Z_
_Operation: service-com-amazonaws-savingsplans-awssavingsplan | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation in `services/service-com-amazonaws-savingsplans-awssavingsplan/src/ops/` is an unmodified scaffold: each carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501), holding no state. Implement a first functional slice of the AWS Savings Plans emulator (restJson1, sigv4 name `savingsplans`, routes `POST /<OperationName>`) covering the purchase lifecycle of the primary resource, a Savings Plan.

### Happy path to implement (in order)

1. **DescribeSavingsPlansOfferings** — input: `offeringIds` (list<string>), `paymentOptions` (list<SavingsPlanPaymentOption>), `productType` (SavingsPlanProductType), `planTypes` (list<SavingsPlanType>), `durations` (list<long>), `currencies` (list<CurrencyCode>), `nextToken`, `maxResults` (0..1000, required/default 0). Output: `searchResults` (list<SavingsPlanOffering>: `offeringId`, `productTypes`, `planType`, `description`, `paymentOption`, `durationSeconds`, `currency`, `serviceCode`, `usageType`, `operation`, `properties`), `nextToken`. Must return a seeded catalog of offerings from state, filtered by the supplied filters and truncated/paged with `maxResults`/`nextToken` — not an empty or canned list.
2. **CreateSavingsPlan** — input: `savingsPlanOfferingId` (required), `commitment` (required), `upfrontPaymentAmount`, `purchaseTime`, `clientToken` (@idempotencyToken), `tags`. Output: `savingsPlanId`. Must look the offering up in the seeded catalog and return `ResourceNotFoundException` (404) when it does not exist; must mint a UUID plan id and ARN `arn:aws:savingsplans::<account>:savingsplan/<uuid>`, persist a plan record derived from the offering (plan type, payment option, currency, term duration, region, product types), set `state` to `payment-pending`/`active`, store `tags`, and make `clientToken` idempotent (same token ⇒ same `savingsPlanId`, no second plan).
3. **DescribeSavingsPlans** — input: `savingsPlanArns` (0..100), `savingsPlanIds`, `states` (list<SavingsPlanState>), `filters`, `maxResults` (1..1000), `nextToken`. Output: `savingsPlans` (list<SavingsPlan>), `nextToken`. Must return the plans actually created, with every field computed from the stored record (`offeringId`, `savingsPlanId`, `savingsPlanArn`, `description`, `start`, `end`, `state`, `region`, `ec2InstanceFamily`, `savingsPlanType`, `paymentOption`, `productTypes`, `currency`, `commitment`, `upfrontPaymentAmount`, `recurringPaymentAmount`, `termDurationInSeconds`, `tags`, `returnableUntil`), honoring the id/ARN/state filters and paging.
4. **TagResource / ListTagsForResource** — `resourceArn` (required, must match the savingsplan ARN pattern) plus `tags` (required) for TagResource; ListTagsForResource returns `tags` for that ARN. Tags written by TagResource (and by CreateSavingsPlan) must be returned by ListTagsForResource and must appear in the plan's `tags` in DescribeSavingsPlans. Unknown ARN ⇒ `ResourceNotFoundException` (404); malformed ARN ⇒ `ValidationException` (400).
5. **ReturnSavingsPlan** — input: `savingsPlanId` (required), `clientToken`. Output: `savingsPlanId`. Must find the stored plan (404 when absent), transition its `state` to `retired`/`queued-deleted` as appropriate, and make that transition observable from a subsequent DescribeSavingsPlans; repeated returns of an already-returned plan must not silently succeed as a fresh return (same clientToken is idempotent).

No canned or echoed responses: every one of these five operations must compute its response from persisted state, so a create is visible to a later describe, a tag is visible to a later list, and a return is visible as a state change. Operations left out of this slice (DescribeSavingsPlanRates, DescribeSavingsPlansOfferingRates, DeleteQueuedSavingsPlan, UntagResource) stay stubbed at 501 and will be filed as individual gap issues.

## State Model

- `SavingsPlansState` (the per-service store reached via `crate::state::service_state`), holding:
  - `offerings: Vec<Offering>` — seeded, read-only catalog. `Offering { offering_id: Uuid, product_types: Vec<SavingsPlanProductType>, plan_type: SavingsPlanType, description: String, payment_option: SavingsPlanPaymentOption, duration_seconds: i64, currency: CurrencyCode, service_code: String, usage_type: String, operation: String, properties: Vec<SavingsPlanOfferingProperty> }`.
  - `plans: IndexMap<String /* savingsPlanId */, SavingsPlanRecord>` — insertion-ordered so paging is stable. `SavingsPlanRecord { savings_plan_id: String, savings_plan_arn: String, offering_id: String, description: String, start: String, end: String, state: SavingsPlanState, region: String, ec2_instance_family: Option<String>, savings_plan_type: SavingsPlanType, payment_option: SavingsPlanPaymentOption, product_types: Vec<SavingsPlanProductType>, currency: CurrencyCode, commitment: String, upfront_payment_amount: Option<String>, recurring_payment_amount: String, term_duration_in_seconds: i64, returnable_until: String, tags: BTreeMap<String, String> }`.
  - `arn_index: HashMap<String /* arn */, String /* savingsPlanId */>` — for ARN-keyed tag operations and `savingsPlanArns` filtering.
  - `idempotency: HashMap<String /* clientToken */, String /* savingsPlanId */>` — for CreateSavingsPlan/ReturnSavingsPlan idempotency.
  - `page_tokens: HashMap<String /* nextToken */, usize /* offset */>` — opaque base64-ish tokens matching `^[A-Za-z0-9/=\+]+$` for the describe paginators.

Done = a client script that lists offerings, purchases one, describes the plan and sees the committed values and tags, tags it and lists its tags, then returns it and sees the new state, passes end to end with correct bodies (not merely 2xx).

## Evidence

`services/service-com-amazonaws-savingsplans-awssavingsplan/OPERATIONS.md` lists all 10 operations with their input/output members and errors (restJson1, SDK id `savingsplans`, `POST /<OperationName> -> 200`). `src/ops/create_savings_plan.rs` lines 4-6 carry `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 166 returns `Err(crate::scaffolded::Scaffolded::not_implemented("CreateSavingsPlan"))`; every sibling file in `src/ops/` is the same generated stub, so no operation holds state. Output field lists come from the Smithy model shapes `SavingsPlan` and `SavingsPlanOffering` (queried via smithy_query); error codes (`ResourceNotFoundException` 404, `ValidationException` 400, `ServiceQuotaExceededException` 402) are from the model's per-operation error lists. No tests exist asserting any of this behavior.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client at it with service `savingsplans`.
2. Call DescribeSavingsPlansOfferings with maxResults=10 and a planTypes filter — expect a non-empty `searchResults` of seeded offerings; today it answers HTTP 501 NotImplemented.
3. Call CreateSavingsPlan with `savingsPlanOfferingId` from step 2, `commitment="0.100"`, `tags={"env":"test"}` and a `clientToken` — expect a `savingsPlanId` back; repeat with the same clientToken and expect the SAME id with no second plan.
4. Call DescribeSavingsPlans with that `savingsPlanIds` — expect one SavingsPlan whose `commitment`, `savingsPlanType`, `paymentOption`, `currency`, `termDurationInSeconds` and `tags` match the offering and the create request, plus a well-formed `savingsPlanArn`.
5. Call TagResource on that ARN with `{"owner":"qa"}`, then ListTagsForResource on the same ARN — expect both `env` and `owner` returned, and the same tags visible in DescribeSavingsPlans.
6. Call ListTagsForResource with an ARN for a nonexistent plan — expect ResourceNotFoundException (HTTP 404), and with a malformed ARN expect ValidationException (HTTP 400).
7. Call ReturnSavingsPlan with the `savingsPlanId` — expect the id echoed and a subsequent DescribeSavingsPlans to show the plan in a returned/retired state.
