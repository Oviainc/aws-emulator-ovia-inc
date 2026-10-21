Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 95c91931c960f7f391e4448ce7f3ebc67d41fa17

# Bootstrap the Marketplace Entitlement Service emulator

_Filed: 2026-10-21T22:08:09Z_
_Operation: service-com-amazonaws-marketplaceentitlementservice-awsmpentitlementservice | Class: stub_

> _Note: this issue was identified against the code base at commit `95c91931c960f7f391e4448ce7f3ebc67d41fa17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The service binds exactly ONE operation, `GetEntitlements` (awsJson1_1, `POST /` with `X-Amz-Target: AWSMPEntitlementService.GetEntitlements`), and it is still the auto-generated scaffold answering HTTP 501. The bootstrap slice is therefore the whole read path for the primary resource (Entitlement), implemented statefully.

## Happy path to implement

1. **Seed state** — entitlements are created out-of-band by AWS Marketplace, so the emulator needs a seeding seam (test/admin fixture inserting `Entitlement` records into the service store). This is the "create" step of the lifecycle; nothing in the model creates entitlements.
2. **GetEntitlements (no filter)** — input `ProductCode` (string, len 1..255, **required**). Must return from persisted state ONLY the entitlements whose `ProductCode` equals the requested one; an unknown product code returns an empty `Entitlements` list (HTTP 200), not an error and not a canned record. Output `Entitlements` (list<Entitlement>) where each `Entitlement` carries `ProductCode`, `Dimension`, `CustomerIdentifier`, `CustomerAWSAccountId`, `Value` (an `EntitlementValue` union-like struct with exactly one of `IntegerValue`/`DoubleValue`/`BooleanValue`/`StringValue` set), `ExpirationDate` (timestamp), `LicenseArn` — every field taken from the stored record, never echoed from the request.
3. **GetEntitlements with `Filter`** — `Filter` is map<GetEntitlementFilterName, list<string>> with keys `CUSTOMER_IDENTIFIER`, `DIMENSION`, `CUSTOMER_AWS_ACCOUNT_ID`, `LICENSE_ARN`. Semantics per the model docs: values within one key are UNIONed, and distinct keys are INTERSECTed. `CustomerIdentifier` and `CustomerAWSAccountId` are mutually exclusive — supplying both keys in one request must fail with `InvalidParameterException` (HTTP 400).
4. **GetEntitlements pagination** — `MaxResults` (integer, range 1..25) caps the page size; `NextToken` (pattern `^\S+$`) resumes where the previous page ended. The token must be opaque, derived from the emulator's own ordering/cursor over stored entitlements, and stable: page 1 + page 2 must together equal the unpaginated result with no duplicates and no omissions. A token that does not correspond to a cursor the emulator issued must be refused with `InvalidParameterException` (400). When the final page is reached, `NextToken` is absent.

Every response must be computed from the persisted store — no hardcoded entitlement, no echoing the request's `ProductCode`/filters back as a synthetic entitlement, no random IDs without storage. `GetEntitlements` is a pure read: it must not mutate stored entitlements (issuing/validating a page cursor is the only state it may touch, and ideally that is stateless/encoded).

## State Model

In `src/state.rs`, replace the empty `ComAmazonawsMarketplaceentitlementserviceAwsmpentitlementserviceState` with:

- `entitlements: Vec<StoredEntitlement>` (or `HashMap<String /* ProductCode */, Vec<StoredEntitlement>>` for fast lookup), in stable insertion order so pagination is deterministic.
- `StoredEntitlement { product_code: String, dimension: Option<String>, customer_identifier: Option<String>, customer_aws_account_id: Option<String>, value: Option<StoredEntitlementValue>, expiration_date: Option<DateTime>, license_arn: Option<String> }`
- `StoredEntitlementValue { integer_value: Option<i32>, double_value: Option<f64>, boolean_value: Option<bool>, string_value: Option<String> }` — exactly one populated, mirroring the modeled `EntitlementValue`.
- Optionally `page_cursors: HashMap<String /* NextToken */, usize /* offset */>` if tokens are server-side rather than encoded offsets.

## Out of scope

Nothing else — this service models no other operations. `ThrottlingException` and `InternalServiceErrorException` need no emulated trigger for the bootstrap slice.

## Evidence

OPERATIONS.md for this crate lists exactly 1 operation: `GetEntitlements` — Input `ProductCode` (string, len 1..255, required), `Filter` (map<GetEntitlementFilterName,list<string>>), `NextToken` (pattern ^\S+$), `MaxResults` (range 1..25); Output `Entitlements` (list<Entitlement>), `NextToken`; Errors `InternalServiceErrorException (500)`, `InvalidParameterException (400)`, `ThrottlingException (400)`; trait `@paginated(inputToken=NextToken, outputToken=NextToken)`. `src/ops/get_entitlements.rs` still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and its `handle` body is `Err(crate::scaffolded::Scaffolded::not_implemented("GetEntitlements"))` (line 177), so the service answers 501 for its only operation. `src/state.rs` has `// TODO: add service-specific state fields` — zero state exists. Field lists for `Entitlement` (ProductCode, Dimension, CustomerIdentifier, CustomerAWSAccountId, Value, ExpirationDate, LicenseArn), `EntitlementValue` (IntegerValue/DoubleValue/BooleanValue/StringValue) and the `GetEntitlementFilterName` enum values come from the Smithy model; the union/intersection filter semantics and the CustomerIdentifier/CustomerAWSAccountId mutual exclusion are stated verbatim in the modeled docs for the `Filter` member.

## Steps to Reproduce

1. Start the emulator and seed two entitlements for ProductCode "prod-abc" (customer "cust-1", dimension "Users", IntegerValue 10) and one for ProductCode "prod-xyz".
2. POST / with X-Amz-Target: AWSMPEntitlementService.GetEntitlements and body {"ProductCode":"prod-abc"} — today returns HTTP 501 NotImplemented; expected HTTP 200 with the two seeded prod-abc entitlements and their stored Dimension/CustomerIdentifier/Value/ExpirationDate, and none from prod-xyz.
3. Repeat with {"ProductCode":"prod-abc","Filter":{"DIMENSION":["Users"]}} — expected only the matching entitlement.
4. Repeat with {"ProductCode":"prod-abc","Filter":{"CUSTOMER_IDENTIFIER":["cust-1"],"CUSTOMER_AWS_ACCOUNT_ID":["111122223333"]}} — expected HTTP 400 InvalidParameterException (mutually exclusive filters).
5. Repeat with {"ProductCode":"prod-abc","MaxResults":1} — expected one entitlement plus a NextToken; pass that NextToken back and expect the remaining entitlement with no NextToken on the last page and no duplicates across pages.
6. POST {"ProductCode":"does-not-exist"} — expected HTTP 200 with an empty Entitlements list.
