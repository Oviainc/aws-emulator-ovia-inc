// SPDX-License-Identifier: CC0-1.0

//! `CreateTokenWithIAM` (`POST /token?aws_iam=t`), end to end over the wire.
//!
//! Every case drives the service through the router the production emulator serves, so
//! what is asserted is the HTTP behaviour an IAM-authenticated caller sees. The harness
//! (the emulator, `registered_client`, `device_flow_token`, `assert_failure`) is the one
//! the device-flow suite beside this file already builds.

use std::collections::HashSet;
use std::time::Duration;

use serde_json::{json, Value};

use ssooidc::state::{
    ACCESS_TOKEN_EXPIRES_IN, GRANT_TYPE_AUTHORIZATION_CODE, GRANT_TYPE_DEVICE_CODE,
    GRANT_TYPE_JWT_BEARER, GRANT_TYPE_REFRESH_TOKEN, GRANT_TYPE_TOKEN_EXCHANGE,
    REFRESH_TOKEN_EXPIRES_IN, TOKEN_TYPE_BEARER, TOKEN_TYPE_URI_ACCESS_TOKEN,
    TOKEN_TYPE_URI_REFRESH_TOKEN,
};
use service_com_amazonaws_ssooidc_awsssooidcservice as ssooidc;

use crate::device_authorization_flow::{
    assert_failure, device_flow_token, seconds, start_url, text, Emulator, REGION,
};

/// The route `CreateTokenWithIAM` is bound to: the `/token` path of `CreateToken` with
/// the `aws_iam=t` query literal the model's `@http` URI carries.
const IAM_TOKEN_PATH: &str = "/token?aws_iam=t";

/// A region other than [`REGION`], for the cases that present a token issued elsewhere.
const OTHER_REGION: &str = "eu-west-1";

/// One row of the refresh table: the case, the scopes the request asks for (if any) and
/// the scopes the rotation must grant.
type RefreshRow<'a> = (&'a str, Option<Vec<&'a str>>, Vec<&'a str>);

/// Call `CreateTokenWithIAM` in `REGION`.
async fn create_token_with_iam(emulator: &Emulator, body: &Value) -> (u16, Value) {
    emulator.post(IAM_TOKEN_PATH, REGION, body).await
}

/// A client registration plus an access/refresh token pair it holds, minted through the
/// device flow: the persisted grant every case below presents.
struct Holder {
    client_id: String,
    access_token: String,
    refresh_token: String,
}

/// Register a client with `extra` merged over the minimal registration and run the
/// device flow for it, returning the registration and the tokens it now holds.
async fn holder(emulator: &Emulator, extra: Value) -> Holder {
    let (client_id, client_secret) = emulator.registered_client(extra).await;
    let token = device_flow_token(emulator, &client_id, &client_secret).await;
    Holder {
        client_id,
        access_token: text(&token, "accessToken"),
        refresh_token: text(&token, "refreshToken"),
    }
}

/// The scopes a response granted, as a set (the granted list carries no order contract).
fn granted_scopes(case: &str, body: &Value) -> HashSet<String> {
    body["scope"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a `scope` list in {body}"))
        .iter()
        .map(|scope| {
            scope
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected scope strings in {body}"))
                .to_string()
        })
        .collect()
}

/// RULE: the token-exchange grant mints a NEW persisted token for the client named by
/// `clientId` out of the access token presented as `subjectToken` — carrying the subject
/// token's scopes, or the narrower set the request asked for — echoes the token type the
/// requester asked to receive in `issuedTokenType`, and hands back a refresh token only
/// when that is what was requested. The table varies what the rule generalises over:
/// whether a `requestedTokenType` is named and which, and how many scopes are asked for.
#[tokio::test]
async fn token_exchange_mints_a_downscoped_token_for_the_requesting_client() {
    let emulator = Emulator::new();
    // The subject's registration fixes the scopes its access token carries.
    let subject = holder(
        &emulator,
        json!({"clientName": "subject-app", "scopes": ["openid", "sso:account:access"]}),
    )
    .await;
    // The requester: a registration that restricts neither grants nor scopes.
    let (requester_id, _) = emulator
        .registered_client(json!({"clientName": "requesting-app"}))
        .await;

    let rows: Vec<(&str, Value, &str, bool, Vec<&str>)> = vec![
        (
            "no requestedTokenType and no scope",
            json!({}),
            TOKEN_TYPE_URI_ACCESS_TOKEN,
            false,
            vec!["openid", "sso:account:access"],
        ),
        (
            "an access token and one of the two scopes",
            json!({
                "requestedTokenType": TOKEN_TYPE_URI_ACCESS_TOKEN,
                "scope": ["openid"],
            }),
            TOKEN_TYPE_URI_ACCESS_TOKEN,
            false,
            vec!["openid"],
        ),
        (
            "a refresh token and no scope",
            json!({"requestedTokenType": TOKEN_TYPE_URI_REFRESH_TOKEN}),
            TOKEN_TYPE_URI_REFRESH_TOKEN,
            true,
            vec!["openid", "sso:account:access"],
        ),
        (
            "a refresh token and the other scope",
            json!({
                "requestedTokenType": TOKEN_TYPE_URI_REFRESH_TOKEN,
                "scope": ["sso:account:access"],
                "subjectTokenType": TOKEN_TYPE_URI_ACCESS_TOKEN,
            }),
            TOKEN_TYPE_URI_REFRESH_TOKEN,
            true,
            vec!["sso:account:access"],
        ),
    ];

    let mut minted: HashSet<String> = HashSet::new();
    let mut refresh_tokens: Vec<String> = Vec::new();

    for (case, extra, expected_issued_type, expects_refresh_token, expected_scopes) in rows {
        let mut body = json!({
            "clientId": requester_id,
            "grantType": GRANT_TYPE_TOKEN_EXCHANGE,
            "subjectToken": subject.access_token,
        });
        for (key, value) in extra.as_object().cloned().unwrap_or_default() {
            body[key] = value;
        }

        let (status, response) = create_token_with_iam(&emulator, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {response}"
        );
        assert_eq!(
            response["issuedTokenType"].as_str(),
            Some(expected_issued_type),
            "case {case:?}: expected issuedTokenType {expected_issued_type:?}, got {response}"
        );
        assert_eq!(
            response["tokenType"].as_str(),
            Some(TOKEN_TYPE_BEARER),
            "case {case:?}: expected tokenType {TOKEN_TYPE_BEARER:?}, got {response}"
        );
        assert_eq!(
            response["expiresIn"].as_i64(),
            Some(i64::from(ACCESS_TOKEN_EXPIRES_IN)),
            "case {case:?}: expected expiresIn {ACCESS_TOKEN_EXPIRES_IN}, got {response}"
        );
        assert_eq!(
            granted_scopes(case, &response),
            expected_scopes
                .iter()
                .map(|scope| (*scope).to_string())
                .collect::<HashSet<String>>(),
            "case {case:?}: expected the scopes {expected_scopes:?} to be granted, got {response}"
        );
        assert_eq!(
            response["refreshToken"].is_string(),
            expects_refresh_token,
            "case {case:?}: expected a refreshToken present={expects_refresh_token}, got {response}"
        );

        let access_token = text(&response, "accessToken");
        assert_ne!(
            access_token, subject.access_token,
            "case {case:?}: the exchange must mint a new token, not hand back the subject token"
        );
        assert!(
            minted.insert(access_token.clone()),
            "case {case:?}: the access token {access_token} was minted twice"
        );
        if expects_refresh_token {
            refresh_tokens.push(text(&response, "refreshToken"));
        }

        // The minted token is persisted for the REQUESTING client, in the region of the
        // endpoint that issued it, and the subject token it was exchanged out of is
        // still usable — an exchange reads the subject, it does not spend it.
        let state = emulator.state();
        let store = state.lock();
        let stored = store
            .access_tokens
            .get(&access_token)
            .unwrap_or_else(|| panic!("case {case:?}: the minted token must be persisted"));
        assert_eq!(
            stored.client_id, requester_id,
            "case {case:?}: expected the token to be held by {requester_id}, got {stored:?}"
        );
        assert_eq!(
            stored.region, REGION,
            "case {case:?}: expected the token to be issued in {REGION}, got {stored:?}"
        );
        assert_eq!(
            stored.scopes.iter().cloned().collect::<HashSet<String>>(),
            expected_scopes
                .iter()
                .map(|scope| (*scope).to_string())
                .collect::<HashSet<String>>(),
            "case {case:?}: expected the stored scopes {expected_scopes:?}, got {stored:?}"
        );
        assert!(
            store.access_tokens.contains_key(&subject.access_token),
            "case {case:?}: the subjectToken must still be usable after the exchange"
        );
    }

    // Round trip through the API: a refresh token the exchange handed out is a grant this
    // same operation can rotate, and rotating it spends it.
    let rotated = refresh_tokens
        .first()
        .expect("the table requested a refresh token");
    let (status, refreshed) = create_token_with_iam(
        &emulator,
        &json!({
            "clientId": requester_id,
            "grantType": GRANT_TYPE_REFRESH_TOKEN,
            "refreshToken": rotated,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"rotate an exchanged refresh token\": expected HTTP 200, got {status} with {refreshed}"
    );
    assert!(
        minted.insert(text(&refreshed, "accessToken")),
        "case \"rotate an exchanged refresh token\": expected a token nothing else was handed, got {refreshed}"
    );
}

/// RULE: the refresh-token grant resolves the presented `refreshToken` through its own
/// persisted record, rotates it (the presented one is spent, a new pair takes its place)
/// and issues an access token whose scopes are the grant's own or the narrower set the
/// request asked for. The table varies whether a scope is asked for and how many.
#[tokio::test]
async fn the_refresh_grant_rotates_a_persisted_refresh_token() {
    let emulator = Emulator::new();
    let holder = holder(
        &emulator,
        json!({"clientName": "cli", "scopes": ["openid", "sso:account:access"]}),
    )
    .await;

    // The rows run in sequence, each rotating what the one before handed back, so the
    // scopes asked for only ever narrow — which is what downscoping allows.
    let rows: Vec<RefreshRow> = vec![
        (
            "no scope asked for",
            None,
            vec!["openid", "sso:account:access"],
        ),
        (
            "both scopes asked for explicitly",
            Some(vec!["openid", "sso:account:access"]),
            vec!["openid", "sso:account:access"],
        ),
        ("narrowed to one scope", Some(vec!["openid"]), vec!["openid"]),
        (
            "that one scope again, on the narrowed grant",
            Some(vec!["openid"]),
            vec!["openid"],
        ),
    ];

    let mut presented = holder.refresh_token.clone();
    let mut seen: HashSet<String> = HashSet::from([holder.access_token.clone()]);

    for (case, asked_for, expected_scopes) in rows {
        let mut body = json!({
            "clientId": holder.client_id,
            "grantType": GRANT_TYPE_REFRESH_TOKEN,
            "refreshToken": presented,
        });
        if let Some(scopes) = asked_for {
            body["scope"] = json!(scopes);
        }

        let (status, response) = create_token_with_iam(&emulator, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {response}"
        );
        assert_eq!(
            response["issuedTokenType"].as_str(),
            Some(TOKEN_TYPE_URI_ACCESS_TOKEN),
            "case {case:?}: a refresh issues an access token, got {response}"
        );
        assert_eq!(
            granted_scopes(case, &response),
            expected_scopes
                .iter()
                .map(|scope| (*scope).to_string())
                .collect::<HashSet<String>>(),
            "case {case:?}: expected the scopes {expected_scopes:?}, got {response}"
        );

        let access_token = text(&response, "accessToken");
        assert!(
            seen.insert(access_token.clone()),
            "case {case:?}: the access token {access_token} was handed out twice"
        );
        let rotated = text(&response, "refreshToken");
        assert!(
            seen.insert(rotated.clone()),
            "case {case:?}: the refresh token was handed out twice"
        );

        // The presented refresh token is spent by the rotation that succeeded.
        let (status, replayed) = create_token_with_iam(
            &emulator,
            &json!({
                "clientId": holder.client_id,
                "grantType": GRANT_TYPE_REFRESH_TOKEN,
                "refreshToken": presented,
            }),
        )
        .await;
        assert_failure(
            case,
            status,
            &replayed,
            400,
            "InvalidGrantException",
            "invalid_grant",
        );

        {
            let state = emulator.state();
            let store = state.lock();
            assert!(
                store.access_tokens.contains_key(&access_token),
                "case {case:?}: the refreshed access token must be persisted"
            );
            assert!(
                store.refresh_tokens.contains_key(&rotated),
                "case {case:?}: the rotated refresh token must be persisted"
            );
        }
        presented = rotated;
    }
}

/// RULE: a grant request this operation cannot resolve to something the service issued or
/// configured is refused with the modeled error for WHY it cannot: a grant type outside
/// the four this API serves authenticates no client of this endpoint and is
/// `InvalidClientException` (HTTP 401), a member the
/// grant needs that is missing or carries an unsupported value is
/// `InvalidRequestException`, and a grant that resolves to nothing — an unknown client, an
/// unknown token, a code or assertion this service never issued, or a token issued to
/// another client — is `InvalidGrantException`. The table varies the grant type, which
/// member is wrong, and whether the client and the presented value are known.
#[tokio::test]
async fn an_unresolvable_grant_is_refused_with_the_error_for_why() {
    let emulator = Emulator::new();
    let holder = holder(&emulator, json!({"clientName": "cli"})).await;
    // A registration restricted to the device-code grant: it is configured for no grant
    // this API serves.
    let (device_only_id, _) = emulator
        .registered_client(json!({
            "clientName": "device-only",
            "grantTypes": [GRANT_TYPE_DEVICE_CODE],
        }))
        .await;

    let rows: Vec<(&str, Value, u16, &str, &str)> = vec![
        // The grant type is settled before anything else, so a request naming a
        // grant this API does not serve is refused as an unauthenticated client
        // whether or not the client it names is known.
        (
            "the device-code grant, which this API does not serve",
            json!({"clientId": "unknown", "grantType": GRANT_TYPE_DEVICE_CODE, "deviceCode": "x"}),
            401,
            "InvalidClientException",
            "invalid_client",
        ),
        (
            "a grant type no OAuth profile of this service defines",
            json!({"clientId": holder.client_id, "grantType": "client_credentials"}),
            401,
            "InvalidClientException",
            "invalid_client",
        ),
        (
            "an empty grant type",
            json!({"clientId": holder.client_id, "grantType": ""}),
            401,
            "InvalidClientException",
            "invalid_client",
        ),
        (
            "a client this service has no OAuth grant configured for",
            json!({
                "clientId": "arn:aws:sso::123456789012:application/ssoins-1234/apl-5678",
                "grantType": GRANT_TYPE_REFRESH_TOKEN,
                "refreshToken": "whatever",
            }),
            400,
            "InvalidGrantException",
            "invalid_grant",
        ),
        (
            "a client configured for the device-code grant only, presenting another\n             client's refresh token",
            json!({
                "clientId": device_only_id,
                "grantType": GRANT_TYPE_REFRESH_TOKEN,
                "refreshToken": holder.refresh_token,
            }),
            400,
            "InvalidGrantException",
            "invalid_grant",
        ),
        (
            "the refresh grant with no refreshToken",
            json!({"clientId": holder.client_id, "grantType": GRANT_TYPE_REFRESH_TOKEN}),
            400,
            "InvalidRequestException",
            "invalid_request",
        ),
        (
            "a refreshToken this service never issued",
            json!({
                "clientId": holder.client_id,
                "grantType": GRANT_TYPE_REFRESH_TOKEN,
                "refreshToken": "f".repeat(64),
            }),
            400,
            "InvalidGrantException",
            "invalid_grant",
        ),
        (
            "the exchange grant with no subjectToken",
            json!({"clientId": holder.client_id, "grantType": GRANT_TYPE_TOKEN_EXCHANGE}),
            400,
            "InvalidRequestException",
            "invalid_request",
        ),
        (
            "a subjectTokenType that is not an access token",
            json!({
                "clientId": holder.client_id,
                "grantType": GRANT_TYPE_TOKEN_EXCHANGE,
                "subjectToken": holder.access_token,
                "subjectTokenType": TOKEN_TYPE_URI_REFRESH_TOKEN,
            }),
            400,
            "InvalidRequestException",
            "invalid_request",
        ),
        (
            "a requestedTokenType this service does not issue",
            json!({
                "clientId": holder.client_id,
                "grantType": GRANT_TYPE_TOKEN_EXCHANGE,
                "subjectToken": holder.access_token,
                "requestedTokenType": "urn:ietf:params:oauth:token-type:saml2",
            }),
            400,
            "InvalidRequestException",
            "invalid_request",
        ),
        (
            "a subjectToken this service never issued",
            json!({
                "clientId": holder.client_id,
                "grantType": GRANT_TYPE_TOKEN_EXCHANGE,
                "subjectToken": "a".repeat(64),
            }),
            400,
            "InvalidGrantException",
            "invalid_grant",
        ),
        (
            "a subjectToken issued to the requesting client itself",
            json!({
                "clientId": holder.client_id,
                "grantType": GRANT_TYPE_TOKEN_EXCHANGE,
                "subjectToken": holder.access_token,
            }),
            400,
            "InvalidGrantException",
            "invalid_grant",
        ),
        (
            "the authorization-code grant with no code",
            json!({"clientId": holder.client_id, "grantType": GRANT_TYPE_AUTHORIZATION_CODE}),
            400,
            "InvalidRequestException",
            "invalid_request",
        ),
        (
            "an authorization code, which no endpoint of this API mints",
            json!({
                "clientId": holder.client_id,
                "grantType": GRANT_TYPE_AUTHORIZATION_CODE,
                "code": "aGVsbG8",
                "redirectUri": "http://127.0.0.1:52000/oauth/callback",
                "codeVerifier": "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
            }),
            400,
            "InvalidGrantException",
            "invalid_grant",
        ),
        (
            "the JWT-bearer grant with no assertion",
            json!({"clientId": holder.client_id, "grantType": GRANT_TYPE_JWT_BEARER}),
            400,
            "InvalidRequestException",
            "invalid_request",
        ),
        (
            "an assertion from an issuer this service does not trust",
            json!({
                "clientId": holder.client_id,
                "grantType": GRANT_TYPE_JWT_BEARER,
                "assertion": "eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJ1c2VyIn0.c2ln",
            }),
            400,
            "InvalidGrantException",
            "invalid_grant",
        ),
    ];

    for (case, body, expected_status, expected_shape, expected_error) in rows {
        let (status, response) = create_token_with_iam(&emulator, &body).await;
        assert_failure(
            case,
            status,
            &response,
            expected_status,
            expected_shape,
            expected_error,
        );
        // A refused request mints nothing.
        let state = emulator.state();
        let store = state.lock();
        assert_eq!(
            store.access_tokens.len(),
            1,
            "case {case:?}: a refused grant must mint no token, store holds {store:?}"
        );
    }
}

/// RULE: a request may only ask for scopes the grant it spends already carries —
/// downscoping narrows, it never widens — so a scope outside the subject token's or the
/// refresh token's own scopes is refused with `InvalidScopeException`. The table varies
/// which grant is spent and whether the scope asked for is unknown to the service or
/// merely outside the grant.
#[tokio::test]
async fn a_scope_broader_than_the_grant_is_refused() {
    let emulator = Emulator::new();
    let subject = holder(
        &emulator,
        json!({"clientName": "subject-app", "scopes": ["openid"]}),
    )
    .await;
    let (requester_id, _) = emulator
        .registered_client(json!({"clientName": "requesting-app"}))
        .await;

    let rows: Vec<(&str, Value)> = vec![
        (
            "an exchange asking for a scope the subject token does not carry",
            json!({
                "clientId": requester_id,
                "grantType": GRANT_TYPE_TOKEN_EXCHANGE,
                "subjectToken": subject.access_token,
                "scope": ["sso:account:access"],
            }),
        ),
        (
            "an exchange asking for one carried and one uncarried scope",
            json!({
                "clientId": requester_id,
                "grantType": GRANT_TYPE_TOKEN_EXCHANGE,
                "subjectToken": subject.access_token,
                "scope": ["openid", "codewhisperer:completions"],
            }),
        ),
        (
            "a refresh asking for a scope the grant does not carry",
            json!({
                "clientId": subject.client_id,
                "grantType": GRANT_TYPE_REFRESH_TOKEN,
                "refreshToken": subject.refresh_token,
                "scope": ["sso:account:access"],
            }),
        ),
    ];

    for (case, body) in rows {
        let (status, response) = create_token_with_iam(&emulator, &body).await;
        assert_failure(
            case,
            status,
            &response,
            400,
            "InvalidScopeException",
            "invalid_scope",
        );
    }
}

/// RULE: a token is usable only against the endpoint of the region that issued it, so a
/// token presented to another region's endpoint is refused with
/// `InvalidRequestRegionException` naming the region and endpoint the caller may use. The
/// table varies which grant presents the foreign token.
#[tokio::test]
async fn a_token_issued_in_another_region_is_refused_with_that_regions_endpoint() {
    let emulator = Emulator::new();
    let (client_id, client_secret) = emulator
        .registered_client(json!({"clientName": "roaming-cli"}))
        .await;
    // Mint the pair against the OTHER region's endpoint: the device authorization is
    // started anywhere, but the token is issued by the endpoint that answered the poll.
    let (status, started) = emulator
        .start_device_authorization(&json!({
            "clientId": client_id,
            "clientSecret": client_secret,
            "startUrl": start_url(),
        }))
        .await;
    assert_eq!(status, 200, "StartDeviceAuthorization answered {started}");
    emulator.approve(&text(&started, "userCode"));
    let (status, foreign) = emulator
        .post(
            "/token",
            OTHER_REGION,
            &json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "grantType": GRANT_TYPE_DEVICE_CODE,
                "deviceCode": text(&started, "deviceCode"),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "the poll against {OTHER_REGION} answered {foreign}"
    );
    let (foreign_access_token, foreign_refresh_token) = (
        text(&foreign, "accessToken"),
        text(&foreign, "refreshToken"),
    );

    // A client of the local region, so the exchange row fails on the subject token's
    // region rather than on the requester.
    let (local_id, _) = emulator
        .registered_client(json!({"clientName": "local-app"}))
        .await;

    let rows: Vec<(&str, Value)> = vec![
        (
            "a refreshToken issued by another region",
            json!({
                "clientId": client_id,
                "grantType": GRANT_TYPE_REFRESH_TOKEN,
                "refreshToken": foreign_refresh_token,
            }),
        ),
        (
            "a subjectToken issued by another region",
            json!({
                "clientId": local_id,
                "grantType": GRANT_TYPE_TOKEN_EXCHANGE,
                "subjectToken": foreign_access_token,
            }),
        ),
    ];

    for (case, body) in rows {
        let (status, response) = create_token_with_iam(&emulator, &body).await;
        assert_failure(
            case,
            status,
            &response,
            400,
            "InvalidRequestRegionException",
            "invalid_request",
        );
        assert_eq!(
            response["region"].as_str(),
            Some(OTHER_REGION),
            "case {case:?}: expected the issuing region {OTHER_REGION:?}, got {response}"
        );
        assert_eq!(
            response["endpoint"].as_str(),
            Some(format!("https://oidc.{OTHER_REGION}.amazonaws.com").as_str()),
            "case {case:?}: expected the issuing region's endpoint, got {response}"
        );
    }
}

/// RULE: a grant whose own lifetime has run out is refused with `ExpiredTokenException`
/// rather than issued against, for every kind of token this operation accepts. The table
/// varies which grant is presented, each aged past its OWN modeled lifetime.
#[tokio::test]
async fn a_grant_past_its_lifetime_is_refused_as_expired() {
    let emulator = Emulator::new();
    let subject = holder(
        &emulator,
        json!({"clientName": "subject-app", "scopes": ["openid"]}),
    )
    .await;
    let (requester_id, _) = emulator
        .registered_client(json!({"clientName": "requesting-app"}))
        .await;

    // Age both records past their own lifetimes; the wall clock cannot be moved, so the
    // persisted issue instants are moved back instead.
    {
        let state = emulator.state();
        let mut store = state.lock();
        let access = store
            .access_tokens
            .get_mut(&subject.access_token)
            .expect("the device flow persisted an access token");
        access.issued_at -= Duration::from_secs(seconds(ACCESS_TOKEN_EXPIRES_IN) + 1);
        let refresh = store
            .refresh_tokens
            .get_mut(&subject.refresh_token)
            .expect("the device flow persisted a refresh token");
        refresh.issued_at -= Duration::from_secs(seconds(REFRESH_TOKEN_EXPIRES_IN) + 1);
    }

    let rows: Vec<(&str, Value)> = vec![
        (
            "an expired refreshToken",
            json!({
                "clientId": subject.client_id,
                "grantType": GRANT_TYPE_REFRESH_TOKEN,
                "refreshToken": subject.refresh_token,
            }),
        ),
        (
            "an expired subjectToken",
            json!({
                "clientId": requester_id,
                "grantType": GRANT_TYPE_TOKEN_EXCHANGE,
                "subjectToken": subject.access_token,
            }),
        ),
    ];

    for (case, body) in rows {
        let (status, response) = create_token_with_iam(&emulator, &body).await;
        assert_failure(
            case,
            status,
            &response,
            400,
            "ExpiredTokenException",
            "expired_token",
        );
    }
}
