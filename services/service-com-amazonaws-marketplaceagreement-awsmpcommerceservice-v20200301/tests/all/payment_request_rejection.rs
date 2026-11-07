// SPDX-License-Identifier: CC0-1.0

//! Rejecting a stored payment request, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the
//! same path a client takes.
//!
//! The rule under test: `RejectAgreementPaymentRequest` acts on the ONE stored
//! payment request the agreement it names holds under that id — it moves that
//! record to `REJECTED`, stores the optional `rejectionReason` as the record's
//! `statusMessage` (absent when the call supplied none), stamps `updatedAt` with
//! the rejecting call while every value the send recorded (and `createdAt`)
//! stays as it was; a request in any other status is refused with a
//! `ConflictException`, a `paymentRequestId` that names no stored request of
//! that agreement with the modelled `ResourceNotFoundException`, an
//! `agreementId` that resolves to no agreement with the modelled input fault on
//! that member (the rule over the whole payment-request family lives in
//! `tests/all/payment_request_agreement_ids.rs`), and nothing in any answer is
//! echoed from the rejecting call's own input. Every later reader
//! — `GetAgreementPaymentRequest` and `ListAgreementPaymentRequests` — reports
//! that same record.
//!
//! The rows span what the rule generalises over: the length of the request's
//! name within its published bounds, the shape of the charge amount, whether the
//! optional description was supplied, whether a rejection reason was supplied
//! and its length at both published bounds of `PaymentRequestRejectionReason`,
//! which status the request is already in, and which id of the required pair
//! names nothing.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::agreement_lifecycle::{call, emulator, Answer};
use super::payment_requests::{
    agreement_with_variable_term, get, is_payment_request_id, items, list, send, sent,
    PENDING_APPROVAL,
};

/// `PaymentRequestStatus`'s value for a rejected request: "Once rejected, the
/// payment request transitions to REJECTED status"
/// (`RejectAgreementPaymentRequest`). Spelled out here rather than imported from
/// the crate so a handler that stores something else fails this test.
const REJECTED: &str = "REJECTED";
/// See [`REJECTED`]: the status the seller's own cancel moves a request to,
/// which is the other terminal decision on the same record.
const CANCELLED: &str = "CANCELLED";

/// The resource kind a lookup names in `resourceType` — a `ResourceType` value
/// (`smithy_query(query="shape", shape="ResourceType")`).
const PAYMENT_REQUEST: &str = "PaymentRequest";

/// The term id every agreement in this suite carries its `VariablePaymentTerm`
/// under — the term a payment request is sent against.
const TERM_ID: &str = "VariablePaymentTerm";

/// Reject one payment request, with the buyer's optional reason when there is
/// one.
async fn reject_payment(
    router: &MultiServiceRouter,
    agreement_id: &str,
    payment_request_id: &str,
    rejection_reason: Option<&str>,
) -> Answer {
    let mut body = pair(agreement_id, payment_request_id);
    if let Some(reason) = rejection_reason {
        body["rejectionReason"] = json!(reason);
    }
    call(router, "RejectAgreementPaymentRequest", body).await
}

/// Cancel one payment request — the seller's decision on the same record, used
/// here only to put a request into a status a reject must refuse.
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

/// The id pair naming one stored payment request, which rejecting and reading it
/// both take.
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

/// One row: a payment request as it was sent, and the reason the buyer rejects
/// it with — so as every later report of it must read.
struct Row {
    case: &'static str,
    /// `PaymentRequestName` is `@length(5, 64)` `^.+$`.
    name: String,
    /// `PositiveAmountUpto8Decimals` is `^[0-9]*(\.[0-9]{0,8})?$`.
    charge_amount: &'static str,
    /// `PaymentRequestDescription` is `@length(1, 2000)`, and optional.
    description: Option<String>,
    /// `PaymentRequestRejectionReason` is `@length(1, 250)`, and optional.
    rejection_reason: Option<String>,
}

/// The table every success case runs over. The member constraints come from
/// `smithy_query(query="operation", operation="RejectAgreementPaymentRequest")`
/// and the amount pattern from `PositiveAmountUpto8Decimals`; the rows differ
/// along the name's length (both published bounds), the shape of the amount
/// (whole, fractional, the eight decimals the pattern admits at most, a trailing
/// point, a long integer part), whether a description was given, and whether a
/// rejection reason was given and at which of its published bounds.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no rejection reason at all, the shortest name and a whole amount",
            name: "March".to_string(),
            charge_amount: "125",
            description: None,
            rejection_reason: None,
        },
        Row {
            case: "the shortest PaymentRequestRejectionReason, 1 character",
            name: "Usage overage for the quarter".to_string(),
            charge_amount: "41.5",
            description: Some("Overage above the committed tier.".to_string()),
            rejection_reason: Some("x".to_string()),
        },
        Row {
            case: "the longest PaymentRequestRejectionReason, 250 characters",
            name: "Metered API calls".to_string(),
            charge_amount: "0.00000001",
            description: Some("d".repeat(2000)),
            rejection_reason: Some("r".repeat(250)),
        },
        Row {
            case: "a sentence of a reason, with a trailing-point amount",
            name: "Rounding".to_string(),
            charge_amount: "7.",
            description: None,
            rejection_reason: Some("The charge does not match the metered usage.".to_string()),
        },
        Row {
            case: "the longest PaymentRequestName, 64 characters, and a long integer part",
            name: format!("n{}", "o".repeat(63)),
            // A long amount that still fits under the ceiling the accepted
            // `VariablePaymentTerm` publishes; a send past that ceiling is
            // refused, which `payment_request_balance.rs` is about.
            charge_amount: "9876543",
            description: Some("x".to_string()),
            rejection_reason: Some("Rejected pending a revised invoice".to_string()),
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
// Rejecting a pending request
// ---------------------------------------------------------------------------

/// Rejecting a `PENDING_APPROVAL` request moves that record to `REJECTED` and
/// reports it from what the send stored: the answer's members are the stored
/// ones, `statusMessage` is the reason this call supplied (and absent when it
/// supplied none), `createdAt` is untouched, `updatedAt` is the rejecting
/// instant, and both readers agree with the answer afterwards.
#[tokio::test]
async fn rejecting_a_pending_request_moves_the_stored_record_to_rejected() {
    let router = emulator();

    for (index, row) in rows().iter().enumerate() {
        let agreement_id =
            agreement_with_variable_term(&router, &format!("ap-prreject{index}"), TERM_ID).await;
        let (payment_request_id, created_at) = send_row(&router, &agreement_id, row).await;
        let case = row.case;

        // Before the reject the request is the one the send documented, and it
        // carries no status message: nothing has decided it.
        let before = get(&router, &agreement_id, &payment_request_id).await;
        assert_eq!(
            before.str_at("/status"),
            PENDING_APPROVAL,
            "case {case:?}: a request nothing has decided is {PENDING_APPROVAL}, got {:?}",
            before.body
        );
        assert!(
            before.body.get("statusMessage").is_none_or(Value::is_null),
            "case {case:?}: an undecided request has no status message, got {:?}",
            before.body
        );

        let rejected = reject_payment(
            &router,
            &agreement_id,
            &payment_request_id,
            row.rejection_reason.as_deref(),
        )
        .await;
        assert_eq!(
            rejected.status, 200,
            "case {case:?}: rejecting must succeed, got {} with {:?}",
            rejected.status, rejected.body
        );
        assert_eq!(
            rejected.str_at("/paymentRequestId"),
            payment_request_id,
            "case {case:?}: the answer names the request that was rejected, got {:?}",
            rejected.body
        );
        assert!(
            is_payment_request_id(&rejected.str_at("/paymentRequestId")),
            "case {case:?}: expected an ^pr-[a-zA-Z0-9]+$ id, got {:?}",
            rejected.body
        );
        assert_eq!(
            rejected.str_at("/agreementId"),
            agreement_id,
            "case {case:?}: the answer names the request's own agreement, got {:?}",
            rejected.body
        );
        assert_eq!(
            rejected.str_at("/status"),
            REJECTED,
            "case {case:?}: a rejected request is {REJECTED}, got {:?}",
            rejected.body
        );
        assert_eq!(
            rejected.body.get("statusMessage").and_then(Value::as_str),
            row.rejection_reason.as_deref(),
            "case {case:?}: statusMessage is the rejection reason exactly when there is one, got \
             {:?}",
            rejected.body
        );
        assert_eq!(
            rejected.str_at("/name"),
            row.name,
            "case {case:?}: the answer reports the stored name, got {:?}",
            rejected.body
        );
        assert_eq!(
            rejected.body.get("description").and_then(Value::as_str),
            row.description.as_deref(),
            "case {case:?}: the answer reports the stored description exactly when there is one, \
             got {:?}",
            rejected.body
        );
        assert_eq!(
            rejected.str_at("/chargeAmount"),
            row.charge_amount,
            "case {case:?}: the answer reports the amount the send recorded, got {:?}",
            rejected.body
        );
        assert_eq!(
            rejected.body["createdAt"].as_f64(),
            Some(created_at),
            "case {case:?}: createdAt stays the instant the request was created, got {:?}",
            rejected.body
        );
        let rejected_at = rejected.body["updatedAt"].as_f64();
        assert!(
            rejected_at.is_some_and(|at| at >= created_at),
            "case {case:?}: updatedAt is the instant of rejection, got {rejected_at:?} against \
             createdAt {created_at}"
        );

        // The reader reports the same record: the transition and the message
        // are persisted, not local to the answer the reject rendered.
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
                REJECTED.to_string(),
                row.name.clone(),
                row.charge_amount.to_string(),
            ),
            "case {case:?}: the reader reports the stored record, got {:?}",
            read.body
        );
        assert_eq!(
            read.body.get("statusMessage").and_then(Value::as_str),
            row.rejection_reason.as_deref(),
            "case {case:?}: the reader reports the stored rejection reason, got {:?}",
            read.body
        );
        assert_eq!(
            (
                read.body["createdAt"].as_f64(),
                read.body["updatedAt"].as_f64()
            ),
            (Some(created_at), rejected_at),
            "case {case:?}: the reader reports the stored timestamps, got {:?}",
            read.body
        );
        assert!(
            read.body.get("chargeId").is_none_or(Value::is_null),
            "case {case:?}: only an approved request has a charge, got {:?}",
            read.body
        );

        // The listing selects on the stored status, so the record MOVED rather
        // than being copied: it is reported under REJECTED and no longer under
        // PENDING_APPROVAL.
        for (status, expected) in [
            (REJECTED, vec![payment_request_id.clone()]),
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

/// A reject acts on ONE request: the other requests of the store, including
/// another request of the same agreement, keep the status and the (absent)
/// status message they had.
#[tokio::test]
async fn rejecting_one_request_leaves_every_other_request_alone() {
    let router = emulator();
    let agreement_id = agreement_with_variable_term(&router, "ap-prrone", TERM_ID).await;
    let other_agreement = agreement_with_variable_term(&router, "ap-prrother", TERM_ID).await;

    let rejected_id = sent(&router, &agreement_id, TERM_ID, "Rejected one", "10").await;
    // A second request over the SAME agreement, plus one over another agreement
    // entirely.
    let sibling = sent(&router, &agreement_id, TERM_ID, "Sibling request", "20").await;
    let elsewhere = sent(&router, &other_agreement, TERM_ID, "Elsewhere one", "30").await;

    let reason = "Charged twice for the same month";
    let answer = reject_payment(&router, &agreement_id, &rejected_id, Some(reason)).await;
    assert_eq!(
        answer.status, 200,
        "rejecting {rejected_id} must succeed, got {} with {:?}",
        answer.status, answer.body
    );

    for (case, agreement, request_id, expected, message) in [
        (
            "the rejected request",
            &agreement_id,
            &rejected_id,
            REJECTED,
            Some(reason),
        ),
        (
            "its sibling over the same agreement",
            &agreement_id,
            &sibling,
            PENDING_APPROVAL,
            None,
        ),
        (
            "a request of another agreement",
            &other_agreement,
            &elsewhere,
            PENDING_APPROVAL,
            None,
        ),
    ] {
        let read = get(&router, agreement, request_id).await;
        assert_eq!(
            read.str_at("/status"),
            expected,
            "case {case:?}: expected {expected}, got {:?}",
            read.body
        );
        assert_eq!(
            read.body.get("statusMessage").and_then(Value::as_str),
            message,
            "case {case:?}: expected statusMessage {message:?}, got {:?}",
            read.body
        );
    }
}

// ---------------------------------------------------------------------------
// A request that is not pending approval
// ---------------------------------------------------------------------------

/// "Only payment requests in PENDING_APPROVAL status can be rejected. A
/// ConflictException is thrown if the payment request is in any other status."
/// The second decision on one request is refused, and the refusal changes
/// nothing: the record keeps the status, the status message and the `updatedAt`
/// the FIRST decision gave it.
///
/// The rows are the whole success table, and each row is run against both ways a
/// request leaves `PENDING_APPROVAL` — the buyer's own reject and the seller's
/// cancel — so the refusal is about the status the record reached and not about
/// which call moved it there or what the request was sent for.
#[tokio::test]
async fn rejecting_a_request_that_is_not_pending_approval_is_a_conflict() {
    let router = emulator();

    for (index, row) in rows().iter().enumerate() {
        for (decision, moved_to) in [("reject", REJECTED), ("cancel", CANCELLED)] {
            let agreement_id = agreement_with_variable_term(
                &router,
                &format!("ap-prragain{index}{decision}"),
                TERM_ID,
            )
            .await;
            let (payment_request_id, _) = send_row(&router, &agreement_id, row).await;
            let case = format!("{}, already {decision}ed", row.case);

            let first = if decision == "reject" {
                reject_payment(
                    &router,
                    &agreement_id,
                    &payment_request_id,
                    row.rejection_reason.as_deref(),
                )
                .await
            } else {
                cancel_payment(&router, &agreement_id, &payment_request_id).await
            };
            assert_eq!(
                first.status, 200,
                "case {case:?}: the first decision must succeed, got {} with {:?}",
                first.status, first.body
            );
            let decided_at = first.body["updatedAt"].as_f64();
            let stored_message = get(&router, &agreement_id, &payment_request_id)
                .await
                .body
                .get("statusMessage")
                .and_then(Value::as_str)
                .map(str::to_string);

            let again = reject_payment(
                &router,
                &agreement_id,
                &payment_request_id,
                Some("A second reason, which must not be stored"),
            )
            .await;
            assert_eq!(
                again.status, 409,
                "case {case:?}: rejecting a {moved_to} request answers HTTP 409, got {} with {:?}",
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
                (
                    read.str_at("/status"),
                    read.body["updatedAt"].as_f64(),
                    read.body
                        .get("statusMessage")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                ),
                (moved_to.to_string(), decided_at, stored_message),
                "case {case:?}: the refused reject left the record as the first decision stored \
                 it, got {:?}",
                read.body
            );
        }
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
/// is the missing `PaymentRequest`. Nothing is written by a refusal, with or
/// without a rejection reason on the refused call.
///
/// The rows span the form of the child id within its modelled pattern, including
/// both published length bounds, and whether it names a stored request of
/// another agreement.
#[tokio::test]
async fn an_id_pair_that_names_no_stored_payment_request_is_not_found() {
    let router = emulator();
    let agreement_id = agreement_with_variable_term(&router, "ap-prrpairs", TERM_ID).await;
    let payment_request_id = sent(&router, &agreement_id, TERM_ID, "Pair probe", "15").await;
    let other_agreement = agreement_with_variable_term(&router, "ap-prrpairs2", TERM_ID).await;
    let other_request = sent(&router, &other_agreement, TERM_ID, "Other probe", "16").await;

    // `PaymentRequestId` is `@length(1, 64)` `^pr-[a-zA-Z0-9]+$`
    // (`smithy_query(query="operation",
    // operation="RejectAgreementPaymentRequest")`), so every child id below
    // satisfies its member's constraints and reaches the lookup, including both
    // published length bounds.
    let rows: Vec<(&str, String, String, &str, String)> = vec![
        (
            "the shortest payment request id the pr- pattern admits",
            agreement_id.clone(),
            "pr-0".to_string(),
            PAYMENT_REQUEST,
            "pr-0".to_string(),
        ),
        (
            "the longest PaymentRequestId, 64 characters",
            agreement_id.clone(),
            format!("pr-{}", "A".repeat(61)),
            PAYMENT_REQUEST,
            format!("pr-{}", "A".repeat(61)),
        ),
        (
            "a payment request that belongs to another agreement",
            agreement_id.clone(),
            other_request.clone(),
            PAYMENT_REQUEST,
            other_request.clone(),
        ),
    ];

    for reason in [None, Some("Not my invoice")] {
        for (case, agreement, request_id, resource_type, resource_id) in &rows {
            let answer = reject_payment(&router, agreement, request_id, reason).await;
            let case = format!("{case} (reason {reason:?})");
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

    // No refused call rejected anything: both stored requests are exactly as
    // they were, and both are still rejectable.
    for (agreement, request_id) in [
        (&agreement_id, &payment_request_id),
        (&other_agreement, &other_request),
    ] {
        let read = get(&router, agreement, request_id).await;
        assert_eq!(
            read.str_at("/status"),
            PENDING_APPROVAL,
            "a refused lookup must not have rejected {request_id}, got {:?}",
            read.body
        );
        let answer = reject_payment(&router, agreement, request_id, None).await;
        assert_eq!(
            (answer.status, answer.str_at("/status")),
            (200, REJECTED.to_string()),
            "{request_id} stayed rejectable after the refused lookups, got {:?}",
            answer.body
        );
    }
}
