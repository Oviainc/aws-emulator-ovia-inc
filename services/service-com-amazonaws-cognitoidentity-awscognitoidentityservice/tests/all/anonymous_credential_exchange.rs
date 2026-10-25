// SPDX-License-Identifier: CC0-1.0

//! The credential exchange a client performs with NO Amazon Web Services credentials,
//! driven through the same `MultiServiceRouter` the production router builds.
//!
//! Two rules are under test.
//!
//! * ROUTING. The framework resolves a request's routing key from the SigV4 credential
//!   scope first and from `X-Amz-Target` only when there is no scope, and those two
//!   carry different names — the signing name (`cognito-identity`) and the model's
//!   service shape name (`AWSCognitoIdentityService`). An unsigned request that names
//!   this service's shape must therefore reach this service, and one that names another
//!   service's must not. The unsigned rows are addressed to a host that is an ADDRESS
//!   rather than a service name, so the target prefix is the only signal left, and one
//!   of them runs through an AUTHENTICATING router, which is how the emulator serves
//!   traffic for real.
//! * TRUSTED LOGIN PROVIDERS. "Any provided logins will be validated against supported
//!   login providers" (`GetCredentialsForIdentity` in this service's model). A `Logins`
//!   entry that names an Amazon Cognito user pool carries its issuer in the provider
//!   name itself (`cognito-idp.<region>.amazonaws.com/<UserPoolId>`), so the identity
//!   pool's stored configuration decides it: the login is accepted only when one of the
//!   pool's `CognitoIdentityProviders` names that user pool, and a login naming any
//!   other user pool is refused `NotAuthorizedException` whatever token it carries. A
//!   login of any other kind is decided by the provider that issued its token, which
//!   this emulator does not reach, so the pool's configuration does not refuse it.
//!
//! Each table row names the case it asserts.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_cognitoidentity_awscognitoidentityservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::identity_pool_flow::error_code;

/// This service's SigV4 signing name, the 4th field of a credential scope.
const SIGNING_NAME: &str = "cognito-identity";

/// The local name of this service's Smithy shape, which is the `X-Amz-Target` prefix a
/// client writes (`smithy_query(query="service_info")`:
/// `com.amazonaws.cognitoidentity#AWSCognitoIdentityService`).
const TARGET_PREFIX: &str = "AWSCognitoIdentityService";

/// The shape name of the SEPARATE Amazon Cognito user-pools service
/// (`com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService`): a target
/// this service must NOT claim.
const ANOTHER_TARGET_PREFIX: &str = "AWSCognitoIdentityProviderService";

/// The emulator's own endpoint, which is what an unsigned client is pointed at: an
/// address, so the `Host` header names no service and cannot route the request.
const EMULATOR_HOST: &str = "127.0.0.1:9297";

/// This service's real regional endpoint host.
const ENDPOINT_HOST: &str = "cognito-identity.us-east-1.amazonaws.com";

/// How a request is addressed — the three signals the claim rule reads.
#[derive(Clone, Copy, Debug)]
struct Addressing {
    /// The service field of the SigV4 credential scope, or `None` for an unsigned
    /// (anonymous) request, which is how the credential exchange is called.
    scope: Option<&'static str>,
    /// The `X-Amz-Target` service prefix, or `None` to send no target at all.
    target_prefix: Option<&'static str>,
    host: &'static str,
    /// The request URI, which is `/` for every `awsJson` operation.
    uri: &'static str,
}

/// A signed call, as the SDK sends every credentialed operation of this service.
const SIGNED: Addressing = Addressing {
    scope: Some(SIGNING_NAME),
    target_prefix: Some(TARGET_PREFIX),
    host: ENDPOINT_HOST,
    uri: "/",
};

/// An unsigned call to the emulator's own endpoint: no credential scope, and a host that
/// names no service, so `X-Amz-Target` is the only routing signal.
const ANONYMOUS: Addressing = Addressing {
    scope: None,
    target_prefix: Some(TARGET_PREFIX),
    host: EMULATOR_HOST,
    uri: "/",
};

/// One emulator with just this service mounted over `engine`.
///
/// `authenticating` turns request authentication ON, as the emulator runs it: a signed
/// request would then need a real signature, while an operation whose model lists no
/// auth scheme is served unsigned.
fn router(engine: &Arc<engine::InMemoryEngine>, authenticating: bool) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router.set_request_authentication(authenticating);
    router
}

/// The shared engine every router in one test mounts, so state written through one is
/// read through the other.
fn engine() -> Arc<engine::InMemoryEngine> {
    Arc::new(engine::InMemoryEngine::new())
}

/// Dispatch `operation` with `body`, addressed as `addressing` says, and return the
/// status and JSON response body.
async fn dispatch(
    router: &MultiServiceRouter,
    addressing: Addressing,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let mut request = http::Request::builder()
        .method("POST")
        .uri(addressing.uri)
        .header("host", addressing.host)
        .header("content-type", "application/x-amz-json-1.1");
    if let Some(prefix) = addressing.target_prefix {
        request = request.header("x-amz-target", format!("{prefix}.{operation}"));
    }
    if let Some(scope) = addressing.scope {
        request = request.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/{scope}/aws4_request, \
                 SignedHeaders=host;x-amz-date, Signature=0000"
            ),
        );
    }
    let response = router
        .dispatch(
            request
                .body(bytes::Bytes::from(body.to_string()))
                .expect("the request is well formed"),
        )
        .await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let payload = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, payload)
}

/// Call `operation` signed, requiring success, and return its response body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, payload) = dispatch(router, SIGNED, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} must succeed, got {status} with {payload}"
    );
    payload
}

/// A user pool's `Logins` provider name: `cognito-idp.<region>.amazonaws.com/<pool id>`
/// (`GetId.Logins` in this service's model).
fn user_pool_provider(user_pool_id: &str) -> String {
    format!("cognito-idp.us-east-1.amazonaws.com/{user_pool_id}")
}

/// The provider name of the pool the fixture pool below DOES trust, and of one it does
/// not: two user pools, told apart by their id exactly as a client's token is.
const TRUSTED_USER_POOL: &str = "us-east-1_trusted1";
const UNTRUSTED_USER_POOL: &str = "us-east-1_foreign1";

/// An IAM OpenID Connect provider ARN, whose provider name is the issuer it ends in
/// (`GetId.Logins` names a provider by that issuer).
const OIDC_PROVIDER_ARN: &str = "arn:aws:iam::123456789012:oidc-provider/accounts.example.com";
const OIDC_PROVIDER_NAME: &str = "accounts.example.com";

/// A pool configured with one Amazon Cognito user pool, one public login provider, one
/// OIDC provider and a developer provider name — one of each kind of trust a pool can
/// hold — and its minted id.
async fn pool_with_providers(router: &MultiServiceRouter) -> String {
    let created = ok(
        router,
        "CreateIdentityPool",
        &json!({
            "IdentityPoolName": "trusted-providers",
            "AllowUnauthenticatedIdentities": true,
            "CognitoIdentityProviders": [
                { "ProviderName": user_pool_provider(TRUSTED_USER_POOL), "ClientId": "abcd1234" },
            ],
            // The public provider names `GetId.Logins` publishes.
            "SupportedLoginProviders": { "graph.facebook.com": "0123456789" },
            "OpenIdConnectProviderARNs": [OIDC_PROVIDER_ARN],
            "DeveloperProviderName": "login.example.devs",
        }),
    )
    .await;
    created["IdentityPoolId"]
        .as_str()
        .expect("CreateIdentityPool returns an IdentityPoolId")
        .to_string()
}

/// ROUTING: this one endpoint answers on its signing name and on its shape name, and on
/// nothing else — the rule an unsigned (anonymous) `GetId` depends on.
#[tokio::test]
async fn an_unsigned_request_naming_this_service_reaches_it() {
    let engine = engine();
    let setup = router(&engine, false);
    let pool_id = pool_with_providers(&setup).await;

    // (case, addressing, authenticating, expected status, expected error code)
    let rows: &[(&str, Addressing, bool, u16, Option<&str>)] = &[
        ("signed for it, targeted at it", SIGNED, false, 200, None),
        (
            "unsigned, targeted at it, on the emulator's own host",
            ANONYMOUS,
            false,
            200,
            None,
        ),
        (
            "unsigned, targeted at it, with request authentication on",
            ANONYMOUS,
            true,
            200,
            None,
        ),
        (
            "unsigned, addressed by this service's endpoint host",
            Addressing {
                scope: None,
                target_prefix: Some(TARGET_PREFIX),
                host: ENDPOINT_HOST,
                uri: "/",
            },
            false,
            200,
            None,
        ),
        (
            "unsigned, lower-cased target prefix",
            Addressing {
                scope: None,
                target_prefix: Some("awscognitoidentityservice"),
                host: EMULATOR_HOST,
                uri: "/",
            },
            false,
            200,
            None,
        ),
        (
            "unsigned, targeted at the separate user-pools service",
            Addressing {
                scope: None,
                target_prefix: Some(ANOTHER_TARGET_PREFIX),
                host: EMULATOR_HOST,
                uri: "/",
            },
            false,
            404,
            Some("UnknownService"),
        ),
        (
            "signed for another service, targeted at it",
            Addressing {
                scope: Some("lambda"),
                target_prefix: Some(TARGET_PREFIX),
                host: EMULATOR_HOST,
                uri: "/",
            },
            false,
            404,
            Some("UnknownService"),
        ),
        (
            "unsigned, targeted at it, on another model's REST path",
            Addressing {
                scope: None,
                target_prefix: Some(TARGET_PREFIX),
                host: EMULATOR_HOST,
                uri: "/identitypools/pool-1",
            },
            false,
            404,
            Some("UnknownService"),
        ),
        (
            "no routing signal at all",
            Addressing {
                scope: None,
                target_prefix: None,
                host: EMULATOR_HOST,
                uri: "/",
            },
            false,
            404,
            Some("UnknownService"),
        ),
    ];

    for (case, addressing, authenticating, expected_status, expected_code) in rows {
        let router = router(&engine, *authenticating);
        let (status, payload) = dispatch(
            &router,
            *addressing,
            "GetId",
            &json!({ "IdentityPoolId": pool_id }),
        )
        .await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with {payload}"
        );
        match expected_code {
            Some(code) => assert_eq!(
                error_code(&payload),
                *code,
                "case {case:?}: expected error code {code}, got {payload}"
            ),
            None => assert!(
                payload["IdentityId"].is_string(),
                "case {case:?}: a served GetId carries an IdentityId, got {payload}"
            ),
        }
    }
}

/// TRUSTED LOGIN PROVIDERS: `GetId` accepts a `Logins` entry naming an Amazon Cognito
/// user pool only when the identity pool is configured with that user pool, over a table
/// spanning the user pools it is and is not configured with and the login kinds its
/// configuration does not decide.
#[tokio::test]
async fn a_login_provider_the_pool_does_not_trust_is_not_authorized() {
    let engine = engine();
    let emulator = router(&engine, false);
    let pool_id = pool_with_providers(&emulator).await;

    // (case, provider name, authorized)
    let rows: Vec<(&str, String, bool)> = vec![
        (
            "the user pool the pool is configured with",
            user_pool_provider(TRUSTED_USER_POOL),
            true,
        ),
        (
            "another user pool, validly signed but not a provider of this pool",
            user_pool_provider(UNTRUSTED_USER_POOL),
            false,
        ),
        (
            "a user pool of another region",
            format!("cognito-idp.eu-west-1.amazonaws.com/{TRUSTED_USER_POOL}"),
            false,
        ),
        (
            "a user pool whose id is a prefix of the configured one",
            user_pool_provider("us-east-1_trusted"),
            false,
        ),
        (
            "a public provider in SupportedLoginProviders",
            "graph.facebook.com".to_string(),
            true,
        ),
        (
            "an OIDC issuer: decided by the issuer, not by this pool's configuration",
            OIDC_PROVIDER_NAME.to_string(),
            true,
        ),
        (
            "the pool's own developer provider name",
            "login.example.devs".to_string(),
            true,
        ),
    ];

    for (case, provider, authorized) in &rows {
        let (status, payload) = dispatch(
            &emulator,
            ANONYMOUS,
            "GetId",
            &json!({
                "IdentityPoolId": pool_id,
                "Logins": { provider.as_str(): "a.token.value" },
            }),
        )
        .await;
        if *authorized {
            assert_eq!(
                status, 200,
                "case {case:?} ({provider}): an accepted provider must be served, got {status} \
                 with {payload}"
            );
            assert!(
                payload["IdentityId"].is_string(),
                "case {case:?} ({provider}): expected an IdentityId, got {payload}"
            );
        } else {
            assert_eq!(
                (status, error_code(&payload).as_str()),
                (403, "NotAuthorizedException"),
                "case {case:?} ({provider}): expected (403, NotAuthorizedException), got \
                 ({status}, {payload})"
            );
            // The caller of this refusal holds no credential, so the refusal reports
            // neither the provider it presented nor the pool's own configuration.
            let message = payload["message"].as_str().unwrap_or_default();
            assert!(
                !message.contains(provider.as_str()) && !message.contains(pool_id.as_str()),
                "case {case:?}: the refusal must name neither the presented provider nor the \
                 pool, got {message:?}"
            );
        }
    }
}

/// TRUSTED LOGIN PROVIDERS, on the operation that exchanges an identity for a session:
/// `GetCredentialsForIdentity` validates the same `Logins` against the pool the IDENTITY
/// belongs to, and a pool configured with no provider at all trusts none.
#[tokio::test]
async fn credentials_for_an_identity_validate_the_logins_they_are_presented_with() {
    let engine = engine();
    let emulator = router(&engine, false);
    let configured_pool = pool_with_providers(&emulator).await;
    let bare_pool = ok(
        &emulator,
        "CreateIdentityPool",
        &json!({
            "IdentityPoolName": "no-providers",
            "AllowUnauthenticatedIdentities": true,
        }),
    )
    .await["IdentityPoolId"]
        .as_str()
        .expect("CreateIdentityPool returns an IdentityPoolId")
        .to_string();

    // An identity in each pool, minted unauthenticated so neither carries a login yet.
    let mut identities = Vec::new();
    for pool_id in [&configured_pool, &bare_pool] {
        let minted = ok(&emulator, "GetId", &json!({ "IdentityPoolId": pool_id })).await;
        identities.push(
            minted["IdentityId"]
                .as_str()
                .expect("GetId returns an IdentityId")
                .to_string(),
        );
    }
    let [configured_identity, bare_identity] = <[String; 2]>::try_from(identities)
        .expect("one identity was minted per pool");

    // (case, identity, logins, authorized)
    let rows: Vec<(&str, &String, Option<String>, bool)> = vec![
        (
            "no Logins at all: an unauthenticated identity of a pool that allows them",
            &configured_identity,
            None,
            true,
        ),
        (
            "a login of the user pool the identity's pool trusts",
            &configured_identity,
            Some(user_pool_provider(TRUSTED_USER_POOL)),
            true,
        ),
        (
            "a login of another user pool, not a provider of the identity's pool",
            &configured_identity,
            Some(user_pool_provider(UNTRUSTED_USER_POOL)),
            false,
        ),
        (
            "a login of a pool configured with no user-pool provider at all",
            &bare_identity,
            Some(user_pool_provider(TRUSTED_USER_POOL)),
            false,
        ),
        (
            "no Logins against a pool configured with no provider",
            &bare_identity,
            None,
            true,
        ),
        (
            "a public-provider login, which this pool's configuration does not decide",
            &bare_identity,
            Some("graph.facebook.com".to_string()),
            true,
        ),
    ];

    for (case, identity_id, provider, authorized) in &rows {
        let body = match provider {
            Some(provider) => json!({
                "IdentityId": identity_id,
                "Logins": { provider.as_str(): "a.token.value" },
            }),
            None => json!({ "IdentityId": identity_id }),
        };
        let (status, payload) =
            dispatch(&emulator, ANONYMOUS, "GetCredentialsForIdentity", &body).await;
        if *authorized {
            assert_eq!(
                status, 200,
                "case {case:?}: expected a session, got {status} with {payload}"
            );
            assert!(
                payload["Credentials"]["AccessKeyId"].is_string(),
                "case {case:?}: expected credentials, got {payload}"
            );
        } else {
            assert_eq!(
                (status, error_code(&payload).as_str()),
                (403, "NotAuthorizedException"),
                "case {case:?}: expected (403, NotAuthorizedException), got ({status}, {payload})"
            );
        }
    }
}
