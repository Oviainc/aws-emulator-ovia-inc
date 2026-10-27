// SPDX-License-Identifier: CC0-1.0

//! Reading one stored agreement cancellation request, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the
//! same path a client takes.
//!
//! The rule under test: `GetAgreementCancellationRequest` REPORTS the one stored
//! cancellation request the agreement it names holds under the id it names —
//! every member of its answer is computed from that record, it carries no member
//! the output shape does not model, it reports a `statusMessage` only when some
//! earlier call supplied one, and it writes nothing, so a later caller sees
//! exactly what it saw before the read. An id pair that names no stored record
//! is refused with the modelled `ResourceNotFoundException`, resolved PARENT
//! FIRST, and such a refusal stores nothing either.
//!
//! The rows span what the rule generalises over: the reason code the request was
//! created with (every value of `AgreementCancellationRequestReasonCode`),
//! whether the optional description was supplied and its length at both
//! published bounds of `AgreementCancellationRequestDescription`, every
//! `AgreementCancellationRequestStatus` a call of this service can move a
//! request to, the length of the message a decision stores at both published
//! bounds, and which id of the required pair names nothing.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::agreement_cancellation_requests::{
    agreement, cancellation_over, is_cancellation_request_id, list, send, PENDING_APPROVAL,
    REASON_CODES,
};
use super::agreement_lifecycle::{call, describe, emulator, Answer};

/// The members `GetAgreementCancellationRequestOutput` models
/// (`smithy_query(query="operation", operation="GetAgreementCancellationRequest")`).
/// Listed here rather than imported from the crate so an answer carrying
/// something the shape does not model fails this test.
const OUTPUT_MEMBERS: &[&str] = &[
    "agreementCancellationRequestId",
    "agreementId",
    "createdAt",
    "description",
    "reasonCode",
    "status",
    "statusMessage",
    "updatedAt",
];

/// The `AgreementCancellationRequestStatus` values the calls of this service can
/// put a request into: the send's initial one is [`PENDING_APPROVAL`], the
/// buyer's accept and reject give `APPROVED` and `REJECTED`, and the seller's
/// withdrawal gives `CANCELLED`. `VALIDATION_FAILED` is the fifth modelled value
/// and no call of this service reaches it, so no row below claims it.
const APPROVED: &str = "APPROVED";
/// See [`APPROVED`].
const REJECTED: &str = "REJECTED";
/// See [`APPROVED`].
const CANCELLED: &str = "CANCELLED";

/// `AgreementStatus`'s value an accepted agreement carries — what the agreement
/// a request was merely READ over has to keep, since reading cancels nothing.
const AGREEMENT_ACTIVE: &str = "ACTIVE";

/// The resource kinds a lookup names in `resourceType` — `ResourceType`'s values
/// (`smithy_query(query="shape", shape="ResourceType")`).
const AGREEMENT: &str = "Agreement";
/// See [`AGREEMENT`].
const AGREEMENT_CANCELLATION_REQUEST: &str = "AgreementCancellationRequest";

/// The id pair that names one stored cancellation request.
fn pair(agreement_id: &str, request_id: &str) -> Value {
    json!({
        "agreementId": agreement_id,
        "agreementCancellationRequestId": request_id,
    })
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

/// The optional `description` member of a read answer, as a reader sees it: an
/// absent member and a null one are the same absence.
fn description_of(answer: &Answer) -> Option<&str> {
    answer.body.get("description").and_then(Value::as_str)
}

/// The `statusMessage` of a read answer, under the same absence rule as
/// [`description_of`].
fn status_message_of(answer: &Answer) -> Option<&str> {
    answer.body.get("statusMessage").and_then(Value::as_str)
}

/// A timestamp member of an answer, in the epoch seconds this protocol renders
/// them as.
fn instant_of(answer: &Answer, member: &str) -> f64 {
    answer
        .body
        .get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("{member} must be a timestamp, got {:?}", answer.body))
}

/// How a send was shaped, and so what the read of its request must report.
struct Row {
    case: String,
    reason_code: &'static str,
    description: Option<String>,
}

/// One row per reason code, cycling the forms the optional description takes.
///
/// The description forms span `AgreementCancellationRequestDescription`'s
/// `@length(1, 2000)` (`smithy_query(query="shape",
/// shape="AgreementCancellationRequestDescription")`): absent, the shortest the
/// bound admits, the longest it admits, and an ordinary sentence.
fn rows() -> Vec<Row> {
    let descriptions: [Option<String>; 4] = [
        None,
        Some("x".to_string()),
        Some("d".repeat(2000)),
        Some("Replaced by a renegotiated agreement, see PO 44/7_1-9.".to_string()),
    ];
    REASON_CODES
        .iter()
        .enumerate()
        .map(|(index, reason_code)| {
            let description = descriptions[index % descriptions.len()].clone();
            let shape = match description.as_deref() {
                None => "no description".to_string(),
                Some(text) => format!("a description of {} characters", text.len()),
            };
            Row {
                case: format!("reasonCode {reason_code}, {shape}"),
                reason_code,
                description,
            }
        })
        .collect()
}

/// Send one cancellation request shaped by `row` over a fresh agreement, and
/// answer the agreement's id together with the send's own answer.
async fn stored_request(router: &MultiServiceRouter, index: usize, row: &Row) -> (String, Answer) {
    let agreement_id = agreement(router, &format!("ap-acrread{index}")).await;
    let mut body = json!({"agreementId": agreement_id, "reasonCode": row.reason_code});
    if let Some(description) = row.description.as_deref() {
        body["description"] = json!(description);
    }
    let sent = send(router, body).await;
    assert_eq!(
        sent.status, 200,
        "case {:?}: the send must succeed, got {} with {:?}",
        row.case, sent.status, sent.body
    );
    (agreement_id, sent)
}

// ---------------------------------------------------------------------------
// What a read reports
// ---------------------------------------------------------------------------

/// Every member of a read answer is computed from the stored record: the ids and
/// the reason code the send recorded, the status the send left the request in,
/// the description exactly when one was supplied, the instants the send stamped
/// — and no member the output shape does not model.
#[tokio::test]
async fn the_read_reports_the_stored_record() {
    let router = emulator();

    for (index, row) in rows().iter().enumerate() {
        let case = &row.case;
        let (agreement_id, sent) = stored_request(&router, index, row).await;
        let request_id = sent.str_at("/agreementCancellationRequestId");

        let read = get_cancellation(&router, &agreement_id, &request_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the stored request must read back, got {} with {:?}",
            read.status, read.body
        );

        assert_eq!(
            read.str_at("/agreementCancellationRequestId"),
            request_id,
            "case {case:?}: the read names the request the send minted, got {:?}",
            read.body
        );
        assert!(
            is_cancellation_request_id(&read.str_at("/agreementCancellationRequestId")),
            "case {case:?}: the reported id must match ^acr-[a-zA-Z0-9]+$, got {:?}",
            read.body
        );
        assert_eq!(
            read.str_at("/agreementId"),
            agreement_id,
            "case {case:?}: the read names the agreement the request belongs to, got {:?}",
            read.body
        );
        assert_eq!(
            read.str_at("/reasonCode"),
            row.reason_code,
            "case {case:?}: the read reports the stored reason code, got {:?}",
            read.body
        );
        assert_eq!(
            read.str_at("/status"),
            PENDING_APPROVAL,
            "case {case:?}: a request no call has decided is still pending, got {:?}",
            read.body
        );
        assert_eq!(
            description_of(&read),
            row.description.as_deref(),
            "case {case:?}: the read reports the stored description exactly when there is one, \
             got a description of {:?} characters",
            description_of(&read).map(str::len)
        );
        assert_eq!(
            status_message_of(&read),
            None,
            "case {case:?}: no call has supplied a status message yet, got {:?}",
            read.body
        );
        assert_eq!(
            read.body.get("createdAt"),
            sent.body.get("createdAt"),
            "case {case:?}: the read reports the instant the send stamped, got {:?} against {:?}",
            read.body.get("createdAt"),
            sent.body.get("createdAt")
        );
        assert_eq!(
            read.body.get("updatedAt"),
            sent.body.get("updatedAt"),
            "case {case:?}: the read reports the stored updatedAt, got {:?} against {:?}",
            read.body.get("updatedAt"),
            sent.body.get("updatedAt")
        );
        assert_eq!(
            instant_of(&read, "createdAt"),
            instant_of(&read, "updatedAt"),
            "case {case:?}: nothing has updated the request since the send, got {:?}",
            read.body
        );

        let answered: Vec<&String> = read
            .body
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: the answer is a structure, got {:?}", read.body))
            .keys()
            .filter(|member| !OUTPUT_MEMBERS.contains(&member.as_str()))
            .collect();
        assert!(
            answered.is_empty(),
            "case {case:?}: the answer carries members the output shape does not model: \
             {answered:?}"
        );
    }
}

/// Reading reports a `statusMessage` only once a call has supplied one: the
/// buyer's rejection reason and the seller's withdrawal reason become it, while
/// a pending request and an approved one — neither of which any call gave a
/// message for — carry none.
///
/// The rows span the statuses a call of this service can reach and, for the two
/// that carry a message, both published length bounds of the member that
/// supplies it (`@length(1, 2000)`).
#[tokio::test]
async fn the_read_reports_the_message_a_call_supplied() {
    /// How a request was moved before it was read: the operation, the member
    /// that carries its reason, the reason itself, and what the read must then
    /// report.
    struct Decision {
        case: &'static str,
        operation: Option<(&'static str, &'static str)>,
        reason: Option<String>,
        status: &'static str,
    }

    let decisions = [
        Decision {
            case: "a request no call has decided",
            operation: None,
            reason: None,
            status: PENDING_APPROVAL,
        },
        Decision {
            case: "a request the buyer approved",
            operation: Some(("AcceptAgreementCancellationRequest", "")),
            reason: None,
            status: APPROVED,
        },
        Decision {
            case: "a request the buyer rejected with the shortest reason",
            operation: Some(("RejectAgreementCancellationRequest", "rejectionReason")),
            reason: Some("n".to_string()),
            status: REJECTED,
        },
        Decision {
            case: "a request the buyer rejected with the longest reason",
            operation: Some(("RejectAgreementCancellationRequest", "rejectionReason")),
            reason: Some("r".repeat(2000)),
            status: REJECTED,
        },
        Decision {
            case: "a request the seller withdrew with the shortest reason",
            operation: Some(("CancelAgreementCancellationRequest", "cancellationReason")),
            reason: Some("w".to_string()),
            status: CANCELLED,
        },
        Decision {
            case: "a request the seller withdrew with the longest reason",
            operation: Some(("CancelAgreementCancellationRequest", "cancellationReason")),
            reason: Some("c".repeat(2000)),
            status: CANCELLED,
        },
    ];

    let router = emulator();
    for (index, decision) in decisions.iter().enumerate() {
        let case = decision.case;
        let agreement_id = agreement(&router, &format!("ap-acrmsg{index}")).await;
        let request_id = cancellation_over(&router, &agreement_id, "OTHER").await;
        let before = get_cancellation(&router, &agreement_id, &request_id).await;

        if let Some((operation, member)) = decision.operation {
            let mut body = pair(&agreement_id, &request_id);
            if let (Some(reason), false) = (decision.reason.as_deref(), member.is_empty()) {
                body[member] = json!(reason);
            }
            let moved = call(&router, operation, body).await;
            assert_eq!(
                moved.status, 200,
                "case {case:?}: {operation} must succeed, got {} with {:?}",
                moved.status, moved.body
            );
        }

        let read = get_cancellation(&router, &agreement_id, &request_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the request must read back, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.str_at("/status"),
            decision.status,
            "case {case:?}: expected status {:?}, got {:?}",
            decision.status,
            read.body
        );
        assert_eq!(
            status_message_of(&read),
            decision.reason.as_deref(),
            "case {case:?}: the read reports the message the call supplied, got a message of \
             {:?} characters",
            status_message_of(&read).map(str::len)
        );
        assert_eq!(
            read.body.get("createdAt"),
            before.body.get("createdAt"),
            "case {case:?}: a decision leaves createdAt as the send stamped it, got {:?} \
             against {:?}",
            read.body.get("createdAt"),
            before.body.get("createdAt")
        );
        assert!(
            instant_of(&read, "updatedAt") >= instant_of(&read, "createdAt"),
            "case {case:?}: updatedAt is never before createdAt, got {:?}",
            read.body
        );
    }
}

// ---------------------------------------------------------------------------
// A read changes nothing
// ---------------------------------------------------------------------------

/// Reading is a report: repeating it answers the same body every time, the
/// listing of the same agreement still reports the same record, the agreement
/// itself stays active, and the request is still decidable afterwards — so no
/// read moved a status or consumed the one decision the request has.
#[tokio::test]
async fn reading_changes_nothing_a_later_caller_sees() {
    let router = emulator();
    let agreement_id = agreement(&router, "ap-acrreport").await;
    let sent = send(
        &router,
        json!({
            "agreementId": agreement_id,
            "reasonCode": "REPLACING_AGREEMENT",
            "description": "Superseded by the agreement that replaces it",
        }),
    )
    .await;
    assert_eq!(
        sent.status, 200,
        "the send must succeed, got {} with {:?}",
        sent.status, sent.body
    );
    let request_id = sent.str_at("/agreementCancellationRequestId");

    let first = get_cancellation(&router, &agreement_id, &request_id).await;
    assert_eq!(
        first.status, 200,
        "the stored request must read back, got {} with {:?}",
        first.status, first.body
    );
    for read_number in 2..=4 {
        let again = get_cancellation(&router, &agreement_id, &request_id).await;
        assert_eq!(
            again.status, first.status,
            "case read {read_number}: a repeated read answers the same status, got {} with {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.body, first.body,
            "case read {read_number}: a repeated read answers the same record, got {:?} \
             against {:?}",
            again.body, first.body
        );
    }

    let listing = list(
        &router,
        json!({"partyType": "Proposer", "agreementId": agreement_id}),
    )
    .await;
    let items = listing.body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("items is a required member, got {:?}", listing.body))
        .clone();
    assert_eq!(
        items.len(),
        1,
        "case after the reads: the agreement still holds its one request, got {:?}",
        listing.body
    );
    assert_eq!(
        items[0].get("status"),
        Some(&json!(PENDING_APPROVAL)),
        "case after the reads: the listing reports the request still pending, got {:?}",
        listing.body
    );

    let described = describe(&router, &agreement_id).await;
    assert_eq!(
        described.str_at("/status"),
        AGREEMENT_ACTIVE,
        "case after the reads: reading a cancellation request cancels nothing, got {:?}",
        described.body
    );

    // The one decision the request has is still open: the reads neither took it
    // nor moved the request out of the only status it can be decided in.
    let mut rejection = pair(&agreement_id, &request_id);
    rejection["rejectionReason"] = json!("The subscription is still in use");
    let rejected = call(&router, "RejectAgreementCancellationRequest", rejection).await;
    assert_eq!(
        rejected.status, 200,
        "case after the reads: the request is still decidable, got {} with {:?}",
        rejected.status, rejected.body
    );
}

// ---------------------------------------------------------------------------
// Selection by the required id pair
// ---------------------------------------------------------------------------

/// Both ids select, and resolution goes parent first: an agreement id that names
/// no agreement is answered about the AGREEMENT, and an agreement that does
/// exist with a request id it does not hold — including a request stored under
/// another agreement — is answered about the CANCELLATION REQUEST. Neither
/// refusal stores anything.
///
/// The rows span which id of the pair names nothing and the form of that id
/// within its modelled pattern, including both published length bounds.
#[tokio::test]
async fn an_id_pair_that_names_no_stored_request_is_not_found() {
    let router = emulator();
    let agreement_id = agreement(&router, "ap-acrpairs").await;
    let request_id = cancellation_over(&router, &agreement_id, "OTHER").await;
    let other_agreement = agreement(&router, "ap-acrpairs2").await;
    let other_request = cancellation_over(&router, &other_agreement, "TEST_AGREEMENT").await;

    // `AgreementId` is `@length(1, 64)` `^[A-Za-z0-9_/-]+$` and
    // `AgreementCancellationRequestId` is `@length(1, 64)`
    // `^acr-[a-zA-Z0-9]+$` (`smithy_query(query="operation",
    // operation="GetAgreementCancellationRequest")`), so every id below
    // satisfies its member's constraints and reaches the lookup, including both
    // published length bounds.
    let rows: Vec<(&str, String, String, &str, String, &str)> = vec![
        (
            "the shortest agreement id, naming no agreement",
            "A".to_string(),
            request_id.clone(),
            AGREEMENT,
            "A".to_string(),
            "agreement",
        ),
        (
            "the longest agreement id, 64 characters",
            "z".repeat(64),
            request_id.clone(),
            AGREEMENT,
            "z".repeat(64),
            "agreement",
        ),
        (
            "an unknown agreement named together with an unknown request",
            "no/such-agreement_1".to_string(),
            "acr-0".to_string(),
            AGREEMENT,
            "no/such-agreement_1".to_string(),
            "agreement",
        ),
        (
            "the shortest request id the acr- pattern admits",
            agreement_id.clone(),
            "acr-0".to_string(),
            AGREEMENT_CANCELLATION_REQUEST,
            "acr-0".to_string(),
            "agreement cancellation request",
        ),
        (
            "the longest AgreementCancellationRequestId, 64 characters",
            agreement_id.clone(),
            format!("acr-{}", "A".repeat(60)),
            AGREEMENT_CANCELLATION_REQUEST,
            format!("acr-{}", "A".repeat(60)),
            "agreement cancellation request",
        ),
        (
            "a request that belongs to another agreement",
            agreement_id.clone(),
            other_request.clone(),
            AGREEMENT_CANCELLATION_REQUEST,
            other_request.clone(),
            "agreement cancellation request",
        ),
    ];

    for (case, agreement_named, request_named, resource_type, resource_id, noun) in &rows {
        let answer = get_cancellation(&router, agreement_named, request_named).await;
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
        let message = answer.str_at("/message");
        assert!(
            message.contains(noun),
            "case {case:?}: the refusal says which kind of resource does not exist, expected a \
             message naming {noun:?}, got {message:?}"
        );
    }

    // No refused read created or moved anything: both stored requests are
    // exactly as they were, each reachable only under its own agreement.
    for (agreement_named, request_named) in [
        (&agreement_id, &request_id),
        (&other_agreement, &other_request),
    ] {
        let read = get_cancellation(&router, agreement_named, request_named).await;
        assert_eq!(
            read.status, 200,
            "case after the refusals: {request_named} must still read back under \
             {agreement_named}, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.str_at("/status"),
            PENDING_APPROVAL,
            "case after the refusals: {request_named} is still pending, got {:?}",
            read.body
        );
        assert_eq!(
            status_message_of(&read),
            None,
            "case after the refusals: no call has supplied a status message, got {:?}",
            read.body
        );
    }
}
