Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 30fa86cd244b801d1c6064a0a321e2bf32f37967

# catalog and agreementType values are never validated, and an inverted createdAfter/createdBefore window is accepted

_Filed: 2026-10-24T13:53:46Z_
_Operation: ListBillingAdjustmentRequests | Class: validation_

> _Note: this issue was identified against the code base at commit `30fa86cd244b801d1c6064a0a321e2bf32f37967`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ListBillingAdjustmentRequests resolves `agreementId` against the store (answering ValidationException/INVALID_AGREEMENT_ID for an unknown one) but applies `catalog` and `agreementType` as plain predicates, and applies `createdAfter`/`createdBefore` without checking that they form a window.

Expected: a `catalog` other than the single catalog AWS Marketplace exposes (`AWSMarketplace`) is refused with 400 ValidationException carrying reason INVALID_CATALOG, and an `agreementType` outside the documented set is refused with reason INVALID_AGREEMENT_TYPE — the same way this handler already refuses an unresolvable `agreementId`. A request whose `createdAfter` is at or after its `createdBefore` describes an empty window and is a malformed period rather than a selector (ValidationExceptionReason models INVALID_INVOICE_ADJUSTMENT_PERIOD).

Actual: src/ops/list_billing_adjustment_requests.rs filters with `is_none_or(|catalog| catalog == request.catalog)` / `is_none_or(|kind| kind == request.agreement_type)` and compares the two bounds independently, so all three cases answer HTTP 200 with an empty `items` list. The inconsistency is inside one handler: one member of the same request is refused when it names nothing, the others are silently treated as selecting nothing.

## Evidence

Spec: ValidationExceptionReason models INVALID_CATALOG, INVALID_AGREEMENT_TYPE and INVALID_INVOICE_ADJUSTMENT_PERIOD (smithy_query shape=ValidationExceptionReason); ListBillingAdjustmentRequestsInput documents catalog/agreementType/createdAfter/createdBefore; BillingAdjustmentSummary$catalog is "the catalog in which the agreement was created", which crate::state::CATALOG fixes to AWSMarketplace. Handler: src/ops/list_billing_adjustment_requests.rs handle() lines ~208-253 (agreementId refused via validation_exception("INVALID_AGREEMENT_ID"), catalog/agreementType only filtered, created_after/created_before compared independently). Test coverage: tests/all/billing_adjustments.rs lines ~243-309 assert the current behavior ("another catalog selects nothing", "another agreementType selects nothing"); line ~968 covers only the agreementId refusal. No test asserts a catalog/agreementType refusal or an inverted window.

## Steps to Reproduce

1. CreateAgreementRequest + AcceptAgreementRequest, then BatchCreateBillingAdjustmentRequest with one valid entry for that agreement
2. ListBillingAdjustmentRequests(catalog="SomeOtherCatalog") -> observe HTTP 200 with items: [] (expected 400 ValidationException, reason INVALID_CATALOG)
3. ListBillingAdjustmentRequests(agreementType="NotAType") -> observe HTTP 200 with items: [] (expected 400 ValidationException, reason INVALID_AGREEMENT_TYPE)
4. ListBillingAdjustmentRequests(createdAfter="2030-01-01T00:00:00Z", createdBefore="2020-01-01T00:00:00Z") -> observe HTTP 200 with items: [] (expected 400 ValidationException for the inverted period)
5. Compare with ListBillingAdjustmentRequests(agreementId="agmt-does-not-exist"), which is refused 400 ValidationException/INVALID_AGREEMENT_ID
