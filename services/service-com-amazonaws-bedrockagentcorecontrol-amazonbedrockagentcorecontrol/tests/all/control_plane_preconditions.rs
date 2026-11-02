// SPDX-License-Identifier: CC0-1.0

//! The pre-conditions this control plane enforces BEFORE it writes, over HTTP.
//!
//! Two rules are under test, each read off the model and the resource's own stored state
//! rather than off any one recorded request:
//!
//! 1. A gateway accepts a target only once the clock-owed transition out of its transient
//!    status has come due: while the gateway is still `CREATING`, `CreateGatewayTarget` is
//!    refused with the operation's modelled `ValidationException` (HTTP 400,
//!    `smithy_query(query="operation",operation="CreateGatewayTarget")`) and nothing is
//!    stored, and the same request succeeds once the gateway reports `READY`. The rows
//!    differ along the target shape the request carries, so the gateway's status is the
//!    only thing that can decide the answer — and the gate is the STATUS, not one named
//!    status: a gateway whose deletion was accepted (`DELETING`) refuses the target too.
//! 2. A caller-supplied execution role is refused only on what IAM ANSWERED. IAM is not
//!    mounted in this single-service router, so nothing is known about any role: every
//!    operation that takes a `roleArn` writes rather than refusing on an assumption. The
//!    rows differ along the account the ARN names, the path the role sits under, and the
//!    operation that takes it. (The grounded half of that rule — IAM answering that the
//!    role is absent or does not trust this service — is
//!    `core/router/tests/all/bedrock_agentcore_execution_role.rs`, the only place both
//!    services can be mounted together.)
//! 3. A request addressing a resource that is not there is refused with the operation's
//!    modelled `ResourceNotFoundException` (HTTP 404), worded as this service words a
//!    resource that does not exist: the FAILED RETRIEVE of it, the resource it was for,
//!    and the remedy — and nothing of the identifier the caller sent. The rows differ
//!    along the operation (read, update, delete, create-through, list-through) and WHICH
//!    resource of the request is missing — the gateway, or a target of a gateway that is
//!    there — because the sentence varies with neither: what failed is the lookup.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region every request here is signed for.
const REGION: &str = "us-east-1";

/// The settle span `src/state.rs` gives the `CREATING` -> `READY` transition, plus slack,
/// so a test can wait the clock out.
const PAST_SETTLE: Duration = Duration::from_millis(900);

/// A role ARN inside `RoleArn`'s pattern `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AgentCoreRole";

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

/// A `CreateGateway` body for `name`, with a role every row shares.
fn gateway_body(name: &str) -> Value {
    json!({"name": name, "roleArn": ROLE_ARN, "authorizerType": "AWS_IAM"})
}

/// Create a gateway and answer with its `gatewayId`, WITHOUT waiting for the clock-owed
/// transition: the status right after the create is what rule 1 is about.
async fn create_gateway(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = send(router, "POST", "/gateways/", Some(&gateway_body(name))).await;
    assert_eq!(
        status, 202,
        "setup {name:?}: CreateGateway must succeed for this suite to say anything, got {status} \
         ({created})"
    );
    assert_eq!(
        text(&created, "status"),
        "CREATING",
        "setup {name:?}: a new gateway is CREATING, got {}",
        text(&created, "status")
    );
    text(&created, "gatewayId")
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

/// One target body per shape the `TargetConfiguration` union declares an implemented
/// handler for. Source: `smithy_query shape=TargetConfiguration,McpTargetConfiguration`.
fn target_bodies() -> [(&'static str, Value); 3] {
    [
        (
            "an MCP target with an inline OpenAPI schema",
            json!({
                "name": "openapi",
                "targetConfiguration": {
                    "mcp": {"openApiSchema": {"inlinePayload": "{\"openapi\":\"3.0.0\"}"}}
                }
            }),
        ),
        (
            "an MCP server endpoint target",
            json!({
                "name": "mcpserver",
                "targetConfiguration": {
                    "mcp": {"mcpServer": {"endpoint": "https://tools.example.com/mcp"}}
                }
            }),
        ),
        (
            "an MCP Lambda target",
            json!({
                "name": "lambdatarget",
                "targetConfiguration": {
                    "mcp": {
                        "lambda": {
                            "lambdaArn":
                                "arn:aws:lambda:us-east-1:000000000000:function:mcp-tools",
                            "toolSchema": {"inlinePayload": [{
                                "name": "echo",
                                "description": "echo a string back",
                                "inputSchema": {"type": "object"}
                            }]}
                        }
                    }
                }
            }),
        ),
    ]
}

/// A gateway still in its transient status takes no target, and the same request succeeds
/// once the clock-owed transition has settled it to `READY`.
#[tokio::test]
async fn a_gateway_takes_a_target_only_once_it_reports_ready() {
    let router = router();

    for (index, (case, body)) in target_bodies().into_iter().enumerate() {
        // One gateway per row, so each row meets a gateway that was just created.
        let gateway_id = create_gateway(&router, &format!("statusgate{index}")).await;

        let (status, refused) = create_target(&router, &gateway_id, &body).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {case:?}: a gateway that is still CREATING must refuse the target with HTTP 400 \
             ValidationException, got {status} {} ({refused})",
            error_code(&refused)
        );
        assert_eq!(
            text(&refused, "message"),
            "CreateGatewayTarget operation can't be performed on gateway when it is in Creating \
             state. Retry the request when the gateway is in a valid state.",
            "case {case:?}: the refusal names the operation and the status to wait out, got {}",
            text(&refused, "message")
        );
        assert!(
            refused.get("targetId").is_none(),
            "case {case:?}: a refused create mints nothing, got {refused}"
        );

        // The gateway is READY once the transition it was owed comes due, and then the
        // very same request is accepted and reads back.
        tokio::time::sleep(PAST_SETTLE).await;
        let (status, gateway) = send(
            &router,
            "GET",
            &format!("/gateways/{gateway_id}/"),
            None,
        )
        .await;
        assert_eq!(
            (status, text(&gateway, "status").as_str()),
            (200, "READY"),
            "case {case:?}: the gateway must settle to READY, got {status} {} ({gateway})",
            text(&gateway, "status")
        );

        let (status, created) = create_target(&router, &gateway_id, &body).await;
        assert_eq!(
            status, 202,
            "case {case:?}: a READY gateway must accept the target, got {status} ({created})"
        );
        let target_id = text(&created, "targetId");
        let (status, fetched) = send(
            &router,
            "GET",
            &format!("/gateways/{gateway_id}/targets/{target_id}/"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the accepted target must read back, got {status} ({fetched})"
        );
        assert_eq!(
            fetched["targetConfiguration"], body["targetConfiguration"],
            "case {case:?}: the target read back must carry the configuration sent, got {} for {}",
            fetched["targetConfiguration"], body["targetConfiguration"]
        );
    }
}

/// The status gate is the GATEWAY's, not the target's: a target created on a settled
/// gateway is itself `CREATING`, and that does not stop the next target.
#[tokio::test]
async fn a_targets_own_transient_status_does_not_gate_the_next_target() {
    let router = router();
    let gateway_id = create_gateway(&router, "targetstatus").await;
    tokio::time::sleep(PAST_SETTLE).await;

    for (case, mut body) in target_bodies() {
        body["name"] = json!(format!("{}-two", text(&body, "name")));
        let (status, created) = create_target(&router, &gateway_id, &body).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 on a READY gateway, got {status} ({created})"
        );
        assert_eq!(
            text(&created, "status"),
            "CREATING",
            "case {case:?}: a new target is CREATING, got {}",
            text(&created, "status")
        );
    }
}

/// The gate is the gateway's STATUS, not the one status the recording happened to show: a
/// gateway whose deletion has been ACCEPTED reports `DELETING` and takes no target either,
/// and the refusal names that status.
#[tokio::test]
async fn a_gateway_on_its_way_out_takes_no_target() {
    let router = router();
    let gateway_id = create_gateway(&router, "deletinggate").await;
    tokio::time::sleep(PAST_SETTLE).await;

    // `DeleteGateway` is `DELETE /gateways/{gatewayIdentifier}/` (HTTP 202,
    // `smithy_query(query="operation",operation="DeleteGateway")`): it accepts the
    // deletion and the gateway reports `DELETING` until the clock removes the record.
    let (status, deleted) = send(
        &router,
        "DELETE",
        &format!("/gateways/{gateway_id}/"),
        None,
    )
    .await;
    assert_eq!(
        (status, text(&deleted, "status").as_str()),
        (202, "DELETING"),
        "setup: DeleteGateway must accept the deletion and report DELETING, got {status} {} \
         ({deleted})",
        text(&deleted, "status")
    );

    for (case, body) in target_bodies() {
        let (status, refused) = create_target(&router, &gateway_id, &body).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {case:?}: a gateway on its way out must refuse the target with HTTP 400 \
             ValidationException, got {status} {} ({refused})",
            error_code(&refused)
        );
        assert_eq!(
            text(&refused, "message"),
            "CreateGatewayTarget operation can't be performed on gateway when it is in Deleting \
             state. Retry the request when the gateway is in a valid state.",
            "case {case:?}: the refusal names the status it found, got {}",
            text(&refused, "message")
        );
    }
}

/// An execution role is refused only on what IAM answered. With no IAM mounted there is
/// nothing to ask, so `CreateAgentRuntime` writes rather than refusing.
#[tokio::test]
async fn an_execution_role_is_refused_only_on_what_iam_answered() {
    let router = router();

    // (case, roleArn) — the rows differ along the account the ARN names, the path the
    // role sits under, the partition, and whether the account field is present at all
    // (`RoleArn`'s pattern makes it optional).
    let rows: [(&str, &str); 5] = [
        ("a role of this account", ROLE_ARN),
        (
            "a role of this account under a path",
            "arn:aws:iam::000000000000:role/service-role/AgentCoreRole",
        ),
        (
            "a role of another account",
            "arn:aws:iam::123456789012:role/AgentCoreRole",
        ),
        (
            "a role in another partition",
            "arn:aws-us-gov:iam::000000000000:role/AgentCoreRole",
        ),
        ("a role ARN carrying no account", "arn:aws:iam:::role/AgentCoreRole"),
    ];

    for (index, (case, role_arn)) in rows.into_iter().enumerate() {
        let runtime_name = format!("ungrounded_role_{index}");
        let (status, created) = send(
            &router,
            "PUT",
            "/runtimes/",
            Some(&json!({
                "agentRuntimeName": runtime_name,
                "agentRuntimeArtifact": {"containerConfiguration": {
                    "containerUri": "public.ecr.aws/agentcore/demo:v1"
                }},
                "networkConfiguration": {"networkMode": "PUBLIC"},
                "roleArn": role_arn,
            })),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: with no IAM to ask, CreateAgentRuntime must write rather than refuse, \
             got {status} ({created})"
        );

        let runtime_id = text(&created, "agentRuntimeId");
        let (status, fetched) = send(
            &router,
            "GET",
            &format!("/runtimes/{runtime_id}/"),
            None,
        )
        .await;
        assert_eq!(
            (status, text(&fetched, "roleArn").as_str()),
            (200, role_arn),
            "case {case:?}: the stored runtime must carry the role it was given, got {status} {} \
             ({fetched})",
            text(&fetched, "roleArn")
        );

        let (status, gateway) = send(
            &router,
            "POST",
            "/gateways/",
            Some(&json!({
                "name": format!("ungrounded-role-{index}"),
                "roleArn": role_arn,
                "authorizerType": "AWS_IAM",
            })),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: CreateGateway stores the role as given, got {status} ({gateway})"
        );
    }
}

/// A request addressing a resource that is not there is refused 404, and the refusal is
/// worded the way this service words a resource that does not exist.
#[tokio::test]
async fn a_resource_that_is_not_there_is_refused_in_this_services_words() {
    let router = router();

    /// An identifier inside `GatewayIdentifier`'s pattern
    /// `^([0-9a-z][-]?){1,100}-[0-9a-z]{10}$` that names no gateway (`smithy_query
    /// operation=GetGateway`).
    const ABSENT_GATEWAY: &str = "absentgateway-abcdefghij";
    /// An id inside `TargetId`'s pattern `^[0-9a-zA-Z]{10}$` that names no target.
    const ABSENT_TARGET: &str = "abcd123456";

    /// What a `ResourceNotFoundException` of this service says for `resource`.
    ///
    /// The verb is "retrieve" on EVERY operation: what failed is the lookup, so the
    /// sentence does not vary with what the caller was trying to do.
    fn expected(resource: &str) -> String {
        format!(
            "Failed to retrieve {resource} because it doesn't exist. Retry the request with a \
             different resource identifier."
        )
    }

    // A gateway that IS there, settled, so the rows that address a target of an existing
    // gateway are missing only the target.
    let gateway_id = create_gateway(&router, "notfoundgate").await;
    tokio::time::sleep(PAST_SETTLE).await;

    let update_body = gateway_body("notfoundgate");
    let (_, target_body) = &target_bodies()[0];
    // An `UpdateGatewayTarget` body that is itself VALID, so these rows reach the lookup
    // the refusal is about rather than the body validation that precedes it.
    let target_update_body = json!({
        "name": "openapi",
        "targetConfiguration": {
            "mcp": {"openApiSchema": {"inlinePayload": "{\"openapi\":\"3.0.0\"}"}}
        },
        "credentialProviderConfigurations": [{"credentialProviderType": "GATEWAY_IAM_ROLE"}],
    });

    // (case, method, uri, body, expected message) — one row per operation that addresses
    // one of these resources, so the wording cannot be one operation's.
    let rows: [(&str, &str, String, Option<&Value>, String); 9] = [
        (
            "GetGateway on a gateway that is not there",
            "GET",
            format!("/gateways/{ABSENT_GATEWAY}/"),
            None,
            expected("gateway"),
        ),
        (
            "UpdateGateway on a gateway that is not there",
            "PUT",
            format!("/gateways/{ABSENT_GATEWAY}/"),
            Some(&update_body),
            expected("gateway"),
        ),
        (
            "DeleteGateway on a gateway that is not there",
            "DELETE",
            format!("/gateways/{ABSENT_GATEWAY}/"),
            None,
            expected("gateway"),
        ),
        (
            "ListGatewayTargets for a gateway that is not there",
            "GET",
            format!("/gateways/{ABSENT_GATEWAY}/targets/"),
            None,
            expected("gateway"),
        ),
        (
            "CreateGatewayTarget on a gateway that is not there",
            "POST",
            format!("/gateways/{ABSENT_GATEWAY}/targets/"),
            Some(target_body),
            expected("gateway"),
        ),
        (
            "GetGatewayTarget on a gateway that is not there",
            "GET",
            format!("/gateways/{ABSENT_GATEWAY}/targets/{ABSENT_TARGET}/"),
            None,
            expected("gateway"),
        ),
        (
            "GetGatewayTarget for a target the gateway does not hold",
            "GET",
            format!("/gateways/{gateway_id}/targets/{ABSENT_TARGET}/"),
            None,
            expected("target"),
        ),
        (
            "UpdateGatewayTarget on a gateway that is not there",
            "PUT",
            format!("/gateways/{ABSENT_GATEWAY}/targets/{ABSENT_TARGET}/"),
            Some(&target_update_body),
            expected("gateway"),
        ),
        (
            "UpdateGatewayTarget for a target the gateway does not hold",
            "PUT",
            format!("/gateways/{gateway_id}/targets/{ABSENT_TARGET}/"),
            Some(&target_update_body),
            expected("target"),
        ),
    ];

    for (case, method, uri, body, expected) in rows {
        let (status, refused) = send(&router, method, &uri, body).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected HTTP 404 ResourceNotFoundException, got {status} {} \
             ({refused})",
            error_code(&refused)
        );
        assert_eq!(
            text(&refused, "message"),
            expected,
            "case {case:?}: expected the refusal {expected:?}, got {}",
            text(&refused, "message")
        );
        assert!(
            !text(&refused, "message").contains(ABSENT_GATEWAY)
                && !text(&refused, "message").contains(ABSENT_TARGET),
            "case {case:?}: the refusal echoes none of the caller's identifiers, got {}",
            text(&refused, "message")
        );
    }
}
