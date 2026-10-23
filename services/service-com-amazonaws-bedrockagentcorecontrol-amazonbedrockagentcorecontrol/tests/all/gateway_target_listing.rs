// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListGatewayTargets`.
//!
//! Every test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handlers are all exercised. The store is the one shared
//! engine, so what `CreateGatewayTarget` writes is what the listing reads.
//!
//! The rule under test: a target listing reports every target persisted on the named
//! gateway, each summarised from that target's own record — so the listing cannot
//! disagree with `GetGatewayTarget` — in one deterministic order, cut into pages of at
//! most `maxResults` entries that resume exactly where the previous page ended.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region the tests sign for.
const REGION: &str = "us-east-1";

/// The settle span `src/state.rs` gives a target's `CREATING` -> `READY` transition, plus
/// slack, so a test can wait the clock out.
const PAST_SETTLE: Duration = Duration::from_millis(900);

/// A role ARN inside `RoleArn`'s pattern `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/GatewayRole";

/// The modelled bounds of `maxResults` (`TargetMaxResults`, range 1..1000, from
/// `smithy_query operation=ListGatewayTargets`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 1000;

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service, which is what the router routes on.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/bedrock-agentcore/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization())
        .header("host", "bedrock-agentcore-control.amazonaws.com");
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

async fn create_gateway(router: &MultiServiceRouter, name: &str) -> String {
    let body = json!({
        "name": name,
        "roleArn": ROLE_ARN,
        "authorizerType": "AWS_IAM",
    });
    let (status, created) = send(router, "POST", "/gateways/", Some(&body)).await;
    assert_eq!(
        status, 202,
        "case {name:?}: expected HTTP 202 from CreateGateway, got {status} ({created})"
    );
    created["gatewayId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

async fn create_target(router: &MultiServiceRouter, gateway: &str, body: &Value) -> (u16, Value) {
    send(
        router,
        "POST",
        &format!("/gateways/{gateway}/targets/"),
        Some(body),
    )
    .await
}

async fn get_target(router: &MultiServiceRouter, gateway: &str, target: &str) -> (u16, Value) {
    send(
        router,
        "GET",
        &format!("/gateways/{gateway}/targets/{target}/"),
        None,
    )
    .await
}

/// `ListGatewayTargets` with an optional page size and resume token.
async fn list_targets(
    router: &MultiServiceRouter,
    gateway: &str,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (u16, Value) {
    let mut query = Vec::new();
    if let Some(max) = max_results {
        query.push(format!("maxResults={max}"));
    }
    if let Some(token) = next_token {
        query.push(format!("nextToken={token}"));
    }
    let uri = if query.is_empty() {
        format!("/gateways/{gateway}/targets/")
    } else {
        format!("/gateways/{gateway}/targets/?{}", query.join("&"))
    };
    send(router, "GET", &uri, None).await
}

/// The error code `restJson1` puts in `__type` / `x-amzn-errortype`.
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

/// The `items` of a listing response as a vector.
fn items(body: &Value) -> Vec<Value> {
    body["items"]
        .as_array()
        .cloned()
        .unwrap_or_else(Vec::new)
}

/// A `CreateGatewayTarget` body for `name` carrying `configuration`.
fn target_body(name: &str, configuration: Value) -> Value {
    json!({
        "name": name,
        "description": "a target this listing reports",
        "targetConfiguration": configuration,
    })
}

/// One row of the target-shape table: the configuration union a target was created with,
/// and what its summary must report for it.
struct Shape {
    case: &'static str,
    configuration: Value,
    target_type: &'static str,
    resource_priority: Option<i64>,
    listing_mode: Option<&'static str>,
}

/// Every target configuration shape the model admits, with the `TargetType` the summary
/// must name for it.
///
/// Source for the value list: `TargetType`'s enum (`OPEN_API_SCHEMA | SMITHY_MODEL |
/// MCP_SERVER | LAMBDA | API_GATEWAY | CONNECTOR | AGENTCORE_RUNTIME | PASSTHROUGH |
/// PROVIDER | HTTP_CONNECTOR`, from `smithy_query shape=TargetType`), and for the shapes
/// themselves `TargetConfiguration`, `McpTargetConfiguration`, `HttpTargetConfiguration`
/// and `InferenceTargetConfiguration`.
///
/// `resourcePriority` and `listingMode` are configurable on exactly one of these shapes,
/// the MCP server target (`smithy_query shape=McpServerTargetConfiguration`), whose
/// `resourcePriority` the model documents as defaulting to 1000 when not set.
fn shapes() -> Vec<Shape> {
    vec![
        Shape {
            case: "an MCP target with an inline OpenAPI schema",
            configuration: json!({
                "mcp": {"openApiSchema": {"inlinePayload": "{\"openapi\":\"3.0.0\"}"}}
            }),
            target_type: "OPEN_API_SCHEMA",
            resource_priority: None,
            listing_mode: None,
        },
        Shape {
            case: "an MCP target with an inline Smithy model",
            configuration: json!({
                "mcp": {"smithyModel": {"inlinePayload": "{\"smithy\":\"2.0\"}"}}
            }),
            target_type: "SMITHY_MODEL",
            resource_priority: None,
            listing_mode: None,
        },
        Shape {
            case: "an MCP Lambda target",
            configuration: json!({
                "mcp": {
                    "lambda": {
                        "lambdaArn": "arn:aws:lambda:us-east-1:000000000000:function:mcp-tools",
                        "toolSchema": {
                            "inlinePayload": [{
                                "name": "echo",
                                "description": "echo a string back",
                                "inputSchema": {"type": "object"}
                            }]
                        }
                    }
                }
            }),
            target_type: "LAMBDA",
            resource_priority: None,
            listing_mode: None,
        },
        Shape {
            case: "an MCP server target with neither listingMode nor resourcePriority",
            configuration: json!({
                "mcp": {"mcpServer": {"endpoint": "https://tools.example.com/mcp"}}
            }),
            target_type: "MCP_SERVER",
            // The model: "Defaults to 1000 when not set."
            resource_priority: Some(1000),
            listing_mode: None,
        },
        Shape {
            case: "an MCP server target naming both of them",
            configuration: json!({
                "mcp": {
                    "mcpServer": {
                        "endpoint": "https://dynamic.example.com/mcp",
                        "listingMode": "DYNAMIC",
                        "resourcePriority": 7
                    }
                }
            }),
            target_type: "MCP_SERVER",
            resource_priority: Some(7),
            listing_mode: Some("DYNAMIC"),
        },
        Shape {
            case: "an API Gateway stage target",
            configuration: json!({
                "mcp": {
                    "apiGateway": {
                        "restApiId": "abc1234567",
                        "stage": "prod",
                        "apiGatewayToolConfiguration": {
                            "toolFilters": [{"filterPath": "/pets", "methods": ["GET"]}]
                        }
                    }
                }
            }),
            target_type: "API_GATEWAY",
            resource_priority: None,
            listing_mode: None,
        },
        Shape {
            case: "an MCP connector target",
            configuration: json!({
                "mcp": {"connector": {"source": {"connectorId": "bedrock-knowledge-bases"}}}
            }),
            target_type: "CONNECTOR",
            resource_priority: None,
            listing_mode: None,
        },
        Shape {
            case: "an HTTP AgentCore Runtime target",
            configuration: json!({
                "http": {
                    "agentcoreRuntime": {
                        "arn": "arn:aws:bedrock-agentcore:us-east-1:000000000000:runtime/\
                                my_runtime-abc1234567"
                    }
                }
            }),
            target_type: "AGENTCORE_RUNTIME",
            resource_priority: None,
            listing_mode: None,
        },
        Shape {
            case: "an HTTP passthrough target",
            configuration: json!({
                "http": {
                    "passthrough": {
                        "endpoint": "https://passthrough.example.com/mcp",
                        "protocolType": "MCP"
                    }
                }
            }),
            target_type: "PASSTHROUGH",
            resource_priority: None,
            listing_mode: None,
        },
        Shape {
            case: "an HTTP connector target",
            configuration: json!({
                "http": {"connector": {"source": {"connectorId": "http-connector"}}}
            }),
            target_type: "HTTP_CONNECTOR",
            resource_priority: None,
            listing_mode: None,
        },
        Shape {
            case: "an inference connector target",
            configuration: json!({
                "inference": {"connector": {"source": {"connectorId": "anthropic"}}}
            }),
            target_type: "CONNECTOR",
            resource_priority: None,
            listing_mode: None,
        },
        Shape {
            case: "an inference provider target",
            configuration: json!({
                "inference": {"provider": {"endpoint": "https://api.example.com/v1"}}
            }),
            target_type: "PROVIDER",
            resource_priority: None,
            listing_mode: None,
        },
    ]
}

/// Every target shape the model admits is listed, and each summary is the one
/// `GetGatewayTarget` reports for that same target.
#[tokio::test]
async fn every_target_shape_is_listed_as_its_own_summary() {
    let router = router();
    let gateway = create_gateway(&router, "shapes").await;

    let rows = shapes();
    let mut target_ids: Vec<String> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let name = format!("shape{index}");
        let (status, target) =
            create_target(&router, &gateway, &target_body(&name, row.configuration.clone())).await;
        assert_eq!(
            status, 202,
            "case {:?}: expected HTTP 202 from CreateGatewayTarget, got {status} ({target})",
            row.case
        );
        target_ids.push(text(&target, "targetId"));
    }

    // Past the settle span, so the status every read reports is the settled one and the
    // listing and the single-target read cannot be compared across the transition.
    tokio::time::sleep(PAST_SETTLE).await;

    let (status, listed) = list_targets(&router, &gateway, None, None).await;
    assert_eq!(
        status, 200,
        "case \"the whole listing\": expected HTTP 200 from ListGatewayTargets, got {status} \
         ({listed})"
    );
    let entries = items(&listed);
    assert_eq!(
        entries.len(),
        target_ids.len(),
        "case \"the whole listing\": expected one summary per created target ({}), got {} \
         ({listed})",
        target_ids.len(),
        entries.len()
    );
    assert!(
        listed["nextToken"].is_null(),
        "case \"the whole listing\": a page holding every target carries no nextToken, got {}",
        listed["nextToken"]
    );

    for (row, target_id) in rows.iter().zip(target_ids) {
        let case = row.case;
        let summary = entries
            .iter()
            .find(|entry| text(entry, "targetId") == target_id)
            .unwrap_or_else(|| panic!("case {case:?}: target {target_id} is missing from {listed}"));

        assert_eq!(
            text(summary, "targetType"),
            row.target_type,
            "case {case:?}: expected the targetType its configuration names, got {}",
            text(summary, "targetType")
        );
        let listed_priority = summary["resourcePriority"].as_i64();
        assert_eq!(
            listed_priority, row.resource_priority,
            "case {case:?}: expected resourcePriority {:?}, got {listed_priority:?}",
            row.resource_priority
        );
        let listed_mode = summary["listingMode"].as_str();
        assert_eq!(
            listed_mode, row.listing_mode,
            "case {case:?}: expected listingMode {:?}, got {listed_mode:?}",
            row.listing_mode
        );
        assert!(
            summary["lastSynchronizedAt"].is_null(),
            "case {case:?}: nothing has synchronized this target, got {}",
            summary["lastSynchronizedAt"]
        );
        // `DateTimestamp` carries `@timestampFormat("date-time")`, so every timestamp of a
        // summary is an RFC 3339 STRING on this service's restJson1 wire, never an epoch
        // number (`smithy_query shape=DateTimestamp`).
        for member in ["createdAt", "updatedAt"] {
            let rendered = summary[member].as_str().unwrap_or_default();
            assert!(
                rendered.ends_with('Z') && rendered.contains('T'),
                "case {case:?}: expected {member} as an RFC 3339 date-time string, got {}",
                summary[member]
            );
        }

        // The summary is the single-target read's own account of the same target.
        let (status, fetched) = get_target(&router, &gateway, &target_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetGatewayTarget, got {status} ({fetched})"
        );
        for member in ["targetId", "name", "description", "status", "createdAt", "updatedAt"] {
            assert_eq!(
                summary[member], fetched[member],
                "case {case:?}: the listing's {member} must be the one GetGatewayTarget reports, \
                 got {} for {}",
                summary[member], fetched[member]
            );
        }
        assert_eq!(
            text(summary, "status"),
            "READY",
            "case {case:?}: past the settle span a target is READY, got {}",
            text(summary, "status")
        );
    }
}

/// A listing reports the status the clock owes each target — the transient one before the
/// settle span passes, the settled one afterwards — and reading it changes nothing.
#[tokio::test]
async fn a_listing_reports_the_status_the_clock_owes_and_writes_nothing() {
    let router = router();
    let gateway = create_gateway(&router, "clock").await;
    let (status, _) = create_target(
        &router,
        &gateway,
        &target_body(
            "settling",
            json!({"mcp": {"mcpServer": {"endpoint": "https://tools.example.com/mcp"}}}),
        ),
    )
    .await;
    assert_eq!(status, 202, "case \"a fresh target\": expected HTTP 202, got {status}");

    let (_, fresh) = list_targets(&router, &gateway, None, None).await;
    assert_eq!(
        text(&items(&fresh)[0], "status"),
        "CREATING",
        "case \"inside the settle span\": a fresh target is listed as CREATING, got {}",
        text(&items(&fresh)[0], "status")
    );
    // A read does not move the status on: the second listing inside the span says the same.
    let (_, again) = list_targets(&router, &gateway, None, None).await;
    assert_eq!(
        items(&again)[0]["status"],
        items(&fresh)[0]["status"],
        "case \"a second listing inside the span\": expected the same status, got {} for {}",
        items(&again)[0]["status"],
        items(&fresh)[0]["status"]
    );

    tokio::time::sleep(PAST_SETTLE).await;
    let (_, settled) = list_targets(&router, &gateway, None, None).await;
    assert_eq!(
        text(&items(&settled)[0], "status"),
        "READY",
        "case \"past the settle span\": the target is listed as READY, got {}",
        text(&items(&settled)[0], "status")
    );
    assert_eq!(
        items(&settled)[0]["createdAt"],
        items(&fresh)[0]["createdAt"],
        "case \"past the settle span\": the creation instant is unchanged, got {} for {}",
        items(&settled)[0]["createdAt"],
        items(&fresh)[0]["createdAt"]
    );
}

/// Paging over a gateway's targets yields every target exactly once, in the order one
/// unpaginated call reports, for every page size — including each published bound of
/// `maxResults`.
#[tokio::test]
async fn paging_yields_every_target_once_in_the_listing_order() {
    let router = router();
    let gateway = create_gateway(&router, "paging").await;

    const TARGETS: usize = 7;
    for index in 0..TARGETS {
        let (status, created) = create_target(
            &router,
            &gateway,
            &target_body(
                &format!("page{index}"),
                json!({"mcp": {"openApiSchema": {"inlinePayload": "{\"openapi\":\"3.0.0\"}"}}}),
            ),
        )
        .await;
        assert_eq!(
            status, 202,
            "case \"target {index} of the paging set\": expected HTTP 202, got {status} ({created})"
        );
    }

    let (_, whole) = list_targets(&router, &gateway, None, None).await;
    let expected: Vec<String> = items(&whole)
        .iter()
        .map(|entry| text(entry, "targetId"))
        .collect();
    assert_eq!(
        expected.len(),
        TARGETS,
        "case \"the unpaginated listing\": expected {TARGETS} targets, got {} ({whole})",
        expected.len()
    );

    // (case, maxResults) — the page size is what pagination generalises over: every size
    // below, at and above the number of targets, plus each published bound of the member.
    let rows: [(&str, i64); 7] = [
        ("the smallest page the model admits", MAX_RESULTS_MIN),
        ("two per page", 2),
        ("three per page", 3),
        ("one short of the whole set", (TARGETS - 1) as i64),
        ("exactly the whole set", TARGETS as i64),
        ("one more than the whole set", (TARGETS + 1) as i64),
        ("the largest page the model admits", MAX_RESULTS_MAX),
    ];

    for (case, max_results) in rows {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let (status, page) =
                list_targets(&router, &gateway, Some(max_results), token.as_deref()).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from ListGatewayTargets, got {status} ({page})"
            );
            let entries = items(&page);
            assert!(
                !entries.is_empty(),
                "case {case:?}: a page of a non-empty listing holds at least one target, got \
                 {page}"
            );
            assert!(
                entries.len() as i64 <= max_results,
                "case {case:?}: a page holds at most maxResults={max_results} targets, got {}",
                entries.len()
            );
            seen.extend(entries.iter().map(|entry| text(entry, "targetId")));
            pages += 1;
            assert!(
                pages <= TARGETS + 1,
                "case {case:?}: paging must terminate, got {pages} pages for {} targets",
                TARGETS
            );
            match page["nextToken"].as_str() {
                None => break,
                Some(next) => token = Some(next.to_string()),
            }
        }
        assert_eq!(
            seen, expected,
            "case {case:?}: paging must yield every target once, in the listing order; got \
             {seen:?} for {expected:?}"
        );
    }

    // One past each published bound is refused by the modelled constraint, not served.
    for (case, max_results) in [
        ("one below the minimum maxResults", MAX_RESULTS_MIN - 1),
        ("one above the maximum maxResults", MAX_RESULTS_MAX + 1),
    ] {
        let (status, answer) = list_targets(&router, &gateway, Some(max_results), None).await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} {} ({answer})",
            error_code(&answer)
        );
    }
}

/// What `ListGatewayTargets` answers for a gateway with no targets, a gateway that does
/// not exist, and a `nextToken` this service never issued.
#[tokio::test]
async fn a_listing_refuses_what_it_cannot_serve() {
    let router = router();
    let gateway = create_gateway(&router, "refusals").await;

    // A gateway with no targets is an empty page, not an error, and carries no token.
    let (status, empty) = list_targets(&router, &gateway, None, None).await;
    assert_eq!(
        (status, items(&empty).len()),
        (200, 0),
        "case \"a gateway holding no targets\": expected HTTP 200 and no items, got {status} \
         ({empty})"
    );
    assert!(
        empty["nextToken"].is_null(),
        "case \"a gateway holding no targets\": expected no nextToken, got {}",
        empty["nextToken"]
    );

    // (case, gateway identifier, token, expected status, expected error code)
    let rows: [(&str, &str, Option<&str>, u16, &str); 3] = [
        (
            // Inside `GatewayIdentifier`'s pattern, so this is the handler's refusal.
            "a gateway identifier naming no stored gateway",
            "no-such-gateway-0123456789",
            None,
            404,
            "ResourceNotFoundException",
        ),
        (
            "a nextToken this service never issued",
            "",
            Some("not-a-token-this-service-issued"),
            400,
            "ValidationException",
        ),
        (
            "a nextToken that decodes to no stored target",
            "",
            // Hex for "zzzzzzzzzz", a well-formed token for an id past every minted one.
            Some("7a7a7a7a7a7a7a7a7a7a"),
            200,
            "",
        ),
    ];

    for (case, identifier, token, expected_status, expected_code) in rows {
        let target_gateway = if identifier.is_empty() {
            gateway.as_str()
        } else {
            identifier
        };
        let (status, answer) = list_targets(&router, target_gateway, None, token).await;
        if expected_code.is_empty() {
            assert_eq!(
                (status, items(&answer).len()),
                (expected_status, 0),
                "case {case:?}: expected HTTP {expected_status} with an empty page, got {status} \
                 ({answer})"
            );
        } else {
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (expected_status, expected_code),
                "case {case:?}: expected HTTP {expected_status} {expected_code}, got {status} {} \
                 ({answer})",
                error_code(&answer)
            );
        }
    }
}

/// One gateway's listing reports that gateway's targets and no other's.
#[tokio::test]
async fn a_listing_reports_only_the_named_gateways_targets() {
    let router = router();
    let first = create_gateway(&router, "firstgw").await;
    let second = create_gateway(&router, "secondgw").await;

    let configuration = json!({"mcp": {"mcpServer": {"endpoint": "https://tools.example.com/mcp"}}});
    let (_, only_on_first) =
        create_target(&router, &first, &target_body("onfirst", configuration.clone())).await;
    let (_, only_on_second) =
        create_target(&router, &second, &target_body("onsecond", configuration)).await;

    // (case, gateway, the one target id it holds, the id it must not report)
    let rows: [(&str, &str, String, String); 2] = [
        (
            "the first gateway",
            first.as_str(),
            text(&only_on_first, "targetId"),
            text(&only_on_second, "targetId"),
        ),
        (
            "the second gateway",
            second.as_str(),
            text(&only_on_second, "targetId"),
            text(&only_on_first, "targetId"),
        ),
    ];

    for (case, gateway, own, other) in rows {
        let (status, listed) = list_targets(&router, gateway, None, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from ListGatewayTargets, got {status} ({listed})"
        );
        let ids: Vec<String> = items(&listed)
            .iter()
            .map(|entry| text(entry, "targetId"))
            .collect();
        assert_eq!(
            ids,
            vec![own.clone()],
            "case {case:?}: expected only its own target {own}, got {ids:?}"
        );
        assert!(
            !ids.contains(&other),
            "case {case:?}: the other gateway's target {other} must not be reported, got {ids:?}"
        );
    }
}
