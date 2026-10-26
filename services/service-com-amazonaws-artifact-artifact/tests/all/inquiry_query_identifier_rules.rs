// SPDX-License-Identifier: CC0-1.0

//! The two compliance-inquiry rules that `ExportComplianceInquiry` and
//! `PutComplianceInquiryFeedback` both apply, driven end to end over HTTP through one
//! operation from each former copy.
//!
//! RULE 1, OWNERSHIP: a query identifier a request carries must name a query of the
//! targeted inquiry, and the question "does this inquiry own that identifier?" has ONE
//! answer — the inquiry's own query set — whichever operation asks it. So an identifier an
//! inquiry owns is served by the export and by the feedback submission alike, and one it
//! does not is refused by both with the modelled `ValidationException` naming the member
//! that carried it (`queryIdentifiers` on an export, `queryIdentifier` on a submission)
//! and the one requirement its value failed.
//!
//! RULE 2, THE SET NORMALISATION: a list member that names a SET rather than a sequence is
//! recorded ascending and without repeats, because neither the order a request listed its
//! values in nor a repeat of one is part of what it named. The export's `queryIdentifiers`
//! and the submission's `reasonCodes` are both such a member.
//!
//! Both rules are observed where the emulator makes them observable: the answer each
//! operation gives, and the exported document, which is rendered from the one stored
//! inquiry record (its `queryIdentifiers` key is the covered set, and its `feedback`
//! entries carry the recorded reason codes).
//!
//! The rows differ along what the rules generalise over — how many queries an inquiry owns
//! (none, one, several), which identifier is probed (each owned one, below the first, past
//! the last, negative, and both bounds of the `Integer` the model declares for
//! `QueryIdentifiersList`'s member), and, for the normalisation, the ORDER, the REPEATS and
//! the SIZE of the list a request carried.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The router bootstrap and the request sender are the crate's test harness': one emulator
// over one shared engine, reached the way a client reaches it.
use super::harness::{call, emulator, Answer};

/// Base64 of an uploaded document's bytes, as restJson1 carries a blob.
const FILE_CONTENT: &str = "cXVlc3Rpb25uYWlyZQ==";

/// `FeedbackRating`'s values. Source:
/// `smithy_query(query="shape",shape="FeedbackRating")`.
const RATING: &str = "THUMBS_DOWN";

/// `FeedbackReasonCode`'s values, in the order the model lists them. Source:
/// `smithy_query(query="shape",shape="FeedbackReasonCode")`.
const REASON_CODES: [&str; 3] = ["OTHER", "PARTIAL_RESPONSE", "IRRELEVANT_RESPONSE"];

/// The requirement half of the clause both operations state for an identifier naming no
/// query of the targeted inquiry.
const UNKNOWN_QUERY_IDENTIFIER: &str =
    "every query identifier must name a query of the compliance inquiry";

/// The clause the refusal states, against the member that carried the value.
fn clause(member: &str) -> String {
    format!("Value at '{member}' failed to satisfy constraint: {UNKNOWN_QUERY_IDENTIFIER}")
}

async fn post(router: &MultiServiceRouter, uri: &str, body: &Value) -> Answer {
    call(router, "POST", uri, Bytes::from(body.to_string())).await
}

async fn create(router: &MultiServiceRouter, body: &Value) -> String {
    let created = post(router, "/v1/compliance-inquiry/create", body).await;
    assert_eq!(
        created.status, 202,
        "the control create must be accepted, got {} with {:?}",
        created.status, created.body
    );
    created.body["complianceInquirySummary"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("the create carries no inquiry id: {:?}", created.body))
        .to_string()
}

/// `ExportComplianceInquiry`, naming `query_identifiers` when the case names any.
async fn export(
    router: &MultiServiceRouter,
    inquiry_id: &str,
    query_identifiers: Option<&[i32]>,
) -> Answer {
    let mut body = json!({ "complianceInquiryId": inquiry_id });
    if let Some(identifiers) = query_identifiers {
        body["queryIdentifiers"] = json!(identifiers);
    }
    post(router, "/v1/compliance-inquiry/export", &body).await
}

/// `PutComplianceInquiryFeedback` on `query_identifier`, against `revision` — a response
/// version of its own per case, because one version accepts exactly one submission.
async fn put_feedback(
    router: &MultiServiceRouter,
    inquiry_id: &str,
    query_identifier: Option<i32>,
    revision: i32,
    reason_codes: Option<&[&str]>,
) -> Answer {
    let mut body = json!({
        "complianceInquiryId": inquiry_id,
        "rating": RATING,
        "responseRevisionId": revision,
    });
    if let Some(identifier) = query_identifier {
        body["queryIdentifier"] = json!(identifier);
    }
    if let Some(codes) = reason_codes {
        body["reasonCodes"] = json!(codes);
    }
    call(
        router,
        "PUT",
        "/v1/compliance-inquiry/putFeedback",
        Bytes::from(body.to_string()),
    )
    .await
}

/// The exported DOCUMENT of `inquiry_id`, covering the queries `query_identifiers` names:
/// where the stored record is observable, followed exactly as a client follows a presigned
/// URL — against the host and path the URL names, with no credential of any kind.
async fn exported_document(
    router: &MultiServiceRouter,
    case: &str,
    inquiry_id: &str,
    query_identifiers: Option<&[i32]>,
) -> Value {
    let exported = export(router, inquiry_id, query_identifiers).await;
    assert_eq!(
        exported.status, 200,
        "case {case:?}: the export must be served, got {} with {:?}",
        exported.status, exported.body
    );
    let url = exported.body["documentPresignedUrl"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the export carries no document URL: {:?}",
                exported.body
            )
        })
        .to_string();
    let rest = url
        .split_once("://")
        .unwrap_or_else(|| panic!("case {case:?}: {url:?} is not an absolute URL"))
        .1;
    let (host, path) = match rest.find('/') {
        Some(mark) => (&rest[..mark], &rest[mark..]),
        None => (rest, "/"),
    };
    let request = ::http::Request::builder()
        .method("GET")
        .uri(path)
        .header("host", host)
        .body(Bytes::new())
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "case {case:?}: following {url:?} must serve the document"
    );
    let bytes = response.into_body();
    serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        panic!(
            "case {case:?}: the exported document must be JSON ({error}): {:?}",
            String::from_utf8_lossy(&bytes)
        )
    })
}

/// One inquiry shape and the query identifiers it owns.
struct Shape {
    label: &'static str,
    body: Value,
    owned: &'static [i32],
}

/// The inquiry shapes under test: a single submitted query owns the one query its content
/// carried, an upload owns one query per unit of work it was submitted in, and an upload
/// carrying no bytes has nothing to process and so owns none.
fn shapes() -> Vec<Shape> {
    vec![
        Shape {
            label: "a text inquiry, one query",
            body: json!({
                "name": "Data residency",
                "inquiryContent": { "query": "Where is customer data stored?" }
            }),
            owned: &[1],
        },
        Shape {
            label: "an upload in three sections, three queries",
            body: json!({
                "name": "Customer DDQ",
                "inquiryContent": {
                    "fileContent": {
                        "content": FILE_CONTENT,
                        "fileSections": ["Sheet 1", "Sheet 2", "Sheet 3"],
                    }
                }
            }),
            owned: &[1, 2, 3],
        },
        Shape {
            label: "an upload carrying no bytes, no query at all",
            body: json!({
                "name": "Empty workbook",
                "inquiryContent": { "fileContent": { "content": "" } }
            }),
            owned: &[],
        },
    ]
}

/// The identifiers every shape is probed with: the ones it owns, the one below the first,
/// the one past the last, a negative one, and both bounds of the `Integer` the model
/// declares for `QueryIdentifiersList`'s member
/// (`smithy_query(query="shape",shape="QueryIdentifiersList")`, which bounds it in no
/// other way).
fn probed(owned: &[i32]) -> Vec<i32> {
    let past_the_last = i32::try_from(owned.len()).unwrap_or(i32::MAX).saturating_add(1);
    let mut identifiers = owned.to_vec();
    identifiers.extend([0, past_the_last, -1, i32::MIN, i32::MAX]);
    identifiers
}

/// Assert `answer` is the modelled refusal for an identifier naming no query, stated
/// against `member`.
fn assert_refused(answer: &Answer, case: &str, member: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: an identifier naming no query must be refused with HTTP 400, got \
         {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ValidationException",
        "case {case:?}: expected the modelled ValidationException, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.body["reason"].as_str(),
        Some("fieldValidationFailed"),
        "case {case:?}: expected the fieldValidationFailed reason, got {:?}",
        answer.body["reason"]
    );
    assert_eq!(
        answer.body["fieldList"][0]["name"].as_str(),
        Some(member),
        "case {case:?}: the refusal must name the member that carried the value, got {:?}",
        answer.body["fieldList"]
    );
    assert_eq!(
        answer.body["message"].as_str(),
        Some(format!("1 validation error detected: {}", clause(member)).as_str()),
        "case {case:?}: expected the validation envelope stating the clause, got {:?}",
        answer.body["message"]
    );
}

#[tokio::test]
async fn one_ownership_rule_decides_both_query_identifier_members() {
    let router = emulator();

    for shape in shapes() {
        let inquiry_id = create(&router, &shape.body).await;
        for (index, identifier) in probed(shape.owned).into_iter().enumerate() {
            let case = format!("{}: identifier {identifier}", shape.label);
            let owns = shape.owned.contains(&identifier);

            // Copy 1's operation: `queryIdentifiers` on the export.
            let exported = export(&router, &inquiry_id, Some(&[identifier])).await;
            // Copy 2's operation: `queryIdentifier` on a feedback submission. A response
            // version of its own per probe, so the one-feedback-per-version rule never
            // decides this case.
            let revision = i32::try_from(index).unwrap_or(0) + 1;
            let feedback =
                put_feedback(&router, &inquiry_id, Some(identifier), revision, None).await;

            assert_eq!(
                exported.status == 200,
                feedback.status == 200,
                "case {case:?}: both operations ask one ownership question, so they must \
                 agree; the export answered {} and the submission {} ({:?} / {:?})",
                exported.status,
                feedback.status,
                exported.body,
                feedback.body
            );

            if owns {
                assert_eq!(
                    exported.status, 200,
                    "case {case:?}: the inquiry owns that identifier, so the export must \
                     be served, got {} with {:?}",
                    exported.status, exported.body
                );
                assert_eq!(
                    feedback.status, 200,
                    "case {case:?}: the inquiry owns that identifier, so the submission \
                     must be recorded, got {} with {:?}",
                    feedback.status, feedback.body
                );
            } else {
                assert_refused(&exported, &case, "queryIdentifiers");
                assert_refused(&feedback, &case, "queryIdentifier");
                assert!(
                    exported.body["documentPresignedUrl"].is_null(),
                    "case {case:?}: a refused export serves no URL, got {:?}",
                    exported.body["documentPresignedUrl"]
                );
                assert!(
                    feedback.body["submittedAt"].is_null(),
                    "case {case:?}: a refused submission answers no timestamp, got {:?}",
                    feedback.body["submittedAt"]
                );
            }
        }
    }
}

/// One list a request carried, and the set it names.
struct SetCase {
    label: &'static str,
    /// The `queryIdentifiers` an export names, over an inquiry owning 1, 2 and 3.
    submitted_identifiers: &'static [i32],
    /// The set that list names: ascending, without repeats.
    normalised_identifiers: &'static [i32],
    /// The `reasonCodes` a feedback submission names.
    submitted_reason_codes: &'static [&'static str],
    /// The set that list names: ascending, without repeats.
    normalised_reason_codes: &'static [&'static str],
}

/// The lists under test. The rows differ along the ORDER a request listed the values in,
/// the REPEATS it carried and the SIZE of the list: already a set, reversed, shuffled, a
/// single value repeated, and every published value with a repeat among them.
fn set_cases() -> Vec<SetCase> {
    vec![
        SetCase {
            label: "already ascending and without repeats",
            submitted_identifiers: &[1, 2],
            normalised_identifiers: &[1, 2],
            submitted_reason_codes: &["IRRELEVANT_RESPONSE", "OTHER"],
            normalised_reason_codes: &["IRRELEVANT_RESPONSE", "OTHER"],
        },
        SetCase {
            label: "descending",
            submitted_identifiers: &[3, 2, 1],
            normalised_identifiers: &[1, 2, 3],
            submitted_reason_codes: &["PARTIAL_RESPONSE", "OTHER", "IRRELEVANT_RESPONSE"],
            normalised_reason_codes: &["IRRELEVANT_RESPONSE", "OTHER", "PARTIAL_RESPONSE"],
        },
        SetCase {
            label: "shuffled",
            submitted_identifiers: &[2, 3, 1],
            normalised_identifiers: &[1, 2, 3],
            submitted_reason_codes: &["OTHER", "IRRELEVANT_RESPONSE"],
            normalised_reason_codes: &["IRRELEVANT_RESPONSE", "OTHER"],
        },
        SetCase {
            label: "one value, repeated",
            submitted_identifiers: &[2, 2, 2],
            normalised_identifiers: &[2],
            submitted_reason_codes: &["OTHER", "OTHER"],
            normalised_reason_codes: &["OTHER"],
        },
        SetCase {
            label: "every value, out of order and with a repeat",
            submitted_identifiers: &[3, 1, 2, 3, 1],
            normalised_identifiers: &[1, 2, 3],
            submitted_reason_codes: &[
                "PARTIAL_RESPONSE",
                "IRRELEVANT_RESPONSE",
                "OTHER",
                "PARTIAL_RESPONSE",
            ],
            normalised_reason_codes: &["IRRELEVANT_RESPONSE", "OTHER", "PARTIAL_RESPONSE"],
        },
    ]
}

#[tokio::test]
async fn a_list_that_names_a_set_is_normalised_the_one_way() {
    let router = emulator();
    // An inquiry owning the three identifiers the rows name, so a row's list is refused by
    // the ownership rule nowhere and only the normalisation is under test.
    let inquiry_id = create(
        &router,
        &json!({
            "name": "Customer DDQ",
            "inquiryContent": {
                "fileContent": {
                    "content": FILE_CONTENT,
                    "fileSections": ["Sheet 1", "Sheet 2", "Sheet 3"],
                }
            }
        }),
    )
    .await;

    for (index, case) in set_cases().into_iter().enumerate() {
        let label = case.label;

        // Copy 1's member: the export covers the SET its `queryIdentifiers` named, which
        // the document it rendered reports.
        let document = exported_document(
            &router,
            label,
            &inquiry_id,
            Some(case.submitted_identifiers),
        )
        .await;
        assert_eq!(
            document["queryIdentifiers"],
            json!(case.normalised_identifiers),
            "case {label:?}: the export of {:?} must cover {:?}, got {:?}",
            case.submitted_identifiers,
            case.normalised_identifiers,
            document["queryIdentifiers"]
        );
        let queries = document["queries"].as_array().unwrap_or_else(|| {
            panic!(
                "case {label:?}: the document must carry a queries list, got {:?}",
                document["queries"]
            )
        });
        assert_eq!(
            queries.len(),
            case.normalised_identifiers.len(),
            "case {label:?}: the document must report one query per covered identifier, \
             got {queries:?}"
        );

        // Copy 2's member: the submission records the SET its `reasonCodes` named, which
        // the exported document reports back. A response version of its own per row.
        let revision = i32::try_from(index).unwrap_or(0) + 1;
        let feedback = put_feedback(
            &router,
            &inquiry_id,
            None,
            revision,
            Some(case.submitted_reason_codes),
        )
        .await;
        assert_eq!(
            feedback.status, 200,
            "case {label:?}: the submission must be recorded, got {} with {:?}",
            feedback.status, feedback.body
        );

        let document = exported_document(&router, label, &inquiry_id, None).await;
        let entries = document["feedback"].as_array().unwrap_or_else(|| {
            panic!(
                "case {label:?}: the document must carry a feedback list, got {:?}",
                document["feedback"]
            )
        });
        let recorded = entries
            .iter()
            .find(|entry| entry["responseRevisionId"] == json!(revision))
            .unwrap_or_else(|| {
                panic!(
                    "case {label:?}: the document must report the submission on revision \
                     {revision}, got {entries:?}"
                )
            });
        assert_eq!(
            recorded["reasonCodes"],
            json!(case.normalised_reason_codes),
            "case {label:?}: the submission of {:?} must be recorded as {:?}, got {:?}",
            case.submitted_reason_codes,
            case.normalised_reason_codes,
            recorded["reasonCodes"]
        );
    }

    // The published value set the rows are drawn from, so a row naming a value the model
    // does not declare would be a test bug rather than a case.
    for case in set_cases() {
        for code in case.submitted_reason_codes {
            assert!(
                REASON_CODES.contains(code),
                "case {:?}: {code:?} is not one of the published reason codes {REASON_CODES:?}",
                case.label
            );
        }
    }
}
