Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 442a99274869dbb92025fb75ce375f34a4aa7eac

# A payment request sent under an AUTO_APPROVE_ON_EXPIRATION variable payment term never auto-approves

_Filed: 2026-10-29T12:11:21Z_
_Operation: SendAgreementPaymentRequest | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `442a99274869dbb92025fb75ce375f34a4aa7eac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model's `VariablePaymentTermConfiguration` states that a payment request sent under a term configured with `paymentRequestApprovalStrategy = AUTO_APPROVE_ON_EXPIRATION` becomes APPROVED once `expirationDuration` has elapsed with no buyer action. This crate persists that configuration on the accepted term (so `GetAgreementTerms` reports it back) and `SendAgreementPaymentRequest` reads that very term to resolve the currency and the `maxTotalChargeAmount` ceiling — but it ignores the configuration's strategy and duration entirely: the stored `PaymentRequest` is given `PENDING_APPROVAL` unconditionally and carries NO due instant (contrast `BillingAdjustmentRequest.completes_at`), and no code path anywhere in the crate ever performs the transition. There is no `settle_payment_requests` to match the crate's existing `settle_billing_adjustments`, and `AUTO_APPROVE_ON_EXPIRATION` appears in `src/` only inside the generated enum-value list.

Consequences: `GetAgreementPaymentRequest`/`ListAgreementPaymentRequests` keep reporting `PENDING_APPROVAL` arbitrarily long after `expirationDuration` elapsed; no `chargeId` is ever minted for such a request, so `ListAgreementCharges` never reports the charge the approval would have created; and `CancelAgreementPaymentRequest`/`RejectAgreementPaymentRequest` still succeed on a request that should already be APPROVED (their own handler comments state that once approved the request "can no longer be accepted or rejected"). The emulator behaves as if `WAIT_FOR_APPROVAL` were the only modelled strategy.

## Evidence

Smithy model — `smithy_query(query="shape", shape="VariablePaymentTermConfiguration")`:
- `paymentRequestApprovalStrategy` (required): "Defines the strategy for approving payment requests. Values include AUTO_APPROVE_ON_EXPIRATION and WAIT_FOR_APPROVAL"
- `expirationDuration`: "Defines the duration after which a payment request is automatically approved if no further action is taken. This only applies when the payment request approval strategy is set to AUTO_APPROVE_ON_EXPIRATION. The duration is represented in the ISO_8601 format (e.g., P10D for 10 days)."

The configuration IS persisted, so the handler has it available — `src/state.rs:1896`-`1909`:
```
"VariablePaymentTerm" => AcceptedTerm::variablePaymentTerm(VariablePaymentTerm {
    ... configuration: match configuration {
        Some(RequestedTermConfiguration::variablePaymentTermConfiguration(cfg)) => Some(cfg),
```

The handler reads that same term and then hardcodes the status — `src/ops/send_agreement_payment_request.rs:259` `let Some(term) = agreement.variable_payment_term(&input.term_id) else {`, and `:307`-`:309`:
```
// "The payment request is created in `PENDING_APPROVAL` status, at
// which point the buyer can accept or reject it."
status: crate::state::PAYMENT_REQUEST_STATUS_PENDING_APPROVAL.to_string(),
```
Nothing in the record it stores can carry the auto-approval instant: `pub struct PaymentRequest` (`src/state.rs:2632`-`2673`) holds `status`, `status_message`, `charge_id`, `created_at`, `updated_at` and no expiry/strategy member.

The crate already owns this exact clock-due pattern for the sibling resource — `src/state.rs:5014`-`5029`:
```
/// A request is submitted `PENDING` and AWS Marketplace processes it; the
/// processing is owed at `completes_at` whether or not anybody calls, so a
/// reporting operation materialises it before it reports.
pub fn settle_billing_adjustments(&mut self, now: SystemTime) {
```
but no analogue exists for payment requests: `grep -n 'AUTO_APPROVE|expiration_duration|payment_request_approval_strategy' src/` matches only `src/ops/types/shared_types.rs:106`-`107` (the generated struct members) and `:201` (`PAYMENT_REQUEST_APPROVAL_STRATEGY_VALUES`).

Stale decisions stay possible: `src/ops/cancel_agreement_payment_request.rs:254`-`261` ("Once cancelled, the payment request transitions to CANCELLED status and can no longer be accepted or rejected by the buyer" ... "CANCELLED is terminal, so nothing is owed to the clock here") will still move such a request long after its `expirationDuration` passed.

Test coverage: no test exercises the strategy — the only fixture in the suite is `tests/all/agreement_lifecycle.rs:276` `"paymentRequestApprovalStrategy": "WAIT_FOR_APPROVAL"`, and `tests/all/payment_request_approval.rs` only covers the buyer's explicit accept. So the behaviour is unpinned, not deliberately fixed.

## Steps to Reproduce

1. CreateAgreementRequest with intent=NEW and a requestedTerms entry whose configuration is `variablePaymentTermConfiguration` = {"paymentRequestApprovalStrategy": "AUTO_APPROVE_ON_EXPIRATION", "expirationDuration": "PT1S"}.
2. AcceptAgreementRequest with the returned agreementRequestId; note the agreementId.
3. GetAgreementTerms on that agreement — the accepted `variablePaymentTerm` reports back the AUTO_APPROVE_ON_EXPIRATION strategy and the PT1S duration, proving the configuration was stored.
4. SendAgreementPaymentRequest with that agreementId, the term's id, a name and a chargeAmount within `maxTotalChargeAmount` — status is PENDING_APPROVAL.
5. Wait longer than `expirationDuration` (1 second), then call GetAgreementPaymentRequest with the agreementId/paymentRequestId pair.
6. Observed: status is still PENDING_APPROVAL and no `chargeId` is reported; ListAgreementCharges shows no charge for the request. Expected per `expirationDuration`: the request is automatically approved (APPROVED, with the charge its approval creates).
7. Call CancelAgreementPaymentRequest (or RejectAgreementPaymentRequest) on the same request — it succeeds with 200, moving a request that should already be terminal-APPROVED.
