// SPDX-License-Identifier: CC0-1.0

//! `PutComplianceInquiryFeedback`, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones the operation states:
//!
//! * a submission is RECORDED on the targeted inquiry and answered with the instant it was
//!   recorded, and that record is what the document `ExportComplianceInquiry` renders
//!   reports back — whatever rating, reason codes, comment or revision the request carried;
//! * one RESPONSE VERSION — the `queryIdentifier` and `responseRevisionId` a request names
//!   together — accepts exactly ONE submitted feedback: a second submission against a
//!   version that already carries feedback is refused with the modelled
//!   `ConflictException`, whatever `clientToken` it carries (a fresh one, none, or the
//!   first request's), while a submission against another version of the same query,
//!   another query, or another inquiry is a record of its own;
//! * `clientToken` is the operation's `@idempotencyToken`, and what the model says a token
//!   this service has already recorded does, it says without exception: the request is
//!   IGNORED — nothing further is recorded — and answered with the `submittedAt` the call
//!   that recorded the token answered, however the rest of that request differs from it, so
//!   no refusal of this operation is keyed on a token;
//! * `queryIdentifier` must name a query of THAT inquiry, so one it does not own is
//!   refused with the modelled `ValidationException` naming the member;
//! * an inquiry id the store does not hold is refused the way every other inquiry
//!   operation refuses it — the front door's 401 `UnauthorizedException`;
//! * recording feedback changes nothing a later reader of the inquiry's own record sees.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_artifact_artifact::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the suite signs for.
const REGION: &str = "eu-west-1";

/// The span after which an inquiry's processing is due to be over, plus slack. The
/// handlers store the due instant rather than sleeping, so only the test waits.
///
/// `std::thread::sleep` rather than `tokio::time::sleep`: the workspace pins tokio
/// without its `time` feature.
const PAST_THE_PROCESSING_SPAN: Duration = Duration::from_millis(900);

/// Base64 of an uploaded document's bytes, as restJson1 carries a blob.
const FILE_CONTENT: &str = "cXVlc3Rpb25uYWlyZQ==";

/// `FeedbackRating`'s values. Source:
/// `smithy_query(query="shape",shape="FeedbackRating")`.
const RATINGS: [&str; 2] = ["THUMBS_UP", "THUMBS_DOWN"];

/// `FeedbackReasonCode`'s values. Source:
/// `smithy_query(query="shape",shape="FeedbackReasonCode")`.
const REASON_CODES: [&str; 3] = ["OTHER", "PARTIAL_RESPONSE", "IRRELEVANT_RESPONSE"];

/// The message the service states for a submission against a response version that already
/// carries feedback, as the recorded AWS refusal carries it. The requirement under test is
/// the CONDITION it names — one feedback per response version — together with the status
/// and the error code; the text is pinned because the service states this one fixed
/// sentence for the condition rather than building it from the request.
const ALREADY_SUBMITTED: &str = "Feedback already submitted for this response version.";

/// A SigV4 credential scope for `REGION` naming this service — what routes the request.
fn signed() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/artifact/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The `submittedAt` a feedback submission answered with.
    fn submitted_at(&self) -> &str {
        self.body["submittedAt"].as_str().unwrap_or_default()
    }
}

async fn send(router: &MultiServiceRouter, request: ::http::Request<Bytes>) -> Answer {
    let uri = request.uri().to_string();
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "artifact.eu-west-1.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", signed())
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    send(router, request).await
}

async fn create(router: &MultiServiceRouter, body: &Value) -> Answer {
    call(router, "POST", "/v1/compliance-inquiry/create", body).await
}

async fn put_feedback(router: &MultiServiceRouter, body: &Value) -> Answer {
    call(router, "PUT", "/v1/compliance-inquiry/putFeedback", body).await
}

async fn get_metadata(router: &MultiServiceRouter, inquiry_id: &str) -> Answer {
    let request = ::http::Request::builder()
        .method("GET")
        .uri(format!(
            "/v1/compliance-inquiry/getMetadata?complianceInquiryId={inquiry_id}"
        ))
        .header("host", "artifact.eu-west-1.amazonaws.com")
        .header("authorization", signed())
        .body(Bytes::new())
        .expect("the test request is well formed");
    send(router, request).await
}

/// Create one inquiry and answer its id.
async fn held_inquiry(router: &MultiServiceRouter, body: &Value) -> String {
    let created = create(router, body).await;
    assert_eq!(
        created.status, 202,
        "the control create must succeed, got {} with {:?}",
        created.status, created.body
    );
    created.body["complianceInquirySummary"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("the create carries no inquiry id: {:?}", created.body))
        .to_string()
}

/// A `CreateComplianceInquiry` body for a single submitted query — a TEXT inquiry, which
/// owns exactly the one query its content carried (identifier 1).
fn text_body(name: &str, query: &str) -> Value {
    json!({ "name": name, "inquiryContent": { "query": query } })
}

/// A `CreateComplianceInquiry` body for an uploaded document — a FILE inquiry, which owns
/// one query per unit of work it is processed in: the sections it names, or the document
/// as a whole under the submitted title when it names none.
fn file_body(name: &str, sections: &[&str]) -> Value {
    let mut file = json!({ "content": FILE_CONTENT });
    if !sections.is_empty() {
        file["fileSections"] = json!(sections);
    }
    json!({ "name": name, "inquiryContent": { "fileContent": file } })
}

/// A `CreateComplianceInquiry` body for an upload carrying no bytes — an inquiry with
/// nothing to process, which therefore owns no query at all.
fn empty_file_body(name: &str) -> Value {
    json!({
        "name": name,
        "inquiryContent": { "fileContent": { "content": "" } }
    })
}

/// The feedback the EXPORTED DOCUMENT of `inquiry_id` reports, which is where the stored
/// record is observable: the export is rendered from the one inquiry record this service
/// holds.
async fn exported_feedback(router: &MultiServiceRouter, inquiry_id: &str) -> Vec<Value> {
    let exported = call(
        router,
        "POST",
        "/v1/compliance-inquiry/export",
        &json!({ "complianceInquiryId": inquiry_id }),
    )
    .await;
    assert_eq!(
        exported.status, 200,
        "the control export must succeed, got {} with {:?}",
        exported.status, exported.body
    );
    let url = exported.body["documentPresignedUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("the export carries no document URL: {:?}", exported.body))
        .to_string();

    // Followed exactly as a client follows a presigned URL: against the host and path the
    // URL names, with no credential of any kind.
    let rest = url
        .split_once("://")
        .unwrap_or_else(|| panic!("{url:?} is not an absolute URL"))
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
        "following {url:?} must serve the document"
    );
    let bytes = response.into_body();
    let document: Value = serde_json::from_slice(&bytes).unwrap_or_else(|error| {
        panic!(
            "the exported document is not JSON ({error}): {:?}",
            String::from_utf8_lossy(&bytes)
        )
    });
    document["feedback"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "the document must carry a feedback list, got {:?}",
                document["feedback"]
            )
        })
        .clone()
}

/// One `PutComplianceInquiryFeedback` request, in the members a caller names.
#[derive(Clone, Debug)]
struct Submission {
    /// The query the feedback is on, or `None` to name none — which records it on the
    /// inquiry's response as a whole.
    query_identifier: Option<i32>,
    rating: &'static str,
    /// The reason codes as the request lists them, or `None` to name none.
    reason_codes: Option<&'static [&'static str]>,
    comment: Option<&'static str>,
    response_revision_id: Option<i32>,
    client_token: Option<&'static str>,
}

impl Submission {
    /// A submission naming only the two required members.
    fn rated(rating: &'static str) -> Self {
        Self {
            query_identifier: None,
            rating,
            reason_codes: None,
            comment: None,
            response_revision_id: None,
            client_token: None,
        }
    }

    fn on_query(mut self, identifier: i32) -> Self {
        self.query_identifier = Some(identifier);
        self
    }

    fn on_no_query(mut self) -> Self {
        self.query_identifier = None;
        self
    }

    fn rated_as(mut self, rating: &'static str) -> Self {
        self.rating = rating;
        self
    }

    fn because(mut self, codes: &'static [&'static str]) -> Self {
        self.reason_codes = Some(codes);
        self
    }

    fn commenting(mut self, comment: &'static str) -> Self {
        self.comment = Some(comment);
        self
    }

    fn without_comment(mut self) -> Self {
        self.comment = None;
        self
    }

    fn on_revision(mut self, revision: i32) -> Self {
        self.response_revision_id = Some(revision);
        self
    }

    fn with_token(mut self, token: &'static str) -> Self {
        self.client_token = Some(token);
        self
    }

    /// The request body, leaving out every member the submission names no value for —
    /// which is how a caller names none.
    fn body(&self, inquiry_id: &str) -> Value {
        let mut body = json!({ "complianceInquiryId": inquiry_id, "rating": self.rating });
        if let Some(identifier) = self.query_identifier {
            body["queryIdentifier"] = json!(identifier);
        }
        if let Some(codes) = self.reason_codes {
            body["reasonCodes"] = json!(codes);
        }
        if let Some(comment) = self.comment {
            body["comment"] = json!(comment);
        }
        if let Some(revision) = self.response_revision_id {
            body["responseRevisionId"] = json!(revision);
        }
        if let Some(token) = self.client_token {
            body["clientToken"] = json!(token);
        }
        body
    }

    /// The reason codes as the record stores them: ascending and without repeats, because
    /// which reasons were given is a set.
    fn stored_reason_codes(&self) -> Vec<&'static str> {
        let mut codes = self.reason_codes.unwrap_or(&[]).to_vec();
        codes.sort_unstable();
        codes.dedup();
        codes
    }

    /// The document entry this submission must be reported as.
    fn expected_entry(&self, submitted_at: &str) -> Value {
        json!({
            "queryIdentifier": self.query_identifier,
            "rating": self.rating,
            "reasonCodes": self.stored_reason_codes(),
            "comment": self.comment,
            "responseRevisionId": self.response_revision_id,
            "submittedAt": submitted_at,
        })
    }
}

/// One recorded submission: the inquiry it is made on, and the request that makes it.
struct Case {
    label: &'static str,
    /// The `CreateComplianceInquiry` body the inquiry is created from.
    inquiry: Value,
    submission: Submission,
    /// Whether the case waits past the processing span before submitting, so the inquiry
    /// is terminal rather than `PROCESSING` — feedback is recorded either way.
    after_terminal: bool,
}

/// The submissions this suite records. The rows differ along everything the rule
/// generalises over: which rating, how many reason codes and in which order, whether a
/// comment, a revision id or a client token is named, whether the feedback is on a query
/// or on the inquiry as a whole, the form the inquiry was submitted in, and whether the
/// inquiry has reached a terminal status yet.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "thumbs up on the inquiry, nothing else named",
            inquiry: text_body("Data residency", "Where is customer data stored?"),
            submission: Submission::rated(RATINGS[0]),
            after_terminal: false,
        },
        Case {
            label: "thumbs down on the one query, with every reason code and a comment",
            inquiry: text_body("Encryption", "Is data encrypted at rest?"),
            submission: Submission::rated(RATINGS[1])
                .on_query(1)
                .because(&REASON_CODES)
                .commenting("The answer missed the EU regions."),
            after_terminal: true,
        },
        Case {
            label: "one reason code, a revision id and a token",
            inquiry: text_body("Retention", "How long are logs kept?"),
            submission: Submission::rated(RATINGS[1])
                .on_query(1)
                .because(&[REASON_CODES[2]])
                .on_revision(3)
                .with_token("token-one-reason"),
            after_terminal: false,
        },
        Case {
            label: "reason codes repeated and out of order are stored as the set they are",
            inquiry: text_body("Subprocessors", "Who are the subprocessors?"),
            submission: Submission::rated(RATINGS[0])
                .on_query(1)
                .because(&[
                    REASON_CODES[1],
                    REASON_CODES[0],
                    REASON_CODES[1],
                    REASON_CODES[2],
                ]),
            after_terminal: true,
        },
        Case {
            label: "an uploaded document, rated as a whole after it went terminal",
            inquiry: file_body("Questionnaire", &[]),
            submission: Submission::rated(RATINGS[1])
                .because(&[REASON_CODES[0]])
                .commenting("The upload was processed as one unit.")
                .with_token("token-file-inquiry"),
            after_terminal: true,
        },
        Case {
            label: "the largest revision id an Integer carries",
            inquiry: text_body("Audit", "When was the last audit?"),
            submission: Submission::rated(RATINGS[0])
                .on_query(1)
                .on_revision(i32::MAX),
            after_terminal: false,
        },
    ]
}

#[tokio::test]
async fn a_submission_is_recorded_on_the_inquiry_and_answered_with_the_instant_it_was_recorded() {
    for case in cases() {
        let label = case.label;
        let router = emulator();
        let inquiry_id = held_inquiry(&router, &case.inquiry).await;
        if case.after_terminal {
            std::thread::sleep(PAST_THE_PROCESSING_SPAN);
        }

        let answered = put_feedback(&router, &case.submission.body(&inquiry_id)).await;
        assert_eq!(
            answered.status, 200,
            "case {label:?}: a submission on a held inquiry must be recorded, got {} with \
             {:?}",
            answered.status, answered.body
        );
        let submitted_at = answered.submitted_at().to_string();
        assert!(
            submitted_at.contains('T') && submitted_at.ends_with('Z'),
            "case {label:?}: submittedAt is a date-time TimestampAttribute, got {:?}",
            answered.body["submittedAt"]
        );

        // The record, read back through the one implemented reader that reports it.
        let feedback = exported_feedback(&router, &inquiry_id).await;
        assert_eq!(
            feedback,
            vec![case.submission.expected_entry(&submitted_at)],
            "case {label:?}: the stored feedback must be the submission, got {feedback:?}"
        );
    }
}

#[tokio::test]
async fn a_repeated_submission_under_one_client_token_replays_the_first_answer() {
    // Each row is a SECOND request that is the same request as the first: the members are
    // the same, down to reason codes a request may list in any order and more than once.
    const TOKEN: &str = "token-replayed";
    let first = Submission::rated(RATINGS[1])
        .on_query(1)
        .because(&[REASON_CODES[0], REASON_CODES[1]])
        .commenting("Partial answer.")
        .on_revision(2)
        .with_token(TOKEN);
    let repeats: Vec<(&str, Submission)> = vec![
        ("the identical body", first.clone()),
        (
            "the reason codes in another order",
            first.clone().because(&[REASON_CODES[1], REASON_CODES[0]]),
        ),
        (
            "a reason code repeated",
            first
                .clone()
                .because(&[REASON_CODES[0], REASON_CODES[1], REASON_CODES[0]]),
        ),
    ];

    for (label, repeat) in repeats {
        let router = emulator();
        let inquiry_id =
            held_inquiry(&router, &text_body("Residency", "Where is data stored?")).await;

        let original = put_feedback(&router, &first.body(&inquiry_id)).await;
        assert_eq!(
            original.status, 200,
            "case {label:?}: the first submission must be recorded, got {} with {:?}",
            original.status, original.body
        );
        let submitted_at = original.submitted_at().to_string();

        // A span passes between the two calls, so a handler answering `now` rather than
        // the remembered instant would answer a different timestamp.
        std::thread::sleep(Duration::from_millis(20));

        let replayed = put_feedback(&router, &repeat.body(&inquiry_id)).await;
        assert_eq!(
            replayed.status, 200,
            "case {label:?}: a repeat under the same token is not an error, got {} with \
             {:?}",
            replayed.status, replayed.body
        );
        assert_eq!(
            replayed.submitted_at(),
            submitted_at,
            "case {label:?}: a repeat must answer the first submittedAt, got {:?}",
            replayed.body["submittedAt"]
        );

        // And the request was IGNORED rather than recorded a second time.
        let feedback = exported_feedback(&router, &inquiry_id).await;
        assert_eq!(
            feedback,
            vec![first.expected_entry(&submitted_at)],
            "case {label:?}: the repeat must record nothing further, got {feedback:?}"
        );
    }
}

#[tokio::test]
async fn the_same_client_token_on_a_different_submission_is_ignored_without_an_error() {
    const TOKEN: &str = "token-reused";
    let first = Submission::rated(RATINGS[0])
        .on_query(1)
        .because(&[REASON_CODES[1]])
        .commenting("Good answer.")
        .on_revision(1)
        .with_token(TOKEN);
    // One row per member a second request carrying the SAME token can differ in — each the
    // only difference, and the rows span the values those members take: both ratings, one
    // reason code against every one of them, a comment against none, a query identifier
    // against none, and a revision from the first one up to the largest an Integer carries.
    // Every row targets a response version NOTHING has rated yet, so a request of its own
    // would be recorded for it: what is under test is the token sentence alone. (On a
    // version that already carries feedback the version rule refuses the submission first,
    // whatever token it carries, which `one_response_version_accepts_exactly_one_feedback`
    // pins.)
    let repeats: Vec<(&str, Submission)> = vec![
        (
            "another rating, on a version nothing has rated",
            first.clone().rated_as(RATINGS[1]).on_revision(2),
        ),
        (
            "another comment, on a version nothing has rated",
            first.clone().commenting("Bad answer.").on_revision(3),
        ),
        (
            "every reason code, on a version nothing has rated",
            first.clone().because(&REASON_CODES).on_revision(4),
        ),
        ("another revision id alone", first.clone().on_revision(5)),
        ("no query identifier", first.clone().on_no_query()),
        (
            "no comment, on a version nothing has rated",
            first.clone().without_comment().on_revision(6),
        ),
        (
            "the largest revision an Integer carries",
            first.clone().on_revision(i32::MAX),
        ),
    ];

    for (label, second) in repeats {
        let router = emulator();
        let inquiry_id = held_inquiry(&router, &text_body("Encryption", "Encrypted?")).await;

        let original = put_feedback(&router, &first.body(&inquiry_id)).await;
        assert_eq!(
            original.status, 200,
            "case {label:?}: the first submission must be recorded, got {} with {:?}",
            original.status, original.body
        );
        let submitted_at = original.submitted_at().to_string();

        // A span passes between the two calls, so a handler answering `now` rather than the
        // remembered instant would answer a different timestamp.
        std::thread::sleep(Duration::from_millis(20));

        let ignored = put_feedback(&router, &second.body(&inquiry_id)).await;
        assert_eq!(
            ignored.status, 200,
            "case {label:?}: a token this service has already seen is ignored rather than \
             answered with an error, got {} with {:?}",
            ignored.status, ignored.body
        );
        assert_eq!(
            ignored.submitted_at(),
            submitted_at,
            "case {label:?}: an ignored request answers the submittedAt the call that \
             recorded the token answered, got {:?}",
            ignored.body["submittedAt"]
        );

        // And the request really was IGNORED: nothing further was recorded, and the first
        // submission stands unreplaced.
        let feedback = exported_feedback(&router, &inquiry_id).await;
        assert_eq!(
            feedback,
            vec![first.expected_entry(&submitted_at)],
            "case {label:?}: an ignored request must record nothing, got {feedback:?}"
        );
    }

    // The same token on another INQUIRY is ignored too: the token names a request this
    // service has already run, whatever resource the later one addresses.
    let router = emulator();
    let one = held_inquiry(&router, &text_body("One", "First question?")).await;
    let other = held_inquiry(&router, &text_body("Two", "Second question?")).await;
    let recorded = put_feedback(&router, &first.body(&one)).await;
    assert_eq!(
        recorded.status, 200,
        "the first submission must be recorded, got {} with {:?}",
        recorded.status, recorded.body
    );
    let submitted_at = recorded.submitted_at().to_string();
    let crossed = put_feedback(
        &router,
        &first.clone().rated_as(RATINGS[1]).on_revision(9).body(&other),
    )
    .await;
    assert_eq!(
        crossed.status, 200,
        "case \"another inquiry\": the reused token is ignored, got {} with {:?}",
        crossed.status, crossed.body
    );
    assert_eq!(
        crossed.submitted_at(),
        submitted_at,
        "case \"another inquiry\": the ignored request answers the first call's \
         submittedAt, got {:?}",
        crossed.body["submittedAt"]
    );
    let crossed_feedback = exported_feedback(&router, &other).await;
    assert!(
        crossed_feedback.is_empty(),
        "case \"another inquiry\": the ignored request records nothing on the inquiry it \
         addressed, got {crossed_feedback:?}"
    );
    let feedback = exported_feedback(&router, &one).await;
    assert_eq!(
        feedback,
        vec![first.expected_entry(&submitted_at)],
        "case \"another inquiry\": the first inquiry keeps its one record, got {feedback:?}"
    );
}

#[tokio::test]
async fn a_query_identifier_the_inquiry_does_not_own_is_refused_with_a_validation_exception() {
    // The clause the service states: the member that carried the value, and the
    // requirement it failed.
    const CLAUSE: &str = "Value at 'queryIdentifier' failed to satisfy constraint: every query \
                          identifier must name a query of the compliance inquiry";

    // A TEXT inquiry owns exactly query 1; an upload owns one query per unit of work it
    // was submitted in, and an upload carrying no bytes owns none at all. The rows differ
    // along the identifier named and along which of those the inquiry is.
    let cases: Vec<(&str, Value, i32)> = vec![
        (
            "one past the queries a text inquiry owns",
            text_body("Residency", "Where is data stored?"),
            2,
        ),
        (
            "below the first identifier",
            text_body("Residency", "Where is data stored?"),
            0,
        ),
        (
            "a negative identifier",
            text_body("Residency", "Where is data stored?"),
            -1,
        ),
        (
            "the largest Integer",
            text_body("Residency", "Where is data stored?"),
            i32::MAX,
        ),
        (
            "one past the sections an upload was submitted in",
            file_body("Questionnaire", &["Sheet 1", "Sheet 2"]),
            3,
        ),
        (
            "the first identifier on an inquiry that owns no query",
            empty_file_body("Empty questionnaire"),
            1,
        ),
    ];

    for (label, inquiry, identifier) in cases {
        let router = emulator();
        let inquiry_id = held_inquiry(&router, &inquiry).await;

        let refused = put_feedback(
            &router,
            &Submission::rated(RATINGS[0])
                .on_query(identifier)
                .body(&inquiry_id),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {label:?}: an identifier the inquiry does not own is refused, got {} \
             with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {label:?}: expected the modelled ValidationException, got {:?}",
            refused.body
        );
        assert_eq!(
            refused.body["reason"],
            json!("fieldValidationFailed"),
            "case {label:?}: the refusal is a field validation failure, got {:?}",
            refused.body["reason"]
        );
        assert_eq!(
            refused.body["fieldList"],
            json!([{ "name": "queryIdentifier", "message": CLAUSE }]),
            "case {label:?}: the field list names the member and the requirement, got {:?}",
            refused.body["fieldList"]
        );
        assert_eq!(
            refused.body["message"],
            json!(format!("1 validation error detected: {CLAUSE}")),
            "case {label:?}: expected the validation envelope, got {:?}",
            refused.body["message"]
        );

        let feedback = exported_feedback(&router, &inquiry_id).await;
        assert!(
            feedback.is_empty(),
            "case {label:?}: a refused submission records nothing, got {feedback:?}"
        );
    }
}

#[tokio::test]
async fn an_inquiry_the_account_does_not_hold_is_refused_by_the_front_door() {
    let router = emulator();
    // Well-formed `InquiryId`s (`^compliance-inquiry-[a-zA-Z0-9]{16}$`) this account holds
    // no inquiry for: the judgement needs this service's state, so the handler makes it
    // and answers the front door's refusal rather than confirming the absence.
    for (label, id) in [
        ("all zeroes", "compliance-inquiry-0000000000000000"),
        ("mixed case letters", "compliance-inquiry-AbCdEfGhIjKlMnOp"),
        ("the id a later create would mint", "compliance-inquiry-0000000000000009"),
    ] {
        let refused = put_feedback(&router, &Submission::rated(RATINGS[0]).body(id)).await;
        assert_eq!(
            refused.status, 401,
            "case {label:?}: an inquiry the account does not hold is refused, got {} with \
             {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "UnauthorizedException",
            "case {label:?}: expected the front door's refusal, got {:?}",
            refused.body
        );
    }
}

#[tokio::test]
async fn one_response_version_accepts_exactly_one_feedback() {
    // Each row is a pair: a first submission, which must be recorded, and a second one
    // against the SAME response version — the (queryIdentifier, responseRevisionId) pair —
    // which must be refused. The rows differ along the version's form (on a query or on the
    // inquiry's response as a whole, a revision named or not, a bound of Integer) and along
    // what the second request carries: a FRESH clientToken, no token at all, or the first
    // request's token on different content. None of them gets past the refusal, because it
    // is keyed on the version's state rather than on a token — a reused token is ignored
    // only once this rule has had its say, which the idempotency test covers.
    let rows: Vec<(&str, Submission, Submission)> = vec![
        (
            "a fresh token, on the same query and revision",
            Submission::rated(RATINGS[0])
                .on_query(1)
                .on_revision(0)
                .with_token("token-first"),
            Submission::rated(RATINGS[1])
                .on_query(1)
                .on_revision(0)
                .with_token("token-second"),
        ),
        (
            "neither request carrying a token",
            Submission::rated(RATINGS[0]).on_query(1).on_revision(2),
            Submission::rated(RATINGS[1])
                .on_query(1)
                .on_revision(2)
                .because(&[REASON_CODES[0]])
                .commenting("Reconsidered."),
        ),
        (
            "the first request's token on different content, same version",
            Submission::rated(RATINGS[0])
                .on_query(1)
                .on_revision(7)
                .with_token("token-shared"),
            Submission::rated(RATINGS[1])
                .on_query(1)
                .on_revision(7)
                .with_token("token-shared"),
        ),
        (
            "no revision named by either request",
            Submission::rated(RATINGS[1])
                .on_query(1)
                .because(&[REASON_CODES[2]]),
            Submission::rated(RATINGS[0]).on_query(1).with_token("token-no-revision"),
        ),
        (
            "the inquiry's response as a whole, rated twice",
            Submission::rated(RATINGS[0]).with_token("token-whole-first"),
            Submission::rated(RATINGS[1])
                .commenting("Still wrong.")
                .with_token("token-whole-second"),
        ),
        (
            "the largest revision an Integer carries",
            Submission::rated(RATINGS[0])
                .on_query(1)
                .on_revision(i32::MAX)
                .because(&REASON_CODES),
            Submission::rated(RATINGS[1])
                .on_query(1)
                .on_revision(i32::MAX)
                .with_token("token-max-revision"),
        ),
        (
            "the smallest revision an Integer carries",
            Submission::rated(RATINGS[1]).on_query(1).on_revision(i32::MIN),
            Submission::rated(RATINGS[1])
                .on_query(1)
                .on_revision(i32::MIN)
                .with_token("token-min-revision"),
        ),
    ];

    for (label, first, second) in rows {
        let router = emulator();
        let inquiry_id =
            held_inquiry(&router, &text_body("Residency", "Where is data stored?")).await;

        let recorded = put_feedback(&router, &first.body(&inquiry_id)).await;
        assert_eq!(
            recorded.status, 200,
            "case {label:?}: the first submission must be recorded, got {} with {:?}",
            recorded.status, recorded.body
        );
        let submitted_at = recorded.submitted_at().to_string();

        let refused = put_feedback(&router, &second.body(&inquiry_id)).await;
        assert_eq!(
            refused.status, 409,
            "case {label:?}: a second feedback on one response version is refused, got {} \
             with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ConflictException",
            "case {label:?}: expected the modelled ConflictException, got {:?}",
            refused.body
        );
        assert_eq!(
            refused.body["message"],
            json!(ALREADY_SUBMITTED),
            "case {label:?}: the refusal states the response version's state, got {:?}",
            refused.body["message"]
        );
        // The modelled `ConflictException` declares both @required.
        assert_eq!(
            refused.body["resourceId"],
            json!(inquiry_id),
            "case {label:?}: the conflict names the inquiry the record lives on, got {:?}",
            refused.body["resourceId"]
        );
        assert_eq!(
            refused.body["resourceType"],
            json!("ComplianceInquiry"),
            "case {label:?}: the conflict names the resource type, got {:?}",
            refused.body["resourceType"]
        );

        // The refused submission recorded NOTHING: the version still carries the first
        // feedback, unreplaced.
        let feedback = exported_feedback(&router, &inquiry_id).await;
        assert_eq!(
            feedback,
            vec![first.expected_entry(&submitted_at)],
            "case {label:?}: the refused submission must record nothing, got {feedback:?}"
        );
    }
}

#[tokio::test]
async fn feedback_on_another_response_version_is_a_record_of_its_own() {
    // Each row is a pair of submissions on ONE inquiry that target DIFFERENT response
    // versions, so both are recorded. The rows differ along which coordinate of the version
    // the two requests differ in — the query, the revision, or a revision named against
    // none — and along the values and bounds those coordinates take.
    let rows: Vec<(&str, Submission, Submission)> = vec![
        (
            "two revisions of the same query",
            Submission::rated(RATINGS[0]).on_query(1).on_revision(0),
            Submission::rated(RATINGS[1]).on_query(1).on_revision(1),
        ),
        (
            "the same query, with and without a revision",
            Submission::rated(RATINGS[0]).on_query(1),
            Submission::rated(RATINGS[1]).on_query(1).on_revision(0),
        ),
        (
            "the inquiry's response as a whole and one of its queries",
            Submission::rated(RATINGS[0]),
            Submission::rated(RATINGS[0]).on_query(1),
        ),
        (
            "two revisions of the inquiry's response as a whole",
            Submission::rated(RATINGS[1]).on_revision(1),
            Submission::rated(RATINGS[1]).on_revision(2),
        ),
        (
            "the largest revision an Integer carries and the one below it",
            Submission::rated(RATINGS[0])
                .on_query(1)
                .on_revision(i32::MAX - 1),
            Submission::rated(RATINGS[1])
                .on_query(1)
                .on_revision(i32::MAX)
                .because(&[REASON_CODES[1]]),
        ),
        (
            "a negative revision and the next one up — Integer declares no range",
            Submission::rated(RATINGS[1]).on_query(1).on_revision(-1),
            Submission::rated(RATINGS[1])
                .on_query(1)
                .on_revision(0)
                .commenting("Better on the new version."),
        ),
    ];

    for (label, first, second) in rows {
        let router = emulator();
        let inquiry_id =
            held_inquiry(&router, &text_body("Residency", "Where is data stored?")).await;

        let first_answer = put_feedback(&router, &first.body(&inquiry_id)).await;
        let second_answer = put_feedback(&router, &second.body(&inquiry_id)).await;
        for (which, answer) in [("the first", &first_answer), ("the second", &second_answer)] {
            assert_eq!(
                answer.status, 200,
                "case {label:?}: {which} submission targets a version of its own and must \
                 be recorded, got {} with {:?}",
                answer.status, answer.body
            );
        }

        // Both records are held, in the store's key order: the version each was submitted
        // against, inquiry-wide entries first and then by ascending query and revision.
        let mut expected = vec![
            (
                (first.query_identifier, first.response_revision_id),
                first.expected_entry(first_answer.submitted_at()),
            ),
            (
                (second.query_identifier, second.response_revision_id),
                second.expected_entry(second_answer.submitted_at()),
            ),
        ];
        expected.sort_by_key(|(target, _)| *target);
        let expected: Vec<Value> = expected.into_iter().map(|(_, entry)| entry).collect();
        let feedback = exported_feedback(&router, &inquiry_id).await;
        assert_eq!(
            feedback, expected,
            "case {label:?}: two response versions carry two records, got {feedback:?}"
        );
    }
}

#[tokio::test]
async fn another_inquirys_untouched_version_accepts_the_same_feedback() {
    // The rule is scoped to the INQUIRY the version belongs to: rating one inquiry's
    // version does not refuse the same rating on another inquiry's untouched one, and the
    // rated version stays refused.
    let router = emulator();
    let one = held_inquiry(&router, &text_body("One", "First question?")).await;
    let other = held_inquiry(&router, &text_body("Two", "Second question?")).await;
    let submission = Submission::rated(RATINGS[1]).on_query(1).on_revision(0);

    let recorded = put_feedback(
        &router,
        &submission.clone().with_token("token-one").body(&one),
    )
    .await;
    assert_eq!(
        recorded.status, 200,
        "the first inquiry's version must accept the feedback, got {} with {:?}",
        recorded.status, recorded.body
    );

    let crossed = put_feedback(
        &router,
        &submission.clone().with_token("token-two").body(&other),
    )
    .await;
    assert_eq!(
        crossed.status, 200,
        "another inquiry's untouched version must accept the same feedback, got {} with \
         {:?}",
        crossed.status, crossed.body
    );

    let repeated = put_feedback(
        &router,
        &submission.clone().with_token("token-three").body(&one),
    )
    .await;
    assert_eq!(
        repeated.status, 409,
        "the already rated version stays refused, got {} with {:?}",
        repeated.status, repeated.body
    );

    for (label, inquiry_id, submitted_at) in [
        ("the first inquiry", &one, recorded.submitted_at()),
        ("the other inquiry", &other, crossed.submitted_at()),
    ] {
        let feedback = exported_feedback(&router, inquiry_id).await;
        assert_eq!(
            feedback,
            vec![submission.expected_entry(submitted_at)],
            "case {label:?}: each inquiry holds its own one record, got {feedback:?}"
        );
    }
}

#[tokio::test]
async fn recording_feedback_changes_nothing_a_later_reader_of_the_inquiry_sees() {
    let router = emulator();
    let inquiry_id = held_inquiry(&router, &text_body("Residency", "Where is data stored?")).await;
    // Past the processing span and read once, so the transition the clock owed the inquiry
    // is already materialised: what the two reads below can differ in is the feedback
    // alone.
    std::thread::sleep(PAST_THE_PROCESSING_SPAN);
    let before = get_metadata(&router, &inquiry_id).await;
    assert_eq!(
        before.status, 200,
        "the control read must succeed, got {} with {:?}",
        before.status, before.body
    );

    let recorded = put_feedback(
        &router,
        &Submission::rated(RATINGS[1])
            .on_query(1)
            .commenting("Not specific enough.")
            .body(&inquiry_id),
    )
    .await;
    assert_eq!(
        recorded.status, 200,
        "the submission must be recorded, got {} with {:?}",
        recorded.status, recorded.body
    );

    let after = get_metadata(&router, &inquiry_id).await;
    assert_eq!(
        after.body, before.body,
        "recording feedback must not change the inquiry's own record, got {:?} against \
         {:?}",
        after.body, before.body
    );
}
