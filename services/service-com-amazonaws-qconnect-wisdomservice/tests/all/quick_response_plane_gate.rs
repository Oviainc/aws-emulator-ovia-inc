// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the knowledge-base-type gate on the quick-response plane, driven
//! over HTTP through the same router the production emulator mounts.
//!
//! The rule under test, asserted over a TABLE of inputs rather than the one request a
//! recording carried: a quick response lives only in a knowledge base whose
//! `knowledgeBaseType` holds quick responses, so EVERY operation of that plane refuses a
//! knowledge base of any other modelled type with `ValidationException` (HTTP 400) naming
//! that stored type, and serves the same calls against a `QUICK_RESPONSES` one.
//!
//! The rows differ along what the rule generalises over: the knowledge-base type — every
//! modelled value, including the two whose content comes from a source outside the service
//! — and the operation of the plane, all six of them.
//!
//! Everything goes over the wire, so a refusal that is not computed from the stored
//! knowledge base cannot pass.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_qconnect_wisdomservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite is signed for.
const REGION: &str = "eu-west-3";

/// A UUID no create ever mints, for the label-addressed rows.
const ABSENT_UUID: &str = "00000000-0000-0000-0000-000000000000";

/// Every value of the modelled `KnowledgeBaseType` enum
/// (`smithy_query(query="shape",shape="KnowledgeBaseType")`): the one whose knowledge bases
/// hold quick responses, and the four that hold none.
const TYPE_QUICK_RESPONSES: &str = "QUICK_RESPONSES";
const TYPES_WITHOUT_QUICK_RESPONSES: [&str; 4] =
    ["CUSTOM", "EXTERNAL", "MESSAGE_TEMPLATES", "MANAGED"];

/// The sentence the quick-response plane refuses a knowledge base holding no quick
/// responses with: fixed text whose one variable part is the type of the knowledge base the
/// request named, written out here rather than imported from the implementation so the
/// assertion pins the wire text.
fn message_plane_unsupported(knowledge_base_type: &str) -> String {
    format!("QuickResponse APIs are not supported for {knowledge_base_type} knowledge bases")
}

/// One wire answer, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_type: String,
    body: Value,
}

impl Reply {
    fn string(&self, pointer: &str) -> String {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The error code a client branches on: the `x-amzn-errortype` header, falling back to
    /// the body's `__type` for a protocol that carries it only there.
    fn code(&self) -> String {
        if self.error_type.is_empty() {
            self.string("/__type")
        } else {
            self.error_type.clone()
        }
    }

    fn message(&self) -> String {
        self.string("/message")
    }
}

/// An emulator with just this service mounted on a fresh engine, driven in-process
/// through the production router.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not what these tests are about; the router still routes by the
        // SigV4 credential scope below, which is also where an ARN's region comes from.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn send(&self, method: &str, uri: &str, body: Option<Value>) -> Reply {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/wisdom/aws4_request, \
             SignedHeaders=host, Signature=not-verified-in-this-suite"
        );
        let builder = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("wisdom.{REGION}.amazonaws.com"))
            .header("authorization", authorization);
        let request = match &body {
            Some(value) => builder
                .header("content-type", "application/json")
                .body(Bytes::from(
                    serde_json::to_vec(value).expect("serialize body"),
                ))
                .expect("build request"),
            None => builder.body(Bytes::new()).expect("build request"),
        };
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    /// A knowledge base of type `kind`, answering with the id its create persisted.
    async fn knowledge_base(&self, name: &str, kind: &str) -> String {
        let mut body = json!({ "name": name, "knowledgeBaseType": kind });
        if let Some(source) = source_configuration(kind) {
            body["sourceConfiguration"] = source;
        }
        let reply = self.send("POST", "/knowledgeBases", Some(body)).await;
        assert_eq!(
            reply.status, 200,
            "case \"knowledge base {name} of type {kind}\": expected HTTP 200, got {} with {}",
            reply.status, reply.body
        );
        reply.string("/knowledgeBase/knowledgeBaseId")
    }
}

/// The `sourceConfiguration` a knowledge base of `knowledge_base_type` cannot be created
/// without, or `None` for a type whose content lives in the service itself. One union
/// member per source (`smithy_query(query="shape",shape="SourceConfiguration")`:
/// `appIntegrations`, `managedSourceConfiguration`), each carrying the member its own shape
/// marks `@required`.
fn source_configuration(knowledge_base_type: &str) -> Option<Value> {
    match knowledge_base_type {
        "EXTERNAL" => Some(json!({
            "appIntegrations": {
                "appIntegrationArn": format!(
                    "arn:aws:app-integrations:{REGION}:{account}:data-integration/kb-source",
                    account = engine::EMULATOR_ACCOUNT_ID
                )
            }
        })),
        "MANAGED" => Some(json!({
            "managedSourceConfiguration": {
                "webCrawlerConfiguration": {
                    "urlConfiguration": { "seedUrls": [{ "url": "https://example.test/docs" }] }
                }
            }
        })),
        _ => None,
    }
}

/// Every operation of the quick-response plane, as a request that would otherwise be
/// served against the knowledge base `knowledge_base_id`: `(case, method, uri, body)`.
fn plane_calls(
    knowledge_base_type: &str,
    knowledge_base_id: &str,
) -> Vec<(String, &'static str, String, Option<Value>)> {
    vec![
        (
            format!("CreateQuickResponse against a {knowledge_base_type} knowledge base"),
            "POST",
            format!("/knowledgeBases/{knowledge_base_id}/quickResponses"),
            Some(json!({ "name": "gated", "content": { "content": "gated" } })),
        ),
        (
            format!("ListQuickResponses against a {knowledge_base_type} knowledge base"),
            "GET",
            format!("/knowledgeBases/{knowledge_base_id}/quickResponses"),
            None,
        ),
        (
            format!("GetQuickResponse against a {knowledge_base_type} knowledge base"),
            "GET",
            format!("/knowledgeBases/{knowledge_base_id}/quickResponses/{ABSENT_UUID}"),
            None,
        ),
        (
            format!("UpdateQuickResponse against a {knowledge_base_type} knowledge base"),
            "POST",
            format!("/knowledgeBases/{knowledge_base_id}/quickResponses/{ABSENT_UUID}"),
            Some(json!({ "name": "gated" })),
        ),
        (
            format!("DeleteQuickResponse against a {knowledge_base_type} knowledge base"),
            "DELETE",
            format!("/knowledgeBases/{knowledge_base_id}/quickResponses/{ABSENT_UUID}"),
            None,
        ),
        (
            format!("SearchQuickResponses against a {knowledge_base_type} knowledge base"),
            "POST",
            format!("/knowledgeBases/{knowledge_base_id}/search/quickResponses"),
            Some(json!({
                "searchExpression": {
                    "queries": [{ "name": "name", "values": ["gated"], "operator": "CONTAINS" }]
                }
            })),
        ),
    ]
}

/// RULE: a quick response lives only in a knowledge base whose type holds quick responses,
/// so every operation of that plane refuses a knowledge base of any other modelled type
/// with HTTP 400 `ValidationException` naming that stored type.
#[tokio::test]
async fn the_quick_response_plane_is_refused_for_a_knowledge_base_type_that_holds_none() {
    let emulator = Emulator::new();

    for knowledge_base_type in TYPES_WITHOUT_QUICK_RESPONSES {
        let knowledge_base_id = emulator
            .knowledge_base(
                &format!("gated-kb-{}", knowledge_base_type.to_lowercase()),
                knowledge_base_type,
            )
            .await;
        let expected = message_plane_unsupported(knowledge_base_type);

        for (case, method, uri, body) in plane_calls(knowledge_base_type, &knowledge_base_id) {
            let refused = emulator.send(method, &uri, body).await;
            assert_eq!(
                refused.status, 400,
                "case {case:?}: expected HTTP 400, got {} with {}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.code(),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {:?} with {}",
                refused.code(),
                refused.body
            );
            assert_eq!(
                refused.message(),
                expected,
                "case {case:?}: expected the refusal to name the stored knowledge-base type, \
                 got {}",
                refused.body
            );
        }
    }
}

/// RULE, from the other side: the gate refuses the TYPE and not the plane, so every one of
/// those six calls is served against a `QUICK_RESPONSES` knowledge base.
#[tokio::test]
async fn the_same_calls_are_served_against_a_quick_responses_knowledge_base() {
    let emulator = Emulator::new();
    let knowledge_base_id = emulator
        .knowledge_base("ungated-kb", TYPE_QUICK_RESPONSES)
        .await;

    let created = emulator
        .send(
            "POST",
            &format!("/knowledgeBases/{knowledge_base_id}/quickResponses"),
            Some(json!({ "name": "ungated", "content": { "content": "ungated" } })),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case \"CreateQuickResponse against a QUICK_RESPONSES knowledge base\": expected \
         HTTP 200, got {} with {}",
        created.status, created.body
    );
    let quick_response_id = created.string("/quickResponse/quickResponseId");

    // Each row: the case, the call, and the status it must answer — the create above
    // having persisted the record the label-addressed ones name.
    let rows: Vec<(&str, &str, String, Option<Value>, u16)> = vec![
        (
            "ListQuickResponses",
            "GET",
            format!("/knowledgeBases/{knowledge_base_id}/quickResponses"),
            None,
            200,
        ),
        (
            "GetQuickResponse",
            "GET",
            format!("/knowledgeBases/{knowledge_base_id}/quickResponses/{quick_response_id}"),
            None,
            200,
        ),
        (
            "SearchQuickResponses",
            "POST",
            format!("/knowledgeBases/{knowledge_base_id}/search/quickResponses"),
            Some(json!({
                "searchExpression": {
                    "queries": [{ "name": "name", "values": ["ungated"], "operator": "CONTAINS" }]
                }
            })),
            200,
        ),
        (
            "UpdateQuickResponse",
            "POST",
            format!("/knowledgeBases/{knowledge_base_id}/quickResponses/{quick_response_id}"),
            Some(json!({ "description": "still ungated" })),
            200,
        ),
        (
            "DeleteQuickResponse",
            "DELETE",
            format!("/knowledgeBases/{knowledge_base_id}/quickResponses/{quick_response_id}"),
            None,
            204,
        ),
    ];

    for (case, method, uri, body, expected) in rows {
        let reply = emulator.send(method, &uri, body).await;
        assert_eq!(
            reply.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} with {}",
            reply.status, reply.body
        );
    }
}
