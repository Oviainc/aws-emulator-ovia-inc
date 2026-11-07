// SPDX-License-Identifier: CC0-1.0

//! Approving a stored payment request, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the
//! same path a client takes.
//!
//! The rule under test: `AcceptAgreementPaymentRequest` acts on the ONE stored
//! payment request the agreement it names holds under that id — it moves that
//! record to `APPROVED`, creates the charge that approval implies and records
//! its identity on the record, associates the buyer's optional
//! `purchaseOrderReference` with that charge (and associates nothing when the
//! call supplied none), stamps `updatedAt` with the approving call while every
//! value the send recorded (and `createdAt`) stays as it was; a request in any
//! other status is refused with a `ConflictException`, a `paymentRequestId`
//! that names no stored request of that agreement with the modelled
//! `ResourceNotFoundException`, an `agreementId` that resolves to no agreement
//! with the modelled input fault on that member (the rule over the whole
//! payment-request family lives in
//! `tests/all/payment_request_agreement_ids.rs`), and nothing in any answer is
//! echoed from the approving call's own input. Every later reader —
//! `GetAgreementPaymentRequest`, `ListAgreementPaymentRequests` and
//! `ListAgreementCharges` — reports that same record.
//!
//! The rows span what the rule generalises over: the length of the request's
//! name within its published bounds, the shape of the charge amount, whether the
//! optional description was supplied, whether a purchase order reference was
//! supplied and its form and length (including the published lower bound of
//! `PurchaseOrderReference`), which status the request is already in and which
//! decision put it there, and which id of the required pair names nothing.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_lifecycle::{call, emulator, Answer};
use crate::payment_requests::{
    agreement_with_variable_term, get, is_payment_request_id, items, list, send, sent,
    PENDING_APPROVAL,
};
use crate::purchase_order_associations::order;
use crate::purchase_order_updates::{assert_unsupported_action, charges, items as charge_items, naming_agreement};

/// `PaymentRequestStatus`'s value for an approved request: "Once accepted, the
/// payment request transitions to APPROVED status"
/// (`AcceptAgreementPaymentRequest`). Spelled out here rather than imported from
/// the crate so a handler that stores something else fails this test.
const APPROVED: &str = "APPROVED";
/// See [`APPROVED`]: the other two terminal decisions on the same record — the
/// buyer's reject and the seller's cancel.
const REJECTED: &str = "REJECTED";
/// See [`APPROVED`].
const CANCELLED: &str = "CANCELLED";

/// The resource kind a lookup names in `resourceType` — a `ResourceType` value
/// (`smithy_query(query="shape", shape="ResourceType")`).
const PAYMENT_REQUEST: &str = "PaymentRequest";

/// The `agreementType` every agreement this service mints carries, which is what
/// `Charge$agreementType` reports for a charge of one.
const AGREEMENT_TYPE: &str = "PurchaseAgreement";

/// A charge is minted once and never revised, so a reported charge sits at
/// `ChargeRevision`'s first value (`@range(min: 1)`,
/// `smithy_query(query="shape", shape="ChargeRevision")`).
const INITIAL_REVISION: i64 = 1;

/// The term id every agreement in this suite carries its `VariablePaymentTerm`
/// under — the term a payment request is sent against.
const TERM_ID: &str = "VariablePaymentTerm";

/// `ChargeId`'s `^ch-[a-zA-Z0-9]+$`, checked here rather than through the
/// crate's own code so a wrong minter cannot make this test pass.
fn is_charge_id(value: &str) -> bool {
    value
        .strip_prefix("ch-")
        .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// The id pair naming one stored payment request, which approving and reading it
/// both take.
fn pair(agreement_id: &str, payment_request_id: &str) -> Value {
    json!({
        "agreementId": agreement_id,
        "paymentRequestId": payment_request_id,
    })
}

/// Approve one payment request, with the buyer's optional purchase order
/// reference when there is one.
async fn accept_payment(
    router: &MultiServiceRouter,
    agreement_id: &str,
    payment_request_id: &str,
    purchase_order_reference: Option<&str>,
) -> Answer {
    let mut body = pair(agreement_id, payment_request_id);
    if let Some(reference) = purchase_order_reference {
        body["purchaseOrderReference"] = json!(reference);
    }
    call(router, "AcceptAgreementPaymentRequest", body).await
}

/// Reject one payment request — the buyer's other decision on the same record,
/// used here only to put a request into a status an approval must refuse.
async fn reject_payment(
    router: &MultiServiceRouter,
    agreement_id: &str,
    payment_request_id: &str,
) -> Answer {
    call(
        router,
        "RejectAgreementPaymentRequest",
        pair(agreement_id, payment_request_id),
    )
    .await
}

/// Cancel one payment request — the seller's decision on the same record, used
/// here only to put a request into a status an approval must refuse.
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

/// The ids one payment request listing reported, in the order it reported them.
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

/// Every charge `ListAgreementCharges` reports for `agreement_id`, keyed by
/// charge id.
async fn reported_charges(router: &MultiServiceRouter, agreement_id: &str) -> Vec<(String, Value)> {
    let reported = charges(router, json!({"agreementId": agreement_id})).await;
    assert_eq!(
        reported.status, 200,
        "listing the charges of {agreement_id} must succeed, got {} with {:?}",
        reported.status, reported.body
    );
    charge_items(&reported)
        .into_iter()
        .map(|charge| {
            (
                charge["id"].as_str().unwrap_or_default().to_string(),
                charge,
            )
        })
        .collect()
}

/// The charge ids `ListAgreementCharges` reports for `agreement_id`.
async fn reported_charge_ids(router: &MultiServiceRouter, agreement_id: &str) -> Vec<String> {
    reported_charges(router, agreement_id)
        .await
        .into_iter()
        .map(|(id, _)| id)
        .collect()
}

/// The charge `charge_id` of `agreement_id`, as the report carries it.
async fn reported_charge(
    router: &MultiServiceRouter,
    agreement_id: &str,
    charge_id: &str,
) -> Option<Value> {
    reported_charges(router, agreement_id)
        .await
        .into_iter()
        .find(|(id, _)| id == charge_id)
        .map(|(_, charge)| charge)
}

/// One row: a payment request as it was sent, and the purchase order reference
/// the buyer approves it with — so as every later report of it must read.
struct Row {
    case: &'static str,
    /// `PaymentRequestName` is `@length(5, 64)` `^.+$`.
    name: String,
    /// `PositiveAmountUpto8Decimals` is `^[0-9]*(\.[0-9]{0,8})?$`.
    charge_amount: &'static str,
    /// `PaymentRequestDescription` is `@length(1, 2000)`, and optional.
    description: Option<String>,
    /// `PurchaseOrderReference` is `@length(min: 1)`, and optional.
    purchase_order_reference: Option<String>,
}

/// The table every success case runs over. The member constraints come from
/// `smithy_query(query="operation", operation="AcceptAgreementPaymentRequest")`
/// and the amount pattern from `PositiveAmountUpto8Decimals`; the rows differ
/// along the name's length (both published bounds), the shape of the amount
/// (whole, fractional, the eight decimals the pattern admits at most, a trailing
/// point, a long integer part), whether a description was given, and whether a
/// purchase order reference was given, in what form and at which length —
/// including the one-character lower bound `PurchaseOrderReference` publishes.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "no purchase order reference at all, the shortest name and a whole amount",
            name: "March".to_string(),
            charge_amount: "125",
            description: None,
            purchase_order_reference: None,
        },
        Row {
            case: "the shortest PurchaseOrderReference, 1 character",
            name: "Usage overage for the quarter".to_string(),
            charge_amount: "41.5",
            description: Some("Overage above the committed tier.".to_string()),
            purchase_order_reference: Some("x".to_string()),
        },
        Row {
            case: "a long purchase order reference, 200 characters",
            name: "Metered API calls".to_string(),
            charge_amount: "0.00000001",
            description: Some("d".repeat(2000)),
            purchase_order_reference: Some(format!("PO-{}", "9".repeat(197))),
        },
        Row {
            case: "a purchase order number of the form a buyer writes, with a trailing-point \
                   amount",
            name: "Rounding".to_string(),
            charge_amount: "7.",
            description: None,
            purchase_order_reference: Some("PO-2026-00417".to_string()),
        },
        Row {
            case: "the longest PaymentRequestName, 64 characters, and a long integer part",
            name: format!("n{}", "o".repeat(63)),
            // A long amount that still fits under the ceiling the accepted
            // `VariablePaymentTerm` publishes; a send past that ceiling is
            // refused, which `payment_request_balance.rs` is about.
            charge_amount: "9876543",
            description: Some("x".to_string()),
            purchase_order_reference: Some("finance/ap queue #17".to_string()),
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
// Approving a pending request
// ---------------------------------------------------------------------------

/// Approving a `PENDING_APPROVAL` request moves that record to `APPROVED` and
/// reports it from what the send stored: the answer's members are the stored
/// ones, `createdAt` is untouched, `updatedAt` is the approving instant, the
/// approval creates the request's charge — which the reader reports as its
/// `chargeId` and which the agreement's charge report carries, with the buyer's
/// purchase order reference exactly when the approving call supplied one — and
/// every reader agrees with the answer afterwards.
#[tokio::test]
async fn approving_a_pending_request_moves_the_stored_record_to_approved() {
    let router = emulator();

    for (index, row) in rows().iter().enumerate() {
        let agreement_id =
            agreement_with_variable_term(&router, &format!("ap-praccept{index}"), TERM_ID).await;
        let (payment_request_id, created_at) = send_row(&router, &agreement_id, row).await;
        let case = row.case;

        // Before the approval the request is the one the send documented, and it
        // carries no charge: nothing has approved it.
        let before = get(&router, &agreement_id, &payment_request_id).await;
        assert_eq!(
            before.str_at("/status"),
            PENDING_APPROVAL,
            "case {case:?}: a request nothing has decided is {PENDING_APPROVAL}, got {:?}",
            before.body
        );
        assert!(
            before.body.get("chargeId").is_none_or(Value::is_null),
            "case {case:?}: only an approved request has a charge, got {:?}",
            before.body
        );
        let currency = before.str_at("/currencyCode");
        // The charges the agreement's own quote minted, which the approval must
        // leave alone: whatever the approval adds, it adds BESIDE these.
        let quoted_charges = reported_charge_ids(&router, &agreement_id).await;

        let approved = accept_payment(
            &router,
            &agreement_id,
            &payment_request_id,
            row.purchase_order_reference.as_deref(),
        )
        .await;
        assert_eq!(
            approved.status, 200,
            "case {case:?}: approving must succeed, got {} with {:?}",
            approved.status, approved.body
        );
        assert_eq!(
            approved.str_at("/paymentRequestId"),
            payment_request_id,
            "case {case:?}: the answer names the request that was approved, got {:?}",
            approved.body
        );
        assert!(
            is_payment_request_id(&approved.str_at("/paymentRequestId")),
            "case {case:?}: expected an ^pr-[a-zA-Z0-9]+$ id, got {:?}",
            approved.body
        );
        assert_eq!(
            approved.str_at("/agreementId"),
            agreement_id,
            "case {case:?}: the answer names the request's own agreement, got {:?}",
            approved.body
        );
        assert_eq!(
            approved.str_at("/status"),
            APPROVED,
            "case {case:?}: an approved request is {APPROVED}, got {:?}",
            approved.body
        );
        assert_eq!(
            approved.str_at("/name"),
            row.name,
            "case {case:?}: the answer reports the stored name, got {:?}",
            approved.body
        );
        assert_eq!(
            approved.body.get("description").and_then(Value::as_str),
            row.description.as_deref(),
            "case {case:?}: the answer reports the stored description exactly when there is one, \
             got {:?}",
            approved.body
        );
        assert_eq!(
            approved.str_at("/chargeAmount"),
            row.charge_amount,
            "case {case:?}: the answer reports the amount the send recorded, got {:?}",
            approved.body
        );
        assert_eq!(
            approved.str_at("/currencyCode"),
            currency,
            "case {case:?}: the answer reports the stored currency, got {:?}",
            approved.body
        );
        assert_eq!(
            approved.body["createdAt"].as_f64(),
            Some(created_at),
            "case {case:?}: createdAt stays the instant the request was created, got {:?}",
            approved.body
        );
        let approved_at = approved.body["updatedAt"].as_f64();
        assert!(
            approved_at.is_some_and(|at| at >= created_at),
            "case {case:?}: updatedAt is the instant of approval, got {approved_at:?} against \
             createdAt {created_at}"
        );

        // The reader reports the same record: the transition and the charge the
        // approval created are persisted, not local to the answer the approval
        // rendered.
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
                APPROVED.to_string(),
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
            (Some(created_at), approved_at),
            "case {case:?}: the reader reports the stored timestamps, got {:?}",
            read.body
        );
        assert!(
            read.body.get("statusMessage").is_none_or(Value::is_null),
            "case {case:?}: an approval carries no message member, so the record has none to \
             report, got {:?}",
            read.body
        );
        let charge_id = read.str_at("/chargeId");
        assert!(
            is_charge_id(&charge_id),
            "case {case:?}: the approval creates the request's charge, expected an \
             ^ch-[a-zA-Z0-9]+$ id, got {:?}",
            read.body
        );
        assert!(
            !quoted_charges.contains(&charge_id),
            "case {case:?}: the approval creates a charge of its own, but {charge_id} is one the \
             quote already minted ({quoted_charges:?})"
        );

        // The listing selects on the stored status, so the record MOVED rather
        // than being copied: it is reported under APPROVED and no longer under
        // PENDING_APPROVAL.
        for (status, expected) in [
            (APPROVED, vec![payment_request_id.clone()]),
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

        // The charge the approval created is a charge of the agreement the
        // request was sent under, carrying the amount and currency of that
        // request and the buyer's reference exactly when one was given.
        let mut expected_charges = quoted_charges.clone();
        expected_charges.push(charge_id.clone());
        assert_eq!(
            reported_charge_ids(&router, &agreement_id).await,
            expected_charges,
            "case {case:?}: the approval adds its charge beside the ones the quote minted"
        );
        let charge = reported_charge(&router, &agreement_id, &charge_id)
            .await
            .unwrap_or_else(|| {
                panic!("case {case:?}: {charge_id} must be reported as a charge of {agreement_id}")
            });
        assert_eq!(
            (
                charge["agreementId"].as_str(),
                charge["agreementType"].as_str(),
                charge["amount"].as_str(),
                charge["currencyCode"].as_str(),
                charge["revision"].as_i64(),
            ),
            (
                Some(agreement_id.as_str()),
                Some(AGREEMENT_TYPE),
                Some(row.charge_amount),
                Some(currency.as_str()),
                Some(INITIAL_REVISION),
            ),
            "case {case:?}: the charge is read from the request the approval moved, got {charge:?}"
        );
        assert_eq!(
            charge["purchaseOrderReference"].as_str(),
            row.purchase_order_reference.as_deref(),
            "case {case:?}: the charge carries the buyer's reference exactly when the approval \
             supplied one, got {charge:?}"
        );
        assert!(
            charge["time"].as_f64().is_some_and(|at| Some(at) == approved_at),
            "case {case:?}: the charge is incurred when the approval created it, got {charge:?} \
             against {approved_at:?}"
        );
    }
}

/// An approval acts on ONE request: the other requests of the store, including
/// another request of the same agreement, keep the status and the (absent)
/// charge they had, and no charge is created for them.
#[tokio::test]
async fn approving_one_request_leaves_every_other_request_alone() {
    let router = emulator();
    let agreement_id = agreement_with_variable_term(&router, "ap-praone", TERM_ID).await;
    let other_agreement = agreement_with_variable_term(&router, "ap-praother", TERM_ID).await;

    let approved_id = sent(&router, &agreement_id, TERM_ID, "Approved one", "10").await;
    // A second request over the SAME agreement, plus one over another agreement
    // entirely.
    let sibling = sent(&router, &agreement_id, TERM_ID, "Sibling request", "20").await;
    let elsewhere = sent(&router, &other_agreement, TERM_ID, "Elsewhere one", "30").await;

    let quoted_elsewhere = reported_charge_ids(&router, &other_agreement).await;

    let answer = accept_payment(&router, &agreement_id, &approved_id, Some("PO-77")).await;
    assert_eq!(
        answer.status, 200,
        "approving {approved_id} must succeed, got {} with {:?}",
        answer.status, answer.body
    );

    for (case, agreement, request_id, expected, has_charge) in [
        (
            "the approved request",
            &agreement_id,
            &approved_id,
            APPROVED,
            true,
        ),
        (
            "its sibling over the same agreement",
            &agreement_id,
            &sibling,
            PENDING_APPROVAL,
            false,
        ),
        (
            "a request of another agreement",
            &other_agreement,
            &elsewhere,
            PENDING_APPROVAL,
            false,
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
            read.body.get("chargeId").and_then(Value::as_str).is_some(),
            has_charge,
            "case {case:?}: expected a charge to be present: {has_charge}, got {:?}",
            read.body
        );
    }

    // The other agreement's charges are untouched: an approval creates a charge
    // of the agreement its own request was sent under and of no other.
    assert_eq!(
        reported_charge_ids(&router, &other_agreement).await,
        quoted_elsewhere,
        "approving a request of {agreement_id} must not add a charge to {other_agreement}"
    );
}

// ---------------------------------------------------------------------------
// A request that is not pending approval
// ---------------------------------------------------------------------------

/// "Only payment requests in PENDING_APPROVAL status can be accepted. A
/// ConflictException is thrown if the payment request is in any other status."
/// The second decision on one request is refused, and the refusal changes
/// nothing: the record keeps the status, the charge and the `updatedAt` the
/// FIRST decision gave it, and no second charge is created.
///
/// The rows are the whole success table, and each row is run against every way a
/// request leaves `PENDING_APPROVAL` — the buyer's own approve and reject, and
/// the seller's cancel — so the refusal is about the status the record reached
/// and not about which call moved it there or what the request was sent for.
#[tokio::test]
async fn approving_a_request_that_is_not_pending_approval_is_a_conflict() {
    let router = emulator();

    for (index, row) in rows().iter().enumerate() {
        for (decision, moved_to) in [
            ("accept", APPROVED),
            ("reject", REJECTED),
            ("cancel", CANCELLED),
        ] {
            let agreement_id = agreement_with_variable_term(
                &router,
                &format!("ap-praagain{index}{decision}"),
                TERM_ID,
            )
            .await;
            let (payment_request_id, _) = send_row(&router, &agreement_id, row).await;
            let case = format!("{}, already {decision}ed", row.case);

            let first = match decision {
                "accept" => {
                    accept_payment(
                        &router,
                        &agreement_id,
                        &payment_request_id,
                        row.purchase_order_reference.as_deref(),
                    )
                    .await
                }
                "reject" => reject_payment(&router, &agreement_id, &payment_request_id).await,
                _ => cancel_payment(&router, &agreement_id, &payment_request_id).await,
            };
            assert_eq!(
                first.status, 200,
                "case {case:?}: the first decision must succeed, got {} with {:?}",
                first.status, first.body
            );
            let decided_at = first.body["updatedAt"].as_f64();
            let stored = get(&router, &agreement_id, &payment_request_id).await;
            let stored_charge = stored
                .body
                .get("chargeId")
                .and_then(Value::as_str)
                .map(str::to_string);
            let stored_charges = reported_charge_ids(&router, &agreement_id).await;

            let again = accept_payment(
                &router,
                &agreement_id,
                &payment_request_id,
                Some("PO-must-not-be-stored"),
            )
            .await;
            assert_eq!(
                again.status, 409,
                "case {case:?}: approving a {moved_to} request answers HTTP 409, got {} with {:?}",
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
                        .get("chargeId")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                ),
                (moved_to.to_string(), decided_at, stored_charge),
                "case {case:?}: the refused approval left the record as the first decision stored \
                 it, got {:?}",
                read.body
            );
            assert_eq!(
                reported_charge_ids(&router, &agreement_id).await,
                stored_charges,
                "case {case:?}: a refused approval creates no charge"
            );
            for (_, charge) in reported_charges(&router, &agreement_id).await {
                assert_ne!(
                    charge["purchaseOrderReference"].as_str(),
                    Some("PO-must-not-be-stored"),
                    "case {case:?}: a refused approval associates nothing, got {charge:?}"
                );
            }
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
/// without a purchase order reference on the refused call.
///
/// The rows span the form of the child id within its modelled pattern, including
/// both published length bounds, and whether it names a stored request of
/// another agreement.
#[tokio::test]
async fn an_id_pair_that_names_no_stored_payment_request_is_not_found() {
    let router = emulator();
    let agreement_id = agreement_with_variable_term(&router, "ap-prapairs", TERM_ID).await;
    let payment_request_id = sent(&router, &agreement_id, TERM_ID, "Pair probe", "15").await;
    let other_agreement = agreement_with_variable_term(&router, "ap-prapairs2", TERM_ID).await;
    let other_request = sent(&router, &other_agreement, TERM_ID, "Other probe", "16").await;

    // `PaymentRequestId` is `@length(1, 64)` `^pr-[a-zA-Z0-9]+$`
    // (`smithy_query(query="operation",
    // operation="AcceptAgreementPaymentRequest")`), so every child id below
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

    for reference in [None, Some("PO-not-my-invoice")] {
        for (case, agreement, request_id, resource_type, resource_id) in &rows {
            let answer = accept_payment(&router, agreement, request_id, reference).await;
            let case = format!("{case} (reference {reference:?})");
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

    // No refused call approved anything: both stored requests are exactly as
    // they were, and both are still approvable.
    for (agreement, request_id) in [
        (&agreement_id, &payment_request_id),
        (&other_agreement, &other_request),
    ] {
        let read = get(&router, agreement, request_id).await;
        assert_eq!(
            read.str_at("/status"),
            PENDING_APPROVAL,
            "a refused lookup must not have approved {request_id}, got {:?}",
            read.body
        );
        let answer = accept_payment(&router, agreement, request_id, None).await;
        assert_eq!(
            (answer.status, answer.str_at("/status")),
            (200, APPROVED.to_string()),
            "{request_id} stayed approvable after the refused lookups, got {:?}",
            answer.body
        );
    }
}

// ---------------------------------------------------------------------------
// The association the approval recorded
// ---------------------------------------------------------------------------

/// The reference an approval recorded is the charge's ONE purchase order
/// association, and no later call replaces it: `UpdatePurchaseOrders` performs
/// no association at all (`purchase_order_updates`), so what the report keeps
/// carrying is the value the approval recorded — or no reference at all for a
/// charge approved without one.
///
/// The rows are the two ways the charge can start out — approved with a
/// reference and approved without one — and the references they are approved
/// with differ in form and length.
#[tokio::test]
async fn the_purchase_order_of_an_approved_charge_has_one_value() {
    let router = emulator();

    for (index, (case, approved_with, then)) in [
        (
            "approved with a reference, then an association is attempted",
            Some("PO-first"),
            "PO-2026-99999",
        ),
        (
            "approved with no reference at all",
            None,
            "finance/ap queue #4",
        ),
        (
            "approved with the shortest reference the model admits",
            Some("x"),
            "p",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let agreement_id =
            agreement_with_variable_term(&router, &format!("ap-prapo{index}"), TERM_ID).await;
        let payment_request_id =
            sent(&router, &agreement_id, TERM_ID, "Association probe", "42").await;

        let approved =
            accept_payment(&router, &agreement_id, &payment_request_id, approved_with).await;
        assert_eq!(
            approved.status, 200,
            "case {case:?}: approving must succeed, got {} with {:?}",
            approved.status, approved.body
        );
        let charge_id = get(&router, &agreement_id, &payment_request_id)
            .await
            .str_at("/chargeId");
        assert!(
            is_charge_id(&charge_id),
            "case {case:?}: the approval creates a charge, got {charge_id:?}"
        );

        let updated = call(
            &router,
            "UpdatePurchaseOrders",
            json!({
                "purchaseOrders":
                    naming_agreement(json!([order(&charge_id, then)]), &agreement_id),
            }),
        )
        .await;
        assert_unsupported_action(case, &updated);

        let charge = reported_charge(&router, &agreement_id, &charge_id)
            .await
            .unwrap_or_else(|| {
                panic!("case {case:?}: {charge_id} must be a charge of {agreement_id}")
            });
        assert_eq!(
            charge["purchaseOrderReference"].as_str(),
            approved_with,
            "case {case:?}: a charge carries ONE reference, the one its approval recorded, got \
             {charge:?}"
        );
    }
}
