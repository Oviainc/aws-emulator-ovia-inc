// SPDX-License-Identifier: CC0-1.0

//! Which of the two refusals a presented OpenID Connect login can earn the caller is told
//! about: what the TOKEN ITSELF says is decided before what the identity pool's stored trust
//! for the key's issuer says.
//!
//! RULE: a `Logins` entry whose value is an OpenID Connect ID token is admitted only where
//! the token's own `iss` claim names the provider the entry is KEYED under — the entry says
//! who issued the token and the token says so too — so a token of any other issuer is
//! refused `NotAuthorizedException` with the sentence that names THAT check, "Invalid login
//! token. Issuer doesn't match providerName", whatever the key's own trust would have
//! decided about the audience the token carries.
//!
//! The rows differ along what the rule generalises over: which stored member states the
//! key's trust (`CognitoIdentityProviders`, `OpenIdConnectProviderARNs`), the shape of the
//! two issuers (a user pool's `cognito-idp.<region>.amazonaws.com/<UserPoolId>` and an
//! OpenID Connect host with a path), whether the token's own issuer is one the pool ALSO
//! configures or one it names nowhere, and whether the audience the token carries is one the
//! key's trust accepts — the audience check's own input, and the refusal this rule must not
//! fall through to. The two controls at the end keep the audience refusal reachable and
//! distinct: the key's own token is served when its trust accepts the audience, and refused
//! in the audience's own words when it does not.
//!
//! The harness, the fixture tokens and the audience sentence are `iam_oidc_federation`'s —
//! IAM on the same engine, so the providers a pool's ARNs resolve against are the records
//! `iam:CreateOpenIDConnectProvider` wrote — and every refusal is asserted to leave the
//! pool's identities and the named identity's links exactly as they were.

use serde_json::{json, Value};

use crate::iam_oidc_federation::{
    admitted, described_logins, id_token, refused, user_pool_issuer, Emulator, CLIENT_A, CLIENT_B,
    CLIENT_C, INCORRECT_AUDIENCE_MESSAGE, NOT_AUTHORIZED,
};
use crate::identity_pool_flow::error_code;

/// The fixed sentence AWS words the issuer-vs-`providerName` refusal with, spelled out here
/// so the wire-visible text is pinned by the test rather than read back out of the crate.
const MISMATCHED_ISSUER_MESSAGE: &str = "Invalid login token. Issuer doesn't match providerName";

/// The status every refusal of this service travels with.
const REFUSED_STATUS: u16 = 400;

/// The largest page `ListIdentities` serves, from its own `MaxResults`
/// (`smithy_query(query="shape",shape="QueryLimit")`, `range 1..60`): the pools below hold
/// far fewer identities than that, so one page is the whole pool.
const WHOLE_POOL: u64 = 60;

/// One row: a token of `issuer`, issued for `audience`, presented under the `Logins` key
/// `key`, and the refusal that earns — `None` where the pool admits it.
struct Row {
    case: &'static str,
    key: String,
    issuer: String,
    audience: Value,
    refusal: Option<&'static str>,
}

/// How many identities the pool holds, read back through `ListIdentities`: the evidence that
/// a refused call minted none.
async fn identity_count(emulator: &Emulator, case: &str, pool_id: &str) -> usize {
    let page = emulator
        .ok(
            "ListIdentities",
            &json!({"IdentityPoolId": pool_id, "MaxResults": WHOLE_POOL}),
        )
        .await;
    page["Identities"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: ListIdentities answers Identities: {page}"))
        .len()
}

/// RULE: the issuer the presented token names has to be the provider its `Logins` key names,
/// and that reading of the token decides the refusal before the key's trusted audiences do.
#[tokio::test]
async fn a_token_issued_by_another_provider_is_refused_the_issuer_mismatch() {
    let emulator = Emulator::new();

    // Two Amazon Cognito user pools, trusted through `CognitoIdentityProviders`, and two
    // OpenID Connect hosts with paths, trusted through `OpenIdConnectProviderARNs` — whose
    // IAM registrations accept one client id each, so the audience check has something to
    // say about a token of the other.
    let user_pool_a = user_pool_issuer("us-east-1_issprec01");
    let user_pool_b = user_pool_issuer("us-east-1_issprec02");
    let tenant_x = "auth.example.net/realms/x".to_string();
    let tenant_y = "login.example.org/tenants/y".to_string();
    let arn_x = emulator.oidc_provider(&tenant_x, &[CLIENT_B]).await;
    let arn_y = emulator.oidc_provider(&tenant_y, &[CLIENT_C]).await;
    let pool_id = emulator
        .pool(
            "issuer-precedence",
            json!({
                "OpenIdConnectProviderARNs": [&arn_x, &arn_y],
                "CognitoIdentityProviders": [
                    {"ProviderName": &user_pool_a, "ClientId": CLIENT_A},
                    {"ProviderName": &user_pool_b, "ClientId": CLIENT_B},
                ],
            }),
        )
        .await;

    // An identity of this pool, reached through a login the pool admits, so a refusal can be
    // asserted to leave its links alone.
    let identity_id = admitted(
        &emulator,
        "the baseline identity's own login",
        &pool_id,
        &json!({ tenant_x.as_str(): id_token(&tenant_x, json!(CLIENT_B)) }),
    )
    .await;
    let linked_before = described_logins(&emulator, "before the refusals", &identity_id).await;

    let rows: Vec<Row> = vec![
        Row {
            case: "a user-pool token under another configured user pool's key",
            key: user_pool_b.clone(),
            issuer: user_pool_a.clone(),
            audience: json!(CLIENT_A),
            refusal: Some(MISMATCHED_ISSUER_MESSAGE),
        },
        Row {
            // The audience check would have had something to say here — the token names a
            // client id the key's provider does not accept — and the issuer check answers
            // first, because that is the one the request violates.
            case: "an OIDC token under another configured provider's key, for a client that \
                   key's provider does not accept",
            key: tenant_y.clone(),
            issuer: tenant_x.clone(),
            audience: json!(CLIENT_B),
            refusal: Some(MISMATCHED_ISSUER_MESSAGE),
        },
        Row {
            case: "the same, for a client that key's provider DOES accept",
            key: tenant_y.clone(),
            issuer: tenant_x.clone(),
            audience: json!(CLIENT_C),
            refusal: Some(MISMATCHED_ISSUER_MESSAGE),
        },
        Row {
            case: "an audience LIST, every entry of which the key's provider accepts",
            key: tenant_y.clone(),
            issuer: tenant_x.clone(),
            audience: json!([CLIENT_C]),
            refusal: Some(MISMATCHED_ISSUER_MESSAGE),
        },
        Row {
            case: "a user-pool token under a configured OIDC provider's key",
            key: tenant_x.clone(),
            issuer: user_pool_a.clone(),
            audience: json!(CLIENT_B),
            refusal: Some(MISMATCHED_ISSUER_MESSAGE),
        },
        Row {
            case: "an OIDC token under a configured user pool's key",
            key: user_pool_a.clone(),
            issuer: tenant_x.clone(),
            audience: json!(CLIENT_B),
            refusal: Some(MISMATCHED_ISSUER_MESSAGE),
        },
        Row {
            case: "a token of an issuer this pool configures nowhere, under a configured key",
            key: tenant_x.clone(),
            issuer: "idp.elsewhere.example/realm".to_string(),
            audience: json!(CLIENT_B),
            refusal: Some(MISMATCHED_ISSUER_MESSAGE),
        },
        Row {
            // The control: the key's OWN token, for a client its provider does not accept,
            // is the audience refusal — so the two refusals are told apart by what the
            // request violates and not by which check happens to run.
            case: "the key's own token, for a client its provider does not accept",
            key: tenant_x.clone(),
            issuer: tenant_x.clone(),
            audience: json!(CLIENT_A),
            refusal: Some(INCORRECT_AUDIENCE_MESSAGE),
        },
        Row {
            case: "the key's own token, for a client its provider accepts",
            key: tenant_x.clone(),
            issuer: tenant_x.clone(),
            audience: json!(CLIENT_B),
            refusal: None,
        },
        Row {
            // A `CognitoIdentityProviders` entry states the issuer it trusts and no audience
            // constraint, so the user pool's own token is admitted whichever of its app
            // clients it was issued for.
            case: "a user pool's own token, whose trust constrains no audience",
            key: user_pool_a.clone(),
            issuer: user_pool_a.clone(),
            audience: json!(CLIENT_C),
            refusal: None,
        },
    ];

    for row in &rows {
        let logins = json!({ row.key.as_str(): id_token(&row.issuer, row.audience.clone()) });
        let Some(message) = row.refusal else {
            admitted(&emulator, row.case, &pool_id, &logins).await;
            continue;
        };

        let identities_before = identity_count(&emulator, row.case, &pool_id).await;
        refused(
            &emulator,
            row.case,
            &pool_id,
            &identity_id,
            &logins,
            message,
        )
        .await;

        // The same predicate decides the other operations a client presents a login to, so
        // they carry the same sentence and mint nothing either.
        let also: Vec<(&str, Value)> = vec![
            (
                "GetOpenIdToken",
                json!({"IdentityId": &identity_id, "Logins": &logins}),
            ),
            (
                "UnlinkIdentity",
                json!({
                    "IdentityId": &identity_id,
                    "Logins": &logins,
                    "LoginsToRemove": [row.key.as_str()],
                }),
            ),
        ];
        for (operation, request) in also {
            let (status, payload) = emulator.call(operation, &request).await;
            assert_eq!(
                (status, error_code(&payload).as_str()),
                (REFUSED_STATUS, NOT_AUTHORIZED),
                "case {:?} on {operation}: expected ({REFUSED_STATUS}, {NOT_AUTHORIZED}), got \
                 ({status}, {payload})",
                row.case
            );
            assert_eq!(
                payload["message"].as_str(),
                Some(message),
                "case {:?} on {operation}: expected the sentence {message:?}, got {payload}",
                row.case
            );
            assert_eq!(
                payload.get("Token"),
                None,
                "case {:?} on {operation}: a refusal carries no Token, got {payload}",
                row.case
            );
        }

        assert_eq!(
            identity_count(&emulator, row.case, &pool_id).await,
            identities_before,
            "case {:?}: a refused call must mint no identity",
            row.case
        );
        assert_eq!(
            described_logins(&emulator, row.case, &identity_id).await,
            linked_before,
            "case {:?}: a refused call must leave the identity's logins alone, expected \
             {linked_before:?}",
            row.case
        );
    }
}
