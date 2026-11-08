// SPDX-License-Identifier: CC0-1.0

//! A `COGNITO_USER_POOLS` authorizer in front of a deployed REST method, over the ENDPOINT
//! the user pool minted the presented token on — driven through the whole assembled router.
//!
//! Two services and one engine, so this is a router suite: the tokens are minted by the
//! Amazon Cognito user pools service (signing name `cognito-idp`) and the gate that reads
//! them is API Gateway's own `execute-api` data plane.
//!
//! The RULE the rows assert, one level above any single deployment:
//!
//! > A user-pool token's `iss` claim is the user-pool endpoint the request that minted it
//! > was served on, with the pool's id appended. An authorizer resolves the pool a token
//! > claims by that whole issuer, so it admits a token minted on ANY endpoint this
//! > emulator serves the pool at, and the claims it publishes reach the integration
//! > verbatim — `iss` included.
//!
//! The rows span what the rule generalises over: WHICH endpoint minted the token (the
//! pool's published regional endpoint, and the emulator's own address, which is the only
//! endpoint a client on this machine resolves), and WHICH token of the sign-in is
//! presented (the identity token and the access token).
//!
//! Assembling the router costs stack, so it runs on an explicitly sized thread for the same
//! reason `boot_serve.rs` does.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Keep this at or above the size `boot_serve.rs` uses.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every call is signed for, so the region of the pool and of the api.
const REGION: &str = "us-west-2";

/// This service's published endpoint host in [`REGION`] — the address a client that
/// resolves AWS's own hostnames reaches the pool at.
fn published_host() -> String {
    format!("cognito-idp.{REGION}.amazonaws.com")
}

/// The emulator's own endpoint: the address a client on this machine is configured with,
/// which resolves no AWS hostname at all.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// A permanent password satisfying a pool's default policy, written for this suite.
const GOOD_PASSWORD: &str = "Str0ng-Passw0rd!";

/// The stage the api is deployed to.
const STAGE: &str = "prod";

/// The header the authorizer's identity source names.
const TOKEN_HEADER: &str = "Authorization";

/// What the `MOCK` integration behind the gated method answers: the issuer the authorizer
/// published, which is how a row observes the claim reaching the integration as minted.
const ISSUER_TEMPLATE: &str = "{\"iss\":\"$context.authorizer.claims.iss\"}";

/// One request dispatched through the router, and what came back.
struct Answer {
    status: u16,
    body: Value,
    text: String,
}

/// Dispatch one request with the host, path, headers and body given.
async fn dispatch(
    router: &MultiServiceRouter,
    host: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    let body = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    Answer { status, body, text }
}

/// A credential scope naming `signing_name`, which is what routes a call to that service.
fn credential_scope(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 \
         Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One awsJson1_1 call to the user pools service, on the endpoint `host` addresses, which
/// must succeed.
async fn cognito(
    router: &MultiServiceRouter,
    host: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let target = format!("AWSCognitoIdentityProviderService.{operation}");
    let authorization = credential_scope("cognito-idp");
    let answer = dispatch(
        router,
        host,
        "POST",
        "/",
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("x-amz-target", &target),
            ("authorization", &authorization),
        ],
        body.to_string().into_bytes(),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "setup: cognito-idp {operation} on {host} must answer 200, got {} {}",
        answer.status, answer.text
    );
    answer.body
}

/// One API Gateway REST control-plane call, which must answer `expected_status`.
async fn control_plane(
    router: &MultiServiceRouter,
    what: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected_status: u16,
) -> Value {
    let authorization = credential_scope("apigateway");
    let answer = dispatch(
        router,
        &format!("apigateway.{REGION}.amazonaws.com"),
        method,
        path,
        &[
            ("content-type", "application/json"),
            ("authorization", &authorization),
        ],
        body.map(|body| body.to_string().into_bytes())
            .unwrap_or_default(),
    )
    .await;
    assert_eq!(
        answer.status, expected_status,
        "setup: {what} must answer HTTP {expected_status}, got {} {}",
        answer.status, answer.text
    );
    answer.body
}

/// A required non-empty string member of a response body.
fn as_str(what: &str, body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| panic!("{what} must carry a non-empty {member:?}, got {body}"))
}

/// One claim of a minted token, as a string: its second segment is base64url without
/// padding (RFC 7515 §2), which is RFC 4648 §5 with the two alphabet substitutions.
fn claim(token: &str, name: &str) -> String {
    let segment = token
        .split('.')
        .nth(1)
        .unwrap_or_else(|| panic!("a minted token is a compact JWS, got {token:?}"));
    let mut standard = segment.replace('-', "+").replace('_', "/");
    while !standard.len().is_multiple_of(4) {
        standard.push('=');
    }
    let bytes = aws_smithy_types::base64::decode(&standard)
        .unwrap_or_else(|error| panic!("a token's claims segment must be base64url ({error})"));
    let claims: Value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("a token's claims segment must be JSON ({error})"));
    claims
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a minted token carries a string {name:?} claim, got {claims}"))
        .to_string()
}

#[test]
fn a_cognito_authorizer_admits_a_token_minted_on_the_endpoint_its_holder_reached() {
    std::thread::Builder::new()
        .name("cognito-authorizer-issuer-endpoint".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("build a current-thread runtime");
            runtime.block_on(async {
                let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
                let mut router = build_router(engine);
                // Signature verification belongs to the emulator, not to this check.
                router.set_request_authentication(false);

                // 1. One pool, one app client and one confirmed user, created on the
                //    published endpoint — so what an issuer names below is the endpoint of
                //    the SIGN-IN rather than of the create.
                let published = published_host();
                let pool = cognito(
                    &router,
                    &published,
                    "CreateUserPool",
                    &json!({ "PoolName": "issuer-endpoint-gate" }),
                )
                .await;
                let pool_id = as_str("CreateUserPool", &pool["UserPool"], "Id");
                let pool_arn = as_str("CreateUserPool", &pool["UserPool"], "Arn");
                let client = cognito(
                    &router,
                    &published,
                    "CreateUserPoolClient",
                    &json!({
                        "UserPoolId": &pool_id,
                        "ClientName": "issuer-endpoint-client",
                        "ExplicitAuthFlows": [
                            "ALLOW_ADMIN_USER_PASSWORD_AUTH",
                            "ALLOW_REFRESH_TOKEN_AUTH",
                        ],
                    }),
                )
                .await;
                let client_id = as_str("CreateUserPoolClient", &client["UserPoolClient"], "ClientId");
                cognito(
                    &router,
                    &published,
                    "AdminCreateUser",
                    &json!({ "UserPoolId": &pool_id, "Username": "gate-user" }),
                )
                .await;
                cognito(
                    &router,
                    &published,
                    "AdminSetUserPassword",
                    &json!({
                        "UserPoolId": &pool_id,
                        "Username": "gate-user",
                        "Password": GOOD_PASSWORD,
                        "Permanent": true,
                    }),
                )
                .await;

                // 2. The api: one COGNITO_USER_POOLS-gated MOCK method over that pool,
                //    answering the issuer the authorizer published, deployed to a stage.
                let api = control_plane(
                    &router,
                    "CreateRestApi",
                    "POST",
                    "/restapis",
                    Some(json!({ "name": "issuer-endpoint-api" })),
                    201,
                )
                .await;
                let api_id = as_str("CreateRestApi", &api, "id");
                let root = as_str("CreateRestApi", &api, "rootResourceId");
                let authorizer = control_plane(
                    &router,
                    "CreateAuthorizer",
                    "POST",
                    &format!("/restapis/{api_id}/authorizers"),
                    Some(json!({
                        "name": "by-pool",
                        "type": "COGNITO_USER_POOLS",
                        "providerARNs": [pool_arn],
                        "identitySource": format!("method.request.header.{TOKEN_HEADER}"),
                    })),
                    201,
                )
                .await;
                let authorizer_id = as_str("CreateAuthorizer", &authorizer, "id");
                let resource = control_plane(
                    &router,
                    "CreateResource",
                    "POST",
                    &format!("/restapis/{api_id}/resources/{root}"),
                    Some(json!({ "pathPart": "issuer" })),
                    201,
                )
                .await;
                let resource_id = as_str("CreateResource", &resource, "id");
                control_plane(
                    &router,
                    "PutMethod",
                    "PUT",
                    &format!("/restapis/{api_id}/resources/{resource_id}/methods/GET"),
                    Some(json!({
                        "authorizationType": "COGNITO_USER_POOLS",
                        "authorizerId": authorizer_id,
                    })),
                    201,
                )
                .await;
                control_plane(
                    &router,
                    "PutIntegration",
                    "PUT",
                    &format!("/restapis/{api_id}/resources/{resource_id}/methods/GET/integration"),
                    Some(json!({ "type": "MOCK" })),
                    201,
                )
                .await;
                control_plane(
                    &router,
                    "PutMethodResponse",
                    "PUT",
                    &format!("/restapis/{api_id}/resources/{resource_id}/methods/GET/responses/200"),
                    Some(json!({})),
                    201,
                )
                .await;
                control_plane(
                    &router,
                    "PutIntegrationResponse",
                    "PUT",
                    &format!(
                        "/restapis/{api_id}/resources/{resource_id}/methods/GET/integration/responses/200"
                    ),
                    Some(json!({ "responseTemplates": { "application/json": ISSUER_TEMPLATE } })),
                    201,
                )
                .await;
                control_plane(
                    &router,
                    "CreateDeployment",
                    "POST",
                    &format!("/restapis/{api_id}/deployments"),
                    Some(json!({ "stageName": STAGE })),
                    201,
                )
                .await;

                // 3. The rows: each endpoint a caller signs in on, and each signed token of
                //    that sign-in.
                let endpoints: [(&str, &str); 2] = [
                    ("the pool's published regional endpoint", published.as_str()),
                    ("the emulator's own endpoint", EMULATOR_HOST),
                ];
                let invoke_host = format!("{api_id}.execute-api.{REGION}.amazonaws.com");
                for (case, host) in endpoints {
                    let signed_in = cognito(
                        &router,
                        host,
                        "AdminInitiateAuth",
                        &json!({
                            "UserPoolId": &pool_id,
                            "ClientId": &client_id,
                            "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
                            "AuthParameters": {
                                "USERNAME": "gate-user",
                                "PASSWORD": GOOD_PASSWORD,
                            },
                        }),
                    )
                    .await;
                    let result = &signed_in["AuthenticationResult"];
                    let expected_issuer = format!("https://{host}/{pool_id}");
                    for member in ["IdToken", "AccessToken"] {
                        let token = result[member]
                            .as_str()
                            .unwrap_or_else(|| panic!("case {case:?}: a sign-in returns {member}"));
                        let issuer = claim(token, "iss");
                        assert_eq!(
                            issuer, expected_issuer,
                            "case {case:?} / {member}: expected iss {expected_issuer:?}, \
                             got {issuer:?}"
                        );

                        let answer = dispatch(
                            &router,
                            &invoke_host,
                            "GET",
                            &format!("/{STAGE}/issuer"),
                            &[(TOKEN_HEADER, token)],
                            Vec::new(),
                        )
                        .await;
                        assert_eq!(
                            answer.status, 200,
                            "case {case:?} / {member}: the authorizer must admit a token the \
                             pool minted on the endpoint its holder reached, got {} {}",
                            answer.status, answer.text
                        );
                        let published_claim = answer
                            .body
                            .get("iss")
                            .and_then(Value::as_str)
                            .unwrap_or("<no iss>");
                        assert_eq!(
                            published_claim, issuer,
                            "case {case:?} / {member}: the integration must receive the iss \
                             claim as minted ({issuer:?}), got {published_claim:?} in {}",
                            answer.text
                        );
                    }
                }
            });
        })
        .expect("spawn the sized thread")
        .join()
        .expect("the suite thread must not panic");
}
