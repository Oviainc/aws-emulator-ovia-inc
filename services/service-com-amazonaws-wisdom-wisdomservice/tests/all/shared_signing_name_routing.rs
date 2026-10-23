// SPDX-License-Identifier: CC0-1.0

//! Which requests this view of the shared `wisdom` endpoint claims, and how the endpoint
//! refuses a value outside a modelled enumeration.
//!
//! Amazon Connect Wisdom and Amazon Q in Connect are two models of ONE AWS endpoint, and
//! this repo carries a crate for each: the same service shape `WisdomService`, the same
//! version `2020-10-19`, the same `restJson1` protocol, the same sigv4Name/arnNamespace
//! `wisdom` and the same endpoint templates `https://wisdom.{Region}.{dnsSuffix}`
//! (`smithy_query(query="service_info")` on both models differs only in sdkId). restJson1
//! carries no `X-Amz-Target`, so a request is dispatched by method + URI, and this model's
//! 41 operations are bound at the SAME routes as their counterparts among the Q in Connect
//! model's 94: nothing in a request tells the two apart. A service that claims on the
//! shared signing name alone therefore claims the other's traffic too, the router finds two
//! claimants and refuses every request to the endpoint as `500 AmbiguousService` — a code
//! no AWS model declares.
//!
//! The rules under test from THIS view's side:
//!
//! * the shared `wisdom` key is served by the model that subsumes this one, so this view
//!   claims a request that resolves to its OWN registration key and claims nothing signed
//!   for the shared name — exactly one service claims every request, and no route of this
//!   model is answered by a routing failure (`500 AmbiguousService`, `404 UnknownService`);
//! * a value outside a modelled enumeration's value set is refused with HTTP 400 and the
//!   error code `BadRequestException`, for every operation and every enum member, including
//!   an operation that is still scaffolded — the refusal happens where the body is read,
//!   before the handler — while a constraint this service validates itself keeps the
//!   modelled `ValidationException`.
//!
//! The cross-service halves (which of the two mounted views claims a request, and that both
//! word the enum refusal the same way) are pinned in `core/router/tests/all/`, the only
//! place both can be mounted together.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_wisdom_wisdomservice::EmulatedService;
use smithy_server_runtime::{MultiServiceRouter, Service};

/// The region every request here is signed for.
const REGION: &str = "us-west-2";

/// The routing key this service answers under (`src/service.rs`'s `OWN_REGISTRATION_KEY`).
const ROUTE_KEY: &str = "com-amazonaws-wisdom-wisdomservice";

/// The signing name BOTH models of this endpoint declare, which the subsuming Q in Connect
/// view serves: nothing signed for it may be claimed here.
const SHARED_SIGNING_NAME: &str = "wisdom";

/// An example `UuidOrArn` path label: the form every id label in this model takes.
const ID: &str = "11111111-1111-1111-1111-111111111111";

/// The two routing failures the router answers with, neither of which may reach a caller
/// for a route this model binds.
const ROUTING_FAILURES: [&str; 2] = ["AmbiguousService", "UnknownService"];

/// Every route THIS model binds: its 41 operations' `smithy.api#http` method + URI, with
/// each `@httpLabel` filled by an example value.
///
/// Source: `smithy_query(query="digest",
/// model_path="services/service-com-amazonaws-wisdom-wisdomservice/smithy-model/\
/// com-amazonaws-wisdom-wisdomservice.json")` — all 41 bindings, nothing elided. The
/// `/tags/{resourceArn}` label is percent-encoded the way an SDK encodes a non-greedy
/// `@httpLabel`, so the ARN's `:` and `/` are not read as path structure.
fn modelled_routes() -> Vec<(&'static str, &'static str, String)> {
    let arn = format!("arn:aws:wisdom:{REGION}:000000000000:assistant/{ID}")
        .replace(':', "%3A")
        .replace('/', "%2F");
    vec![
        ("CreateAssistant", "POST", "/assistants".to_string()),
        (
            "CreateAssistantAssociation",
            "POST",
            format!("/assistants/{ID}/associations"),
        ),
        (
            "CreateContent",
            "POST",
            format!("/knowledgeBases/{ID}/contents"),
        ),
        ("CreateKnowledgeBase", "POST", "/knowledgeBases".to_string()),
        (
            "CreateQuickResponse",
            "POST",
            format!("/knowledgeBases/{ID}/quickResponses"),
        ),
        ("CreateSession", "POST", format!("/assistants/{ID}/sessions")),
        ("DeleteAssistant", "DELETE", format!("/assistants/{ID}")),
        (
            "DeleteAssistantAssociation",
            "DELETE",
            format!("/assistants/{ID}/associations/{ID}"),
        ),
        (
            "DeleteContent",
            "DELETE",
            format!("/knowledgeBases/{ID}/contents/{ID}"),
        ),
        (
            "DeleteImportJob",
            "DELETE",
            format!("/knowledgeBases/{ID}/importJobs/{ID}"),
        ),
        (
            "DeleteKnowledgeBase",
            "DELETE",
            format!("/knowledgeBases/{ID}"),
        ),
        (
            "DeleteQuickResponse",
            "DELETE",
            format!("/knowledgeBases/{ID}/quickResponses/{ID}"),
        ),
        ("GetAssistant", "GET", format!("/assistants/{ID}")),
        (
            "GetAssistantAssociation",
            "GET",
            format!("/assistants/{ID}/associations/{ID}"),
        ),
        (
            "GetContent",
            "GET",
            format!("/knowledgeBases/{ID}/contents/{ID}"),
        ),
        (
            "GetContentSummary",
            "GET",
            format!("/knowledgeBases/{ID}/contents/{ID}/summary"),
        ),
        (
            "GetImportJob",
            "GET",
            format!("/knowledgeBases/{ID}/importJobs/{ID}"),
        ),
        ("GetKnowledgeBase", "GET", format!("/knowledgeBases/{ID}")),
        (
            "GetQuickResponse",
            "GET",
            format!("/knowledgeBases/{ID}/quickResponses/{ID}"),
        ),
        (
            "GetRecommendations",
            "GET",
            format!("/assistants/{ID}/sessions/{ID}/recommendations"),
        ),
        ("GetSession", "GET", format!("/assistants/{ID}/sessions/{ID}")),
        (
            "ListAssistantAssociations",
            "GET",
            format!("/assistants/{ID}/associations"),
        ),
        ("ListAssistants", "GET", "/assistants".to_string()),
        (
            "ListContents",
            "GET",
            format!("/knowledgeBases/{ID}/contents"),
        ),
        (
            "ListImportJobs",
            "GET",
            format!("/knowledgeBases/{ID}/importJobs"),
        ),
        ("ListKnowledgeBases", "GET", "/knowledgeBases".to_string()),
        (
            "ListQuickResponses",
            "GET",
            format!("/knowledgeBases/{ID}/quickResponses"),
        ),
        ("ListTagsForResource", "GET", format!("/tags/{arn}")),
        (
            "NotifyRecommendationsReceived",
            "POST",
            format!("/assistants/{ID}/sessions/{ID}/recommendations/notify"),
        ),
        ("QueryAssistant", "POST", format!("/assistants/{ID}/query")),
        (
            "RemoveKnowledgeBaseTemplateUri",
            "DELETE",
            format!("/knowledgeBases/{ID}/templateUri"),
        ),
        (
            "SearchContent",
            "POST",
            format!("/knowledgeBases/{ID}/search"),
        ),
        (
            "SearchQuickResponses",
            "POST",
            format!("/knowledgeBases/{ID}/search/quickResponses"),
        ),
        (
            "SearchSessions",
            "POST",
            format!("/assistants/{ID}/searchSessions"),
        ),
        (
            "StartContentUpload",
            "POST",
            format!("/knowledgeBases/{ID}/upload"),
        ),
        (
            "StartImportJob",
            "POST",
            format!("/knowledgeBases/{ID}/importJobs"),
        ),
        ("TagResource", "POST", format!("/tags/{arn}")),
        ("UntagResource", "DELETE", format!("/tags/{arn}?tagKeys=owner")),
        (
            "UpdateContent",
            "POST",
            format!("/knowledgeBases/{ID}/contents/{ID}"),
        ),
        (
            "UpdateKnowledgeBaseTemplateUri",
            "POST",
            format!("/knowledgeBases/{ID}/templateUri"),
        ),
        (
            "UpdateQuickResponse",
            "POST",
            format!("/knowledgeBases/{ID}/quickResponses/{ID}"),
        ),
    ]
}

/// One expectation row: case, method, URI, JSON body, expected status, expected error
/// code.
type RefusalRow = (&'static str, &'static str, String, Value, u16, &'static str);

/// This service, over a fresh engine.
fn service() -> EmulatedService<engine::InMemoryEngine> {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    EmulatedService::new(engine)
}

/// A router with only this service mounted, as the emulator mounts it.
fn router() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service());
    router
}

/// A SigV4 `Authorization` header whose credential scope names `route_key`. The signature
/// is never checked here: the claim reads the scope, not the signature.
fn authorization(route_key: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{route_key}/aws4_request, \
         SignedHeaders=host, Signature=not-verified-in-this-suite"
    )
}

/// One request, signed the way a client addressed at `route_key` signs it.
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
        .header("authorization", authorization(route_key));
    match body {
        Some(json) => builder
            .header("content-type", "application/json")
            .body(Bytes::from(json.to_string()))
            .expect("build a request with a JSON body"),
        None => builder.body(Bytes::new()).expect("build a bodyless request"),
    }
}

/// The request parts the router hands to [`Service::can_handle`].
fn parts(method: &str, uri: &str, route_key: &str) -> http::request::Parts {
    request(method, uri, route_key, None).into_parts().0
}

/// The status, the restJson1 `__type` error code (empty on a success) and the parsed body
/// of one call through the router.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, String, Value) {
    let response = router
        .dispatch(request(method, uri, ROUTE_KEY, body))
        .await;
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

#[test]
fn every_route_this_model_binds_is_claimed_under_its_own_registration_key() {
    let service = service();

    for (operation, method, uri) in modelled_routes() {
        let claimed = service.can_handle(&parts(method, &uri, ROUTE_KEY));
        assert!(
            claimed,
            "case {operation:?} ({method} {uri}): this service must claim a route its own \
             model binds when the request resolves to its registration key, got \
             claimed={claimed}"
        );
    }

    // The same claim through the forms a request can also arrive in: a query string on a
    // bound route, and a path label that is an ARN rather than a uuid.
    let variants: Vec<(&str, &str, String)> = vec![
        (
            "ListAssistants with a query string",
            "GET",
            "/assistants?maxResults=2&nextToken=abc".to_string(),
        ),
        (
            "GetAssistant with an ARN label",
            "GET",
            format!(
                "/assistants/{}",
                format!("arn:aws:wisdom:{REGION}:000000000000:assistant/{ID}")
                    .replace(':', "%3A")
                    .replace('/', "%2F")
            ),
        ),
    ];
    for (case, method, uri) in variants {
        let claimed = service.can_handle(&parts(method, &uri, ROUTE_KEY));
        assert!(
            claimed,
            "case {case:?} ({method} {uri}): this service must claim it, got claimed={claimed}"
        );
    }
}

#[test]
fn nothing_signed_for_the_shared_endpoint_name_or_outside_this_path_space_is_claimed() {
    let service = service();

    // Every route this model binds, signed for the SHARED signing name both models of this
    // endpoint declare: that key is served by the model that subsumes this one, so claiming
    // any of them here would put two claimants back on the route and the router would
    // refuse it as `500 AmbiguousService`.
    for (operation, method, uri) in modelled_routes() {
        let claimed = service.can_handle(&parts(method, &uri, SHARED_SIGNING_NAME));
        assert!(
            !claimed,
            "case {operation:?} ({method} {uri}): a request signed for the shared endpoint \
             name {SHARED_SIGNING_NAME:?} must be left to the model that serves it, got \
             claimed={claimed}"
        );
    }

    // (case, uri) — paths outside the space this model publishes, under its own key.
    let outside: Vec<(&str, String)> = vec![
        ("a path no model under this endpoint binds", "/NoSuchOperation".to_string()),
        (
            "a first segment only the subsuming model publishes",
            format!("/assistants-extra/{ID}"),
        ),
    ];
    for (case, uri) in outside {
        let claimed = service.can_handle(&parts("POST", &uri, ROUTE_KEY));
        assert!(
            !claimed,
            "case {case:?} (POST {uri}): this service must not claim a path outside the \
             space its own model publishes, got claimed={claimed}"
        );
    }

    // A route this model DOES bind, resolved to some other service's key.
    let not_ours: [(&str, &str); 3] = [
        ("another service's signing name", "connect"),
        ("the subsuming model's sdkId-derived key", "qconnect"),
        ("a key that is a prefix of this one", "com-amazonaws-wisdom"),
    ];
    for (case, route_key) in not_ours {
        let claimed = service.can_handle(&parts("POST", "/assistants", route_key));
        assert!(
            !claimed,
            "case {case:?} (POST /assistants resolved to {route_key:?}): this service must \
             claim only its own key, got claimed={claimed}"
        );
    }
}

#[tokio::test]
async fn every_modelled_route_is_answered_by_the_service_and_never_by_a_routing_failure() {
    let router = router();

    // Each route is answered by the service that binds it: an implemented operation
    // answers its own success or modelled error, a still-scaffolded one answers the
    // scaffold's `501 NotImplemented`. Both are answers FROM the service — what no route
    // may get is one of the router's own refusals.
    for (operation, method, uri) in modelled_routes() {
        let body = json!({});
        let (status, code, parsed) = call(&router, method, &uri, Some(&body)).await;
        assert!(
            !ROUTING_FAILURES.contains(&code.as_str()),
            "case {operation:?} ({method} {uri}): a route this model binds must be answered \
             by the service, got HTTP {status} {code:?} ({parsed})"
        );
    }
}

#[tokio::test]
async fn a_value_outside_a_modelled_enum_is_refused_as_a_bad_request() {
    let router = router();

    // (case, method, uri, body, expected status, expected error code)
    //
    // One row per enumeration the model declares on an INPUT member, spanning both an
    // implemented operation and a still-scaffolded one (the refusal happens where the body
    // is read, so the scaffold's 501 never gets the chance to answer), a member on the
    // request's top level and one nested inside a structure, and — as controls — the same
    // operations with a value the enum does declare, plus a violation of a constraint the
    // service validates itself, which keeps the modelled `ValidationException`.
    //
    // The value sets are the model's own: `AssistantType` [AGENT], `AssociationType`
    // [KNOWLEDGE_BASE], `KnowledgeBaseType` [CUSTOM, EXTERNAL, QUICK_RESPONSES],
    // `ImportJobType` [QUICK_RESPONSES], `FilterField` [NAME], `FilterOperator` [EQUALS]
    // (`services/service-com-amazonaws-wisdom-wisdomservice/src/ops/types/shared_types.rs`,
    // generated from the model).
    let rows: Vec<RefusalRow> = vec![
        (
            "CreateAssistant with a type outside AssistantType",
            "POST",
            "/assistants".to_string(),
            json!({"name": "enum-row-1", "type": "NOT_A_REAL_ASSISTANT_TYPE"}),
            400,
            "BadRequestException",
        ),
        (
            "CreateAssistant with a lowercased AssistantType value",
            "POST",
            "/assistants".to_string(),
            json!({"name": "enum-row-2", "type": "agent"}),
            400,
            "BadRequestException",
        ),
        (
            "CreateKnowledgeBase with a knowledgeBaseType outside KnowledgeBaseType",
            "POST",
            "/knowledgeBases".to_string(),
            json!({"name": "enum-row-3", "knowledgeBaseType": "NOT_A_KNOWLEDGE_BASE_TYPE"}),
            400,
            "BadRequestException",
        ),
        (
            "CreateAssistantAssociation with an associationType outside AssociationType",
            "POST",
            format!("/assistants/{ID}/associations"),
            json!({
                "associationType": "NOT_AN_ASSOCIATION_TYPE",
                "association": {"knowledgeBaseId": ID}
            }),
            400,
            "BadRequestException",
        ),
        (
            "StartImportJob with an importJobType outside ImportJobType",
            "POST",
            format!("/knowledgeBases/{ID}/importJobs"),
            json!({"importJobType": "NOT_AN_IMPORT_JOB_TYPE", "uploadId": "upload-1"}),
            400,
            "BadRequestException",
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
            "BadRequestException",
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
            "BadRequestException",
        ),
        // The refused value is echoed into the message, so a value carrying the error
        // envelope's own metacharacters must come back as a well-formed JSON body with the
        // code still readable — the row fails if the body cannot be parsed at all, because
        // the code is then read as empty.
        (
            "CreateAssistant with a type that carries JSON envelope metacharacters",
            "POST",
            "/assistants".to_string(),
            json!({"name": "enum-row-8", "type": "\"}, \"__type\": \"Spoofed\", \"x\": \""}),
            400,
            "BadRequestException",
        ),
        (
            "CreateAssistant with a type that carries a backslash, a newline and markup",
            "POST",
            "/assistants".to_string(),
            json!({"name": "enum-row-9", "type": "A\\B\nC</script>"}),
            400,
            "BadRequestException",
        ),
        (
            "CreateAssistant breaking BOTH the enum and the Name pattern",
            "POST",
            "/assistants".to_string(),
            json!({"name": "!!!", "type": "NOT_A_REAL_ASSISTANT_TYPE"}),
            400,
            "BadRequestException",
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

    // The controls: a declared enum value is NOT refused this way, and a constraint this
    // service validates itself answers the error its model declares for it.
    let controls: Vec<(&str, &str, String, Value, &str)> = vec![
        (
            "CreateAssistant with the AssistantType value the model declares",
            "POST",
            "/assistants".to_string(),
            json!({"name": "control-row-1", "type": "AGENT"}),
            "",
        ),
        (
            "CreateAssistant with a name outside the modelled Name pattern",
            "POST",
            "/assistants".to_string(),
            // `Name`'s pattern is `^[a-zA-Z0-9\s_.,-]+` (anchored at the start only), so a
            // name must be refused on its FIRST character: `!!!` matches nowhere.
            json!({"name": "!!!", "type": "AGENT"}),
            "ValidationException",
        ),
        (
            "CreateAssistant with the required name absent",
            "POST",
            "/assistants".to_string(),
            json!({"type": "AGENT"}),
            "ValidationException",
        ),
    ];

    for (case, method, uri, body, expected_code) in controls {
        let (status, code, parsed) = call(&router, method, &uri, Some(&body)).await;
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected the error code {expected_code:?}, got HTTP {status} \
             {code:?} ({parsed})"
        );
    }
}

#[tokio::test]
async fn an_assistant_created_through_this_view_reads_back() {
    let router = router();

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
        "case \"CreateAssistant\": expected HTTP 200, got {status} {code:?} ({created})"
    );

    let assistant_id = created["assistant"]["assistantId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !assistant_id.is_empty(),
        "case \"CreateAssistant\": expected a minted assistantId, got {created}"
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
