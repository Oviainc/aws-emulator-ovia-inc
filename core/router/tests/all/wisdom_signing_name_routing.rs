// SPDX-License-Identifier: CC0-1.0

//! The `wisdom` endpoint with BOTH of its emulated views mounted: how the endpoint words
//! a refusal, whichever view answers the request.
//!
//! Amazon Connect Wisdom and Amazon Q in Connect are two models of ONE AWS endpoint
//! (`smithy_query(query="service_info")` on both: the same service shape `WisdomService`
//! version 2020-10-19, the same `restJson1` protocol, the same sigv4Name/arnNamespace
//! `wisdom`, the same endpoint templates `https://wisdom.{Region}.{dnsSuffix}`, differing
//! only in sdkId `Wisdom` vs `QConnect`), and this repo carries a crate for each. Both
//! resolve to the routing key `wisdom`, so a service claiming on the signing name alone
//! claims the other's traffic too and the router — which refuses to pick — answers every
//! request to the endpoint `500 AmbiguousService`, a code no AWS model declares.
//!
//! WHICH of the two claims a request, route by route, and that a `wisdom`-signed request
//! is dispatched rather than refused, are pinned in
//! `wisdom_qconnect_signing_name_routing.rs`; this suite does not restate them. What it
//! adds is the ONE endpoint-wide property neither that suite nor a service-level suite
//! asserts, because it needs both registrations mounted at once: a value outside a
//! modelled enumeration's value set is refused with HTTP 400 and the error code
//! `BadRequestException` — the deserialization boundary's code — whichever view answers
//! the request, the shared endpoint name's or the legacy model's own registration key's,
//! while a constraint a view validates itself keeps the modelled `ValidationException`.
//!
//! Rows span the keys a request can resolve to, the enumerations the two models declare
//! (one shared by both, one only the superset model has), an implemented and a scaffolded
//! operation, and the two refusal classes.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here is signed for.
const REGION: &str = "us-west-2";

/// The endpoint identity both models publish, which the superset (Q in Connect) view
/// serves.
const SHARED_SIGNING_NAME: &str = "wisdom";

/// The registration key the legacy Wisdom view answers under, so it stays reachable
/// without contending for the shared name (its crate's `OWN_REGISTRATION_KEY`).
const LEGACY_REGISTRATION_KEY: &str = "com-amazonaws-wisdom-wisdomservice";

/// An example `UuidOrArn` path label: the form every id label in both models takes.
const ID: &str = "11111111-1111-1111-1111-111111111111";

type QConnect =
    service_com_amazonaws_qconnect_wisdomservice::EmulatedService<engine::InMemoryEngine>;
type Wisdom = service_com_amazonaws_wisdom_wisdomservice::EmulatedService<engine::InMemoryEngine>;

/// A router with BOTH views mounted over one shared engine, exactly as
/// `core/router/src/lib.rs` mounts them.
fn router_with_both_views() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(QConnect::new(Arc::clone(&engine)));
    router.register_service(Wisdom::new(engine));
    router
}

/// One request, signed so that its credential scope resolves to `route_key`. The
/// signature is never checked here: the router runs with request authentication off.
fn request(
    method: &str,
    uri: &str,
    route_key: &str,
    body: Option<&Value>,
) -> http::Request<Bytes> {
    let builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("wisdom.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{REGION}/{route_key}/aws4_request, \
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
    route_key: &str,
    body: Option<&Value>,
) -> (u16, String, Value) {
    let response = router.dispatch(request(method, uri, route_key, body)).await;
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
async fn an_undeclared_enum_value_is_refused_the_same_way_by_either_view() {
    let router = router_with_both_views();

    // (case, method, uri, route key, body, expected status, expected code)
    //
    // The same class of input under both keys: the enumerations both models declare
    // (`AssistantType` [AGENT], `KnowledgeBaseType` [EXTERNAL, CUSTOM, QUICK_RESPONSES, …])
    // and one only the superset model declares (`AIAgentType`), on an implemented and on a
    // scaffolded operation, against the controls for the refusal class each view validates
    // itself.
    let rows: Vec<(&str, &str, String, &str, Value, u16, &str)> = vec![
        (
            "CreateAssistant with a type outside AssistantType, shared endpoint name",
            "POST",
            "/assistants".to_string(),
            SHARED_SIGNING_NAME,
            json!({"name": "enum-1", "type": "NOT_A_REAL_ASSISTANT_TYPE"}),
            400,
            "BadRequestException",
        ),
        (
            "CreateAssistant with a type outside AssistantType, legacy registration key",
            "POST",
            "/assistants".to_string(),
            LEGACY_REGISTRATION_KEY,
            json!({"name": "enum-2", "type": "NOT_A_REAL_ASSISTANT_TYPE"}),
            400,
            "BadRequestException",
        ),
        (
            "CreateKnowledgeBase with a knowledgeBaseType outside KnowledgeBaseType, \
             shared endpoint name",
            "POST",
            "/knowledgeBases".to_string(),
            SHARED_SIGNING_NAME,
            json!({"name": "enum-3", "knowledgeBaseType": "NOT_A_KNOWLEDGE_BASE_TYPE"}),
            400,
            "BadRequestException",
        ),
        (
            "CreateKnowledgeBase with a knowledgeBaseType outside KnowledgeBaseType, \
             legacy registration key",
            "POST",
            "/knowledgeBases".to_string(),
            LEGACY_REGISTRATION_KEY,
            json!({"name": "enum-4", "knowledgeBaseType": "NOT_A_KNOWLEDGE_BASE_TYPE"}),
            400,
            "BadRequestException",
        ),
        (
            "CreateAIAgent with a type outside AIAgentType — an enum only the superset \
             model declares",
            "POST",
            format!("/assistants/{ID}/aiagents"),
            SHARED_SIGNING_NAME,
            json!({
                "name": "enum-5",
                "type": "NOT_AN_AI_AGENT_TYPE",
                "visibilityStatus": "SAVED",
                "configuration": {"manualSearchAIAgentConfiguration": {}}
            }),
            400,
            "BadRequestException",
        ),
        // Controls: the refusal classes each view validates itself keep the code their
        // models declare.
        (
            "CreateAssistant with a name outside the modelled Name pattern, shared \
             endpoint name",
            "POST",
            "/assistants".to_string(),
            SHARED_SIGNING_NAME,
            json!({"name": "!!!", "type": "AGENT"}),
            400,
            "ValidationException",
        ),
        (
            "CreateAssistant with the required name absent, legacy registration key",
            "POST",
            "/assistants".to_string(),
            LEGACY_REGISTRATION_KEY,
            json!({"type": "AGENT"}),
            400,
            "ValidationException",
        ),
    ];

    for (case, method, uri, route_key, body, expected_status, expected_code) in rows {
        let (status, code, parsed) =
            call(&router, method, &uri, route_key, Some(&body)).await;
        assert_eq!(
            (status, code.as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected HTTP {expected_status} {expected_code:?}, got {status} \
             {code:?} ({parsed})"
        );
    }
}
