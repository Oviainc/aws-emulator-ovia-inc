// SPDX-License-Identifier: CC0-1.0

//! What a QUESTION-LEVEL `PutComplianceInquiryFeedback` submission must carry, over HTTP.
//!
//! ONE rule, asserted over a SET of submissions rather than any single one: **feedback
//! scoped to one query of an inquiry — a submission naming `queryIdentifier` — is feedback
//! on a VERSION of that query's response, so it must name the `responseRevisionId` of the
//! version it is about; one that names none is refused with the modelled 400
//! `ValidationException` and records nothing.** Feedback on the inquiry's response as a
//! whole names no query, so it names no revision of one either and the requirement does not
//! apply.
//!
//! WHY: `responseRevisionId` is documented as "The response revision ID. Use this value to
//! prevent submitting feedback on a stale response" (`smithy_query` shape
//! `PutComplianceInquiryFeedbackRequest`), so a question-level submission without it is a
//! rating with nothing saying which response it rates — exactly the stale-response
//! attribution the member exists to prevent.
//!
//! The rows differ along what the rule generalises over: the rating, how many reason codes
//! the request lists, whether it carries a comment or a `clientToken`, which query of the
//! inquiry it names, how many queries that inquiry owns, and the form the inquiry was
//! submitted in (a typed query against an uploaded document with sections).
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, emulator, follow_announced_url, Answer};

/// The status, wire error code and fixed sentence this service refuses a question-level
/// submission naming no revision with. Spelled out here rather than imported from the crate
/// so a changed constant fails this suite instead of redefining what it asserts.
const REVISION_REQUIRED: (u16, &str, &str) = (
    400,
    "ValidationException",
    "expectedResponseRevisionId is required for question-level feedback submission.",
);

/// `ValidationExceptionReason`'s value this refusal reports. Source:
/// `smithy_query(query="shape",shape="ValidationExceptionReason")`.
const FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// `FeedbackRating`'s values. Source:
/// `smithy_query(query="shape",shape="FeedbackRating")`.
const RATINGS: [&str; 2] = ["THUMBS_UP", "THUMBS_DOWN"];

/// `FeedbackReasonCode`'s values. Source:
/// `smithy_query(query="shape",shape="FeedbackReasonCode")`.
const REASON_CODES: [&str; 3] = ["OTHER", "PARTIAL_RESPONSE", "IRRELEVANT_RESPONSE"];

/// Base64 of an uploaded document's bytes, as restJson1 carries a blob.
const FILE_CONTENT: &str = "cXVlc3Rpb25uYWlyZQ==";

/// A `CreateComplianceInquiry` body carrying one typed query, which owns query 1.
fn typed_inquiry(name: &str, query: &str) -> Value {
    json!({ "name": name, "inquiryContent": { "query": query } })
}

/// A `CreateComplianceInquiry` body carrying an uploaded document submitted in `sections`
/// units of work, which owns one query per section.
fn uploaded_inquiry(name: &str, sections: &[&str]) -> Value {
    json!({
        "name": name,
        "inquiryContent": {
            "fileContent": { "content": FILE_CONTENT, "fileSections": sections }
        }
    })
}

/// Create one inquiry and answer with its id.
async fn held_inquiry(router: &MultiServiceRouter, case: &str, body: &Value) -> String {
    let created = call(
        router,
        "POST",
        "/v1/compliance-inquiry/create",
        Bytes::from(body.to_string()),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case {case:?}: the create must be accepted, got {} with {:?}",
        created.status, created.body
    );
    created.body["complianceInquirySummary"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the create carries an id, got {:?}", created.body))
        .to_string()
}

/// `PUT /v1/compliance-inquiry/putFeedback` with `body`.
async fn put_feedback(router: &MultiServiceRouter, body: &Value) -> Answer {
    call(
        router,
        "PUT",
        "/v1/compliance-inquiry/putFeedback",
        Bytes::from(body.to_string()),
    )
    .await
}

/// The feedback the inquiry holds, read back through the one implemented reader that
/// reports it: the document `ExportComplianceInquiry` announces, followed the way a client
/// follows an announced URL (the harness owns that fetch).
async fn recorded_feedback(router: &MultiServiceRouter, inquiry_id: &str) -> Vec<Value> {
    let exported = call(
        router,
        "POST",
        "/v1/compliance-inquiry/export",
        Bytes::from(json!({ "complianceInquiryId": inquiry_id }).to_string()),
    )
    .await;
    assert_eq!(
        exported.status, 200,
        "the export must be served, got {} with {:?}",
        exported.status, exported.body
    );
    let url = exported.body["documentPresignedUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("the export announces a URL, got {:?}", exported.body))
        .to_string();
    let served = follow_announced_url(router, "the exported document", &url).await;
    assert_eq!(
        served.status, 200,
        "following {url:?} must serve the exported document, got {}",
        served.status
    );
    let document: Value = serde_json::from_slice(&served.bytes).unwrap_or_else(|error| {
        panic!(
            "the exported document is not JSON ({error}): {:?}",
            String::from_utf8_lossy(&served.bytes)
        )
    });
    document["feedback"]
        .as_array()
        .unwrap_or_else(|| panic!("the document carries a feedback list, got {document:?}"))
        .clone()
}

/// One submission the rule refuses or admits: the inquiry it is made on, the members the
/// request carries besides `complianceInquiryId`, and whether the revision requirement
/// applies to it.
struct Case {
    label: &'static str,
    inquiry: Value,
    /// The request's members other than the inquiry id.
    submission: Value,
}

/// The question-level submissions that name NO revision — every one of them refused.
fn without_a_revision() -> Vec<Case> {
    vec![
        Case {
            label: "the one query of a typed inquiry, nothing else named",
            inquiry: typed_inquiry("Data residency", "Where is customer data stored?"),
            submission: json!({ "rating": RATINGS[0], "queryIdentifier": 1 }),
        },
        Case {
            label: "the same query with every reason code and a comment",
            inquiry: typed_inquiry("Encryption", "Is customer data encrypted at rest?"),
            submission: json!({
                "rating": RATINGS[1],
                "queryIdentifier": 1,
                "reasonCodes": REASON_CODES,
                "comment": "The answer missed the EU regions."
            }),
        },
        Case {
            label: "the same query under a clientToken",
            inquiry: typed_inquiry("Retention", "How long are logs kept?"),
            submission: json!({
                "rating": RATINGS[0],
                "queryIdentifier": 1,
                "clientToken": "token-no-revision"
            }),
        },
        Case {
            label: "the first query of an upload submitted in three sections",
            inquiry: uploaded_inquiry("Customer DDQ", &["Sheet 1", "Sheet 2", "Sheet 3"]),
            submission: json!({
                "rating": RATINGS[1],
                "queryIdentifier": 1,
                "reasonCodes": [REASON_CODES[1]]
            }),
        },
        Case {
            label: "the last query of an upload submitted in three sections",
            inquiry: uploaded_inquiry("Vendor questionnaire", &["A", "B", "C"]),
            submission: json!({ "rating": RATINGS[0], "queryIdentifier": 3 }),
        },
    ]
}

/// The submissions the requirement does not reach — each recorded. Either they name a
/// revision beside the query, or they name no query at all.
fn admitted() -> Vec<Case> {
    vec![
        Case {
            label: "the one query of a typed inquiry at its first revision",
            inquiry: typed_inquiry("Data residency", "Where is customer data stored?"),
            submission: json!({
                "rating": RATINGS[0],
                "queryIdentifier": 1,
                "responseRevisionId": 1
            }),
        },
        Case {
            label: "a query at revision zero, which is a revision like any other",
            inquiry: typed_inquiry("Encryption", "Is customer data encrypted at rest?"),
            submission: json!({
                "rating": RATINGS[1],
                "queryIdentifier": 1,
                "responseRevisionId": 0,
                "comment": "Covers one region only."
            }),
        },
        Case {
            label: "a query at the largest revision an Integer carries",
            inquiry: typed_inquiry("Audit", "When was the last audit?"),
            submission: json!({
                "rating": RATINGS[0],
                "queryIdentifier": 1,
                "responseRevisionId": i32::MAX,
                "reasonCodes": [REASON_CODES[2]]
            }),
        },
        Case {
            label: "the third query of an upload at a revision of its own",
            inquiry: uploaded_inquiry("Customer DDQ", &["Sheet 1", "Sheet 2", "Sheet 3"]),
            submission: json!({
                "rating": RATINGS[1],
                "queryIdentifier": 3,
                "responseRevisionId": 4
            }),
        },
        Case {
            label: "the inquiry's response as a whole, naming neither query nor revision",
            inquiry: typed_inquiry("Subprocessors", "Who are the subprocessors?"),
            submission: json!({ "rating": RATINGS[0], "reasonCodes": [REASON_CODES[0]] }),
        },
        Case {
            label: "the inquiry's response as a whole at a named revision",
            inquiry: typed_inquiry("Backups", "How often are backups taken?"),
            submission: json!({
                "rating": RATINGS[1],
                "responseRevisionId": 2,
                "clientToken": "token-whole-response"
            }),
        },
    ]
}

/// The request body a case sends: its submission members plus the inquiry it addresses.
fn body_for(case: &Case, inquiry_id: &str) -> Value {
    let mut body = case.submission.clone();
    body["complianceInquiryId"] = json!(inquiry_id);
    body
}

#[tokio::test]
async fn a_question_level_submission_naming_no_response_revision_is_refused() {
    let (status, code, sentence) = REVISION_REQUIRED;

    for case in without_a_revision() {
        let label = case.label;
        let router = emulator();
        let inquiry_id = held_inquiry(&router, label, &case.inquiry).await;

        let refused = put_feedback(&router, &body_for(&case, &inquiry_id)).await;
        assert_eq!(
            refused.status, status,
            "case {label:?}: expected HTTP {status}, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            code,
            "case {label:?}: expected the modelled {code}, got {:?}",
            refused.body
        );
        assert_eq!(
            refused.body["message"].as_str(),
            Some(sentence),
            "case {label:?}: expected the fixed sentence {sentence:?}, got {:?}",
            refused.body
        );
        assert_eq!(
            refused.body["reason"].as_str(),
            Some(FIELD_VALIDATION_FAILED),
            "case {label:?}: expected reason {FIELD_VALIDATION_FAILED}, got {:?}",
            refused.body
        );
        assert!(
            refused.body["submittedAt"].is_null(),
            "case {label:?}: a refused submission answers no submittedAt, got {:?}",
            refused.body
        );

        // And nothing was recorded: the rating is not attached to whatever revision
        // happened to be current.
        let feedback = recorded_feedback(&router, &inquiry_id).await;
        assert!(
            feedback.is_empty(),
            "case {label:?}: a refused submission must record nothing, got {feedback:?}"
        );
    }
}

#[tokio::test]
async fn a_submission_the_requirement_does_not_reach_is_recorded() {
    for case in admitted() {
        let label = case.label;
        let router = emulator();
        let inquiry_id = held_inquiry(&router, label, &case.inquiry).await;

        let body = body_for(&case, &inquiry_id);
        let answered = put_feedback(&router, &body).await;
        assert_eq!(
            answered.status, 200,
            "case {label:?}: the submission must be recorded, got {} with {:?}",
            answered.status, answered.body
        );
        let submitted_at = answered.body["submittedAt"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: expected submittedAt, got {:?}", answered.body))
            .to_string();

        // Read back through the one implemented reader that reports the record: what was
        // recorded is the version the request named.
        let feedback = recorded_feedback(&router, &inquiry_id).await;
        assert_eq!(
            feedback,
            vec![json!({
                "queryIdentifier": body["queryIdentifier"],
                "rating": body["rating"],
                "reasonCodes": body["reasonCodes"]
                    .as_array()
                    .map_or_else(Vec::new, |codes| {
                        let mut sorted: Vec<&Value> = codes.iter().collect();
                        sorted.sort_by_key(|code| code.as_str().unwrap_or_default());
                        sorted.dedup();
                        sorted
                    }),
                "comment": body["comment"],
                "responseRevisionId": body["responseRevisionId"],
                "submittedAt": submitted_at,
            })],
            "case {label:?}: the stored feedback must be the submission, got {feedback:?}"
        );
    }
}

#[tokio::test]
async fn the_requirement_is_judged_after_the_query_the_request_named() {
    // A question-level submission that names no revision AND no query of the inquiry is
    // refused for the query first: which query the request named is the first thing about a
    // question-level submission, so the identifier's own refusal states its member and the
    // requirement it failed. The rows differ along how the identifier misses.
    let router = emulator();
    let inquiry_id = held_inquiry(
        &router,
        "a typed inquiry owning one query",
        &typed_inquiry("Residency", "Where is data stored?"),
    )
    .await;

    for (case, identifier) in [
        ("one past the queries it owns", 2),
        ("below the first identifier", 0),
        ("a negative identifier", -1),
        ("the largest Integer", i32::MAX),
    ] {
        let refused = put_feedback(
            &router,
            &json!({
                "complianceInquiryId": inquiry_id,
                "rating": RATINGS[0],
                "queryIdentifier": identifier
            }),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: an identifier the inquiry does not own is refused, got {} with {:?}",
            refused.status, refused.body
        );
        let message = refused.body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("queryIdentifier"),
            "case {case:?}: the refusal must name the identifier's own member, got {message:?}"
        );
        assert_ne!(
            message,
            REVISION_REQUIRED.2,
            "case {case:?}: the identifier is judged before the revision requirement, got \
             {message:?}"
        );
    }
}
