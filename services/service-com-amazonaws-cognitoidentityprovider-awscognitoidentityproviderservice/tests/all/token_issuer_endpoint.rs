// SPDX-License-Identifier: CC0-1.0

//! The ISSUER a user-pool token announces, over the endpoints a caller reaches this
//! service on.
//!
//! The RULE the rows assert, one level above any single deployment:
//!
//! > A user-pool token's `iss` claim is the user-pool endpoint the request that minted it
//! > was served on, with the minting pool's id appended — `<endpoint>/<UserPoolId>` — so
//! > the issuer a token announces is the address its holder is already talking to, and
//! > `<iss>/.well-known/jwks.json` is served there.
//!
//! The issuer is token content, which this service's model does not describe: `smithy_query`
//! on `InitiateAuth` / `AdminInitiateAuth` / `GetClientToken` models only the token members
//! as opaque strings, and `IssuerConfigurationType.Type` is the only place the model states
//! an issuer at all — "the issuer URL is hosted in the user pool's region", the published
//! `https://cognito-idp.[region].amazonaws.com/[userPoolId]` form, which is exactly this
//! rule for a caller that reaches the service at AWS's own hostname.
//!
//! The rows span what the rule generalises over: WHICH address the minting request reached
//! (the published regional endpoint of the pool's own region, a loopback address with a
//! port, a named local endpoint with a port, a bracketed IPv6 literal, an address that is
//! no host at all, a request naming no host), WHICH minting path answered (a sign-in, an
//! admin sign-in, a refresh, a machine-to-machine grant) and WHICH token of it carries the
//! claim (the id token and the access token).

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice as cognito_idp;
use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::state;

/// The local name of this service's Smithy shape, which is the `X-Amz-Target` prefix a
/// client writes.
const TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// This service's SigV4 signing name, the 4th field of a credential scope.
const SIGNING_NAME: &str = "cognito-idp";

/// The region every call of this suite is signed for, so the region each pool it creates
/// lives in and the region of each pool's published endpoint.
const REGION: &str = "us-east-1";

/// This service's published endpoint host in [`REGION`].
const PUBLISHED_HOST: &str = "cognito-idp.us-east-1.amazonaws.com";

/// A password satisfying a default pool's password policy, written for this suite.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// A `ClientSecretType`-shaped secret (pattern `^[\w+]+$`, length 24..64) the
/// machine-to-machine app client below is created with.
const CLIENT_SECRET: &str = "issuerm2msecret0000000000001";

/// The scope that client is granted, defined by the resource server the fixture creates.
const RESOURCE_SERVER: &str = "https://api.example.test";
const SCOPE_NAME: &str = "read";

/// One emulator with just this service mounted, beside the engine its state lives in.
fn router_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(cognito_idp::EmulatedService::new(Arc::clone(&engine)));
    // Signature verification belongs to the emulator, not to this check.
    router.set_request_authentication(false);
    (router, engine)
}

/// A credential scope naming this service in [`REGION`], which is what routes a call here
/// whatever host it addressed.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{SIGNING_NAME}/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// Call `operation` on the endpoint `host` addresses, requiring success, and return its
/// response body. `host` is `None` for a request that names no host at all.
async fn ok(
    router: &MultiServiceRouter,
    host: Option<&str>,
    operation: &str,
    body: &Value,
) -> Value {
    let mut request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header("authorization", authorization());
    if let Some(host) = host {
        request = request.header("host", host);
    }
    let response = router
        .dispatch(
            request
                .body(Bytes::from(body.to_string()))
                .expect("the request is well formed"),
        )
        .await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or_else(|error| {
        panic!(
            "{operation} answered a body that is not JSON ({error}): {:?}",
            String::from_utf8_lossy(response.body())
        )
    });
    assert_eq!(
        status, 200,
        "{operation} on host {host:?} must succeed, got {status} with {payload}"
    );
    payload
}

/// The claim set of a compact JWS, decoded from its second segment.
fn claims(token: &str) -> Value {
    let segment = token
        .split('.')
        .nth(1)
        .unwrap_or_else(|| panic!("{token:?} must be a compact JWS"));
    let mut standard = segment.replace('-', "+").replace('_', "/");
    while !standard.len().is_multiple_of(4) {
        standard.push('=');
    }
    let bytes = aws_smithy_types::base64::decode(&standard)
        .unwrap_or_else(|error| panic!("the claims segment must be base64url ({error})"));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("the claims segment must be JSON ({error})"))
}

/// The JOSE header of a compact JWS, decoded from its first segment.
fn jose_header(token: &str) -> Value {
    let segment = token
        .split('.')
        .next()
        .unwrap_or_else(|| panic!("{token:?} must be a compact JWS"));
    let mut standard = segment.replace('-', "+").replace('_', "/");
    while !standard.len().is_multiple_of(4) {
        standard.push('=');
    }
    let bytes = aws_smithy_types::base64::decode(&standard)
        .unwrap_or_else(|error| panic!("the header segment must be base64url ({error})"));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("the header segment must be JSON ({error})"))
}

/// The string claim `name` of `token`.
fn claim(token: &str, name: &str) -> String {
    claims(token)[name]
        .as_str()
        .unwrap_or_else(|| panic!("a token carries a {name} claim, got {}", claims(token)))
        .to_string()
}

/// One endpoint a caller reaches this service on.
struct EndpointCase {
    case: &'static str,
    /// The `Host` header the minting request carries, `None` for one naming no host.
    host: Option<&'static str>,
    /// The authority the tokens that request mints must name as their issuer.
    authority: &'static str,
}

/// The endpoints the rows below mint on. Each pool is CREATED on the published endpoint
/// and minted on the row's, so what the issuer names is the endpoint of the minting
/// request rather than of the create.
const ENDPOINTS: &[EndpointCase] = &[
    EndpointCase {
        case: "the pool's own published regional endpoint",
        host: Some(PUBLISHED_HOST),
        authority: PUBLISHED_HOST,
    },
    EndpointCase {
        case: "a loopback address with a port, which is how this emulator is reached",
        host: Some("127.0.0.1:9297"),
        authority: "127.0.0.1:9297",
    },
    EndpointCase {
        case: "a named local endpoint with a port",
        host: Some("localhost:4566"),
        authority: "localhost:4566",
    },
    EndpointCase {
        case: "a bracketed IPv6 loopback endpoint",
        host: Some("[::1]:9297"),
        authority: "[::1]:9297",
    },
    EndpointCase {
        case: "a value no host is shaped like, which addresses no endpoint",
        host: Some("not a host"),
        authority: PUBLISHED_HOST,
    },
    EndpointCase {
        case: "a request naming no host at all",
        host: None,
        authority: PUBLISHED_HOST,
    },
];

/// A pool with one sign-in app client and one confirmed user, created on the PUBLISHED
/// endpoint.
struct Fixture {
    user_pool_id: String,
    client_id: String,
    username: String,
    /// The machine-to-machine app client of the same pool, and its granted scope.
    m2m_client_id: String,
}

/// Create the pool, its app clients and its user, all on the published endpoint.
async fn fixture(router: &MultiServiceRouter, pool_name: &str, username: &str) -> Fixture {
    let host = Some(PUBLISHED_HOST);
    let created = ok(router, host, "CreateUserPool", &json!({ "PoolName": pool_name })).await;
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();

    let client = ok(
        router,
        host,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": format!("{pool_name}-client"),
            // The flows the rows sign in and refresh with.
            "ExplicitAuthFlows": [
                "ALLOW_ADMIN_USER_PASSWORD_AUTH",
                "ALLOW_USER_PASSWORD_AUTH",
                "ALLOW_REFRESH_TOKEN_AUTH",
            ],
        }),
    )
    .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();

    ok(
        router,
        host,
        "CreateResourceServer",
        &json!({
            "UserPoolId": user_pool_id,
            "Identifier": RESOURCE_SERVER,
            "Name": "suite resource server",
            "Scopes": [{ "ScopeName": SCOPE_NAME, "ScopeDescription": "read access" }],
        }),
    )
    .await;
    let m2m = ok(
        router,
        host,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": format!("{pool_name}-m2m"),
            "ClientSecret": CLIENT_SECRET,
            "ExplicitAuthFlows": ["ALLOW_CLIENT_TOKEN_AUTH"],
            "AllowedOAuthFlows": ["client_credentials"],
            "AllowedOAuthFlowsUserPoolClient": true,
            "AllowedOAuthScopes": [format!("{RESOURCE_SERVER}/{SCOPE_NAME}")],
        }),
    )
    .await;
    let m2m_client_id = m2m["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();

    ok(
        router,
        host,
        "AdminCreateUser",
        &json!({ "UserPoolId": user_pool_id, "Username": username }),
    )
    .await;
    ok(
        router,
        host,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": user_pool_id,
            "Username": username,
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;

    Fixture {
        user_pool_id,
        client_id,
        username: username.to_string(),
        m2m_client_id,
    }
}

/// Every token minted for a request announces, as its issuer, the endpoint that request
/// was served on with the pool's id appended — on each minting path and for each signed
/// token of it.
#[tokio::test]
async fn a_minted_token_issues_from_the_endpoint_its_request_was_served_on() {
    let (router, _engine) = router_and_engine();
    let pool = fixture(&router, "issuer-endpoint-pool", "wren").await;

    for row in ENDPOINTS {
        let case = row.case;
        let expected = format!("https://{}/{}", row.authority, pool.user_pool_id);

        // A sign-in: the id token and the access token of one unsigned USER_PASSWORD_AUTH
        // call, which is the flow a client with no credentials uses.
        let signed_in = ok(
            &router,
            row.host,
            "InitiateAuth",
            &json!({
                "ClientId": pool.client_id,
                "AuthFlow": "USER_PASSWORD_AUTH",
                "AuthParameters": { "USERNAME": pool.username, "PASSWORD": GOOD_PASSWORD },
            }),
        )
        .await;
        let result = &signed_in["AuthenticationResult"];
        for member in ["IdToken", "AccessToken"] {
            let token = result[member]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a sign-in returns {member}"));
            let actual = claim(token, "iss");
            assert_eq!(
                actual, expected,
                "case {case:?} / InitiateAuth {member}: expected iss {expected:?}, \
                 got {actual:?}"
            );
        }
        // The claim is composed of the endpoint and the POOL, so its last path segment is
        // the id of the pool that minted it and nothing else is in the path.
        let id_token = result["IdToken"].as_str().expect("an id token");
        assert_eq!(
            claim(id_token, "iss")
                .rsplit('/')
                .next()
                .unwrap_or_default(),
            pool.user_pool_id,
            "case {case:?}: the issuer's last segment is the minting pool's id"
        );

        // A refresh on the same endpoint mints a fresh pair, which issues from there too.
        let refresh_token = result["RefreshToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a sign-in returns a RefreshToken"))
            .to_string();
        let refreshed = ok(
            &router,
            row.host,
            "InitiateAuth",
            &json!({
                "ClientId": pool.client_id,
                "AuthFlow": "REFRESH_TOKEN_AUTH",
                "AuthParameters": { "REFRESH_TOKEN": refresh_token },
            }),
        )
        .await;
        for member in ["IdToken", "AccessToken"] {
            let token = refreshed["AuthenticationResult"][member]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a refresh returns {member}"));
            let actual = claim(token, "iss");
            assert_eq!(
                actual, expected,
                "case {case:?} / refreshed {member}: expected iss {expected:?}, got {actual:?}"
            );
        }

        // An admin sign-in, the signed path, on the same endpoint.
        let admin = ok(
            &router,
            row.host,
            "AdminInitiateAuth",
            &json!({
                "UserPoolId": pool.user_pool_id,
                "ClientId": pool.client_id,
                "AuthFlow": "ADMIN_USER_PASSWORD_AUTH",
                "AuthParameters": { "USERNAME": pool.username, "PASSWORD": GOOD_PASSWORD },
            }),
        )
        .await;
        for member in ["IdToken", "AccessToken"] {
            let token = admin["AuthenticationResult"][member]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: an admin sign-in returns {member}"));
            let actual = claim(token, "iss");
            assert_eq!(
                actual, expected,
                "case {case:?} / AdminInitiateAuth {member}: expected iss {expected:?}, \
                 got {actual:?}"
            );
        }

        // The machine-to-machine grant, whose access token is issued to the app client and
        // carries the same issuer.
        let granted = ok(
            &router,
            row.host,
            "GetClientToken",
            &json!({ "ClientId": pool.m2m_client_id, "Secret": CLIENT_SECRET }),
        )
        .await;
        let token = granted["ClientAuthenticationResult"]["AccessToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a grant returns an AccessToken"));
        let actual = claim(token, "iss");
        assert_eq!(
            actual, expected,
            "case {case:?} / GetClientToken AccessToken: expected iss {expected:?}, \
             got {actual:?}"
        );
    }
}

/// The issuer a token announces is a location its holder can FETCH the pool's key set
/// from: an anonymous GET of `<iss>/.well-known/jwks.json`, on the endpoint the issuer
/// names, serves the key the token's `kid` names.
#[tokio::test]
async fn a_tokens_issuer_leads_to_the_key_set_on_the_endpoint_it_names() {
    let (router, _engine) = router_and_engine();
    let pool = fixture(&router, "issuer-discovery-pool", "quinn").await;

    for row in ENDPOINTS {
        let case = row.case;
        let signed_in = ok(
            &router,
            row.host,
            "InitiateAuth",
            &json!({
                "ClientId": pool.client_id,
                "AuthFlow": "USER_PASSWORD_AUTH",
                "AuthParameters": { "USERNAME": pool.username, "PASSWORD": GOOD_PASSWORD },
            }),
        )
        .await;
        let token = signed_in["AuthenticationResult"]["IdToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a sign-in returns an IdToken"))
            .to_string();
        let issuer = claim(&token, "iss");
        let kid = jose_header(&token)["kid"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a signed token names its key by kid"))
            .to_string();

        // A verifier follows the issuer itself: the authority it names is the endpoint the
        // request is addressed to, and the rest of it is the path.
        let authority = issuer
            .strip_prefix("https://")
            .unwrap_or_else(|| panic!("case {case:?}: an issuer is an https URL, got {issuer:?}"));
        let (authority, path) = authority
            .split_once('/')
            .unwrap_or_else(|| panic!("case {case:?}: an issuer carries the pool in its path"));
        let discovery = format!("/{path}/.well-known/jwks.json");

        // NO credential: a key set is public, exactly as AWS serves it.
        let request = http::Request::builder()
            .method("GET")
            .uri(&discovery)
            .header("host", authority)
            .body(Bytes::new())
            .expect("the request is well formed");
        let response = router.dispatch(request).await;
        let status = response.status().as_u16();
        assert_eq!(
            status,
            200,
            "case {case:?}: GET https://{authority}{discovery} must serve the pool's key \
             set, got {status} with {:?}",
            String::from_utf8_lossy(response.body())
        );
        let served: Value = serde_json::from_slice(response.body()).unwrap_or_else(|error| {
            panic!(
                "case {case:?}: the key set must be JSON ({error}): {:?}",
                String::from_utf8_lossy(response.body())
            )
        });
        let names_the_key = served["keys"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: a key set has a keys list, got {served}"))
            .iter()
            .any(|key| key["kid"].as_str() == Some(kid.as_str()));
        assert!(
            names_the_key,
            "case {case:?}: the key set at the announced issuer must hold the token's key \
             {kid:?}, got {served}"
        );
    }
}

/// The reading every verifier of a user-pool token shares: which issuers name one pool.
///
/// Each row is an issuer a holder could present, and whether it names the pool. The
/// endpoints a token really issues from are the rows of [`ENDPOINTS`], so the minted
/// values are accepted here by construction; the rest are values no endpoint of this pool
/// mints.
#[test]
fn an_issuer_names_a_pool_only_on_an_endpoint_that_serves_it() {
    let pool_id = "us-east-1_abc123def";
    let rows: &[(&str, String, bool)] = &[
        (
            "the pool's published regional endpoint",
            format!("https://{PUBLISHED_HOST}/{pool_id}"),
            true,
        ),
        (
            "the emulator's loopback endpoint",
            format!("https://127.0.0.1:9297/{pool_id}"),
            true,
        ),
        (
            "a named local endpoint with a port",
            format!("https://localhost:4566/{pool_id}"),
            true,
        ),
        (
            "a bracketed IPv6 loopback endpoint",
            format!("https://[::1]:9297/{pool_id}"),
            true,
        ),
        (
            "the same issuer with a trailing slash, as a verifier may configure it",
            format!("https://{PUBLISHED_HOST}/{pool_id}/"),
            true,
        ),
        (
            "an endpoint reached over plain HTTP",
            format!("http://127.0.0.1:9297/{pool_id}"),
            true,
        ),
        (
            "this service's published endpoint in another region, where the pool is not",
            format!("https://cognito-idp.eu-west-1.amazonaws.com/{pool_id}"),
            false,
        ),
        (
            "another service's endpoint",
            format!("https://s3.us-east-1.amazonaws.com/{pool_id}"),
            false,
        ),
        (
            "the published endpoint of a different pool",
            format!("https://{PUBLISHED_HOST}/us-east-1_999zzz000"),
            false,
        ),
        (
            "an issuer with the pool deeper in its path",
            format!("https://{PUBLISHED_HOST}/tenants/{pool_id}"),
            false,
        ),
        (
            "an issuer with no scheme, which is a provider NAME and not a URL",
            format!("{PUBLISHED_HOST}/{pool_id}"),
            false,
        ),
        ("an empty issuer", String::new(), false),
    ];

    for (case, issuer, expected) in rows {
        let actual = state::is_pool_issuer(REGION, pool_id, issuer);
        assert_eq!(
            actual, *expected,
            "case {case:?} ({issuer:?}): expected is_pool_issuer == {expected}, got {actual}"
        );
    }
}
