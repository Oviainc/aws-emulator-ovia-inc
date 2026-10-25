// SPDX-License-Identifier: CC0-1.0

//! `CreateComplianceInquiry`'s `supportMode` against the FORM of the submitted content,
//! end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test is one rule:
//!
//! * a request member the model documents as supported only for one form of the submitted
//!   content is REFUSED when the content takes another form, with the modelled 400
//!   `ValidationException` whose `reason` is `fieldValidationFailed` and whose `fieldList`
//!   names that member — it is never accepted, stored and reported back. Here the member
//!   is `supportMode`: "Support mode for inquiry processing. Only supported for file
//!   upload mode. Defaults to AI_ONLY if not specified."
//!   (`smithy_query(query="doc",shape="CreateComplianceInquiryRequest",member="supportMode")`),
//!   so `FULL_SUPPORT` — the mode that "includes human expert review"
//!   (`InquiryDetail$supportMode`) — can only be asked for of an uploaded document, while
//!   the mode the same sentence makes every submission's default asks for nothing a
//!   submission cannot be given;
//! * so every accepted inquiry reports a support mode it really is processed under: the
//!   `supportMode` a read reports and the `ReviewType` its queries carry agree —
//!   `FULL_SUPPORT` with `HUMAN`, `AI_ONLY` with `AI` — rather than advertising a human
//!   expert review of queries the service's own model reviewed.
//!
//! The rows differ along what the rule generalises over: the form of the content (a
//! submitted query versus an uploaded document), the value of the member (each of
//! `InquirySupportMode`'s two, and the member left out), the size of the query text (one
//! character, a sentence, and the longest `LongStringAttribute` admits), how many sections
//! an upload names, whether the upload carries any bytes, and whether the request also
//! carries tags and a `clientToken`.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_artifact_artifact::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service, which is what routes the request.
const SIGNED: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/artifact/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// Base64 of an uploaded document's bytes, as restJson1 carries a blob.
const UPLOAD: &str = "cXVlc3Rpb25uYWlyZQ==";

/// `LongStringAttribute`'s published maximum, which `InquiryContent$query` targets
/// (`smithy_query(query="shape",shape="LongStringAttribute")` — `@length(min: 1, max:
/// 2048)`): a row submits a query of exactly this length, so the rule is asserted at the
/// bound as well as below it.
const QUERY_TEXT_CEILING: usize = 2048;

/// `ValidationExceptionReason`'s value for a member the service itself refused
/// (`smithy_query(query="shape",shape="ValidationExceptionReason")`).
const REASON_FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// The member the refusal names.
const MEMBER: &str = "supportMode";

/// `ReviewType`'s value for a response the service's own model reviews, and the one a
/// human expert reviews (`smithy_query(query="shape",shape="ReviewType")`).
const REVIEW_AI: &str = "AI";
/// See [`REVIEW_AI`].
const REVIEW_HUMAN: &str = "HUMAN";

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

    /// The `message` the error body carries, or the empty string when it carries none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }

    /// The `fieldList` entries the refusal names, as (name, message) pairs.
    fn field_list(&self) -> Vec<(String, String)> {
        self.body["fieldList"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| {
                        (
                            entry["name"].as_str().unwrap_or_default().to_string(),
                            entry["message"].as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The id of the inquiry whichever projection this answer carries names.
    fn inquiry_id(&self) -> String {
        for projection in ["complianceInquirySummary", "complianceInquiryDetail"] {
            if let Some(id) = self.body[projection]["id"].as_str() {
                return id.to_string();
            }
        }
        panic!("expected an answer carrying an inquiry id, got {:?}", self.body)
    }

    /// The `queries` a listing reported.
    fn queries(&self) -> &[Value] {
        self.body["queries"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// The `complianceInquiries` a listing reported.
    fn inquiries(&self) -> &[Value] {
        self.body["complianceInquiries"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// The `supportMode` a metadata read reported.
    fn detail_support_mode(&self) -> Option<&str> {
        self.body["complianceInquiryDetail"]["supportMode"].as_str()
    }
}

async fn send(router: &MultiServiceRouter, method: &str, uri: &str, body: Bytes) -> Answer {
    let request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "artifact.eu-west-1.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", SIGNED)
        .body(body)
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

async fn create(router: &MultiServiceRouter, body: &Value) -> Answer {
    send(
        router,
        "POST",
        "/v1/compliance-inquiry/create",
        Bytes::from(body.to_string()),
    )
    .await
}

async fn get_metadata(router: &MultiServiceRouter, inquiry_id: &str) -> Answer {
    let uri = format!("/v1/compliance-inquiry/getMetadata?complianceInquiryId={inquiry_id}");
    send(router, "GET", &uri, Bytes::new()).await
}

async fn list_queries(router: &MultiServiceRouter, inquiry_id: &str) -> Answer {
    let uri = format!("/v1/compliance-inquiry/listQueries?complianceInquiryId={inquiry_id}");
    send(router, "GET", &uri, Bytes::new()).await
}

async fn list_inquiries(router: &MultiServiceRouter) -> Answer {
    send(router, "GET", "/v1/compliance-inquiry/list", Bytes::new()).await
}

/// A `CreateComplianceInquiry` body for a single submitted query.
fn text_body(name: &str, query: &str, support_mode: Option<&str>) -> Value {
    body(name, json!({ "query": query }), support_mode)
}

/// A `CreateComplianceInquiry` body for an uploaded document of `content` bytes.
fn file_body(name: &str, content: &str, sections: &[&str], support_mode: Option<&str>) -> Value {
    let mut file = json!({ "content": content });
    if !sections.is_empty() {
        file["fileSections"] = json!(sections);
    }
    body(name, json!({ "fileContent": file }), support_mode)
}

fn body(name: &str, inquiry_content: Value, support_mode: Option<&str>) -> Value {
    let mut body = json!({ "name": name, "inquiryContent": inquiry_content });
    if let Some(mode) = support_mode {
        body["supportMode"] = json!(mode);
    }
    body
}

/// One submission, and what the service must answer it with.
struct Case {
    label: &'static str,
    body: Value,
    /// `Some(mode)` when the submission is accepted and every read of the inquiry must
    /// report that `supportMode`; `None` when the request must be refused.
    reported_mode: Option<&'static str>,
}

/// The submissions this suite covers.
fn cases() -> Vec<Case> {
    let longest_query = "a".repeat(QUERY_TEXT_CEILING);
    let mut tagged = text_body(
        "Subprocessors",
        "Which subprocessors handle customer content?",
        Some("FULL_SUPPORT"),
    );
    tagged["tags"] = json!({ "team": "legal", "env": "prod" });
    tagged["clientToken"] = json!("token-tagged-full-support");

    vec![
        // A submission that is not a file upload cannot be made under the mode the model
        // restricts to one, whatever the query text is and whatever else the request
        // carries.
        Case {
            label: "a submitted query under FULL_SUPPORT",
            body: text_body(
                "Data residency",
                "Where is customer data stored?",
                Some("FULL_SUPPORT"),
            ),
            reported_mode: None,
        },
        Case {
            label: "a one-character query under FULL_SUPPORT",
            body: text_body("Shortest", "?", Some("FULL_SUPPORT")),
            reported_mode: None,
        },
        Case {
            label: "the longest query the shape admits, under FULL_SUPPORT",
            body: text_body("Longest", &longest_query, Some("FULL_SUPPORT")),
            reported_mode: None,
        },
        Case {
            label: "a query under FULL_SUPPORT with tags and a clientToken",
            body: tagged,
            reported_mode: None,
        },
        // The same content under the mode every submission defaults to is accepted, named
        // or left out.
        Case {
            label: "a submitted query under AI_ONLY",
            body: text_body(
                "Encryption at rest",
                "Is customer data encrypted at rest?",
                Some("AI_ONLY"),
            ),
            reported_mode: Some("AI_ONLY"),
        },
        Case {
            label: "a submitted query naming no supportMode",
            body: text_body("Retention", "How long are audit logs retained?", None),
            reported_mode: Some("AI_ONLY"),
        },
        Case {
            label: "the longest query the shape admits, naming no supportMode",
            body: text_body("Longest", &longest_query, None),
            reported_mode: Some("AI_ONLY"),
        },
        // File upload mode is the form the member is supported for, so both values are
        // accepted there — however many sections the upload names, and even when it
        // carries no bytes.
        Case {
            label: "an upload naming no section, under FULL_SUPPORT",
            body: file_body("Vendor questionnaire", UPLOAD, &[], Some("FULL_SUPPORT")),
            reported_mode: Some("FULL_SUPPORT"),
        },
        Case {
            label: "an upload naming two sections, under FULL_SUPPORT",
            body: file_body(
                "Annual audit workbook",
                UPLOAD,
                &["Section A", "Section B"],
                Some("FULL_SUPPORT"),
            ),
            reported_mode: Some("FULL_SUPPORT"),
        },
        Case {
            label: "an upload carrying no bytes, under FULL_SUPPORT",
            body: file_body("Empty workbook", "", &["Section A"], Some("FULL_SUPPORT")),
            reported_mode: Some("FULL_SUPPORT"),
        },
        Case {
            label: "an upload naming one section, under AI_ONLY",
            body: file_body("Customer DDQ", UPLOAD, &["Sheet 1"], Some("AI_ONLY")),
            reported_mode: Some("AI_ONLY"),
        },
        Case {
            label: "an upload naming no supportMode",
            body: file_body("Security addendum", UPLOAD, &[], None),
            reported_mode: Some("AI_ONLY"),
        },
    ]
}

#[tokio::test]
async fn a_support_mode_the_submitted_content_is_not_processed_under_is_refused() {
    for case in cases() {
        let label = case.label;
        // One emulator per case, so what one case created cannot decide what another
        // observes.
        let router = emulator();
        let created = create(&router, &case.body).await;

        let Some(expected_mode) = case.reported_mode else {
            assert_eq!(
                created.status, 400,
                "case {label:?}: expected the modelled ValidationException's HTTP 400, \
                 got {} with {:?}",
                created.status, created.body
            );
            assert_eq!(
                created.error_code(),
                "ValidationException",
                "case {label:?}: expected the modelled ValidationException, got {:?}",
                created.body
            );
            assert_eq!(
                created.body["reason"].as_str(),
                Some(REASON_FIELD_VALIDATION_FAILED),
                "case {label:?}: expected reason {REASON_FIELD_VALIDATION_FAILED:?}, got {:?}",
                created.body["reason"]
            );
            let fields = created.field_list();
            assert_eq!(
                fields.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(),
                vec![MEMBER],
                "case {label:?}: the fieldList must name exactly the refused member, got {:?}",
                created.body["fieldList"]
            );
            let (_, clause) = &fields[0];
            assert!(
                clause.contains(MEMBER) && clause.contains("file upload mode"),
                "case {label:?}: the clause must name {MEMBER} and the requirement it \
                 failed, got {clause:?}"
            );
            assert!(
                created.message().starts_with("1 validation error detected:")
                    && created.message().contains(clause),
                "case {label:?}: the message must state the one clause the fieldList \
                 carries, got {:?}",
                created.message()
            );
            continue;
        };

        assert_eq!(
            created.status, 202,
            "case {label:?}: a submission the member is supported for must be accepted \
             with HTTP 202, got {} with {:?}",
            created.status, created.body
        );
        let id = created.inquiry_id();

        let read = get_metadata(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: the created inquiry must read back, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.detail_support_mode(),
            Some(expected_mode),
            "case {label:?}: expected the detail to report supportMode \
             {expected_mode:?}, got {:?}",
            read.body["complianceInquiryDetail"]["supportMode"]
        );

        // The reported mode and the review its queries carry are one decision: a reader
        // never sees a human expert support mode on queries the model reviewed.
        let expected_review = if expected_mode == "FULL_SUPPORT" {
            REVIEW_HUMAN
        } else {
            REVIEW_AI
        };
        let listed = list_queries(&router, &id).await;
        assert_eq!(
            listed.status, 200,
            "case {label:?}: the inquiry's queries must list, got {} with {:?}",
            listed.status, listed.body
        );
        for query in listed.queries() {
            assert_eq!(
                query["reviewType"].as_str(),
                Some(expected_review),
                "case {label:?}: a query of an inquiry reported under {expected_mode:?} \
                 must carry reviewType {expected_review:?}, got {:?}",
                query["reviewType"]
            );
        }
    }
}

#[tokio::test]
async fn a_refused_support_mode_creates_nothing_and_spends_no_client_token() {
    let router = emulator();
    let token = "token-refused-support-mode";

    let mut refused_body = text_body(
        "Subprocessors",
        "Which subprocessors handle customer content?",
        Some("FULL_SUPPORT"),
    );
    refused_body["clientToken"] = json!(token);

    let refused = create(&router, &refused_body).await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (400, "ValidationException"),
        "the refused submission must answer the modelled ValidationException, got {:?}",
        refused.body
    );

    let listed = list_inquiries(&router).await;
    assert_eq!(
        listed.status, 200,
        "ListComplianceInquiries must answer, got {} with {:?}",
        listed.status, listed.body
    );
    assert!(
        listed.inquiries().is_empty(),
        "a refused create must persist no inquiry, got {:?}",
        listed.body
    );

    // The refused call spent nothing: the same token on a submission the member IS
    // supported for creates the inquiry, and repeating it replays that one.
    let mut accepted_body = text_body(
        "Subprocessors",
        "Which subprocessors handle customer content?",
        Some("AI_ONLY"),
    );
    accepted_body["clientToken"] = json!(token);
    let accepted = create(&router, &accepted_body).await;
    assert_eq!(
        accepted.status, 202,
        "the token must still be available to a submission the member is supported for, \
         got {} with {:?}",
        accepted.status, accepted.body
    );
    let id = accepted.inquiry_id();
    let replay = create(&router, &accepted_body).await;
    assert_eq!(
        (replay.status, replay.inquiry_id()),
        (202, id.clone()),
        "the token must replay the inquiry it created, got {:?}",
        replay.body
    );

    let read = get_metadata(&router, &id).await;
    assert_eq!(
        read.detail_support_mode(),
        Some("AI_ONLY"),
        "the created inquiry must report the mode it was created under, got {:?}",
        read.body["complianceInquiryDetail"]["supportMode"]
    );
}
