Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the BCM Pricing Calculator emulator

_Filed: 2026-10-21T21:18:53Z_
_Operation: service-com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator | Class: stub_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 36 operations in `services/service-com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator/src/ops/` is an unmodified scaffold: the handler bodies end in `Err(crate::scaffolded::Scaffolded::not_implemented(..))` and answer HTTP 501, and the crate holds no state. A client cannot complete any workflow.

This issue scopes the FIRST buildable slice: the bill-scenario lifecycle, which is this service's primary resource (bill estimates are derived from a bill scenario, so the scenario must exist first). Implement these 6 operations, in this order, statefully over the `awsJson1_0` wire protocol (`POST /`, `X-Amz-Target: AWSBCMPricingCalculator.<OperationName>`):

1. **CreateBillScenario** — input `name` (required, `^[a-zA-Z0-9-]+$`, <=64), optional `clientToken`, `tags`, `groupSharingPreference` (OPEN|PRIORITIZED|RESTRICTED), `costCategoryGroupSharingPreferenceArn`. Must mint a fresh lowercase-hex UUID `id` matching `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`, persist the scenario, and return `id`, `name`, `status` = `READY`, `createdAt` = now, `expiresAt` (now + retention window), `billInterval{start,end}` and the group-sharing fields as stored. Repeating the same `clientToken` with the same payload must return the SAME `id` (the op is `@idempotent` and `clientToken` is `@idempotencyToken`), not a second scenario.
2. **BatchCreateBillScenarioUsageModification** — input `billScenarioId` (required), `usageModifications` (1..25 `BatchCreateBillScenarioUsageModificationEntry`: required `key`, `serviceCode`, `usageType`, `operation`, `usageAccountId`, optional `availabilityZone`, `group`, `amounts`, `historicalUsage`), optional `clientToken`. For each entry, mint a UUID `id`, store it under the parent scenario, and return it in `items` as a `BatchCreateBillScenarioUsageModificationItem` echoing the entry fields plus the assigned `id` and computed `quantities`. An unknown `billScenarioId` must return `ResourceNotFoundException` (HTTP 404), not a 200 with empty items. Per-entry failures go in `errors` keyed by the entry's `key`, not as a top-level exception.
3. **ListBillScenarioUsageModifications** — input `billScenarioId` (required), optional `filters`, `nextToken`, `maxResults` (1..25). Must return exactly the modifications persisted by step 2 as `BillScenarioUsageModificationItem` (`id`, `serviceCode`, `usageType`, `operation`, `usageAccountId`, `group`, `quantities`), honour `maxResults` as a page size, and emit an opaque `nextToken` only when more items remain (and nothing more when the last page is reached). Unknown `billScenarioId` → `ResourceNotFoundException` (404).
4. **GetBillScenario** — input `identifier` (required). Must return the stored scenario: the same `id`, `name`, `status`, `createdAt`, `expiresAt`, `billInterval`, `groupSharingPreference`, `costCategoryGroupSharingPreferenceArn` created in step 1, including any `name`/`expiresAt` change made via UpdateBillScenario. Unknown or deleted id → `ResourceNotFoundException` (404).
5. **ListBillScenarios** — optional `filters`, `createdAtFilter`, `expiresAtFilter`, `nextToken`, `maxResults` (1..25). Must return one `BillScenarioSummary` per persisted scenario (`id`, `name`, `billInterval`, `status`, `createdAt`, `expiresAt`, group-sharing fields), paginate via `maxResults`/`nextToken`, and must NOT include scenarios deleted in step 6.
6. **DeleteBillScenario** — input `identifier` (required), empty output. Must remove the scenario and its usage modifications from state so GetBillScenario afterwards returns `ResourceNotFoundException` and ListBillScenarios no longer lists it. Being `@idempotent`, a second delete of the same id must not 500.

Every response must be COMPUTED FROM PERSISTED STATE. No canned bodies, no echoing the request back, no minting an id that is never stored. A `CreateBillScenario` followed by `GetBillScenario` must return what was created; a `BatchCreate...UsageModification` followed by `List...UsageModifications` must return the created rows with their assigned ids.

## State Model

Service state (behind the existing `crate::state::service_state(&*self.engine)` lock) should hold:

- `bill_scenarios: HashMap<String /* scenario id (uuid) */, BillScenario>` where
  `BillScenario { id: String, arn: String /* arn:aws:bcm-pricing-calculator:<region>:<account>:bill-scenario/<id> */, name: String, status: BillScenarioStatus /* READY|LOCKED|STALE|FAILED */, created_at: DateTime, expires_at: DateTime, bill_interval: BillInterval{ start, end }, failure_message: Option<String>, group_sharing_preference: Option<GroupSharingPreferenceEnum>, cost_category_group_sharing_preference_arn: Option<String>, tags: HashMap<String,String>, usage_modifications: IndexMap<String /* modification id */, UsageModification> }`
- `UsageModification { id: String, key: Option<String> /* batch entry key */, service_code: String, usage_type: String, operation: String, usage_account_id: String, availability_zone: Option<String>, group: Option<String>, amounts: Vec<UsageAmount>, quantities: Vec<UsageQuantity>, historical_usage: Option<HistoricalUsageEntity> }` — insertion-ordered so list pagination is stable.
- `idempotency: HashMap<String /* clientToken */, String /* created resource id */>` so a replayed `clientToken` returns the first result.
- (reserve, for later slices) `bill_estimates: HashMap<String, BillEstimate>`, `workload_estimates: HashMap<String, WorkloadEstimate>`, `preferences: Preferences`.

Pagination tokens must encode a position into the insertion-ordered collection (e.g. an opaque base64 of the offset or last-seen id) and must be rejected/ignored safely if they refer to a vanished item.

Out of scope for this bootstrap (stay 501, to be filed as individual gap issues later): the bill-estimate operations (CreateBillEstimate/GetBillEstimate/UpdateBillEstimate/DeleteBillEstimate/ListBillEstimates and the four ListBillEstimate* readers), the workload-estimate family and its batch usage ops, the commitment-modification batch ops, Update/DeleteBillScenario variants beyond the above, Get/UpdatePreferences, and Tag/Untag/ListTagsForResource.

## Evidence

OPERATIONS.md at the crate root documents all 36 operations with `awsJson1_0` / `X-Amz-Target: AWSBCMPricingCalculator.<Op>` routing (lines 8-14) and the exact signatures cited above: CreateBillScenario (lines 122-127, required `name`, output required `id`, `status` BillScenarioStatus FAILED|LOCKED|READY|STALE), GetBillScenario (129-134, required `identifier`, errors DataUnavailableException 400 / ResourceNotFoundException 404), DeleteBillScenario (143-148, @idempotent, empty output), ListBillScenarios (150-155, @paginated inputToken=nextToken outputToken=nextToken items=items, maxResults 1..25), ListBillScenarioUsageModifications (185-190, required `billScenarioId`, @paginated, ResourceNotFoundException 404), BatchCreateBillScenarioUsageModification (192-197, required `billScenarioId` + `usageModifications` 1..25, output `items`/`errors`). Shape detail from smithy_query on BatchCreateBillScenarioUsageModificationEntry (required key/serviceCode/usageType/operation/usageAccountId), BillScenarioUsageModificationItem, BillScenarioSummary and BillInterval{start,end}. Proof of all-stub status: `src/ops/create_bill_scenario.rs` carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header (line 4) and its `handle` body is `Err(crate::scaffolded::Scaffolded::not_implemented("CreateBillScenario"))` (line 182); all 36 files in `src/ops/` are the same generated stub. There are no tests and no state module populated for this crate.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client at it with signing name `bcm-pricing-calculator`.
2. Call CreateBillScenario with name="my-scenario" and a clientToken; expect HTTP 200 with a UUID `id` and status=READY. Observed: HTTP 501 NotImplemented.
3. Re-call CreateBillScenario with the SAME clientToken and payload; expect the same `id` back (idempotent replay), not a new scenario.
4. Call BatchCreateBillScenarioUsageModification with billScenarioId=<id> and one entry {key:"k1", serviceCode:"AmazonEC2", usageType:"BoxUsage:m5.large", operation:"RunInstances", usageAccountId:"123456789012", amounts:[...]}; expect 200 with items[0].id set.
5. Call ListBillScenarioUsageModifications with billScenarioId=<id>; expect exactly the one modification just created, with the same id and serviceCode/usageType/operation/usageAccountId.
6. Call GetBillScenario with identifier=<id>; expect the name, status, createdAt, expiresAt and billInterval returned at create time.
7. Call ListBillScenarios with maxResults=1; expect the scenario in `items` and `nextToken` present only if a second scenario exists.
8. Call DeleteBillScenario with identifier=<id>; then GetBillScenario with the same id expecting ResourceNotFoundException (HTTP 404), and ListBillScenarios expecting the scenario to be absent.
9. Call BatchCreateBillScenarioUsageModification with a billScenarioId that was never created; expect ResourceNotFoundException (HTTP 404), not a 200.
