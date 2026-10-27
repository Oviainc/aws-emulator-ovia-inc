// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the identity-LINK lifecycle: what an unlink leaves behind, what
//! authorizes it, and which of the two unlink operations a login belongs to.
//!
//! Every case asserts a RULE over a table of inputs rather than one recorded call:
//!
//! * an identity an unlink leaves linked to NO login provider survives as a DISABLED one:
//!   every read of it — `DescribeIdentity` and the descriptions a `ListIdentities` page
//!   carries — projects its `Logins` as the single fixed sentinel standing in place of the
//!   providers that were removed, which is what tells it from an identity that never
//!   carried a login (an empty list), and `ListIdentities$HideDisabled` hides exactly those
//!   identities;
//! * `UnlinkIdentity` is unsigned, so the TOKEN each `Logins` entry presents is the whole
//!   authorization for the call: for a provider of the pool whose logins are OpenID Connect
//!   identity tokens, a presented value that is not such a token at all is refused with a
//!   403 `NotAuthorizedException` and the identity's logins are left as they were;
//! * `UnlinkIdentity` detaches PUBLIC providers, so a `Logins` map naming none — only the
//!   pool's own developer provider, or nothing — is a 400 `InvalidParameterException` and
//!   the developer login survives;
//! * `UnlinkDeveloperIdentity` decides the `DeveloperProviderName` against the pool's
//!   configured one FIRST, and a provider the pool does not support is a bad input
//!   parameter (400 `InvalidParameterException`), not an access decision (403
//!   `NotAuthorizedException`), which is what an identifier the pool has registered against
//!   no identity is.
//!
//! The harness is `identity_pool_flow`'s: one shared engine behind the same
//! `MultiServiceRouter` the production router builds.

use serde_json::{json, Map, Value};

use crate::developer_identity_tokens::{pool_with_developer_provider, token_request};
use crate::identity_pool_flow::{error_code, Emulator};

/// The one element the `Logins` list of a DISABLED identity carries. Spelled out as the
/// literal, so the wire-visible sentinel is pinned by the test rather than by the code
/// under test: it is a fixed value, with no part taken from the request or the record.
const DISABLED: &str = "DISABLED";

/// The message a login whose provider issues OpenID Connect identity tokens is refused with
/// when the presented value is not one. Fixed text, asserted against the literal.
const INVALID_TOKEN_MESSAGE: &str = "Invalid login token. Not a valid OpenId Connect identity token.";

/// The message a call that must be authorized by a login of the identity it names is
/// refused with when none of the logins it presents is one of that identity's — which is
/// every such call for an identity an unlink left linked to nothing. Fixed text, asserted
/// against the literal.
const UNMATCHED_LOGINS_MESSAGE: &str = "Logins don't match. Please include at least one \
                                        valid login for this identity or identity pool.";

/// The message a `Logins` map naming no public provider is refused with.
const NO_PUBLIC_PROVIDER_MESSAGE: &str = "Please provide a valid public provider";

/// The message a developer provider the pool does not support is refused with.
const UNSUPPORTED_DEVELOPER_PROVIDER_MESSAGE: &str =
    "This identity pool does not support the specified developer provider.";

/// The message a developer-identity mapping the pool holds against nobody is refused with.
const NO_ACCESS_MESSAGE: &str = "You do not have access to this identity";

/// The `Logins` provider name of the Amazon Cognito user pool the fixture pool below
/// trusts: a user pool is named `cognito-idp.<region>.amazonaws.com/<UserPoolId>`
/// (`smithy_query(query="doc",shape="GetIdInput",member="Logins")`), and its login is that
/// user pool's id token.
const USER_POOL_PROVIDER: &str = "cognito-idp.us-east-1.amazonaws.com/us-east-1_linked01";

/// The issuer of the OpenID Connect provider the fixture pool names in its
/// `OpenIdConnectProviderARNs`; its login is that issuer's id token.
const OIDC_PROVIDER: &str = "accounts.example.com";

/// A PUBLIC provider whose login is NOT an identity token: Facebook's login is an OAuth
/// access token ("Facebook: graph.facebook.com", `GetIdInput$Logins`), an opaque value the
/// provider that issued it decides.
const PUBLIC_PROVIDER: &str = "graph.facebook.com";

/// One row of the developer-unlink refusal table: the request it makes and the refusal it
/// is owed.
struct DeveloperUnlinkCase<'a> {
    case: &'a str,
    pool: &'a str,
    identity: &'a str,
    provider: &'a str,
    identifier: &'a str,
    status: u16,
    code: &'a str,
    message: &'a str,
}

/// A `Logins` map as JSON: provider name -> provider token.
fn logins_map(entries: &[(&str, String)]) -> Value {
    let mut map = Map::new();
    for (provider, token) in entries {
        map.insert((*provider).to_string(), json!(token));
    }
    Value::Object(map)
}

/// The `UnlinkIdentity` request naming `identity_id`, presenting `logins` and removing
/// `logins_to_remove`.
fn unlink_request(identity_id: &str, logins: Value, logins_to_remove: &[&str]) -> Value {
    json!({
        "IdentityId": identity_id,
        "Logins": logins,
        "LoginsToRemove": logins_to_remove,
    })
}

/// The `UnlinkDeveloperIdentity` request for one case.
fn unlink_developer_request(
    pool_id: &str,
    identity_id: &str,
    developer_provider: &str,
    identifier: &str,
) -> Value {
    json!({
        "IdentityPoolId": pool_id,
        "IdentityId": identity_id,
        "DeveloperProviderName": developer_provider,
        "DeveloperUserIdentifier": identifier,
    })
}

/// The `message` an error response carries.
fn error_message(case: &str, body: &Value) -> String {
    body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: an error response carries a message: {body}"))
        .to_string()
}

/// The `Logins` list a read reports, as a list of strings.
fn logins_of(case: &str, described: &Value) -> Vec<String> {
    described["Logins"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: a read of an identity carries Logins: {described}"))
        .iter()
        .map(|provider| {
            provider
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a Logins element is a string: {described}"))
                .to_string()
        })
        .collect()
}

/// The `Logins` list `DescribeIdentity` reports for one identity.
async fn described_logins(emulator: &Emulator, case: &str, identity_id: &str) -> Vec<String> {
    let described = emulator
        .ok("DescribeIdentity", &json!({"IdentityId": identity_id}))
        .await;
    logins_of(case, &described)
}

/// The `Logins` list the description `ListIdentities` carries for one identity of a pool,
/// and `None` when the page does not report that identity at all (which is what
/// `HideDisabled` does to a disabled one).
async fn listed_logins(
    emulator: &Emulator,
    case: &str,
    pool_id: &str,
    identity_id: &str,
    hide_disabled: bool,
) -> Option<Vec<String>> {
    let listed = emulator
        .ok(
            "ListIdentities",
            &json!({
                "IdentityPoolId": pool_id,
                "MaxResults": 60,
                "HideDisabled": hide_disabled,
            }),
        )
        .await;
    listed["Identities"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: ListIdentities answers an Identities list: {listed}"))
        .iter()
        .find(|described| described["IdentityId"] == json!(identity_id))
        .map(|described| logins_of(case, described))
}

/// `providers` sorted, which is the order an identity's `Logins` list is stored in.
fn sorted(providers: &[&str]) -> Vec<String> {
    let mut sorted: Vec<String> = providers.iter().map(|name| (*name).to_string()).collect();
    sorted.sort();
    sorted
}

/// The projection a read owes for an identity left linked to `kept`: those provider names,
/// and the single sentinel for one an unlink left linked to none.
fn projected(kept: &[&str]) -> Vec<String> {
    if kept.is_empty() {
        return vec![DISABLED.to_string()];
    }
    sorted(kept)
}

/// One base64url segment, unpadded, as a compact JWS carries it (RFC 7515 §2).
fn segment(value: &str) -> String {
    aws_smithy_types::base64::encode(value)
        .trim_end_matches('=')
        .replace('+', "-")
        .replace('/', "_")
}

/// An identity token as an OpenID Connect provider issues one: a compact JWS whose header
/// names a signing algorithm, whose payload is a claim set and whose signature segment is
/// not empty (RFC 7515 §3.1, RFC 7519 §3). The signature cannot be verified by anything in
/// this emulator — that needs the issuer's published key — so what a presented token is
/// decided by is this form.
fn identity_token(subject: &str) -> String {
    format!(
        "{}.{}.{}",
        segment(r#"{"alg":"RS256","kid":"kid-1","typ":"JWT"}"#),
        segment(
            &json!({
                "iss": "https://issuer.example.com",
                "sub": subject,
                "aud": "client-1",
                "token_use": "id",
                "exp": 4_102_444_800u64,
            })
            .to_string()
        ),
        segment("a-signature")
    )
}

/// A pool trusting the providers the rows below present: one Amazon Cognito user pool, one
/// OpenID Connect issuer and one public provider, with unauthenticated identities allowed.
async fn pool_trusting_providers(emulator: &Emulator, name: &str) -> String {
    let created = emulator
        .ok(
            "CreateIdentityPool",
            &json!({
                "IdentityPoolName": name,
                "AllowUnauthenticatedIdentities": true,
                "CognitoIdentityProviders": [
                    { "ProviderName": USER_POOL_PROVIDER, "ClientId": "client_1" },
                ],
                "OpenIdConnectProviderARNs": [
                    format!("arn:aws:iam::123456789012:oidc-provider/{OIDC_PROVIDER}"),
                ],
                "SupportedLoginProviders": { PUBLIC_PROVIDER: "0123456789" },
            }),
        )
        .await;
    created["IdentityPoolId"]
        .as_str()
        .expect("CreateIdentityPool returns an IdentityPoolId")
        .to_string()
}

/// RULE: an identity an unlink leaves linked to NO login provider survives as a DISABLED
/// one — every read projects its `Logins` as the single fixed sentinel in place of the
/// providers that were removed, permanently, while an identity that still has a provider
/// projects the providers it has and one that never carried a login projects an empty list —
/// and `HideDisabled` hides exactly the disabled identities.
#[tokio::test]
async fn an_unlink_that_takes_the_last_login_off_an_identity_disables_it() {
    let emulator = Emulator::new();

    // The rows differ along what the rule generalises over: HOW MANY providers the identity
    // is linked to, how many of them the call removes (one, several, every one), and whether
    // any provider is left at all.
    // (case, the logins the identity is minted with, the providers the call removes)
    let cases: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        (
            "one provider, the only one removed",
            vec!["graph.facebook.com"],
            vec!["graph.facebook.com"],
        ),
        (
            "two providers, both removed in one call",
            vec!["graph.facebook.com", "accounts.google.com"],
            vec!["graph.facebook.com", "accounts.google.com"],
        ),
        (
            "two providers, one removed",
            vec!["graph.facebook.com", "accounts.google.com"],
            vec!["accounts.google.com"],
        ),
        (
            "three providers, every one removed",
            vec!["api.twitter.com", "www.digits.com", "www.amazon.com"],
            vec!["api.twitter.com", "www.digits.com", "www.amazon.com"],
        ),
        (
            "three providers, two removed",
            vec!["api.twitter.com", "www.digits.com", "www.amazon.com"],
            vec!["api.twitter.com", "www.amazon.com"],
        ),
        (
            "a provider the identity is not linked to, so nothing is removed",
            vec!["graph.facebook.com"],
            vec!["oidc.example.com"],
        ),
    ];

    for (row, (case, linked, removed)) in cases.iter().enumerate() {
        // One pool per row, so the rows cannot interfere through a shared login index.
        let pool_id = emulator.pool(&format!("disabled-{row}"), true).await;
        let logins: Vec<(&str, String)> = linked
            .iter()
            .map(|provider| (*provider, format!("{provider}-token-{row}")))
            .collect();
        let identity_id = emulator.identity(&pool_id, Some(logins_map(&logins))).await;
        // An identity of the same pool minted with NO login at all: the control for the
        // sentinel, since it too is linked to nothing.
        let never_linked = emulator.identity(&pool_id, None).await;

        emulator
            .ok(
                "UnlinkIdentity",
                &unlink_request(&identity_id, logins_map(&logins), removed),
            )
            .await;

        let kept: Vec<&str> = linked
            .iter()
            .filter(|provider| !removed.contains(provider))
            .copied()
            .collect();
        let expected = projected(&kept);
        for attempt in 1..=2 {
            // Read twice: the projection is the stored state's, so it does not change by
            // being read.
            let described = described_logins(&emulator, case, &identity_id).await;
            assert_eq!(
                described, expected,
                "case {case:?}: read {attempt}: expected DescribeIdentity to project \
                 {expected:?} after removing {removed:?}, got {described:?}"
            );
            let listed = listed_logins(&emulator, case, &pool_id, &identity_id, false).await;
            assert_eq!(
                listed.as_ref(),
                Some(&expected),
                "case {case:?}: read {attempt}: expected ListIdentities to project \
                 {expected:?}, got {listed:?}"
            );
        }

        // An identity that never carried a login is linked to nothing WITHOUT being
        // disabled, which is the difference the sentinel makes.
        let control = described_logins(&emulator, case, &never_linked).await;
        assert_eq!(
            control,
            Vec::<String>::new(),
            "case {case:?}: an identity minted with no login projects an empty list, got \
             {control:?}"
        );

        // `HideDisabled` hides exactly the identities an unlink left linked to nothing.
        let hidden = listed_logins(&emulator, case, &pool_id, &identity_id, true).await;
        assert_eq!(
            hidden.is_none(),
            kept.is_empty(),
            "case {case:?}: expected HideDisabled to hide this identity == {}, got {hidden:?}",
            kept.is_empty()
        );
        let control_hidden = listed_logins(&emulator, case, &pool_id, &never_linked, true).await;
        assert_eq!(
            control_hidden,
            Some(Vec::<String>::new()),
            "case {case:?}: HideDisabled hides no identity that was never unlinked, got \
             {control_hidden:?}"
        );

        // The projection outlives a later call that removes nothing more: it is the end
        // state of the unlink, not a one-read answer.
        // Repeating the call does not move the projection: an identity the unlink left
        // linked to nothing is INACCESSIBLE, so the repeat is refused for it, while one that
        // still carries a provider is served and projects the same list either way.
        let (status, body) = emulator
            .call(
                "UnlinkIdentity",
                &unlink_request(&identity_id, logins_map(&logins), removed),
            )
            .await;
        if kept.is_empty() {
            assert_eq!(
                status, 403,
                "case {case:?}: expected the repeat on a disabled identity to be refused, got \
                 {status} with {body}"
            );
            assert!(
                error_code(&body).contains("NotAuthorizedException"),
                "case {case:?}: expected NotAuthorizedException on the repeat, got {body}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: an identity that still carries a provider is still reachable, \
                 got {status} with {body}"
            );
        }
        let settled = described_logins(&emulator, case, &identity_id).await;
        assert_eq!(
            settled, expected,
            "case {case:?}: repeating the unlink leaves the projection at {expected:?}, got \
             {settled:?}"
        );
    }
}

/// RULE: the call is unsigned, so the token each `Logins` entry presents is the whole
/// authorization for it — a value presented for a provider of the pool whose logins are
/// OpenID Connect identity tokens, and that is not such a token at all, is refused with a
/// 403 `NotAuthorizedException` and removes nothing, while a login whose provider issues no
/// identity token is decided by that provider and is honoured.
#[tokio::test]
async fn a_login_token_that_is_not_an_identity_token_authorizes_nothing() {
    let emulator = Emulator::new();
    let pool_id = pool_trusting_providers(&emulator, "login-tokens").await;

    // The rows differ along what the rule generalises over: the PROVIDER the login is
    // presented for (the pool's Amazon Cognito user pool, its OpenID Connect issuer, a
    // public provider whose login is no identity token) and the FORM of the presented value
    // — each row a different way of not being a compact JWS carrying a signed claim set
    // (RFC 7515 §3.1), plus the forms that are one.
    // (case, provider, the value presented, whether the call must be served)
    let claims = segment(r#"{"sub":"user-1"}"#);
    let header = segment(r#"{"alg":"RS256"}"#);
    let signature = segment("a-signature");
    let cases: Vec<(&str, &str, String, bool)> = vec![
        (
            "an opaque value that is no token at all",
            USER_POOL_PROVIDER,
            "not-a-valid-jwt-token".to_string(),
            false,
        ),
        (
            "two segments instead of three",
            USER_POOL_PROVIDER,
            format!("{header}.{claims}"),
            false,
        ),
        (
            "four segments instead of three",
            USER_POOL_PROVIDER,
            format!("{header}.{claims}.{signature}.{signature}"),
            false,
        ),
        (
            "a header segment that is not base64url",
            USER_POOL_PROVIDER,
            format!("@@@@.{claims}.{signature}"),
            false,
        ),
        (
            "a header segment that is not a JSON object",
            USER_POOL_PROVIDER,
            format!("{}.{claims}.{signature}", segment(r#"["RS256"]"#)),
            false,
        ),
        (
            "a header naming no signing algorithm",
            USER_POOL_PROVIDER,
            format!("{}.{claims}.{signature}", segment(r#"{"typ":"JWT"}"#)),
            false,
        ),
        (
            "a payload segment that is not JSON",
            USER_POOL_PROVIDER,
            format!("{header}.{}.{signature}", segment("not json")),
            false,
        ),
        (
            "the unsecured algorithm, so nothing signed it",
            USER_POOL_PROVIDER,
            format!("{}.{claims}.{signature}", segment(r#"{"alg":"none"}"#)),
            false,
        ),
        (
            "an empty signature segment, so nothing signed it",
            USER_POOL_PROVIDER,
            format!("{header}.{claims}."),
            false,
        ),
        (
            "an opaque value presented for the pool's OpenID Connect issuer",
            OIDC_PROVIDER,
            "an-opaque-value".to_string(),
            false,
        ),
        (
            "an identity token presented for the pool's user pool",
            USER_POOL_PROVIDER,
            identity_token("user-pool-user"),
            true,
        ),
        (
            "an identity token presented for the pool's OpenID Connect issuer",
            OIDC_PROVIDER,
            identity_token("oidc-user"),
            true,
        ),
        (
            "an opaque access token presented for a public provider that issues one",
            PUBLIC_PROVIDER,
            "an-oauth-access-token".to_string(),
            true,
        ),
    ];

    for (row, (case, provider, presented, served)) in cases.iter().enumerate() {
        // Each row gets its own identity, linked through the row's provider with a login the
        // provider itself would have issued, so what the row varies is the value the UNLINK
        // presents.
        let linked_login = if *provider == PUBLIC_PROVIDER {
            format!("an-oauth-access-token-{row}")
        } else {
            identity_token(&format!("linked-user-{row}"))
        };
        let identity_id = emulator
            .identity(&pool_id, Some(logins_map(&[(provider, linked_login.clone())])))
            .await;

        let (status, body) = emulator
            .call(
                "UnlinkIdentity",
                &unlink_request(
                    &identity_id,
                    logins_map(&[(provider, (*presented).clone())]),
                    &[provider],
                ),
            )
            .await;

        if *served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the unlink to be served, got {status} with {body}"
            );
            let after = described_logins(&emulator, case, &identity_id).await;
            assert_eq!(
                after,
                vec![DISABLED.to_string()],
                "case {case:?}: the served unlink took the only provider off the identity, got \
                 {after:?}"
            );
            continue;
        }

        assert_eq!(
            status, 403,
            "case {case:?}: expected HTTP 403 for a value that is no identity token, got \
             {status} with {body}"
        );
        assert!(
            error_code(&body).contains("NotAuthorizedException"),
            "case {case:?}: expected NotAuthorizedException, got {body}"
        );
        assert_eq!(
            error_message(case, &body),
            INVALID_TOKEN_MESSAGE,
            "case {case:?}: expected the fixed invalid-token message, got {body}"
        );

        // The refusal wrote nothing: the identity still carries its provider, and the login
        // the provider issued still resolves to it.
        let after = described_logins(&emulator, case, &identity_id).await;
        assert_eq!(
            after,
            vec![(*provider).to_string()],
            "case {case:?}: a refused unlink leaves the identity linked to {provider:?}, got \
             {after:?}"
        );
        let resolved = emulator
            .identity(&pool_id, Some(logins_map(&[(provider, linked_login)])))
            .await;
        assert_eq!(
            resolved, identity_id,
            "case {case:?}: a refused unlink leaves the login resolving to {identity_id}, got \
             {resolved}"
        );
    }
}

/// RULE: the public-provider unlink acts for PUBLIC providers, so a `Logins` map naming none
/// of them — only the pool's own developer provider, or nothing at all — is refused as a bad
/// input parameter and the developer login it names survives; a map that does name one is
/// served.
#[tokio::test]
async fn an_unlink_presenting_no_public_provider_is_refused() {
    let emulator = Emulator::new();

    // The rows differ in the pool's own `DeveloperProviderName` (modelled
    // `length 1..128, pattern ^[\w._-]+$`, one row sitting on its published maximum) and in
    // the `Logins` map the call presents: only the developer provider, nothing at all, or a
    // public provider beside the developer one.
    let at_max_provider = "d".repeat(128);
    // (case, the pool's developer provider, the Logins entries, whether the call is served)
    let cases: Vec<(&str, &str, Vec<&str>, bool)> = vec![
        (
            "only the pool's developer provider",
            "login.example.app",
            vec!["login.example.app"],
            false,
        ),
        (
            "only a developer provider at the maximum modelled length",
            at_max_provider.as_str(),
            vec![at_max_provider.as_str()],
            false,
        ),
        (
            "no entry at all",
            "backend_1.example-app",
            vec![],
            false,
        ),
        (
            "a public provider beside the developer one",
            "login.example.app",
            vec!["login.example.app", PUBLIC_PROVIDER],
            true,
        ),
    ];

    for (row, (case, developer_provider, presented, served)) in cases.iter().enumerate() {
        let pool_id = pool_with_developer_provider(
            &emulator,
            &format!("public-provider-{row}"),
            Some(developer_provider),
        )
        .await;
        let identifier = format!("developer-user-{row}");
        let minted = emulator
            .ok(
                "GetOpenIdTokenForDeveloperIdentity",
                &token_request(&pool_id, &[(developer_provider, &identifier)], None, None),
            )
            .await;
        let identity_id = minted["IdentityId"]
            .as_str()
            .expect("GetOpenIdTokenForDeveloperIdentity returns an IdentityId")
            .to_string();

        let logins: Vec<(&str, String)> = presented
            .iter()
            .map(|provider| {
                let value = if *provider == *developer_provider {
                    identifier.clone()
                } else {
                    format!("{provider}-token-{row}")
                };
                (*provider, value)
            })
            .collect();
        let (status, body) = emulator
            .call(
                "UnlinkIdentity",
                // The call asks for a PUBLIC provider to be removed, so what decides it is
                // the map it presents, not the list it removes.
                &unlink_request(&identity_id, logins_map(&logins), &[PUBLIC_PROVIDER]),
            )
            .await;

        if *served {
            assert_eq!(
                status, 200,
                "case {case:?}: a map naming a public provider is served, got {status} with \
                 {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for a map naming no public provider, got \
                 {status} with {body}"
            );
            assert!(
                error_code(&body).contains("InvalidParameterException"),
                "case {case:?}: expected InvalidParameterException, got {body}"
            );
            assert_eq!(
                error_message(case, &body),
                NO_PUBLIC_PROVIDER_MESSAGE,
                "case {case:?}: expected the fixed public-provider message, got {body}"
            );
        }

        // Either way the developer login is untouched: this operation never detaches one.
        let after = described_logins(&emulator, case, &identity_id).await;
        assert_eq!(
            after,
            vec![(*developer_provider).to_string()],
            "case {case:?}: the developer login survives, got {after:?}"
        );
        let looked_up = emulator
            .ok(
                "LookupDeveloperIdentity",
                &json!({"IdentityPoolId": &pool_id, "DeveloperUserIdentifier": &identifier}),
            )
            .await;
        assert_eq!(
            looked_up["IdentityId"],
            json!(identity_id),
            "case {case:?}: the developer user identifier still resolves to {identity_id}, got \
             {looked_up}"
        );
    }
}

/// RULE: the developer unlink decides the `DeveloperProviderName` against the pool's
/// configured one first — a provider the pool does not support is a bad input parameter the
/// caller has to correct (400 `InvalidParameterException`), which is a different answer from
/// the access decision an identifier the pool has registered against no identity gets (403
/// `NotAuthorizedException`) — and every refusal leaves the mapping registered.
#[tokio::test]
async fn an_unsupported_developer_provider_is_a_bad_parameter_not_an_access_decision() {
    let emulator = Emulator::new();
    let developer_provider = "login.example.app";
    let pool_id = pool_with_developer_provider(&emulator, "dev-provider", Some(developer_provider))
        .await;
    // A pool configured with NO developer provider supports none, so every provider a
    // request names is one it does not support.
    let no_developer_provider =
        pool_with_developer_provider(&emulator, "no-dev-provider", None).await;
    let identifier = "developer-user-1";
    let minted = emulator
        .ok(
            "GetOpenIdTokenForDeveloperIdentity",
            &token_request(&pool_id, &[(developer_provider, identifier)], None, None),
        )
        .await;
    let identity_id = minted["IdentityId"]
        .as_str()
        .expect("GetOpenIdTokenForDeveloperIdentity returns an IdentityId")
        .to_string();
    let bare_identity = emulator.identity(&no_developer_provider, None).await;

    // The rows differ along what the rule generalises over: HOW the named provider differs
    // from the pool's configured one (another domain, a different case of the same one, one
    // at the modelled maximum length, a pool that names none at all) and, for the contrast
    // rows, whether what is wrong is the IDENTIFIER instead.
    let at_max_provider = "p".repeat(128);
    // (case, pool, identity, the provider named, the identifier named, status, code, message)
    let cases: Vec<DeveloperUnlinkCase<'_>> = vec![
        DeveloperUnlinkCase {
            case: "another developer provider domain",
            pool: pool_id.as_str(),
            identity: identity_id.as_str(),
            provider: "another.example.app",
            identifier,
            status: 400,
            code: "InvalidParameterException",
            message: UNSUPPORTED_DEVELOPER_PROVIDER_MESSAGE,
        },
        DeveloperUnlinkCase {
            case: "the pool's provider in another case",
            pool: pool_id.as_str(),
            identity: identity_id.as_str(),
            provider: "Login.Example.App",
            identifier,
            status: 400,
            code: "InvalidParameterException",
            message: UNSUPPORTED_DEVELOPER_PROVIDER_MESSAGE,
        },
        DeveloperUnlinkCase {
            case: "a provider name at the modelled maximum length",
            pool: pool_id.as_str(),
            identity: identity_id.as_str(),
            provider: at_max_provider.as_str(),
            identifier,
            status: 400,
            code: "InvalidParameterException",
            message: UNSUPPORTED_DEVELOPER_PROVIDER_MESSAGE,
        },
        DeveloperUnlinkCase {
            case: "a pool that names no developer provider at all",
            pool: no_developer_provider.as_str(),
            identity: bare_identity.as_str(),
            provider: developer_provider,
            identifier,
            status: 400,
            code: "InvalidParameterException",
            message: UNSUPPORTED_DEVELOPER_PROVIDER_MESSAGE,
        },
        DeveloperUnlinkCase {
            case: "the pool's own provider with an identifier it registered against nobody",
            pool: pool_id.as_str(),
            identity: identity_id.as_str(),
            provider: developer_provider,
            identifier: "never-registered",
            status: 403,
            code: "NotAuthorizedException",
            message: NO_ACCESS_MESSAGE,
        },
    ];

    for row in &cases {
        let case = row.case;
        let (answered, body) = emulator
            .call(
                "UnlinkDeveloperIdentity",
                &unlink_developer_request(row.pool, row.identity, row.provider, row.identifier),
            )
            .await;
        assert_eq!(
            answered, row.status,
            "case {case:?}: expected HTTP {}, got {answered} with {body}",
            row.status
        );
        assert!(
            error_code(&body).contains(row.code),
            "case {case:?}: expected {}, got {body}",
            row.code
        );
        assert_eq!(
            error_message(case, &body),
            row.message,
            "case {case:?}: expected the fixed message for this refusal, got {body}"
        );

        // Every refusal above forgot nothing: the mapping the pool holds still resolves.
        let looked_up = emulator
            .ok(
                "LookupDeveloperIdentity",
                &json!({"IdentityPoolId": &pool_id, "DeveloperUserIdentifier": identifier}),
            )
            .await;
        assert_eq!(
            looked_up["IdentityId"],
            json!(identity_id),
            "case {case:?}: a refused unlink leaves {identifier:?} resolving to {identity_id}, \
             got {looked_up}"
        );
    }

    // The pool's own provider with the identifier it DID register is served, which is what
    // makes the refusals above about the provider and the identifier rather than about the
    // operation.
    emulator
        .ok(
            "UnlinkDeveloperIdentity",
            &unlink_developer_request(&pool_id, &identity_id, developer_provider, identifier),
        )
        .await;
    let (status, body) = emulator
        .call(
            "LookupDeveloperIdentity",
            &json!({"IdentityPoolId": &pool_id, "DeveloperUserIdentifier": identifier}),
        )
        .await;
    assert_eq!(
        status, 403,
        "the served unlink forgot the mapping, so a lookup of it resolves nothing, got {status} \
         with {body}"
    );
}

/// RULE: an identity an unlink left linked to no provider can no longer be reached THROUGH
/// A LOGIN — "removing the last linked login will make this identity inaccessible" — so a
/// call that presents one for it presents a login that is not its own and is refused with a
/// 403 `NotAuthorizedException` naming the login mismatch; a call that presents none asks
/// nothing of a login (the unauthenticated exchange, the pool's administrative reads) and
/// still answers for the record.
#[tokio::test]
async fn a_disabled_identity_can_no_longer_be_reached_through_a_login() {
    let emulator = Emulator::new();
    let pool_id = emulator.pool_with_roles("inaccessible", true).await;
    let token = "an-oauth-access-token".to_string();
    let identity_id = emulator
        .identity(&pool_id, Some(logins_map(&[(PUBLIC_PROVIDER, token.clone())])))
        .await;

    // While the identity still carries its provider, each of these is served: what the rows
    // below change is only that the unlink takes its last login off.
    for (case, operation, request) in [
        (
            "a basic-flow token for a linked identity",
            "GetOpenIdToken",
            json!({"IdentityId": &identity_id}),
        ),
        (
            "a credential vend for a linked identity",
            "GetCredentialsForIdentity",
            json!({"IdentityId": &identity_id}),
        ),
    ] {
        let (status, body) = emulator.call(operation, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} is served while the identity carries a login, got \
             {status} with {body}"
        );
    }
    emulator
        .ok(
            "UnlinkIdentity",
            &unlink_request(
                &identity_id,
                logins_map(&[(PUBLIC_PROVIDER, token.clone())]),
                &[PUBLIC_PROVIDER],
            ),
        )
        .await;

    // The rows differ in WHICH call presents a login for the identity and in WHICH login it
    // presents — the one the unlink removed, or one of another provider: neither is a login
    // of this identity any more.
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "a basic-flow token presenting the login the unlink removed",
            "GetOpenIdToken",
            json!({
                "IdentityId": &identity_id,
                "Logins": logins_map(&[(PUBLIC_PROVIDER, token.clone())]),
            }),
        ),
        (
            "a basic-flow token presenting a login of another provider",
            "GetOpenIdToken",
            json!({
                "IdentityId": &identity_id,
                "Logins": logins_map(&[("accounts.google.com", token.clone())]),
            }),
        ),
        (
            "a credential vend presenting the login the unlink removed",
            "GetCredentialsForIdentity",
            json!({
                "IdentityId": &identity_id,
                "Logins": logins_map(&[(PUBLIC_PROVIDER, token.clone())]),
            }),
        ),
        (
            "another unlink presenting the same login",
            "UnlinkIdentity",
            unlink_request(
                &identity_id,
                logins_map(&[(PUBLIC_PROVIDER, token.clone())]),
                &[PUBLIC_PROVIDER],
            ),
        ),
        (
            "another unlink presenting a login of a different provider",
            "UnlinkIdentity",
            unlink_request(
                &identity_id,
                logins_map(&[("accounts.google.com", token.clone())]),
                &["accounts.google.com"],
            ),
        ),
    ];

    for (case, operation, request) in &cases {
        let (status, body) = emulator.call(operation, request).await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected HTTP 403 for {operation} on a disabled identity, got \
             {status} with {body}"
        );
        assert!(
            error_code(&body).contains("NotAuthorizedException"),
            "case {case:?}: expected NotAuthorizedException, got {body}"
        );
        assert_eq!(
            error_message(case, &body),
            UNMATCHED_LOGINS_MESSAGE,
            "case {case:?}: expected the fixed login-mismatch message, got {body}"
        );
    }

    // A call that presents NO login is unaffected: it asks nothing of a login, so the vend
    // assumes the pool's unauthenticated role for the identity id and the basic flow still
    // mints a token for it, and the administrative reads still report the record — which is
    // how an operator sees a disabled identity at all.
    let vended = emulator
        .ok(
            "GetCredentialsForIdentity",
            &json!({"IdentityId": &identity_id}),
        )
        .await;
    assert_eq!(
        vended["IdentityId"],
        json!(identity_id),
        "the unauthenticated vend still answers for the disabled identity, got {vended}"
    );
    let minted = emulator
        .ok("GetOpenIdToken", &json!({"IdentityId": &identity_id}))
        .await;
    assert_eq!(
        minted["IdentityId"],
        json!(identity_id),
        "a token asked for with no login still names the disabled identity, got {minted}"
    );

    // And a login that DOES name this identity is still one of its own: a token this service
    // minted for it, presented under this service's own provider name, is a valid login for
    // that identity, so the vend presenting it is served rather than refused for a mismatch.
    let own_token = minted["Token"]
        .as_str()
        .expect("GetOpenIdToken returns a Token")
        .to_string();
    let exchanged = emulator
        .ok(
            "GetCredentialsForIdentity",
            &json!({
                "IdentityId": &identity_id,
                "Logins": logins_map(&[("cognito-identity.amazonaws.com", own_token)]),
            }),
        )
        .await;
    assert_eq!(
        exchanged["IdentityId"],
        json!(identity_id),
        "a token this service minted for the identity is a login of it, got {exchanged}"
    );
    let described = described_logins(&emulator, "the disabled identity", &identity_id).await;
    assert_eq!(
        described,
        vec![DISABLED.to_string()],
        "DescribeIdentity still reports the disabled identity, got {described:?}"
    );
    let listed = listed_logins(
        &emulator,
        "the disabled identity",
        &pool_id,
        &identity_id,
        false,
    )
    .await;
    assert_eq!(
        listed,
        Some(vec![DISABLED.to_string()]),
        "ListIdentities still reports the disabled identity, got {listed:?}"
    );
}
