// SPDX-License-Identifier: CC0-1.0

//! Cancelling a stored payment request, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the
//! same path a client takes.
//!
//! The rule under test: `CancelAgreementPaymentRequest` acts on the ONE stored
//! payment request the agreement it names holds under that id — it moves that
//! record to `CANCELLED`, stamps `updatedAt` with the cancelling call while
//! every value the send recorded (and `createdAt`) stays as it was; a request in
//! any other status is refused with a `ConflictException`, a
//! `paymentRequestId` that names no stored request of that agreement with the
//! modelled `ResourceNotFoundException`, an `agreementId` that resolves to no
//! agreement with the modelled input fault on that member (the rule over the
//! whole payment-request family lives in
//! `tests/all/payment_request_agreement_ids.rs`), and nothing in any answer is
//! echoed from the cancelling call's own input. Every later reader — `GetAgreementPaymentRequest` and
//! `ListAgreementPaymentRequests` — reports that same record.
//!
//! The rows span what the rule generalises over: the length of the request's
//! name within its published bounds, the shape of the charge amount, whether the
//! optional description was supplied, how many requests the store holds and
//! which agreement they belong to, the status the request is already in, and
//! which id of the required pair names nothing.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::agreement_lifecycle::{call, emulator, Answer};
use super::payment_requests::{
    agreement_with_variable_term, get, is_payment_request_id, items, list, send, sent,
    PENDING_APPROVAL,
};

/// `PaymentRequestStatus`'s value for a cancelled request: "Once cancelled, the
/// payment request transitions to CANCELLED status"
/// (`CancelAgreementPaymentRequest`). Spelled out here rather than imported from
/// the crate so a handler that stores something else fails this test.
const CANCELLED: &str = "CANCELLED";

/// The resource kind a lookup names in `resourceType` — a `ResourceType` value
/// (`smithy_query(query="shape", shape="ResourceType")`).
const PAYMENT_REQUEST: &str = "PaymentRequest";

/// The term id every agreement in this suite carries its `VariablePaymentTerm`
/// under — the term a payment request is sent against.
const TERM_ID: &str = "VariablePaymentTerm";

async fn cancel_payment(
    router: &MultiServiceRouter,
    agreement_id: &str,
    payment_request_id: &str,
) -> Answer {
    call(
        router,
        "CancelAgreementPaymentRequest",
        pair(agreement_id, payment_request_id),
    )
    .await
}

/// The id pair naming one stored payment request, which cancelling and reading
/// it both take.
fn pair(agreement_id: &str, payment_request_id: &str) -> Value {
    json!({
        "agreementId": agreement_id,
        "paymentRequestId": payment_request_id,
    })
}

/// The ids one listing reported, in the order it reported them.
fn listed_ids(answer: &Answer) -> Vec<String> {
    items(answer)
        .iter()
        .map(|item| {
            item["paymentRequestId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// One row: a payment request as it was sent, and so as every later report of it
/// must read.
struct Row {
    case: &'static str,
    /// `PaymentRequestName` is `@length(5, 64)` `^.+$`.
    name: String,
    /// `PositiveAmountUpto8Decimals` is `^[0-9]*(\.[0-9]{0,8})?$`.
    charge_amount: &'static str,
    /// `PaymentRequestDescription` is `@length(1, 2000)`, and optional.
    description: Option<String>,
}

/// The table every success case runs over. The member constraints come from
/// `smithy_query(query="operation", operation="CancelAgreementPaymentRequest")`
/// and the amount pattern from `PositiveAmountUpto8Decimals`; the rows differ
/// along the name's length (both published bounds), the shape of the amount
/// (whole, fractional, the eight decimals the pattern admits at most, a trailing
/// point, a long integer part) and whether a description was given.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the shortest PaymentRequestName, 5 characters, a whole amount",
            name: "March".to_string(),
            charge_amount: "125",
            description: None,
        },
        Row {
            case: "a fractional amount with a description",
            name: "Usage overage for the quarter".to_string(),
            charge_amount: "41.5",
            description: Some("Overage above the committed tier.".to_string()),
        },
        Row {
            case: "the eight decimals the pattern admits at most",
            name: "Metered API calls".to_string(),
            charge_amount: "0.00000001",
            description: Some("d".repeat(2000)),
        },
        Row {
            case: "a trailing point, which the pattern admits",
            name: "Rounding".to_string(),
            charge_amount: "7.",
            description: None,
        },
        Row {
            case: "the longest PaymentRequestName, 64 characters, and a long integer part",
            name: format!("n{}", "o".repeat(63)),
            // A long amount that still fits under the ceiling the accepted
            // `VariablePaymentTerm` publishes; a send past that ceiling is
            // refused, which `payment_request_balance.rs` is about.
            charge_amount: "9876543",
            description: Some("x".to_string()),
        },
    ]
}

/// Send one row's payment request under `agreement_id` and answer the id it
/// minted together with the `createdAt` it recorded.
async fn send_row(router: &MultiServiceRouter, agreement_id: &str, row: &Row) -> (String, f64) {
    let mut body = json!({
        "agreementId": agreement_id,
        "termId": TERM_ID,
        "name": row.name,
        "chargeAmount": row.charge_amount,
    });
    if let Some(description) = row.description.as_deref() {
        body["description"] = json!(description);
    }
    let answer = send(router, body).await;
    assert_eq!(
        answer.status, 200,
        "case {:?}: sending the payment request must succeed, got {} with {:?}",
        row.case, answer.status, answer.body
    );
    let created_at = answer.body["createdAt"].as_f64().unwrap_or_else(|| {
        panic!(
            "case {:?}: a sent payment request carries createdAt, got {:?}",
            row.case, answer.body
        )
    });
    (answer.str_at("/paymentRequestId"), created_at)
}

// ---------------------------------------------------------------------------
// Cancelling a pending request
// ---------------------------------------------------------------------------

/// Cancelling a `PENDING_APPROVAL` request moves that record to `CANCELLED` and
/// reports it from what the send stored: the answer's members are the stored
/// ones, `createdAt` is untouched, `updatedAt` is the cancelling instant, and
/// both readers agree with the answer afterwards.
#[tokio::test]
async fn cancelling_a_pending_request_moves_the_stored_record_to_cancelled() {
    let router = emulator();

    for (index, row) in rows().iter().enumerate() {
        let agreement_id =
            agreement_with_variable_term(&router, &format!("ap-prcancel{index}"), TERM_ID).await;
        let (payment_request_id, created_at) = send_row(&router, &agreement_id, row).await;
        let case = row.case;

        // Before the cancel the request is the one the send documented.
        let before = get(&router, &agreement_id, &payment_request_id).await;
        assert_eq!(
            before.str_at("/status"),
            PENDING_APPROVAL,
            "case {case:?}: a request nothing has decided is {PENDING_APPROVAL}, got {:?}",
            before.body
        );

        let cancelled = cancel_payment(&router, &agreement_id, &payment_request_id).await;
        assert_eq!(
            cancelled.status, 200,
            "case {case:?}: cancelling must succeed, got {} with {:?}",
            cancelled.status, cancelled.body
        );
        assert_eq!(
            cancelled.str_at("/paymentRequestId"),
            payment_request_id,
            "case {case:?}: the answer names the request that was cancelled, got {:?}",
            cancelled.body
        );
        assert!(
            is_payment_request_id(&cancelled.str_at("/paymentRequestId")),
            "case {case:?}: expected an ^pr-[a-zA-Z0-9]+$ id, got {:?}",
            cancelled.body
        );
        assert_eq!(
            cancelled.str_at("/agreementId"),
            agreement_id,
            "case {case:?}: the answer names the request's own agreement, got {:?}",
            cancelled.body
        );
        assert_eq!(
            cancelled.str_at("/status"),
            CANCELLED,
            "case {case:?}: a cancelled request is {CANCELLED}, got {:?}",
            cancelled.body
        );
        assert_eq!(
            cancelled.str_at("/name"),
            row.name,
            "case {case:?}: the answer reports the stored name, got {:?}",
            cancelled.body
        );
        assert_eq!(
            cancelled.body.get("description").and_then(Value::as_str),
            row.description.as_deref(),
            "case {case:?}: the answer reports the stored description exactly when there is \
             one, got {:?}",
            cancelled.body
        );
        assert_eq!(
            cancelled.str_at("/chargeAmount"),
            row.charge_amount,
            "case {case:?}: the answer reports the amount the send recorded, got {:?}",
            cancelled.body
        );
        assert_eq!(
            cancelled.body["createdAt"].as_f64(),
            Some(created_at),
            "case {case:?}: createdAt stays the instant the request was created, got {:?}",
            cancelled.body
        );
        let cancelled_at = cancelled.body["updatedAt"].as_f64();
        assert!(
            cancelled_at.is_some_and(|at| at >= created_at),
            "case {case:?}: updatedAt is the instant of cancellation, got {cancelled_at:?} \
             against createdAt {created_at}"
        );

        // The reader reports the same record: the transition is persisted, not
        // local to the answer the cancel rendered.
        let read = get(&router, &agreement_id, &payment_request_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: reading the request back must succeed, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            (
                read.str_at("/paymentRequestId"),
                read.str_at("/agreementId"),
                read.str_at("/status"),
                read.str_at("/name"),
                read.str_at("/chargeAmount"),
            ),
            (
                payment_request_id.clone(),
                agreement_id.clone(),
                CANCELLED.to_string(),
                row.name.clone(),
                row.charge_amount.to_string(),
            ),
            "case {case:?}: the reader reports the stored record, got {:?}",
            read.body
        );
        assert_eq!(
            (
                read.body["createdAt"].as_f64(),
                read.body["updatedAt"].as_f64()
            ),
            (Some(created_at), cancelled_at),
            "case {case:?}: the reader reports the stored timestamps, got {:?}",
            read.body
        );
        assert!(
            read.body.get("chargeId").is_none_or(Value::is_null),
            "case {case:?}: only an approved request has a charge, got {:?}",
            read.body
        );
        assert!(
            read.body.get("statusMessage").is_none_or(Value::is_null),
            "case {case:?}: cancelling carries no reason member, so no message was stored, got \
             {:?}",
            read.body
        );

        // The listing selects on the stored status, so the record MOVED rather
        // than being copied: it is reported under CANCELLED and no longer under
        // PENDING_APPROVAL.
        for (status, expected) in [
            (CANCELLED, vec![payment_request_id.clone()]),
            (PENDING_APPROVAL, Vec::new()),
        ] {
            let listed = list(
                &router,
                json!({
                    "partyType": "Acceptor",
                    "agreementId": agreement_id,
                    "status": status,
                }),
            )
            .await;
            assert_eq!(
                listed_ids(&listed),
                expected,
                "case {case:?}: the {status} listing must report {expected:?}, got {:?}",
                listed.body
            );
        }
    }
}

/// A cancel acts on ONE request: the other requests of the store, including
/// another request of the same agreement, keep the status they had.
#[tokio::test]
async fn cancelling_one_request_leaves_every_other_request_alone() {
    let router = emulator();
    let agreement_id = agreement_with_variable_term(&router, "ap-prone", TERM_ID).await;
    let other_agreement = agreement_with_variable_term(&router, "ap-prother", TERM_ID).await;

    let cancelled_id = sent(&router, &agreement_id, TERM_ID, "Cancelled one", "10").await;
    // A second request over the SAME agreement, plus one over another agreement
    // entirely.
    let sibling = sent(&router, &agreement_id, TERM_ID, "Sibling request", "20").await;
    let elsewhere = sent(&router, &other_agreement, TERM_ID, "Elsewhere one", "30").await;

    let answer = cancel_payment(&router, &agreement_id, &cancelled_id).await;
    assert_eq!(
        answer.status, 200,
        "cancelling {cancelled_id} must succeed, got {} with {:?}",
        answer.status, answer.body
    );

    for (case, agreement, request_id, expected) in [
        ("the cancelled request", &agreement_id, &cancelled_id, CANCELLED),
        (
            "its sibling over the same agreement",
            &agreement_id,
            &sibling,
            PENDING_APPROVAL,
        ),
        (
            "a request of another agreement",
            &other_agreement,
            &elsewhere,
            PENDING_APPROVAL,
        ),
    ] {
        let read = get(&router, agreement, request_id).await;
        assert_eq!(
            read.str_at("/status"),
            expected,
            "case {case:?}: expected {expected}, got {:?}",
            read.body
        );
    }
}

// ---------------------------------------------------------------------------
// A request that is not pending approval
// ---------------------------------------------------------------------------

/// "Only payment requests in PENDING_APPROVAL status can be cancelled. A
/// ConflictException is thrown if the payment request is in any other status."
/// The second cancel of one request is refused, and the refusal changes nothing:
/// the record keeps the status and the `updatedAt` the FIRST cancel gave it.
///
/// The rows are the whole success table, so the refusal is about the status the
/// record reached and not about what the request was sent for.
#[tokio::test]
async fn cancelling_a_request_that_is_not_pending_approval_is_a_conflict() {
    let router = emulator();

    for (index, row) in rows().iter().enumerate() {
        let agreement_id =
            agreement_with_variable_term(&router, &format!("ap-pragain{index}"), TERM_ID).await;
        let (payment_request_id, _) = send_row(&router, &agreement_id, row).await;
        let case = row.case;

        let first = cancel_payment(&router, &agreement_id, &payment_request_id).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first cancel must succeed, got {} with {:?}",
            first.status, first.body
        );
        let cancelled_at = first.body["updatedAt"].as_f64();

        let again = cancel_payment(&router, &agreement_id, &payment_request_id).await;
        assert_eq!(
            again.status, 409,
            "case {case:?}: cancelling a {CANCELLED} request answers HTTP 409, got {} with {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.error_code(),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {:?}",
            again.body
        );
        assert_eq!(
            again.str_at("/resourceType"),
            PAYMENT_REQUEST,
            "case {case:?}: the conflict names the resource kind it refused, got {:?}",
            again.body
        );
        assert_eq!(
            again.str_at("/resourceId"),
            payment_request_id,
            "case {case:?}: the conflict names the request it refused, got {:?}",
            again.body
        );
        assert!(
            !again.str_at("/message").is_empty(),
            "case {case:?}: a refusal must say what was wrong, got {:?}",
            again.body
        );

        let read = get(&router, &agreement_id, &payment_request_id).await;
        assert_eq!(
            (read.str_at("/status"), read.body["updatedAt"].as_f64()),
            (CANCELLED.to_string(), cancelled_at),
            "case {case:?}: the refused cancel left the record as the first one stored it, got \
             {:?}",
            read.body
        );
    }
}

// ---------------------------------------------------------------------------
// The id pair
// ---------------------------------------------------------------------------

/// The required id pair is resolved in the class each id belongs to: an
/// `agreementId` that resolves to no agreement is the input fault on that member
/// (`payment_request_agreement_ids` spans that half of the rule over every
/// operation on a payment request), and a `paymentRequestId` that names no
/// request of THAT agreement — including one that belongs to another agreement —
/// is the missing `PaymentRequest`. Nothing is written by a refusal.
///
/// The rows span the form of the child id within its modelled pattern (both
/// published length bounds), whether it names a stored request of another
/// agreement, and which of the two operations that resolve the pair is called.
#[tokio::test]
async fn an_id_pair_that_names_no_stored_payment_request_is_not_found() {
    let router = emulator();
    let agreement_id = agreement_with_variable_term(&router, "ap-prpairs", TERM_ID).await;
    let payment_request_id = sent(&router, &agreement_id, TERM_ID, "Pair probe", "15").await;
    let other_agreement = agreement_with_variable_term(&router, "ap-prpairs2", TERM_ID).await;
    let other_request = sent(&router, &other_agreement, TERM_ID, "Other probe", "16").await;

    // `PaymentRequestId` is `@length(1, 64)` `^pr-[a-zA-Z0-9]+$`
    // (`smithy_query(query="operation",
    // operation="CancelAgreementPaymentRequest,GetAgreementPaymentRequest")`),
    // so every child id below satisfies its member's constraints and reaches
    // the lookup, including both published length bounds.
    let rows: Vec<(&str, Value, &str, String)> = vec![
        (
            "the shortest payment request id the pr- pattern admits",
            pair(&agreement_id, "pr-0"),
            PAYMENT_REQUEST,
            "pr-0".to_string(),
        ),
        (
            "the longest PaymentRequestId, 64 characters",
            pair(&agreement_id, &format!("pr-{}", "A".repeat(61))),
            PAYMENT_REQUEST,
            format!("pr-{}", "A".repeat(61)),
        ),
        (
            "a payment request that belongs to another agreement",
            pair(&agreement_id, &other_request),
            PAYMENT_REQUEST,
            other_request.clone(),
        ),
    ];

    for operation in [
        "CancelAgreementPaymentRequest",
        "GetAgreementPaymentRequest",
    ] {
        for (case, body, resource_type, resource_id) in &rows {
            let answer = call(&router, operation, body.clone()).await;
            let case = format!("{operation}: {case}");
            assert_eq!(
                answer.status, 400,
                "case {case:?}: a lookup that found nothing answers HTTP 400, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.str_at("/resourceType"),
                *resource_type,
                "case {case:?}: expected resourceType {resource_type:?}, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.str_at("/resourceId"),
                *resource_id,
                "case {case:?}: the refusal names the id that was looked for, got {:?}",
                answer.body
            );
            assert!(
                !answer.str_at("/message").is_empty(),
                "case {case:?}: a refusal must say what was wrong, got {:?}",
                answer.body
            );
        }
    }

    // No refused call cancelled anything: both stored requests are exactly as
    // they were, and both are still cancellable.
    for (agreement, request_id) in [
        (&agreement_id, &payment_request_id),
        (&other_agreement, &other_request),
    ] {
        let read = get(&router, agreement, request_id).await;
        assert_eq!(
            read.str_at("/status"),
            PENDING_APPROVAL,
            "a refused lookup must not have cancelled {request_id}, got {:?}",
            read.body
        );
        let answer = cancel_payment(&router, agreement, request_id).await;
        assert_eq!(
            (answer.status, answer.str_at("/status")),
            (200, CANCELLED.to_string()),
            "{request_id} stayed cancellable after the refused lookups, got {:?}",
            answer.body
        );
    }
}
