// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the AgentCore identity plane's
//! `GetWorkloadAccessTokenForUserId`.
//!
//! The rule under test: obtaining a workload access token MINTS one — a compact JWS whose
//! claims are computed from the request (the workload identity the `workloadName`
//! addresses, the user the call acts for, the issuing authority of the region the request
//! was signed for) and from the issuance the emulator records (the instant it was issued
//! and the instant it stops being valid) — answers it in the modelled output member, and
//! records the issuance so this service resolves a token it minted back to the workload
//! and user it names until that instant and no longer. Every call mints its own token, and
//! a request outside a modelled constraint is refused without recording anything.
//!
//! Every case drives real HTTP through the same `MultiServiceRouter` the emulator serves
//! with, so the wire shape (route, status code, `restJson1` body, modelled error code) is
//! exercised, not just the handler function.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore as agentcore;

/// `GetWorkloadAccessTokenForUserId`: `POST /identities/GetWorkloadAccessTokenForUserId`
/// (source: smithy_query query="operation"
/// operation="GetWorkloadAccessTokenForUserId": `http`).
const ROUTE: &str = "/identities/GetWorkloadAccessTokenForUserId";

/// The canonical account every ARN this emulator emits carries.
const ACCOUNT: &str = "000000000000";

/// The emulator, with request authentication off so a test signs nothing, plus the shared
/// engine so a case can read the issuance the handler recorded.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(agentcore::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Send one request and return its status and parsed JSON body (`Value::Null` for an empty
/// body). The SigV4 credential scope is what routes the request to this service, and its
/// region is the region the request is signed for.
async fn get_token(
    router: &MultiServiceRouter,
    region: &str,
    body: Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://bedrock-agentcore.{region}.amazonaws.com{ROUTE}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/bedrock-agentcore/aws4_request, \
                 SignedHeaders=host, Signature=0"
            ),
        )
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

fn error_code(body: &Value) -> &str {
    body.get("__type").and_then(Value::as_str).unwrap_or("")
}

fn text<'a>(body: &'a Value, name: &str) -> &'a str {
    body.get(name).and_then(Value::as_str).unwrap_or("")
}

/// One base64url segment of a compact JWS, read as JSON (RFC 7515 §2: base64url without
/// padding), through the tree's own segment decoder.
fn segment(case: &str, which: &str, encoded: &str) -> Value {
    let raw = service_common::jwt::decode_base64url(encoded).unwrap_or_else(|| {
        panic!("case {case:?}: expected the {which} segment in base64url, got {encoded:?}")
    });
    serde_json::from_slice(&raw).unwrap_or_else(|error| {
        panic!("case {case:?}: expected the {which} segment to be JSON, got {raw:?} ({error})")
    })
}

/// The JOSE header and the claim set of a compact JWS, with its three `.`-separated
/// segments checked (RFC 7515 §3.1): an unsecured JWS carries an empty signature segment.
/// The same token must also be readable by the tree's own reader of a token a service
/// minted itself, so the issued form cannot drift from the form that reader accepts.
fn read_token(case: &str, token: &str) -> (Value, Value) {
    let parts: Vec<&str> = token.split('.').collect();
    assert_eq!(
        parts.len(),
        3,
        "case {case:?}: expected three '.'-separated segments, got {token:?}"
    );
    assert_eq!(
        parts.get(2).copied().unwrap_or("x"),
        "",
        "case {case:?}: expected an unsecured JWS with an empty signature segment, got {token:?}"
    );
    let read = service_common::jwt::read_unsecured(token);
    assert!(
        read.is_ok(),
        "case {case:?}: expected the minted token to be readable as a compact JWS, got {read:?}"
    );
    (
        segment(case, "header", parts.first().copied().unwrap_or_default()),
        segment(case, "claims", parts.get(1).copied().unwrap_or_default()),
    )
}

/// What the emulator recorded for `token`, at `now`: the workload and user it names, or
/// `None` when the registry does not resolve it then. This is the test reading the store
/// the handlers share, not an emulator behaviour.
fn issuance(
    engine: &engine::InMemoryEngine,
    token: &str,
    now: SystemTime,
) -> Option<(String, Option<String>)> {
    let state = agentcore::state::service_state(engine).expect("the service state");
    let store = state.lock();
    store
        .workload_access_tokens
        .issued(token, now)
        .map(|held| (held.workload_name.clone(), held.user_id.clone()))
}

/// One issuance record, for the cases that drive the registry's own bounds directly.
fn issuance_record(
    workload: &str,
    user: &str,
    issued_at: SystemTime,
    expires_at: SystemTime,
) -> agentcore::state::IssuedWorkloadAccessToken {
    agentcore::state::IssuedWorkloadAccessToken {
        workload_name: workload.to_string(),
        user_id: Some(user.to_string()),
        issued_at,
        expires_at,
    }
}

/// How many issuances the emulator holds — the check that a refused request recorded
/// nothing.
fn issuance_count(engine: &engine::InMemoryEngine) -> usize {
    let state = agentcore::state::service_state(engine).expect("the service state");
    let store = state.lock();
    store.workload_access_tokens.len()
}

/// A token is minted for the workload and user the request names, in the region the
/// request was signed for, and the issuance it records resolves until its own expiry.
///
/// Rows differ along the workload name (every character class
/// `WorkloadIdentityNameType`'s pattern `^[A-Za-z0-9_.-]+$` admits, and both published
/// bounds of its 3..255 length), the user id (both published bounds of `UserIdType`'s
/// 1..128 length, and a user id that is not an identifier at all), and the signed region
/// (source: smithy_query query="operation"
/// operation="GetWorkloadAccessTokenForUserId").
#[tokio::test]
async fn a_minted_token_names_the_workload_and_user_the_request_asked_for() {
    let (router, engine) = emulator();

    let cases: Vec<(&str, &str, String, String)> = vec![
        (
            "shortest-workload-shortest-user",
            "us-east-1",
            "abc".to_string(),
            "u".to_string(),
        ),
        (
            "every-character-class-the-pattern-admits",
            "us-east-1",
            "Agent_0.workload-9".to_string(),
            "user-0123456789".to_string(),
        ),
        (
            "longest-workload-longest-user",
            "eu-west-2",
            "w".repeat(255),
            "u".repeat(128),
        ),
        (
            "another-region-signs-another-issuer",
            "ap-southeast-2",
            "payments.agent".to_string(),
            "user@example.com".to_string(),
        ),
        (
            "a-user-id-that-is-an-opaque-subject",
            "eu-west-2",
            "support-agent".to_string(),
            "a3f1c0de-0000-4a1b-9c2d-5e6f70819203".to_string(),
        ),
    ];

    let mut minted: BTreeSet<String> = BTreeSet::new();
    for (case, region, workload, user) in cases {
        let before = SystemTime::now();
        let (status, body) = get_token(
            &router,
            region,
            json!({"workloadName": workload, "userId": user}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetWorkloadAccessTokenForUserId, got \
             {status} with {body:?}"
        );
        let token = text(&body, "workloadAccessToken").to_string();
        // `WorkloadIdentityTokenType` is len 1..131072.
        assert!(
            (1..=131_072).contains(&token.len()),
            "case {case:?}: expected a token inside the modelled 1..131072 length, got {} \
             characters",
            token.len()
        );
        assert!(
            minted.insert(token.clone()),
            "case {case:?}: expected a token no earlier call minted, got one that repeats"
        );

        let (header, claims) = read_token(case, &token);
        assert_eq!(
            text(&header, "alg"),
            "none",
            "case {case:?}: expected an unsecured JWS header, got {header:?}"
        );
        assert_eq!(
            text(&claims, "iss"),
            format!("https://bedrock-agentcore.{region}.amazonaws.com"),
            "case {case:?}: expected the issuer of the signed region, got {claims:?}"
        );
        assert_eq!(
            text(&claims, "aud"),
            format!(
                "arn:aws:bedrock-agentcore:{region}:{ACCOUNT}:workload-identity-directory/default/workload-identity/{workload}"
            ),
            "case {case:?}: expected the workload identity the request named, got {claims:?}"
        );
        assert_eq!(
            text(&claims, "sub"),
            user,
            "case {case:?}: expected the user the request named as the subject, got {claims:?}"
        );
        assert_eq!(
            text(&claims, "workloadName"),
            workload,
            "case {case:?}: expected the workload the request named, got {claims:?}"
        );

        // The validity window is the store's own time-to-live, measured from the instant
        // the token was issued.
        let ttl = i64::try_from(agentcore::state::WORKLOAD_ACCESS_TOKEN_TTL.as_secs())
            .expect("the time-to-live fits an i64");
        let issued_at = claims.get("iat").and_then(Value::as_i64).unwrap_or_default();
        let expires_at = claims.get("exp").and_then(Value::as_i64).unwrap_or_default();
        assert_eq!(
            expires_at - issued_at,
            ttl,
            "case {case:?}: expected a window of {ttl} seconds, got {claims:?}"
        );
        let signed_at = i64::try_from(
            before
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("the clock is past the epoch")
                .as_secs(),
        )
        .expect("the current instant fits an i64");
        assert!(
            (signed_at..=signed_at + 60).contains(&issued_at),
            "case {case:?}: expected the token to be issued at the instant of the call \
             ({signed_at}), got {issued_at}"
        );

        // The issuance was recorded, and it resolves to the workload and user the call
        // named until its own expiry and no longer.
        let at_issue = issuance(&engine, &token, before);
        assert_eq!(
            at_issue,
            Some((workload.to_string(), Some(user.to_string()))),
            "case {case:?}: expected the recorded issuance to name the workload and user, \
             got {at_issue:?}"
        );
        let again = issuance(&engine, &token, before);
        assert_eq!(
            again, at_issue,
            "case {case:?}: expected resolving a token twice to answer alike, got {again:?}"
        );
        let past_expiry = issuance(
            &engine,
            &token,
            before + agentcore::state::WORKLOAD_ACCESS_TOKEN_TTL + Duration::from_secs(1),
        );
        assert_eq!(
            past_expiry, None,
            "case {case:?}: expected the issuance to stop resolving once its window has \
             run out, got {past_expiry:?}"
        );
    }
}

/// Two calls naming the same workload and the same user are two tokens: a workload access
/// token is minted per call, never looked up, so neither call hands out the other's token
/// and both issuances are recorded.
#[tokio::test]
async fn every_call_mints_its_own_token() {
    let (router, engine) = emulator();

    let request = json!({"workloadName": "repeat.agent", "userId": "user-1"});
    let mut tokens: Vec<String> = Vec::new();
    for attempt in 1..=3 {
        let (status, body) = get_token(&router, "us-east-1", request.clone()).await;
        assert_eq!(
            status, 200,
            "case attempt-{attempt}: expected HTTP 200, got {status} with {body:?}"
        );
        tokens.push(text(&body, "workloadAccessToken").to_string());
    }

    let distinct: BTreeSet<&String> = tokens.iter().collect();
    assert_eq!(
        distinct.len(),
        tokens.len(),
        "expected one token per call, got {} distinct of {}",
        distinct.len(),
        tokens.len()
    );
    assert_eq!(
        issuance_count(&engine),
        tokens.len(),
        "expected every minted token to be recorded as its own issuance"
    );
    let now = SystemTime::now();
    for (index, token) in tokens.iter().enumerate() {
        let held = issuance(&engine, token, now);
        assert_eq!(
            held,
            Some(("repeat.agent".to_string(), Some("user-1".to_string()))),
            "case token-{index}: expected the issuance of the workload and user the call \
             named, got {held:?}"
        );
    }
}

/// Input outside a modelled constraint is refused with a 400 `ValidationException`, never
/// accepted and never answered with a 5xx: one case on each side of `UserIdType`'s 1..128
/// length and of `WorkloadIdentityNameType`'s 3..255 length, plus one outside that name's
/// `^[A-Za-z0-9_.-]+$` pattern and one request missing a required member. A refused
/// request mints nothing.
#[tokio::test]
async fn input_outside_a_modelled_constraint_is_refused_with_400() {
    let (router, engine) = emulator();

    let cases: Vec<(&str, Value)> = vec![
        (
            "empty-user-id",
            json!({"workloadName": "demo.agent", "userId": ""}),
        ),
        (
            "user-id-past-its-length",
            json!({"workloadName": "demo.agent", "userId": "u".repeat(129)}),
        ),
        (
            "workload-name-below-its-length",
            json!({"workloadName": "ab", "userId": "user-1"}),
        ),
        (
            "workload-name-past-its-length",
            json!({"workloadName": "w".repeat(256), "userId": "user-1"}),
        ),
        (
            "workload-name-outside-its-pattern",
            json!({"workloadName": "demo agent!", "userId": "user-1"}),
        ),
        ("no-user-id-at-all", json!({"workloadName": "demo.agent"})),
        ("no-workload-name-at-all", json!({"userId": "user-1"})),
    ];

    for (case, request) in cases {
        let (status, body) = get_token(&router, "us-east-1", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body:?}"
        );
        assert_eq!(
            issuance_count(&engine),
            0,
            "case {case:?}: a refused request must mint and record nothing"
        );
    }
}

/// The registry's two bounds are materialised on the WRITE path, so the store a caller
/// can grow by asking for tokens cannot grow without end: a later write drops what the
/// clock has already run out on, and at the ceiling the issuance closest to its own expiry
/// gives way while the rest stay resolvable.
#[test]
fn a_write_prunes_what_expired_and_holds_the_ceiling() {
    let ttl = agentcore::state::WORKLOAD_ACCESS_TOKEN_TTL;
    let mut registry = agentcore::state::IssuedWorkloadAccessTokens::default();
    let now = SystemTime::now();

    // Recorded at an instant it was still live, so this write keeps it.
    registry.record(
        "already-expired".to_string(),
        issuance_record("expiring.agent", "user-0", now - ttl - ttl, now - ttl),
        now - ttl - ttl,
    );
    assert_eq!(
        registry.len(),
        1,
        "case nothing due yet: expected the issuance written inside its own window to be \
         held, got {} record(s)",
        registry.len()
    );

    // The next write is owed the expiry the clock already made due.
    registry.record(
        "live".to_string(),
        issuance_record("live.agent", "user-1", now, now + ttl),
        now,
    );
    assert_eq!(
        registry.len(),
        1,
        "case expired: expected the next write to drop the issuance the clock ran out on, \
         got {} record(s)",
        registry.len()
    );
    assert_eq!(
        registry.issued("already-expired", now).map(|held| held.workload_name.clone()),
        None,
        "case expired: an issuance past its window must not resolve"
    );
    assert_eq!(
        registry.issued("live", now).map(|held| held.workload_name.clone()),
        Some("live.agent".to_string()),
        "case live: the issuance inside its window must resolve"
    );

    // At the ceiling the record expiring soonest gives way, so the registry never grows
    // past it however many tokens a caller asks for.
    let ceiling = agentcore::state::MAX_ISSUED_WORKLOAD_ACCESS_TOKENS;
    for index in 0..ceiling {
        let seconds = u64::try_from(index).unwrap_or(0);
        registry.record(
            format!("token-{index}"),
            issuance_record(
                "ceiling.agent",
                "user-2",
                now,
                now + ttl + Duration::from_secs(seconds + 1),
            ),
            now,
        );
    }
    assert_eq!(
        registry.len(),
        ceiling,
        "case at the ceiling: expected at most {ceiling} records, got {}",
        registry.len()
    );
    assert_eq!(
        registry.issued("live", now).map(|held| held.workload_name.clone()),
        None,
        "case at the ceiling: expected the issuance closest to its own expiry to give way"
    );
    assert_eq!(
        registry
            .issued(&format!("token-{}", ceiling - 1), now)
            .map(|held| held.workload_name.clone()),
        Some("ceiling.agent".to_string()),
        "case at the ceiling: expected the newest issuance to stay resolvable"
    );
}
