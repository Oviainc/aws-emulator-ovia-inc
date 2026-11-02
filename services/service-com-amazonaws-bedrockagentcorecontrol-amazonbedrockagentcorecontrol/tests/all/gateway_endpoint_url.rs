// SPDX-License-Identifier: CC0-1.0

//! The endpoint a gateway publishes in `gatewayUrl`.
//!
//! RULE: the `gatewayUrl` a gateway reports is its whole endpoint — the host AWS assigned
//! it FOLLOWED by the transport path the protocol in its `protocolType` is served on — and
//! it is the same value for the create, the update and every read of that gateway. The
//! path is part of the value a client is handed, not a suffix the client is owed:
//! `GatewayProtocolType` carries the single value `MCP`, which `protocolType` also carries
//! as its modelled default (`smithy_query(query="operation",operation="CreateGateway")`),
//! and the recorded endpoint of such a gateway ends in the MCP transport path `/mcp`.
//!
//! The rows span what the rule generalises over: the Region (which the host carries), the
//! shape of the name the gateway id is derived from, whether `protocolType` was sent or
//! left to its default, and the authorizer the gateway was created with.
//!
//! The second test pins the other half of the same rule — the published URL is the URL
//! this emulator SERVES — by driving the value verbatim, appending nothing, and showing
//! that the origin alone addresses nothing.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::{
    state, EmulatedService,
};

// The `(host, path)` splitter the MCP endpoint suite already states for a URL this service
// published; it reports the root path for a URL that carries none.
use crate::gateway_mcp_endpoint::split_url;

/// A role ARN inside `RoleArn`'s pattern `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/GatewayRole";

/// The transport path an MCP gateway's endpoint carries, as the recorded gateway URL of a
/// gateway whose `protocolType` is `MCP` ends in.
const MCP_TRANSPORT_PATH: &str = "/mcp";

/// The MCP protocol revision an MCP client declares in `MCP-Protocol-Version`.
const MCP_PROTOCOL_VERSION: &str = "2025-03-26";

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
        .header(
            "host",
            format!("bedrock-agentcore-control.{region}.amazonaws.com"),
        );
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("a well-formed request");
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

/// Drive `url` the way a client holding it does: its own host in the `Host` header, its own
/// path as the request target, and nothing appended to either.
async fn drive(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    region: &str,
    message: &Value,
) -> (u16, Value) {
    let (host, path) = split_url(case, url);
    let request = http::Request::builder()
        .method("POST")
        .uri(path)
        .header("host", host)
        .header("authorization", authorization(region))
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", MCP_PROTOCOL_VERSION)
        .body(Bytes::from(message.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, body_json(response.body()))
}

/// A `CreateGateway` body, with `protocolType` sent only where the row names one.
fn create_body(name: &str, protocol_type: Option<&str>, authorizer_type: &str) -> Value {
    let mut body = json!({
        "name": name,
        "roleArn": ROLE_ARN,
        "authorizerType": authorizer_type,
    });
    if let Some(protocol_type) = protocol_type {
        body["protocolType"] = json!(protocol_type);
    }
    body
}

/// Make every clock-owed `CREATING` -> `READY` transition fall due, so the next request
/// materialises `READY` without waiting the span out on the wall clock. Writes only the
/// due instant the emulator itself stores.
fn settle_now(engine: &Arc<engine::InMemoryEngine>) {
    let store = engine
        .state::<state::ComAmazonawsBedrockagentcorecontrolAmazonbedrockagentcorecontrolState>();
    let mut store = store.lock();
    let past = SystemTime::now() - Duration::from_millis(1);
    for gateway in store.gateways.values_mut() {
        gateway.settles_at = past;
    }
}

/// RULE: `gatewayUrl` is the gateway's host plus its protocol's transport path, reported
/// identically by `CreateGateway`, `UpdateGateway` and `GetGateway`.
#[tokio::test]
async fn a_gateway_publishes_its_host_and_its_protocol_transport_path() {
    // (case, name, region, protocolType sent, authorizerType)
    let rows: [(&str, &str, &str, Option<&str>, &str); 6] = [
        (
            "protocolType left to its default, us-west-2",
            "defaultproto",
            "us-west-2",
            None,
            "AWS_IAM",
        ),
        (
            "protocolType sent explicitly, us-east-1",
            "explicitproto",
            "us-east-1",
            Some("MCP"),
            "AWS_IAM",
        ),
        (
            "a mixed-case name, eu-west-1",
            "MixedCaseGw",
            "eu-west-1",
            Some("MCP"),
            "NONE",
        ),
        (
            "dashes between characters, ap-southeast-2",
            "my-mcp-gw",
            "ap-southeast-2",
            None,
            "NONE",
        ),
        (
            "a single-character name, us-gov-west-1",
            "g",
            "us-gov-west-1",
            Some("MCP"),
            "AWS_IAM",
        ),
        (
            "a name ending in a dash, eu-central-1",
            "trailing-dash-",
            "eu-central-1",
            None,
            "AWS_IAM",
        ),
    ];

    for (case, name, region, protocol_type, authorizer_type) in rows {
        let (router, engine) = router();
        let (status, created) = control(
            &router,
            "POST",
            "/gateways/",
            region,
            Some(&create_body(name, protocol_type, authorizer_type)),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from CreateGateway, got {status} ({created})"
        );
        let id = text(&created, "gatewayId");
        let expected = format!(
            "https://{id}.gateway.bedrock-agentcore.{region}.amazonaws.com{MCP_TRANSPORT_PATH}"
        );
        let url = text(&created, "gatewayUrl");
        assert_eq!(
            url, expected,
            "case {case:?}: CreateGateway must publish {expected:?}, got {url:?}"
        );
        let (_, path) = split_url(case, &url);
        assert_eq!(
            path, MCP_TRANSPORT_PATH,
            "case {case:?}: the published endpoint carries the transport path of the protocol \
             the gateway reports ({}), got {path:?}",
            text(&created, "protocolType")
        );

        let (status, fetched) =
            control(&router, "GET", &format!("/gateways/{id}/"), region, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetGateway, got {status} ({fetched})"
        );
        assert_eq!(
            text(&fetched, "gatewayUrl"),
            url,
            "case {case:?}: GetGateway must report the published endpoint {url:?}, got {:?}",
            text(&fetched, "gatewayUrl")
        );

        // An update reconfigures the gateway; it does not move its endpoint.
        settle_now(&engine);
        let (status, updated) = control(
            &router,
            "PUT",
            &format!("/gateways/{id}/"),
            region,
            Some(&create_body(name, protocol_type, authorizer_type)),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from UpdateGateway, got {status} ({updated})"
        );
        assert_eq!(
            text(&updated, "gatewayUrl"),
            url,
            "case {case:?}: UpdateGateway must report the published endpoint {url:?}, got {:?}",
            text(&updated, "gatewayUrl")
        );

        let (status, reread) =
            control(&router, "GET", &format!("/gateways/{id}/"), region, None).await;
        assert_eq!(
            (status, text(&reread, "gatewayUrl")),
            (200, url.clone()),
            "case {case:?}: the read after an update must report {url:?}, got {status} ({reread})"
        );
    }
}

/// RULE (the other half): the published value is the address this emulator serves the
/// gateway's protocol on — driven verbatim, with nothing appended — while the origin alone,
/// and a deeper path under it, address nothing.
#[tokio::test]
async fn the_published_endpoint_is_the_address_the_gateway_is_served_on() {
    // The Regions span the host forms a gateway endpoint takes, including a Region name of
    // three hyphen-separated parts.
    let regions = ["us-west-2", "eu-west-1", "ap-southeast-2", "us-gov-west-1"];
    let message = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}});

    for region in regions {
        let case = format!("{region}: the published endpoint");
        let (router, engine) = router();
        let (status, created) = control(
            &router,
            "POST",
            "/gateways/",
            region,
            Some(&create_body("servedgw", None, "AWS_IAM")),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from CreateGateway, got {status} ({created})"
        );
        let url = text(&created, "gatewayUrl");
        settle_now(&engine);

        let (status, answer) = drive(&router, &case, &url, region, &message).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the URL the gateway published must serve its protocol as \
             published ({url}), got {status} ({answer})"
        );
        assert!(
            answer["result"]["tools"].is_array(),
            "case {case:?}: the answer is the gateway's own tool listing, got {answer}"
        );

        // The path is part of the published address: the origin on its own is not the
        // endpoint, and neither is a deeper path under it.
        let (host, _) = split_url(&case, &url);
        for (row, address) in [
            ("the origin alone", format!("https://{host}")),
            ("the origin's root path", format!("https://{host}/")),
            (
                "a deeper path under the endpoint",
                format!("{url}/messages"),
            ),
        ] {
            let row = format!("{case} / {row}");
            let (status, answer) = drive(&router, &row, &address, region, &message).await;
            assert_eq!(
                status, 404,
                "case {row:?}: {address} addresses no endpoint of this gateway, got {status} \
                 ({answer})"
            );
            assert!(
                answer["result"].is_null(),
                "case {row:?}: nothing is served there, got {answer}"
            );
        }
    }
}
