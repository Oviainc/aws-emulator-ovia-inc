Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: dc6b9d697210888fba6fd46cd7393ea22255dcd4

# Payment requests carry no currencyCode and no maxTotalChargeAmount ceiling, because the accepted VariablePaymentTerm is stored without either member

_Filed: 2026-10-24T12:48:09Z_
_Operation: SendAgreementPaymentRequest | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `dc6b9d697210888fba6fd46cd7393ea22255dcd4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

SendAgreementPaymentRequestOutput$currencyCode ("The currency code for the charge amount", 3-letter ^[A-Z]+$) is documented as the currency of the accepted term, and the operation states the amount "must not exceed the remaining available balance under the VariablePaymentTerm maxTotalChargeAmount". Expected: every payment request reports a currencyCode (the accepted VariablePaymentTerm's), and a send whose amount pushes the agreement's cumulative approved+pending charges past the term's maxTotalChargeAmount is refused with the modelled ValidationException.

Actual: crate::state::accepted_term() builds the accepted VariablePaymentTerm from the request's RequestedTerm with `..Default::default()`, so currency_code and max_total_charge_amount are always None. src/ops/send_agreement_payment_request.rs then does `let currency_code = term.currency_code.clone();` — always None — so the stored PaymentRequest and every later report of it (SendAgreementPaymentRequest output, GetAgreementPaymentRequest, ListAgreementPaymentRequests' PaymentRequestSummary) omits currencyCode entirely, and the handler explicitly skips the cap check because no term carries a maximum. A seller can therefore send unbounded payment requests of arbitrary size against an agreement, and no consumer can tell what currency a charge is in.

## Evidence

Spec: VariablePaymentTerm$currencyCode ("Defines the currency for the prices mentioned in the term") and $maxTotalChargeAmount ("The maximum total amount that can be charged to the customer through variable payment requests under this term"); SendAgreementPaymentRequestOutput$currencyCode, GetAgreementPaymentRequestOutput$currencyCode, PaymentRequestSummary$currencyCode. Handler/state: src/ops/send_agreement_payment_request.rs lines ~253-280 (term lookup, comment stating no balance can be measured, currency_code taken from the term); src/state.rs accepted_term() VariablePaymentTerm arm (`..Default::default()`). Tests: tests/all/payment_requests.rs:338 and :1243 assert currencyCode is null — the divergent behavior is pinned by tests; no test exercises a cap.

## Steps to Reproduce

1. CreateAgreementRequest(intent="NEW", agreementProposalIdentifier="ap-offer1", requestedTerms=[{id:"vpt", configuration:{variablePaymentTermConfiguration:{...}}}]) then AcceptAgreementRequest
2. GetAgreementTerms(agreementId=<id>) -> the accepted variablePaymentTerm carries no currencyCode and no maxTotalChargeAmount
3. SendAgreementPaymentRequest(agreementId=<id>, termId="vpt", name="invoice-1", chargeAmount="100.50") -> observe the 200 response omits currencyCode (expected e.g. "USD")
4. GetAgreementPaymentRequest / ListAgreementPaymentRequests for the same request -> currencyCode absent in both
5. SendAgreementPaymentRequest(... chargeAmount="98765432109876") repeatedly -> every call succeeds; no ceiling is ever reached
