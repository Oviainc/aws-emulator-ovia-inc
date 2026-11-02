// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the MCP wire surface a gateway publishes: the endpoint
//! `CreateGateway` hands back in `gatewayUrl`, and what answers on it.
//!
//! RULE: a URL this service advertises is a URL it SERVES. A JSON-RPC 2.0 `POST` to the
//! URL a `READY` gateway published — its own host and the `/mcp` transport path that URL
//! carries, driven exactly as published — answers the MCP exchange
//! computed from that gateway's own persisted record: `initialize` naming the gateway,
//! `tools/list` aggregating the tools its `READY` targets DECLARE under
//! `<targetName>___<toolName>`, and `tools/call` translated into an invocation of the
//! named target's `lambdaArn`. A request the gateway's inbound authorizer does not admit
//! is refused before anything is served, and nothing at all is served for a host, a path
//! or a gateway status no gateway endpoint of this account backs.
//!
//! Every assertion goes through [`MultiServiceRouter::dispatch`], the same path the
//! emulator serves production traffic on, and every URL driven is one a DIFFERENT
//! operation (`CreateGateway`, then `GetGateway`) advertised, so an answer that is not
//! computed from the persisted gateway record cannot pass.
//!
//! Lambda is NOT mounted in this router, so a tool call cannot reach a function: the
//! other half of that rule — the call really reaching Lambda — is pinned in
//! `core/router/tests/all/bedrock_agentcore_gateway_mcp_endpoint.rs`, the only place both
//! services can be mounted together. Here what matters is that an invocation nothing
//! performed is never reported as a tool result.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::{
    state, EmulatedService,
};

/// A role ARN inside `RoleArn`'s pattern `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/GatewayRole";

/// A function ARN inside the Lambda ARN grammar `service_common::arn` reads a Region off.
/// No Lambda is mounted here, so nothing resolves it — which is exactly the case this
/// suite pins for a tool call.
const LAMBDA_ARN: &str = "arn:aws:lambda:us-west-2:000000000000:function:mcp-tools";

/// The MCP protocol revision the surface answers `initialize` with, and the one the
/// recorded client declares in `MCP-Protocol-Version`.
const MCP_PROTOCOL_VERSION: &str = "2025-03-26";

/// The Regions the tables span: the recorded one, two other geographies and a Region name
/// carrying three hyphen-separated parts — so the published host resolves back to its
/// gateway for every Region-name shape a gateway ARN may carry (`GatewayArn` spells the
/// Region `[a-z0-9-]{1,20}`).
const REGIONS: &[&str] = &["us-west-2", "eu-west-1", "ap-southeast-2", "us-gov-west-1"];

fn router() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// A SigV4 `Authorization` header for this service in `region`.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/bedrock-agentcore/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// One control-plane call, signed for `region`.
async fn control(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    region: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Bytes::new, |value| Bytes::from(value.to_string()));
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization(region))
        .header("host", format!("bedrock-agentcore-control.{region}.amazonaws.com"));
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, body_json(response.body()))
}

/// The `(host, path)` an `https://` URL addresses. Shared with the suites that drive a
/// URL this service published.
pub(crate) fn split_url(case: &str, url: &str) -> (String, String) {
    let rest = url
        .strip_prefix("https://")
        .unwrap_or_else(|| panic!("case {case:?}: expected an https URL, got {url:?}"));
    match rest.find('/') {
        Some(path_start) => (
            rest[..path_start].to_string(),
            rest[path_start..].to_string(),
        ),
        None => (rest.to_string(), "/".to_string()),
    }
}

/// Drive `url` the way an MCP client holding it does: the URL's own host in the `Host`
/// header, its path as the request target, the JSON-RPC message as the body, and the
/// headers the recorded client sends. `region` is `None` for an UNSIGNED request — an MCP
/// client that holds no Amazon Web Services credential.
async fn mcp(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    url: &str,
    region: Option<&str>,
    body: Option<&str>,
) -> (u16, Value) {
    let (host, path) = split_url(case, url);
    let mut request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", host)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", MCP_PROTOCOL_VERSION);
    if let Some(region) = region {
        request = request.header("authorization", authorization(region));
    }
    let request = request
        .body(body.map_or_else(Bytes::new, |text| Bytes::from(text.to_string())))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, body_json(response.body()))
}

/// One signed JSON-RPC call to a gateway's MCP endpoint.
async fn jsonrpc(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    region: &str,
    message: &Value,
) -> (u16, Value) {
    mcp(
        router,
        case,
        "POST",
        url,
        Some(region),
        Some(&message.to_string()),
    )
    .await
}

/// A JSON-RPC request object for `method`.
fn call(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

/// A response body as JSON, or `Value::Null` when it is empty.
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

/// The `restJson1` error code (`__type`) of an endpoint-level refusal.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// Create a gateway in `region` and return `(gatewayId, gatewayUrl)` as the create
/// reported them.
async fn create_gateway(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    name: &str,
    authorizer_type: &str,
) -> (String, String) {
    let mut body = json!({"name": name, "roleArn": ROLE_ARN, "authorizerType": authorizer_type});
    if authorizer_type == "CUSTOM_JWT" {
        body["authorizerConfiguration"] = json!({
            "customJWTAuthorizer": {
                "discoveryUrl": "https://example.com/.well-known/openid-configuration",
                "allowedClients": ["client-1"]
            }
        });
    }
    let (status, created) = control(router, "POST", "/gateways/", region, Some(&body)).await;
    assert_eq!(
        status, 202,
        "case {case:?}: expected HTTP 202 from CreateGateway, got {status} ({created})"
    );
    (text(&created, "gatewayId"), text(&created, "gatewayUrl"))
}

/// The `gatewayUrl` `GetGateway` reports — the URL a client reads back, which is the one
/// this suite drives.
async fn published_url(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    gateway_id: &str,
) -> String {
    let (status, fetched) =
        control(router, "GET", &format!("/gateways/{gateway_id}/"), region, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetGateway, got {status} ({fetched})"
    );
    text(&fetched, "gatewayUrl")
}

/// Create an MCP Lambda target declaring `tools` inline, and return its `targetId`.
async fn create_target(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    gateway_id: &str,
    name: &str,
    tools: &Value,
) -> String {
    let body = json!({
        "name": name,
        "targetConfiguration": {
            "mcp": {"lambda": {"lambdaArn": LAMBDA_ARN, "toolSchema": {"inlinePayload": tools}}}
        }
    });
    let (status, created) = control(
        router,
        "POST",
        &format!("/gateways/{gateway_id}/targets/"),
        region,
        Some(&body),
    )
    .await;
    assert_eq!(
        status, 202,
        "case {case:?}: expected HTTP 202 from CreateGatewayTarget, got {status} ({created})"
    );
    text(&created, "targetId")
}

/// Make every clock-owed `CREATING` -> `READY` transition fall due, so the next request
/// materialises `READY` without this test waiting out
/// [`state::GATEWAY_SETTLE_DELAY`] on the wall clock.
///
/// Writes only the due instant the emulator itself stores: the transition is still applied
/// by the next request that reads the store, exactly as in production.
fn settle_now(engine: &Arc<engine::InMemoryEngine>) {
    let store = engine
        .state::<state::ComAmazonawsBedrockagentcorecontrolAmazonbedrockagentcorecontrolState>();
    let mut store = store.lock();
    let past = SystemTime::now() - Duration::from_millis(1);
    for gateway in store.gateways.values_mut() {
        gateway.settles_at = past;
        for target in gateway.targets.values_mut() {
            target.settles_at = past;
        }
    }
}

/// Hold the target named `target_name` in `CREATING` for the rest of the test by moving
/// the transition it is owed an hour out.
fn hold_target_creating(engine: &Arc<engine::InMemoryEngine>, target_name: &str) {
    let store = engine
        .state::<state::ComAmazonawsBedrockagentcorecontrolAmazonbedrockagentcorecontrolState>();
    let mut store = store.lock();
    let held = SystemTime::now() + Duration::from_secs(3600);
    for gateway in store.gateways.values_mut() {
        for target in gateway.targets.values_mut() {
            if target.name == target_name {
                target.settles_at = held;
            }
        }
    }
}

/// Hold every gateway in `CREATING`, for the rows that assert an endpoint serves nothing
/// before the gateway reaches `READY`.
fn hold_gateways_creating(engine: &Arc<engine::InMemoryEngine>) {
    let store = engine
        .state::<state::ComAmazonawsBedrockagentcorecontrolAmazonbedrockagentcorecontrolState>();
    let mut store = store.lock();
    let held = SystemTime::now() + Duration::from_secs(3600);
    for gateway in store.gateways.values_mut() {
        gateway.settles_at = held;
    }
}

/// The tool definitions each row of the `tools/list` table declares, and the tool entries
/// the gateway must then report for them.
///
/// The rows differ along what the rule generalises over: how many tools one target
/// declares, the tool name's shape (including the underscores a `ToolName` may carry while
/// a `TargetName` — `^([0-9a-zA-Z][-]?){1,100}$` — may not), and the `SchemaDefinition`
/// shape (`type` alone, an object with properties and required members, an array with
/// items, a declared `outputSchema`). Every `type` is from `SchemaType`'s value list
/// (`smithy_query shape=SchemaType`: string, number, object, array, boolean, integer).
fn tool_table() -> Vec<(&'static str, &'static str, Value, Value)> {
    vec![
        (
            "one tool with an object schema",
            "writer",
            json!([{
                "name": "put_object",
                "description": "write an object",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "key": {"type": "string", "description": "the key"},
                        "body": {"type": "string"}
                    },
                    "required": ["key", "body"]
                }
            }]),
            json!([{
                "name": "writer___put_object",
                "description": "write an object",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "body": {"type": "string"},
                        "key": {"type": "string", "description": "the key"}
                    },
                    "required": ["key", "body"]
                }
            }]),
        ),
        (
            "three tools, each a different schema type",
            "calc",
            json!([
                {"name": "add", "description": "add numbers", "inputSchema": {"type": "number"}},
                {
                    "name": "flags",
                    "description": "a boolean",
                    "inputSchema": {"type": "boolean", "description": "a flag"}
                },
                {
                    "name": "list_items",
                    "description": "a list",
                    "inputSchema": {"type": "array", "items": {"type": "integer"}}
                }
            ]),
            json!([
                {"name": "calc___add", "description": "add numbers", "inputSchema": {"type": "number"}},
                {
                    "name": "calc___flags",
                    "description": "a boolean",
                    "inputSchema": {"type": "boolean", "description": "a flag"}
                },
                {
                    "name": "calc___list_items",
                    "description": "a list",
                    "inputSchema": {"type": "array", "items": {"type": "integer"}}
                }
            ]),
        ),
        (
            "a tool declaring an outputSchema as well",
            "echo",
            json!([{
                "name": "echo_back",
                "description": "echo a string back",
                "inputSchema": {"type": "string"},
                "outputSchema": {"type": "string", "description": "the same string"}
            }]),
            json!([{
                "name": "echo___echo_back",
                "description": "echo a string back",
                "inputSchema": {"type": "string"},
                "outputSchema": {"type": "string", "description": "the same string"}
            }]),
        ),
    ]
}

/// RULE: the published host serves `tools/list`, aggregating the tools of the gateway's
/// `READY` targets under `<targetName>___<toolName>` with the schema each target declared
/// — in every Region, and with nothing in the answer that the gateway's own record does
/// not hold.
#[tokio::test]
async fn the_published_gateway_url_serves_tools_list_from_its_targets() {
    for region in REGIONS {
        let (router, engine) = router();
        let case = format!("{region} tools/list");
        let (gateway_id, created_url) =
            create_gateway(&router, &case, region, "toolsgateway", "AWS_IAM").await;
        let url = published_url(&router, &case, region, &gateway_id).await;
        assert_eq!(
            url, created_url,
            "case {case:?}: GetGateway must report the URL CreateGateway published, got {url}"
        );
        assert_eq!(
            url,
            format!("https://{gateway_id}.gateway.bedrock-agentcore.{region}.amazonaws.com/mcp"),
            "case {case:?}: expected the gateway's own endpoint — its host and the MCP \
             transport path — got {url}"
        );

        // A gateway takes a target only once the clock-owed transition out of `CREATING`
        // has come due, so the transition is made due before the targets are added.
        settle_now(&engine);

        // Every row's target on this one gateway, so one endpoint aggregates them all.
        let mut expected: Vec<Value> = Vec::new();
        for (row, target_name, tools, reported) in tool_table() {
            let case = format!("{case}: {row}");
            create_target(&router, &case, region, &gateway_id, target_name, &tools).await;
            expected.extend(
                reported
                    .as_array()
                    .expect("the table's expected entries are an array")
                    .iter()
                    .cloned(),
            );
        }
        settle_now(&engine);

        // The published URL is driven EXACTLY as it was published: its transport path is
        // part of the value, so a client holding it appends nothing.
        let endpoint = url.clone();
        let (status, answer) = jsonrpc(
            &router,
            &case,
            &endpoint,
            region,
            &call(1, "tools/list", json!({})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: MCP tools/list to {endpoint} must be served, got {status} ({answer})"
        );
        assert_eq!(
            answer["jsonrpc"], "2.0",
            "case {case:?}: expected a JSON-RPC 2.0 response, got {answer}"
        );
        assert_eq!(
            answer["id"], 1,
            "case {case:?}: a response carries the request's id, got {answer}"
        );
        let listed = answer["result"]["tools"].clone();
        // The gateway holds its targets in a deterministic order, so the whole list is
        // compared rather than a membership test.
        let mut expected_sorted = expected.clone();
        let mut listed_sorted = listed.as_array().cloned().unwrap_or_default();
        expected_sorted.sort_by_key(|tool| text(tool, "name"));
        listed_sorted.sort_by_key(|tool| text(tool, "name"));
        assert_eq!(
            Value::Array(listed_sorted),
            Value::Array(expected_sorted),
            "case {case:?}: expected the tools the targets declare, got {listed}"
        );

        // A read must not change what a later caller sees: the same call answers the same
        // list, and the gateway record is untouched.
        let (_, again) = jsonrpc(
            &router,
            &case,
            &endpoint,
            region,
            &call(2, "tools/list", json!({})),
        )
        .await;
        assert_eq!(
            again["result"], answer["result"],
            "case {case:?}: a second tools/list must answer the same tools, got {}",
            again["result"]
        );
        let (status, fetched) =
            control(&router, "GET", &format!("/gateways/{gateway_id}/"), region, None).await;
        assert_eq!(
            (status, text(&fetched, "status").as_str()),
            (200, "READY"),
            "case {case:?}: driving the MCP endpoint must leave the gateway READY, got {status} \
             ({fetched})"
        );
    }
}

/// RULE: only a `READY` target contributes tools, and a gateway with no serving target
/// lists none — an empty `tools` array, never a tool no target declares.
#[tokio::test]
async fn only_ready_targets_contribute_tools() {
    let region = REGIONS[0];
    let (router, engine) = router();
    let case = "a target still CREATING";
    let (gateway_id, _) = create_gateway(&router, case, region, "mixedtargets", "AWS_IAM").await;
    let url = published_url(&router, case, region, &gateway_id).await;
    let endpoint = url.clone();

    // With no target at all: an empty list.
    settle_now(&engine);
    let (status, answer) = jsonrpc(
        &router,
        case,
        &endpoint,
        region,
        &call(1, "tools/list", json!({})),
    )
    .await;
    assert_eq!(
        (status, answer["result"]["tools"].clone()),
        (200, json!([])),
        "case {case:?}: a gateway with no target lists no tool, got {status} ({answer})"
    );

    let tools = json!([{
        "name": "run",
        "description": "run it",
        "inputSchema": {"type": "object"}
    }]);
    create_target(&router, case, region, &gateway_id, "pending", &tools).await;
    create_target(&router, case, region, &gateway_id, "serving", &tools).await;
    settle_now(&engine);
    hold_target_creating(&engine, "pending");

    let (status, answer) = jsonrpc(
        &router,
        case,
        &endpoint,
        region,
        &call(2, "tools/list", json!({})),
    )
    .await;
    assert_eq!(
        (status, answer["result"]["tools"].clone()),
        (
            200,
            json!([{
                "name": "serving___run",
                "description": "run it",
                "inputSchema": {"type": "object"}
            }])
        ),
        "case {case:?}: only the READY target's tool is aggregated, got {status} ({answer})"
    );

    // ...and once the transition the clock owes the held target falls due, the very same
    // endpoint lists both.
    settle_now(&engine);
    let (_, answer) = jsonrpc(
        &router,
        case,
        &endpoint,
        region,
        &call(3, "tools/list", json!({})),
    )
    .await;
    // The gateway holds its targets by the id it minted for each, so the listing's order
    // is that store's; what the rule fixes is WHICH tools are listed.
    let mut names: Vec<String> = answer["result"]["tools"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|tool| text(tool, "name"))
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec!["pending___run".to_string(), "serving___run".to_string()],
        "case {case:?}: once READY, both targets' tools are listed, got {names:?}"
    );
}

/// RULE: a target whose tools are not DECLARED in the record contributes none — a schema
/// in S3, and every target shape that names no tool at all.
#[tokio::test]
async fn a_target_that_declares_no_tools_contributes_none() {
    let region = REGIONS[0];
    let (router, engine) = router();
    let case = "targets declaring no inline tools";
    let (gateway_id, _) = create_gateway(&router, case, region, "undeclared", "AWS_IAM").await;
    let url = published_url(&router, case, region, &gateway_id).await;
    // A gateway takes a target only once the clock-owed transition out of `CREATING` has
    // come due, so the transition is made due before the targets are added.
    settle_now(&engine);

    // (row, the targetConfiguration) — one per target shape that declares no inline tool
    // schema. Source: `TargetConfiguration` / `McpTargetConfiguration`
    // (`smithy_query shape=TargetConfiguration,McpTargetConfiguration`).
    let rows: Vec<(&str, Value)> = vec![
        (
            "a Lambda target whose tool schema lives in S3",
            json!({"mcp": {"lambda": {
                "lambdaArn": LAMBDA_ARN,
                "toolSchema": {"s3": {"uri": "s3://schemas/tools.json"}}
            }}}),
        ),
        (
            "an MCP target with an inline OpenAPI schema",
            json!({"mcp": {"openApiSchema": {"inlinePayload": "{\"openapi\":\"3.0.0\"}"}}}),
        ),
        (
            "an MCP server endpoint target",
            json!({"mcp": {"mcpServer": {"endpoint": "https://tools.example.com/mcp"}}}),
        ),
        (
            "an MCP target with an inline Smithy model",
            json!({"mcp": {"smithyModel": {"inlinePayload": "{\"smithy\":\"2.0\"}"}}}),
        ),
    ];

    for (index, (row, configuration)) in rows.into_iter().enumerate() {
        let body = json!({"name": format!("target{index}"), "targetConfiguration": configuration});
        let (status, created) = control(
            &router,
            "POST",
            &format!("/gateways/{gateway_id}/targets/"),
            region,
            Some(&body),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {row:?}: expected HTTP 202 from CreateGatewayTarget, got {status} ({created})"
        );
    }
    settle_now(&engine);

    let (status, answer) = jsonrpc(
        &router,
        case,
        &url,
        region,
        &call(1, "tools/list", json!({})),
    )
    .await;
    assert_eq!(
        (status, answer["result"]["tools"].clone()),
        (200, json!([])),
        "case {case:?}: a tool must be listed only where a target declares it, got {status} \
         ({answer})"
    );
}

/// RULE: `tools/call` is translated into an invocation of the named target's function, and
/// what it reports is what that invocation answered. No Lambda is mounted here, so no
/// function ran — and the answer must say so (`isError`), never a result.
#[tokio::test]
async fn a_tool_call_reports_the_invocation_and_never_invents_a_result() {
    let region = REGIONS[0];
    let (router, engine) = router();
    let case = "tools/call with no Lambda to reach";
    let (gateway_id, _) = create_gateway(&router, case, region, "callgateway", "AWS_IAM").await;
    let url = published_url(&router, case, region, &gateway_id).await;
    let endpoint = url.clone();
    let tools = json!([{
        "name": "put_object",
        "description": "write an object",
        "inputSchema": {"type": "object"}
    }]);
    // A gateway takes a target only once the clock-owed transition out of `CREATING` has
    // come due, so the transition is made due before the target is added.
    settle_now(&engine);
    create_target(&router, case, region, &gateway_id, "writer", &tools).await;
    settle_now(&engine);

    let (status, answer) = jsonrpc(
        &router,
        case,
        &endpoint,
        region,
        &call(
            1,
            "tools/call",
            json!({"name": "writer___put_object", "arguments": {"key": "k", "body": "b"}}),
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the JSON-RPC exchange itself succeeds, got {status} ({answer})"
    );
    assert_eq!(
        answer["result"]["isError"], json!(true),
        "case {case:?}: an invocation nothing performed must be reported as a tool error, got {}",
        answer["result"]
    );
    assert!(
        answer["result"]["content"][0]["text"]
            .as_str()
            .is_some_and(|text| !text.is_empty()),
        "case {case:?}: the tool error must carry what the invocation answered, got {}",
        answer["result"]
    );

    // (row, the params sent, the JSON-RPC error code expected) — a call that names no tool
    // this gateway serves is a protocol-level error, not a tool error, so no invocation is
    // attempted at all. -32602 is JSON-RPC 2.0 §5.1's "Invalid params".
    let rows: Vec<(&str, Value)> = vec![
        (
            "a qualified name no target serves",
            json!({"name": "absent___put_object", "arguments": {}}),
        ),
        (
            "a tool name the target does not declare",
            json!({"name": "writer___delete_object", "arguments": {}}),
        ),
        (
            "an unqualified tool name",
            json!({"name": "put_object", "arguments": {}}),
        ),
        ("no tool name at all", json!({})),
        (
            "arguments that are not an object",
            json!({"name": "writer___put_object", "arguments": ["k"]}),
        ),
    ];
    for (index, (row, params)) in rows.into_iter().enumerate() {
        let id = 10 + index as u64;
        let (status, answer) =
            jsonrpc(&router, row, &endpoint, region, &call(id, "tools/call", params)).await;
        assert_eq!(
            (status, answer["error"]["code"].clone()),
            (200, json!(-32602)),
            "case {row:?}: expected the JSON-RPC Invalid params error, got {status} ({answer})"
        );
        assert!(
            answer["result"].is_null(),
            "case {row:?}: a refused call carries no result, got {}",
            answer["result"]
        );
    }
}

/// RULE: the JSON-RPC exchange itself — `initialize`, `ping`, a notification, a method
/// this surface does not serve, and a body that is not a JSON-RPC request.
#[tokio::test]
async fn the_endpoint_serves_the_json_rpc_exchange() {
    let region = REGIONS[0];
    let (router, engine) = router();
    let case = "the JSON-RPC exchange";
    let (gateway_id, _) = create_gateway(&router, case, region, "rpcgateway", "AWS_IAM").await;
    let url = published_url(&router, case, region, &gateway_id).await;
    let endpoint = url.clone();
    settle_now(&engine);

    let (status, answer) = jsonrpc(
        &router,
        case,
        &endpoint,
        region,
        &call(
            1,
            "initialize",
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "test-client", "version": "0.1.0"}
            }),
        ),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: initialize must be served, got {status} ({answer})"
    );
    assert_eq!(
        answer["result"]["protocolVersion"], MCP_PROTOCOL_VERSION,
        "case {case:?}: expected the protocol revision this surface speaks, got {}",
        answer["result"]
    );
    assert!(
        answer["result"]["capabilities"]
            .get("tools")
            .is_some(),
        "case {case:?}: a gateway's MCP server has the tools capability, got {}",
        answer["result"]
    );
    assert_eq!(
        answer["result"]["serverInfo"]["name"], "rpcgateway",
        "case {case:?}: serverInfo names the gateway itself, got {}",
        answer["result"]
    );

    let (status, answer) = jsonrpc(&router, case, &endpoint, region, &call(2, "ping", json!({}))).await;
    assert_eq!(
        (status, answer["result"].clone()),
        (200, json!({})),
        "case {case:?}: ping answers an empty result, got {status} ({answer})"
    );

    // A notification carries no id, so it gets no response at all.
    let (status, answer) = jsonrpc(
        &router,
        case,
        &endpoint,
        region,
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    )
    .await;
    assert_eq!(
        (status, answer.clone()),
        (202, Value::Null),
        "case {case:?}: a notification is accepted with no body, got {status} ({answer})"
    );

    // (row, the body sent, expected status, expected JSON-RPC error code) — the rows differ
    // along WHY the message is not an exchange this surface can serve. Codes are JSON-RPC
    // 2.0 §5.1's reserved ones.
    let rows: Vec<(&str, String, u16, i64)> = vec![
        (
            "a method this surface does not serve",
            call(3, "resources/list", json!({})).to_string(),
            200,
            -32601,
        ),
        (
            "a notification method asked for as a request",
            call(4, "notifications/initialized", json!({})).to_string(),
            200,
            -32601,
        ),
        ("a body that is not JSON", "not json at all".to_string(), 400, -32700),
        (
            "a JSON body that is not an object",
            json!([call(5, "ping", json!({}))]).to_string(),
            400,
            -32600,
        ),
        (
            "an object that declares no jsonrpc version",
            json!({"id": 6, "method": "ping"}).to_string(),
            400,
            -32600,
        ),
        (
            "an object that names no method",
            json!({"jsonrpc": "2.0", "id": 7}).to_string(),
            400,
            -32600,
        ),
    ];
    for (row, body, expected_status, expected_code) in rows {
        let (status, answer) = mcp(&router, row, "POST", &endpoint, Some(region), Some(&body)).await;
        assert_eq!(
            (status, answer["error"]["code"].clone()),
            (expected_status, json!(expected_code)),
            "case {row:?}: expected HTTP {expected_status} and JSON-RPC {expected_code}, got \
             {status} ({answer})"
        );
    }
}

/// RULE: the gateway's inbound authorizer decides whether a request is served at all, for
/// every `authorizerType` the model declares.
///
/// The rows are the whole `AuthorizerType` value list
/// (`shared_types::AUTHORIZER_TYPE_VALUES`, from `smithy_query shape=AuthorizerType`),
/// each driven signed and unsigned, so what decides is the gateway's own configuration and
/// the credential the request carries.
#[tokio::test]
async fn the_inbound_authorizer_decides_whether_the_endpoint_answers() {
    let region = REGIONS[0];

    // (authorizerType, status signed, status unsigned)
    let rows: [(&str, u16, u16); 4] = [
        // "AWS_IAM - Authorize with your Amazon Web Services IAM credentials": served for
        // a request that presents one, refused for a request that presents none.
        ("AWS_IAM", 200, 403),
        // "NONE - No authorization": served either way.
        ("NONE", 200, 200),
        // A bearer-token authorizer this emulator verifies no token of: 501 rather than an
        // answer that would report an authorization nothing performed.
        ("CUSTOM_JWT", 501, 501),
        ("AUTHENTICATE_ONLY", 501, 501),
    ];

    for (index, (authorizer_type, signed_status, unsigned_status)) in rows.into_iter().enumerate() {
        let (router, engine) = router();
        let case = format!("authorizerType {authorizer_type}");
        let (gateway_id, _) = create_gateway(
            &router,
            &case,
            region,
            &format!("authgw{index}"),
            authorizer_type,
        )
        .await;
        let url = published_url(&router, &case, region, &gateway_id).await;
        let endpoint = url.clone();
        settle_now(&engine);
        let message = call(1, "tools/list", json!({})).to_string();

        let (status, answer) = mcp(
            &router,
            &case,
            "POST",
            &endpoint,
            Some(region),
            Some(&message),
        )
        .await;
        assert_eq!(
            status, signed_status,
            "case {case:?} (signed): expected HTTP {signed_status}, got {status} ({answer})"
        );

        let (status, answer) =
            mcp(&router, &case, "POST", &endpoint, None, Some(&message)).await;
        assert_eq!(
            status, unsigned_status,
            "case {case:?} (unsigned): expected HTTP {unsigned_status}, got {status} ({answer})"
        );
        if unsigned_status != 200 {
            let expected_code = if unsigned_status == 403 {
                "AccessDeniedException"
            } else {
                "NotImplemented"
            };
            assert_eq!(
                error_code(&answer),
                expected_code,
                "case {case:?} (unsigned): expected {expected_code}, got {answer}"
            );
            assert!(
                answer["result"].is_null() && answer["error"].is_null(),
                "case {case:?} (unsigned): a refused request carries no JSON-RPC answer, got \
                 {answer}"
            );
        }
    }
}

/// RULE: nothing is served where no `READY` gateway endpoint of this account backs the
/// address — an unknown gateway id, another Region's host, a path the gateway publishes
/// nothing at, a method it is not addressed with, a gateway still `CREATING`, and a
/// gateway that has been deleted.
#[tokio::test]
async fn nothing_is_served_without_a_ready_gateway_behind_the_address() {
    let region = REGIONS[0];
    let other_region = REGIONS[1];
    let (router, engine) = router();
    let case = "addresses no gateway serves";
    let (gateway_id, _) = create_gateway(&router, case, region, "servinggw", "AWS_IAM").await;
    let (elsewhere_id, _) =
        create_gateway(&router, case, other_region, "elsewheregw", "AWS_IAM").await;
    let url = published_url(&router, case, region, &gateway_id).await;
    settle_now(&engine);
    let host = format!("{gateway_id}.gateway.bedrock-agentcore.{region}.amazonaws.com");
    let message = call(1, "tools/list", json!({})).to_string();

    // (row, the URL driven, the method, the expected status)
    let rows: Vec<(&str, String, &str, u16)> = vec![
        (
            "a gateway id no record answers to",
            format!("https://no-such-gateway-0123456789.gateway.bedrock-agentcore.{region}.amazonaws.com/mcp"),
            "POST",
            404,
        ),
        (
            "another Region's gateway on this Region's host form",
            format!("https://{elsewhere_id}.gateway.bedrock-agentcore.{region}.amazonaws.com/mcp"),
            "POST",
            404,
        ),
        (
            "this gateway through another Region's host form",
            format!("https://{gateway_id}.gateway.bedrock-agentcore.{other_region}.amazonaws.com/mcp"),
            "POST",
            404,
        ),
        ("a path the gateway publishes nothing at", format!("https://{host}/tools"), "POST", 404),
        ("the bare host the URL publishes", format!("https://{host}/"), "POST", 404),
        ("a deeper path under the MCP endpoint", format!("https://{host}/mcp/messages"), "POST", 404),
        ("the MCP endpoint reached with GET", url.clone(), "GET", 405),
        ("the MCP endpoint reached with DELETE", url.clone(), "DELETE", 405),
    ];
    for (row, address, method, expected) in rows {
        let (status, answer) = mcp(
            &router,
            row,
            method,
            &address,
            Some(region),
            Some(&message),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {row:?}: expected HTTP {expected} for {address}, got {status} ({answer})"
        );
        assert!(
            answer["result"].is_null(),
            "case {row:?}: nothing is served there, got {answer}"
        );
    }

    // The same URL while its gateway is still CREATING: the endpoint is reachable only
    // from READY onwards.
    hold_gateways_creating(&engine);
    let (status, answer) = mcp(
        &router,
        "a gateway still CREATING",
        "POST",
        &url,
        Some(region),
        Some(&message),
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"a gateway still CREATING\": {url} serves nothing yet, got {status} ({answer})"
    );

    // ...and once the clock-owed transition falls due, the very same URL serves.
    settle_now(&engine);
    let (status, answer) = mcp(
        &router,
        "a gateway that reached READY",
        "POST",
        &url,
        Some(region),
        Some(&message),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"a gateway that reached READY\": {url} must serve, got {status} ({answer})"
    );

    // A deleted gateway serves nothing: the removal the clock owes is materialised on the
    // next request, and the endpoint goes with the record.
    let (status, deleted) =
        control(&router, "DELETE", &format!("/gateways/{gateway_id}/"), region, None).await;
    assert_eq!(
        status, 202,
        "case \"a deleted gateway\": expected HTTP 202 from DeleteGateway, got {status} \
         ({deleted})"
    );
    expire_deletions_now(&engine);
    let (status, answer) = mcp(
        &router,
        "a deleted gateway",
        "POST",
        &url,
        Some(region),
        Some(&message),
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"a deleted gateway\": {url} must serve nothing, got {status} ({answer})"
    );
}

/// Make the removal every deleted gateway is still owed fall due, the same way
/// [`settle_now`] does for a status transition.
fn expire_deletions_now(engine: &Arc<engine::InMemoryEngine>) {
    let store = engine
        .state::<state::ComAmazonawsBedrockagentcorecontrolAmazonbedrockagentcorecontrolState>();
    let mut store = store.lock();
    let past = SystemTime::now() - Duration::from_millis(1);
    for gateway in store.gateways.values_mut() {
        if gateway.removal_due_at.is_some() {
            gateway.removal_due_at = Some(past);
        }
    }
}

/// RULE: the MCP surface is published on the gateway's OWN host and nowhere else — the
/// control-plane endpoint does not serve it, so the one address a client is handed is the
/// one that answers.
#[tokio::test]
async fn the_mcp_surface_is_not_served_on_the_control_plane_host() {
    let region = REGIONS[0];
    let (router, engine) = router();
    let case = "the MCP path on the control-plane host";
    let (gateway_id, _) = create_gateway(&router, case, region, "hostcheck", "AWS_IAM").await;
    settle_now(&engine);
    let _ = gateway_id;

    let (status, answer) = control(
        &router,
        "POST",
        "/mcp",
        region,
        Some(&call(1, "tools/list", json!({}))),
    )
    .await;
    assert_ne!(
        status, 200,
        "case {case:?}: the MCP surface is published on the gateway host only, but the \
         control-plane endpoint answered {status} ({answer})"
    );
}
