// SPDX-License-Identifier: CC0-1.0

//! `UpdateGatewayTarget` end to end over HTTP, every row read back through
//! `GetGatewayTarget`.
//!
//! Each test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handlers are all exercised, against the one shared engine
//! — so what the update writes is what the read reports.
//!
//! The rules under test, read off the Smithy model rather than off any one recorded request
//! (`smithy_query(query="operation",operation="UpdateGatewayTarget,GetGatewayTarget")`):
//!
//! 1. `UpdateGatewayTarget` (`PUT /gateways/{gatewayIdentifier}/targets/{targetId}/`)
//!    REPLACES the stored target's configuration with the members the request carries: a
//!    configurable member the request omits is cleared, `name` excepted (optional on the
//!    request, `@required` on every response, so it keeps the stored one). The target's
//!    identity — its id, its gateway's ARN and its creation instant — is untouched, and the
//!    update's own response reports exactly what a later read does. The rows differ along
//!    which member changes, the kind of value it takes (a string, a union of a different
//!    shape, a list, a nested structure), and along clearing a member rather than setting
//!    one. `credentialProviderConfigurations` is carried by EVERY row: it is optional on
//!    the request and `@required` on the response, so an update that defines none is
//!    refused rather than defaulted (`gateway_target_update_validation.rs`).
//! 2. An updated target is not ready again at once: the update answers 202 reporting
//!    `UPDATING`, a caller acting straight after still sees that status, and the clock owes
//!    the transition back to `READY`.
//! 3. A target is addressed only through its gateway: an identifier naming no stored gateway,
//!    and a target id naming no target of that gateway, are each the modelled
//!    `ResourceNotFoundException` (404), and nothing is written.
//! 4. A target is served BY its gateway, so the gateway has to be in the terminal status it
//!    settles into: while the clock still owes it a transition (`UPDATING` after an update),
//!    the change is refused 400 `ValidationException` naming the operation and the status it
//!    found. (A gateway cannot be `DELETING` while a live target hangs off it: its own
//!    deletion is refused until every target has been asked to go, and a target whose
//!    deletion was accepted is the 404 of rule 3.)

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

/// The settle span `src/state.rs` gives a gateway's and a target's
/// `CREATING`/`UPDATING` -> `READY` transition, plus slack, so a row can wait the clock out.
const PAST_SETTLE: Duration = Duration::from_millis(900);

/// A `gatewayIdentifier` inside `GatewayIdentifier`'s pattern
/// `^([0-9a-z][-]?){1,100}-[0-9a-z]{10}$` that no row ever creates.
const ABSENT_GATEWAY_ID: &str = "no-such-gateway-0123456789";

/// A `targetId` inside `TargetId`'s pattern `^[0-9a-zA-Z]{10}$` that no row ever creates.
const ABSENT_TARGET_ID: &str = "ZZZZ999999";

/// The published bounds of `TargetDescription` (length 1..200, from
/// `smithy_query(query="shape",shape="TargetDescription")`).
const DESCRIPTION_MIN_LEN: usize = 1;
const DESCRIPTION_MAX_LEN: usize = 200;

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

async fn send(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<&Value>) -> Answer {
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

async fn create_target(router: &MultiServiceRouter, gateway: &str, body: &Value) -> Answer {
    send(
        router,
        "POST",
        &format!("/gateways/{gateway}/targets/"),
        Some(body),
    )
    .await
}

async fn update_target(
    router: &MultiServiceRouter,
    gateway: &str,
    target: &str,
    body: &Value,
) -> Answer {
    send(
        router,
        "PUT",
        &format!("/gateways/{gateway}/targets/{target}/"),
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

/// Create a gateway and hand back its `gatewayId`, waited out to `READY`.
///
/// A gateway serves a target change only once the clock-owed transition out of `CREATING`
/// has come due, so every row that is about the CHANGE starts from a settled gateway.
async fn settled_gateway(router: &MultiServiceRouter, name: &str) -> String {
    let body = json!({
        "name": name,
        "roleArn": ROLE_ARN,
        "authorizerType": "AWS_IAM",
    });
    let created = send(router, "POST", "/gateways/", Some(&body)).await;
    assert_eq!(
        created.status, 202,
        "case {name:?}: expected HTTP 202 from CreateGateway, got {} ({})",
        created.status, created.body
    );
    let gateway_id = text(&created.body, "gatewayId");
    tokio::time::sleep(PAST_SETTLE).await;
    let settled = send(router, "GET", &format!("/gateways/{gateway_id}/"), None).await;
    assert_eq!(
        (settled.status, text(&settled.body, "status").as_str()),
        (200, "READY"),
        "case {name:?}: the gateway must settle to READY before its targets can change, got \
         {} {} ({})",
        settled.status,
        text(&settled.body, "status"),
        settled.body
    );
    gateway_id
}

/// A target configuration union: an MCP target serving an inline OpenAPI schema.
fn openapi_configuration(payload: &str) -> Value {
    json!({"mcp": {"openApiSchema": {"inlinePayload": payload}}})
}

/// A `CreateGatewayTarget` body carrying a value on every optional member an update can
/// replace, so a row that OMITS one observes it being cleared.
fn baseline_create_body(name: &str) -> Value {
    json!({
        "name": name,
        "description": "the target before the update",
        "targetConfiguration": openapi_configuration("{\"openapi\":\"3.0.0\"}"),
        "credentialProviderConfigurations": [{"credentialProviderType": "CALLER_IAM_CREDENTIALS"}],
        "metadataConfiguration": {
            "allowedRequestHeaders": ["x-before"],
            "allowedQueryParameters": ["before"],
            "allowedResponseHeaders": ["x-before-response"],
        },
        "privateEndpoint": {
            "selfManagedLatticeResource": {
                "resourceConfigurationIdentifier": "rcfg-01234567890123456",
            }
        },
        "certificateConfigurations": [{
            "s3": {"uri": "s3://certs-before/private-ca.pem"}
        }],
    })
}

/// The credential provider a row names when the change it is about is not the credential
/// provider itself: the gateway's own IAM role (`CredentialProviderType`'s
/// `GATEWAY_IAM_ROLE`, from `smithy_query(query="shape",shape="CredentialProviderType")`).
///
/// Every `UpdateGatewayTarget` body needs one: the member is optional on the request and
/// `@required` with one entry on the response, so a body that defines none is refused.
fn gateway_iam_role_providers() -> Value {
    json!([{"credentialProviderType": "GATEWAY_IAM_ROLE"}])
}

/// One row of the replacement table: the `UpdateGatewayTarget` body, and the members the
/// target must report once it answers.
struct Row {
    case: &'static str,
    update: Value,
    /// Member -> expected value, where `Value::Null` means the member must be ABSENT.
    expected: Vec<(&'static str, Value)>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a new configuration of the same union shape, with every optional member set",
            update: json!({
                "name": "renamedtarget",
                "description": "the target after the update",
                "targetConfiguration": openapi_configuration("{\"openapi\":\"3.1.0\"}"),
                "credentialProviderConfigurations": [
                    {"credentialProviderType": "JWT_PASSTHROUGH"}
                ],
                "metadataConfiguration": {
                    "allowedRequestHeaders": ["x-after", "x-also-after"],
                },
                "privateEndpoint": {
                    "managedVpcResource": {
                        "vpcIdentifier": "vpc-01234567",
                        "subnetIds": ["subnet-0123456789abcdef0"],
                        "endpointIpAddressType": "IPV4",
                    }
                },
                "certificateConfigurations": [{
                    "secretsManager": {
                        "secretArn":
                            "arn:aws:secretsmanager:us-east-1:000000000000:secret:private-ca-AbCdEf",
                    }
                }],
            }),
            expected: vec![
                ("name", json!("renamedtarget")),
                ("description", json!("the target after the update")),
                (
                    "targetConfiguration",
                    openapi_configuration("{\"openapi\":\"3.1.0\"}"),
                ),
                (
                    "credentialProviderConfigurations",
                    json!([{"credentialProviderType": "JWT_PASSTHROUGH"}]),
                ),
                (
                    "metadataConfiguration",
                    json!({"allowedRequestHeaders": ["x-after", "x-also-after"]}),
                ),
                (
                    "privateEndpoint",
                    json!({
                        "managedVpcResource": {
                            "vpcIdentifier": "vpc-01234567",
                            "subnetIds": ["subnet-0123456789abcdef0"],
                            "endpointIpAddressType": "IPV4",
                        }
                    }),
                ),
                (
                    "certificateConfigurations",
                    json!([{
                        "secretsManager": {
                            "secretArn":
                                "arn:aws:secretsmanager:us-east-1:000000000000:secret:private-ca-AbCdEf",
                        }
                    }]),
                ),
            ],
        },
        Row {
            case: "a configuration of a DIFFERENT union shape (an MCP server endpoint)",
            update: json!({
                "name": "mcpservertarget",
                "description": "now an MCP server endpoint",
                "credentialProviderConfigurations": [
                    {"credentialProviderType": "GATEWAY_IAM_ROLE"}
                ],
                "targetConfiguration": {
                    "mcp": {
                        "mcpServer": {
                            "endpoint": "https://tools.example.com/mcp",
                            "listingMode": "DYNAMIC",
                            "resourcePriority": 7,
                        }
                    }
                },
            }),
            expected: vec![
                ("name", json!("mcpservertarget")),
                ("description", json!("now an MCP server endpoint")),
                (
                    "targetConfiguration",
                    json!({
                        "mcp": {
                            "mcpServer": {
                                "endpoint": "https://tools.example.com/mcp",
                                "listingMode": "DYNAMIC",
                                "resourcePriority": 7,
                            }
                        }
                    }),
                ),
                // Every optional member the request omitted is cleared, and the one the
                // model makes `@required` on the response keeps what the request defined:
                // the stored name is kept, and the credential provider is the one above.
                ("metadataConfiguration", Value::Null),
                ("privateEndpoint", Value::Null),
                ("certificateConfigurations", Value::Null),
                (
                    "credentialProviderConfigurations",
                    gateway_iam_role_providers(),
                ),
            ],
        },
        Row {
            case: "an HTTP passthrough configuration and nothing else, clearing the rest",
            update: json!({
                "credentialProviderConfigurations": [
                    {"credentialProviderType": "GATEWAY_IAM_ROLE"}
                ],
                "targetConfiguration": {
                    "http": {
                        "passthrough": {
                            "endpoint": "https://passthrough.example.com/mcp",
                            "protocolType": "MCP",
                        }
                    }
                },
            }),
            expected: vec![
                // `name` is optional on the request and `@required` on the response, so a
                // request naming none keeps the name the target was created with.
                ("name", json!("baselinetarget")),
                ("description", Value::Null),
                (
                    "targetConfiguration",
                    json!({
                        "http": {
                            "passthrough": {
                                "endpoint": "https://passthrough.example.com/mcp",
                                "protocolType": "MCP",
                            }
                        }
                    }),
                ),
                ("metadataConfiguration", Value::Null),
                ("privateEndpoint", Value::Null),
                ("certificateConfigurations", Value::Null),
                (
                    "credentialProviderConfigurations",
                    gateway_iam_role_providers(),
                ),
            ],
        },
        Row {
            case: "a description at the shortest length the model admits",
            update: json!({
                "description": "x".repeat(DESCRIPTION_MIN_LEN),
                "credentialProviderConfigurations": [
                    {"credentialProviderType": "GATEWAY_IAM_ROLE"}
                ],
                "targetConfiguration": openapi_configuration("{\"openapi\":\"3.0.1\"}"),
            }),
            expected: vec![
                ("name", json!("baselinetarget")),
                ("description", json!("x".repeat(DESCRIPTION_MIN_LEN))),
            ],
        },
        Row {
            case: "a description at the longest length the model admits",
            update: json!({
                "description": "y".repeat(DESCRIPTION_MAX_LEN),
                "credentialProviderConfigurations": [
                    {"credentialProviderType": "GATEWAY_IAM_ROLE"}
                ],
                "targetConfiguration": openapi_configuration("{\"openapi\":\"3.0.2\"}"),
            }),
            expected: vec![
                ("name", json!("baselinetarget")),
                ("description", json!("y".repeat(DESCRIPTION_MAX_LEN))),
            ],
        },
        Row {
            case: "a Lambda configuration, which no mounted Lambda contradicts",
            update: json!({
                "credentialProviderConfigurations": [
                    {"credentialProviderType": "GATEWAY_IAM_ROLE"}
                ],
                "targetConfiguration": {
                    "mcp": {
                        "lambda": {
                            "lambdaArn":
                                "arn:aws:lambda:us-east-1:000000000000:function:mcp-tools",
                            "toolSchema": {
                                "inlinePayload": [{
                                    "name": "echo",
                                    "description": "echo a string back",
                                    "inputSchema": {"type": "object"},
                                }]
                            },
                        }
                    }
                },
            }),
            expected: vec![
                ("name", json!("baselinetarget")),
                (
                    "targetConfiguration",
                    json!({
                        "mcp": {
                            "lambda": {
                                "lambdaArn":
                                    "arn:aws:lambda:us-east-1:000000000000:function:mcp-tools",
                                "toolSchema": {
                                    "inlinePayload": [{
                                        "name": "echo",
                                        "description": "echo a string back",
                                        "inputSchema": {"type": "object"},
                                    }]
                                },
                            }
                        }
                    }),
                ),
            ],
        },
    ]
}

/// Rule 1: the update replaces the stored target, reports what it wrote, and a later read
/// agrees with it member for member.
#[tokio::test]
async fn an_update_replaces_the_stored_target_and_the_read_agrees() {
    for row in rows() {
        let case = row.case;
        let router = router();
        let gateway_id = settled_gateway(&router, "mutategateway").await;
        let created = create_target(
            &router,
            &gateway_id,
            &baseline_create_body("baselinetarget"),
        )
        .await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: the baseline target must be created, got {} ({})",
            created.status, created.body
        );
        let target_id = text(&created.body, "targetId");

        let updated = update_target(&router, &gateway_id, &target_id, &row.update).await;
        assert_eq!(
            updated.status, 202,
            "case {case:?}: expected HTTP 202 from UpdateGatewayTarget, got {} ({})",
            updated.status, updated.body
        );

        for (member, want) in &row.expected {
            let got = updated.body.get(member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                &got, want,
                "case {case:?}: the update must report {member} as {want}, got {got:?}"
            );
        }

        // The target's identity is untouched by a change to its configuration.
        assert_eq!(
            text(&updated.body, "targetId"),
            target_id,
            "case {case:?}: the update must report the same targetId, got {:?}",
            text(&updated.body, "targetId")
        );
        assert_eq!(
            text(&updated.body, "gatewayArn"),
            text(&created.body, "gatewayArn"),
            "case {case:?}: the update must report the same gatewayArn, got {:?}",
            text(&updated.body, "gatewayArn")
        );
        assert_eq!(
            updated.body.get("createdAt"),
            created.body.get("createdAt"),
            "case {case:?}: the update must not move the creation instant, got {:?} for {}",
            updated.body.get("createdAt"),
            created.body
        );

        // `UpdateGatewayTargetResponse` and `GetGatewayTargetResponse` declare the same
        // members, so the two answers may differ only in the status the clock owes.
        let fetched = get_target(&router, &gateway_id, &target_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: the updated target must read back, got {} ({})",
            fetched.status, fetched.body
        );
        let mut from_update = updated.body.clone();
        let mut from_read = fetched.body.clone();
        for body in [&mut from_update, &mut from_read] {
            if let Some(members) = body.as_object_mut() {
                members.remove("status");
            }
        }
        assert_eq!(
            from_update, from_read,
            "case {case:?}: the read must report exactly what the update wrote, got \
             {from_read} for {from_update}"
        );
    }
}

/// Rule 2: an updated target reports `UPDATING` and the clock owes it `READY`.
#[tokio::test]
async fn an_updated_target_settles_from_updating_back_to_ready() {
    let router = router();
    let gateway_id = settled_gateway(&router, "settletarget").await;
    let created = create_target(
        &router,
        &gateway_id,
        &baseline_create_body("settlingtarget"),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "setup: the target must be created, got {} ({})",
        created.status, created.body
    );
    let target_id = text(&created.body, "targetId");
    assert_eq!(
        text(&created.body, "status"),
        "CREATING",
        "setup: a new target is CREATING, got {}",
        text(&created.body, "status")
    );
    tokio::time::sleep(PAST_SETTLE).await;
    let settled = get_target(&router, &gateway_id, &target_id).await;
    assert_eq!(
        text(&settled.body, "status"),
        "READY",
        "setup: the target must settle to READY, got {} ({})",
        text(&settled.body, "status"),
        settled.body
    );

    let update = json!({
        "credentialProviderConfigurations": gateway_iam_role_providers(),
        "targetConfiguration": openapi_configuration("{\"openapi\":\"3.0.3\"}"),
    });
    let updated = update_target(&router, &gateway_id, &target_id, &update).await;
    assert_eq!(
        (updated.status, text(&updated.body, "status").as_str()),
        (202, "UPDATING"),
        "an accepted update reports UPDATING, got {} {} ({})",
        updated.status,
        text(&updated.body, "status"),
        updated.body
    );
    // A caller acting straight after the call still sees the transient status.
    let during = get_target(&router, &gateway_id, &target_id).await;
    assert_eq!(
        text(&during.body, "status"),
        "UPDATING",
        "a read straight after the update still reports UPDATING, got {} ({})",
        text(&during.body, "status"),
        during.body
    );

    tokio::time::sleep(PAST_SETTLE).await;
    let after = get_target(&router, &gateway_id, &target_id).await;
    assert_eq!(
        text(&after.body, "status"),
        "READY",
        "the clock owes the transition back to READY, got {} ({})",
        text(&after.body, "status"),
        after.body
    );
}

/// The published bounds of `TargetDescription`: both ends are served (pinned by the table
/// above) and one character past the longest is refused by the constraint walk.
#[tokio::test]
async fn a_description_past_its_published_bound_is_refused() {
    let router = router();
    let gateway_id = settled_gateway(&router, "boundstarget").await;
    let created = create_target(&router, &gateway_id, &baseline_create_body("boundtarget")).await;
    let target_id = text(&created.body, "targetId");

    let update = json!({
        "description": "z".repeat(DESCRIPTION_MAX_LEN + 1),
        "credentialProviderConfigurations": gateway_iam_role_providers(),
        "targetConfiguration": openapi_configuration("{\"openapi\":\"3.0.0\"}"),
    });
    let refused = update_target(&router, &gateway_id, &target_id, &update).await;
    assert_eq!(
        (refused.status, error_code(&refused.body).as_str()),
        (400, "ValidationException"),
        "a description one character past the longest the model admits must be refused 400 \
         ValidationException, got {} {} ({})",
        refused.status,
        error_code(&refused.body),
        refused.body
    );

    // Nothing is written by a refused update.
    let fetched = get_target(&router, &gateway_id, &target_id).await;
    assert_eq!(
        text(&fetched.body, "description"),
        "the target before the update",
        "a refused update must leave the stored target alone, got {} ({})",
        text(&fetched.body, "description"),
        fetched.body
    );
}

/// Rule 3: a target is addressed only through its gateway, and an update mints neither.
#[tokio::test]
async fn an_update_of_something_that_is_not_there_is_the_modelled_404() {
    let router = router();
    let gateway_id = settled_gateway(&router, "absentgateway").await;
    let created = create_target(&router, &gateway_id, &baseline_create_body("presenttarget")).await;
    let present_target = text(&created.body, "targetId");

    // (case, gatewayIdentifier, targetId, the sentence the refusal takes). The verb is
    // "retrieve" for both resources: what failed is the lookup, not the update.
    let rows: [(&str, String, String, &str); 3] = [
        (
            "no such gateway",
            ABSENT_GATEWAY_ID.to_string(),
            present_target.clone(),
            "Failed to retrieve gateway because it doesn't exist. Retry the request with a \
             different resource identifier.",
        ),
        (
            "no such target on a stored gateway",
            gateway_id.clone(),
            ABSENT_TARGET_ID.to_string(),
            "Failed to retrieve target because it doesn't exist. Retry the request with a \
             different resource identifier.",
        ),
        (
            "neither the gateway nor the target",
            ABSENT_GATEWAY_ID.to_string(),
            ABSENT_TARGET_ID.to_string(),
            "Failed to retrieve gateway because it doesn't exist. Retry the request with a \
             different resource identifier.",
        ),
    ];

    for (case, gateway, target, message) in rows {
        let update = json!({
            "credentialProviderConfigurations": gateway_iam_role_providers(),
            "targetConfiguration": openapi_configuration("{\"openapi\":\"3.0.0\"}"),
        });
        let refused = update_target(&router, &gateway, &target, &update).await;
        assert_eq!(
            (refused.status, error_code(&refused.body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected HTTP 404 ResourceNotFoundException, got {} {} ({})",
            refused.status,
            error_code(&refused.body),
            refused.body
        );
        assert_eq!(
            text(&refused.body, "message"),
            message,
            "case {case:?}: the refusal names what it could not find, got {}",
            text(&refused.body, "message")
        );
    }
}

/// Rule 4: a gateway the clock still owes a transition serves no change to its targets, and
/// the refusal names the status it found.
#[tokio::test]
async fn a_gateway_that_cannot_serve_the_change_refuses_the_update() {
    // (case, the request that moves the gateway out of READY, the status it then reports,
    // that status as the refusal's prose spells it)
    //
    // `DELETING` is not a row here: a gateway that still has a target ATTACHED serves no
    // deletion of its own, so a gateway can only reach `DELETING` once every target on it
    // has been asked to go — and a target whose own deletion was accepted is the modelled
    // 404 to an update, which rule 5's test covers. `UPDATING` is the one transient status
    // a gateway carrying a live target can be in.
    let rows: [(&str, &str, &str, &str); 1] =
        [("an update in flight on the gateway", "PUT", "UPDATING", "Updating")];

    for (case, method, status_name, status_prose) in rows {
        let router = router();
        let gateway_id = settled_gateway(&router, "gatewaygate").await;
        let created =
            create_target(&router, &gateway_id, &baseline_create_body("gatedtarget")).await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: the target must be created while the gateway is READY, got {} ({})",
            created.status, created.body
        );
        let target_id = text(&created.body, "targetId");

        let body = (method == "PUT").then(|| {
            json!({
                "name": "gatewaygate",
                "roleArn": ROLE_ARN,
                "authorizerType": "AWS_IAM",
            })
        });
        let moved = send(
            &router,
            method,
            &format!("/gateways/{gateway_id}/"),
            body.as_ref(),
        )
        .await;
        assert_eq!(
            (moved.status, text(&moved.body, "status").as_str()),
            (202, status_name),
            "case {case:?}: the setup call must leave the gateway in {status_name}, got {} {} \
             ({})",
            moved.status,
            text(&moved.body, "status"),
            moved.body
        );

        let update = json!({
            "description": "a change the gateway cannot serve",
            "credentialProviderConfigurations": gateway_iam_role_providers(),
            "targetConfiguration": openapi_configuration("{\"openapi\":\"3.0.9\"}"),
        });
        let refused = update_target(&router, &gateway_id, &target_id, &update).await;
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
            format!(
                "UpdateGatewayTarget operation can't be performed on gateway when it is in \
                 {status_prose} state. Retry the request when the gateway is in a valid state."
            ),
            "case {case:?}: the refusal names the operation and the status it found, got {}",
            text(&refused.body, "message")
        );
    }
}
