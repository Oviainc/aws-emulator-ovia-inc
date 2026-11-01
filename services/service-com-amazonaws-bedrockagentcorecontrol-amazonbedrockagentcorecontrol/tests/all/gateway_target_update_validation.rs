// SPDX-License-Identifier: CC0-1.0

//! `UpdateGatewayTarget`'s request validation: what it refuses about the BODY, and where
//! that decision sits relative to the resource the path names.
//!
//! Each test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handler are all exercised, against the one shared engine.
//!
//! The rule under test, read off the model
//! (`smithy_query(query="operation",operation="UpdateGatewayTarget")`):
//! `credentialProviderConfigurations` is optional on the request and `@required` with
//! exactly one entry on the response, and the update REPLACES the target's whole
//! configuration — so a body that defines none leaves the response nothing to report and is
//! refused with the operation's modelled `ValidationException` (HTTP 400) in the words this
//! service uses for a member a request did not define. The resource the PATH addresses is
//! resolved first, so that refusal is what a request naming a gateway and a target that are
//! both THERE gets, while a path naming either one that is not is the modelled
//! `ResourceNotFoundException` (404) whatever the body leaves out; a gateway that is there
//! but not in the status that admits a change still gets the body's refusal. A body that
//! DOES define the member is served, and nothing a refused request carried is written.

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

/// The sentence this service gives a request that defined no credential provider
/// configurations: entirely fixed text, naming the member in prose and quoting nothing of
/// the request.
const NOT_DEFINED: &str = "Credential provider configurations is not defined";

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
        .header("host", "bedrock-agentcore-control.us-east-1.amazonaws.com");
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

/// A target configuration union: an MCP target serving an inline OpenAPI schema.
fn openapi_configuration(payload: &str) -> Value {
    json!({"mcp": {"openApiSchema": {"inlinePayload": payload}}})
}

/// Create a gateway, wait the clock-owed `CREATING` -> `READY` transition out, and answer
/// its `gatewayId`.
async fn settled_gateway(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = send(
        router,
        "POST",
        "/gateways/",
        Some(&json!({"name": name, "roleArn": ROLE_ARN, "authorizerType": "AWS_IAM"})),
    )
    .await;
    assert_eq!(
        status, 202,
        "setup {name:?}: CreateGateway must succeed, got {status} ({created})"
    );
    let gateway_id = text(&created, "gatewayId");
    tokio::time::sleep(PAST_SETTLE).await;
    gateway_id
}

/// Create a target on `gateway` and answer its `targetId`.
async fn created_target(router: &MultiServiceRouter, gateway: &str, name: &str) -> String {
    let (status, created) = send(
        router,
        "POST",
        &format!("/gateways/{gateway}/targets/"),
        Some(&json!({
            "name": name,
            "description": "the target before any update",
            "targetConfiguration": openapi_configuration("{\"openapi\":\"3.0.0\"}"),
            "credentialProviderConfigurations": [
                {"credentialProviderType": "CALLER_IAM_CREDENTIALS"}
            ],
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "setup {name:?}: CreateGatewayTarget must succeed, got {status} ({created})"
    );
    text(&created, "targetId")
}

async fn update_target(
    router: &MultiServiceRouter,
    gateway: &str,
    target: &str,
    body: &Value,
) -> (u16, Value) {
    send(
        router,
        "PUT",
        &format!("/gateways/{gateway}/targets/{target}/"),
        Some(body),
    )
    .await
}

/// A body that defines no credential provider configurations is refused 400 whenever the
/// path addresses a target that IS there, whatever else the body carries — and nothing it
/// asked for is written. A path naming a resource that is not there is the modelled 404
/// first, since the resource is resolved before the body is judged.
#[tokio::test]
async fn an_update_that_defines_no_credential_providers_is_refused() {
    let router = router();
    let gateway_id = settled_gateway(&router, "novalidategate").await;
    let target_id = created_target(&router, &gateway_id, "novalidatetarget").await;
    tokio::time::sleep(PAST_SETTLE).await;

    // A second gateway that is NOT in the status a change needs: the row addressing it
    // would otherwise be refused 400 for the gateway's status, with a different sentence.
    let unsettled_gateway = settled_gateway(&router, "unsettledgate").await;
    let unsettled_target = created_target(&router, &unsettled_gateway, "unsettledtarget").await;
    let (status, moved) = send(
        &router,
        "PUT",
        &format!("/gateways/{unsettled_gateway}/"),
        Some(&json!({
            "name": "unsettledgate",
            "roleArn": ROLE_ARN,
            "authorizerType": "AWS_IAM",
        })),
    )
    .await;
    assert_eq!(
        (status, text(&moved, "status").as_str()),
        (202, "UPDATING"),
        "setup: the gateway must be left in UPDATING, got {status} {} ({moved})",
        text(&moved, "status")
    );

    // (case, gatewayIdentifier, targetId, the body the row sends)
    let rows: [(&str, String, String, Value); 4] = [
        (
            "a gateway and a target that are both there",
            gateway_id.clone(),
            target_id.clone(),
            json!({"targetConfiguration": openapi_configuration("{\"openapi\":\"3.1.0\"}")}),
        ),
        (
            "every other configurable member carried, and only this one left out",
            gateway_id.clone(),
            target_id.clone(),
            json!({
                "name": "renamedtarget",
                "description": "a description the refusal must not store",
                "targetConfiguration": openapi_configuration("{\"openapi\":\"3.1.0\"}"),
                "metadataConfiguration": {"allowedRequestHeaders": ["x-after"]},
                "certificateConfigurations": [{"s3": {"uri": "s3://certs/private-ca.pem"}}],
            }),
        ),
        (
            "a gateway the clock still owes a transition",
            unsettled_gateway.clone(),
            unsettled_target.clone(),
            json!({"targetConfiguration": openapi_configuration("{\"openapi\":\"3.1.0\"}")}),
        ),
        (
            "a Lambda configuration naming a function no mounted service resolves",
            gateway_id.clone(),
            target_id.clone(),
            json!({"targetConfiguration": {"mcp": {"lambda": {
                "lambdaArn": "arn:aws:lambda:us-east-1:000000000000:function:absent",
                "toolSchema": {"inlinePayload": [{
                    "name": "echo",
                    "description": "echo a string back",
                    "inputSchema": {"type": "object"},
                }]},
            }}}}),
        ),
    ];

    for (case, gateway, target, body) in rows {
        let (status, refused) = update_target(&router, &gateway, &target, &body).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} {} ({refused})",
            error_code(&refused)
        );
        assert_eq!(
            text(&refused, "message"),
            NOT_DEFINED,
            "case {case:?}: expected the fixed sentence {NOT_DEFINED:?}, got {}",
            text(&refused, "message")
        );
    }

    // The resource the path addresses is resolved BEFORE the body is judged, so a path
    // naming one that is not there answers the modelled 404 even for a body this operation
    // would otherwise refuse: an update of a target that is gone cannot be served whatever
    // the body carries.
    let absent_rows: [(&str, String, String, &str); 2] = [
        (
            "a targetId naming no target of a gateway that is there",
            gateway_id.clone(),
            ABSENT_TARGET_ID.to_string(),
            "Failed to retrieve target because it doesn't exist. Retry the request with a \
             different resource identifier.",
        ),
        (
            "a gatewayIdentifier naming no gateway",
            ABSENT_GATEWAY_ID.to_string(),
            ABSENT_TARGET_ID.to_string(),
            "Failed to retrieve gateway because it doesn't exist. Retry the request with a \
             different resource identifier.",
        ),
    ];

    for (case, gateway, target, message) in absent_rows {
        let body = json!({"targetConfiguration": openapi_configuration("{\"openapi\":\"3.1.0\"}")});
        let (status, refused) = update_target(&router, &gateway, &target, &body).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected HTTP 404 ResourceNotFoundException, got {status} {} \
             ({refused})",
            error_code(&refused)
        );
        assert_eq!(
            text(&refused, "message"),
            message,
            "case {case:?}: the refusal names what it could not find, got {}",
            text(&refused, "message")
        );
    }

    // Nothing a refused row asked for reached the stored target: it still carries the
    // name, description and credential provider the create gave it.
    let (status, fetched) = send(
        &router,
        "GET",
        &format!("/gateways/{gateway_id}/targets/{target_id}/"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "the target must still read back, got {status} ({fetched})"
    );
    assert_eq!(
        text(&fetched, "name"),
        "novalidatetarget",
        "a refused update must not rename the target, got {fetched}"
    );
    assert_eq!(
        text(&fetched, "description"),
        "the target before any update",
        "a refused update must not change the description, got {fetched}"
    );
    assert_eq!(
        fetched["credentialProviderConfigurations"],
        json!([{"credentialProviderType": "CALLER_IAM_CREDENTIALS"}]),
        "a refused update must leave the stored credential providers alone, got {}",
        fetched["credentialProviderConfigurations"]
    );
}

/// The accepted half of the same rule: a body that DOES define the member is served, and
/// the target reports exactly what it defined.
#[tokio::test]
async fn an_update_that_defines_the_credential_providers_is_served() {
    let router = router();
    let gateway_id = settled_gateway(&router, "servedgate").await;

    // One row per `CredentialProviderType` the enum declares
    // (`smithy_query(query="shape",shape="CredentialProviderType")`), since the rule is
    // about the member being DEFINED and not about which provider it names.
    let rows: [(&str, Value); 4] = [
        (
            "the gateway's own IAM role",
            json!([{"credentialProviderType": "GATEWAY_IAM_ROLE"}]),
        ),
        (
            "the caller's IAM credentials",
            json!([{"credentialProviderType": "CALLER_IAM_CREDENTIALS"}]),
        ),
        (
            "a JWT passed through",
            json!([{"credentialProviderType": "JWT_PASSTHROUGH"}]),
        ),
        (
            "an API key provider of its own",
            json!([{
                "credentialProviderType": "API_KEY",
                "credentialProvider": {"apiKeyCredentialProvider": {
                    "providerArn":
                        "arn:aws:bedrock-agentcore:us-east-1:000000000000:token-vault/default/\
                         apikeycredentialprovider/keys",
                    "credentialLocation": "HEADER",
                    "credentialParameterName": "x-api-key",
                }},
            }]),
        ),
    ];

    for (index, (case, providers)) in rows.into_iter().enumerate() {
        let target_id = created_target(&router, &gateway_id, &format!("servedtarget{index}")).await;
        tokio::time::sleep(PAST_SETTLE).await;

        let (status, updated) = update_target(
            &router,
            &gateway_id,
            &target_id,
            &json!({
                "targetConfiguration": openapi_configuration("{\"openapi\":\"3.1.0\"}"),
                "credentialProviderConfigurations": providers.clone(),
            }),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from UpdateGatewayTarget, got {status} ({updated})"
        );
        assert_eq!(
            updated["credentialProviderConfigurations"], providers,
            "case {case:?}: the update must report the providers it was given, got {}",
            updated["credentialProviderConfigurations"]
        );

        let (status, fetched) = send(
            &router,
            "GET",
            &format!("/gateways/{gateway_id}/targets/{target_id}/"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the updated target must read back, got {status} ({fetched})"
        );
        assert_eq!(
            fetched["credentialProviderConfigurations"], providers,
            "case {case:?}: the read must report what the update stored, got {}",
            fetched["credentialProviderConfigurations"]
        );
    }
}
