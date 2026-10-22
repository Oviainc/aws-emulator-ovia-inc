// SPDX-License-Identifier: CC0-1.0

//! Which requests this service claims when its SigV4 signing name is not its alone.
//!
//! The AgentCore CONTROL plane signs as `bedrock-agentcore` — a signing name it SHARES
//! with the AgentCore data plane, whose endpoint is `bedrock-agentcore` while this one's
//! is `bedrock-agentcore-control`. The router resolves a request's routing key from the
//! SigV4 credential scope first, so both services see the key `bedrock-agentcore`: a
//! service that claims on the signing name alone claims its sibling's traffic too and
//! the router refuses every AgentCore request as `500 AmbiguousService`, a code no AWS
//! model carries.
//!
//! The rule under test, from this service's side of that pair: a request signed for a
//! shared signing name (or addressed to this endpoint's host) is claimed only when THIS
//! service's own model binds a route — a method AND a URI — for it. A first path segment
//! cannot decide it: the two models publish the same first segment for nine resource
//! families.
//!
//! The cross-service half of the rule (that exactly ONE of the two claims every such
//! request) is pinned in
//! `core/router/tests/all/bedrock_agentcore_signing_name_routing.rs`, the only place
//! both services can be mounted together.

use std::sync::Arc;

use serde_json::Value;
use smithy_server_runtime::{MultiServiceRouter, Service};

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// A SigV4 `Authorization` header whose credential scope names `signing_name`. The
/// signature is never checked here: `can_handle` reads the scope, not the signature.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// The request parts the router hands to [`Service::can_handle`]. `scope` is the signing
/// name the request is signed for, `None` for an unsigned (anonymous) request.
fn parts(method: &str, uri: &str, scope: Option<&str>, host: &str) -> http::request::Parts {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", host)
        .header("content-type", "application/json");
    if let Some(scope) = scope {
        builder = builder.header("authorization", authorization(scope));
    }
    builder
        .body(bytes::Bytes::new())
        .expect("the test request is well formed")
        .into_parts()
        .0
}

/// This service alone, over its own engine.
fn service() -> EmulatedService<engine::InMemoryEngine> {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    EmulatedService::new(engine)
}

/// This service alone behind a router, so a claimed request is really dispatched.
fn router() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service());
    router
}

/// One answer from the router: HTTP status and the `restJson1` `__type` error code (the
/// empty string on a response that carries none).
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Value) -> (u16, String) {
    let payload = if body.is_null() {
        bytes::Bytes::new()
    } else {
        bytes::Bytes::from(body.to_string())
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "bedrock-agentcore-control.us-east-1.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", authorization("bedrock-agentcore"))
        .body(payload)
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    let code = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    (status, code)
}

/// Every row is a METHOD + URI, because the discriminator is the whole modelled route.
///
/// The control-plane routes come from this crate's `OPERATIONS.md` (`smithy_query`'s
/// `http` bindings); the data-plane ones from
/// `services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/OPERATIONS.md`.
/// Rows differ along what the rule generalises over: which model binds the route, the
/// method, the first path segment (shared or not), the depth at which the two models
/// diverge, and whether a route is bound at all.
#[test]
fn this_service_claims_the_routes_its_own_model_binds_and_no_more() {
    let service = service();

    // (case, method, uri, claimed)
    let rows: [(&str, &str, &str, bool); 24] = [
        // Its own routes, including the trailing-slash ones and the gateway family the
        // issue was filed about.
        ("own: CreateGateway", "POST", "/gateways/", true),
        ("own: GetGateway", "GET", "/gateways/gw-1/", true),
        ("own: ListGateways", "GET", "/gateways/", true),
        ("own: UpdateGateway", "PUT", "/gateways/gw-1/", true),
        ("own: DeleteGateway", "DELETE", "/gateways/gw-1/", true),
        ("own: CreateGatewayTarget", "POST", "/gateways/gw-1/targets/", true),
        ("own: GetGatewayTarget", "GET", "/gateways/gw-1/targets/t-1/", true),
        ("own: CreateAgentRuntime", "PUT", "/runtimes/", true),
        ("own: GetAgentRuntime", "GET", "/runtimes/r-1/", true),
        ("own: CreateAgentRuntimeVersion-listing", "POST", "/runtimes/r-1/versions/", true),
        ("own: CreateMemory", "POST", "/memories/create", true),
        ("own: GetMemory", "GET", "/memories/m-1/details", true),
        ("own: CreateBrowser", "POST", "/browsers", true),
        ("own: GetCodeInterpreter", "GET", "/code-interpreters/ci-1", true),
        ("own: CreateWorkloadIdentity", "POST", "/identities/CreateWorkloadIdentity", true),
        ("own: ListTagsForResource", "GET", "/tags/arn", true),
        // The data plane's routes, on the first path segments the two models share.
        ("sibling: InvokeAgentRuntime", "POST", "/runtimes/r-1/invocations", false),
        ("sibling: CreateEvent", "POST", "/memories/m-1/events", false),
        ("sibling: GetMemoryRecord", "GET", "/memories/m-1/memoryRecord/rec-1", false),
        ("sibling: StartBrowserSession", "PUT", "/browsers/b-1/sessions/start", false),
        (
            "sibling: GetWorkloadAccessToken",
            "POST",
            "/identities/GetWorkloadAccessToken",
            false,
        ),
        (
            // Modelled by the sibling but UNWIRED there (a `@streaming` event-stream
            // output the scaffold generator declines), so it is in no route table at all
            // — which is still not a claim by THIS service.
            "sibling: InvokeHarness, modelled but unwired",
            "POST",
            "/harnesses/invoke",
            false,
        ),
        // A method neither model binds on a path this one publishes, and a path neither
        // model binds at all: both reach nobody rather than two claimants.
        ("neither: PATCH on its own gateway path", "PATCH", "/gateways/gw-1/", false),
        ("neither: a path no model binds", "POST", "/no-such-resource", false),
    ];

    for (case, method, uri, expected) in rows {
        let request = parts(method, uri, Some("bedrock-agentcore"), "localhost:9297");
        let actual = service.can_handle(&request);
        assert_eq!(
            actual, expected,
            "case {case:?} ({method} {uri}): expected can_handle == {expected}, got {actual}"
        );
    }
}

/// The routing key may name this endpoint two ways: the signing name a signed request
/// carries, and the `endpointPrefix` an unsigned, host-addressed request resolves to.
#[test]
fn this_service_answers_on_its_signing_name_and_on_its_endpoint_host() {
    let service = service();

    // (case, scope, host, claimed) — all on one of its own routes, `POST /gateways/`.
    let rows: [(&str, Option<&str>, &str, bool); 6] = [
        (
            "signed for the shared signing name",
            Some("bedrock-agentcore"),
            "localhost:9297",
            true,
        ),
        (
            "signed for the shared signing name, on this endpoint's host",
            Some("bedrock-agentcore"),
            "bedrock-agentcore-control.us-east-1.amazonaws.com",
            true,
        ),
        (
            "unsigned, addressed to this endpoint's host",
            None,
            "bedrock-agentcore-control.eu-west-1.amazonaws.com",
            true,
        ),
        (
            // The key `bedrock-agentcore` IS this service's signing name as well as the
            // sibling endpoint's prefix, so this seam cannot tell the two apart — and
            // does not need to: the ROUTE decides, and `POST /gateways/` is a route only
            // this model binds, so this service still claims it alone.
            "unsigned, addressed to the sibling endpoint's host",
            None,
            "bedrock-agentcore.eu-west-1.amazonaws.com",
            true,
        ),
        (
            "signed for another service entirely",
            Some("lambda"),
            "bedrock-agentcore-control.us-east-1.amazonaws.com",
            false,
        ),
        ("unsigned, with no host that names a service", None, "localhost:9297", false),
    ];

    for (case, scope, host, expected) in rows {
        let request = parts("POST", "/gateways/", scope, host);
        let actual = service.can_handle(&request);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected can_handle == {expected}, got {actual}"
        );
    }
}

/// The narrowed claim still reaches the handlers: a request on one of this service's own
/// routes is answered by this service, never by a routing refusal.
///
/// Each row names the answer of the handler that owns the route — its modelled success,
/// its modelled error, or (for an operation still scaffolded) the scaffold's `501
/// NotImplemented`. All three are this service answering, unlike `500 AmbiguousService` /
/// `404 UnknownService`, which are the router refusing.
#[tokio::test]
async fn requests_on_its_own_routes_reach_this_service() {
    let router = router();

    // (case, method, uri, body, expected status, expected error code)
    let rows: [(&str, &str, &str, Value, u16, &str); 5] = [
        (
            "CreateGateway, implemented",
            "POST",
            "/gateways/",
            serde_json::json!({
                "name": "lambda-mcp-gateway",
                "roleArn": "arn:aws:iam::123456789012:role/GatewayRole",
                "protocolType": "MCP",
                "authorizerType": "AWS_IAM"
            }),
            202,
            "",
        ),
        (
            "ListGateways, still scaffolded",
            "GET",
            "/gateways/",
            Value::Null,
            501,
            "NotImplemented",
        ),
        (
            "GetGatewayTarget on a gateway that does not exist, implemented",
            "GET",
            "/gateways/my-gateway-abcdefghij/targets/abcdefghij/",
            Value::Null,
            404,
            "ResourceNotFoundException",
        ),
        (
            "CreateAgentRuntime, implemented",
            "PUT",
            "/runtimes/",
            serde_json::json!({
                "agentRuntimeName": "routing_probe",
                "agentRuntimeArtifact": {
                    "containerConfiguration": {
                        "containerUri": "123456789012.dkr.ecr.us-east-1.amazonaws.com/agent:1"
                    }
                },
                "networkConfiguration": {"networkMode": "PUBLIC"},
                "roleArn": "arn:aws:iam::123456789012:role/AgentRuntimeRole"
            }),
            202,
            "",
        ),
        (
            "ListAgentRuntimes, implemented",
            "POST",
            "/runtimes/",
            Value::Null,
            200,
            "",
        ),
    ];

    for (case, method, uri, body, expected_status, expected_code) in rows {
        let (status, code) = call(&router, method, uri, body).await;
        assert_eq!(
            (status, code.as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected HTTP {expected_status} {expected_code:?} from this service, \
             got {status} {code:?}"
        );
    }
}
