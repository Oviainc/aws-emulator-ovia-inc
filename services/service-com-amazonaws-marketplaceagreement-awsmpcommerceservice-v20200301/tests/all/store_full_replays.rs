// SPDX-License-Identifier: CC0-1.0

//! The store-full refusal at the boundary between a create and a retry, end to
//! end over HTTP, for both creates that answer it as the modelled
//! `ThrottlingException`.
//!
//! The rule under test: the one store-full refusal answers a call the full store
//! cannot take — a call that would add a record — so it is never the answer to a
//! call that only reports a record already stored. A send whose
//! `@idempotencyToken` names a stored record keeps answering that record, with
//! its own id and the status it was stored in, after its store has filled up,
//! while every send that would store a new record is refused with the modelled
//! `ThrottlingException` at HTTP 429, and the retry leaves the bound where it
//! was.
//!
//! The rows differ along what the rule generalises over: which operation is
//! called, which store fills up, the kind of record it holds and the member its
//! id is reported in. The exact wording of the refusal is pinned by
//! `store_full_refusals`; what this suite adds is which calls reach that one
//! refusal once the store is full.

use serde_json::{json, Value};

use crate::agreement_cancellation_requests::agreement;
use crate::agreement_lifecycle::{call, emulator, Answer};
use crate::payment_requests::agreement_with_variable_term;

/// How many records each bounded store holds — `state::PAYMENT_REQUEST_QUOTA`
/// and `state::AGREEMENT_CANCELLATION_REQUEST_QUOTA`, spelled out here rather
/// than imported so a handler that stopped refusing at this bound, or reported a
/// different number, fails this test.
const QUOTA: usize = 1_000;

/// The status both bounded creates store a new record in — `PENDING_APPROVAL` of
/// `PaymentRequestStatus` and of `AgreementCancellationRequestStatus`
/// (`smithy_query(query="shape", shape="PaymentRequestStatus")`), which is what a
/// retry of a stored create still reports.
const PENDING_APPROVAL: &str = "PENDING_APPROVAL";

/// The `clientToken` the first of the records that fill a store is sent with, so
/// that create is replayable once the store is full.
const REPLAY_TOKEN: &str = "token-of-the-first-record";

/// The operation whose store this suite fills, named here because the agreement a
/// payment request is sent under needs a `VariablePaymentTerm` and a cancellation
/// request does not.
const PAYMENT_SEND: &str = "SendAgreementPaymentRequest";

/// The `VariablePaymentTerm` the payment requests of this suite are sent under.
const TERM_ID: &str = "vpt-store-full-replay";

/// One bounded create: the operation whose store fills up, the kind of record
/// that store holds, the member its id is reported in, and how to call it.
struct Bounded {
    case: &'static str,
    operation: &'static str,
    /// The `ResourceType` value naming what its store holds, which the refusal
    /// names (`smithy_query(query="shape", shape="ResourceType")`).
    resource_type: &'static str,
    /// Where the create reports the id of the record it stored.
    id_member: &'static str,
    /// One send's body, by index so each fill call is distinct where the
    /// operation needs it to be.
    body: fn(&str, usize) -> Value,
}

/// The two creates this service refuses with a `ThrottlingException` once their
/// store is full — the same pair `store_full_refusals` spans, each of which
/// reaches that refusal through the crate's single writer of it.
///
/// `CreateAgreementRequest` is not a row: it models
/// `ServiceQuotaExceededException` and answers that instead, and
/// `BatchCreateBillingAdjustmentRequest` reports the same condition on its own
/// per-entry `errors` channel rather than as an exception.
const BOUNDED: [Bounded; 2] = [
    Bounded {
        case: "the payment-request store",
        operation: PAYMENT_SEND,
        resource_type: "PaymentRequest",
        id_member: "/paymentRequestId",
        body: payment_request_body,
    },
    Bounded {
        case: "the cancellation-request store",
        operation: "SendAgreementCancellationRequest",
        resource_type: "AgreementCancellationRequest",
        id_member: "/agreementCancellationRequestId",
        body: cancellation_request_body,
    },
];

fn payment_request_body(agreement_id: &str, index: usize) -> Value {
    json!({
        "agreementId": agreement_id,
        "termId": TERM_ID,
        "name": format!("Bounded payment {index}"),
        "chargeAmount": "1",
    })
}

fn cancellation_request_body(agreement_id: &str, _index: usize) -> Value {
    json!({"agreementId": agreement_id, "reasonCode": "OTHER"})
}

/// Assert `refused` is the one store-full refusal: the modelled
/// `ThrottlingException`, at its modelled status, naming the maximum the store
/// holds and the kind of record it holds.
fn assert_store_full(case: &str, resource_type: &str, refused: &Answer) {
    assert_eq!(
        refused.status, 429,
        "case {case:?}: a full store is refused at HTTP 429, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ThrottlingException",
        "case {case:?}: expected ThrottlingException, got {:?}",
        refused.body
    );
    let message = refused.str_at("/message");
    assert!(
        message.contains(resource_type) && message.contains(&QUOTA.to_string()),
        "case {case:?}: the refusal names the maximum {QUOTA} and the {resource_type} records \
         it holds, got {message:?}"
    );
    assert!(
        !refused.str_at("/requestId").is_empty(),
        "case {case:?}: expected a requestId on the refusal, got {:?}",
        refused.body
    );
}

/// A full store refuses every send that would store another record — through the
/// one store-full refusal — and still answers a send whose idempotency token
/// names a record already stored, from that record.
#[tokio::test]
async fn a_full_store_refuses_a_create_and_still_answers_a_retry() {
    for bounded in BOUNDED {
        let Bounded {
            case,
            operation,
            resource_type,
            id_member,
            body,
        } = bounded;

        let router = emulator();
        let agreement_id = if operation == PAYMENT_SEND {
            agreement_with_variable_term(&router, "ap-storefullreplay", TERM_ID).await
        } else {
            agreement(&router, "ap-storefullreplay").await
        };

        // The first of the records that fill the store carries an idempotency
        // token, so that create — and only that one — is replayable below.
        let first = {
            let mut request = body(&agreement_id, 0);
            request["clientToken"] = json!(REPLAY_TOKEN);
            request
        };
        let stored = call(&router, operation, first.clone()).await;
        assert_eq!(
            stored.status, 200,
            "case {case:?}: the first send is within the maximum of {QUOTA}, got {} with {:?}",
            stored.status, stored.body
        );
        let stored_id = stored.str_at(id_member);
        assert!(
            !stored_id.is_empty(),
            "case {case:?}: the first send reports the id it stored at {id_member}, got {:?}",
            stored.body
        );

        for index in 1..QUOTA {
            let sent = call(&router, operation, body(&agreement_id, index)).await;
            assert_eq!(
                sent.status, 200,
                "case {case:?}: send {index} is within the maximum of {QUOTA}, got {} with {:?}",
                sent.status, sent.body
            );
        }

        // A send that would store record number QUOTA + 1 is the call the full
        // store cannot take.
        let refused = call(&router, operation, body(&agreement_id, QUOTA)).await;
        assert_store_full(&format!("{case}: a create past the maximum"), resource_type, &refused);

        // The retry of the stored create reports that record, not the refusal:
        // nothing new would be stored, so the bound is not what answers it.
        let replayed = call(&router, operation, first).await;
        assert_eq!(
            replayed.status, 200,
            "case {case:?}: a retried create is answered from the record its token names even at \
             the maximum of {QUOTA}, got {} with {:?}",
            replayed.status, replayed.body
        );
        assert_eq!(
            replayed.str_at(id_member),
            stored_id,
            "case {case:?}: the retry reports the record the token already named, got {:?}",
            replayed.body
        );
        assert_eq!(
            replayed.str_at("/status"),
            PENDING_APPROVAL,
            "case {case:?}: the retry reports the status the record was stored in, got {:?}",
            replayed.body
        );

        // And the retry stored nothing, so the next create is still refused —
        // with the same one refusal, whether or not it carries a token of its
        // own.
        for (form, token) in [
            ("no idempotency token", None),
            ("a fresh idempotency token", Some("token-after-the-retry")),
        ] {
            let mut request = body(&agreement_id, QUOTA + 1);
            if let Some(token) = token {
                request["clientToken"] = json!(token);
            }
            let refused_again = call(&router, operation, request).await;
            assert_store_full(
                &format!("{case}: a create after the retry, with {form}"),
                resource_type,
                &refused_again,
            );
        }
    }
}
