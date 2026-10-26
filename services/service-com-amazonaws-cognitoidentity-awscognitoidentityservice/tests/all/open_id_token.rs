// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetOpenIdToken`: the OpenID Connect token this service mints for
//! an identity it already holds — the token half of the classic (basic) authentication
//! flow.
//!
//! Every case asserts a RULE over a table of inputs rather than one recorded call:
//!
//! * the token is minted from the STORED identity the request names: the identity's own
//!   pool is the audience, the identity is the subject, the window is the documented ten
//!   minutes, and each call mints its own token;
//! * the authentication methods the token names come from the logins THIS call presented —
//!   a call presenting a login is an authenticated one and names this service and every
//!   provider it presented, a call presenting none is an unauthenticated one — which is
//!   the same answer the `...:amr` condition key publishes to a role's trust policy, so a
//!   policy clause written against the methods a caller's token carries decides the vend
//!   the same way;
//! * the logins presented are additional logins OF that identity, persisted, so a later
//!   `GetId` presenting one resolves to the same identity and a reader reports it;
//! * an identity this service does not hold (404), a login the identity's pool does not
//!   accept (403) and a login already linked to another identity (409) are each refused
//!   and change nothing.
//!
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

use crate::developer_identity_tokens::{jws_header_and_claims, listed_logins};
use crate::identity_pool_flow::{cognito_trust_policy, error_code, seeded_role_arn, Emulator};

/// The operation under test, spelled once.
const OPERATION: &str = "GetOpenIdToken";

/// The issuer of an Amazon Cognito Identity OIDC token, as this service's state module
/// names it (`crate::state::OIDC_TOKEN_ISSUER`) — asserted here against the literal so the
/// wire-visible value is pinned by the test, not by the code under test.
const ISSUER: &str = "https://cognito-identity.amazonaws.com";

/// The Cognito Identity service principal, which an authenticated exchange names among its
/// authentication methods.
const PRINCIPAL: &str = "cognito-identity.amazonaws.com";

/// The method an exchange that presented NO login is performed under.
const UNAUTHENTICATED: &str = "unauthenticated";

/// The method an exchange that presented a login is performed under.
const AUTHENTICATED: &str = "authenticated";

/// How long a token this operation mints is valid, from the operation's own documentation:
/// "The OpenID token is valid for 10 minutes."
/// (`smithy_query(query="operation",operation="GetOpenIdToken")`).
const TOKEN_SECONDS: i64 = 600;

/// A well-formed `IdentityId` that nothing ever minted.
const ABSENT_IDENTITY_ID: &str = "us-east-1:33333333-3333-3333-3333-333333333333";

/// The `Logins` provider name of the Amazon Cognito user pool the fixture pool below
/// trusts, and of one it does not: a user pool is named
/// `cognito-idp.<region>.amazonaws.com/<UserPoolId>` (`GetId.Logins` in this service's
/// model), so the two are told apart by their user-pool id exactly as a client's token is.
const TRUSTED_USER_POOL_PROVIDER: &str = "cognito-idp.us-east-1.amazonaws.com/us-east-1_trusted1";
const FOREIGN_USER_POOL_PROVIDER: &str = "cognito-idp.us-east-1.amazonaws.com/us-east-1_foreign1";

/// The issuer an OIDC provider of the fixture pool is named by.
const OIDC_PROVIDER_NAME: &str = "accounts.example.com";

/// One `Logins` entry as a row carries it: the provider name and the token it carries.
type Login<'a> = (&'a str, &'a str);

/// A row of the minting table: the case name, the logins presented to `GetOpenIdToken`,
/// and whether `GetId` had already linked them (the classic flow, where the same logins
/// are presented twice) or they reach this identity for the first time here (the implicit
/// link this operation documents).
type MintingCase<'a> = (&'a str, Vec<Login<'a>>, bool);

/// The `GetOpenIdToken` request for one case. `Logins` is sent only when the row carries
/// entries, so a row with none exercises the member's ABSENCE.
fn token_request(identity_id: &str, logins: &[Login<'_>]) -> Value {
    let mut request = Map::new();
    request.insert("IdentityId".to_string(), json!(identity_id));
    if !logins.is_empty() {
        let logins: Map<String, Value> = logins
            .iter()
            .map(|(provider, login)| ((*provider).to_string(), json!(login)))
            .collect();
        request.insert("Logins".to_string(), Value::Object(logins));
    }
    Value::Object(request)
}

/// A `Logins` map for `GetId`, which takes one whenever the row presents any login.
fn logins_map(logins: &[Login<'_>]) -> Option<Value> {
    (!logins.is_empty()).then(|| {
        Value::Object(
            logins
                .iter()
                .map(|(provider, login)| ((*provider).to_string(), json!(login)))
                .collect(),
        )
    })
}

/// A pool that trusts one Amazon Cognito user pool, the public providers the rows below
/// present and one OIDC issuer, and that allows unauthenticated identities — the shape a
/// pool has to be in for any of those logins to be accepted for it. Answers its minted id.
async fn pool_trusting_providers(emulator: &Emulator, name: &str) -> String {
    let created = emulator
        .ok(
            "CreateIdentityPool",
            &json!({
                "IdentityPoolName": name,
                "AllowUnauthenticatedIdentities": true,
                "CognitoIdentityProviders": [
                    { "ProviderName": TRUSTED_USER_POOL_PROVIDER, "ClientId": "abcd1234" },
                ],
                // The public provider names `GetId.Logins` publishes.
                "SupportedLoginProviders": {
                    "graph.facebook.com": "0123456789",
                    "accounts.google.com": "0123456789.apps.googleusercontent.com",
                    "www.amazon.com": "amzn1.application.1",
                },
                "OpenIdConnectProviderARNs": [
                    format!("arn:aws:iam::123456789012:oidc-provider/{OIDC_PROVIDER_NAME}"),
                ],
            }),
        )
        .await;
    created["IdentityPoolId"]
        .as_str()
        .expect("CreateIdentityPool returns an IdentityPoolId")
        .to_string()
}

/// The authentication methods a token's `amr` claim carries, as a set.
fn authentication_methods(case: &str, claims: &Value) -> BTreeSet<String> {
    claims["amr"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: amr is a list: {claims}"))
        .iter()
        .map(|method| method.as_str().unwrap_or_default().to_string())
        .collect()
}

/// The methods an exchange presenting `providers` is performed under: `unauthenticated`
/// alone when it presented none, else `authenticated`, this service and every provider it
/// presented.
fn expected_methods(providers: &[String]) -> BTreeSet<String> {
    if providers.is_empty() {
        return BTreeSet::from([UNAUTHENTICATED.to_string()]);
    }
    let mut methods = BTreeSet::from([AUTHENTICATED.to_string(), PRINCIPAL.to_string()]);
    methods.extend(providers.iter().cloned());
    methods
}

/// RULE: the token is minted from the stored identity the request names — that identity's
/// pool as audience, the identity as subject, the documented ten-minute window and the
/// authentication methods of the logins the call presented — and the logins it presents
/// become logins of that identity.
#[tokio::test]
async fn a_token_is_minted_from_the_stored_identity_it_names() {
    let emulator = Emulator::new();

    // The rows differ along what the rule generalises over: how many logins are presented,
    // what KIND of provider each names (a public provider, the pool's own Amazon Cognito
    // user pool, an OIDC issuer), and whether the identity already carried them.
    let cases: Vec<MintingCase<'_>> = vec![
        ("no logins at all, an unauthenticated identity", vec![], false),
        (
            "one public provider, presented for the first time here",
            vec![("graph.facebook.com", "a-facebook-access-token")],
            false,
        ),
        (
            "one public provider the identity was minted for",
            vec![("accounts.google.com", "a-google-id-token")],
            true,
        ),
        (
            "two public providers, an implicit link between them",
            vec![
                ("graph.facebook.com", "another-facebook-access-token"),
                ("www.amazon.com", "an-amazon-access-token"),
            ],
            false,
        ),
        (
            "the pool's own Amazon Cognito user pool",
            vec![(TRUSTED_USER_POOL_PROVIDER, "a-user-pool-id-token")],
            true,
        ),
        (
            "an OIDC issuer beside a public provider",
            vec![
                (OIDC_PROVIDER_NAME, "an-oidc-id-token"),
                ("graph.facebook.com", "a-third-facebook-access-token"),
            ],
            false,
        ),
    ];

    let mut minted_tokens: BTreeSet<String> = BTreeSet::new();

    for (index, (case, logins, linked_by_get_id)) in cases.into_iter().enumerate() {
        // The pool NAME is modelled `[\w\s+=,.@-]+`, so it is the row's index rather than
        // its prose case name.
        let pool_id = pool_trusting_providers(&emulator, &format!("minted-token-{index}")).await;
        let identity_id = emulator
            .identity(
                &pool_id,
                if linked_by_get_id {
                    logins_map(&logins)
                } else {
                    None
                },
            )
            .await;

        let answered = emulator.ok(OPERATION, &token_request(&identity_id, &logins)).await;

        assert_eq!(
            answered["IdentityId"],
            json!(identity_id),
            "case {case:?}: expected the token to be answered for {identity_id:?}, got {answered}"
        );
        let token = answered["Token"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a Token is answered: {answered}"))
            .to_string();
        assert!(
            minted_tokens.insert(token.clone()),
            "case {case:?}: token {token:?} was issued twice"
        );

        let (header, claims) = jws_header_and_claims(case, &token);
        assert_eq!(
            header,
            json!({"alg": "none", "typ": "JWT"}),
            "case {case:?}: expected the unsecured-JWS header, got {header}"
        );
        assert_eq!(
            claims["iss"],
            json!(ISSUER),
            "case {case:?}: expected issuer {ISSUER}, got {claims}"
        );
        assert_eq!(
            claims["aud"],
            json!(pool_id),
            "case {case:?}: expected the identity's own pool as audience, got {claims}"
        );
        assert_eq!(
            claims["sub"],
            json!(identity_id),
            "case {case:?}: expected the identity as subject, got {claims}"
        );

        let issued_at = claims["iat"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case:?}: iat is a number: {claims}"));
        let expires_at = claims["exp"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case:?}: exp is a number: {claims}"));
        assert_eq!(
            expires_at - issued_at,
            TOKEN_SECONDS,
            "case {case:?}: expected a {TOKEN_SECONDS}s window, got {claims}"
        );

        let mut providers: Vec<String> = logins
            .iter()
            .map(|(provider, _)| (*provider).to_string())
            .collect();
        providers.sort();
        let expected = expected_methods(&providers);
        let methods = authentication_methods(case, &claims);
        assert_eq!(
            methods, expected,
            "case {case:?}: expected amr {expected:?}, got {methods:?}"
        );

        // The logins presented are logins OF this identity now: a reader reports each of
        // them exactly once, and a `GetId` presenting one resolves to the same identity.
        let mut reported = listed_logins(&emulator, &pool_id, &identity_id).await;
        reported.sort();
        assert_eq!(
            reported, providers,
            "case {case:?}: expected the identity's logins to be {providers:?}, got {reported:?}"
        );
        for login in &logins {
            let resolved = emulator.identity(&pool_id, logins_map(&[*login])).await;
            assert_eq!(
                resolved, identity_id,
                "case {case:?}: expected the login for {:?} to resolve to {identity_id:?}, \
                 got {resolved:?}",
                login.0
            );
        }

        // Asked again, the same identity gets a FRESH token with the same claims about it.
        let again = emulator.ok(OPERATION, &token_request(&identity_id, &logins)).await;
        let second = again["Token"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a Token is answered: {again}"))
            .to_string();
        assert_ne!(
            second, token,
            "case {case:?}: each call mints its own token, got {second:?} twice"
        );
        assert!(
            minted_tokens.insert(second.clone()),
            "case {case:?}: a re-issued token must still be distinct"
        );
        let (_, second_claims) = jws_header_and_claims(case, &second);
        assert_eq!(
            (&second_claims["aud"], &second_claims["sub"]),
            (&claims["aud"], &claims["sub"]),
            "case {case:?}: expected the same identity and pool, got {second_claims}"
        );
        assert_eq!(
            authentication_methods(case, &second_claims),
            expected,
            "case {case:?}: expected the same amr on re-issue, got {second_claims}"
        );
    }
}

/// RULE: a call this service cannot serve — an identity it does not hold, a login the
/// identity's pool does not accept, a login already linked to another identity — is
/// refused with the modelled error at its modelled status, and the identity it named is
/// left exactly as it was.
#[tokio::test]
async fn a_call_this_service_cannot_serve_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let pool_id = pool_trusting_providers(&emulator, "refused-open-id-token").await;

    // An identity carrying one login, and a second identity holding a login of its own —
    // the login the conflict row presents for the first one.
    let identity_id = emulator
        .identity(
            &pool_id,
            logins_map(&[("graph.facebook.com", "the-identitys-own-token")]),
        )
        .await;
    let other_login: Login<'_> = ("accounts.google.com", "another-accounts-token");
    let other_identity = emulator
        .identity(&pool_id, logins_map(&[other_login]))
        .await;

    let before = listed_logins(&emulator, &pool_id, &identity_id).await;

    // One row per reason the call cannot be served: the identity, the logins it presents,
    // and the status and error code owed.
    let rows: Vec<(&str, &str, Vec<Login<'_>>, u16, &str)> = vec![
        (
            "an identity this service never minted",
            ABSENT_IDENTITY_ID,
            vec![],
            404,
            "ResourceNotFoundException",
        ),
        (
            "an absent identity presenting a login",
            ABSENT_IDENTITY_ID,
            vec![("graph.facebook.com", "a-facebook-access-token")],
            404,
            "ResourceNotFoundException",
        ),
        (
            "a login minted by an Amazon Cognito user pool this pool does not trust",
            identity_id.as_str(),
            vec![(FOREIGN_USER_POOL_PROVIDER, "a-foreign-user-pool-id-token")],
            403,
            "NotAuthorizedException",
        ),
        (
            "a user-pool login beside one the pool does accept",
            identity_id.as_str(),
            vec![
                ("www.amazon.com", "an-amazon-access-token"),
                (FOREIGN_USER_POOL_PROVIDER, "another-foreign-id-token"),
            ],
            403,
            "NotAuthorizedException",
        ),
        (
            "a login already linked to another identity of the pool",
            identity_id.as_str(),
            vec![other_login],
            409,
            "ResourceConflictException",
        ),
    ];

    for (case, identity, logins, owed_status, owed_code) in rows {
        let (status, body) = emulator.call(OPERATION, &token_request(identity, &logins)).await;
        assert_eq!(
            status, owed_status,
            "case {case:?}: expected HTTP {owed_status}, got {status} with {body}"
        );
        let code = error_code(&body);
        assert!(
            code.ends_with(owed_code),
            "case {case:?}: expected error {owed_code}, got {code:?} with {body}"
        );

        // Nothing was linked and nothing was minted: the identity's logins are what they
        // were, and the other identity still holds its own login.
        let after = listed_logins(&emulator, &pool_id, &identity_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the identity's logins to stay {before:?}, got {after:?}"
        );
        let still = emulator.identity(&pool_id, logins_map(&[other_login])).await;
        assert_eq!(
            still, other_identity,
            "case {case:?}: expected {other_login:?} to still resolve to {other_identity:?}, \
             got {still:?}"
        );
    }
}

/// RULE: the authentication methods a token names are the ones the exchange publishes to a
/// role's trust policy, so a clause written against the methods of the token a caller
/// holds decides that caller's vend the same way.
#[tokio::test]
async fn the_methods_a_token_names_are_the_ones_a_trust_policy_decides_on() {
    let emulator = Emulator::new();
    let pool_id = pool_trusting_providers(&emulator, "amr-conditioned-role").await;

    // The pool's authenticated role admits only an exchange made through ONE named
    // provider; its unauthenticated role admits its own users whatever they presented.
    let admitted_provider = "graph.facebook.com";
    let mut policy = cognito_trust_policy(&pool_id);
    policy["Statement"][0]["Condition"]["ForAnyValue:StringEquals"] = json!({
        "cognito-identity.amazonaws.com:amr": admitted_provider,
    });
    let authenticated = emulator.seed_role(&seeded_role_arn("amr", "auth"), policy);
    let unauthenticated = emulator.seed_role(
        &seeded_role_arn("amr", "unauth"),
        cognito_trust_policy(&pool_id),
    );
    emulator
        .ok(
            "SetIdentityPoolRoles",
            &json!({
                "IdentityPoolId": &pool_id,
                "Roles": {
                    "authenticated": authenticated,
                    "unauthenticated": unauthenticated,
                },
            }),
        )
        .await;

    // One row per exchange: the login presented, whether the conditioned role admits it,
    // and the method the token must name for that decision to be the policy's own.
    let rows: Vec<(&str, Option<Login<'_>>, bool)> = vec![
        (
            "the provider the condition names",
            Some((admitted_provider, "a-facebook-access-token")),
            true,
        ),
        (
            "another provider the pool accepts",
            Some(("www.amazon.com", "an-amazon-access-token")),
            false,
        ),
        (
            "the pool's own user pool",
            Some((TRUSTED_USER_POOL_PROVIDER, "a-user-pool-id-token")),
            false,
        ),
        ("no login at all", None, true),
    ];

    for (case, login, admitted) in rows {
        let logins: Vec<Login<'_>> = login.into_iter().collect();
        let identity_id = emulator.identity(&pool_id, logins_map(&logins)).await;

        let answered = emulator.ok(OPERATION, &token_request(&identity_id, &logins)).await;
        let token = answered["Token"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a Token is answered: {answered}"))
            .to_string();
        let (_, claims) = jws_header_and_claims(case, &token);
        let methods = authentication_methods(case, &claims);
        let mut providers: Vec<String> = logins
            .iter()
            .map(|(provider, _)| (*provider).to_string())
            .collect();
        providers.sort();
        assert_eq!(
            methods,
            expected_methods(&providers),
            "case {case:?}: expected the methods of this exchange, got {methods:?}"
        );

        // The vend of the SAME exchange is decided on exactly those methods: the
        // conditioned authenticated role admits only the provider it names, while an
        // exchange presenting no login is vended the unauthenticated role instead.
        let mut vend = Map::new();
        vend.insert("IdentityId".to_string(), json!(identity_id));
        if let Some(logins) = logins_map(&logins) {
            vend.insert("Logins".to_string(), logins);
        }
        let (status, body) = emulator
            .call("GetCredentialsForIdentity", &Value::Object(vend))
            .await;
        if admitted {
            assert_eq!(
                status, 200,
                "case {case:?}: the methods {methods:?} are admitted, so the vend must be \
                 served, got {status} with {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: the methods {methods:?} are not admitted, so no session \
                 stands for them, got {status} with {body}"
            );
            assert!(
                error_code(&body).ends_with("InvalidIdentityPoolConfigurationException"),
                "case {case:?}: expected the pool-configuration refusal, got {body}"
            );
        }
    }
}
