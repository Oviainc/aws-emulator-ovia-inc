Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 1211f7611505016a47ab1bf8932fdc778b7fd399

# The per-entry quota refusal is hand-rolled, so its message carries none of the ValidationExceptionReason its four sibling validation entries carry

_Filed: 2026-10-29T01:46:20Z_
_Operation: BatchCreateBillingAdjustmentRequest | Class: error-response_

> _Note: this issue was identified against the code base at commit `1211f7611505016a47ab1bf8932fdc778b7fd399`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Inside one loop of `BatchCreateBillingAdjustmentRequest`, every per-entry refusal coded `VALIDATION_EXCEPTION` is worded through the crate's single per-entry refusal writer, `crate::state::billing_adjustment_entry_message(reason, detail)`, whose whole purpose is to put the modelled `ValidationExceptionReason` a client branches on into the `message` — because `BatchCreateBillingAdjustmentError` has no `reason` member. Four entries do this (invalid amount, non-decimal amount, wrong currency, exceeded maximum adjustment). The fifth, the store-quota refusal, builds its `message` with a bare `format!` and therefore ships the same `VALIDATION_EXCEPTION` code with NO reason prefix at all.

So a client reading the per-entry `errors` list gets a machine-readable reason for four of the five validation refusals of this one call and nothing for the fifth: the same channel reports two different message shapes for the same error code, and the one place the reason could have been stated is empty. This is the same defect class the crate already carries for the sibling send (`issues/20261028T200907Z-marketplace-agreement-sendagreementpaymentrequest-error-response.md`, open: "The store-full refusal is hand-rolled instead of going through state::store_full, which the sibling send uses"), but on a different operation and a different writer, and it is the only `VALIDATION_EXCEPTION` entry of this handler that bypasses the writer.

## Evidence

The writer's own stated rule — `services/.../src/state.rs:736-744`:
```
/// How a per-entry refusal of `BatchCreateBillingAdjustmentRequest` states the
/// rule the entry broke: the modelled `ValidationExceptionReason` a client
/// branches on, then what about the entry broke it.
///
/// `BatchCreateBillingAdjustmentError` carries only a `code`, a `message` and
/// the entry's `clientToken` — no `reason` member — so the reason is named in
/// the message, which is the only place left for it.
pub fn billing_adjustment_entry_message(reason: &str, detail: impl AsRef<str>) -> String {
    format!("{reason}: {}", detail.as_ref())
}
```

A sibling entry in the same loop that honours it — `services/.../src/ops/batch_create_billing_adjustment_request.rs:286-299`:
```
            if !crate::state::is_acceptable_amount(&entry.adjustment_amount) {
                errors.push(BatchCreateBillingAdjustmentError {
                    code: crate::state::BILLING_ADJUSTMENT_ERROR_VALIDATION.to_string(),
                    message: crate::state::billing_adjustment_entry_message(
                        crate::state::REASON_INVALID_ADJUSTMENT_AMOUNT,
```
(the same shape at lines 324-332 `REASON_INVALID_CURRENCY_CODE`, 344-352 `REASON_INVALID_ADJUSTMENT_AMOUNT`, 355-363 `REASON_EXCEEDED_MAXIMUM_ADJUSTMENT_AMOUNT`)

The one that does not — `services/.../src/ops/batch_create_billing_adjustment_request.rs:374-386`:
```
            if store.billing_adjustment_requests.len()
                >= crate::state::BILLING_ADJUSTMENT_REQUEST_QUOTA
            {
                errors.push(BatchCreateBillingAdjustmentError {
                    code: crate::state::BILLING_ADJUSTMENT_ERROR_VALIDATION.to_string(),
                    message: format!(
                        "this account already holds the maximum of {} billing adjustment requests",
                        crate::state::BILLING_ADJUSTMENT_REQUEST_QUOTA
                    ),
```
No reason prefix, where every sibling has one.

Test coverage: the per-entry error channel is exercised by `services/.../tests/all/billing_adjustments.rs` and `tests/all/invoice_adjustments.rs`; neither drives the 1 000-record quota branch (`state.rs:294 pub const BILLING_ADJUSTMENT_REQUEST_QUOTA: usize = 1_000;`), so nothing pins the hand-rolled wording.

## Steps to Reproduce

1. Read `services/service-com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301/src/state.rs:736-744` — `billing_adjustment_entry_message` is the crate's single writer of a per-entry refusal message, and its doc says the reason must be named in the message because the error shape has no `reason` member.
2. Read `src/ops/batch_create_billing_adjustment_request.rs:286-299`, `:324-332`, `:344-352`, `:355-363` — all four `VALIDATION_EXCEPTION` entries call that writer with a modelled `ValidationExceptionReason`.
3. Read `src/ops/batch_create_billing_adjustment_request.rs:374-386` — the quota entry emits the same `VALIDATION_EXCEPTION` code with a `format!` message that names no reason.
4. Fill the store to `BILLING_ADJUSTMENT_REQUEST_QUOTA` (state.rs:294) and submit one more entry: the answer is 200 with an `errors[0].code == "VALIDATION_EXCEPTION"` whose `message` a client cannot parse a reason out of, unlike every other validation entry of the same operation.
