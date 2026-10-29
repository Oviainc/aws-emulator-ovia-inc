// SPDX-License-Identifier: CC0-1.0

//! The store-full refusal, end to end over HTTP, for every create that answers
//! it as the modelled `ThrottlingException`.
//!
//! The rule under test: one condition is reported in one body — a create whose
//! store already holds the emulator's maximum of its records is refused with its
//! modelled `ThrottlingException` at HTTP 429, carrying the same message for
//! every such create, naming the maximum and the kind of record (a `ResourceType`
//! value) and what the caller can do about it, with a `requestId` of its own, and
//! nothing is stored. The message differs between two such creates only in the
//! numbers and nouns of the store that filled up, never in its wording.
//!
//! The rows differ along what the rule generalises over: which operation is
//! called, which store fills up, the kind of record it holds, and whether the
//! refused call carries a fresh idempotency token or none — `maximum of 1000
//! PaymentRequest records` and `maximum of 1000 AgreementCancellationRequest
//! records` are two rows of one table, not two wordings.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_cancellation_requests::agreement;
use crate::agreement_lifecycle::{call, emulator, Answer};
use crate::payment_requests::agreement_with_variable_term;

/// The largest page a listing of this service serves (`MaxResults` is
/// `@range(min: 1, max: 50)`), used here only to count a filled store.
const MAX_RESULTS: i64 = 50;

/// How many records each bounded store holds — `state::PAYMENT_REQUEST_QUOTA`
/// and `state::AGREEMENT_CANCELLATION_REQUEST_QUOTA`, spelled out here rather
/// than imported so a handler that stopped refusing at this bound, or reported a
/// different number, fails this test.
const QUOTA: usize = 1_000;

/// The ONE body a store-full refusal carries, spelled out here rather than
/// imported from the crate so a handler that hand-rolled its own wording fails
/// this test. `resource_type` is a `ResourceType` value (from
/// `smithy_query(query="shape", shape="ResourceType")`) and `quota` the maximum
/// the store holds.
fn store_full_message(resource_type: &str, quota: usize) -> String {
    format!(
        "this emulator already holds the maximum of {quota} {resource_type} records; \
         release some before creating another"
    )
}

/// One bounded create: the operation whose store fills up, how to call it, and
/// the kind of record that store holds.
struct Bounded {
    case: &'static str,
    /// The operation under test.
    operation: &'static str,
    /// The `ResourceType` value naming what its store holds.
    resource_type: &'static str,
    /// One send's body, by index so each fill call is distinct where the
    /// operation needs it to be.
    body: fn(&str, usize) -> Value,
    /// The listing that reports the store, and the party the caller is of it.
    listing: &'static str,
    party_type: &'static str,
}

/// The two creates this service refuses with a `ThrottlingException` once their
/// store is full.
///
/// `CreateAgreementRequest` is not a row: it models
/// `ServiceQuotaExceededException` and answers that instead, and
/// `BatchCreateBillingAdjustmentRequest` reports the same condition on its own
/// per-entry `errors` channel rather than as an exception.
const BOUNDED: [Bounded; 2] = [
    Bounded {
        case: "the payment-request store",
        operation: "SendAgreementPaymentRequest",
        resource_type: "PaymentRequest",
        body: payment_request_body,
        listing: "ListAgreementPaymentRequests",
        party_type: "Proposer",
    },
    Bounded {
        case: "the cancellation-request store",
        operation: "SendAgreementCancellationRequest",
        resource_type: "AgreementCancellationRequest",
        body: cancellation_request_body,
        listing: "ListAgreementCancellationRequests",
        party_type: "Acceptor",
    },
];

/// The `VariablePaymentTerm` the payment requests of this suite are sent under.
const TERM_ID: &str = "vpt-store-full";

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

/// `RequestId` is `@length(min: 1, max: 128)` with pattern `^[A-Za-z0-9-]+$`
/// (`smithy_query(query="shape", shape="ThrottlingException")`).
fn is_request_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// How many records `listing` reports, paged to the end.
async fn stored(router: &MultiServiceRouter, listing: &str, party_type: &str) -> usize {
    let mut counted = 0;
    let mut next: Option<String> = None;
    loop {
        let mut body = json!({"partyType": party_type, "maxResults": MAX_RESULTS});
        if let Some(token) = next.as_deref() {
            body["nextToken"] = json!(token);
        }
        let page = call(router, listing, body).await;
        assert_eq!(
            page.status, 200,
            "{listing} must answer the store, got {} with {:?}",
            page.status, page.body
        );
        counted += page.body["items"]
            .as_array()
            .unwrap_or_else(|| panic!("{listing} answers an items array, got {:?}", page.body))
            .len();
        match page.body["nextToken"].as_str() {
            Some(token) => next = Some(token.to_string()),
            None => break,
        }
    }
    counted
}

/// Assert `refused` is the one store-full refusal, worded for `resource_type`.
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
    assert_eq!(
        refused.str_at("/message"),
        store_full_message(resource_type, QUOTA),
        "case {case:?}: expected the one store-full wording, got {:?}",
        refused.body
    );
    assert!(
        is_request_id(&refused.str_at("/requestId")),
        "case {case:?}: expected a RequestId-shaped requestId, got {:?}",
        refused.body
    );
}

/// Every create bounded by the store answers a full store with the SAME body:
/// the one store-full refusal, naming its own maximum and record kind, and
/// stores nothing — whether the refused call carries a fresh idempotency token
/// or none.
#[tokio::test]
async fn every_bounded_create_answers_one_store_full_refusal() {
    let mut messages: Vec<String> = Vec::new();

    for bounded in BOUNDED {
        let Bounded {
            case,
            operation,
            resource_type,
            body,
            listing,
            party_type,
        } = bounded;

        let router = emulator();
        let agreement_id = if operation == "SendAgreementPaymentRequest" {
            agreement_with_variable_term(&router, "ap-storefull", TERM_ID).await
        } else {
            agreement(&router, "ap-storefull").await
        };

        for index in 0..QUOTA {
            let sent = call(&router, operation, body(&agreement_id, index)).await;
            assert_eq!(
                sent.status, 200,
                "case {case:?}: send {index} is within the maximum of {QUOTA}, got {} with {:?}",
                sent.status, sent.body
            );
        }

        for (form, token) in [
            ("no idempotency token", None),
            ("a fresh idempotency token", Some("token-past-the-maximum")),
        ] {
            let mut request = body(&agreement_id, QUOTA);
            if let Some(token) = token {
                request["clientToken"] = json!(token);
            }
            let refused = call(&router, operation, request).await;
            let case = format!("{case} with {form}");
            assert_store_full(&case, resource_type, &refused);
            messages.push(refused.str_at("/message"));

            assert_eq!(
                stored(&router, listing, party_type).await,
                QUOTA,
                "case {case:?}: the refused call stored nothing, so the store still holds \
                 {QUOTA}"
            );
        }
    }

    // One condition, one wording: the rows differ only where the store they
    // report differs.
    let wordings: Vec<String> = messages
        .iter()
        .map(|message| {
            message
                .replace("PaymentRequest", "<kind>")
                .replace("AgreementCancellationRequest", "<kind>")
        })
        .collect();
    assert!(
        wordings.windows(2).all(|pair| pair[0] == pair[1]),
        "every store-full refusal carries one wording, got {wordings:?}"
    );
}
