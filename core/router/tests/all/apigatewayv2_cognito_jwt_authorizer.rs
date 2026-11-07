// SPDX-License-Identifier: CC0-1.0

//! A Cognito-backed JWT authorizer in front of an HTTP api's route, driven end to end
//! through the WHOLE assembled router — the client sequence this issue reported as
//! unusable, because `CreateAuthorizer` answered the scaffold's unmodelled
//! `NotImplemented` and so a JWT-gated route could never exist.
//!
//! Two services and one engine, so this is a router suite: the tokens are minted by the
//! Amazon Cognito user pools service (signing name `cognito-idp`) and the gate that reads
//! them is API Gateway V2's own `execute-api` data plane (control plane signing name
//! `apigateway`). Nothing about a token is written here that a pool did not mint: the
//! admitted leg presents the ACCESS TOKEN `AdminInitiateAuth` answered with, and the
//! refused legs present another client's token, another pool's token, or that same minted
//! token re-encoded with one claim or its JOSE header changed.
//!
//! The rule the rows assert, one level above the recorded case:
//!
//! > `CreateAuthorizer` with `AuthorizerType` `JWT` persists the authorizer document its
//! > `JWTConfiguration` describes and reports it with a minted `AuthorizerId` (HTTP 201),
//! > which a later read reports unchanged; and a deployed route whose `AuthorizationType`
//! > is `JWT` admits a request only when the token at the authorizer's identity source is
//! > a signed JWT whose `iss` is that authorizer's `Issuer` and whose `aud` — or, for an
//! > access token, `client_id` — matches one entry of its `Audience`, and only inside the
//! > token's validity window. Anything else is `401` `{"message":"Unauthorized"}`; a token
//! > the gate accepts whose claimed scopes meet none of the route's `AuthorizationScopes`
//! > is `403` `{"message":"Forbidden"}` ("otherwise, the invocation is not authorized",
//! > `Route$AuthorizationScopes`). A refused request never reaches the route's
//! > integration.
//!
//! The rows span what that rule generalises over: where the token came from (the api's own
//! pool and client, a second client of the same pool, a second pool), how it is presented
//! (RFC 6750 bearer credentials, bare, absent, not a token at all), which claim or header
//! was changed (`exp`, `alg`), how many entries the authorizer's `Audience` carries (one,
//! two) and what the route requires (no scope, a scope the token does not claim, a list
//! one of whose entries it does).
//!
//! What an ADMITTED request is answered with is its integration's answer. An `HTTP_PROXY`
//! upstream is a URI the caller can reach, so the caller is sent there (`307`, with the
//! stored `IntegrationUri` in `Location`) — an answer that names the integration that gave
//! it, which is how these rows observe that the gate let the request THROUGH. The
//! `AWS_PROXY` row the recorded check fronts asserts only that the gate admitted it (the
//! answer is neither of the authorizer's refusals and not a `404`), because what a Lambda
//! integration answers with awaits an in-process runtime for a deployment package, which
//! other open issues own.
//!
//! Assembling the router costs stack, so it runs on an explicitly sized thread for the
//! same reason `boot_serve.rs` does.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Keep this at or above the size `boot_serve.rs` uses.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every call is signed for, and so the region of the pool's issuer and of the
/// api's own endpoint host.
const REGION: &str = "us-west-2";

/// The permanent password this suite's fixture user signs in with — a value written for
/// this test and used nowhere else. It satisfies a pool's default password policy (a
/// length, an upper and a lower case letter, a digit and a symbol), spelled as the user
/// pools suites spell theirs.
const GOOD_PASSWORD: &str = "Str0ng-Passw0rd!";

/// The upstream an `HTTP_PROXY` integration names: a URI the caller is sent to, so the
/// answer to an admitted request names the integration that gave it.
const UPSTREAM: &str = "https://upstream.example.com/guarded";

/// The status and sentence a token the authorizer cannot accept earns, and that nothing
/// the route fronts ran.
const UNAUTHORIZED: u16 = 401;
const UNAUTHORIZED_MESSAGE: &str = "Unauthorized";

/// The status and sentence a token the authorizer ACCEPTS earns at a route requiring a
/// scope it does not claim.
const FORBIDDEN: u16 = 403;
const FORBIDDEN_MESSAGE: &str = "Forbidden";

/// The status an admitted request to a route fronting an `HTTP_PROXY` upstream earns.
const TEMPORARY_REDIRECT: u16 = 307;

/// The status a request matching no route of the api earns — never the answer to a row
/// here, since every row addresses a route the setup created.
const NOT_FOUND: u16 = 404;

/// The error code the front door answers when NO service claims a request.
const UNKNOWN_SERVICE: &str = "UnknownService";

/// A scope no Amazon Cognito access token claims, used as the route scope a token cannot
/// meet. The form is a resource server's `{identifier}/{scope}`.
const UNCLAIMED_SCOPE: &str = "things/read";

/// One request dispatched through the router, and what came back.
struct Answer {
    status: u16,
    error_type: Option<String>,
    /// Where a redirect sends the caller: the header an `HTTP_PROXY` integration's answer
    /// carries.
    location: Option<String>,
    body: Value,
}

impl Answer {
    /// The `message` member of a data-plane body, or `"<no message>"`.
    fn message(&self) -> String {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("<no message>")
            .to_string()
    }

    /// Everything an assertion needs to say about an answer that was not the one expected.
    fn summary(&self) -> String {
        format!(
            "{} {} (x-amzn-errortype: {:?}, location: {:?})",
            self.status, self.body, self.error_type, self.location
        )
    }
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
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .map(|value| String::from_utf8_lossy(value.as_bytes()).to_string())
    };
    let error_type = header("x-amzn-errortype");
    let location = header("location");
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_type,
        location,
        body,
    }
}

/// The credential scope a signed SDK request carries: the key id, the region a handler
/// builds its records in, and the service it is for. The signature is never checked —
/// this router runs with authentication off.
fn credential_scope(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 \
         Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One awsJson1_1 call to the Amazon Cognito user pools service, which must succeed.
async fn cognito(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let target = format!("AWSCognitoIdentityProviderService.{operation}");
    let authorization = credential_scope("cognito-idp");
    let answer = dispatch(
        router,
        &format!("cognito-idp.{REGION}.amazonaws.com"),
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
        answer.status,
        200,
        "setup: cognito-idp {operation} must answer 200, got {}",
        answer.summary()
    );
    answer.body
}

/// One API Gateway V2 control-plane call, which must answer `expected_status`.
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
    assert!(
        !answer.body.to_string().contains("NotImplemented"),
        "{what} must be served, not answered with the scaffold's unmodelled \
         NotImplemented: got {}",
        answer.summary()
    );
    assert_eq!(
        answer.status,
        expected_status,
        "{what} must answer HTTP {expected_status}, got {}",
        answer.summary()
    );
    answer.body
}

/// A required string member of a response body.
fn as_str(what: &str, body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| panic!("{what} must carry a non-empty {member:?}, got {body}"))
}

/// One base64url segment, decoded: "base64url without padding" (RFC 7515 §2), which is
/// RFC 4648 §5 — the two alphabet substitutions and the restored padding turn it back into
/// the standard alphabet this workspace already decodes.
fn decode_segment(segment: &str) -> Vec<u8> {
    let mut standard = segment.replace('-', "+").replace('_', "/");
    while !standard.len().is_multiple_of(4) {
        standard.push('=');
    }
    aws_smithy_types::base64::decode(&standard)
        .unwrap_or_else(|e| panic!("a minted token's segment must be base64url ({e})"))
}

/// `value` as one base64url segment of a compact JWS.
fn encode_segment(value: &Value) -> String {
    aws_smithy_types::base64::encode(value.to_string().as_bytes())
        .replace('+', "-")
        .replace('/', "_")
        .trim_end_matches('=')
        .to_string()
}

/// The JOSE header and the claim set of a compact JWS: its first two segments.
fn parts_of(token: &str) -> (Value, Value) {
    let segments: Vec<&str> = token.split('.').collect();
    assert_eq!(
        segments.len(),
        3,
        "a minted token must be a compact JWS of three segments, got {} segment(s)",
        segments.len()
    );
    let read = |segment: &str| -> Value {
        serde_json::from_slice(&decode_segment(segment))
            .unwrap_or_else(|e| panic!("a minted token's segment must be JSON ({e})"))
    };
    (read(segments[0]), read(segments[1]))
}

/// The token the pool minted, re-encoded with the JOSE header and claim set given and its
/// signature segment kept — so the only difference from the admitted token is what the row
/// changed.
fn rebuilt(token: &str, header: &Value, claims: &Value) -> String {
    let signature = token
        .split('.')
        .next_back()
        .expect("a compact JWS has a signature segment");
    format!(
        "{}.{}.{signature}",
        encode_segment(header),
        encode_segment(claims)
    )
}

/// Mint one pool, one app client and one confirmed user, and answer the pool id, the app
/// client id and the ACCESS TOKEN that client's sign-in produced.
async fn pool_with_signed_in_user(
    router: &MultiServiceRouter,
    pool_name: &str,
    client_names: &[&str],
    username: &str,
) -> (String, Vec<String>, Vec<String>) {
    let pool = cognito(router, "CreateUserPool", &json!({ "PoolName": pool_name })).await;
    let pool_id = as_str("CreateUserPool", &pool["UserPool"], "Id");

    let mut client_ids = Vec::new();
    for client_name in client_names {
        let client = cognito(
            router,
            "CreateUserPoolClient",
            &json!({
                "UserPoolId": pool_id,
                "ClientName": client_name,
                // The flow the sign-in below uses, plus the refresh flow AWS requires
                // beside any other `ALLOW_` flow.
                "ExplicitAuthFlows": [
                    "ALLOW_ADMIN_USER_PASSWORD_AUTH",
                    "ALLOW_REFRESH_TOKEN_AUTH"
                ]
            }),
        )
        .await;
        client_ids.push(as_str(
            "CreateUserPoolClient",
            &client["UserPoolClient"],
            "ClientId",
        ));
    }

    cognito(
        router,
        "AdminCreateUser",
        &json!({ "UserPoolId": pool_id, "Username": username }),
    )
    .await;
    cognito(
        router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": pool_id,
            "Username": username,
            "Password": GOOD_PASSWORD,
            "Permanent": true
        }),
    )
    .await;

    let mut access_tokens = Vec::new();
    for client_id in &client_ids {
        let signed_in = cognito(
            router,
            "AdminInitiateAuth",
            &json!({
                "UserPoolId": pool_id,
                "ClientId": client_id,
                "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
                "AuthParameters": { "USERNAME": username, "PASSWORD": GOOD_PASSWORD }
            }),
        )
        .await;
        access_tokens.push(as_str(
            "AdminInitiateAuth",
            &signed_in["AuthenticationResult"],
            "AccessToken",
        ));
    }

    (pool_id, client_ids, access_tokens)
}

/// What the gate owes one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The gate admitted the request and the route's `HTTP_PROXY` integration answered it,
    /// sending the caller to the upstream it names.
    Redirected,
    /// The gate admitted the request at a route whose integration this emulator cannot
    /// call: the answer is not a refusal of the authorizer's and not a missing route.
    Admitted,
    /// The authorizer could not accept the presented token at all.
    Unauthorized,
    /// The authorizer accepted the token and the route requires a scope it does not claim.
    Forbidden,
}

/// One request to the deployed stage: the token presented at the authorizer's identity
/// source, the route it is presented to, and what the gate owes it.
struct Row {
    case: String,
    path: &'static str,
    /// The value the `Authorization` header carries, or `None` to present nothing at all.
    presented: Option<String>,
    expected: Expected,
}

#[test]
fn a_cognito_jwt_authorizer_gates_a_deployed_http_api_route() {
    std::thread::Builder::new()
        .name("apigatewayv2-cognito-jwt".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            // Timers ENABLED, which is what the emulator's own boot path builds
            // (`core/router/src/main.rs` builds its runtime with `enable_all`): the HTTP
            // api's data plane gives the route's integration the deadline the integration's
            // own `TimeoutInMillis` states, and a runtime with no timer cannot hold one.
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .expect("build a current-thread runtime");
            runtime.block_on(async {
                let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
                let mut router = build_router(engine);
                // Signature verification belongs to the emulator, not to this check.
                router.set_request_authentication(false);

                // 1. The pool whose tokens the api accepts, with two app clients, and a
                //    second pool whose tokens are from a FOREIGN issuer.
                let (pool_id, client_ids, access_tokens) = pool_with_signed_in_user(
                    &router,
                    "jwt-gate-pool",
                    &["admitted-client", "second-client"],
                    "gate-user",
                )
                .await;
                let (_foreign_pool_id, _foreign_client_ids, foreign_tokens) =
                    pool_with_signed_in_user(
                        &router,
                        "jwt-gate-foreign-pool",
                        &["foreign-client"],
                        "foreign-user",
                    )
                    .await;
                let admitted_token = access_tokens[0].clone();
                let second_client_token = access_tokens[1].clone();
                let foreign_token = foreign_tokens[0].clone();

                // The issuer a JWT authorizer records for a user pool: "an Amazon Cognito
                // user pool has the following format: https://cognito-idp.{region}
                // .amazonaws.com/{userPoolId}" (`JWTConfiguration$Issuer`). The minted
                // token has to name exactly that, or no authorizer a client configures
                // from the documented form could ever accept it.
                let issuer = format!("https://cognito-idp.{REGION}.amazonaws.com/{pool_id}");
                let (header, claims) = parts_of(&admitted_token);
                assert_eq!(
                    claims.get("iss").and_then(Value::as_str),
                    Some(issuer.as_str()),
                    "the pool's access token must name the documented issuer, got {claims}"
                );
                // An ACCESS token names its client in `client_id` rather than in `aud`,
                // which is the audience a JWT authorizer of a user pool matches.
                assert_eq!(
                    claims.get("client_id").and_then(Value::as_str),
                    Some(client_ids[0].as_str()),
                    "the access token must name the client it was minted for, got {claims}"
                );
                // The scope claim is "a JSON string containing a space-separated list of
                // scopes" (RFC 8693 §4.2); the route below requires one of its entries, so
                // the matching scope is the token's own rather than a value written here.
                let claimed_scope = claims
                    .get("scope")
                    .and_then(Value::as_str)
                    .and_then(|scope| scope.split_whitespace().next())
                    .unwrap_or_else(|| {
                        panic!("the access token must carry a scope claim, got {claims}")
                    })
                    .to_string();
                assert_ne!(
                    claimed_scope, UNCLAIMED_SCOPE,
                    "the unmatched route scope must be one the token does not claim"
                );

                // 2. The api, its two authorizers, its integrations, its routes and the
                //    stage that serves its root.
                let api = control_plane(
                    &router,
                    "CreateApi",
                    "POST",
                    "/v2/apis",
                    Some(json!({ "name": "jwt-gated-api", "protocolType": "HTTP" })),
                    201,
                )
                .await;
                let api_id = as_str("CreateApi", &api, "apiId");
                let endpoint = as_str("CreateApi", &api, "apiEndpoint");
                let endpoint_host = endpoint
                    .split_once("://")
                    .map_or(endpoint.as_str(), |(_scheme, host)| host)
                    .to_string();

                // The step this issue was filed about: a JWT authorizer fronting the pool.
                // Its two rows differ in how many clients the `Audience` admits.
                let mut authorizer_ids = Vec::new();
                for (name, audience) in [
                    ("one-audience", vec![client_ids[0].clone()]),
                    (
                        "both-audiences",
                        vec![client_ids[0].clone(), client_ids[1].clone()],
                    ),
                ] {
                    let request = json!({
                        "name": name,
                        "authorizerType": "JWT",
                        "identitySource": ["$request.header.Authorization"],
                        "jwtConfiguration": { "issuer": issuer, "audience": audience }
                    });
                    let created = control_plane(
                        &router,
                        &format!("CreateAuthorizer({name})"),
                        "POST",
                        &format!("/v2/apis/{api_id}/authorizers"),
                        Some(request.clone()),
                        201,
                    )
                    .await;
                    let authorizer_id = as_str("CreateAuthorizer", &created, "authorizerId");
                    assert_eq!(
                        created.get("jwtConfiguration"),
                        request.get("jwtConfiguration"),
                        "case {name:?}: CreateAuthorizer must report the configuration it \
                         stored, got {created}"
                    );
                    // Read back through the implemented reader: what the create answered
                    // is what a later reader reports, so the authorizer was persisted
                    // rather than echoed.
                    let fetched = control_plane(
                        &router,
                        &format!("GetAuthorizer({name})"),
                        "GET",
                        &format!("/v2/apis/{api_id}/authorizers/{authorizer_id}"),
                        None,
                        200,
                    )
                    .await;
                    assert_eq!(
                        fetched, created,
                        "case {name:?}: GetAuthorizer must report the created authorizer \
                         unchanged, got {fetched}"
                    );
                    authorizer_ids.push(authorizer_id);
                }

                let upstream = control_plane(
                    &router,
                    "CreateIntegration(HTTP_PROXY)",
                    "POST",
                    &format!("/v2/apis/{api_id}/integrations"),
                    Some(json!({
                        "integrationType": "HTTP_PROXY",
                        "integrationMethod": "GET",
                        "integrationUri": UPSTREAM,
                        "payloadFormatVersion": "1.0"
                    })),
                    201,
                )
                .await;
                let upstream_id = as_str("CreateIntegration", &upstream, "integrationId");

                let lambda = control_plane(
                    &router,
                    "CreateIntegration(AWS_PROXY)",
                    "POST",
                    &format!("/v2/apis/{api_id}/integrations"),
                    Some(json!({
                        "integrationType": "AWS_PROXY",
                        "integrationUri": format!(
                            "arn:aws:lambda:{REGION}:000000000000:function:jwt-claims-echo"
                        ),
                        "payloadFormatVersion": "2.0"
                    })),
                    201,
                )
                .await;
                let lambda_id = as_str("CreateIntegration", &lambda, "integrationId");

                // Each route names the authorizer, the scopes and the integration one
                // group of rows is about.
                let routes: Vec<(&str, &str, Vec<String>, &str)> = vec![
                    ("GET /guarded", &authorizer_ids[0], Vec::new(), &upstream_id),
                    ("GET /both", &authorizer_ids[1], Vec::new(), &upstream_id),
                    (
                        "GET /scoped",
                        &authorizer_ids[0],
                        vec![UNCLAIMED_SCOPE.to_string()],
                        &upstream_id,
                    ),
                    (
                        "GET /pool-scope",
                        &authorizer_ids[0],
                        vec![UNCLAIMED_SCOPE.to_string(), claimed_scope.clone()],
                        &upstream_id,
                    ),
                    ("GET /proxy", &authorizer_ids[0], Vec::new(), &lambda_id),
                ];
                for (route_key, authorizer_id, scopes, integration_id) in &routes {
                    let mut request = json!({
                        "routeKey": route_key,
                        "target": format!("integrations/{integration_id}"),
                        "authorizationType": "JWT",
                        "authorizerId": authorizer_id
                    });
                    if !scopes.is_empty() {
                        request["authorizationScopes"] = json!(scopes);
                    }
                    control_plane(
                        &router,
                        &format!("CreateRoute({route_key})"),
                        "POST",
                        &format!("/v2/apis/{api_id}/routes"),
                        Some(request),
                        201,
                    )
                    .await;
                }

                let stage = control_plane(
                    &router,
                    "CreateStage",
                    "POST",
                    &format!("/v2/apis/{api_id}/stages"),
                    Some(json!({ "stageName": "$default", "autoDeploy": true })),
                    201,
                )
                .await;
                assert!(
                    stage.get("deploymentId").and_then(Value::as_str).is_some(),
                    "an auto-deploying stage must carry a deployment, got {stage}"
                );

                // 3. The table: one request per row to the stage that serves the api's
                //    root, addressed at the endpoint `CreateApi` itself reported.
                let bearer = |token: &str| Some(format!("Bearer {token}"));
                let mut expired_claims = claims.clone();
                expired_claims["exp"] = json!(claims
                    .get("iat")
                    .and_then(Value::as_i64)
                    .unwrap_or_default());
                let rows = vec![
                    Row {
                        case: "the pool's own access token, as RFC 6750 bearer credentials"
                            .to_string(),
                        path: "/guarded",
                        presented: bearer(&admitted_token),
                        expected: Expected::Redirected,
                    },
                    Row {
                        case: "the same token presented bare, with no scheme prefix"
                            .to_string(),
                        path: "/guarded",
                        presented: Some(admitted_token.clone()),
                        expected: Expected::Redirected,
                    },
                    Row {
                        case: "a second client of the same pool, outside the authorizer's \
                               one-entry Audience"
                            .to_string(),
                        path: "/guarded",
                        presented: bearer(&second_client_token),
                        expected: Expected::Unauthorized,
                    },
                    Row {
                        case: "that same second client's token, at the route whose \
                               authorizer records both clients"
                            .to_string(),
                        path: "/both",
                        presented: bearer(&second_client_token),
                        expected: Expected::Redirected,
                    },
                    Row {
                        case: "an access token of another user pool: a foreign issuer"
                            .to_string(),
                        path: "/guarded",
                        presented: bearer(&foreign_token),
                        expected: Expected::Unauthorized,
                    },
                    Row {
                        case: "the pool's own token with its exp no longer in the future"
                            .to_string(),
                        path: "/guarded",
                        presented: bearer(&rebuilt(&admitted_token, &header, &expired_claims)),
                        expected: Expected::Unauthorized,
                    },
                    Row {
                        case: "the pool's own claim set in an unsecured token (alg none)"
                            .to_string(),
                        path: "/guarded",
                        presented: bearer(&rebuilt(
                            &admitted_token,
                            &json!({ "alg": "none", "typ": "JWT" }),
                            &claims,
                        )),
                        expected: Expected::Unauthorized,
                    },
                    Row {
                        case: "a value that is not a token at all".to_string(),
                        path: "/guarded",
                        presented: bearer("not-a-token"),
                        expected: Expected::Unauthorized,
                    },
                    Row {
                        case: "nothing at the authorizer's identity source".to_string(),
                        path: "/guarded",
                        presented: None,
                        expected: Expected::Unauthorized,
                    },
                    Row {
                        case: "a route scope the token's scope claim does not carry"
                            .to_string(),
                        path: "/scoped",
                        presented: bearer(&admitted_token),
                        expected: Expected::Forbidden,
                    },
                    Row {
                        case: "a route scope list one of whose entries the token claims"
                            .to_string(),
                        path: "/pool-scope",
                        presented: bearer(&admitted_token),
                        expected: Expected::Redirected,
                    },
                    Row {
                        case: "the route fronting the AWS_PROXY integration the recorded \
                               check uses"
                            .to_string(),
                        path: "/proxy",
                        presented: bearer(&admitted_token),
                        expected: Expected::Admitted,
                    },
                ];

                for row in &rows {
                    let headers: Vec<(&str, &str)> = row
                        .presented
                        .as_deref()
                        .map(|presented| vec![("authorization", presented)])
                        .unwrap_or_default();
                    let answer =
                        dispatch(&router, &endpoint_host, "GET", row.path, &headers, Vec::new())
                            .await;
                    assert_ne!(
                        answer.error_type.as_deref(),
                        Some(UNKNOWN_SERVICE),
                        "case {:?}: the invoke URL must be served by the api's own data \
                         plane, got {}",
                        row.case,
                        answer.summary()
                    );
                    match row.expected {
                        Expected::Redirected => {
                            assert_eq!(
                                answer.status,
                                TEMPORARY_REDIRECT,
                                "case {:?}: an admitted request must reach the route's \
                                 integration, got {}",
                                row.case,
                                answer.summary()
                            );
                            assert_eq!(
                                answer.location.as_deref(),
                                Some(UPSTREAM),
                                "case {:?}: the integration's own upstream must be where \
                                 the caller is sent, got {}",
                                row.case,
                                answer.summary()
                            );
                        }
                        Expected::Admitted => {
                            assert!(
                                !matches!(answer.status, UNAUTHORIZED | FORBIDDEN | NOT_FOUND),
                                "case {:?}: the gate must admit the token and the matched \
                                 route must front its integration, got {}",
                                row.case,
                                answer.summary()
                            );
                            // Whatever status the answer carries, it is not one of the
                            // authorizer's two refusals: those are identified by their
                            // sentence as much as by their status.
                            assert!(
                                !matches!(
                                    answer.message().as_str(),
                                    UNAUTHORIZED_MESSAGE | FORBIDDEN_MESSAGE
                                ),
                                "case {:?}: the gate must admit the token, so neither \
                                 refusal sentence may be answered, got {}",
                                row.case,
                                answer.summary()
                            );
                            // And by the integration the ROUTE names: a Lambda invocation
                            // is no URI a client can fetch, so this row is never answered
                            // with the other integration's redirect.
                            assert_eq!(
                                answer.location,
                                None,
                                "case {:?}: an AWS_PROXY route must not be answered with a \
                                 redirect to another integration's upstream, got {}",
                                row.case,
                                answer.summary()
                            );
                        }
                        Expected::Unauthorized | Expected::Forbidden => {
                            let (status, message) = if row.expected == Expected::Unauthorized {
                                (UNAUTHORIZED, UNAUTHORIZED_MESSAGE)
                            } else {
                                (FORBIDDEN, FORBIDDEN_MESSAGE)
                            };
                            assert_eq!(
                                answer.status,
                                status,
                                "case {:?}: expected HTTP {status}, got {}",
                                row.case,
                                answer.summary()
                            );
                            assert_eq!(
                                answer.message(),
                                message,
                                "case {:?}: expected the {message:?} body, got {}",
                                row.case,
                                answer.summary()
                            );
                            assert_eq!(
                                answer.location,
                                None,
                                "case {:?}: a refused request must not reach the route's \
                                 integration, got {}",
                                row.case,
                                answer.summary()
                            );
                        }
                    }
                }
            });
        })
        .expect("spawn the cognito JWT authorizer thread")
        .join()
        .expect("the cognito JWT authorizer check must pass");
}
