// SPDX-License-Identifier: CC0-1.0

//! `GetDelegatedAccessToken` exchanges a trade-in token for credentials of the principal
//! the ACCOUNT's own record for that token names.
//!
//! One rule, asserted over a table rather than one request: the exchange is decided by
//! the record the account holds for the presented token and by nothing in the request, so
//!
//! * a token the account holds no record of is refused with `ValidationError` / HTTP 400
//!   and STS's fixed sentence for it, whatever the shape of the value presented — the
//!   operation models no error for a token it cannot resolve;
//! * a token the account holds whose validity window has run out is
//!   `ExpiredTradeInTokenException` / HTTP 400, the one failure the model declares for a
//!   token it knows;
//! * a token the account holds and that is unexpired is exchanged for a session of the
//!   recorded principal — `AssumedPrincipal` is that principal's ARN, the credentials
//!   resolve afterwards AS that principal, and the record is not consumed by the
//!   exchange.
//!
//! Every call goes over the awsQuery wire through the same `MultiServiceRouter` the
//! emulator serves with.

use std::time::{Duration, SystemTime};

use service_com_amazonaws_sts_awssecuritytokenservicev20110615 as sts;
use sts::state::{
    ComAmazonawsStsAwssecuritytokenservicev20110615State as StsState, Identity, TradeInToken,
};

use super::sts_sessions::{
    call, element, emulator, emulator_account, member, now_secs, timestamp_secs, LONG_TERM_KEY,
};

/// How AWS words the refusal of a trade-in token the account holds no record of: code
/// `ValidationError`, HTTP 400, and this sentence, which carries nothing of the request
/// (the token is `@sensitive` in this service's model).
const INVALID_TOKEN_MESSAGE: &str = "Invalid trade-in token";

/// How this service words the refusal of a trade-in token it holds whose validity window
/// has run out, in the same sentence shape as the invalid-token refusal above.
const EXPIRED_TOKEN_MESSAGE: &str = "The trade-in token included in the request is expired";

/// The principal a seeded trade-in token stands for: a role session of this account, as
/// `AssumeRole` would report one.
fn assumed_role_principal(role_name: &str, session_name: &str, role_id: &str) -> Identity {
    Identity {
        user_id: format!("{role_id}:{session_name}"),
        account: emulator_account(),
        arn: format!(
            "arn:aws:sts::{}:assumed-role/{role_name}/{session_name}",
            emulator_account()
        ),
    }
}

/// A trade-in token recorded in the STS store of the SAME shared engine the service
/// reads, the way the delegation flow that issued the token records it: which principal
/// it stands for, until when it may be exchanged, and how long the credentials it is
/// exchanged for last.
///
/// No operation of this model issues a trade-in token
/// (`smithy_query(query="digest")` binds eleven and none returns one), so the record is
/// seeded here exactly as the IAM-held role records these suites seed for `AssumeRole`.
fn seed_trade_in_token(
    engine: &engine::InMemoryEngine,
    token: &str,
    assumed_principal: Identity,
    role_arn: Option<String>,
    duration_seconds: Option<i32>,
    expires_at: SystemTime,
) {
    let now = SystemTime::now();
    engine
        .state::<StsState>()
        .lock()
        .record_trade_in_token(
            TradeInToken {
                token: token.to_string(),
                issued_at: now,
                expires_at,
                assumed_principal,
                role_arn,
                duration_seconds,
            },
            now,
        );
}

/// How many sessions this service currently holds — the registry a mint would grow, so a
/// refusal that mints nothing leaves it where it was.
fn sessions_held(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<StsState>().lock().sessions.len()
}

/// One `GetDelegatedAccessToken` call presenting `token`.
async fn exchange(
    router: &smithy_server_runtime::MultiServiceRouter,
    token: &str,
) -> (u16, String) {
    call(
        router,
        LONG_TERM_KEY,
        "GetDelegatedAccessToken",
        &[("TradeInToken", token.to_string())],
    )
    .await
}

/// A token the account holds no record of is refused, whatever the value's shape — and a
/// near-miss of a token it DOES hold is just as unresolvable, because the value presented
/// is the whole key.
#[tokio::test]
async fn a_trade_in_token_the_account_does_not_hold_is_refused_as_invalid() {
    let (router, engine) = emulator();
    // One record IS held, so no row below is refused merely because the registry is
    // empty: the held token is exchanged in the test further down.
    let held = "delegation/8a6c1f2e4b7d";
    seed_trade_in_token(
        &engine,
        held,
        assumed_role_principal("delegated", "held", "AROASEEDEDDELEGATED1"),
        None,
        None,
        SystemTime::now() + Duration::from_secs(900),
    );
    let before = sessions_held(&engine);

    // Rows differ along what the rule generalises over: the value's size, its character
    // set, its form (opaque, base64-looking, JWT-looking), and how near it is to the
    // token the account does hold (one character changed, case folded, padded).
    let cases = [
        "x",
        "not-a-trade-in-token",
        "AQoDYXdzEExampleTradeInTokenValue0123456789abcdefghijklmnopqrstuvwxyz==",
        "eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJkZWxlZ2F0aW9uIn0.c2lnbmF0dXJl",
        "délégation/8a6c1f2e4b7d",
        "delegation/8a6c1f2e4b7e",
        "DELEGATION/8A6C1F2E4B7D",
        "delegation/8a6c1f2e4b7d ",
        "delegation",
    ];

    for token in cases {
        let (status, body) = exchange(&router, token).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some("ValidationError"),
            "case {token:?}: expected the ValidationError code, got {body}"
        );
        let message = member(token, &body, "Message");
        assert_eq!(
            message, INVALID_TOKEN_MESSAGE,
            "case {token:?}: expected STS's invalid-token sentence, got {body}"
        );
        // The token is `@sensitive`: the refusal states the service's sentence and
        // nothing of the value it was handed.
        assert!(
            !message.contains(token),
            "case {token:?}: the refusal must not echo the token, got {message:?}"
        );
        assert!(
            element(&body, "AccessKeyId").is_none(),
            "case {token:?}: a refused exchange must vend no credential, got {body}"
        );
        assert_eq!(
            sessions_held(&engine),
            before,
            "case {token:?}: a refused exchange must mint no session"
        );
    }
}

/// A token the account HOLDS but whose validity window has run out is refused as expired
/// — the one failure the model declares for a token it knows — and still mints nothing,
/// however long ago it expired.
#[tokio::test]
async fn an_expired_trade_in_token_is_refused_as_expired() {
    let (router, engine) = emulator();
    let now = SystemTime::now();
    // Rows differ in how long ago the window ran out, from the instant it closed to the
    // far end of the window an expired record is kept for.
    let expired_for = [
        Duration::from_secs(1),
        Duration::from_secs(60),
        Duration::from_secs(1800),
        sts::state::EXPIRED_TRADE_IN_TOKEN_RETENTION - Duration::from_secs(60),
    ];

    for (index, ago) in expired_for.iter().enumerate() {
        let token = format!("delegation/expired-{index}");
        seed_trade_in_token(
            &engine,
            &token,
            assumed_role_principal("delegated", &format!("expired-{index}"), "AROASEEDEDEXPIRED001"),
            None,
            None,
            now - *ago,
        );
        let before = sessions_held(&engine);

        let case = format!("{token} (expired {ago:?} ago)");
        let (status, body) = exchange(&router, &token).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some("ExpiredTradeInTokenException"),
            "case {case:?}: expected the ExpiredTradeInTokenException code, got {body}"
        );
        assert_eq!(
            member(&case, &body, "Message"),
            EXPIRED_TOKEN_MESSAGE,
            "case {case:?}: expected the expired-token sentence, got {body}"
        );
        assert!(
            element(&body, "AccessKeyId").is_none(),
            "case {case:?}: an expired token must vend no credential, got {body}"
        );
        assert_eq!(
            sessions_held(&engine),
            before,
            "case {case:?}: an expired token must mint no session"
        );
    }

    // Past the window an expired record is kept for, the record is gone altogether, so
    // the same token reads as one the account does not hold at all.
    let forgotten = "delegation/forgotten";
    seed_trade_in_token(
        &engine,
        forgotten,
        assumed_role_principal("delegated", "forgotten", "AROASEEDEDFORGOTTEN1"),
        None,
        None,
        now - (sts::state::EXPIRED_TRADE_IN_TOKEN_RETENTION + Duration::from_secs(60)),
    );
    let (status, body) = exchange(&router, forgotten).await;
    assert_eq!(
        status, 400,
        "a token past the retention window: expected HTTP 400, got {status} {body}"
    );
    assert_eq!(
        element(&body, "Code").as_deref(),
        Some("ValidationError"),
        "a token past the retention window is no longer held, so it must read as invalid: {body}"
    );
    assert_eq!(
        member("past the retention window", &body, "Message"),
        INVALID_TOKEN_MESSAGE,
        "a token past the retention window must get the invalid-token sentence: {body}"
    );
}

/// A token the account holds and that is unexpired is exchanged for a session of the
/// RECORDED principal: `AssumedPrincipal` is that principal's ARN, the credential resolves
/// afterwards as that principal, its lifetime is the one the record states, and
/// `PackedPolicySize` — "not populated for GetDelegatedAccessToken" — is absent.
#[tokio::test]
async fn a_held_trade_in_token_is_exchanged_for_credentials_of_its_principal() {
    let (router, engine) = emulator();
    let account = emulator_account();
    // Rows differ along what the rule generalises over: the principal the record names
    // (a role session, a federated user, an IAM user), whether a role ARN travels with
    // it, and the session length the record states — absent (the default), the floor and
    // the twelve-hour end.
    let cases: [(&str, Identity, Option<String>, Option<i32>); 4] = [
        (
            "delegation/role-session-default",
            assumed_role_principal("delegated", "one", "AROASEEDEDDELEGATED1"),
            Some(format!("arn:aws:iam::{account}:role/delegated")),
            None,
        ),
        (
            "delegation/role-session-900",
            assumed_role_principal("Admin_Role", "user@example.com", "AROASEEDEDDELEGATED2"),
            Some(format!("arn:aws:iam::{account}:role/Admin_Role")),
            Some(900),
        ),
        (
            "delegation/federated-user",
            Identity {
                user_id: format!("{account}:delegate"),
                account: account.clone(),
                arn: format!("arn:aws:sts::{account}:federated-user/delegate"),
            },
            None,
            Some(43_200),
        ),
        (
            "delegation/iam-user",
            Identity {
                user_id: "AIDASEEDEDDELEGATEUSR".to_string(),
                account: account.clone(),
                arn: format!("arn:aws:iam::{account}:user/delegate"),
            },
            None,
            Some(3600),
        ),
    ];

    let mut minted_keys: Vec<String> = Vec::new();
    for (token, principal, role_arn, duration) in cases {
        seed_trade_in_token(
            &engine,
            token,
            principal.clone(),
            role_arn.clone(),
            duration,
            SystemTime::now() + Duration::from_secs(1800),
        );

        let case = format!("{token} for {} lasting {duration:?}", principal.arn);
        let exchanged_at = now_secs();
        let (status, body) = exchange(&router, token).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        assert_eq!(
            member(&case, &body, "AssumedPrincipal"),
            principal.arn,
            "case {case:?}: expected the recorded principal's ARN, got {body}"
        );
        assert!(
            element(&body, "PackedPolicySize").is_none(),
            "case {case:?}: PackedPolicySize is not populated for this operation, got {body}"
        );

        let access_key_id = member(&case, &body, "AccessKeyId");
        assert!(
            !member(&case, &body, "SecretAccessKey").is_empty()
                && !member(&case, &body, "SessionToken").is_empty(),
            "case {case:?}: the exchange must vend complete credentials, got {body}"
        );
        // The session lasts what the record states, counted from the exchange.
        let expected_seconds =
            i64::from(duration.unwrap_or(sts::state::DELEGATED_ACCESS_DEFAULT_DURATION_SECONDS));
        let expiration = timestamp_secs(&case, &member(&case, &body, "Expiration"));
        let lifetime = expiration - exchanged_at;
        assert!(
            (expected_seconds - lifetime).abs() <= 5,
            "case {case:?}: expected a {expected_seconds}-second session, got {lifetime}"
        );

        // The credential resolves afterwards AS the recorded principal: the session was
        // persisted, not synthesised into the response.
        let (status, who) = call(&router, &access_key_id, "GetCallerIdentity", &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetCallerIdentity with the vended key: {status} {who}"
        );
        assert_eq!(
            member(&case, &who, "Arn"),
            principal.arn,
            "case {case:?}: the vended credential must speak for the recorded principal, got {who}"
        );
        assert_eq!(
            member(&case, &who, "UserId"),
            principal.user_id,
            "case {case:?}: wrong UserId for the vended credential, got {who}"
        );
        assert_eq!(
            member(&case, &who, "Account"),
            principal.account,
            "case {case:?}: wrong Account for the vended credential, got {who}"
        );

        // Nothing in the model says one exchange consumes the token, so presenting it
        // again is served — with its OWN credential, not the one already vended.
        let (status, again) = exchange(&router, token).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a second exchange of the same token: {status} {again}"
        );
        assert_eq!(
            member(&case, &again, "AssumedPrincipal"),
            principal.arn,
            "case {case:?}: the second exchange must name the same principal, got {again}"
        );
        let second_key = member(&case, &again, "AccessKeyId");
        assert_ne!(
            second_key, access_key_id,
            "case {case:?}: each exchange must vend its own credential"
        );
        minted_keys.push(access_key_id);
        minted_keys.push(second_key);
    }

    let unique: std::collections::BTreeSet<&String> = minted_keys.iter().collect();
    assert_eq!(
        unique.len(),
        minted_keys.len(),
        "every exchange must vend a distinct access key id, got {minted_keys:?}"
    );
}
