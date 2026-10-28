Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: db5b8cf4b40010fa5fa0137914026d79724270f1

# The store-full refusal is hand-rolled instead of going through state::store_full, which the sibling send uses

_Filed: 2026-10-28T20:09:07Z_
_Operation: SendAgreementPaymentRequest | Class: error-response_

> _Note: this issue was identified against the code base at commit `db5b8cf4b40010fa5fa0137914026d79724270f1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two sibling creates in this crate refuse the SAME condition — "the store already holds the emulator's maximum number of records of this kind" — with the SAME modelled shape (`ThrottlingException`). `SendAgreementCancellationRequest` builds it through `crate::state::store_full`, which state.rs documents as the one construction of that refusal ("The `ThrottlingException` a create answers when its store already holds the most records the emulator keeps"). `SendAgreementPaymentRequest` skips that existing helper and constructs the exception inline, so the one condition is reported to clients in two different bodies: `store_full` says "this emulator already holds the maximum of {quota} {resource_type} records; release some before creating another", while the payment-request path says "this account already holds the maximum of {n} payment requests" — a different subject ("account" vs "emulator"), no resource-type noun, and no remediation clause. Nothing about a payment request makes `store_full` unusable here: the shape, the `requestId` source and the quota argument are identical, and the crate even has `RESOURCE_TYPE_PAYMENT_REQUEST` for the noun it would pass. (The other two bounded creates genuinely cannot use it: `CreateAgreementRequest` answers a different modelled shape, `ServiceQuotaExceededException`, and `BatchCreateBillingAdjustmentRequest` reports per-entry on the batch `errors` channel rather than as an exception.)

## Evidence

The helper and its "one construction" claim — services/service-com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301/src/state.rs:4040-4051:
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

The sibling that calls it — src/ops/send_agreement_cancellation_request.rs:274-286:
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

The path that needs it and does not call it — src/ops/send_agreement_payment_request.rs:281-295:
```
        if store.payment_requests.len() >= crate::state::PAYMENT_REQUEST_QUOTA {
            return Err(SendAgreementPaymentRequestErrorModeled::ThrottlingException(
                ThrottlingException {
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

Test coverage: both bounds are exercised end to end but only on the error CODE and on "nothing was stored", not on the message — tests/all/payment_requests.rs:466-522 ("expected ThrottlingException, got {:?}" / "the refused send stored nothing, so the store still holds {QUOTA}") and tests/all/agreement_cancellation_requests.rs:693-774 ("case {case:?}: expected a ThrottlingException, got {:?}"). So routing the payment-request refusal through `store_full` leaves both suites passing.

## Steps to Reproduce

1. Fill the cancellation-request store to `state::AGREEMENT_CANCELLATION_REQUEST_QUOTA` (1000) and call SendAgreementCancellationRequest once more: the 429 body reads "this emulator already holds the maximum of 1000 AgreementCancellationRequest records; release some before creating another" (built by state::store_full).
2. Fill the payment-request store to `state::PAYMENT_REQUEST_QUOTA` (1000) and call SendAgreementPaymentRequest once more: the 429 body reads "this account already holds the maximum of 1000 payment requests" — the same condition and the same modelled shape, built inline instead of by the crate's single store-full helper.
3. Observe that nothing in src/ops/send_agreement_payment_request.rs explains the divergence: the comment above it ("The store is bounded: past its bound a send is refused rather than adding another record … so the refusal is its modelled `ThrottlingException`") states exactly the condition `state::store_full` exists for.
