// SPDX-License-Identifier: CC0-1.0

//! How this endpoint refuses a request the boundary in front of the service rejects, over
//! the wire, for requests signed for the `wisdom` endpoint identity this model serves.
//!
//! The rules under test:
//!
//! * a value a modelled enumeration does not declare — which the operation's input shape
//!   cannot hold at all — is refused where the request body is read, before any handler
//!   runs, with HTTP 400, the error code `BadRequestException` and the one sentence this
//!   endpoint states for a body it will not read (`Invalid request body`), which carries
//!   nothing from the request, for every operation and every enum member;
//! * a BODY value outside a modelled `@length` bound is refused there too, with the same
//!   code and the same sentence — the recorded wire answer for each of those cases;
//! * every other refusal class keeps the error this model declares, with the
//!   constraint-stating sentence AWS words it with: a value outside a `@pattern`, an absent
//!   `@required` member, a value outside a `@range`, and a `@length` refusal of a value
//!   that travelled in the query string rather than in the body.
//!
//! `BadRequestException` is that boundary's code, not a modelled operation error: no
//! operation in this model declares it
//! (`smithy_query(query="operation", operation="CreateAssistant")` gives
//! `AccessDeniedException`, `ConflictException`, `ServiceQuotaExceededException`,
//! `ValidationException`).
//!
//! The tables' rows differ along what each rule generalises over: which enumeration or
//! bound is violated (one-value, multi-value, and one only this model declares; a maximum
//! and a minimum), where the member sits (the request's top level or nested inside a
//! structure), whether the operation is implemented or still scaffolded, the FORM of the
//! rejected value (unknown token, a declared value in the wrong case, a value carrying the
//! error envelope's own metacharacters), and — as controls — a body the schema admits and
//! every class the service validates itself.
//!
//! Every assertion goes over the wire through the same [`MultiServiceRouter`] the
//! production emulator mounts, signed for `wisdom`: this model is the view of that endpoint
//! the router dispatches its traffic to (see `src/service.rs`'s `claims_endpoint_traffic`),
//! so these are the answers a `wisdom` SDK client gets.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_qconnect_wisdomservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here is signed for.
const REGION: &str = "us-west-2";

/// The endpoint identity both models of this endpoint publish, and which this model
/// serves: `smithy_query(query="service_info")` gives sigv4Name/arnNamespace `wisdom` and
/// the endpoint templates `https://wisdom.{Region}.{dnsSuffix}`.
const SIGNING_NAME: &str = "wisdom";

/// An example `UuidOrArn` path label: the form every id label in this model takes.
const ID: &str = "11111111-1111-1111-1111-111111111111";

/// The error code the boundary in front of the service answers both of its classes with.
const BAD_REQUEST: &str = "BadRequestException";

/// The published bounds this suite's rows sit on: `Name` is `len 1..255`, `maxResults` is
/// `range 1..100` and `ListKnowledgeBases`' `nextToken` is `len 1..4096`
/// (`smithy_query(query="operation",operation="CreateAssistant,ListAssistants,ListKnowledgeBases")`).
const NAME_MAX: usize = 255;
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;
const NEXT_TOKEN_MAX: usize = 4096;

/// The one sentence the boundary in front of the service refuses a body it will not read
/// with, which reports nothing from the request.
const INVALID_REQUEST_BODY: &str = "Invalid request body";

/// The error code every class the service validates itself keeps — the one its model
/// declares.
const VALIDATION: &str = "ValidationException";

/// One expectation row: case, method, URI, JSON body, expected status, expected code.
type RefusalRow = (&'static str, &'static str, String, Value, u16, &'static str);

/// A fresh emulator: one engine, this service mounted, no state.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// One request, signed the way an SDK client of this endpoint signs it. The signature is
/// never checked here: the routers in this suite run with request authentication off.
fn request(method: &str, uri: &str, body: Option<&Value>) -> http::Request<Bytes> {
    let builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("wisdom.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{REGION}/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host, Signature=not-verified-in-this-suite"
            ),
        );
    match body {
        Some(json) => builder
            .header("content-type", "application/json")
            .body(Bytes::from(json.to_string()))
            .expect("build a request with a JSON body"),
        None => builder.body(Bytes::new()).expect("build a bodyless request"),
    }
}

/// The status, the restJson1 `__type` error code (empty on a success) and the parsed body
/// of one call through the router.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, String, Value) {
    let response = router.dispatch(request(method, uri, body)).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    let code = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    (status, code, parsed)
}

#[tokio::test]
async fn a_value_outside_a_modelled_enum_is_refused_as_a_bad_request() {
    let router = router();

    // The value sets are the model's own, from `smithy_query(query="shape", shape=…)`:
    // `AssistantType` [AGENT]; `KnowledgeBaseType` [EXTERNAL, CUSTOM, QUICK_RESPONSES,
    // MESSAGE_TEMPLATES, MANAGED]; `AssociationType` [EXTERNAL_BEDROCK_KNOWLEDGE_BASE,
    // KNOWLEDGE_BASE]; `AIAgentType` [MANUAL_SEARCH, ANSWER_RECOMMENDATION, SELF_SERVICE,
    // EMAIL_RESPONSE, EMAIL_OVERVIEW, EMAIL_GENERATIVE_ANSWER, ORCHESTRATION, NOTE_TAKING,
    // CASE_SUMMARIZATION]; `VisibilityStatus` [SAVED, PUBLISHED]; `FilterField` [NAME];
    // `FilterOperator` [EQUALS, PREFIX].
    let rows: Vec<RefusalRow> = vec![
        (
            "CreateAssistant with a type outside AssistantType (one-value enum)",
            "POST",
            "/assistants".to_string(),
            json!({"name": "enum-row-1", "type": "NOT_A_REAL_ASSISTANT_TYPE"}),
            400,
            BAD_REQUEST,
        ),
        (
            "CreateAssistant with a declared AssistantType value in the wrong case",
            "POST",
            "/assistants".to_string(),
            json!({"name": "enum-row-2", "type": "agent"}),
            400,
            BAD_REQUEST,
        ),
        (
            "CreateKnowledgeBase with a knowledgeBaseType outside KnowledgeBaseType \
             (multi-value enum)",
            "POST",
            "/knowledgeBases".to_string(),
            json!({"name": "enum-row-3", "knowledgeBaseType": "NOT_A_KNOWLEDGE_BASE_TYPE"}),
            400,
            BAD_REQUEST,
        ),
        (
            "CreateAssistantAssociation (scaffolded) with an associationType outside \
             AssociationType",
            "POST",
            format!("/assistants/{ID}/associations"),
            json!({
                "associationType": "NOT_AN_ASSOCIATION_TYPE",
                "association": {"knowledgeBaseId": ID}
            }),
            400,
            BAD_REQUEST,
        ),
        (
            "CreateAIAgent (scaffolded) with a type outside AIAgentType — an enum only \
             this model declares",
            "POST",
            format!("/assistants/{ID}/aiagents"),
            json!({
                "name": "enum-row-5",
                "type": "NOT_AN_AI_AGENT_TYPE",
                "visibilityStatus": "SAVED",
                "configuration": {"manualSearchAIAgentConfiguration": {}}
            }),
            400,
            BAD_REQUEST,
        ),
        (
            "CreateAIAgent (scaffolded) with a visibilityStatus outside VisibilityStatus",
            "POST",
            format!("/assistants/{ID}/aiagents"),
            json!({
                "name": "enum-row-6",
                "type": "MANUAL_SEARCH",
                "visibilityStatus": "NOT_A_VISIBILITY_STATUS",
                "configuration": {"manualSearchAIAgentConfiguration": {}}
            }),
            400,
            BAD_REQUEST,
        ),
        (
            "SearchContent with a nested filter field outside FilterField",
            "POST",
            format!("/knowledgeBases/{ID}/search"),
            json!({
                "searchExpression": {
                    "filters": [
                        {"field": "NOT_A_FILTER_FIELD", "operator": "EQUALS", "value": "x"}
                    ]
                }
            }),
            400,
            BAD_REQUEST,
        ),
        (
            "SearchContent with a nested filter operator outside FilterOperator",
            "POST",
            format!("/knowledgeBases/{ID}/search"),
            json!({
                "searchExpression": {
                    "filters": [
                        {"field": "NAME", "operator": "NOT_AN_OPERATOR", "value": "x"}
                    ]
                }
            }),
            400,
            BAD_REQUEST,
        ),
        // The refused value must NOT travel into the answer: a value carrying the error
        // envelope's own metacharacters comes back as a well-formed JSON body stating the
        // one sentence this class states, with nothing of the request in it.
        (
            "CreateAssistant with a type that carries JSON envelope metacharacters",
            "POST",
            "/assistants".to_string(),
            json!({"name": "enum-row-9", "type": "\"}, \"__type\": \"Spoofed\", \"x\": \""}),
            400,
            BAD_REQUEST,
        ),
        (
            "CreateAssistant breaking BOTH the enum and the Name pattern",
            "POST",
            "/assistants".to_string(),
            json!({"name": "!!!", "type": "NOT_A_REAL_ASSISTANT_TYPE"}),
            400,
            BAD_REQUEST,
        ),
    ];

    for (case, method, uri, body, expected_status, expected_code) in rows {
        let (status, code, parsed) = call(&router, method, &uri, Some(&body)).await;
        assert_eq!(
            (status, code.as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected HTTP {expected_status} {expected_code:?}, got {status} \
             {code:?} ({parsed})"
        );
        assert_eq!(
            parsed.get("message").and_then(Value::as_str),
            Some(INVALID_REQUEST_BODY),
            "case {case:?}: the refusal must carry the one sentence this class states, \
             got {parsed}"
        );
    }
}

/// The other class the boundary in front of the service refuses: a BODY value outside a
/// modelled `@length` bound, answered with the same code and the one sentence this
/// endpoint states for it, which carries nothing from the request.
///
/// The rows differ along what the rule generalises over: which bound is broken (a maximum
/// and a minimum), the member's position (the request's top level and nested inside a
/// structure), the operation (implemented and still scaffolded), and the FORM of the
/// refused value (a long run, and one carrying the error envelope's own metacharacters).
#[tokio::test]
async fn a_body_value_outside_a_modelled_length_bound_is_refused_as_a_bad_request() {
    let router = router();

    // The bounds are the model's own
    // (`smithy_query(query="operation",operation="CreateAssistant,CreateKnowledgeBase,CreateSession")`:
    // `Name` and `Description` are `len 1..255`, `ClientToken` is `len 1..4096`;
    // `smithy_query(query="shape",shape="AppIntegrationsConfiguration")`:
    // `appIntegrationArn` is `len 1..2048`).
    let rows: Vec<RefusalRow> = vec![
        (
            "CreateAssistant with a name one character past the Name maximum",
            "POST",
            "/assistants".to_string(),
            json!({"name": "x".repeat(NAME_MAX + 1), "type": "AGENT"}),
            400,
            BAD_REQUEST,
        ),
        (
            "CreateAssistant with a name under the Name minimum",
            "POST",
            "/assistants".to_string(),
            json!({"name": "", "type": "AGENT"}),
            400,
            BAD_REQUEST,
        ),
        (
            "CreateKnowledgeBase with a description under the Description minimum",
            "POST",
            "/knowledgeBases".to_string(),
            json!({
                "name": "length-row-3",
                "knowledgeBaseType": "CUSTOM",
                "description": ""
            }),
            400,
            BAD_REQUEST,
        ),
        (
            "CreateKnowledgeBase with a nested appIntegrationArn under its minimum",
            "POST",
            "/knowledgeBases".to_string(),
            json!({
                "name": "length-row-4",
                "knowledgeBaseType": "EXTERNAL",
                "sourceConfiguration": {"appIntegrations": {"appIntegrationArn": ""}}
            }),
            400,
            BAD_REQUEST,
        ),
        (
            "CreateSession (implemented) with a clientToken under the ClientToken minimum",
            "POST",
            format!("/assistants/{ID}/sessions"),
            json!({"name": "length-row-5", "clientToken": ""}),
            400,
            BAD_REQUEST,
        ),
        (
            "CreateQuickResponse (scaffolded) with a name past the Name maximum",
            "POST",
            format!("/knowledgeBases/{ID}/quickResponses"),
            json!({
                "name": "x".repeat(NAME_MAX + 1),
                "content": {"content": "hello"}
            }),
            400,
            BAD_REQUEST,
        ),
        (
            "CreateAssistant with an over-long name carrying JSON envelope metacharacters",
            "POST",
            "/assistants".to_string(),
            json!({
                "name": "\"}, \"__type\": \"Spoofed\", \"x\": \"".repeat(NAME_MAX),
                "type": "AGENT"
            }),
            400,
            BAD_REQUEST,
        ),
    ];

    for (case, method, uri, body, expected_status, expected_code) in rows {
        let (status, code, parsed) = call(&router, method, &uri, Some(&body)).await;
        assert_eq!(
            (status, code.as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected HTTP {expected_status} {expected_code:?}, got {status} \
             {code:?} ({parsed})"
        );
        assert_eq!(
            parsed.get("message").and_then(Value::as_str),
            Some(INVALID_REQUEST_BODY),
            "case {case:?}: the refusal must carry the one sentence this class states, \
             got {parsed}"
        );
    }
}

#[tokio::test]
async fn a_constraint_this_service_validates_itself_keeps_its_modelled_error() {
    let router = router();

    // (case, method, uri, body, expected error code) — the controls for the rules above: a
    // body the schema admits is not refused at all, and every other refusal class — a
    // `@pattern`, an absent `@required` member, a `@range`, and a `@length` refusal of a
    // value that travelled in the query string — keeps the code this model declares.
    let rows: Vec<(&str, &str, String, Option<Value>, &str)> = vec![
        (
            "CreateAssistant with the AssistantType value the model declares",
            "POST",
            "/assistants".to_string(),
            Some(json!({"name": "control-row-1", "type": "AGENT"})),
            "",
        ),
        (
            "CreateKnowledgeBase with a KnowledgeBaseType value the model declares",
            "POST",
            "/knowledgeBases".to_string(),
            Some(json!({"name": "control-row-2", "knowledgeBaseType": "CUSTOM"})),
            "",
        ),
        (
            "CreateAssistant with a name outside the modelled Name pattern",
            "POST",
            "/assistants".to_string(),
            // `Name`'s pattern is `^[a-zA-Z0-9\s_.,-]+` (anchored at the start only), so a
            // name is refused on its FIRST character: `!!!` matches nowhere.
            Some(json!({"name": "!!!", "type": "AGENT"})),
            VALIDATION,
        ),
        (
            "CreateAssistant with the required name absent",
            "POST",
            "/assistants".to_string(),
            Some(json!({"type": "AGENT"})),
            VALIDATION,
        ),
        (
            "CreateKnowledgeBase with the required knowledgeBaseType absent",
            "POST",
            "/knowledgeBases".to_string(),
            Some(json!({"name": "control-row-5"})),
            VALIDATION,
        ),
        (
            "GetAssistant with a path label outside the modelled UuidOrArn pattern",
            "GET",
            "/assistants/not-a-uuid-at-all".to_string(),
            None,
            VALIDATION,
        ),
        (
            "GetKnowledgeBase with a path label outside the modelled UuidOrArn pattern",
            "GET",
            "/knowledgeBases/not-a-uuid-at-all".to_string(),
            None,
            VALIDATION,
        ),
        (
            "ListAssistants with a maxResults one past the modelled range",
            "GET",
            format!("/assistants?maxResults={}", MAX_RESULTS_MAX + 1),
            None,
            VALIDATION,
        ),
        (
            "ListKnowledgeBases with a maxResults one below the modelled range",
            "GET",
            format!("/knowledgeBases?maxResults={}", MAX_RESULTS_MIN - 1),
            None,
            VALIDATION,
        ),
        (
            // The one `@length` class that is NOT the boundary's: the value travelled in
            // the query string (`nextToken` is `@httpQuery` on this operation, `len
            // 1..4096`), which the body check never reads.
            "ListKnowledgeBases with a nextToken past its modelled maximum",
            "GET",
            format!("/knowledgeBases?nextToken={}", "t".repeat(NEXT_TOKEN_MAX + 1)),
            None,
            VALIDATION,
        ),
    ];

    for (case, method, uri, body, expected_code) in rows {
        let (status, code, parsed) = call(&router, method, &uri, body.as_ref()).await;
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected the error code {expected_code:?}, got HTTP {status} \
             {code:?} ({parsed})"
        );
    }
}

#[tokio::test]
async fn a_wisdom_signed_request_is_served_by_this_view_and_reads_back() {
    let router = router();

    // The same signing name both models of this endpoint declare reaches an operation here
    // rather than one of the router's own refusals, and what it creates is persisted.
    let (status, code, created) = call(
        &router,
        "POST",
        "/assistants",
        Some(&json!({"name": "round-trip", "type": "AGENT"})),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (200, ""),
        "case \"CreateAssistant signed for the shared endpoint name\": expected HTTP 200, \
         got {status} {code:?} ({created})"
    );

    let assistant_id = created["assistant"]["assistantId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !assistant_id.is_empty(),
        "case \"CreateAssistant signed for the shared endpoint name\": expected a minted \
         assistantId, got {created}"
    );

    let (status, code, fetched) =
        call(&router, "GET", &format!("/assistants/{assistant_id}"), None).await;
    assert_eq!(
        (status, code.as_str()),
        (200, ""),
        "case \"GetAssistant after CreateAssistant\": expected HTTP 200, got {status} \
         {code:?} ({fetched})"
    );
    assert_eq!(
        fetched["assistant"], created["assistant"],
        "case \"GetAssistant after CreateAssistant\": the assistant read back must be the \
         one created, got {} for {}",
        fetched["assistant"], created["assistant"]
    );
}
