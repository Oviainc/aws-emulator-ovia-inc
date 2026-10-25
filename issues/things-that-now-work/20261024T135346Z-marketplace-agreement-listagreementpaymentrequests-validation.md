Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 30fa86cd244b801d1c6064a0a321e2bf32f37967

# agreementId, catalog and agreementType are never resolved: a bogus value answers 200 with an empty page instead of ValidationException

_Filed: 2026-10-24T13:53:46Z_
_Operation: ListAgreementPaymentRequests | Class: validation_

> _Note: this issue was identified against the code base at commit `30fa86cd244b801d1c6064a0a321e2bf32f37967`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ListAgreementPaymentRequests validates only `partyType` (against state::PARTY_TYPE_VALUES). Every other selector is applied as a pure predicate over the stored records, so a value that names nothing this account can hold is answered as a filter that selects nothing.

Expected: an `agreementId` that names no agreement of the caller is refused with 400 ValidationException carrying reason INVALID_AGREEMENT_ID (the reason the model defines and the one this very crate answers the same member with on ListAgreementCharges and ListBillingAdjustmentRequests); a `catalog` other than the single catalog AWS Marketplace exposes (`AWSMarketplace`) is refused with reason INVALID_CATALOG; an `agreementType` outside the documented set (PurchaseAgreement / VendorInsightsAgreement) is refused with reason INVALID_AGREEMENT_TYPE.

Actual: src/ops/list_agreement_payment_requests.rs skips non-matching records (`is_some_and(|id| id != request.agreement_id)`, `agreement.is_none_or(... catalog != catalog)`) and returns HTTP 200 with `items: []`. A client that typos an agreement id or catalog cannot tell "no payment requests" from "you asked about something that does not exist".

Note this is also an internal inconsistency: ListAgreementCharges (src/ops/list_agreement_charges.rs lines ~180-187) and ListBillingAdjustmentRequests (lines ~208-217) DO refuse an unknown `agreementId` with ValidationException/INVALID_AGREEMENT_ID, citing "the same refusal the sibling listings of this service answer the same member with" — but this sibling does not.

## Evidence

Spec: ValidationExceptionReason models INVALID_AGREEMENT_ID, INVALID_CATALOG and INVALID_AGREEMENT_TYPE (smithy_query shape=ValidationExceptionReason); ListAgreementPaymentRequestsInput members agreementId/catalog/agreementType (`e.g., AWSMarketplace`, `e.g., PurchaseAgreement`); AWS Marketplace exposes the single catalog AWSMarketplace, which crate::state::CATALOG itself hard-codes. Handler: src/ops/list_agreement_payment_requests.rs handle() — only PARTY_TYPE_VALUES is enforced, no store lookup for agreementId and no value check for catalog/agreementType before the scan. Test coverage: tests/all/payment_requests.rs lines ~583-599 assert the CURRENT behavior ("another catalog selects nothing", "an agreement and a catalog it is not in select nothing"), so a fix must update those rows; no test asserts a refusal for an unknown agreementId on this operation. The sibling ListAgreementCharges validation gap is already filed (20261024T132719Z-marketplace-agreement-listagreementcharges-validation) and covers only that operation.

## Steps to Reproduce

1. CreateAgreementRequest + AcceptAgreementRequest to store one agreement, then SendAgreementPaymentRequest under its VariablePaymentTerm
2. ListAgreementPaymentRequests(partyType="Proposer", agreementId="agmt-does-not-exist") -> observe HTTP 200 with items: [] (expected 400 ValidationException, reason INVALID_AGREEMENT_ID, fields[0].name = agreementId)
3. ListAgreementPaymentRequests(partyType="Proposer", catalog="SomeOtherCatalog") -> observe HTTP 200 with items: [] (expected 400 ValidationException, reason INVALID_CATALOG)
4. ListAgreementPaymentRequests(partyType="Proposer", agreementType="NotAnAgreementType") -> observe HTTP 200 with items: [] (expected 400 ValidationException, reason INVALID_AGREEMENT_TYPE)
5. Compare with ListAgreementCharges(agreementId="agmt-does-not-exist"), which answers 400 ValidationException/INVALID_AGREEMENT_ID for the same member
