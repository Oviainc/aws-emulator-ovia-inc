// SPDX-License-Identifier: CC0-1.0

//! Rejecting a stored agreement cancellation request, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the
//! same path a client takes.
//!
//! The rule under test: `RejectAgreementCancellationRequest` acts on the ONE
//! stored cancellation request the agreement it names holds under that id — it
//! moves that record to `REJECTED`, stores the required `rejectionReason` as the
//! record's `statusMessage`, stamps `updatedAt` with the rejecting call while
//! every value the send recorded (and `createdAt`) stays as it was, and leaves
//! the agreement the request was sent over in the status it already had; a
//! request in any other status is refused with a `ConflictException`, an id pair
//! that names no stored request with the modelled `ResourceNotFoundException`
//! resolved PARENT FIRST, and nothing in any answer is echoed from the rejecting
//! call's own input beyond the message it stored. Every later reader —
//! `GetAgreementCancellationRequest` and `ListAgreementCancellationRequests` —
//! reports that same record.
//!
//! The rows span what the rule generalises over: the reason code the request was
//! created with (every value of `AgreementCancellationRequestReasonCode`),
//! whether the optional description was supplied, the length and shape of the
//! rejection reason at both published bounds of
//! `AgreementCancellationRequestRejectionReason`, the status the request is
//! already in when it is rejected, and which id of the required pair names
//! nothing.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::agreement_cancellation_requests::{
    agreement, cancellation_over, is_cancellation_request_id, list, send, PENDING_APPROVAL,
    REASON_CODES,
};
use super::agreement_lifecycle::{call, emulator, Answer};

/// `AgreementCancellationRequestStatus`'s value for a rejected request: "Once
/// rejected, the cancellation request transitions to REJECTED status"
/// (`RejectAgreementCancellationRequest`). Spelled out here rather than imported
/// from the crate so a handler that stores something else fails this test.
const REJECTED: &str = "REJECTED";
/// See [`REJECTED`]: the status the buyer's accept moves a request to, which is
/// the other decision on the same record.
const APPROVED: &str = "APPROVED";

/// `AgreementStatus`'s value an accepted agreement carries
/// (`smithy_query(query="shape", shape="AgreementStatus")`) — the one the
/// agreement a rejected cancellation was sent over must keep: "the agreement
/// remains active".
const AGREEMENT_ACTIVE: &str = "ACTIVE";

/// The resource kinds a lookup names in `resourceType` — `ResourceType`'s values
/// (`smithy_query(query="shape", shape="ResourceType")`).
const AGREEMENT: &str = "Agreement";
/// See [`AGREEMENT`].
const AGREEMENT_CANCELLATION_REQUEST: &str = "AgreementCancellationRequest";

/// The id pair naming one stored cancellation request, which deciding and
/// reading it both take.
fn pair(agreement_id: &str, request_id: &str) -> Value {
    json!({
        "agreementId": agreement_id,
        "agreementCancellationRequestId": request_id,
    })
}

/// Reject one cancellation request with the buyer's required reason.
async fn reject(
    router: &MultiServiceRouter,
    agreement_id: &str,
    request_id: &str,
    rejection_reason: &str,
) -> Answer {
    let mut body = pair(agreement_id, request_id);
    body["rejectionReason"] = json!(rejection_reason);
    call(router, "RejectAgreementCancellationRequest", body).await
}

/// Accept one cancellation request — the buyer's other decision on the same
/// record, used here only to put a request into a status a reject must refuse.
async fn accept(router: &MultiServiceRouter, agreement_id: &str, request_id: &str) -> Answer {
    call(
        router,
        "AcceptAgreementCancellationRequest",
        pair(agreement_id, request_id),
    )
    .await
}

async fn get_cancellation(
    router: &MultiServiceRouter,
    agreement_id: &str,
    request_id: &str,
) -> Answer {
    call(
        router,
        "GetAgreementCancellationRequest",
        pair(agreement_id, request_id),
    )
    .await
}

/// The `status` `DescribeAgreement` reports for `agreement_id`.
async fn agreement_status(router: &MultiServiceRouter, agreement_id: &str) -> String {
    let described = call(
        router,
        "DescribeAgreement",
        json!({"agreementId": agreement_id}),
    )
    .await;
    assert_eq!(
        described.status, 200,
        "describing {agreement_id} must succeed, got {} with {:?}",
        described.status, described.body
    );
    described.str_at("/status")
}

/// The ids one listing reported, in the order it reported them.
fn listed_ids(answer: &Answer) -> Vec<String> {
    answer.body["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            item["agreementCancellationRequestId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// One row: a cancellation request as the seller sent it, and the reason the
/// buyer rejects it with — so as every later report of it must read.
struct Row {
    case: String,
    /// One of `AgreementCancellationRequestReasonCode`'s values.
    reason_code: &'static str,
    /// `AgreementCancellationRequestDescription` is `@length(1, 2000)`, and
    /// optional.
    description: Option<String>,
    /// `AgreementCancellationRequestRejectionReason` is `@length(1, 2000)`, and
    /// REQUIRED.
    rejection_reason: String,
}

/// The table every success case runs over. The member constraints come from
/// `smithy_query(query="operation",
/// operation="RejectAgreementCancellationRequest")` and the reason codes from
/// `AgreementCancellationRequestReasonCode`; the rows differ along the reason
/// code the request carries (every value of the modelled enum, so an entry the
/// issue never cited is covered too), whether a description was given, and the
/// length and shape of the rejection reason — the two published bounds of its
/// length among them.
fn rows() -> Vec<Row> {
    REASON_CODES
        .iter()
        .enumerate()
        .map(|(index, reason_code)| {
            // Every other row supplies the optional description.
            let description = (index % 2 == 0).then(|| format!("row {index} is cancelling"));
            // The rejection reasons cycle through the shapes the member admits,
            // starting at each published length bound.
            let (shape, rejection_reason) = match index % 4 {
                0 => ("the shortest reason, 1 character", "x".to_string()),
                1 => ("the longest reason, 2000 characters", "r".repeat(2000)),
                2 => (
                    "a sentence of a reason",
                    "The agreement is still in use by the team.".to_string(),
                ),
                _ => (
                    "a reason with punctuation and digits",
                    "Keep it: 3 of 4 seats are still active — see ticket #91.".to_string(),
                ),
            };
            Row {
                case: format!("reasonCode {reason_code}, {shape}, description {description:?}"),
                reason_code,
                description,
                rejection_reason,
            }
        })
        .collect()
}

/// Send one row's cancellation request over `agreement_id` and answer the id it
/// minted together with the `createdAt` it recorded.
async fn send_row(router: &MultiServiceRouter, agreement_id: &str, row: &Row) -> (String, f64) {
    let mut body = json!({"agreementId": agreement_id, "reasonCode": row.reason_code});
    if let Some(description) = row.description.as_deref() {
        body["description"] = json!(description);
    }
    let answer = send(router, body).await;
    assert_eq!(
        answer.status, 200,
        "case {:?}: sending the cancellation request must succeed, got {} with {:?}",
        row.case, answer.status, answer.body
    );
    let created_at = answer.body["createdAt"].as_f64().unwrap_or_else(|| {
        panic!(
            "case {:?}: a sent cancellation request carries createdAt, got {:?}",
            row.case, answer.body
        )
    });
    (answer.str_at("/agreementCancellationRequestId"), created_at)
}

// ---------------------------------------------------------------------------
// Rejecting a pending request
// ---------------------------------------------------------------------------

/// Rejecting a `PENDING_APPROVAL` request moves that record to `REJECTED` and
/// reports it from what the send stored: the answer's members are the stored
/// ones, `statusMessage` is the reason this call supplied, `createdAt` is
/// untouched, `updatedAt` is the rejecting instant, the reader agrees with the
/// answer afterwards, and the agreement the request was sent over is still
/// active.
#[tokio::test]
async fn rejecting_a_pending_request_moves_the_stored_record_to_rejected() {
    let router = emulator();

    for (index, row) in rows().iter().enumerate() {
        let agreement_id = agreement(&router, &format!("ap-acrreject{index}")).await;
        let (request_id, created_at) = send_row(&router, &agreement_id, row).await;
        let case = &row.case;

        // Before the reject the request is the one the send documented, and it
        // carries no status message: nothing has decided it.
        let before = get_cancellation(&router, &agreement_id, &request_id).await;
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

        let rejected = reject(&router, &agreement_id, &request_id, &row.rejection_reason).await;
        assert_eq!(
            rejected.status, 200,
            "case {case:?}: rejecting must succeed, got {} with {:?}",
            rejected.status, rejected.body
        );
        assert_eq!(
            rejected.str_at("/agreementCancellationRequestId"),
            request_id,
            "case {case:?}: the answer names the request that was rejected, got {:?}",
            rejected.body
        );
        assert!(
            is_cancellation_request_id(&rejected.str_at("/agreementCancellationRequestId")),
            "case {case:?}: expected an ^acr-[a-zA-Z0-9]+$ id, got {:?}",
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
            Some(row.rejection_reason.as_str()),
            "case {case:?}: statusMessage is the rejection reason this call supplied, got {:?}",
            rejected.body
        );
        assert_eq!(
            rejected.str_at("/reasonCode"),
            row.reason_code,
            "case {case:?}: the answer reports the reason code the request was created with, got \
             {:?}",
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
        let read = get_cancellation(&router, &agreement_id, &request_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: reading the request back must succeed, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            (
                read.str_at("/agreementCancellationRequestId"),
                read.str_at("/agreementId"),
                read.str_at("/status"),
                read.str_at("/reasonCode"),
            ),
            (
                request_id.clone(),
                agreement_id.clone(),
                REJECTED.to_string(),
                row.reason_code.to_string(),
            ),
            "case {case:?}: the reader reports the stored record, got {:?}",
            read.body
        );
        assert_eq!(
            read.body.get("statusMessage").and_then(Value::as_str),
            Some(row.rejection_reason.as_str()),
            "case {case:?}: the reader reports the stored rejection reason, got {:?}",
            read.body
        );
        assert_eq!(
            read.body.get("description").and_then(Value::as_str),
            row.description.as_deref(),
            "case {case:?}: the reader reports the stored description, got {:?}",
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

        // The listing selects on the stored status, so the record MOVED rather
        // than being copied: it is reported under REJECTED and no longer under
        // PENDING_APPROVAL.
        for (status, expected) in [
            (REJECTED, vec![request_id.clone()]),
            (PENDING_APPROVAL, Vec::new()),
            (APPROVED, Vec::new()),
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

        // "… and the agreement remains active": rejecting the cancellation
        // leaves the agreement exactly as it was.
        assert_eq!(
            agreement_status(&router, &agreement_id).await,
            AGREEMENT_ACTIVE,
            "case {case:?}: a rejected cancellation leaves its agreement {AGREEMENT_ACTIVE}"
        );
    }
}

/// A reject acts on ONE request: the other requests of the store, including
/// another request of the same agreement, keep the status and the (absent)
/// status message they had, and so does every other agreement.
#[tokio::test]
async fn rejecting_one_request_leaves_every_other_request_alone() {
    let router = emulator();
    let decided_agreement = agreement(&router, "ap-acrrone").await;
    let other_agreement = agreement(&router, "ap-acrrother").await;
    let decided = cancellation_over(&router, &decided_agreement, "OTHER").await;
    // A second request over the SAME agreement, which the model refuses nothing
    // about, plus one over another agreement entirely.
    let sibling = cancellation_over(&router, &decided_agreement, "TEST_AGREEMENT").await;
    let elsewhere = cancellation_over(&router, &other_agreement, "BUYER_DISSATISFACTION").await;

    let reason = "The team still depends on this agreement";
    let answer = reject(&router, &decided_agreement, &decided, reason).await;
    assert_eq!(
        answer.status, 200,
        "rejecting {decided} must succeed, got {} with {:?}",
        answer.status, answer.body
    );

    for (case, agreement_id, request_id, expected, message) in [
        (
            "the rejected request",
            &decided_agreement,
            &decided,
            REJECTED,
            Some(reason),
        ),
        (
            "its sibling over the same agreement",
            &decided_agreement,
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
        let read = get_cancellation(&router, agreement_id, request_id).await;
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

    for agreement_id in [&decided_agreement, &other_agreement] {
        assert_eq!(
            agreement_status(&router, agreement_id).await,
            AGREEMENT_ACTIVE,
            "no reject cancels an agreement, so {agreement_id} stays {AGREEMENT_ACTIVE}"
        );
    }

    // Its sibling is still decidable: the reject consumed nothing but its own
    // record.
    let sibling_rejected = reject(&router, &decided_agreement, &sibling, "Also keeping this").await;
    assert_eq!(
        (sibling_rejected.status, sibling_rejected.str_at("/status")),
        (200, REJECTED.to_string()),
        "the sibling stayed decidable, got {:?}",
        sibling_rejected.body
    );
}

// ---------------------------------------------------------------------------
// A request that is not pending approval
// ---------------------------------------------------------------------------

/// "Only cancellation requests in PENDING_APPROVAL status can be rejected. A
/// ConflictException is thrown if the cancellation request is in any other
/// status." The second decision on one request is refused, and the refusal
/// changes nothing: the record keeps the status, the status message and the
/// `updatedAt` the FIRST decision gave it.
///
/// The rows are the whole success table, and each row is run against both ways a
/// request leaves `PENDING_APPROVAL` — this call's own reject and the buyer's
/// accept — so the refusal is about the status the record reached and not about
/// which call moved it there or what the request was sent for.
#[tokio::test]
async fn rejecting_a_request_that_is_not_pending_approval_is_a_conflict() {
    let router = emulator();

    for (index, row) in rows().iter().enumerate() {
        for (decision, moved_to) in [("reject", REJECTED), ("accept", APPROVED)] {
            let agreement_id =
                agreement(&router, &format!("ap-acragain{index}{decision}")).await;
            let (request_id, _) = send_row(&router, &agreement_id, row).await;
            let case = format!("{}, already {decision}ed", row.case);

            let first = if decision == "reject" {
                reject(&router, &agreement_id, &request_id, &row.rejection_reason).await
            } else {
                accept(&router, &agreement_id, &request_id).await
            };
            assert_eq!(
                first.status, 200,
                "case {case:?}: the first decision must succeed, got {} with {:?}",
                first.status, first.body
            );
            let decided_at = first.body["updatedAt"].as_f64();
            let stored_message = get_cancellation(&router, &agreement_id, &request_id)
                .await
                .body
                .get("statusMessage")
                .and_then(Value::as_str)
                .map(str::to_string);

            let again = reject(
                &router,
                &agreement_id,
                &request_id,
                "A second reason, which must not be stored",
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
                AGREEMENT_CANCELLATION_REQUEST,
                "case {case:?}: the conflict names the resource kind it refused, got {:?}",
                again.body
            );
            assert_eq!(
                again.str_at("/resourceId"),
                request_id,
                "case {case:?}: the conflict names the request it refused, got {:?}",
                again.body
            );
            assert!(
                !again.str_at("/message").is_empty(),
                "case {case:?}: a refusal must say what was wrong, got {:?}",
                again.body
            );

            let read = get_cancellation(&router, &agreement_id, &request_id).await;
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

/// The required id pair is resolved PARENT FIRST: an `agreementId` that names no
/// agreement is reported as the missing `Agreement`, and an
/// `agreementCancellationRequestId` that names no request of THAT agreement —
/// including one that belongs to another agreement — as the missing
/// `AgreementCancellationRequest`. Nothing is written by a refusal.
///
/// The rows span which id of the pair names nothing and the form of that id
/// within its modelled pattern, including both published length bounds, and each
/// is refused the same way whatever the rejection reason carries.
#[tokio::test]
async fn an_id_pair_that_names_no_stored_request_is_not_found() {
    let router = emulator();
    let agreement_id = agreement(&router, "ap-acrrpairs").await;
    let request_id = cancellation_over(&router, &agreement_id, "OTHER").await;
    let other_agreement = agreement(&router, "ap-acrrpairs2").await;
    let other_request = cancellation_over(&router, &other_agreement, "OTHER").await;

    // `AgreementId` is `@length(1, 64)` `^[A-Za-z0-9_/-]+$` and
    // `AgreementCancellationRequestId` is `@length(1, 64)`
    // `^acr-[a-zA-Z0-9]+$` (`smithy_query(query="operation",
    // operation="RejectAgreementCancellationRequest")`), so every id below
    // satisfies its member's constraints and reaches the lookup, including both
    // published length bounds.
    let rows: Vec<(&str, String, String, &str, String)> = vec![
        (
            "the shortest agreement id, naming no agreement",
            "A".to_string(),
            request_id.clone(),
            AGREEMENT,
            "A".to_string(),
        ),
        (
            "the longest agreement id, 64 characters",
            "z".repeat(64),
            request_id.clone(),
            AGREEMENT,
            "z".repeat(64),
        ),
        (
            "the shortest request id the acr- pattern admits",
            agreement_id.clone(),
            "acr-0".to_string(),
            AGREEMENT_CANCELLATION_REQUEST,
            "acr-0".to_string(),
        ),
        (
            "the longest AgreementCancellationRequestId, 64 characters",
            agreement_id.clone(),
            format!("acr-{}", "A".repeat(60)),
            AGREEMENT_CANCELLATION_REQUEST,
            format!("acr-{}", "A".repeat(60)),
        ),
        (
            "a request that belongs to another agreement",
            agreement_id.clone(),
            other_request.clone(),
            AGREEMENT_CANCELLATION_REQUEST,
            other_request.clone(),
        ),
    ];

    for reason in ["x", &"r".repeat(2000), "Not this agreement"] {
        for (case, agreement, request, resource_type, resource_id) in &rows {
            let answer = reject(&router, agreement, request, reason).await;
            let case = format!("{case} (reason of {} characters)", reason.len());
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
    // they were, their agreements are still active, and both are still
    // rejectable.
    for (agreement_id, request_id) in [
        (&agreement_id, &request_id),
        (&other_agreement, &other_request),
    ] {
        let read = get_cancellation(&router, agreement_id, request_id).await;
        assert_eq!(
            read.str_at("/status"),
            PENDING_APPROVAL,
            "a refused lookup must not have rejected {request_id}, got {:?}",
            read.body
        );
        assert_eq!(
            agreement_status(&router, agreement_id).await,
            AGREEMENT_ACTIVE,
            "a refused lookup must not have touched {agreement_id}"
        );
        let answer = reject(&router, agreement_id, request_id, "Still rejectable").await;
        assert_eq!(
            (answer.status, answer.str_at("/status")),
            (200, REJECTED.to_string()),
            "{request_id} stayed rejectable after the refused lookups, got {:?}",
            answer.body
        );
    }
}
