// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the gateway lifecycle (`CreateGateway`, `GetGateway`,
//! `CreateGatewayTarget`, `GetGatewayTarget`).
//!
//! Every test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handlers are all exercised. The store is the one shared
//! engine, so what one request writes is what the next request reads.
//!
//! The cross-service half of `CreateGatewayTarget` — a Lambda MCP target resolved against
//! the emulator's own Lambda — is pinned in
//! `core/router/tests/all/bedrock_agentcore_gateway_lambda_target.rs`, the only place both
//! services can be mounted together. Here Lambda is NOT mounted, which is the other half
//! of that rule: with no Lambda to ask, nothing is known about the function and the target
//! is written rather than refused on an assumption.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region the tests sign for unless a case names another.
const REGION: &str = "us-east-1";

/// The account id every emulated ARN carries (`engine::mock_account_id`).
const ACCOUNT: &str = "000000000000";

/// The settle span `src/state.rs` gives the `CREATING` -> `READY` transition, plus slack,
/// so a test can wait the clock out.
const PAST_SETTLE: Duration = Duration::from_millis(900);

/// A role ARN inside `RoleArn`'s pattern `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/GatewayRole";

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service in `region`. It is what the router
/// routes on, and what the handlers read the ARN region from.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/bedrock-agentcore/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    region: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization(region))
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

async fn create_gateway(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    send(router, "POST", "/gateways/", REGION, Some(body)).await
}

async fn get_gateway(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    send(router, "GET", &format!("/gateways/{id}/"), REGION, None).await
}

async fn create_target(router: &MultiServiceRouter, id: &str, body: &Value) -> (u16, Value) {
    send(
        router,
        "POST",
        &format!("/gateways/{id}/targets/"),
        REGION,
        Some(body),
    )
    .await
}

async fn get_target(router: &MultiServiceRouter, id: &str, target: &str) -> (u16, Value) {
    send(
        router,
        "GET",
        &format!("/gateways/{id}/targets/{target}/"),
        REGION,
        None,
    )
    .await
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

/// `GatewayId`'s pattern, `^([0-9a-z][-]?){1,100}-[0-9a-z]{10}$`
/// (`smithy_query shape=GatewayId`), checked by hand because this crate carries no regex
/// engine: lower-case alphanumerics with single dashes between them, ending in a dash and
/// ten lower-case alphanumerics.
fn is_modelled_gateway_id(id: &str) -> bool {
    let Some((stem, suffix)) = id.rsplit_once('-') else {
        return false;
    };
    let suffix_ok =
        suffix.len() == 10 && suffix.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    let stem_ok = !stem.is_empty()
        && !stem.starts_with('-')
        && !stem.ends_with("--")
        && stem
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    suffix_ok && stem_ok
}

/// `TargetId`'s pattern, `^[0-9a-zA-Z]{10}$` (`smithy_query shape=TargetId`).
fn is_modelled_target_id(id: &str) -> bool {
    id.len() == 10 && id.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// A `CreateGateway` body for `name` with `authorizer_type`, and its configuration when
/// the type needs one.
fn gateway_body(name: &str, authorizer_type: &str, with_configuration: bool) -> Value {
    let mut body = json!({
        "name": name,
        "roleArn": ROLE_ARN,
        "authorizerType": authorizer_type,
    });
    if with_configuration {
        body["authorizerConfiguration"] = json!({
            "customJWTAuthorizer": {
                "discoveryUrl": "https://example.com/.well-known/openid-configuration",
                "allowedClients": ["client-1"]
            }
        });
    }
    body
}

/// An MCP Lambda target body: the one target shape that names a Lambda function.
fn lambda_target_body(name: &str, lambda_arn: &str) -> Value {
    json!({
        "name": name,
        "targetConfiguration": {
            "mcp": {
                "lambda": {
                    "lambdaArn": lambda_arn,
                    "toolSchema": {
                        "inlinePayload": [{
                            "name": "echo",
                            "description": "echo a string back",
                            "inputSchema": {"type": "object"}
                        }]
                    }
                }
            }
        }
    })
}

/// A gateway is created from the request and read back through `GetGateway`, for every
/// `authorizerType` the model declares.
///
/// The rows differ along the authorizer, which is the one member whose value changes what
/// the request must carry: `CUSTOM_JWT` needs a configuration, the other three do not.
/// Source for the value list: `AuthorizerType`'s enum
/// (`shared_types::AUTHORIZER_TYPE_VALUES`, from `smithy_query shape=AuthorizerType`).
#[tokio::test]
async fn a_gateway_is_created_and_read_back_for_every_authorizer_type() {
    let router = router();

    // (case, authorizerType, carries a configuration)
    let rows: [(&str, &str, bool); 4] = [
        ("AWS_IAM, no configuration", "AWS_IAM", false),
        ("NONE, no configuration", "NONE", false),
        ("AUTHENTICATE_ONLY, no configuration", "AUTHENTICATE_ONLY", false),
        ("CUSTOM_JWT with its configuration", "CUSTOM_JWT", true),
    ];

    for (index, (case, authorizer_type, with_configuration)) in rows.into_iter().enumerate() {
        let name = format!("gw{index}");
        let body = gateway_body(&name, authorizer_type, with_configuration);
        let (status, created) = create_gateway(&router, &body).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from CreateGateway, got {status} ({created})"
        );

        let id = text(&created, "gatewayId");
        assert!(
            is_modelled_gateway_id(&id),
            "case {case:?}: expected a gatewayId matching ^([0-9a-z][-]?){{1,100}}-[0-9a-z]{{10}}$, \
             got {id:?}"
        );
        assert_eq!(
            text(&created, "gatewayArn"),
            format!("arn:aws:bedrock-agentcore:{REGION}:{ACCOUNT}:gateway/{id}"),
            "case {case:?}: expected the modelled GatewayArn for {id}, got {}",
            text(&created, "gatewayArn")
        );
        assert_eq!(
            text(&created, "gatewayUrl"),
            format!("https://{id}.gateway.bedrock-agentcore.{REGION}.amazonaws.com/mcp"),
            "case {case:?}: expected the assigned gateway host plus the MCP transport path, \
             got {}",
            text(&created, "gatewayUrl")
        );
        assert_eq!(
            text(&created, "status"),
            "CREATING",
            "case {case:?}: a gateway is created in CREATING, got {}",
            text(&created, "status")
        );
        assert_eq!(
            text(&created, "protocolType"),
            "MCP",
            "case {case:?}: expected the modelled MCP protocolType default, got {}",
            text(&created, "protocolType")
        );
        assert_eq!(
            text(&created, "authorizerType"),
            authorizer_type,
            "case {case:?}: expected the authorizerType the request carried, got {}",
            text(&created, "authorizerType")
        );
        assert_eq!(
            created["authorizerConfiguration"].is_null(),
            !with_configuration,
            "case {case:?}: expected the authorizerConfiguration to be reported only when the \
             request carried one, got {}",
            created["authorizerConfiguration"]
        );
        assert!(
            created["workloadIdentityDetails"]["workloadIdentityArn"]
                .as_str()
                .unwrap_or_default()
                .starts_with(&format!(
                    "arn:aws:bedrock-agentcore:{REGION}:{ACCOUNT}:workload-identity-directory/"
                )),
            "case {case:?}: expected a workload identity ARN, got {}",
            created["workloadIdentityDetails"]
        );

        // The round trip: every member the create reported is what the read reports.
        let (status, fetched) = get_gateway(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetGateway, got {status} ({fetched})"
        );
        for member in [
            "gatewayArn",
            "gatewayId",
            "gatewayUrl",
            "createdAt",
            "name",
            "roleArn",
            "protocolType",
            "authorizerType",
            "authorizerConfiguration",
            "workloadIdentityDetails",
        ] {
            assert_eq!(
                fetched[member], created[member],
                "case {case:?}: GetGateway must report the {member} CreateGateway stored, got {} \
                 for {}",
                fetched[member], created[member]
            );
        }

        // The status is the one the clock owes, materialised on a read and not by it:
        // CREATING until the span passes, READY afterwards, for every later read.
        tokio::time::sleep(PAST_SETTLE).await;
        let (_, settled) = get_gateway(&router, &id).await;
        assert_eq!(
            text(&settled, "status"),
            "READY",
            "case {case:?}: after the settle span the gateway is READY, got {}",
            text(&settled, "status")
        );
        let (_, again) = get_gateway(&router, &id).await;
        assert_eq!(
            text(&again, "status"),
            "READY",
            "case {case:?}: a second read reports the same status, got {}",
            text(&again, "status")
        );
    }
}

/// The minted `gatewayId` is inside `GatewayId`'s pattern for every name
/// `GatewayName`'s pattern admits — including the forms the two patterns disagree on:
/// upper case, and a name ending in a dash (the id's own separator follows it, and no
/// two dashes may be adjacent).
#[tokio::test]
async fn a_minted_gateway_id_is_modelled_for_every_name_shape() {
    let router = router();

    // (case, the GatewayName sent) — each inside `^([0-9a-zA-Z][-]?){1,48}$`
    // (`smithy_query shape=GatewayName`).
    let rows: [(&str, &str); 5] = [
        ("all lower case", "plaingateway"),
        ("mixed case, which the id must lower", "MixedCaseGateway"),
        ("dashes between characters", "my-mcp-gateway"),
        ("a name ending in a dash", "trailing-dash-"),
        ("a single character", "g"),
    ];

    for (case, name) in rows {
        let (status, created) =
            create_gateway(&router, &gateway_body(name, "AWS_IAM", false)).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from CreateGateway, got {status} ({created})"
        );
        let id = text(&created, "gatewayId");
        assert!(
            is_modelled_gateway_id(&id),
            "case {case:?} (name {name:?}): expected a gatewayId matching \
             ^([0-9a-z][-]?){{1,100}}-[0-9a-z]{{10}}$, got {id:?}"
        );
        assert!(
            !id.contains("--"),
            "case {case:?} (name {name:?}): the id's pattern admits no two adjacent dashes, \
             got {id:?}"
        );
        assert_eq!(
            text(&created, "gatewayArn"),
            format!("arn:aws:bedrock-agentcore:{REGION}:{ACCOUNT}:gateway/{id}"),
            "case {case:?}: the ARN carries the minted id, got {}",
            text(&created, "gatewayArn")
        );
        // The gateway reads back under the id it minted, whatever the name's shape.
        let (status, fetched) = get_gateway(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetGateway on {id}, got {status} ({fetched})"
        );
        assert_eq!(
            text(&fetched, "name"),
            name,
            "case {case:?}: the name reads back as sent, got {}",
            text(&fetched, "name")
        );
    }
}

/// What `CreateGateway` refuses, and with which modelled error.
#[tokio::test]
async fn create_gateway_refuses_what_the_model_states_it_must() {
    let router = router();

    // The gateway a conflict row collides with.
    let (status, _) = create_gateway(&router, &gateway_body("taken", "AWS_IAM", false)).await;
    assert_eq!(status, 202, "the gateway a later case collides with is created");

    // (case, body, expected status, expected error code)
    let rows: [(&str, Value, u16, &str); 4] = [
        (
            // The model: "If you specify CUSTOM_JWT as the authorizerType, you must
            // provide an authorizerConfiguration."
            "CUSTOM_JWT without an authorizerConfiguration",
            gateway_body("jwtless", "CUSTOM_JWT", false),
            400,
            "ValidationException",
        ),
        (
            "a name another gateway already has",
            gateway_body("taken", "AWS_IAM", false),
            409,
            "ConflictException",
        ),
        (
            "a name outside GatewayName's pattern",
            gateway_body("not a valid name!", "AWS_IAM", false),
            400,
            "ValidationException",
        ),
        (
            "a roleArn outside RoleArn's pattern",
            json!({"name": "badrole", "roleArn": "not-an-arn", "authorizerType": "AWS_IAM"}),
            400,
            "ValidationException",
        ),
    ];

    for (case, body, expected_status, expected_code) in rows {
        let (status, answer) = create_gateway(&router, &body).await;
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected HTTP {expected_status} {expected_code}, got {status} {} \
             ({answer})",
            error_code(&answer)
        );
    }

    // A gateway identifier naming no stored gateway is the modelled 404, not an empty
    // envelope. The identifier itself is inside `GatewayIdentifier`'s pattern, so this is
    // the handler's refusal and not the boundary's.
    let (status, answer) = get_gateway(&router, "no-such-gateway-0123456789").await;
    assert_eq!(
        (status, error_code(&answer).as_str()),
        (404, "ResourceNotFoundException"),
        "case \"GetGateway on an unknown identifier\": expected HTTP 404 \
         ResourceNotFoundException, got {status} {} ({answer})",
        error_code(&answer)
    );
}

/// A repeated `clientToken` replays the first result instead of minting a second gateway.
#[tokio::test]
async fn a_repeated_client_token_replays_the_gateway_it_first_created() {
    let router = router();

    // Inside `ClientToken`'s length (33..256) and pattern
    // `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,256}$`.
    let token = "idempotent-create-gateway-token-0001";
    let mut body = gateway_body("idem", "AWS_IAM", false);
    body["clientToken"] = json!(token);

    let (first_status, first) = create_gateway(&router, &body).await;
    let (second_status, second) = create_gateway(&router, &body).await;
    assert_eq!(
        (first_status, second_status),
        (202, 202),
        "case \"a replayed create\": both calls answer 202, got {first_status} and \
         {second_status} ({second})"
    );
    assert_eq!(
        text(&second, "gatewayId"),
        text(&first, "gatewayId"),
        "case \"a replayed create\": the second call must answer with the first gateway, got {} \
         for {}",
        text(&second, "gatewayId"),
        text(&first, "gatewayId")
    );

    // A DIFFERENT token under the same name is a conflict, not a replay: the name is what
    // must be unique.
    body["clientToken"] = json!("idempotent-create-gateway-token-0002");
    let (status, answer) = create_gateway(&router, &body).await;
    assert_eq!(
        (status, error_code(&answer).as_str()),
        (409, "ConflictException"),
        "case \"a second create under a new token\": expected HTTP 409 ConflictException, got \
         {status} {} ({answer})",
        error_code(&answer)
    );
}

/// Targets are created on a gateway and read back, for target shapes that name a Lambda
/// function and shapes that do not.
///
/// Lambda is not mounted in this router, so the function a Lambda target names cannot be
/// resolved: nothing is known about it and the target is written. (The grounded refusal,
/// where Lambda answers that the function is absent, is the router suite's.)
#[tokio::test]
async fn targets_are_created_on_a_gateway_and_read_back() {
    let router = router();
    let (_, gateway) = create_gateway(&router, &gateway_body("targets", "AWS_IAM", false)).await;
    let gateway_id = text(&gateway, "gatewayId");
    let gateway_arn = text(&gateway, "gatewayArn");
    // A gateway takes no target until the clock-owed `CREATING` -> `READY` transition
    // has come due, so the suite waits it out before creating one. The refusal before
    // that is `control_plane_preconditions.rs`'s.
    tokio::time::sleep(PAST_SETTLE).await;

    // (case, body) — one row per target shape, so the rows differ along the configuration
    // union the request carries. Source: `TargetConfiguration` and
    // `McpTargetConfiguration` (`smithy_query shape=TargetConfiguration,McpTargetConfiguration`).
    let rows: [(&str, Value); 3] = [
        (
            "an MCP Lambda target",
            lambda_target_body(
                "lambda-target",
                "arn:aws:lambda:us-east-1:000000000000:function:mcp-tools",
            ),
        ),
        (
            "an MCP target with an inline OpenAPI schema",
            json!({
                "name": "openapi-target",
                "targetConfiguration": {
                    "mcp": {"openApiSchema": {"inlinePayload": "{\"openapi\":\"3.0.0\"}"}}
                }
            }),
        ),
        (
            "an MCP server target, with a credential provider of its own",
            json!({
                "name": "mcpserver-target",
                "targetConfiguration": {
                    "mcp": {"mcpServer": {"endpoint": "https://tools.example.com/mcp"}}
                },
                "credentialProviderConfigurations": [{"credentialProviderType": "OAUTH"}]
            }),
        ),
    ];

    for (case, body) in rows {
        let (status, created) = create_target(&router, &gateway_id, &body).await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from CreateGatewayTarget, got {status} ({created})"
        );

        let target_id = text(&created, "targetId");
        assert!(
            is_modelled_target_id(&target_id),
            "case {case:?}: expected a targetId matching ^[0-9a-zA-Z]{{10}}$, got {target_id:?}"
        );
        assert_eq!(
            text(&created, "gatewayArn"),
            gateway_arn,
            "case {case:?}: a target reports its gateway's ARN, got {}",
            text(&created, "gatewayArn")
        );
        assert_eq!(
            text(&created, "status"),
            "CREATING",
            "case {case:?}: a target is created in CREATING, got {}",
            text(&created, "status")
        );
        assert_eq!(
            created["targetConfiguration"], body["targetConfiguration"],
            "case {case:?}: the stored configuration is the one sent, got {} for {}",
            created["targetConfiguration"], body["targetConfiguration"]
        );
        // `credentialProviderConfigurations` is optional on the request and required on
        // the response, so a target configured with none reports the gateway's own role.
        let expected_providers = body
            .get("credentialProviderConfigurations")
            .cloned()
            .unwrap_or_else(|| json!([{"credentialProviderType": "GATEWAY_IAM_ROLE"}]));
        assert_eq!(
            created["credentialProviderConfigurations"], expected_providers,
            "case {case:?}: expected credentialProviderConfigurations {expected_providers}, got {}",
            created["credentialProviderConfigurations"]
        );

        let (status, fetched) = get_target(&router, &gateway_id, &target_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetGatewayTarget, got {status} ({fetched})"
        );
        for member in [
            "gatewayArn",
            "targetId",
            "createdAt",
            "name",
            "targetConfiguration",
            "credentialProviderConfigurations",
        ] {
            assert_eq!(
                fetched[member], created[member],
                "case {case:?}: GetGatewayTarget must report the {member} CreateGatewayTarget \
                 stored, got {} for {}",
                fetched[member], created[member]
            );
        }
    }

    // Each target got its own id, so the gateway holds three of them.
    let (status, answer) = get_target(&router, &gateway_id, "0123456789").await;
    assert_eq!(
        (status, error_code(&answer).as_str()),
        (404, "ResourceNotFoundException"),
        "case \"GetGatewayTarget on an unknown target\": expected HTTP 404 \
         ResourceNotFoundException, got {status} {} ({answer})",
        error_code(&answer)
    );

    // A target on a gateway that does not exist, and a target with no name: both are the
    // modelled refusal, and neither is written.
    let (status, answer) = create_target(
        &router,
        "no-such-gateway-0123456789",
        &lambda_target_body(
            "orphan",
            "arn:aws:lambda:us-east-1:000000000000:function:mcp-tools",
        ),
    )
    .await;
    assert_eq!(
        (status, error_code(&answer).as_str()),
        (404, "ResourceNotFoundException"),
        "case \"CreateGatewayTarget on an unknown gateway\": expected HTTP 404 \
         ResourceNotFoundException, got {status} {} ({answer})",
        error_code(&answer)
    );

    let unnamed = json!({
        "targetConfiguration": {
            "mcp": {"openApiSchema": {"inlinePayload": "{\"openapi\":\"3.0.0\"}"}}
        }
    });
    let (status, answer) = create_target(&router, &gateway_id, &unnamed).await;
    assert_eq!(
        (status, error_code(&answer).as_str()),
        (400, "ValidationException"),
        "case \"CreateGatewayTarget with no name\": expected HTTP 400 ValidationException, got \
         {status} {} ({answer})",
        error_code(&answer)
    );
}
