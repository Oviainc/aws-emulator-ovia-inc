// SPDX-License-Identifier: CC0-1.0

//! Deciding a stored agreement cancellation request, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the
//! same path a client takes.
//!
//! The rule under test: a decision on a cancellation request acts on the ONE
//! stored request the agreement it names holds under that id — accepting it
//! moves that record to `APPROVED`, stamps `updatedAt` with the deciding call
//! while every value the record was created with stays as it was, and processes
//! the cancellation of the agreement the request belongs to; a request in any
//! other status is refused with a `ConflictException`, an id pair that names no
//! stored request with the modelled `ResourceNotFoundException`, and nothing in
//! any answer is echoed from the deciding call's own input. Reading one back
//! (`GetAgreementCancellationRequest`) reports that same record and changes
//! nothing for a later caller.
//!
//! The rows span what the rule generalises over: the reason code the request was
//! created with (every value of `AgreementCancellationRequestReasonCode`),
//! whether the optional description was supplied, the status the request is
//! already in when it is decided, the status its agreement is already in, which
//! id of the required pair names nothing, and which of the two operations that
//! resolve the pair is called.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::agreement_cancellation_requests::{
    agreement, cancellation_over, is_cancellation_request_id, list, send, PENDING_APPROVAL,
    REASON_CODES,
};
use super::agreement_lifecycle::{call, cancel, emulator, Answer};

/// `AgreementCancellationRequestStatus`'s value for an accepted request: "Once
/// accepted, the cancellation request transitions to APPROVED status"
/// (`AcceptAgreementCancellationRequest`). Spelled out here rather than imported
/// from the crate so a handler that stores something else fails this test.
const APPROVED: &str = "APPROVED";

/// `AgreementStatus`'s values this suite asserts over
/// (`smithy_query(query="shape", shape="AgreementStatus")`): the one an accepted
/// agreement carries, and the one a processed cancellation leaves it in.
const AGREEMENT_ACTIVE: &str = "ACTIVE";
/// See [`AGREEMENT_ACTIVE`].
const AGREEMENT_CANCELLED: &str = "CANCELLED";

/// The resource kinds a lookup names in `resourceType` — `ResourceType`'s values
/// (`smithy_query(query="shape", shape="ResourceType")`).
const AGREEMENT: &str = "Agreement";
/// See [`AGREEMENT`].
const AGREEMENT_CANCELLATION_REQUEST: &str = "AgreementCancellationRequest";

async fn accept_cancellation(router: &MultiServiceRouter, body: Value) -> Answer {
    call(router, "AcceptAgreementCancellationRequest", body).await
}

async fn get_cancellation(router: &MultiServiceRouter, body: Value) -> Answer {
    call(router, "GetAgreementCancellationRequest", body).await
}

async fn describe(router: &MultiServiceRouter, agreement_id: &str) -> Answer {
    call(
        router,
        "DescribeAgreement",
        json!({"agreementId": agreement_id}),
    )
    .await
}

/// The `status` `DescribeAgreement` reports for `agreement_id`.
async fn agreement_status(router: &MultiServiceRouter, agreement_id: &str) -> String {
    let described = describe(router, agreement_id).await;
    assert_eq!(
        described.status, 200,
        "describing {agreement_id} must succeed, got {} with {:?}",
        described.status, described.body
    );
    described.str_at("/status")
}

/// The id pair naming one stored cancellation request, which both deciding and
/// reading it take.
fn pair(agreement_id: &str, request_id: &str) -> Value {
    json!({
        "agreementId": agreement_id,
        "agreementCancellationRequestId": request_id,
    })
}

// ---------------------------------------------------------------------------
// Accepting a pending request
// ---------------------------------------------------------------------------

/// Accepting a `PENDING_APPROVAL` request approves that record and processes the
/// cancellation of its agreement: the answer is computed from the stored
/// request, reading it back reports the same thing, and the agreement the
/// request belongs to has left `ACTIVE` for `CANCELLED`.
///
/// The rows differ along what the rule generalises over: the reason code the
/// request carries (every value of the modelled enum, so an entry nothing else
/// cites is covered too) and whether the optional description was supplied.
#[tokio::test]
async fn accepting_a_pending_request_approves_it_and_cancels_its_agreement() {
    let router = emulator();

    for (index, reason_code) in REASON_CODES.iter().enumerate() {
        // Every other row supplies the optional description.
        let description = (index % 2 == 0).then(|| format!("row {index} is cancelling"));
        let agreement_id = agreement(&router, &format!("ap-decide{index}")).await;
        let mut request = json!({"agreementId": agreement_id, "reasonCode": reason_code});
        if let Some(description) = description.as_deref() {
            request["description"] = json!(description);
        }
        let sent = send(&router, request).await;
        let case = format!("reasonCode {reason_code}, description {description:?}");
        assert_eq!(
            sent.status, 200,
            "case {case}: sending the cancellation request must succeed, got {} with {:?}",
            sent.status, sent.body
        );
        let request_id = sent.str_at("/agreementCancellationRequestId");
        let created_at = sent.body["createdAt"].as_f64();
        assert_eq!(
            agreement_status(&router, &agreement_id).await,
            AGREEMENT_ACTIVE,
            "case {case}: the agreement is still {AGREEMENT_ACTIVE} before the decision"
        );

        let accepted = accept_cancellation(&router, pair(&agreement_id, &request_id)).await;
        assert_eq!(
            accepted.status, 200,
            "case {case}: accepting must succeed, got {} with {:?}",
            accepted.status, accepted.body
        );
        assert_eq!(
            accepted.str_at("/agreementCancellationRequestId"),
            request_id,
            "case {case}: the answer names the request that was accepted, got {:?}",
            accepted.body
        );
        assert!(
            is_cancellation_request_id(&accepted.str_at("/agreementCancellationRequestId")),
            "case {case}: expected an ^acr-[a-zA-Z0-9]+$ id, got {:?}",
            accepted.body
        );
        assert_eq!(
            accepted.str_at("/agreementId"),
            agreement_id,
            "case {case}: the answer names the request's own agreement, got {:?}",
            accepted.body
        );
        assert_eq!(
            accepted.str_at("/status"),
            APPROVED,
            "case {case}: an accepted request is {APPROVED}, got {:?}",
            accepted.body
        );
        assert_eq!(
            accepted.str_at("/reasonCode"),
            *reason_code,
            "case {case}: the answer reports the reason code the request was created with, got {:?}",
            accepted.body
        );
        assert_eq!(
            accepted.body.get("description").and_then(Value::as_str),
            description.as_deref(),
            "case {case}: the answer reports the stored description exactly when there is one, got {:?}",
            accepted.body
        );
        assert_eq!(
            accepted.body["createdAt"].as_f64(),
            created_at,
            "case {case}: createdAt stays the instant the request was created, got {:?}",
            accepted.body
        );
        let accepted_at = accepted.body["updatedAt"].as_f64();
        assert!(
            accepted_at.is_some() && accepted_at >= created_at,
            "case {case}: updatedAt is the instant of acceptance, got {accepted_at:?} against \
             createdAt {created_at:?}"
        );

        // The reader reports the same record: the status change is persisted,
        // not local to the answer the decision rendered.
        let read = get_cancellation(&router, pair(&agreement_id, &request_id)).await;
        assert_eq!(
            read.status, 200,
            "case {case}: reading the request back must succeed, got {} with {:?}",
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
                APPROVED.to_string(),
                (*reason_code).to_string(),
            ),
            "case {case}: the reader reports the stored record, got {:?}",
            read.body
        );
        assert_eq!(
            read.body.get("description").and_then(Value::as_str),
            description.as_deref(),
            "case {case}: the reader reports the stored description, got {:?}",
            read.body
        );
        assert_eq!(
            (read.body["createdAt"].as_f64(), read.body["updatedAt"].as_f64()),
            (created_at, accepted_at),
            "case {case}: the reader reports the stored timestamps, got {:?}",
            read.body
        );
        assert!(
            read.body.get("statusMessage").is_none_or(Value::is_null),
            "case {case}: no message was ever stored for this request, got {:?}",
            read.body
        );

        // The listing selects on the stored status, so the record moved rather
        // than being copied: it is reported under APPROVED and no longer under
        // PENDING_APPROVAL.
        for (status, expected) in [
            (APPROVED, vec![request_id.clone()]),
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
            let ids: Vec<String> = listed.body["items"]
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
                .collect();
            assert_eq!(
                ids, expected,
                "case {case}: the {status} listing must report {expected:?}, got {:?}",
                listed.body
            );
        }

        // "… and the agreement cancellation will be processed": the agreement
        // the approved request belongs to has been cancelled.
        assert_eq!(
            agreement_status(&router, &agreement_id).await,
            AGREEMENT_CANCELLED,
            "case {case}: an approved cancellation leaves its agreement {AGREEMENT_CANCELLED}"
        );
    }
}

/// A decision acts on ONE request: the other requests of the store, including
/// another request of the same agreement, keep the status they had.
#[tokio::test]
async fn accepting_one_request_leaves_every_other_request_alone() {
    let router = emulator();
    let decided_agreement = agreement(&router, "ap-decidedone").await;
    let other_agreement = agreement(&router, "ap-untouched").await;
    let decided = cancellation_over(&router, &decided_agreement, "OTHER").await;
    // A second request over the SAME agreement, which the model refuses nothing
    // about, plus one over another agreement entirely.
    let sibling = cancellation_over(&router, &decided_agreement, "TEST_AGREEMENT").await;
    let elsewhere = cancellation_over(&router, &other_agreement, "BUYER_DISSATISFACTION").await;

    let accepted = accept_cancellation(&router, pair(&decided_agreement, &decided)).await;
    assert_eq!(
        accepted.status, 200,
        "accepting {decided} must succeed, got {} with {:?}",
        accepted.status, accepted.body
    );

    for (case, agreement_id, request_id, expected) in [
        ("the accepted request", &decided_agreement, &decided, APPROVED),
        (
            "its sibling over the same agreement",
            &decided_agreement,
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
        let read = get_cancellation(&router, pair(agreement_id, request_id)).await;
        assert_eq!(
            read.str_at("/status"),
            expected,
            "case {case:?}: expected {expected}, got {:?}",
            read.body
        );
    }

    assert_eq!(
        agreement_status(&router, &other_agreement).await,
        AGREEMENT_ACTIVE,
        "the agreement no decision was made on stays {AGREEMENT_ACTIVE}"
    );
}

// ---------------------------------------------------------------------------
// A request that is not pending approval
// ---------------------------------------------------------------------------

/// "Only cancellation requests in PENDING_APPROVAL status can be accepted. A
/// ConflictException is thrown if the cancellation request is in any other
/// status." The second accept of one request is refused, and the refusal changes
/// nothing: the record keeps the status and the `updatedAt` the FIRST decision
/// gave it.
///
/// The rows span the reason codes a request can carry, so the refusal is about
/// the status and not about what the request was created for.
#[tokio::test]
async fn accepting_a_request_that_is_not_pending_approval_is_a_conflict() {
    let router = emulator();

    for (index, reason_code) in REASON_CODES.iter().enumerate() {
        let agreement_id = agreement(&router, &format!("ap-again{index}")).await;
        let request_id = cancellation_over(&router, &agreement_id, reason_code).await;
        let case = format!("reasonCode {reason_code}");

        let first = accept_cancellation(&router, pair(&agreement_id, &request_id)).await;
        assert_eq!(
            first.status, 200,
            "case {case}: the first accept must succeed, got {} with {:?}",
            first.status, first.body
        );
        let approved_at = first.body["updatedAt"].as_f64();

        let again = accept_cancellation(&router, pair(&agreement_id, &request_id)).await;
        assert_eq!(
            again.status, 409,
            "case {case}: accepting an {APPROVED} request answers HTTP 409, got {} with {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.error_code(),
            "ConflictException",
            "case {case}: expected ConflictException, got {:?}",
            again.body
        );
        assert_eq!(
            again.str_at("/resourceType"),
            AGREEMENT_CANCELLATION_REQUEST,
            "case {case}: the conflict names the resource kind it refused, got {:?}",
            again.body
        );
        assert_eq!(
            again.str_at("/resourceId"),
            request_id,
            "case {case}: the conflict names the request it refused, got {:?}",
            again.body
        );
        assert!(
            !again.str_at("/message").is_empty(),
            "case {case}: a refusal must say what was wrong, got {:?}",
            again.body
        );

        let read = get_cancellation(&router, pair(&agreement_id, &request_id)).await;
        assert_eq!(
            (read.str_at("/status"), read.body["updatedAt"].as_f64()),
            (APPROVED.to_string(), approved_at),
            "case {case}: the refused decision left the record as the first one stored it, got {:?}",
            read.body
        );
    }
}

/// An agreement that has already left `ACTIVE` keeps the status it reached: the
/// request's own status rule is what admits the decision, and processing the
/// cancellation never moves an agreement backwards into a status it has already
/// passed.
#[tokio::test]
async fn accepting_a_request_over_an_already_cancelled_agreement_keeps_its_status() {
    let router = emulator();
    let agreement_id = agreement(&router, "ap-alreadygone").await;
    let request_id = cancellation_over(&router, &agreement_id, "REPLACING_AGREEMENT").await;

    // The acceptor cancels the agreement directly while the request is still
    // pending, so the cancellation this decision would process has already
    // happened.
    let cancelled = cancel(&router, &agreement_id).await;
    assert_eq!(
        cancelled.status, 200,
        "cancelling {agreement_id} must succeed, got {} with {:?}",
        cancelled.status, cancelled.body
    );

    let accepted = accept_cancellation(&router, pair(&agreement_id, &request_id)).await;
    assert_eq!(
        accepted.status, 200,
        "a {PENDING_APPROVAL} request is accepted whatever its agreement's status, got {} with {:?}",
        accepted.status, accepted.body
    );
    assert_eq!(
        accepted.str_at("/status"),
        APPROVED,
        "the accepted request is {APPROVED}, got {:?}",
        accepted.body
    );
    assert_eq!(
        agreement_status(&router, &agreement_id).await,
        AGREEMENT_CANCELLED,
        "the agreement keeps the terminal status it already reached"
    );
}

// ---------------------------------------------------------------------------
// The id pair
// ---------------------------------------------------------------------------

/// Both operations resolve the required id pair the same way, PARENT FIRST: an
/// `agreementId` that names no agreement is reported as the missing
/// `Agreement`, and an `agreementCancellationRequestId` that names no request of
/// THAT agreement — including one that belongs to another agreement — as the
/// missing `AgreementCancellationRequest`. Nothing is written by a refusal.
///
/// The rows span which id of the pair names nothing, the form of that id within
/// its modelled pattern, and which of the two operations is called.
#[tokio::test]
async fn an_id_pair_that_names_no_stored_request_is_not_found() {
    let router = emulator();
    let agreement_id = agreement(&router, "ap-pairs").await;
    let request_id = cancellation_over(&router, &agreement_id, "OTHER").await;
    let other_agreement = agreement(&router, "ap-otherpairs").await;
    let other_request = cancellation_over(&router, &other_agreement, "OTHER").await;

    // `AgreementId` is `@length(1, 64)` `^[A-Za-z0-9_/-]+$` and
    // `AgreementCancellationRequestId` is `@length(1, 64)` `^acr-[a-zA-Z0-9]+$`
    // (`smithy_query(query="operation",
    // operation="AcceptAgreementCancellationRequest,GetAgreementCancellationRequest")`),
    // so every id below satisfies its member's constraints and reaches the
    // lookup, including both published length bounds of the request id.
    let rows: Vec<(&str, Value, &str, String)> = vec![
        (
            "an agreement id naming no agreement",
            pair("A", &request_id),
            AGREEMENT,
            "A".to_string(),
        ),
        (
            "the longest agreement id, 64 characters",
            pair(&"z".repeat(64), &request_id),
            AGREEMENT,
            "z".repeat(64),
        ),
        (
            "the shortest request id the acr- pattern admits",
            pair(&agreement_id, "acr-0"),
            AGREEMENT_CANCELLATION_REQUEST,
            "acr-0".to_string(),
        ),
        (
            "the longest AgreementCancellationRequestId, 64 characters",
            pair(&agreement_id, &format!("acr-{}", "A".repeat(60))),
            AGREEMENT_CANCELLATION_REQUEST,
            format!("acr-{}", "A".repeat(60)),
        ),
        (
            "a request that belongs to another agreement",
            pair(&agreement_id, &other_request),
            AGREEMENT_CANCELLATION_REQUEST,
            other_request.clone(),
        ),
    ];

    for operation in [
        "AcceptAgreementCancellationRequest",
        "GetAgreementCancellationRequest",
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

    // No refused call decided anything: both stored requests, and both
    // agreements, are exactly as they were.
    for (agreement_id, request_id) in [
        (&agreement_id, &request_id),
        (&other_agreement, &other_request),
    ] {
        let read = get_cancellation(&router, pair(agreement_id, request_id)).await;
        assert_eq!(
            read.str_at("/status"),
            PENDING_APPROVAL,
            "a refused lookup must not have decided {request_id}, got {:?}",
            read.body
        );
        assert_eq!(
            agreement_status(&router, agreement_id).await,
            AGREEMENT_ACTIVE,
            "a refused lookup must not have cancelled {agreement_id}"
        );
    }
}

/// Reading a cancellation request is a report: repeated reads answer the same
/// record, and what a later caller sees is unchanged by them.
#[tokio::test]
async fn reading_a_cancellation_request_changes_nothing() {
    let router = emulator();
    let agreement_id = agreement(&router, "ap-reports").await;
    let request_id = cancellation_over(&router, &agreement_id, "PRODUCT_DISCONTINUED").await;
    let body = pair(&agreement_id, &request_id);

    let first = get_cancellation(&router, body.clone()).await;
    assert_eq!(
        first.status, 200,
        "reading {request_id} must succeed, got {} with {:?}",
        first.status, first.body
    );
    assert_eq!(
        first.str_at("/status"),
        PENDING_APPROVAL,
        "a request nothing has decided is {PENDING_APPROVAL}, got {:?}",
        first.body
    );

    for read in 1..4 {
        let again = get_cancellation(&router, body.clone()).await;
        assert_eq!(
            again.body, first.body,
            "read {read}: a report must answer the same record, got {:?}",
            again.body
        );
    }
    assert_eq!(
        agreement_status(&router, &agreement_id).await,
        AGREEMENT_ACTIVE,
        "reading a cancellation request must not touch its agreement"
    );

    // And the request is still decidable afterwards: no read consumed it.
    let accepted = accept_cancellation(&router, body).await;
    assert_eq!(
        (accepted.status, accepted.str_at("/status")),
        (200, APPROVED.to_string()),
        "the request stayed decidable after being read, got {:?}",
        accepted.body
    );
}
