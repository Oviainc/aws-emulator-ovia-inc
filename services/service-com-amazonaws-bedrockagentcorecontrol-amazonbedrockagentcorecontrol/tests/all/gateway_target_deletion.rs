// SPDX-License-Identifier: CC0-1.0

//! `DeleteGatewayTarget` end to end over HTTP, every row read back through
//! `GetGatewayTarget` and `ListGatewayTargets`.
//!
//! Each test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handlers are all exercised against the one shared engine —
//! so what the deletion writes is what the reads report.
//!
//! The rules under test, read off the Smithy model rather than off any one recorded request
//! (`smithy_query(query="operation",operation="DeleteGatewayTarget,GetGatewayTarget,\
//! ListGatewayTargets")`):
//!
//! 1. `DeleteGatewayTarget` (`DELETE /gateways/{gatewayIdentifier}/targets/{targetId}/`)
//!    ACCEPTS the deletion of one target of a gateway: it answers 202 reporting that
//!    target's id, its gateway's ARN and `DELETING`, a caller reading straight after still
//!    sees the target in `DELETING`, and the removal itself is owed by the clock — once the
//!    span passes the target is gone from both reads while the gateway and its other
//!    targets are untouched. The rows differ along the shape of the configuration the
//!    deleted target carries, how many targets the gateway serves, which of them is
//!    deleted, and whether the target had settled to `READY` first.
//! 2. The operation is `@idempotent`: a repeat while the deletion is still under way
//!    reports the same target in `DELETING` without re-arming the clock, and a repeat after
//!    the removal came due is the modelled `ResourceNotFoundException` (404).
//! 3. A target is addressed only through its gateway: an identifier naming no stored
//!    gateway, and a target id naming no target of that gateway, are each the modelled
//!    `ResourceNotFoundException` (404), and nothing is removed.
//! 4. A target is served BY its gateway, so the gateway has to be in the terminal status it
//!    settles into: while the clock still owes it a transition (`UPDATING` after an update,
//!    `DELETING` after a delete), the deletion is refused 400 `ValidationException` naming
//!    the operation and the status it found.
//! 5. A target whose deletion was accepted is no longer there to change:
//!    `UpdateGatewayTarget` on it is the modelled `ResourceNotFoundException` (404), the
//!    same refusal a target id naming nothing gets.

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

/// A `gatewayIdentifier` inside `GatewayIdentifier`'s pattern
/// `^([0-9a-z][-]?){1,100}-[0-9a-z]{10}$` that no row ever creates.
const ABSENT_GATEWAY_ID: &str = "no-such-gateway-0123456789";

/// A `targetId` inside `TargetId`'s pattern `^[0-9a-zA-Z]{10}$` that no row ever creates.
const ABSENT_TARGET_ID: &str = "ZZZZ999999";

/// The sentence this service refuses a lookup of a missing gateway with, and of a missing
/// target of a stored gateway.
const GATEWAY_ABSENT_MESSAGE: &str = "Failed to retrieve gateway because it doesn't exist. \
                                      Retry the request with a different resource identifier.";
const TARGET_ABSENT_MESSAGE: &str = "Failed to retrieve target because it doesn't exist. \
                                     Retry the request with a different resource identifier.";

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

/// A `CreateGatewayTarget` body for `name` carrying `configuration`.
///
/// `credentialProviderConfigurations` is optional on the request and `@required` on every
/// response, so the gateway's own IAM role is what these rows name
/// (`smithy_query(query="shape",shape="CredentialProviderType")`).
fn create_body(name: &str, configuration: Value) -> Value {
    json!({
        "name": name,
        "description": "a target about to be deleted",
        "targetConfiguration": configuration,
        "credentialProviderConfigurations": [{"credentialProviderType": "GATEWAY_IAM_ROLE"}],
    })
}

/// One row of the deletion table.
struct Row {
    case: &'static str,
    /// The configuration the deleted target carries.
    configuration: Value,
    /// How many targets the gateway serves when the deletion arrives.
    targets: usize,
    /// Which of them is deleted (0-based, in creation order).
    deleted: usize,
    /// Whether the targets are waited out to `READY` before the deletion.
    settled_first: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the only target of the gateway, an inline OpenAPI schema, settled to READY",
            configuration: openapi_configuration("{\"openapi\":\"3.0.0\"}"),
            targets: 1,
            deleted: 0,
            settled_first: true,
        },
        Row {
            case: "the FIRST of three targets, an MCP server endpoint",
            configuration: json!({
                "mcp": {
                    "mcpServer": {
                        "endpoint": "https://tools.example.com/mcp",
                        "listingMode": "DYNAMIC",
                        "resourcePriority": 7,
                    }
                }
            }),
            targets: 3,
            deleted: 0,
            settled_first: true,
        },
        Row {
            case: "the LAST of three targets, an HTTP passthrough endpoint",
            configuration: json!({
                "http": {
                    "passthrough": {
                        "endpoint": "https://passthrough.example.com/mcp",
                        "protocolType": "MCP",
                    }
                }
            }),
            targets: 3,
            deleted: 2,
            settled_first: true,
        },
        Row {
            // AWS refuses a deletion only for a target in a pending authorization state
            // (`CREATE_PENDING_AUTH`, `UPDATE_PENDING_AUTH`, `SYNCHRONIZE_PENDING_AUTH`,
            // from `smithy_query(query="doc",shape="DeleteGatewayTarget")`), which no call
            // of this emulator puts a target in, so a target the clock has not finished
            // creating is deleted like any other.
            case: "a target the clock has not finished creating yet",
            configuration: openapi_configuration("{\"openapi\":\"3.1.0\"}"),
            targets: 2,
            deleted: 1,
            settled_first: false,
        },
    ]
}

/// Rule 1: the deletion is accepted, reports `DELETING`, and the clock owes the removal.
#[tokio::test]
async fn an_accepted_deletion_removes_only_that_target_when_the_clock_makes_it_due() {
    for row in rows() {
        let case = row.case;
        let router = router();
        let gateway_id = settled_gateway(&router, "deletiongateway").await;

        let mut target_ids = Vec::new();
        for index in 0..row.targets {
            let configuration = if index == row.deleted {
                row.configuration.clone()
            } else {
                openapi_configuration("{\"openapi\":\"3.0.2\"}")
            };
            let created = create_target(
                &router,
                &gateway_id,
                &create_body(&format!("target{index}"), configuration),
            )
            .await;
            assert_eq!(
                created.status, 202,
                "case {case:?}: target {index} must be created, got {} ({})",
                created.status, created.body
            );
            target_ids.push(text(&created.body, "targetId"));
        }
        let target_id = target_ids[row.deleted].clone();
        let gateway_arn = {
            let fetched = send(&router, "GET", &format!("/gateways/{gateway_id}/"), None).await;
            text(&fetched.body, "gatewayArn")
        };

        if row.settled_first {
            tokio::time::sleep(PAST_SETTLE).await;
            let settled = get_target(&router, &gateway_id, &target_id).await;
            assert_eq!(
                text(&settled.body, "status"),
                "READY",
                "case {case:?}: the target must settle to READY first, got {} ({})",
                text(&settled.body, "status"),
                settled.body
            );
        }

        let deleted = delete_target(&router, &gateway_id, &target_id).await;
        assert_eq!(
            (deleted.status, text(&deleted.body, "status").as_str()),
            (202, "DELETING"),
            "case {case:?}: an accepted deletion answers 202 DELETING, got {} {} ({})",
            deleted.status,
            text(&deleted.body, "status"),
            deleted.body
        );
        assert_eq!(
            text(&deleted.body, "targetId"),
            target_id,
            "case {case:?}: the deletion names the target it accepted, got {:?}",
            text(&deleted.body, "targetId")
        );
        assert_eq!(
            text(&deleted.body, "gatewayArn"),
            gateway_arn,
            "case {case:?}: the deletion names the gateway that served the target, got {:?}",
            text(&deleted.body, "gatewayArn")
        );

        // A caller acting straight after the call still sees the target, in DELETING.
        let during = get_target(&router, &gateway_id, &target_id).await;
        assert_eq!(
            (during.status, text(&during.body, "status").as_str()),
            (200, "DELETING"),
            "case {case:?}: a read straight after the deletion still reports DELETING, got \
             {} {} ({})",
            during.status,
            text(&during.body, "status"),
            during.body
        );
        let listed_during = listed_target_ids(&router, &gateway_id).await;
        assert!(
            listed_during.contains(&target_id),
            "case {case:?}: a target still DELETING is still listed, got {listed_during:?}"
        );

        tokio::time::sleep(PAST_SETTLE).await;

        let gone = get_target(&router, &gateway_id, &target_id).await;
        assert_eq!(
            (gone.status, error_code(&gone.body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: once the removal comes due the target is the modelled 404, got \
             {} {} ({})",
            gone.status,
            error_code(&gone.body),
            gone.body
        );
        assert_eq!(
            text(&gone.body, "message"),
            TARGET_ABSENT_MESSAGE,
            "case {case:?}: the refusal names what it could not find, got {}",
            text(&gone.body, "message")
        );

        // Every other target of the gateway, and the gateway itself, is untouched. The
        // listing pages the gateway's targets in its own deterministic order, so the rows
        // compare the SET of ids rather than the order they were created in.
        let mut survivors: Vec<String> = target_ids
            .iter()
            .filter(|id| **id != target_id)
            .cloned()
            .collect();
        survivors.sort();
        let mut listed = listed_target_ids(&router, &gateway_id).await;
        listed.sort();
        assert_eq!(
            listed, survivors,
            "case {case:?}: the listing must report exactly the targets that were not \
             deleted, got {listed:?} for {survivors:?}"
        );
        for survivor in &survivors {
            let fetched = get_target(&router, &gateway_id, survivor).await;
            assert_eq!(
                (fetched.status, text(&fetched.body, "status").as_str()),
                (200, "READY"),
                "case {case:?}: target {survivor} must still read back READY, got {} {} ({})",
                fetched.status,
                text(&fetched.body, "status"),
                fetched.body
            );
        }
        let gateway = send(&router, "GET", &format!("/gateways/{gateway_id}/"), None).await;
        assert_eq!(
            (gateway.status, text(&gateway.body, "status").as_str()),
            (200, "READY"),
            "case {case:?}: deleting a target leaves its gateway READY, got {} {} ({})",
            gateway.status,
            text(&gateway.body, "status"),
            gateway.body
        );
    }
}

/// Rule 2: the deletion is `@idempotent` while it is under way, and the modelled 404 once
/// the removal has come due.
#[tokio::test]
async fn a_repeated_deletion_is_idempotent_and_then_the_modelled_404() {
    let router = router();
    let gateway_id = settled_gateway(&router, "idempotentgateway").await;
    let created = create_target(
        &router,
        &gateway_id,
        &create_body("repeatedtarget", openapi_configuration("{\"openapi\":\"3.0.0\"}")),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "setup: the target must be created, got {} ({})",
        created.status, created.body
    );
    let target_id = text(&created.body, "targetId");
    tokio::time::sleep(PAST_SETTLE).await;

    let first = delete_target(&router, &gateway_id, &target_id).await;
    assert_eq!(
        (first.status, text(&first.body, "status").as_str()),
        (202, "DELETING"),
        "the first deletion answers 202 DELETING, got {} {} ({})",
        first.status,
        text(&first.body, "status"),
        first.body
    );
    let repeat = delete_target(&router, &gateway_id, &target_id).await;
    assert_eq!(
        repeat.body, first.body,
        "a repeat while the deletion is under way reports the same target, got {} for {}",
        repeat.body, first.body
    );
    assert_eq!(
        repeat.status, 202,
        "a repeat while the deletion is under way answers 202, got {} ({})",
        repeat.status, repeat.body
    );

    // The repeat does not re-arm the clock: the removal is still due on the span the FIRST
    // deletion started, so waiting that span out once is enough.
    tokio::time::sleep(PAST_SETTLE).await;
    let after = delete_target(&router, &gateway_id, &target_id).await;
    assert_eq!(
        (after.status, error_code(&after.body).as_str()),
        (404, "ResourceNotFoundException"),
        "a deletion of an already-removed target is the modelled 404, got {} {} ({})",
        after.status,
        error_code(&after.body),
        after.body
    );
    assert_eq!(
        text(&after.body, "message"),
        TARGET_ABSENT_MESSAGE,
        "the refusal names what it could not find, got {}",
        text(&after.body, "message")
    );
}

/// Rule 3: a target is addressed only through its gateway, and a deletion removes neither
/// anything else nor anything when it refuses.
#[tokio::test]
async fn a_deletion_of_something_that_is_not_there_is_the_modelled_404() {
    let router = router();
    let gateway_id = settled_gateway(&router, "absentgatewaydelete").await;
    let created = create_target(
        &router,
        &gateway_id,
        &create_body("presenttarget", openapi_configuration("{\"openapi\":\"3.0.0\"}")),
    )
    .await;
    let present_target = text(&created.body, "targetId");

    // (case, gatewayIdentifier, targetId, the sentence the refusal takes). The verb is
    // "retrieve" for both resources: what failed is the lookup, not the deletion.
    let rows: [(&str, String, String, &str); 3] = [
        (
            "no such gateway",
            ABSENT_GATEWAY_ID.to_string(),
            present_target.clone(),
            GATEWAY_ABSENT_MESSAGE,
        ),
        (
            "no such target on a stored gateway",
            gateway_id.clone(),
            ABSENT_TARGET_ID.to_string(),
            TARGET_ABSENT_MESSAGE,
        ),
        (
            "neither the gateway nor the target",
            ABSENT_GATEWAY_ID.to_string(),
            ABSENT_TARGET_ID.to_string(),
            GATEWAY_ABSENT_MESSAGE,
        ),
    ];

    for (case, gateway, target, message) in rows {
        let refused = delete_target(&router, &gateway, &target).await;
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

    // Nothing was removed by a refused deletion.
    let listed = listed_target_ids(&router, &gateway_id).await;
    assert_eq!(
        listed,
        vec![present_target.clone()],
        "a refused deletion leaves the stored targets alone, got {listed:?}"
    );
}

/// Rule 4: a gateway the clock still owes a transition serves no deletion of its targets,
/// and the refusal names the status it found.
#[tokio::test]
async fn a_gateway_that_cannot_serve_the_change_refuses_the_deletion() {
    // (case, the request that moves the gateway out of READY, the status it then reports,
    // that status as the refusal's prose spells it)
    let rows: [(&str, &str, &str, &str); 2] = [
        ("an update in flight on the gateway", "PUT", "UPDATING", "Updating"),
        ("a deletion accepted for the gateway", "DELETE", "DELETING", "Deleting"),
    ];

    for (case, method, status_name, status_prose) in rows {
        let router = router();
        let gateway_id = settled_gateway(&router, "gatewaygatedelete").await;
        let created = create_target(
            &router,
            &gateway_id,
            &create_body("gatedtarget", openapi_configuration("{\"openapi\":\"3.0.0\"}")),
        )
        .await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: the target must be created while the gateway is READY, got {} ({})",
            created.status, created.body
        );
        let target_id = text(&created.body, "targetId");

        // A gateway with a target still ATTACHED serves no deletion of its own, so the row
        // that needs a `DELETING` gateway asks for the target to go first. The target
        // record is still there — on its way out on the clock, which the removal span has
        // not reached — so the refusal below is the gateway's status and not a missing
        // target.
        if method == "DELETE" {
            let dropped = delete_target(&router, &gateway_id, &target_id).await;
            assert_eq!(
                dropped.status, 202,
                "case {case:?}: the target must go before the gateway's own delete, got {} ({})",
                dropped.status, dropped.body
            );
        }

        let body = (method == "PUT").then(|| {
            json!({
                "name": "gatewaygatedelete",
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

        let refused = delete_target(&router, &gateway_id, &target_id).await;
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
                "DeleteGatewayTarget operation can't be performed on gateway when it is in \
                 {status_prose} state. Retry the request when the gateway is in a valid state."
            ),
            "case {case:?}: the refusal names the operation and the status it found, got {}",
            text(&refused.body, "message")
        );
    }
}

/// Rule 5: a target whose deletion was accepted is no longer there to change.
#[tokio::test]
async fn a_deleting_target_serves_no_update() {
    let router = router();
    let gateway_id = settled_gateway(&router, "deletingtargetgate").await;
    let created = create_target(
        &router,
        &gateway_id,
        &create_body("deletingtarget", openapi_configuration("{\"openapi\":\"3.0.0\"}")),
    )
    .await;
    let target_id = text(&created.body, "targetId");
    tokio::time::sleep(PAST_SETTLE).await;

    let deleted = delete_target(&router, &gateway_id, &target_id).await;
    assert_eq!(
        (deleted.status, text(&deleted.body, "status").as_str()),
        (202, "DELETING"),
        "setup: the deletion must be accepted, got {} {} ({})",
        deleted.status,
        text(&deleted.body, "status"),
        deleted.body
    );

    let update = json!({
        "credentialProviderConfigurations": [{"credentialProviderType": "GATEWAY_IAM_ROLE"}],
        "targetConfiguration": openapi_configuration("{\"openapi\":\"3.0.3\"}"),
    });
    let refused = send(
        &router,
        "PUT",
        &format!("/gateways/{gateway_id}/targets/{target_id}/"),
        Some(&update),
    )
    .await;
    assert_eq!(
        (refused.status, error_code(&refused.body).as_str()),
        (404, "ResourceNotFoundException"),
        "an update of a target whose deletion was accepted is the modelled 404, got {} {} ({})",
        refused.status,
        error_code(&refused.body),
        refused.body
    );
    assert_eq!(
        text(&refused.body, "message"),
        TARGET_ABSENT_MESSAGE,
        "the refusal names what it could not find, got {}",
        text(&refused.body, "message")
    );
}
