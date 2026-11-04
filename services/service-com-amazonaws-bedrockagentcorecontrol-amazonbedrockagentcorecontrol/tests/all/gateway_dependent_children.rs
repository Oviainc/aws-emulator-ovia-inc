// SPDX-License-Identifier: CC0-1.0

//! The dependent-children precondition on `DeleteGateway`, end to end over HTTP.
//!
//! Each test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handlers are all exercised against the one shared engine.
//!
//! The rule under test, read off the Smithy model rather than off any one recorded request
//! (`smithy_query(query="operation",operation="DeleteGateway,CreateGatewayTarget,\
//! DeleteGatewayTarget,ListGatewayTargets")`): a gateway target is addressed only THROUGH
//! the gateway that serves it (`/gateways/{gatewayIdentifier}/targets/{targetId}/`), so a
//! gateway that still has a target attached is not destroyed — `DeleteGateway` is refused
//! the modelled `ValidationException` (HTTP 400) naming the gateway and the children to
//! delete first, and the refused call writes nothing: the gateway and every target on it
//! keep the status they had. Once the last target's own deletion has been asked for, the
//! identical `DeleteGateway` call is accepted (HTTP 202, `DELETING`) and the clock removes
//! the record.
//!
//! The rows differ along how many targets the gateway serves, the shape of the
//! `targetConfiguration` union each carries, and whether the targets were waited out to
//! `READY` first.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region the rows sign for.
const REGION: &str = "us-east-1";

/// A role ARN inside `RoleArn`'s pattern `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/GatewayRole";

/// The settle span `src/state.rs` gives a gateway's and a target's clock-owed transitions,
/// plus slack, so a row can wait the clock out.
const PAST_SETTLE: Duration = Duration::from_millis(900);

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service, which is what the router routes on.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/bedrock-agentcore/\
         aws4_request, SignedHeaders=host, Signature=abc"
    )
}

/// One answer: the HTTP status and the parsed body.
struct Answer {
    status: u16,
    body: Value,
}

async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> Answer {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization())
        .header("host", "bedrock-agentcore-control.us-east-1.amazonaws.com");
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    Answer {
        status: response.status().as_u16(),
        body: if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        },
    }
}

/// The error code `restJson1` puts in the body's `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

async fn delete_gateway(router: &MultiServiceRouter, gateway: &str) -> Answer {
    send(router, "DELETE", &format!("/gateways/{gateway}/"), None).await
}

async fn get_gateway(router: &MultiServiceRouter, gateway: &str) -> Answer {
    send(router, "GET", &format!("/gateways/{gateway}/"), None).await
}

async fn create_target(router: &MultiServiceRouter, gateway: &str, body: &Value) -> Answer {
    send(
        router,
        "POST",
        &format!("/gateways/{gateway}/targets/"),
        Some(body),
    )
    .await
}

async fn get_target(router: &MultiServiceRouter, gateway: &str, target: &str) -> Answer {
    send(
        router,
        "GET",
        &format!("/gateways/{gateway}/targets/{target}/"),
        None,
    )
    .await
}

async fn delete_target(router: &MultiServiceRouter, gateway: &str, target: &str) -> Answer {
    send(
        router,
        "DELETE",
        &format!("/gateways/{gateway}/targets/{target}/"),
        None,
    )
    .await
}

/// The `targetId`s `ListGatewayTargets` reports for a gateway, in the order it pages them.
async fn listed_target_ids(router: &MultiServiceRouter, gateway: &str) -> Vec<String> {
    let listed = send(router, "GET", &format!("/gateways/{gateway}/targets/"), None).await;
    assert_eq!(
        listed.status, 200,
        "the target listing must answer, got {} ({})",
        listed.status, listed.body
    );
    listed
        .body
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| text(item, "targetId"))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

/// Create a gateway and hand back its `gatewayId`, waited out to `READY` — a gateway takes
/// a target only from the terminal status it settles into.
async fn settled_gateway(router: &MultiServiceRouter, name: &str) -> String {
    let body = json!({
        "name": name,
        "roleArn": ROLE_ARN,
        "authorizerType": "AWS_IAM",
    });
    let created = send(router, "POST", "/gateways/", Some(&body)).await;
    assert_eq!(
        created.status, 202,
        "setup {name:?}: expected HTTP 202 from CreateGateway, got {} ({})",
        created.status, created.body
    );
    let gateway_id = text(&created.body, "gatewayId");
    tokio::time::sleep(PAST_SETTLE).await;
    let settled = get_gateway(router, &gateway_id).await;
    assert_eq!(
        (settled.status, text(&settled.body, "status").as_str()),
        (200, "READY"),
        "setup {name:?}: the gateway must settle to READY, got {} {} ({})",
        settled.status,
        text(&settled.body, "status"),
        settled.body
    );
    gateway_id
}

/// A `CreateGatewayTarget` body for `name` carrying `configuration`.
///
/// `credentialProviderConfigurations` is optional on the request and `@required` on every
/// response, so the gateway's own IAM role is what these rows name
/// (`smithy_query(query="shape",shape="CredentialProviderType")`).
fn create_body(name: &str, configuration: Value) -> Value {
    json!({
        "name": name,
        "targetConfiguration": configuration,
        "credentialProviderConfigurations": [{"credentialProviderType": "GATEWAY_IAM_ROLE"}],
    })
}

/// A target configuration union: an MCP target serving an inline OpenAPI schema.
fn openapi_configuration(payload: &str) -> Value {
    json!({"mcp": {"openApiSchema": {"inlinePayload": payload}}})
}

/// The one sentence this service refuses a gateway's deletion with while a target is still
/// attached to it. The fixed text is the service's; the only variable part is the gateway
/// id the request's path carried.
fn has_targets_message(gateway_id: &str) -> String {
    format!(
        "Gateway with ID: {gateway_id} has targets associated with it. Delete all targets \
         before deleting the gateway."
    )
}

/// One row of the precondition table.
struct Row {
    case: &'static str,
    /// The gateway name the row creates (`GatewayName` is `^[0-9a-zA-Z-]+$`).
    name: &'static str,
    /// The `targetConfiguration` of each target the gateway serves, in creation order.
    configurations: Vec<Value>,
    /// Whether the targets are waited out to `READY` before the gateway's delete is tried.
    settled_first: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one target, an inline OpenAPI schema, settled to READY",
            name: "childgateone",
            configurations: vec![openapi_configuration("{\"openapi\":\"3.0.0\"}")],
            settled_first: true,
        },
        Row {
            case: "one target, an HTTP passthrough endpoint",
            name: "childgatehttp",
            configurations: vec![json!({
                "http": {
                    "passthrough": {
                        "endpoint": "https://passthrough.example.com/mcp",
                        "protocolType": "MCP",
                    }
                }
            })],
            settled_first: true,
        },
        Row {
            case: "three targets of three different configuration shapes",
            name: "childgatethree",
            configurations: vec![
                openapi_configuration("{\"openapi\":\"3.1.0\"}"),
                json!({
                    "mcp": {
                        "mcpServer": {
                            "endpoint": "https://tools.example.com/mcp",
                            "listingMode": "DYNAMIC",
                            "resourcePriority": 7,
                        }
                    }
                }),
                json!({
                    "http": {
                        "passthrough": {
                            "endpoint": "https://other.example.com/mcp",
                            "protocolType": "CUSTOM",
                        }
                    }
                }),
            ],
            settled_first: true,
        },
        Row {
            case: "two targets the clock has not finished creating yet",
            name: "childgatepending",
            configurations: vec![
                openapi_configuration("{\"openapi\":\"3.0.1\"}"),
                openapi_configuration("{\"openapi\":\"3.0.2\"}"),
            ],
            settled_first: false,
        },
    ]
}

/// A gateway that still has a target attached refuses its own deletion in the service's one
/// sentence, writes nothing — and serves the identical call once every target's deletion has
/// been asked for.
#[tokio::test]
async fn a_gateway_with_targets_attached_refuses_its_own_deletion() {
    for row in rows() {
        let Row {
            case,
            name,
            configurations,
            settled_first,
        } = row;
        let router = router();
        let gateway_id = settled_gateway(&router, name).await;

        let mut target_ids = Vec::new();
        for (index, configuration) in configurations.iter().enumerate() {
            let body = create_body(&format!("{name}target{index}"), configuration.clone());
            let created = create_target(&router, &gateway_id, &body).await;
            assert_eq!(
                created.status, 202,
                "case {case:?}: target {index} must be created on the READY gateway, got {} ({})",
                created.status, created.body
            );
            target_ids.push(text(&created.body, "targetId"));
        }
        if settled_first {
            tokio::time::sleep(PAST_SETTLE).await;
        }

        // The whole point: while a target is attached the gateway's own delete is the
        // modelled 400, carrying the service's sentence with THIS gateway's id in it.
        let refused = delete_gateway(&router, &gateway_id).await;
        assert_eq!(
            (refused.status, error_code(&refused.body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {} {} ({})",
            refused.status,
            error_code(&refused.body),
            refused.body
        );
        assert_eq!(
            text(&refused.body, "message"),
            has_targets_message(&gateway_id),
            "case {case:?}: the refusal names the gateway and the children to delete first, \
             got {}",
            text(&refused.body, "message")
        );
        assert!(
            refused.body.get("gatewayId").is_none() && refused.body.get("status").is_none(),
            "case {case:?}: a refused delete answers no success envelope, got {}",
            refused.body
        );

        // Nothing moved: the gateway is still READY and every target still reads back, in
        // the order the listing pages them.
        let read = get_gateway(&router, &gateway_id).await;
        assert_eq!(
            (read.status, text(&read.body, "status").as_str()),
            (200, "READY"),
            "case {case:?}: the refused delete leaves the gateway READY, got {} {} ({})",
            read.status,
            text(&read.body, "status"),
            read.body
        );
        let listed = listed_target_ids(&router, &gateway_id).await;
        let mut expected = target_ids.clone();
        expected.sort();
        assert_eq!(
            listed, expected,
            "case {case:?}: the refused delete removes no target, got {listed:?}"
        );
        for target in &target_ids {
            let fetched = get_target(&router, &gateway_id, target).await;
            assert_eq!(
                fetched.status, 200,
                "case {case:?}: target {target} still reads back after the refusal, got {} ({})",
                fetched.status, fetched.body
            );
            assert!(
                text(&fetched.body, "status") != "DELETING",
                "case {case:?}: the refused gateway delete starts no target deletion, got {}",
                fetched.body
            );
        }

        // The children go one at a time: the gateway is refused until the LAST of them has
        // been asked to go, and then the identical call is accepted.
        for (index, target) in target_ids.iter().enumerate() {
            let dropped = delete_target(&router, &gateway_id, target).await;
            assert_eq!(
                (dropped.status, text(&dropped.body, "status").as_str()),
                (202, "DELETING"),
                "case {case:?}: DeleteGatewayTarget on {target} must be accepted, got {} {} ({})",
                dropped.status,
                text(&dropped.body, "status"),
                dropped.body
            );
            if index + 1 < target_ids.len() {
                let still = delete_gateway(&router, &gateway_id).await;
                assert_eq!(
                    (still.status, error_code(&still.body).as_str()),
                    (400, "ValidationException"),
                    "case {case:?}: {} target(s) are still attached, so the gateway's delete is \
                     still refused, got {} {} ({})",
                    target_ids.len() - index - 1,
                    still.status,
                    error_code(&still.body),
                    still.body
                );
            }
        }

        let accepted = delete_gateway(&router, &gateway_id).await;
        assert_eq!(
            (accepted.status, text(&accepted.body, "status").as_str()),
            (202, "DELETING"),
            "case {case:?}: with no target attached the identical call is accepted, got {} {} \
             ({})",
            accepted.status,
            text(&accepted.body, "status"),
            accepted.body
        );
        assert_eq!(
            text(&accepted.body, "gatewayId"),
            gateway_id,
            "case {case:?}: the accepted delete reports the gateway it is deleting, got {}",
            text(&accepted.body, "gatewayId")
        );

        // And the clock carries it out, so a caller polling for the gateway to disappear
        // finishes.
        tokio::time::sleep(PAST_SETTLE).await;
        let gone = get_gateway(&router, &gateway_id).await;
        assert_eq!(
            (gone.status, error_code(&gone.body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: once the span passes the gateway is gone, got {} {} ({})",
            gone.status,
            error_code(&gone.body),
            gone.body
        );
    }
}

/// The precondition is each gateway's OWN children: a gateway with no target is deleted
/// while another gateway of the same account, in the same store, is refused for its target —
/// and the refusal quotes the gateway the request named.
#[tokio::test]
async fn the_precondition_is_the_gateways_own_children() {
    let router = router();
    let with_target = settled_gateway(&router, "ownchildrenheld").await;
    let bare = settled_gateway(&router, "ownchildrenbare").await;
    let created = create_target(
        &router,
        &with_target,
        &create_body(
            "ownchildrentarget",
            openapi_configuration("{\"openapi\":\"3.0.0\"}"),
        ),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "setup: the target must be created, got {} ({})",
        created.status, created.body
    );

    let accepted = delete_gateway(&router, &bare).await;
    assert_eq!(
        (accepted.status, text(&accepted.body, "status").as_str()),
        (202, "DELETING"),
        "a gateway with no target of its own is deleted, got {} {} ({})",
        accepted.status,
        text(&accepted.body, "status"),
        accepted.body
    );

    let refused = delete_gateway(&router, &with_target).await;
    assert_eq!(
        (refused.status, error_code(&refused.body).as_str()),
        (400, "ValidationException"),
        "the gateway holding the target is refused, got {} {} ({})",
        refused.status,
        error_code(&refused.body),
        refused.body
    );
    assert_eq!(
        text(&refused.body, "message"),
        has_targets_message(&with_target),
        "the refusal quotes the gateway the request named, got {}",
        text(&refused.body, "message")
    );
}
