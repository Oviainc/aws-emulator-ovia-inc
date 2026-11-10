// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what a JWT authorizer tells the INTEGRATION behind the route
//! it admitted.
//!
//! The rule, one level above the recorded case (a Cognito access token presented to an
//! `AWS_PROXY` route whose handler reads `requestContext.authorizer.jwt.claims`):
//!
//! > A route admitted by a JWT authorizer carries the accepted token's own claim set and
//! > the scopes it claims into the proxy event's `requestContext.authorizer` — under `jwt`
//! > for payload format `2.0` and directly on the authorizer object for `1.0` — because
//! > that object is the only place a handler reads which caller the authorizer admitted.
//! > The claims are the token's, every one of them, as the map of text values the event
//! > carries; the scopes are "the scopes parsed from the access token in the incoming
//! > request" (`Route$AuthorizationScopes`), and a token claiming none reports none.
//!
//! The rows span what the rule generalises over: the payload format version, which claims
//! the token carries and of which JSON types (strings, numbers, a flag, an `aud` array),
//! which spelling the scopes are claimed under (the space-delimited `scope` of RFC 8693
//! §4.2 and the `scp` array providers issue instead), whether the token claims any scope
//! at all, and whether the route requires one. The recorded case — an access token with a
//! `scope` claim on a `2.0` route — is ONE row.
//!
//! Nothing here is a value this emulator fills in: every assertion is on the event a
//! function RAN and echoed back, which is why AWS Lambda is mounted beside this service on
//! one engine. `http_api_jwt_data_plane` owns the gate's own decisions (which token earns
//! which status); this suite owns what an ADMITTED token hands on.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call_at_host};
use crate::http_api_backend_dispatch::{
    emulator_with_lambda, given_function, lambda_integration, ECHO_EVENT,
};
use crate::websocket_handshake_data_plane::send;

/// The Region every request below is signed for, and the Region of the function the
/// integration names.
const REGION: &str = "us-east-1";

/// The issuer the authorizer records, in the form `JWTConfiguration$Issuer` documents for a
/// user pool.
const ISSUER: &str = "https://cognito-idp.us-east-1.amazonaws.com/us-east-1_abc123def";

/// The one audience the authorizer admits — the pool client a token names.
const AUDIENCE: &str = "1example23456789abcdef";

/// The identity source the authorizer extracts the token from: "for HTTP APIs, use
/// selection expressions prefixed with $, for example, $request.header.Auth"
/// (`Authorizer$IdentitySource`).
const IDENTITY_SOURCE: &str = "$request.header.Authorization";

/// The route every row deploys, and the path within the root stage that matches it.
const ROUTE_KEY: &str = "GET /items";
const ROUTE_PATH: &str = "/items";

/// Now, in whole seconds since the epoch: the clock a token's `exp` is read against.
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// `value` as one base64url segment of a compact JWS: "base64url without padding"
/// (RFC 7515 §2).
fn segment(value: &Value) -> String {
    aws_smithy_types::base64::encode(value.to_string().as_bytes())
        .replace('+', "-")
        .replace('/', "_")
        .trim_end_matches('=')
        .to_string()
}

/// A compact JWS over `claims`, in the shape a user pool's token has: a JOSE header naming
/// a signing algorithm and a key, and a non-empty signature segment.
fn token(claims: &Value) -> String {
    format!(
        "{}.{}.c2lnbmF0dXJlLWJ5dGVz",
        segment(&json!({ "alg": "ES256", "kid": "e3b0c44298fc1c14", "typ": "JWT" })),
        segment(claims)
    )
}

/// One row: the token presented, the payload format of the route's integration, and what
/// the event must then report about the caller.
struct Row {
    case: &'static str,
    payload_format: &'static str,
    /// The whole claim set of the presented token.
    claims: Value,
    /// Each claim the event must carry, and the TEXT it must carry for it.
    expect_claims: Vec<(&'static str, String)>,
    /// The `scopes` the event must report: the list, or JSON null for a token claiming
    /// none.
    expect_scopes: Value,
    /// The route's `AuthorizationScopes`.
    required_scopes: &'static [&'static str],
}

fn rows() -> Vec<Row> {
    let expiry = now() + 3600;
    let issued = now();
    vec![
        Row {
            case: "the recorded case: an access token with a space-delimited scope claim, \
                   on a 2.0 route",
            payload_format: "2.0",
            claims: json!({
                "iss": ISSUER,
                "client_id": AUDIENCE,
                "token_use": "access",
                "sub": "b3994ede-a08f-422c-b14f-eda2471450ce",
                "username": "gate-user",
                "scope": "aws.cognito.signin.user.admin orders/read",
                "iat": issued,
                "exp": expiry
            }),
            expect_claims: vec![
                ("iss", ISSUER.to_string()),
                ("client_id", AUDIENCE.to_string()),
                ("token_use", "access".to_string()),
                ("sub", "b3994ede-a08f-422c-b14f-eda2471450ce".to_string()),
                ("username", "gate-user".to_string()),
                (
                    "scope",
                    "aws.cognito.signin.user.admin orders/read".to_string(),
                ),
                // A numeric claim travels as its decimal text: the claims map's values are
                // text.
                ("iat", issued.to_string()),
                ("exp", expiry.to_string()),
            ],
            expect_scopes: json!(["aws.cognito.signin.user.admin", "orders/read"]),
            required_scopes: &[],
        },
        Row {
            case: "the same token on a route that REQUIRES one of the scopes it claims",
            payload_format: "2.0",
            claims: json!({
                "iss": ISSUER,
                "client_id": AUDIENCE,
                "token_use": "access",
                "sub": "0f8fad5b-d9cb-469f-a165-70867728950e",
                "scope": "orders/read",
                "exp": expiry
            }),
            expect_claims: vec![
                ("iss", ISSUER.to_string()),
                ("client_id", AUDIENCE.to_string()),
                ("token_use", "access".to_string()),
                ("sub", "0f8fad5b-d9cb-469f-a165-70867728950e".to_string()),
                ("scope", "orders/read".to_string()),
                ("exp", expiry.to_string()),
            ],
            expect_scopes: json!(["orders/read"]),
            required_scopes: &["orders/read"],
        },
        Row {
            case: "an identity token claiming no scope at all, with an aud array and a flag \
                   claim, on a 2.0 route",
            payload_format: "2.0",
            claims: json!({
                "iss": ISSUER,
                "aud": [AUDIENCE, "9another87654321zyxw"],
                "token_use": "id",
                "sub": "11111111-2222-3333-4444-555555555555",
                "email_verified": true,
                "exp": expiry
            }),
            expect_claims: vec![
                ("iss", ISSUER.to_string()),
                // A claim that is neither a string nor a number travels as the JSON text it
                // had, so nothing of it is lost.
                (
                    "aud",
                    format!("[\"{AUDIENCE}\",\"9another87654321zyxw\"]"),
                ),
                ("token_use", "id".to_string()),
                ("sub", "11111111-2222-3333-4444-555555555555".to_string()),
                ("email_verified", "true".to_string()),
                ("exp", expiry.to_string()),
            ],
            expect_scopes: Value::Null,
            required_scopes: &[],
        },
        Row {
            case: "a token claiming its scopes in an scp ARRAY, on a 1.0 route",
            payload_format: "1.0",
            claims: json!({
                "iss": ISSUER,
                "aud": AUDIENCE,
                "token_use": "access",
                "sub": "9c858901-8a57-4791-81fe-4c455b099bc9",
                "scp": ["invoices/write", "invoices/read"],
                "exp": expiry
            }),
            expect_claims: vec![
                ("iss", ISSUER.to_string()),
                ("aud", AUDIENCE.to_string()),
                ("token_use", "access".to_string()),
                ("sub", "9c858901-8a57-4791-81fe-4c455b099bc9".to_string()),
                ("scp", "[\"invoices/write\",\"invoices/read\"]".to_string()),
                ("exp", expiry.to_string()),
            ],
            // The scopes a token claims are a SET, so they are reported in one order
            // whichever order the claim listed them in.
            expect_scopes: json!(["invoices/read", "invoices/write"]),
            required_scopes: &[],
        },
        Row {
            case: "an identity token on a 1.0 route, which carries the same two members on \
                   the authorizer object itself",
            payload_format: "1.0",
            claims: json!({
                "iss": ISSUER,
                "aud": AUDIENCE,
                "token_use": "id",
                "sub": "7c9e6679-7425-40de-944b-e07fc1f90ae7",
                "exp": expiry
            }),
            expect_claims: vec![
                ("iss", ISSUER.to_string()),
                ("aud", AUDIENCE.to_string()),
                ("token_use", "id".to_string()),
                ("sub", "7c9e6679-7425-40de-944b-e07fc1f90ae7".to_string()),
                ("exp", expiry.to_string()),
            ],
            expect_scopes: Value::Null,
            required_scopes: &[],
        },
    ]
}

/// One control-plane call against this service, signed for [`REGION`] and addressed at this
/// service's own endpoint host.
async fn control(
    emulator: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    call_at_host(
        emulator,
        &format!("apigateway.{REGION}.amazonaws.com"),
        REGION,
        method,
        uri,
        body,
    )
    .await
}

/// Deploy the row's api: a JWT authorizer of the recorded issuer and audience, a Lambda
/// proxy integration that echoes the event it is given, the route that fronts it behind
/// that authorizer, and the stage that serves the api's root. Answers the host its
/// `ApiEndpoint` names.
async fn deploy(emulator: &MultiServiceRouter, row: &Row, function_arn: &str) -> String {
    let case = row.case;
    let (status, api) = control(
        emulator,
        "POST",
        "/v2/apis",
        Some(json!({ "name": "jwt-claims-api", "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateApi must answer 201, got {status} {api}"
    );
    let api_id = as_str(&api, "apiId");
    let host = as_str(&api, "apiEndpoint")
        .split_once("://")
        .map(|(_scheme, rest)| rest.to_string())
        .unwrap_or_default();

    let (status, authorizer) = control(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/authorizers"),
        Some(json!({
            "name": "pool-authorizer",
            "authorizerType": "JWT",
            "identitySource": [IDENTITY_SOURCE],
            "jwtConfiguration": { "issuer": ISSUER, "audience": [AUDIENCE] }
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateAuthorizer must answer 201, got {status} {authorizer}"
    );
    let authorizer_id = as_str(&authorizer, "authorizerId");

    let (status, integration) = control(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/integrations"),
        Some(lambda_integration(function_arn, row.payload_format)),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateIntegration must answer 201, got {status} {integration}"
    );
    let integration_id = as_str(&integration, "integrationId");

    let mut route = json!({
        "routeKey": ROUTE_KEY,
        "target": format!("integrations/{integration_id}"),
        "authorizationType": "JWT",
        "authorizerId": authorizer_id
    });
    if !row.required_scopes.is_empty() {
        route["authorizationScopes"] = json!(row.required_scopes);
    }
    let (status, created) = control(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/routes"),
        Some(route),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateRoute must answer 201, got {status} {created}"
    );

    let (status, stage) = control(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(json!({ "stageName": "$default", "autoDeploy": true })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateStage must answer 201, got {status} {stage}"
    );
    host
}

#[tokio::test]
async fn the_claims_of_an_admitted_token_reach_the_integrations_event() {
    for row in rows() {
        let case = row.case;
        let (emulator, engine) = emulator_with_lambda();
        let function =
            given_function(&emulator, &engine, case, "jwt-claims-echo", ECHO_EVENT).await;
        let host = deploy(&emulator, &row, &function).await;

        let presented = token(&row.claims);
        let answer = send(
            &emulator,
            &host,
            "GET",
            ROUTE_PATH,
            &[("authorization", &format!("Bearer {presented}"))],
        )
        .await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: the admitted token must reach the integration, got {}",
            answer.summary()
        );
        let event: Value = serde_json::from_str(&answer.text).unwrap_or(Value::Null);

        // Where the format carries the authorizer's answer: `2.0` keeps it under `jwt`,
        // `1.0` on the authorizer object itself.
        let (claims_pointer, scopes_pointer) = if row.payload_format == "2.0" {
            (
                "/requestContext/authorizer/jwt/claims",
                "/requestContext/authorizer/jwt/scopes",
            )
        } else {
            (
                "/requestContext/authorizer/claims",
                "/requestContext/authorizer/scopes",
            )
        };

        let claims = event
            .pointer(claims_pointer)
            .cloned()
            .unwrap_or(Value::Null);
        for (claim, expected) in &row.expect_claims {
            let actual = claims.get(claim).cloned().unwrap_or(Value::Null);
            assert_eq!(
                actual,
                json!(expected),
                "case {case:?}: expected the token's {claim:?} claim at \
                 {claims_pointer}/{claim} to be {expected:?}, got {actual} in {}",
                answer.text
            );
        }
        // Exactly the claims the token carried: nothing dropped, and nothing the event
        // invented.
        let mut reported: Vec<String> = claims
            .as_object()
            .map(|members| members.keys().cloned().collect())
            .unwrap_or_default();
        reported.sort();
        let mut presented_claims: Vec<String> = row
            .claims
            .as_object()
            .expect("the row's claim set is an object")
            .keys()
            .cloned()
            .collect();
        presented_claims.sort();
        assert_eq!(
            reported, presented_claims,
            "case {case:?}: expected exactly the claims the token carried, got {claims} in {}",
            answer.text
        );

        let scopes = event
            .pointer(scopes_pointer)
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            scopes, row.expect_scopes,
            "case {case:?}: expected the scopes the token claims at {scopes_pointer} to be \
             {}, got {scopes} in {}",
            row.expect_scopes, answer.text
        );
    }
}

/// A route no JWT authorizer fronts reports no caller at all: the member exists only where
/// an authorizer decided the request, so an open route's event carries no `authorizer` and
/// nothing is invented for it.
#[tokio::test]
async fn an_open_route_reports_no_authorizer_at_all() {
    for payload_format in ["2.0", "1.0"] {
        let case = format!("an open route on a {payload_format} integration");
        let (emulator, engine) = emulator_with_lambda();
        let function =
            given_function(&emulator, &engine, &case, "open-route-echo", ECHO_EVENT).await;
        let (_api_id, host) = crate::http_api_backend_dispatch::given_api(
            &emulator,
            &case,
            lambda_integration(&function, payload_format),
            ROUTE_KEY,
            json!({ "stageName": "$default", "autoDeploy": true }),
        )
        .await;

        let answer = send(&emulator, &host, "GET", ROUTE_PATH, &[]).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: an open route must reach the integration, got {}",
            answer.summary()
        );
        let event: Value = serde_json::from_str(&answer.text).unwrap_or(Value::Null);
        let authorizer = event.pointer("/requestContext/authorizer").cloned();
        assert_eq!(
            authorizer, None,
            "case {case:?}: expected no authorizer member at all, got {authorizer:?} in {}",
            answer.text
        );
    }
}
