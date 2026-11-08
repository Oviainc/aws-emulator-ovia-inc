// SPDX-License-Identifier: CC0-1.0

//! A login presented to an identity pool whose stored configuration names NO identity
//! provider at all, over the same `MultiServiceRouter` the production router builds.
//!
//! RULE under test: an identity pool that is configured with no identity provider of any
//! kind federates nobody, so a `Logins` entry whose issuer this service decides is refused
//! `NotAuthorizedException` on the POOL — `Identity pool - <IdentityPoolId> does not have
//! identity providers configured.` — while a pool that DOES name providers, none of which
//! is the presented issuer, is refused on the token it presented (`Token is not from a
//! supported provider of this identity pool.`). Which of the two a caller gets is decided
//! from the pool's configuration as stored at the instant of the call, so a configuration
//! change moves a pool from one refusal to the other.
//!
//! `iam_oidc_federation.rs` owns the ADMISSION rule those refusals are the negative half
//! of, and this suite reuses its emulator, its pools and its two assertion helpers so a
//! refusal is asserted the one way across both.
//!
//! The rows differ along what the rule generalises over: WHICH of the five provider members
//! a pool holds something in — the four federated ones a `Logins` entry can be keyed by
//! (`SupportedLoginProviders`, `CognitoIdentityProviders`, `OpenIdConnectProviderARNs`,
//! `SamlProviderARNs`) and the pool's own `DeveloperProviderName` — against a pool holding
//! nothing in any of them. Every row presents the SAME login, so the configuration is the
//! only thing that varies.

use serde_json::{json, Value};

use crate::iam_oidc_federation::{
    admitted, id_token, refused, user_pool_issuer, Emulator, CLIENT_A, NOT_AUTHORIZED,
};
use crate::identity_pool_flow::error_code;

/// The user pool whose ID token every row below presents, and the one row seven's pool is
/// configured with.
const PRESENTED_USER_POOL: &str = "us-east-1_presented1";

/// Another user pool, named by the row whose pool trusts a DIFFERENT Amazon Cognito user
/// pool than the one presented.
const SIBLING_USER_POOL: &str = "us-east-1_sibling01";

/// An OpenID Connect issuer that is not an Amazon Cognito user pool, for the row whose pool
/// holds an `OpenIdConnectProviderARNs` entry.
const OTHER_ISSUER: &str = "auth.example.org/realms/other";

/// The two sentences the refusals are worded with, spelled out here so the wire-visible text
/// is pinned by the test rather than read back out of the crate. The first one's variable
/// part is the pool the call addressed.
fn no_identity_providers_message(identity_pool_id: &str) -> String {
    format!("Identity pool - {identity_pool_id} does not have identity providers configured.")
}
const UNSUPPORTED_PROVIDER_MESSAGE: &str =
    "Token is not from a supported provider of this identity pool.";

/// One row: a pool configuration, and whether a login of [`PRESENTED_USER_POOL`] is
/// admitted, refused on the pool, or refused on the token.
struct Row {
    case: &'static str,
    /// The pool name, which is also the name the fixture's role is seeded under.
    name: &'static str,
    /// What the pool is created with beside the members every fixture pool carries.
    configuration: fn(&str) -> Value,
    /// What the login is answered with.
    expected: Expected,
}

/// What a row expects of the login it presents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The pool trusts the presented issuer: the login reaches an identity.
    Admitted,
    /// The pool names no identity provider at all.
    RefusedOnThePool,
    /// The pool names providers, and the presented issuer is none of them.
    RefusedOnTheToken,
}

/// A configuration naming nothing: the pool holds none of the five provider members.
fn no_providers(_oidc_arn: &str) -> Value {
    json!({})
}

/// Each remaining row's one configured member. `oidc_arn` is the ARN of the IAM OpenID
/// Connect provider the fixture registered for [`OTHER_ISSUER`]; the other arms ignore it.
fn supported_login_providers(_oidc_arn: &str) -> Value {
    json!({"SupportedLoginProviders": {"graph.facebook.com": "1234567890123456"}})
}
fn developer_provider(_oidc_arn: &str) -> Value {
    json!({"DeveloperProviderName": "login.example.tenant"})
}
fn saml_provider(_oidc_arn: &str) -> Value {
    json!({"SamlProviderARNs": ["arn:aws:iam::000000000000:saml-provider/corp"]})
}
fn sibling_user_pool(_oidc_arn: &str) -> Value {
    json!({"CognitoIdentityProviders": [
        {"ProviderName": user_pool_issuer(SIBLING_USER_POOL), "ClientId": CLIENT_A}
    ]})
}
fn oidc_provider_arn(oidc_arn: &str) -> Value {
    json!({"OpenIdConnectProviderARNs": [oidc_arn]})
}
fn the_presented_user_pool(_oidc_arn: &str) -> Value {
    json!({"CognitoIdentityProviders": [
        {"ProviderName": user_pool_issuer(PRESENTED_USER_POOL), "ClientId": CLIENT_A}
    ]})
}

/// Every provider member a pool can be configured with, one row each, against the pool
/// configured with none.
const ROWS: &[Row] = &[
    Row {
        case: "a pool configured with no identity provider at all",
        name: "no-providers",
        configuration: no_providers,
        expected: Expected::RefusedOnThePool,
    },
    Row {
        case: "a pool whose only provider is a public login provider",
        name: "only-supported-logins",
        configuration: supported_login_providers,
        expected: Expected::RefusedOnTheToken,
    },
    Row {
        case: "a pool whose only provider is its own developer provider",
        name: "only-developer-provider",
        configuration: developer_provider,
        expected: Expected::RefusedOnTheToken,
    },
    Row {
        case: "a pool whose only provider is a SAML provider",
        name: "only-saml-provider",
        configuration: saml_provider,
        expected: Expected::RefusedOnTheToken,
    },
    Row {
        case: "a pool whose only provider is ANOTHER Amazon Cognito user pool",
        name: "only-sibling-user-pool",
        configuration: sibling_user_pool,
        expected: Expected::RefusedOnTheToken,
    },
    Row {
        case: "a pool whose only provider is an OpenID Connect provider ARN",
        name: "only-oidc-arn",
        configuration: oidc_provider_arn,
        expected: Expected::RefusedOnTheToken,
    },
    Row {
        case: "a pool configured with the presented user pool itself",
        name: "the-presented-user-pool",
        configuration: the_presented_user_pool,
        expected: Expected::Admitted,
    },
];

/// A login of the presented user pool, as a `Logins` map.
fn presented_logins() -> Value {
    let issuer = user_pool_issuer(PRESENTED_USER_POOL);
    json!({ issuer.clone(): id_token(&issuer, json!(CLIENT_A)) })
}

/// Mint an identity of `pool_id` the refusal rows can address
/// `GetCredentialsForIdentity`/`GetOpenIdToken` with: every fixture pool admits
/// unauthenticated identities, so a `GetId` carrying no `Logins` answers one.
async fn an_identity_of(emulator: &Emulator, pool_id: &str) -> String {
    let minted = emulator
        .ok("GetId", &json!({"IdentityPoolId": pool_id}))
        .await;
    minted["IdentityId"]
        .as_str()
        .unwrap_or_else(|| panic!("GetId answers an IdentityId for {pool_id}: {minted}"))
        .to_string()
}

/// Which refusal a login of an Amazon Cognito user pool gets is decided by WHICH provider
/// members the pool holds something in — nothing at all in any of them being the one state
/// that is refused on the pool.
#[tokio::test]
async fn a_pool_with_no_provider_configured_refuses_a_login_on_the_pool() {
    let emulator = Emulator::new();
    let oidc_arn = emulator.oidc_provider(OTHER_ISSUER, &[CLIENT_A]).await;
    let logins = presented_logins();

    for row in ROWS {
        let pool_id = emulator
            .pool(row.name, (row.configuration)(&oidc_arn))
            .await;
        match row.expected {
            Expected::Admitted => {
                admitted(&emulator, row.case, &pool_id, &logins).await;
            }
            Expected::RefusedOnThePool | Expected::RefusedOnTheToken => {
                let identity_id = an_identity_of(&emulator, &pool_id).await;
                let message = match row.expected {
                    Expected::RefusedOnThePool => no_identity_providers_message(&pool_id),
                    _ => UNSUPPORTED_PROVIDER_MESSAGE.to_string(),
                };
                refused(
                    &emulator,
                    row.case,
                    &pool_id,
                    &identity_id,
                    &logins,
                    &message,
                )
                .await;
                // The basic (classic) flow's entry point decides a presented login against
                // the same pool, so it answers the same refusal: the wording belongs to the
                // pool's configuration, not to one operation.
                let (status, payload) = emulator
                    .call(
                        "GetOpenIdToken",
                        &json!({"IdentityId": &identity_id, "Logins": &logins}),
                    )
                    .await;
                assert_eq!(
                    (status, error_code(&payload).as_str()),
                    (400, NOT_AUTHORIZED),
                    "case {:?} on GetOpenIdToken: expected (400, {NOT_AUTHORIZED}), got \
                     ({status}, {payload})",
                    row.case
                );
                assert_eq!(
                    payload["message"].as_str(),
                    Some(message.as_str()),
                    "case {:?} on GetOpenIdToken: expected the message {message:?}, got \
                     {payload}",
                    row.case
                );
            }
        }
    }
}

/// The refusal names the pool the call addressed: two unconfigured pools in one account are
/// each refused with their own `IdentityPoolId`, so the sentence's variable part is read
/// from the call and the store rather than fixed.
#[tokio::test]
async fn the_refusal_names_the_pool_the_call_addressed() {
    let emulator = Emulator::new();
    let logins = presented_logins();
    let first = emulator.pool("unconfigured-one", json!({})).await;
    let second = emulator.pool("unconfigured-two", json!({})).await;
    assert_ne!(
        first, second,
        "the fixture needs two distinct pools, got {first:?} twice"
    );
    for pool_id in [&first, &second] {
        let identity_id = an_identity_of(&emulator, pool_id).await;
        refused(
            &emulator,
            "an unconfigured pool among several",
            pool_id,
            &identity_id,
            &logins,
            &no_identity_providers_message(pool_id),
        )
        .await;
    }
}

/// A configuration CHANGE moves a pool across the rule: a pool whose providers an
/// `UpdateIdentityPool` removes stops refusing a login on the token it presented and starts
/// refusing it on the pool, with no other call in between.
#[tokio::test]
async fn removing_a_pools_last_provider_moves_the_refusal_onto_the_pool() {
    let case = "a pool whose last identity provider an update removes";
    let emulator = Emulator::new();
    let logins = presented_logins();
    let pool_id = emulator
        .pool("provider-removed", sibling_user_pool(""))
        .await;
    let identity_id = an_identity_of(&emulator, &pool_id).await;

    // While the pool still names a provider, the presented issuer is simply not among them.
    refused(
        &emulator,
        "before the update, the pool still names a provider",
        &pool_id,
        &identity_id,
        &logins,
        UNSUPPORTED_PROVIDER_MESSAGE,
    )
    .await;

    // `UpdateIdentityPool` replaces the pool's configuration with what the call carries, so
    // a call carrying no provider member leaves the pool with no identity provider at all.
    emulator
        .ok(
            "UpdateIdentityPool",
            &json!({
                "IdentityPoolId": &pool_id,
                "IdentityPoolName": "provider-removed",
                "AllowUnauthenticatedIdentities": true,
            }),
        )
        .await;

    refused(
        &emulator,
        case,
        &pool_id,
        &identity_id,
        &logins,
        &no_identity_providers_message(&pool_id),
    )
    .await;
}
