Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f

# An adjustment entry is accepted without resolving originalInvoiceId, checking currencyCode against the agreement, or bounding adjustmentAmount by the invoice

_Filed: 2026-10-24T13:27:19Z_
_Operation: BatchCreateBillingAdjustmentRequest | Class: validation_

> _Note: this issue was identified against the code base at commit `d9cda5a9b6bc2cdf59d194d1abe409fa5e2cae3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Each batch entry adjusts a specific invoice of an agreement, and the model names the refusals the service makes for an entry whose invoice details do not hold up: ValidationExceptionReason carries MISSING_INVOICE_ID, INVALID_INVOICE_ADJUSTMENT_PERIOD, INVALID_CURRENCY_CODE and EXCEEDED_MAXIMUM_ADJUSTMENT_AMOUNT, and BatchCreateBillingAdjustmentError$code carries VALIDATION_EXCEPTION / RESOURCE_NOT_FOUND_EXCEPTION for reporting a single bad entry. BatchCreateBillingAdjustmentRequestEntry$currencyCode is documented "Must match the currency code of the offer associated with the agreement (e.g., USD)" and $originalInvoiceId is "the identifier of the original invoice to adjust".

Expected: an entry naming an `originalInvoiceId` that is not an invoice of the named agreement is reported on the per-entry `errors` channel (RESOURCE_NOT_FOUND_EXCEPTION / VALIDATION_EXCEPTION), an entry whose `currencyCode` differs from the agreement's currency is refused (INVALID_CURRENCY_CODE), an `adjustmentAmount` larger than the invoice being adjusted is refused (EXCEEDED_MAXIMUM_ADJUSTMENT_AMOUNT), and an invoice outside the adjustable period is refused (INVALID_INVOICE_ADJUSTMENT_PERIOD).

Actual: the handler applies only three per-entry checks — clientToken replay/conflict, the agreement exists, and `is_positive_amount(adjustmentAmount)` — then stores the entry verbatim. `originalInvoiceId` is accepted as any 1..255-character string and never resolved against anything (the emulator holds no invoice records at all; ListAgreementInvoiceLineItems is still a scaffold), `currencyCode` is accepted as any 3 uppercase letters and never compared with the agreement (so "EUR" on a USD agreement succeeds), and `adjustmentAmount` is bounded by nothing but positivity (so an adjustment orders of magnitude larger than anything the agreement was ever invoiced is accepted). Every such entry answers in `items` with a `ba-…` id and settles to COMPLETED, so GetBillingAdjustmentRequest / ListBillingAdjustmentRequests then report a credit AWS would never have created. Note the currency and amount checks need the agreement to carry a currency/charge amount at all, which it currently does not (see the open CreateAgreementRequest "quote calculates no money" issue); the invoice-resolution and currency-consistency rules are the part this issue asks for.

## Evidence

Spec: smithy_query shape=ValidationExceptionReason lists MISSING_INVOICE_ID, INVALID_INVOICE_ADJUSTMENT_PERIOD, INVALID_CURRENCY_CODE, EXCEEDED_MAXIMUM_ADJUSTMENT_AMOUNT; shape=BillingAdjustmentErrorCode lists RESOURCE_NOT_FOUND_EXCEPTION/VALIDATION_EXCEPTION/CONFLICT_EXCEPTION; BatchCreateBillingAdjustmentRequestEntry$currencyCode ("Must match the currency code of the offer associated with the agreement") and $originalInvoiceId. Handler: src/ops/batch_create_billing_adjustment_request.rs lines ~232-341 — per entry it checks only billing_adjustment_by_client_token(), store.agreements.get(&entry.agreement_id), is_positive_amount(&entry.adjustment_amount) and the store quota, then inserts a BillingAdjustmentRequest copying original_invoice_id/currency_code/adjustment_amount straight from the entry (src/state.rs BillingAdjustmentRequest). src/state.rs own comment admits "The checks AWS can only make later (against the invoice being adjusted) need the invoice, which this service neither models nor stores." Tests: tests/all/billing_adjustments.rs asserts the batch-level MULTIPLE_AGREEMENT_IDS/INVALID_CLIENT_TOKEN/limit refusals and the not-found/non-positive-amount per-entry errors; no test asserts an invoice, currency or maximum-amount refusal.

## Steps to Reproduce

1. CreateAgreementRequest(intent="NEW", agreementProposalIdentifier="ap-offer1", requestedTerms=[{id:"cfg", configuration:{configurableUpfrontPricingTermConfiguration:{selectorValue:"P12M", dimensions:[{dimensionKey:"Users", dimensionValue:5}]}}}]) then AcceptAgreementRequest -> agreement A
2. BatchCreateBillingAdjustmentRequest(billingAdjustmentRequestEntries=[{agreementId:A, originalInvoiceId:"not-an-invoice-of-this-agreement", adjustmentAmount:"1", currencyCode:"USD", adjustmentReasonCode:"OTHER", clientToken:"t1"}]) -> observe 200 with items[0].billingAdjustmentRequestId and an empty errors[] (expected the entry on errors[] with code RESOURCE_NOT_FOUND_EXCEPTION/VALIDATION_EXCEPTION, since the invoice is not one of the agreement's)
3. BatchCreateBillingAdjustmentRequest(entries=[{agreementId:A, originalInvoiceId:"inv-1", adjustmentAmount:"1", currencyCode:"EUR", adjustmentReasonCode:"OTHER", clientToken:"t2"}]) -> observe 200 (expected a refusal with reason INVALID_CURRENCY_CODE: the currency must match the agreement's offer)
4. BatchCreateBillingAdjustmentRequest(entries=[{agreementId:A, originalInvoiceId:"inv-1", adjustmentAmount:"99999999", currencyCode:"USD", adjustmentReasonCode:"OTHER", clientToken:"t3"}]) -> observe 200 (expected EXCEEDED_MAXIMUM_ADJUSTMENT_AMOUNT: the adjustment exceeds the invoice being adjusted)
5. GetBillingAdjustmentRequest(agreementId=A, billingAdjustmentRequestId=<each id>) -> after ~500ms each reports status COMPLETED for an adjustment against an invoice that does not exist
