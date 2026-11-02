// SPDX-License-Identifier: CC0-1.0

//! The MCP endpoint a gateway publishes, across the services that own the two halves:
//! AgentCore Control publishes the endpoint and holds the gateway's targets, Lambda owns
//! the function a tool call is an invocation of.
//!
//! Two rules are pinned here, neither of which can be seen with one service mounted:
//!
//! 1. The host `CreateGateway` publishes in `gatewayUrl` is claimed by exactly ONE
//!    emulated service — the control plane that minted it — even though the AgentCore
//!    CONTROL and DATA planes share the `bedrock-agentcore` signing name the request is
//!    signed with. Two claimants would be the router's `500 AmbiguousService`, none its
//!    `404 UnknownService`; both were what that URL answered before it was served at all.
//!
//! 2. `tools/call` on that endpoint is TRANSLATED into an invocation of the target's own
//!    `lambdaArn`, through the in-process callback into this same emulator — so what the
//!    tool reports is what Lambda answered for that function, and two different states of
//!    the function give two different answers. Running a deployment package is not
//!    implemented in this emulator's Lambda
//!    (`issues/things-that-do-not-work/20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md`),
//!    so no tool call here can return a result — which is the point: the answer carries
//!    Lambda's own refusal rather than a result nothing produced.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::{MultiServiceRouter, Service};

/// The Region every request here is signed for, and the Region the gateway's endpoint host
/// carries.
const REGION: &str = "us-west-2";

/// A role ARN inside `RoleArn`'s pattern, for both services.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/mcp-exec";

/// A deployment package as `Code.ZipFile` carries it: base64 in a `restJson1` JSON body.
const ZIP_FILE: &str = "UEsDBAoAAAAAAAAAAAA=";

/// Past the 500 ms the gateway, its target and the Lambda function each take to leave
/// their transient status, so one wait settles all three.
const PAST_SETTLE: Duration = Duration::from_millis(900);

type Control =
    service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService<
        engine::InMemoryEngine,
    >;
type Data = service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore::EmulatedService<
    engine::InMemoryEngine,
>;
type Lambda =
    service_com_amazonaws_lambda_awsgirapiservice::EmulatedService<engine::InMemoryEngine>;

/// The three services over ONE shared engine, as `router/src/lib.rs` mounts them: the two
/// that share the AgentCore signing name, and the Lambda a gateway tool call reaches.
fn services() -> (Control, Data, Lambda) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (
        Control::new(Arc::clone(&engine)),
        Data::new(Arc::clone(&engine)),
        Lambda::new(engine),
    )
}

fn router() -> MultiServiceRouter {
    let (control, data, lambda) = services();
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(control);
    router.register_service(data);
    router.register_service(lambda);
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name`. The
/// signature is never checked: the router here runs with request authentication off.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// One call to the service whose signing name is `signing_name`, at `uri`.
async fn call(
    router: &MultiServiceRouter,
    signing_name: &str,
    method: &str,
    uri: &str,
    body: Value,
) -> (u16, Value) {
    let payload = if body.is_null() {
        Bytes::new()
    } else {
        Bytes::from(body.to_string())
    };
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization(signing_name))
        .body(payload)
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, body_json(response.body()))
}

/// The `(host, path)` a published gateway URL addresses. The URL carries BOTH: the
/// gateway's own endpoint host and the transport path its protocol is served on, so a
/// client holding it drives exactly that path and appends nothing.
fn split_url(url: &str) -> (String, String) {
    let rest = url
        .strip_prefix("https://")
        .expect("the published gateway URL is an https URL");
    match rest.find('/') {
        Some(path_start) => (
            rest[..path_start].to_string(),
            rest[path_start..].to_string(),
        ),
        None => (rest.to_string(), "/".to_string()),
    }
}

/// One JSON-RPC call to a published gateway endpoint, driven the way an MCP client holding
/// that URL does: the URL's own host and path, and a SigV4 credential for the
/// `bedrock-agentcore` signing name the gateway's `AWS_IAM` authorizer admits.
async fn mcp(router: &MultiServiceRouter, url: &str, message: &Value) -> (u16, Value) {
    let (host, path) = split_url(url);
    let request = http::Request::builder()
        .method("POST")
        .uri(path)
        .header("host", host)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-03-26")
        .header("authorization", authorization("bedrock-agentcore"))
        .body(Bytes::from(message.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, body_json(response.body()))
}

fn body_json(bytes: &Bytes) -> Value {
    if bytes.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(bytes).unwrap_or(Value::Null)
}

fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Create a Lambda function and return the ARN Lambda minted for it.
async fn create_function(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        router,
        "lambda",
        "POST",
        "/2015-03-31/functions",
        json!({
            "FunctionName": name,
            "Role": ROLE_ARN,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": {"ZipFile": ZIP_FILE},
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {name:?}: expected HTTP 201 from Lambda CreateFunction, got {status} ({created})"
    );
    text(&created, "FunctionArn")
}

/// Create a gateway and return `(gatewayId, gatewayUrl)`.
async fn create_gateway(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let (status, created) = call(
        router,
        "bedrock-agentcore",
        "POST",
        "/gateways/",
        json!({
            "name": name,
            "roleArn": ROLE_ARN,
            "protocolType": "MCP",
            "authorizerType": "AWS_IAM",
        }),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {name:?}: expected HTTP 202 from CreateGateway, got {status} ({created})"
    );
    (text(&created, "gatewayId"), text(&created, "gatewayUrl"))
}

/// Create an MCP Lambda target on `gateway_id` declaring one tool named `tool`.
async fn create_target(
    router: &MultiServiceRouter,
    gateway_id: &str,
    name: &str,
    tool: &str,
    lambda_arn: &str,
) {
    let (status, created) = call(
        router,
        "bedrock-agentcore",
        "POST",
        &format!("/gateways/{gateway_id}/targets/"),
        json!({
            "name": name,
            "targetConfiguration": {"mcp": {"lambda": {
                "lambdaArn": lambda_arn,
                "toolSchema": {"inlinePayload": [{
                    "name": tool,
                    "description": "write an object to a bucket",
                    "inputSchema": {
                        "type": "object",
                        "properties": {"key": {"type": "string"}, "body": {"type": "string"}},
                        "required": ["key", "body"]
                    }
                }]}
            }}},
        }),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {name:?}: expected HTTP 202 from CreateGatewayTarget, got {status} ({created})"
    );
}

/// The request parts the router hands to `Service::can_handle`.
fn parts(method: &str, host: &str, uri: &str) -> http::request::Parts {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", host)
        .header("content-type", "application/json")
        .header("authorization", authorization("bedrock-agentcore"))
        .body(Bytes::new())
        .expect("the test request is well formed");
    request.into_parts().0
}

/// RULE 1: exactly one service claims a request to a published gateway endpoint, and the
/// router dispatches it instead of refusing it as ambiguous or unknown.
#[tokio::test]
async fn exactly_one_service_claims_the_published_gateway_endpoint() {
    let router = router();
    let (_, url) = create_gateway(&router, "claimgateway").await;
    let (host, _) = split_url(&url);
    let (control, data, lambda) = services();

    // (case, method, host, uri, claimed by the control plane)
    //
    // Rows span the gateway endpoint's own addresses — the MCP path, a method it is not
    // addressed with, a path it publishes nothing at — beside the control-plane and
    // data-plane routes on their own hosts, so the gateway host's claim is shown not to
    // take any of the traffic the shared signing name already routes.
    let rows: [(&str, &str, &str, &str, bool); 7] = [
        ("the gateway's MCP endpoint", "POST", &host, "/mcp", true),
        ("the gateway endpoint reached with GET", "GET", &host, "/mcp", true),
        ("a path the gateway publishes nothing at", "POST", &host, "/tools", true),
        (
            "a control-plane route on the control-plane host",
            "POST",
            "bedrock-agentcore-control.us-west-2.amazonaws.com",
            "/gateways/",
            true,
        ),
        (
            "a data-plane route on the data-plane host",
            "POST",
            "bedrock-agentcore.us-west-2.amazonaws.com",
            "/runtimes/probe-0123456789/invocations",
            false,
        ),
        (
            "a Lambda route on Lambda's host",
            "POST",
            "lambda.us-west-2.amazonaws.com",
            "/2015-03-31/functions",
            false,
        ),
        (
            "a host that is not a gateway endpoint",
            "POST",
            "not-a-gateway.example.com",
            "/mcp",
            false,
        ),
    ];

    for (case, method, request_host, uri, control_claims) in rows {
        let parts = parts(method, request_host, uri);
        let claimants: Vec<&str> = [
            ("bedrock-agentcore-control", control.can_handle(&parts)),
            ("bedrock-agentcore", data.can_handle(&parts)),
            ("lambda", lambda.can_handle(&parts)),
        ]
        .into_iter()
        .filter_map(|(name, claims)| claims.then_some(name))
        .collect();
        if control_claims {
            assert_eq!(
                claimants,
                vec!["bedrock-agentcore-control"],
                "case {case:?} ({method} {uri} to {request_host}): the control plane must be the \
                 only claimant, got {claimants:?}"
            );
        } else {
            assert!(
                !claimants.contains(&"bedrock-agentcore-control"),
                "case {case:?} ({method} {uri} to {request_host}): the control plane must not \
                 claim this, got {claimants:?}"
            );
        }
    }

    // And through the router itself: the published endpoint answers the MCP exchange, not
    // a routing refusal — once the `CREATING` -> `READY` transition the create owed the
    // gateway has fallen due, because an endpoint is reachable only from `READY` onwards.
    tokio::time::sleep(PAST_SETTLE).await;
    let (status, answer) = mcp(
        &router,
        &url,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"tools/list through the router\": the published endpoint must be served, got \
         {status} ({answer})"
    );
    assert_eq!(
        answer["result"]["tools"], json!([]),
        "case \"tools/list through the router\": a gateway with no target lists no tool, got {}",
        answer["result"]
    );
}

/// RULE 2: a tool call is an invocation of the target's function, so what it reports is
/// what Lambda answered for THAT function — and two different states of the function give
/// two different answers.
#[tokio::test]
async fn a_tool_call_is_translated_into_an_invocation_of_the_target_function() {
    let router = router();
    let lambda_arn = create_function(&router, "mcp-writer").await;
    let (gateway_id, url) = create_gateway(&router, "callgateway").await;
    // A gateway takes a target only once the clock-owed transition out of `CREATING` has
    // come due, so this waits that transition out before the target is added.
    tokio::time::sleep(PAST_SETTLE).await;
    create_target(&router, &gateway_id, "writer", "put_object", &lambda_arn).await;
    tokio::time::sleep(PAST_SETTLE).await;

    // The tool the gateway aggregates for that target, under the qualified name a gateway
    // reports: `<targetName>___<toolName>`.
    let (status, listed) = mcp(
        &router,
        &url,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"tools/list\": expected HTTP 200, got {status} ({listed})"
    );
    let names: Vec<String> = listed["result"]["tools"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|tool| text(tool, "name"))
        .collect();
    assert_eq!(
        names,
        vec!["writer___put_object".to_string()],
        "case \"tools/list\": expected the target's tool under its qualified name, got {names:?}"
    );

    let message = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "writer___put_object",
            "arguments": {"key": "probe.txt", "body": "hello"}
        }
    });
    let (status, called) = mcp(&router, &url, &message).await;
    assert_eq!(
        status, 200,
        "case \"tools/call on a function that exists\": the JSON-RPC exchange succeeds, got \
         {status} ({called})"
    );
    let present = text(&called["result"]["content"][0], "text");
    assert_eq!(
        called["result"]["isError"], json!(true),
        "case \"tools/call on a function that exists\": this emulator's Lambda runs no \
         deployment package, so no result may be reported, got {}",
        called["result"]
    );
    assert!(
        present.contains("NotImplemented"),
        "case \"tools/call on a function that exists\": the tool error must carry what Lambda \
         answered for the invocation, got {present:?}"
    );

    // The SAME tool call once Lambda no longer holds the function: the answer changes with
    // the function's state, which is what shows the call really reaches Lambda.
    let (status, deleted) = call(
        &router,
        "lambda",
        "DELETE",
        &format!("/2015-03-31/functions/{lambda_arn}"),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 204,
        "case \"DeleteFunction\": expected HTTP 204 from Lambda DeleteFunction, got {status} \
         ({deleted})"
    );
    let (status, after) = mcp(&router, &url, &message).await;
    assert_eq!(
        status, 200,
        "case \"tools/call after the function was deleted\": the JSON-RPC exchange succeeds, got \
         {status} ({after})"
    );
    let absent = text(&after["result"]["content"][0], "text");
    assert_eq!(
        after["result"]["isError"], json!(true),
        "case \"tools/call after the function was deleted\": expected a tool error, got {}",
        after["result"]
    );
    assert!(
        absent.contains("ResourceNotFound"),
        "case \"tools/call after the function was deleted\": the tool error must carry Lambda's \
         own answer, got {absent:?}"
    );
    assert_ne!(
        absent, present,
        "case \"tools/call across two states of the function\": the answer must come from the \
         invocation, so the two states cannot report the same text, got {absent:?}"
    );

    // The gateway still serves: a failed tool call is the tool's answer, not the
    // endpoint's.
    let (status, again) = mcp(
        &router,
        &url,
        &json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list", "params": {}}),
    )
    .await;
    assert_eq!(
        (status, again["result"]["tools"].clone()),
        (200, listed["result"]["tools"].clone()),
        "case \"tools/list after a failed tool call\": the endpoint still lists the target's \
         tools, got {status} ({again})"
    );
}
