Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 16fab4a727e8e34e48fe5cd80cb0080f7932bd9f

# Bootstrap the AWS Marketplace Metering Service emulator

_Filed: 2026-10-21T23:18:05Z_
_Operation: service-com-amazonaws-marketplacemetering-awsmpmeteringservice | Class: missing_

> _Note: this issue was identified against the code base at commit `16fab4a727e8e34e48fe5cd80cb0080f7932bd9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 4 operations this service binds (`src/ops/resolve_customer.rs`, `register_usage.rs`, `meter_usage.rs`, `batch_meter_usage.rs`) still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and answers HTTP 501, so no client can complete a single metering call. Implement a first stateful slice covering the service's whole core loop — this is a 4-operation service, so the happy path is the entire primary workflow.

Wire protocol (README/OPERATIONS.md): `awsJson1_1`, `POST /` with `X-Amz-Target: AWSMPMeteringService.<OperationName>`, sigv4 signing name `aws-marketplace`.

### Happy path, in order

1. **ResolveCustomer** — input `RegistrationToken` (required, pattern `^[\s\S]+$`). Must look the token up in persisted state and return the registration it was minted for: `CustomerIdentifier`, `ProductCode`, `CustomerAWSAccountId` (digits only, pattern `^[0-9]+$`), optionally `LicenseArn` and `Metadata`. The SAME token resolved twice must return the SAME `CustomerIdentifier`/`ProductCode` — not a freshly randomized identity. An unknown/garbage token must fail with `InvalidTokenException` (HTTP 400), and a token whose expiry has passed with `ExpiredTokenException` (HTTP 400). To make this seedable for tests, maintain a token table in state that a test fixture can pre-populate; a token not present is invalid, never auto-created.

2. **RegisterUsage** — input `ProductCode` (required), `PublicKeyVersion` (required, range 1..), `Nonce` (optional). Must record the (ProductCode, CustomerIdentifier, PublicKeyVersion, Nonce) registration in state and return `Signature` plus `PublicKeyRotationTimestamp` derived from the stored public-key entry. Repeated calls with the same inputs must return the same stored signature value (the real service's signature is stable per registration/nonce). `PublicKeyVersion` that does not exist in state must fail with `InvalidPublicKeyVersionException`; a `ProductCode` not registered in state must fail with `InvalidProductCodeException`.

3. **MeterUsage** — input `ProductCode` (required), `Timestamp` (required), `UsageDimension` (required, len 1..255), `UsageQuantity` (optional, range 0..2147483647, defaults to 0), `UsageAllocations` (optional, len 1..2500), `DryRun` (optional), `ClientToken` (optional, `@idempotencyToken`, len 1..64). Must append a metering record to state and return a freshly generated, stored `MeteringRecordId`. Required stateful semantics for the first slice:
   - **De-duplication:** a second call with the same (ProductCode, CustomerIdentifier, UsageDimension, Timestamp hour) and the SAME quantity returns the already-stored `MeteringRecordId`; with a DIFFERENT quantity it fails with `DuplicateRequestException` (HTTP 400).
   - **Idempotency:** replaying the same `ClientToken` with identical inputs returns the stored `MeteringRecordId`; replaying it with different inputs fails with `IdempotencyConflictException` (HTTP 409).
   - **DryRun=true:** validate and return without persisting any record (a later `BatchMeterUsage`/dedup check must not see it).
   - **Validation:** `ProductCode` not known in state → `InvalidProductCodeException`; unknown `UsageDimension` for that product → `InvalidUsageDimensionException`; `Timestamp` more than 24 hours in the past or in the future → `TimestampOutOfBoundsException`; `UsageAllocations` whose `AllocatedUsageQuantity` values do not sum to `UsageQuantity` → `InvalidUsageAllocationsException`.

4. **BatchMeterUsage** — input `UsageRecords` (required, len 0..25), `ProductCode`. Must process each `UsageRecord{Timestamp, CustomerIdentifier|CustomerAWSAccountId, Dimension, Quantity, UsageAllocations, LicenseArn}` against the SAME persisted store `MeterUsage` writes to, and return one `UsageRecordResult{UsageRecord, MeteringRecordId, Status}` per input record, in input order. `Status` must be computed per record, not canned: `Success` for an accepted record (with a stored `MeteringRecordId`), `CustomerNotSubscribed` when the record's customer is unknown/not entitled for the product, `DuplicateRecord` when a previously metered record had the same customer+dimension+time but a different quantity. Records that could not be attempted go to `UnprocessedRecords`. More than 25 records must be rejected (modeled length constraint), and a bad `ProductCode` with `InvalidProductCodeException`.

Every one of these four MUST compute its response from persisted state held in the shared engine reached via `crate::state::service_state(&*self.engine)` — no canned signatures, no echoed inputs, no random `MeteringRecordId` that is never stored. A `MeterUsage` call followed by a duplicate `MeterUsage` or an overlapping `BatchMeterUsage` record must observe the earlier write.

## State Model

In `src/state.rs`, on the service state struct:

- `registration_tokens: HashMap<String /*RegistrationToken*/, TokenEntry>` where `TokenEntry { customer_identifier: String, customer_aws_account_id: String, product_code: String, license_arn: Option<String>, metadata: Option<Metadata>, expires_at: Option<SystemTime> }` — backs `ResolveCustomer`; seedable by tests, never auto-created on lookup.
- `products: HashMap<String /*ProductCode*/, Product>` where `Product { dimensions: HashSet<String>, entitled_customers: HashSet<String /*CustomerIdentifier or CustomerAWSAccountId*/>, public_key_versions: HashMap<i32, PublicKeyEntry> }` and `PublicKeyEntry { rotation_timestamp: SystemTime }` — backs `InvalidProductCodeException`, `InvalidUsageDimensionException`, `CustomerNotEntitledException`/`CustomerNotSubscribed`, `InvalidPublicKeyVersionException`.
- `registrations: HashMap<(String /*ProductCode*/, i32 /*PublicKeyVersion*/, String /*Nonce*/), Registration>` where `Registration { signature: String, public_key_rotation_timestamp: SystemTime, registered_at: SystemTime }` — backs `RegisterUsage` returning a stable signature.
- `metering_records: Vec<MeteringRecord>` where `MeteringRecord { metering_record_id: String, product_code: String, customer_identifier: String, customer_aws_account_id: Option<String>, dimension: String, quantity: i32, timestamp: SystemTime, usage_allocations: Vec<UsageAllocation>, created_at: SystemTime }` — the ledger both `MeterUsage` and `BatchMeterUsage` append to and read for dedup.
- `dedup_index: HashMap<(String /*ProductCode*/, String /*customer*/, String /*Dimension*/, i64 /*Timestamp truncated to the hour*/), DedupEntry>` where `DedupEntry { metering_record_id: String, quantity: i32 }` — backs `DuplicateRequestException` / `DuplicateRecord`.
- `idempotency: HashMap<String /*ClientToken*/, IdempotencyEntry>` where `IdempotencyEntry { request_fingerprint: u64, metering_record_id: String }` — backs `IdempotencyConflictException` vs. replay.

Anything beyond this slice (e.g. richer `Metadata`/`LicenseArn` resolution, tag-level allocation reporting, throttling simulation) stays stubbed and will be filed as individual gap issues later.

## Evidence

`services/service-com-amazonaws-marketplacemetering-awsmpmeteringservice/OPERATIONS.md` lists exactly 4 operations (`BatchMeterUsage`, `MeterUsage`, `RegisterUsage`, `ResolveCustomer`) with the input/output members and errors cited above, plus the `awsJson1_1` / `X-Amz-Target: AWSMPMeteringService.<Op>` wire contract. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:'` over `src/ops/` matches all four handler files (`resolve_customer.rs`, `register_usage.rs`, `meter_usage.rs`, `batch_meter_usage.rs`), each of which returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` → HTTP 501, so the service holds zero state today. Per-record `Status` semantics (`Success`, `CustomerNotSubscribed`, `DuplicateRecord`) and the 24-hour backdating window come from the Smithy model's documentation on `UsageRecordResult.Status` and `UsageRecord.Timestamp`; `UsageAllocation.AllocatedUsageQuantity` summing to the record quantity comes from the `UsageRecord.UsageAllocations` doc. `MeterUsage.ClientToken` is `@idempotencyToken` in the model and `IdempotencyConflictException` is modeled at HTTP 409.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client at it with service `meteringmarketplace` (endpoint prefix `metering.marketplace`, signing name `aws-marketplace`).
2. Seed/obtain a registration token, then call ResolveCustomer with `RegistrationToken` — expect HTTP 200 with a stable `CustomerIdentifier`, `ProductCode` and numeric `CustomerAWSAccountId`; today it returns HTTP 501 NotImplemented.
3. Call ResolveCustomer again with the SAME token — expect the identical `CustomerIdentifier`/`ProductCode` (proving state, not per-call randomness).
4. Call ResolveCustomer with a garbage token — expect `InvalidTokenException` (HTTP 400), not 501 and not a success.
5. Call RegisterUsage with `ProductCode` + `PublicKeyVersion=1` — expect HTTP 200 with `Signature` and `PublicKeyRotationTimestamp`; repeat the call and expect the same `Signature`.
6. Call MeterUsage with `ProductCode`, `UsageDimension`, `UsageQuantity=10` and a `Timestamp` within the last hour — expect HTTP 200 with a non-empty `MeteringRecordId`.
7. Replay that exact MeterUsage call — expect the SAME `MeteringRecordId` returned (de-dup), then replay with `UsageQuantity=20` for the same dimension/timestamp — expect `DuplicateRequestException` (HTTP 400).
8. Call MeterUsage with `DryRun=true` for a new dimension, then re-issue it without `DryRun` — expect the second call to succeed with a fresh record, proving the dry run persisted nothing.
9. Call BatchMeterUsage with 3 `UsageRecords`: one valid new record, one duplicating the earlier MeterUsage with a different quantity, and one for an unknown customer — expect 3 `Results` in input order with `Status` values `Success` (plus a `MeteringRecordId`), `DuplicateRecord`, and `CustomerNotSubscribed` respectively.
10. Call BatchMeterUsage with 26 records — expect a 400 validation error for the modeled 0..25 length constraint.
