// SPDX-License-Identifier: CC0-1.0

//! The two end-to-end rules the implemented compliance-inquiry operations obey, each
//! asserted over every operation it covers:
//!
//! 1. A well-formed `InquiryId` the account holds no inquiry for names no inquiry the
//!    caller may reach, so EVERY operation targeting an inquiry answers it with the front
//!    door's HTTP 401 `UnauthorizedException` and that gateway's constant `Unauthorized`
//!    text — not a modelled 404, which would confirm the resource's absence. The judgement
//!    is on the TARGET, which is what the same four operations serving an inquiry the
//!    account does hold is the evidence for.
//! 2. `PutComplianceInquiryFeedbackResponse$submittedAt` targets `TimestampAttribute`,
//!    which the model marks `@timestampFormat("date-time")`
//!    (`smithy_query(query="shape",shape="TimestampAttribute")`), so a submission that is
//!    recorded reports it on restJson1 as an RFC 3339 / ISO-8601 date-time STRING — never
//!    as the protocol's default epoch-seconds number — naming an instant no earlier than
//!    the inquiry it rates was created at.
//!
//! Both rules are STATED by suites that cover other resources too: `front_door_errors.rs`
//! states rule 1 over every target class of this service, and `timestamp_wire_form.rs`
//! states rule 2 over every timestamp member it models. The rows here are the ones those
//! suites reach only through the four operations' own identifier syntax (rule 1) or only
//! through the member schema (rule 2).
//!
//! Every assertion goes through the router, so each case exercises the same path a client
//! takes: routing, the front door, deserialization, the handler, the persisted state and
//! serialization.

use std::time::Duration;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, emulator, get, Answer};

/// The front door's authorization refusal: the status, the wire error code, and the
/// gateway's own constant text, which names neither member nor resource.
const UNAUTHORIZED: (u16, &str, &str) = (401, "UnauthorizedException", "Unauthorized");

/// `FeedbackRating`'s values. Source:
/// `smithy_query(query="shape",shape="FeedbackRating")`.
const RATINGS: [&str; 2] = ["THUMBS_UP", "THUMBS_DOWN"];

/// `FeedbackReasonCode`'s values. Source:
/// `smithy_query(query="shape",shape="FeedbackReasonCode")`.
const REASON_CODES: [&str; 3] = ["OTHER", "PARTIAL_RESPONSE", "IRRELEVANT_RESPONSE"];

/// Base64 of an uploaded document's bytes, as restJson1 carries a blob.
const FILE_CONTENT: &str = "cXVlc3Rpb25uYWlyZQ==";

/// The span after which an inquiry's processing is due to be over, plus slack. The
/// handlers store the due instant rather than sleeping, so only the test waits.
///
/// `std::thread::sleep` rather than `tokio::time::sleep`: the workspace pins tokio without
/// its `time` feature.
const PAST_THE_PROCESSING_SPAN: Duration = Duration::from_millis(900);

/// Well-formed inquiry ids the account holds no inquiry for — a target judged against this
/// service's STATE rather than against the identifier syntax. Spelled out from
/// `InquiryId`'s `^compliance-inquiry-[a-zA-Z0-9]{16}$`
/// (`smithy_query(query="shape",shape="InquiryId")`) rather than taken from the crate, so a
/// wrong predicate in `src/state.rs` cannot make this suite pass. The rows differ along
/// what the 16-character suffix generalises over: its alphabet and its case.
const UNHELD_INQUIRY_IDS: &[(&str, &str)] = &[
    ("all digits", "compliance-inquiry-0000000000000000"),
    ("all lowercase", "compliance-inquiry-zzzzzzzzzzzzzzzz"),
    ("all uppercase", "compliance-inquiry-ZZZZZZZZZZZZZZZZ"),
    ("mixed case and digits", "compliance-inquiry-AbCdEf0123456789"),
    ("leading and trailing digits", "compliance-inquiry-0aBcDeFgHiJkLmN9"),
];

/// A JSON body as restJson1 carries it.
fn json_body(body: &Value) -> Bytes {
    Bytes::from(body.to_string())
}

/// The `message` an error body carries, or the empty string when it carries none.
fn message(answer: &Answer) -> &str {
    answer.body["message"].as_str().unwrap_or_default()
}

/// One request per operation that targets a compliance inquiry, each at its modelled URI:
/// the identifier is bound to the query string on two of them and to the request body on
/// the other two, and every one of them is implemented.
async fn every_inquiry_operation(
    router: &MultiServiceRouter,
    id: &str,
) -> Vec<(&'static str, Answer)> {
    vec![
        (
            "GetComplianceInquiryMetadata",
            get(
                router,
                &format!("/v1/compliance-inquiry/getMetadata?complianceInquiryId={id}"),
            )
            .await,
        ),
        (
            "ListComplianceInquiryQueries",
            get(
                router,
                &format!("/v1/compliance-inquiry/listQueries?complianceInquiryId={id}"),
            )
            .await,
        ),
        (
            "ExportComplianceInquiry",
            call(
                router,
                "POST",
                "/v1/compliance-inquiry/export",
                json_body(&json!({ "complianceInquiryId": id })),
            )
            .await,
        ),
        (
            "PutComplianceInquiryFeedback",
            call(
                router,
                "PUT",
                "/v1/compliance-inquiry/putFeedback",
                json_body(&json!({ "complianceInquiryId": id, "rating": RATINGS[0] })),
            )
            .await,
        ),
    ]
}

/// A `CreateComplianceInquiry` body for a single submitted query — a text inquiry, which
/// owns exactly the one query its content carried (identifier 1).
fn text_body(name: &str, query: &str) -> Value {
    json!({ "name": name, "inquiryContent": { "query": query } })
}

/// A `CreateComplianceInquiry` body for an uploaded document, processed as one unit.
fn file_body(name: &str) -> Value {
    json!({ "name": name, "inquiryContent": { "fileContent": { "content": FILE_CONTENT } } })
}

/// Create one inquiry and answer its id together with the instant it reports as created
/// at — the earliest instant any feedback on it can have been submitted.
async fn held_inquiry(router: &MultiServiceRouter, body: &Value) -> (String, String) {
    let created = call(
        router,
        "POST",
        "/v1/compliance-inquiry/create",
        json_body(body),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "the control create must succeed, got {} with {:?}",
        created.status, created.body
    );
    let summary = &created.body["complianceInquirySummary"];
    let id = summary["id"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "the create reports the minted inquiry id: {:?}",
                created.body
            )
        })
        .to_string();
    let created_at = summary["createdAt"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "the create reports the inquiry's createdAt: {:?}",
                created.body
            )
        })
        .to_string();
    (id, created_at)
}

/// Assert `answer` is the front door's authorization refusal: the status, the wire error
/// code a restJson1 client resolves the shape from, and the gateway's constant message.
fn assert_refused(case: &str, answer: &Answer) {
    let (status, code, text) = UNAUTHORIZED;
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected HTTP {status}, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        code,
        "case {case:?}: expected __type {code}, got {:?}",
        answer.body
    );
    assert_eq!(
        message(answer),
        text,
        "case {case:?}: expected message {text:?}, got {:?}",
        answer.body
    );
}

/// Assert a `TimestampAttribute` member's wire form: a JSON STRING that parses as an RFC
/// 3339 date-time and renders back to the very same text, naming a real instant.
///
/// Parsed and re-rendered rather than compared against a literal: the instant is the
/// environment's clock, so what is under test is the JSON type and the lexical form.
fn assert_date_time(case: &str, member: &str, value: &Value) -> DateTime {
    assert!(
        !value.is_number(),
        "case {case:?}: {member} targets TimestampAttribute (@timestampFormat date-time), so it \
         must be an RFC 3339 string and never an epoch-seconds number, got {value:?}"
    );
    let text = value.as_str().unwrap_or_else(|| {
        panic!("case {case:?}: {member} must be an RFC 3339 date-time string, got {value:?}")
    });
    let parsed = DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: {member} must parse as a date-time ({error}), got {text:?}")
    });
    let rendered = parsed
        .fmt(Format::DateTime)
        .unwrap_or_else(|error| panic!("case {case:?}: {member} does not re-render ({error})"));
    assert_eq!(
        rendered, text,
        "case {case:?}: {member} must already be in the canonical date-time form, got {text:?} \
         against {rendered:?}"
    );
    assert!(
        parsed.secs() > 0,
        "case {case:?}: {member} must name a real instant, got {text:?}"
    );
    parsed
}

#[tokio::test]
async fn every_inquiry_operation_refuses_a_well_formed_id_the_account_holds_no_inquiry_for() {
    let router = emulator();

    for (case, id) in UNHELD_INQUIRY_IDS {
        for (operation, answer) in every_inquiry_operation(&router, id).await {
            assert_refused(&format!("{operation} unheld id, {case}"), &answer);
        }
    }

    // And holding ONE inquiry does not make another account-held id of them: the refusal is
    // keyed on the targeted inquiry, not on the store being empty.
    let (held, _) = held_inquiry(
        &router,
        &text_body("Data residency", "Where is customer data stored?"),
    )
    .await;
    assert!(
        !UNHELD_INQUIRY_IDS.iter().any(|(_, id)| *id == held),
        "the minted id must not be one of the ids this suite calls unheld, got {held:?}"
    );
    for (case, id) in UNHELD_INQUIRY_IDS {
        for (operation, answer) in every_inquiry_operation(&router, id).await {
            assert_refused(
                &format!("{operation} unheld id with another inquiry held, {case}"),
                &answer,
            );
        }
    }
}

#[tokio::test]
async fn every_inquiry_operation_serves_an_inquiry_the_account_holds() {
    let router = emulator();
    let (held, _) = held_inquiry(
        &router,
        &text_body("Subprocessors", "Which subprocessors handle customer content?"),
    )
    .await;

    for (operation, answer) in every_inquiry_operation(&router, &held).await {
        assert!(
            (200..300).contains(&answer.status),
            "case {operation:?}: an inquiry the account holds is a target the front door \
             authorizes, so the operation must serve it, got {} with {:?}",
            answer.status,
            answer.body
        );
    }
}

#[tokio::test]
async fn a_recorded_submission_reports_submitted_at_as_an_rfc3339_date_time_string() {
    // (case, the inquiry's submitted content, the members the submission names besides the
    // inquiry it targets, whether the inquiry reached its terminal status first). The rows
    // differ along what rule 2 generalises over: the rating, whether reason codes, a
    // comment, a query identifier, a response revision or a client token is named, the form
    // the inquiry was submitted in, and whether the clock-owed transition has fallen due.
    let cases: Vec<(&str, Value, Value, bool)> = vec![
        (
            "thumbs up on the inquiry's response as a whole",
            text_body("Data residency", "Where is customer data stored?"),
            json!({ "rating": RATINGS[0] }),
            false,
        ),
        (
            "thumbs down on one query, with reason codes, a comment and a revision",
            text_body("Subprocessors", "Which subprocessors handle customer content?"),
            json!({
                "rating": RATINGS[1],
                "queryIdentifier": 1,
                "reasonCodes": [REASON_CODES[1], REASON_CODES[2]],
                "comment": "The answer covers one region only.",
                "responseRevisionId": 4,
            }),
            false,
        ),
        (
            "on an uploaded document's inquiry, under a client token",
            file_body("Vendor questionnaire"),
            json!({
                "rating": RATINGS[0],
                "queryIdentifier": 1,
                "responseRevisionId": 2,
                "clientToken": "token-submitted-at",
            }),
            false,
        ),
        (
            "after the inquiry reached its terminal status",
            text_body("Encryption", "Is customer content encrypted at rest?"),
            json!({ "rating": RATINGS[1], "reasonCodes": [REASON_CODES[0]] }),
            true,
        ),
    ];

    for (case, content, submission, after_terminal) in cases {
        let router = emulator();
        let (inquiry_id, created_at) = held_inquiry(&router, &content).await;
        if after_terminal {
            std::thread::sleep(PAST_THE_PROCESSING_SPAN);
        }

        let mut body = submission;
        body["complianceInquiryId"] = json!(inquiry_id);
        let answered = call(
            &router,
            "PUT",
            "/v1/compliance-inquiry/putFeedback",
            json_body(&body),
        )
        .await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: the submission must be recorded, got {} with {:?}",
            answered.status, answered.body
        );

        let reported = answered.body["submittedAt"].clone();
        assert!(
            !reported.is_null(),
            "case {case:?}: submittedAt is @required, so the answer must carry it, got {:?}",
            answered.body
        );
        let submitted_at = assert_date_time(case, "submittedAt", &reported);

        // The instant comes from the clock the call ran on, not from a literal: it cannot
        // precede the creation of the inquiry the feedback is on.
        let created_at = assert_date_time(case, "the inquiry's createdAt", &json!(created_at));
        assert!(
            submitted_at.secs() >= created_at.secs(),
            "case {case:?}: the feedback was submitted after the inquiry was created, so \
             submittedAt must not precede createdAt, got {reported:?} against \
             {created_at:?}"
        );
    }
}
