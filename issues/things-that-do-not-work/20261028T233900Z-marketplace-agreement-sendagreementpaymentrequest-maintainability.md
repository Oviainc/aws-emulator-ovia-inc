Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 9f84f44c6767c5c874b7cc6e16fb6469b8a0f678

# Route the store-full refusal of SendAgreementPaymentRequest through the crate's single `store_full` writer

_Filed: 2026-10-28T23:39:00Z_
_Operation: SendAgreementPaymentRequest | Class: maintainability_

> _Note: this issue was identified against the code base at commit `9f84f44c6767c5c874b7cc6e16fb6469b8a0f678`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate has exactly one writer for "this store is full, decline the create with the modelled `ThrottlingException`" — `crate::state::store_full(resource_type, quota)` — and `SendAgreementCancellationRequest` calls it. `SendAgreementPaymentRequest` implements the same rule a second time: it hand-rolls a `ThrequestlingException` literal (`request_id: next_request_id()`, plus its own `message` format) inline in the handler instead of calling the helper. So one condition (a bounded store reached by a create) exists in two copies that can drift: the two sends already disagree on the wording they answer the identical condition with ("this account already holds the maximum of N payment requests" versus store_full's "this emulator already holds the maximum of N <resource> records; release some before creating another"), even though the crate states the opposite principle for exactly this kind of shared refusal — see `inactive_agreement()`'s doc: "One writer, so `CancelAgreement`, `SendAgreementCancellationRequest` and `SendAgreementPaymentRequest` cannot disagree on the class, the status or the member for the same condition."

The defect is the duplication, not the behaviour: the modelled error shape (`ThrottlingException`), the HTTP status (429) and the fact that nothing is stored are identical on both paths, and both existing quota tests assert only the status and the error code, so collapsing the second copy onto `crate::state::store_full(crate::state::RESOURCE_TYPE_PAYMENT_REQUEST, crate::state::PAYMENT_REQUEST_QUOTA)` keeps every response's code/status/shape and both tests unchanged (only the free-text `message` converges onto the one the helper already emits for the sibling send).

## Evidence

Both copies of the one rule, in this crate:

1. The single writer, `src/state.rs:4151-4162`:
```
/// The `ThrottlingException` a create answers when its store already holds the
/// most records the emulator keeps: the request is declined, and nothing is
/// stored, rather than letting state grow without end.
pub fn store_full(resource_type: &str, quota: usize) -> ThrottlingException {
    ThrottlingException {
        request_id: Some(next_request_id()),
        message: Some(format!(
            "this emulator already holds the maximum of {quota} {resource_type} records; \
             release some before creating another"
        )),
    }
}
```

2. The path that calls it — `src/ops/send_agreement_cancellation_request.rs:274-286`:
```
        if store.agreement_cancellation_requests.len()
            >= crate::state::AGREEMENT_CANCELLATION_REQUEST_QUOTA
        {
            return Err(
                SendAgreementCancellationRequestErrorModeled::ThrottlingException(
                    crate::state::store_full(
                        crate::state::RESOURCE_TYPE_AGREEMENT_CANCELLATION_REQUEST,
                        crate::state::AGREEMENT_CANCELLATION_REQUEST_QUOTA,
                    ),
                )
                .into(),
            );
        }
```

3. The path needing it that does not call it — `src/ops/send_agreement_payment_request.rs:281-295`:
```
        if store.payment_requests.len() >= crate::state::PAYMENT_REQUEST_QUOTA {
            return Err(SendAgreementPaymentRequestErrorModeled::ThrottlingException(
                ThrottlingException {
                    // "The unique identifier associated with the error": every
                    // exception shape this service models carries it
                    // (`crate::state::next_request_id`).
                    request_id: Some(crate::state::next_request_id()),
                    message: Some(format!(
                        "this account already holds the maximum of {} payment requests",
                        crate::state::PAYMENT_REQUEST_QUOTA
                    )),
                },
            )
            .into());
        }
```

4. The crate's own single-writer principle for a shared refusal, `src/state.rs:3952-3957`:
```
/// ... One
/// writer, so `CancelAgreement`, `SendAgreementCancellationRequest` and
/// `SendAgreementPaymentRequest` cannot disagree on the class, the status or the
/// member for the same condition.
pub fn inactive_agreement() -> ValidationException {
```
and the two quota constants state the same rule in the same words — `src/state.rs:306-309` ("`SendAgreementPaymentRequest` models no `ServiceQuotaExceededException`, so reaching the bound is answered with the modelled `ThrottlingException`") versus `src/state.rs:318-321` ("`SendAgreementCancellationRequest` models no `ServiceQuotaExceededException`, so a send past the ceiling is refused with its modelled `ThrottlingException` ([`store_full`])").

Test coverage that stays unchanged by the consolidation (neither pins the message):
- `tests/all/payment_requests.rs:506-516` asserts `refused.status == 429` and `refused.error_code() == "ThrottlingException"` only, then `:518-523` asserts the refused send stored nothing.
- `tests/all/agreement_cancellation_requests.rs:693-774` makes the same two assertions for the cancellation send (`:729` `"ThrottlingException"`).

## Steps to Reproduce

1. Read `src/state.rs:4151-4162`: `store_full(resource_type, quota)` is the crate's one constructor of the "store is full" `ThrottlingException`.
2. Grep the crate for its callers: `src/ops/send_agreement_cancellation_request.rs:279` is the only one; `src/ops/send_agreement_payment_request.rs:283` builds a second `ThrottlingException { request_id, message }` for the same condition by hand.
3. Compare the two messages: the payment send answers "this account already holds the maximum of 1000 payment requests", the cancellation send answers "this emulator already holds the maximum of 1000 AgreementCancellationRequest records; release some before creating another" — one condition, two renderings that can drift further.
4. Fill the payment-request store to `PAYMENT_REQUEST_QUOTA` and send one more (as `tests/all/payment_requests.rs:466-523` does): the refusal is a 429 `ThrottlingException` on both paths, so replacing the inline literal with `crate::state::store_full(crate::state::RESOURCE_TYPE_PAYMENT_REQUEST, crate::state::PAYMENT_REQUEST_QUOTA)` keeps that test and the sibling's green.
