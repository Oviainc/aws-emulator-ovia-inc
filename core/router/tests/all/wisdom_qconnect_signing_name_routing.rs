// SPDX-License-Identifier: CC0-1.0

//! Cross-service routing for the `wisdom` SigV4 signing name — the endpoint Amazon Q in
//! Connect and Amazon Connect Wisdom SHARE.
//!
//! TWO emulated services resolve to the key `wisdom`: the Amazon Q in Connect model
//! (`com.amazonaws.qconnect#WisdomService`, sdkId `QConnect`, 94 operations) and the
//! Amazon Connect Wisdom model of that same endpoint
//! (`com.amazonaws.wisdom#WisdomService`, sdkId `Wisdom`, 41 operations). Their
//! `smithy_query(query="service_info")` differs ONLY in sdkId: the same service shape
//! `WisdomService` version 2020-10-19, the same `restJson1` protocol, the same
//! sigv4Name/arnNamespace `wisdom`, the same endpoint templates
//! `https://wisdom.{Region}.{dnsSuffix}` — and the 41 operations of the smaller model are
//! bound at the SAME method+URI routes as their 94-operation counterparts. restJson1
//! carries no `X-Amz-Target`, so a request is dispatched by method+URI, which here cannot
//! tell the two models apart: a service that claims on the signing name alone claims the
//! other's traffic too, the router finds two claimants and answers `500 AmbiguousService`
//! — an error code no AWS model declares, serialized in the wrong protocol envelope
//! (`application/x-amz-json-1.1`) for a restJson1 client.
//!
//! The rule these suites pin: a request is claimed by exactly ONE emulated service — the
//! one whose own wire identity matches it, which is its route key (signing name, endpoint
//! prefix, sdkId-derived key, or the registration key an alias model answers under) AND
//! the URI path space its protocol publishes; where two mounted models are the SAME AWS
//! service at one endpoint, the shared key is served by the model that SUBSUMES the other
//! (the endpoint's current API, so no client loses an operation) and the subsumed model
//! answers under its own key.
//!
//! Rows therefore span the dimensions that rule generalises over: which signal resolves
//! the route key (a SigV4 `Authorization` scope, a presigned `X-Amz-Credential`, or the
//! endpoint host on an anonymous request), which key the request names (`wisdom`,
//! `qconnect`, the legacy model's registration key), a `Host` that agrees with,
//! disagrees with or says nothing about the target, the path space each model publishes
//! (`/assistants`, `/knowledgeBases`, `/tags`), a route only the superset model declares,
//! and a path no model binds.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::{MultiServiceRouter, Service};

/// The signing name BOTH models declare, as every signed SDK request to this endpoint
/// carries it. The signature is never checked: the routers here run with request
/// authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTWISDOM/20260101/us-east-1/wisdom/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The same header with the scope naming the superset model's own sdkId-derived key, so a
/// client addressed at Amazon Q in Connect by product name reaches it too.
const AUTHORIZATION_QCONNECT: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTWISDOM/20260101/us-east-1/qconnect/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The same header with the scope naming the registration key the LEGACY Wisdom model
/// answers under (its crate's `service.rs` `OWN_REGISTRATION_KEY`), so the alias crate
/// stays reachable without contending for `wisdom`.
const AUTHORIZATION_LEGACY: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTWISDOM/20260101/us-east-1/com-amazonaws-wisdom-wisdomservice/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The `wisdom` credential scope as a presigned URL carries it: in the query string, with
/// its `/` separators percent-encoded.
const PRESIGNED_CREDENTIAL: &str =
    "X-Amz-Credential=AKIDTESTWISDOM%2F20260101%2Fus-east-1%2Fwisdom%2Faws4_request";

/// How a row names the service that must claim it.
const OWNER_QCONNECT: &str = "qconnect (QConnect, the endpoint's superset API)";
const OWNER_WISDOM: &str = "wisdom (the legacy Wisdom alias model)";

/// A UUID no create mints, for the rows that address a resource by id.
const ABSENT_UUID: &str = "00000000-0000-0000-0000-000000000000";

type QConnect =
    service_com_amazonaws_qconnect_wisdomservice::EmulatedService<engine::InMemoryEngine>;
type Wisdom = service_com_amazonaws_wisdom_wisdomservice::EmulatedService<engine::InMemoryEngine>;

/// The two services that resolve to the `wisdom` key, over one shared engine, exactly as
/// `core/router/src/lib.rs` mounts them.
fn both() -> (QConnect, Wisdom) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (
        QConnect::new(Arc::clone(&engine)),
        Wisdom::new(Arc::clone(&engine)),
    )
}

/// A router with both of them mounted, so a request that two services claim really is
/// refused as `AmbiguousService`.
fn router_with_both() -> MultiServiceRouter {
    let (qconnect, wisdom) = both();
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(qconnect);
    router.register_service(wisdom);
    router
}

/// How a row's request is signed: what resolves its route key.
#[derive(Debug, Clone, Copy)]
enum Signing {
    /// A SigV4 `Authorization` header naming `wisdom`, as every signed SDK request to
    /// this endpoint carries — whichever of the two APIs the client speaks.
    Header,
    /// A SigV4 `Authorization` header naming the superset model's own sdkId key.
    QConnectHeader,
    /// A SigV4 `Authorization` header naming the legacy alias model's registration key.
    LegacyHeader,
    /// A presigned URL's `X-Amz-Credential` query parameter, naming `wisdom`.
    Query,
    /// Nothing at all, so the endpoint host resolves the key.
    Anonymous,
}

fn authorization(signing: Signing) -> Option<&'static str> {
    match signing {
        Signing::Header => Some(AUTHORIZATION),
        Signing::QConnectHeader => Some(AUTHORIZATION_QCONNECT),
        Signing::LegacyHeader => Some(AUTHORIZATION_LEGACY),
        Signing::Query | Signing::Anonymous => None,
    }
}

/// The request parts the router hands to `Service::can_handle`.
fn parts(method: &str, signing: Signing, host: &str, uri: &str) -> http::request::Parts {
    let uri = match signing {
        Signing::Query if uri.contains('?') => format!("{uri}&{PRESIGNED_CREDENTIAL}"),
        Signing::Query => format!("{uri}?{PRESIGNED_CREDENTIAL}"),
        _ => uri.to_string(),
    };
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", host);
    if let Some(authorization) = authorization(signing) {
        builder = builder.header("authorization", authorization);
    }
    builder
        .body(Bytes::new())
        .expect("the test request is well formed")
        .into_parts()
        .0
}

/// Every service that claims a request, by label — so a row pins BOTH that the right
/// service claims it and that no other does.
fn claimants(
    qconnect: &QConnect,
    wisdom: &Wisdom,
    parts: &http::request::Parts,
) -> Vec<&'static str> {
    [
        (OWNER_QCONNECT, qconnect.can_handle(parts)),
        (OWNER_WISDOM, wisdom.can_handle(parts)),
    ]
    .into_iter()
    .filter_map(|(label, claimed)| claimed.then_some(label))
    .collect()
}

/// One answer from the router: the HTTP status, the `x-amzn-errortype` code (restJson1
/// carries the modelled error shape's name there), the `content-type` and the parsed
/// body.
struct Answer {
    status: u16,
    error_type: String,
    content_type: String,
    body: Value,
}

async fn call(
    router: &MultiServiceRouter,
    signing: Signing,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "wisdom.us-east-1.amazonaws.com");
    if let Some(authorization) = authorization(signing) {
        builder = builder.header("authorization", authorization);
    }
    let request = match &body {
        Some(value) => builder
            .header("content-type", "application/json")
            .body(Bytes::from(value.to_string()))
            .expect("the test request is well formed"),
        None => builder
            .body(Bytes::new())
            .expect("the test request is well formed"),
    };
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    let error_type = header("x-amzn-errortype");
    let content_type = header("content-type");
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_type: error_type
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .split(':')
            .next()
            .unwrap_or_default()
            .to_string(),
        content_type,
        body: parsed,
    }
}

/// The wire error code in the body — `__type` for a restJson1 error and for the router's
/// OWN refusals, which is how a routing failure is told apart from a service's answer.
fn body_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// One row of the claim table: the case name, the HTTP method, how the request is
/// signed, its `Host`, its URI, and every service that must claim it.
type ClaimRow = (
    &'static str,
    &'static str,
    Signing,
    &'static str,
    String,
    Vec<&'static str>,
);

/// One row of the served table: the case name, the method, the URI, the request body,
/// the status the owning service answers (or `None`) and the modelled error code.
type ServedRow = (
    &'static str,
    &'static str,
    String,
    Option<Value>,
    Option<u16>,
    &'static str,
);

#[test]
fn exactly_one_service_claims_each_request_for_the_wisdom_endpoint() {
    let (qconnect, wisdom) = both();

    // (case, method, how the request is signed, Host, URI, the services that must claim
    // it). The paths are the ones the two models bind: both publish the first segments
    // `assistants`, `knowledgeBases` and `tags`.
    let rows: Vec<ClaimRow> = vec![
        (
            "CreateAssistant, signed, on the wisdom host",
            "POST",
            Signing::Header,
            "wisdom.us-east-1.amazonaws.com",
            "/assistants".to_string(),
            vec![OWNER_QCONNECT],
        ),
        (
            "ListAssistants, signed, on a bare emulator endpoint",
            "GET",
            Signing::Header,
            "localhost:9297",
            "/assistants".to_string(),
            vec![OWNER_QCONNECT],
        ),
        (
            "GetAssistant by id, signed",
            "GET",
            Signing::Header,
            "wisdom.eu-west-1.amazonaws.com",
            format!("/assistants/{ABSENT_UUID}"),
            vec![OWNER_QCONNECT],
        ),
        (
            "CreateKnowledgeBase, signed",
            "POST",
            Signing::Header,
            "wisdom.us-east-1.amazonaws.com",
            "/knowledgeBases".to_string(),
            vec![OWNER_QCONNECT],
        ),
        (
            "ListTagsForResource, signed: the third shared path segment",
            "GET",
            Signing::Header,
            "wisdom.us-east-1.amazonaws.com",
            format!(
                "/tags/arn:aws:wisdom:us-east-1:000000000000:assistant%2F{ABSENT_UUID}"
            ),
            vec![OWNER_QCONNECT],
        ),
        (
            "CreateAIAgent, a route only the superset model declares",
            "POST",
            Signing::Header,
            "wisdom.us-east-1.amazonaws.com",
            format!("/assistants/{ABSENT_UUID}/aiagents"),
            vec![OWNER_QCONNECT],
        ),
        (
            "ListAssistants, presigned in the query string",
            "GET",
            Signing::Query,
            "wisdom.us-east-1.amazonaws.com",
            "/assistants".to_string(),
            vec![OWNER_QCONNECT],
        ),
        (
            "an anonymous request addressed to the wisdom host",
            "GET",
            Signing::Anonymous,
            "wisdom.us-east-1.amazonaws.com",
            "/assistants".to_string(),
            vec![OWNER_QCONNECT],
        ),
        (
            "an anonymous request addressed to the qconnect product host",
            "GET",
            Signing::Anonymous,
            "qconnect.us-east-1.amazonaws.com",
            "/assistants".to_string(),
            vec![OWNER_QCONNECT],
        ),
        (
            "CreateAssistant signed for the superset model's own sdkId key",
            "POST",
            Signing::QConnectHeader,
            "wisdom.us-east-1.amazonaws.com",
            "/assistants".to_string(),
            vec![OWNER_QCONNECT],
        ),
        // The legacy alias model's own registration key: claimed by that crate alone, so
        // it stays reachable without ever contending for `wisdom`.
        (
            "CreateAssistant signed for the legacy model's registration key",
            "POST",
            Signing::LegacyHeader,
            "wisdom.us-east-1.amazonaws.com",
            "/assistants".to_string(),
            vec![OWNER_WISDOM],
        ),
        (
            "CreateKnowledgeBase signed for the legacy model's registration key",
            "POST",
            Signing::LegacyHeader,
            "wisdom.us-east-1.amazonaws.com",
            "/knowledgeBases".to_string(),
            vec![OWNER_WISDOM],
        ),
        (
            "an anonymous request addressed to the legacy model's registration host",
            "GET",
            Signing::Anonymous,
            "com-amazonaws-wisdom-wisdomservice.emulator.local",
            "/assistants".to_string(),
            vec![OWNER_WISDOM],
        ),
        // Paths no `wisdom` model binds, and a request for another service entirely:
        // claimed by nobody, which is the router's `404 UnknownService` and NOT an
        // ambiguity.
        (
            "a path no wisdom model binds, signed",
            "GET",
            Signing::Header,
            "wisdom.us-east-1.amazonaws.com",
            "/v1/assistants".to_string(),
            vec![],
        ),
        (
            "a path no wisdom model binds, on the legacy key",
            "GET",
            Signing::LegacyHeader,
            "wisdom.us-east-1.amazonaws.com",
            "/v1/assistants".to_string(),
            vec![],
        ),
        (
            "an anonymous request addressed to an unrelated host",
            "GET",
            Signing::Anonymous,
            "sqs.us-east-1.amazonaws.com",
            "/assistants".to_string(),
            vec![],
        ),
    ];

    for (case, method, signing, host, uri, expected) in rows {
        let parts = parts(method, signing, host, &uri);
        let actual = claimants(&qconnect, &wisdom, &parts);
        assert_eq!(
            actual, expected,
            "case {case:?} ({method} {uri} to host {host}, {signing:?}): expected it to be \
             claimed by exactly {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_wisdom_signed_request_is_served_in_restjson1_instead_of_refused_as_ambiguous() {
    let router = router_with_both();

    // The knowledge base the conflict row needs, created first so the rows below can be
    // a flat table.
    let created_kb = call(
        &router,
        Signing::Header,
        "POST",
        "/knowledgeBases",
        Some(json!({ "name": "routing-probe-kb", "knowledgeBaseType": "CUSTOM" })),
    )
    .await;
    assert_eq!(
        (created_kb.status, body_code(&created_kb.body).as_str()),
        (200, ""),
        "case \"CreateKnowledgeBase\": expected HTTP 200 from the service that owns the \
         wisdom key, got {} {:?} ({})",
        created_kb.status,
        created_kb.error_type,
        created_kb.body
    );

    // (case, method, URI, body, the status the owning service answers or `None` when
    // only "a service answered in restJson1" is pinned, the modelled error code)
    //
    // Each row names the answer of the ONE service that owns the shared key, which is
    // what excludes both routing failures: `AmbiguousService` (500, several claimants)
    // and `UnknownService` (404, none). `CreateAIAgent` carries `None`: that route is
    // declared only by the superset model and its handler is still scaffolded, so the row
    // pins that the claimant answered in ITS protocol rather than pinning a 501 that
    // would fail the day it is implemented.
    let rows: Vec<ServedRow> = vec![
        (
            "CreateAssistant on a free name",
            "POST",
            "/assistants".to_string(),
            Some(json!({ "name": "routing-probe", "type": "AGENT", "tags": { "env": "test" } })),
            Some(200),
            "",
        ),
        (
            "ListAssistants",
            "GET",
            "/assistants".to_string(),
            None,
            Some(200),
            "",
        ),
        (
            "GetAssistant on an assistant that does not exist",
            "GET",
            format!("/assistants/{ABSENT_UUID}"),
            None,
            Some(404),
            "ResourceNotFoundException",
        ),
        (
            "CreateKnowledgeBase on a name already in use",
            "POST",
            "/knowledgeBases".to_string(),
            Some(json!({ "name": "routing-probe-kb", "knowledgeBaseType": "CUSTOM" })),
            Some(409),
            "ConflictException",
        ),
        (
            "GetKnowledgeBase on a knowledge base that does not exist",
            "GET",
            format!("/knowledgeBases/{ABSENT_UUID}"),
            None,
            Some(404),
            "ResourceNotFoundException",
        ),
        (
            "CreateAIAgent, a route only the superset model declares",
            "POST",
            format!("/assistants/{ABSENT_UUID}/aiagents"),
            Some(json!({
                "name": "routing-probe-agent",
                "type": "MANUAL_SEARCH",
                "visibilityStatus": "SAVED",
                "configuration": { "manualSearchAIAgentConfiguration": {} }
            })),
            None,
            "",
        ),
    ];

    for (case, method, uri, body, expected_status, expected_code) in rows {
        let answer = call(&router, Signing::Header, method, &uri, body).await;
        let code = body_code(&answer.body);
        assert!(
            code != "AmbiguousService" && code != "UnknownService",
            "case {case:?}: {method} {uri} must be served by exactly one of the two services \
             that resolve to the `wisdom` key, got HTTP {} {code:?} ({})",
            answer.status,
            answer.body
        );
        assert_eq!(
            answer.content_type, "application/json",
            "case {case:?}: a restJson1 service answers application/json, never the router's \
             awsJson envelope, got {:?} with HTTP {} ({})",
            answer.content_type, answer.status, answer.body
        );
        if let Some(expected_status) = expected_status {
            assert_eq!(
                (answer.status, answer.error_type.as_str()),
                (expected_status, expected_code),
                "case {case:?}: expected HTTP {expected_status} {expected_code:?} from the \
                 service that owns the wisdom key, got {} {:?} ({})",
                answer.status,
                answer.error_type,
                answer.body
            );
        } else {
            assert_ne!(
                answer.status, 500,
                "case {case:?}: the claimant must answer its own modelled envelope, not a \
                 router refusal, got HTTP {} ({})",
                answer.status, answer.body
            );
        }
    }

    // The required output member of the paginated list is present, which is what an SDK
    // paginator reads.
    let listed = call(&router, Signing::Header, "GET", "/assistants", None).await;
    let summaries = listed.body.get("assistantSummaries");
    assert!(
        summaries.is_some_and(Value::is_array),
        "case \"ListAssistants\": `assistantSummaries` is a REQUIRED output member, got {}",
        listed.body
    );
    assert_eq!(
        summaries.and_then(Value::as_array).map(Vec::len),
        Some(1),
        "case \"ListAssistants\": expected the one assistant created above, got {}",
        listed.body
    );
}

#[tokio::test]
async fn the_legacy_model_serves_its_own_key_and_its_own_store() {
    let router = router_with_both();

    // An assistant created through the SHARED key is held by the superset service.
    let shared = call(
        &router,
        Signing::Header,
        "POST",
        "/assistants",
        Some(json!({ "name": "shared-key-assistant", "type": "AGENT" })),
    )
    .await;
    assert_eq!(
        shared.status, 200,
        "case \"CreateAssistant on the shared key\": expected HTTP 200, got {} ({})",
        shared.status, shared.body
    );
    let shared_id = shared
        .body
        .pointer("/assistant/assistantId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    // One created through the legacy model's own key is held by THAT service: the two
    // keys reach two services, each answering out of its own store.
    let legacy = call(
        &router,
        Signing::LegacyHeader,
        "POST",
        "/assistants",
        Some(json!({ "name": "legacy-key-assistant", "type": "AGENT" })),
    )
    .await;
    assert_eq!(
        legacy.status, 200,
        "case \"CreateAssistant on the legacy key\": expected HTTP 200, got {} ({})",
        legacy.status, legacy.body
    );
    let legacy_id = legacy
        .body
        .pointer("/assistant/assistantId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    for (case, signing, id, expected_status) in [
        (
            "the shared key's assistant, read on the shared key",
            Signing::Header,
            shared_id.clone(),
            200,
        ),
        (
            "the legacy key's assistant, read on the legacy key",
            Signing::LegacyHeader,
            legacy_id.clone(),
            200,
        ),
        (
            "the legacy key's assistant, read on the shared key",
            Signing::Header,
            legacy_id,
            404,
        ),
        (
            "the shared key's assistant, read on the legacy key",
            Signing::LegacyHeader,
            shared_id,
            404,
        ),
    ] {
        let answer = call(&router, signing, "GET", &format!("/assistants/{id}"), None).await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} ({})",
            answer.status, answer.body
        );
        assert_eq!(
            answer.content_type, "application/json",
            "case {case:?}: a restJson1 service answers application/json, got {:?}",
            answer.content_type
        );
    }
}
