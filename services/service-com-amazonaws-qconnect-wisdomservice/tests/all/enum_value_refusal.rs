// SPDX-License-Identifier: CC0-1.0

//! How this endpoint refuses a value outside a modelled enumeration's value set, over the
//! wire, for requests signed for the `wisdom` endpoint identity this model serves.
//!
//! The rule under test: a value a modelled enumeration does not declare is refused where
//! the request body is read into the operation's input shape — before any handler runs —
//! with HTTP 400 and the error code `BadRequestException`, for every operation and every
//! enum member, while a constraint the service validates ITSELF (an absent `@required`
//! member, a `@length` or `@pattern` violation) keeps the modelled `ValidationException`.
//! `BadRequestException` is the deserialization boundary's code, not a modelled operation
//! error: no operation in this model declares it
//! (`smithy_query(query="operation", operation="CreateAssistant")` gives
//! `AccessDeniedException`, `ConflictException`, `ServiceQuotaExceededException`,
//! `ValidationException`).
//!
//! The table's rows differ along what the rule generalises over: which enumeration is
//! violated (one-value, multi-value, and one only this model declares), where the member
//! sits (the request's top level or nested inside a structure), whether the operation is
//! implemented or still scaffolded, the FORM of the rejected value (unknown token, a
//! declared value in the wrong case, a value carrying the error envelope's own
//! metacharacters), and — as controls — a declared value and the constraints this service
//! validates itself.
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

/// The error code the deserialization boundary answers an undeclared enumeration value
/// with.
const BAD_REQUEST: &str = "BadRequestException";

/// The error code a constraint this service validates itself answers with — the one its
/// model declares.
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
        // The refused value travels into the constraint message, so a value carrying the
        // error envelope's own metacharacters must still come back as a well-formed JSON
        // body with the code readable — the row fails if the body cannot be parsed at all,
        // because the code is then read as empty.
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
        assert!(
            parsed.get("message").and_then(Value::as_str).is_some(),
            "case {case:?}: the refusal must carry a parsed `message` stating the \
             constraint, got {parsed}"
        );
    }
}

#[tokio::test]
async fn a_constraint_this_service_validates_itself_keeps_its_modelled_error() {
    let router = router();

    // (case, method, uri, body, expected error code) — the controls for the rule above: a
    // declared enum value is not refused at all, and every other refusal class keeps the
    // code this model declares for it.
    let rows: Vec<(&str, &str, String, Value, &str)> = vec![
        (
            "CreateAssistant with the AssistantType value the model declares",
            "POST",
            "/assistants".to_string(),
            json!({"name": "control-row-1", "type": "AGENT"}),
            "",
        ),
        (
            "CreateKnowledgeBase with a KnowledgeBaseType value the model declares",
            "POST",
            "/knowledgeBases".to_string(),
            json!({"name": "control-row-2", "knowledgeBaseType": "CUSTOM"}),
            "",
        ),
        (
            "CreateAssistant with a name outside the modelled Name pattern",
            "POST",
            "/assistants".to_string(),
            // `Name`'s pattern is `^[a-zA-Z0-9\s_.,-]+` (anchored at the start only), so a
            // name is refused on its FIRST character: `!!!` matches nowhere.
            json!({"name": "!!!", "type": "AGENT"}),
            VALIDATION,
        ),
        (
            "CreateAssistant with the required name absent",
            "POST",
            "/assistants".to_string(),
            json!({"type": "AGENT"}),
            VALIDATION,
        ),
        (
            "CreateKnowledgeBase with the required knowledgeBaseType absent",
            "POST",
            "/knowledgeBases".to_string(),
            json!({"name": "control-row-5"}),
            VALIDATION,
        ),
    ];

    for (case, method, uri, body, expected_code) in rows {
        let (status, code, parsed) = call(&router, method, &uri, Some(&body)).await;
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
