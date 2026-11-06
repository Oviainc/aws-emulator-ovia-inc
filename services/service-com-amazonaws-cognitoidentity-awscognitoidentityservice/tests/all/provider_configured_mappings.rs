// SPDX-License-Identifier: CC0-1.0

//! Which identity provider a pool's principal-tag attribute map may be written for.
//!
//! RULE: `SetPrincipalTagAttributeMap` stores a mapping only for an identity provider the
//! pool's own FEDERATED-PROVIDER configuration declares — an entry of
//! `SupportedLoginProviders`, a `CognitoIdentityProviders` user pool, a `SamlProviderARNs`
//! or an `OpenIdConnectProviderARNs` provider — because the mapping maps the CLAIMS of a
//! token that provider issues onto the session tags of the role an identity of the pool
//! assumes. A name that configuration does not carry, the pool's own
//! `DeveloperProviderName` included, is refused with the modelled HTTP 400
//! `InvalidParameterException` and the message AWS builds from the request, and nothing is
//! stored for it.
//!
//! Each case asserts that rule over a TABLE spanning the class, its rows differing along
//! what the rule generalises over: WHICH configuration member declares the provider (so
//! every provider kind a pool can be fronted by is a row), the FORM of the name (a user
//! pool's issuer, a public provider's host, an ARN), and — for the refused rows — why the
//! name is not declared (a provider of another pool, a provider of no pool, the pool's own
//! developer provider, a prefix of a declared name, a declared name in another case).
//!
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds.

use serde_json::{json, Value};

use crate::identity_pool_flow::{error_code, Emulator};

/// An Amazon Cognito user pool's provider name, which is the issuer form `GetId.Logins`
/// documents (`cognito-idp.<region>.amazonaws.com/<UserPoolId>`).
const USER_POOL_PROVIDER: &str = "cognito-idp.us-east-1.amazonaws.com/us-east-1_declared1";

/// A SAML provider is named by the ARN of its IAM registration ("A list of the SAML
/// provider ARNs", this service's model).
const SAML_PROVIDER_ARN: &str = "arn:aws:iam::000000000000:saml-provider/declared-saml";

/// The developer provider the pools below are created with — "the 'domain' by which Cognito
/// will refer to your users", which is this account's own back end rather than a federated
/// provider, so no token of it carries claims a mapping could read.
const DEVELOPER_PROVIDER: &str = "login.declared.developer";

/// The message AWS words the refusal with, built from the provider the request named.
fn not_configured(provider: &str) -> String {
    format!("Provider {provider} not configured for Identity Pool")
}

/// The message an error response carries.
fn message(body: &Value) -> String {
    body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("an error response carries a message: {body}"))
        .to_string()
}

/// A pool fronted by one provider of every federated kind, plus a developer provider.
async fn pool(emulator: &Emulator, name: &str) -> String {
    emulator
        .ok(
            "CreateIdentityPool",
            &json!({
                "IdentityPoolName": name,
                "AllowUnauthenticatedIdentities": true,
                "DeveloperProviderName": DEVELOPER_PROVIDER,
                "SupportedLoginProviders": {"graph.facebook.com": "declared-app-id"},
                "CognitoIdentityProviders": [{
                    "ProviderName": USER_POOL_PROVIDER,
                    "ClientId": "declaredclient",
                }],
                "SamlProviderARNs": [SAML_PROVIDER_ARN],
                "OpenIdConnectProviderARNs": [
                    "arn:aws:iam::000000000000:oidc-provider/accounts.google.com",
                ],
            }),
        )
        .await["IdentityPoolId"]
        .as_str()
        .expect("CreateIdentityPool answers an IdentityPoolId")
        .to_string()
}

/// The mapping every row writes, and reads back.
fn principal_tags() -> Value {
    json!({"department": "custom:department"})
}

#[tokio::test]
async fn a_mapping_is_stored_only_for_a_provider_the_pool_declares() {
    let emulator = Emulator::new();
    let pool_id = pool(&emulator, "declared-providers").await;

    // (case, the provider the mapping is written for, declared by the pool?)
    let rows: Vec<(&str, String, bool)> = vec![
        (
            "a SupportedLoginProviders entry",
            "graph.facebook.com".to_string(),
            true,
        ),
        (
            "a CognitoIdentityProviders user pool",
            USER_POOL_PROVIDER.to_string(),
            true,
        ),
        (
            "a SamlProviderARNs provider",
            SAML_PROVIDER_ARN.to_string(),
            true,
        ),
        (
            "an OpenIdConnectProviderARNs issuer",
            "accounts.google.com".to_string(),
            true,
        ),
        (
            "the pool's own developer provider",
            DEVELOPER_PROVIDER.to_string(),
            false,
        ),
        (
            "a public provider the pool was never given",
            "www.amazon.com".to_string(),
            false,
        ),
        (
            "another account's user pool",
            "cognito-idp.us-east-1.amazonaws.com/us-east-1_other999".to_string(),
            false,
        ),
        (
            "a prefix of a declared user pool's name",
            "cognito-idp.us-east-1.amazonaws.com".to_string(),
            false,
        ),
        (
            "a declared name in another case",
            "GRAPH.FACEBOOK.COM".to_string(),
            false,
        ),
        (
            "a SAML provider ARN the pool does not carry",
            "arn:aws:iam::000000000000:saml-provider/other-saml".to_string(),
            false,
        ),
    ];

    for (case, provider, declared) in rows {
        let (status, body) = emulator
            .call(
                "SetPrincipalTagAttributeMap",
                &json!({
                    "IdentityPoolId": &pool_id,
                    "IdentityProviderName": &provider,
                    "PrincipalTags": principal_tags(),
                }),
            )
            .await;
        if declared {
            assert_eq!(
                status, 200,
                "case {case:?}: a mapping for a declared provider is stored, got {status} with \
                 {body}"
            );
            // Read back through the implemented reader: what was stored is what the get
            // reports.
            let got = emulator
                .ok(
                    "GetPrincipalTagAttributeMap",
                    &json!({"IdentityPoolId": &pool_id, "IdentityProviderName": &provider}),
                )
                .await;
            assert_eq!(
                got["PrincipalTags"],
                principal_tags(),
                "case {case:?}: the get reports the mapping that was stored, got {got}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidParameterException"),
            "case {case:?}: expected (400, InvalidParameterException), got ({status}, {body})"
        );
        assert_eq!(
            message(&body),
            not_configured(&provider),
            "case {case:?}: expected {:?}, got {body}",
            not_configured(&provider)
        );
        // Nothing was stored for the refused provider: the read reports no mapping.
        let (status, read) = emulator
            .call(
                "GetPrincipalTagAttributeMap",
                &json!({"IdentityPoolId": &pool_id, "IdentityProviderName": &provider}),
            )
            .await;
        assert_eq!(
            (status, error_code(&read).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: a refused mapping is not stored, got ({status}, {read})"
        );
    }
}

/// Write one provider's mapping, answering the status and body.
async fn write_mapping(emulator: &Emulator, pool_id: &str, provider: &str) -> (u16, Value) {
    emulator
        .call(
            "SetPrincipalTagAttributeMap",
            &json!({
                "IdentityPoolId": pool_id,
                "IdentityProviderName": provider,
                "PrincipalTags": principal_tags(),
            }),
        )
        .await
}

/// RULE, applied to the configuration a pool ends up with: the providers a pool DECLARES
/// are whatever its latest configuration states, so a mapping follows the configuration —
/// a provider an update adds becomes mappable, and one an update drops stops being.
#[tokio::test]
async fn the_declared_set_follows_the_pool_s_current_configuration() {
    let emulator = Emulator::new();
    let pool_id = pool(&emulator, "declared-providers-updated").await;
    let added = "www.amazon.com";

    // Before the update: the pool does not carry it.
    let (status, body) = write_mapping(&emulator, &pool_id, added).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidParameterException"),
        "case \"before the update\": expected the provider to be refused, got ({status}, {body})"
    );

    // The update REPLACES the pool's configuration with one carrying the added provider and
    // no longer carrying the public provider it was created with.
    emulator
        .ok(
            "UpdateIdentityPool",
            &json!({
                "IdentityPoolId": &pool_id,
                "IdentityPoolName": "declared-providers-updated",
                "AllowUnauthenticatedIdentities": true,
                "DeveloperProviderName": DEVELOPER_PROVIDER,
                "SupportedLoginProviders": {added: "added-app-id"},
            }),
        )
        .await;

    let (status, body) = write_mapping(&emulator, &pool_id, added).await;
    assert_eq!(
        status, 200,
        "case \"after the update\": a provider the configuration now declares is mappable, got \
         {status} with {body}"
    );
    let (status, body) = write_mapping(&emulator, &pool_id, "graph.facebook.com").await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidParameterException"),
        "case \"a provider the update dropped\": expected it to be refused, got ({status}, \
         {body})"
    );
}
