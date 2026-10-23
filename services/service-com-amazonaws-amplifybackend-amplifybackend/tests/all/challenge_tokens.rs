// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Admin UI challenge token slice: `CreateToken`, `GetToken`
//! and `DeleteToken`.
//!
//! Each case drives the real wire path — `MultiServiceRouter::dispatch` with this one
//! service registered — so the route templates, the `restJson1` codec, the modelled
//! constraints and the handlers are all exercised exactly as a served request does.
//! The harness mirrors `backend_lifecycle.rs`'s (whose helpers are private to that
//! suite), with one addition: a variant that keeps the shared engine, so the expiry
//! cases can seed a token the clock has already retired instead of waiting out a
//! five-minute TTL.
//!
//! Every app id here is REGISTERED first (through `CreateBackend`, the only app registry
//! reachable with this service mounted alone): a challenge token addresses an Amplify
//! app, so an `AppId` that identifies no registered app is refused with HTTP 400
//! `ValidationException` before anything is minted — the rule
//! `app_registration_gate.rs` is about.
//!
//! Three recorded rules of the slice have their own tables below, because each is a rule
//! about a CLASS of request rather than about one session id:
//!
//! * the READ path reports no lifetime — `GetToken` answers `Ttl` as the empty string
//!   whatever lifetime the token it reports carries, while the MINT (`CreateToken`)
//!   reports the lifetime it granted;
//! * `DeleteToken` validates app identity ONLY, so it is idempotent over session ids: a
//!   session this app holds no live token for is `IsSuccess: true`, not a refusal;
//! * a `GetToken` whose session does not resolve — never minted, deleted, expired, or
//!   minted under another app — is HTTP 404 with the error code
//!   `ResourceNotFoundException` and this service's fixed prefixed message.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_amplifybackend_amplifybackend::state::{
    ChallengeToken, ComAmazonawsAmplifybackendAmplifybackendState, CHALLENGE_CODE_CHARS,
    CHALLENGE_TOKEN_TTL, ERROR_MESSAGE_PREFIX, LIMIT_TYPE_CHALLENGE_TOKENS,
    MAX_CHALLENGE_TOKENS_PER_APP, MAX_IDENTIFIER_CHARS, READ_PATH_TTL,
};
use service_com_amazonaws_amplifybackend_amplifybackend::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The endpoint host an AmplifyBackend request addresses; its first label is the routing
/// key the router claims the request by.
const HOST: &str = "amplifybackend.us-east-1.amazonaws.com";

/// The wire error code a challenge-token read that does not resolve answers with
/// (recorded behaviour; not a shape this service's model declares).
const RESOURCE_NOT_FOUND_EXCEPTION: &str = "ResourceNotFoundException";

/// A router with just this service mounted, and the shared engine it was mounted on.
fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These tests exercise the service, not SigV4: requests carry no signature.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// A router with just this service mounted on a fresh shared engine.
fn router() -> MultiServiceRouter {
    router_with_engine().0
}

/// Send one request and read back `(status, body)`. `body` is `Value::Null` when the
/// response carries no JSON document.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let bytes = match &body {
        Some(document) => {
            Bytes::from(serde_json::to_vec(document).expect("serialize the request body"))
        }
        None => Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{HOST}{path}"))
        .header("host", HOST)
        .header("content-type", "application/json")
        .body(bytes)
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let document = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    (status, document)
}

/// A string member of a response, for an assertion that names the case.
fn field<'a>(body: &'a Value, name: &str) -> &'a str {
    body.get(name).and_then(Value::as_str).unwrap_or_default()
}

/// `CreateToken`: `POST /backend/{AppId}/challenge`.
async fn create_token(router: &MultiServiceRouter, app_id: &str) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/backend/{app_id}/challenge"),
        Some(json!({})),
    )
    .await
}

/// `GetToken`: `GET /backend/{AppId}/challenge/{SessionId}`.
async fn get_token(router: &MultiServiceRouter, app_id: &str, session_id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/backend/{app_id}/challenge/{session_id}"),
        None,
    )
    .await
}

/// `DeleteToken`: `POST /backend/{AppId}/challenge/{SessionId}/remove`.
async fn delete_token(router: &MultiServiceRouter, app_id: &str, session_id: &str) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/backend/{app_id}/challenge/{session_id}/remove"),
        Some(json!({})),
    )
    .await
}

/// Register `app_id` as an Amplify app this emulator knows, by creating a backend for
/// it: every token operation gates on the app's IDENTITY first, and `CreateBackend` is
/// the one registry reachable with this service mounted alone.
async fn register_app(router: &MultiServiceRouter, app_id: &str) {
    let (status, body) = call(
        router,
        "POST",
        "/backend",
        Some(json!({
            "appId": app_id,
            "appName": "tokens",
            "backendEnvironmentName": "staging"
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "seeding CreateBackend for {app_id:?} expected 200, got {status} {body}"
    );
}

/// The `Ttl` the MINT reports: [`CHALLENGE_TOKEN_TTL`] in whole seconds, as a string.
/// The read path reports [`READ_PATH_TTL`] instead.
fn expected_ttl() -> String {
    CHALLENGE_TOKEN_TTL.as_secs().to_string()
}

/// Seed one challenge token straight into the shared store, with a lifetime and an age of
/// its own, and hand back the record as stored.
///
/// The only way to place a token at a chosen point in its own lifetime without waiting
/// the lifetime out.
fn seed_token(
    engine: &engine::InMemoryEngine,
    app_id: &str,
    session_id: &str,
    lifetime: Duration,
    age: Duration,
) -> ChallengeToken {
    let created_at = SystemTime::now() - age;
    let token = ChallengeToken {
        app_id: app_id.to_string(),
        session_id: session_id.to_string(),
        challenge_code: format!("seeded-{session_id}"),
        created_at,
        expires_at: created_at + lifetime,
    };
    engine
        .state::<ComAmazonawsAmplifybackendAmplifybackendState>()
        .lock()
        .store_challenge_token(token)
        .clone()
}

/// Assert one refusal of the challenge-token read path: HTTP 404, the recorded error
/// code, and this service's fixed prefixed message, which names NOTHING from the request.
fn assert_token_not_found(case: &str, app_id: &str, session_id: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 404,
        "case {case:?}: GetToken on a session that does not resolve expected 404, got {status} {body}"
    );
    assert_eq!(
        field(body, "code"),
        RESOURCE_NOT_FOUND_EXCEPTION,
        "case {case:?}: expected code {RESOURCE_NOT_FOUND_EXCEPTION:?}, got {body}"
    );
    let message = field(body, "message");
    assert!(
        message.starts_with(ERROR_MESSAGE_PREFIX) && message.len() > ERROR_MESSAGE_PREFIX.len(),
        "case {case:?}: the refusal carries this service's fixed prefix and a sentence, got {body}"
    );
    assert!(
        !message.contains(session_id) && !message.contains(app_id),
        "case {case:?}: the refusal is a fixed sentence and leaks no identifier, got {body}"
    );
}

/// One round-trip case: the app a token is minted for, whether that app has a backend
/// ENVIRONMENT beyond the one that registered it, and how many tokens are minted for it
/// at once.
struct Case {
    name: &'static str,
    app_id: String,
    /// A challenge token addresses an APP, not an environment, so the rows differ in
    /// whether a SECOND `CreateBackend` created another environment for the app.
    extra_environment: bool,
    tokens: usize,
}

/// The round-trip cases. They differ along what the slice generalises over: how many
/// environments the app holds, how many live tokens one app holds, and the SIZE of the
/// app id — including one at `MAX_IDENTIFIER_CHARS`, the longest identifier this emulator
/// keys a record by.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "app with a second backend environment, one token",
            app_id: "d1ab2cd3ef4gh5".to_string(),
            extra_environment: true,
            tokens: 1,
        },
        Case {
            name: "app with just the one environment, one token",
            app_id: "dnobackend0001".to_string(),
            extra_environment: false,
            tokens: 1,
        },
        Case {
            name: "several live tokens at once",
            app_id: "dmanytokens099".to_string(),
            extra_environment: false,
            tokens: 5,
        },
        Case {
            name: "app id at the identifier length bound",
            app_id: "d".repeat(MAX_IDENTIFIER_CHARS),
            extra_environment: false,
            tokens: 2,
        },
    ]
}

/// A minted token, as the `CreateToken` response reported it.
struct Minted {
    session_id: String,
    challenge_code: String,
    body: Value,
}

/// `CreateToken` mints a token, stores it, and `GetToken` reports the SAME token back
/// (every member but the lifetime, which the read path does not report); `DeleteToken`
/// reclaims it, after which `GetToken` answers `ResourceNotFoundException` and
/// `DeleteToken` still answers `IsSuccess: true`.
#[tokio::test]
async fn a_challenge_token_round_trips_and_is_reclaimed_by_delete() {
    for case in cases() {
        let router = router();
        let Case {
            name,
            app_id,
            extra_environment,
            tokens,
        } = case;
        register_app(&router, &app_id).await;
        if extra_environment {
            let (status, body) = call(
                &router,
                "POST",
                "/backend",
                Some(json!({
                    "appId": app_id,
                    "appName": "tokens",
                    "backendEnvironmentName": "production"
                })),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {name:?}: seeding a second environment expected 200, got {status} {body}"
            );
        }

        let mut minted = Vec::new();
        for index in 0..tokens {
            let (status, body) = create_token(&router, &app_id).await;
            assert_eq!(
                status, 200,
                "case {name:?} token {index}: CreateToken expected 200, got {status} {body}"
            );
            assert_eq!(
                field(&body, "appId"),
                app_id,
                "case {name:?} token {index}: the response echoes the app id, got {body}"
            );
            assert_eq!(
                field(&body, "challengeCode").chars().count(),
                CHALLENGE_CODE_CHARS,
                "case {name:?} token {index}: a challenge code is {CHALLENGE_CODE_CHARS} characters, got {body}"
            );
            assert_eq!(
                field(&body, "ttl"),
                expected_ttl(),
                "case {name:?} token {index}: expected ttl {:?}, got {body}",
                expected_ttl()
            );
            assert!(
                !field(&body, "sessionId").is_empty(),
                "case {name:?} token {index}: CreateToken reports a session id, got {body}"
            );
            minted.push(Minted {
                session_id: field(&body, "sessionId").to_string(),
                challenge_code: field(&body, "challengeCode").to_string(),
                body: body.clone(),
            });
        }

        // Every token of one app is its own: no two share a session id or a code.
        let session_ids: BTreeSet<&str> = minted.iter().map(|m| m.session_id.as_str()).collect();
        let codes: BTreeSet<&str> = minted.iter().map(|m| m.challenge_code.as_str()).collect();
        assert_eq!(
            (session_ids.len(), codes.len()),
            (tokens, tokens),
            "case {name:?}: {tokens} tokens have {tokens} distinct session ids and codes, got {:?}",
            (session_ids.len(), codes.len())
        );

        // Read each token back, TWICE: `GetToken` reports what was stored and reading it
        // changes nothing a later caller sees. The MINT's own document is the expected
        // one with its lifetime blanked: the read path reports no lifetime.
        for token in &minted {
            let mut expected = token.body.clone();
            expected["ttl"] = json!(READ_PATH_TTL);
            for read in 1..=2 {
                let (status, body) = get_token(&router, &app_id, &token.session_id).await;
                assert_eq!(
                    status, 200,
                    "case {name:?} read {read}: GetToken expected 200, got {status} {body}"
                );
                assert_eq!(
                    body, expected,
                    "case {name:?} read {read}: GetToken reports the minted token with an empty ttl, expected {expected}, got {body}"
                );
            }
        }

        // Delete the first token: it is gone, and only it.
        let reclaimed = &minted[0];
        let (status, body) = delete_token(&router, &app_id, &reclaimed.session_id).await;
        assert_eq!(
            status, 200,
            "case {name:?}: DeleteToken expected 200, got {status} {body}"
        );
        assert_eq!(
            body.get("isSuccess").and_then(Value::as_bool),
            Some(true),
            "case {name:?}: DeleteToken reports isSuccess true, got {body}"
        );
        let (status, body) = get_token(&router, &app_id, &reclaimed.session_id).await;
        assert_token_not_found(
            &format!("{name}: GetToken after the delete"),
            &app_id,
            &reclaimed.session_id,
            status,
            &body,
        );
        // Deleting it again is the SAME answer: the postcondition already holds.
        let (status, body) = delete_token(&router, &app_id, &reclaimed.session_id).await;
        assert_eq!(
            (status, body.get("isSuccess").and_then(Value::as_bool)),
            (200, Some(true)),
            "case {name:?}: a repeated DeleteToken is the same success, got {status} {body}"
        );
        for token in minted.iter().skip(1) {
            let (status, body) = get_token(&router, &app_id, &token.session_id).await;
            assert_eq!(
                status, 200,
                "case {name:?}: the app's other tokens survive a delete, got {status} {body}"
            );
        }
    }
}

/// Session ids are unique across every token the service mints — within one app and
/// across apps — and each one resolves to its own challenge code.
#[tokio::test]
async fn every_minted_session_id_is_unique() {
    let router = router();
    // Two apps, so uniqueness is not merely per-app, and enough tokens that a collision
    // would be likely if ids were derived from anything but the mint itself.
    let apps = ["dsharedone001", "dsharedtwo002"];
    let per_app = 12;
    let mut all: Vec<(String, String, String)> = Vec::new();
    for app_id in apps {
        register_app(&router, app_id).await;
        for index in 0..per_app {
            let (status, body) = create_token(&router, app_id).await;
            assert_eq!(
                status, 200,
                "case {app_id:?} token {index}: CreateToken expected 200, got {status} {body}"
            );
            all.push((
                app_id.to_string(),
                field(&body, "sessionId").to_string(),
                field(&body, "challengeCode").to_string(),
            ));
        }
    }
    let session_ids: BTreeSet<&str> = all.iter().map(|(_, session, _)| session.as_str()).collect();
    let codes: BTreeSet<&str> = all.iter().map(|(_, _, code)| code.as_str()).collect();
    assert_eq!(
        (session_ids.len(), codes.len()),
        (all.len(), all.len()),
        "case {apps:?}: {} tokens have that many distinct session ids and codes, got {:?}",
        all.len(),
        (session_ids.len(), codes.len())
    );
    for (app_id, session_id, challenge_code) in &all {
        let (status, body) = get_token(&router, app_id, session_id).await;
        assert_eq!(
            status, 200,
            "case ({app_id:?}, {session_id:?}): GetToken expected 200, got {status} {body}"
        );
        assert_eq!(
            field(&body, "challengeCode"),
            challenge_code,
            "case ({app_id:?}, {session_id:?}): each session resolves to its own code, got {body}"
        );
    }
}

/// A token is addressed by (app id, session id): a session minted under one app does not
/// resolve under another, and the mismatched call — read or delete — leaves it alone.
#[tokio::test]
async fn a_token_lookup_is_scoped_to_its_app() {
    let router = router();
    let (owner, other) = ("downerapp0001", "dotherapp0002");
    // BOTH apps are registered: the lookup under `other` must miss on the TOKEN, which
    // is what this case is about, not on the app's identity.
    register_app(&router, owner).await;
    register_app(&router, other).await;
    let (status, body) = create_token(&router, owner).await;
    assert_eq!(status, 200, "seeding CreateToken expected 200, got {body}");
    let session_id = field(&body, "sessionId").to_string();

    // The read misses, as a read of a session this app holds no token for.
    let (status, body) = get_token(&router, other, &session_id).await;
    assert_token_not_found(
        "GetToken under the wrong app",
        other,
        &session_id,
        status,
        &body,
    );
    // The delete succeeds — it validates app identity only — and must not reach into the
    // owning app's tokens.
    let (status, body) = delete_token(&router, other, &session_id).await;
    assert_eq!(
        (status, body.get("isSuccess").and_then(Value::as_bool)),
        (200, Some(true)),
        "case DeleteToken under the wrong app: expected 200 isSuccess true, got {status} {body}"
    );

    let (status, body) = get_token(&router, owner, &session_id).await;
    assert_eq!(
        status, 200,
        "the owner's token survives a lookup and a delete under another app, got {status} {body}"
    );
}

/// A token stops resolving once the clock has passed its `Ttl`: the expiry is owed at the
/// token's own expiry instant, so the next call materialises it.
///
/// The rows are seeded directly into the shared engine, which is the only way to place a
/// token on either side of an instant `CHALLENGE_TOKEN_TTL` away without waiting it out.
#[tokio::test]
async fn a_token_stops_resolving_once_its_ttl_has_passed() {
    let (router, engine) = router_with_engine();
    let app_id = "dexpiryapp0001";
    register_app(&router, app_id).await;
    let now = SystemTime::now();
    // (case, when the token expires, whether it still resolves)
    let rows = [
        (
            "expired an hour ago",
            now - Duration::from_secs(3_600),
            false,
        ),
        ("expired at this instant", now, false),
        (
            "expires one TTL from now",
            now + CHALLENGE_TOKEN_TTL,
            true,
        ),
    ];
    for (index, (case, expires_at, _)) in rows.iter().enumerate() {
        let token = ChallengeToken {
            app_id: app_id.to_string(),
            session_id: format!("00000000-0000-0000-0000-{index:012}"),
            challenge_code: format!("seeded-code-{index}"),
            created_at: *expires_at - CHALLENGE_TOKEN_TTL,
            expires_at: *expires_at,
        };
        assert!(
            engine
                .state::<ComAmazonawsAmplifybackendAmplifybackendState>()
                .lock()
                .store_challenge_token(token)
                .session_id
                .ends_with(&format!("{index:012}")),
            "case {case:?}: the seeded token is in the store"
        );
    }

    for (index, (case, _, resolves)) in rows.iter().enumerate() {
        let session_id = format!("00000000-0000-0000-0000-{index:012}");
        let (status, body) = get_token(&router, app_id, &session_id).await;
        let expected = if *resolves { 200 } else { 404 };
        assert_eq!(
            status, expected,
            "case {case:?}: GetToken expected {expected}, got {status} {body}"
        );
        // Whether the clock retired the token or not, the delete is the same success:
        // only app identity is validated on that path.
        let (status, body) = delete_token(&router, app_id, &session_id).await;
        assert_eq!(
            (status, body.get("isSuccess").and_then(Value::as_bool)),
            (200, Some(true)),
            "case {case:?}: DeleteToken expected 200 isSuccess true, got {status} {body}"
        );
    }

    // The expired rows were forgotten rather than merely hidden.
    let held = engine
        .state::<ComAmazonawsAmplifybackendAmplifybackendState>()
        .lock()
        .challenge_tokens
        .get(app_id)
        .map(|sessions| sessions.len())
        .unwrap_or_default();
    assert_eq!(
        held, 0,
        "every seeded token is expired or deleted by now, got {held} still held"
    );
}

/// The caller-driven token store is bounded: an over-long app id is refused with
/// `BadRequestException` (400) naming the member, and past
/// `MAX_CHALLENGE_TOKENS_PER_APP` live tokens one more is refused with
/// `TooManyRequestsException` (429) naming the limit. The bound is a ceiling on LIVE
/// tokens, so reclaiming one makes room again.
#[tokio::test]
async fn the_token_store_is_bounded() {
    let router = router();
    // An over-long app id is refused by the LENGTH bound before the app-identity gate
    // runs: it is malformed input, and the refusal names the member so the caller can
    // fix it.
    let over_long = "d".repeat(MAX_IDENTIFIER_CHARS + 1);
    let (status, body) = create_token(&router, &over_long).await;
    assert_eq!(
        status, 400,
        "case an app id past the length bound: expected 400, got {status} {body}"
    );
    assert!(
        field(&body, "message").contains("appId"),
        "case an app id past the length bound: the refusal names the member, got {body}"
    );

    let app_id = "dboundedapp001";
    register_app(&router, app_id).await;
    let mut sessions = Vec::new();
    for index in 0..MAX_CHALLENGE_TOKENS_PER_APP {
        let (status, body) = create_token(&router, app_id).await;
        assert_eq!(
            status, 200,
            "case token {index} of the bound: expected 200, got {status} {body}"
        );
        sessions.push(field(&body, "sessionId").to_string());
    }
    let (status, body) = create_token(&router, app_id).await;
    assert_eq!(
        status, 429,
        "case one token past the bound: expected 429, got {status} {body}"
    );
    assert_eq!(
        field(&body, "limitType"),
        LIMIT_TYPE_CHALLENGE_TOKENS,
        "case one token past the bound: the refusal names the limit, got {body}"
    );

    let (status, body) = delete_token(&router, app_id, &sessions[0]).await;
    assert_eq!(
        status, 200,
        "case reclaiming a token: expected 200, got {status} {body}"
    );
    let (status, body) = create_token(&router, app_id).await;
    assert_eq!(
        status, 200,
        "case minting after a reclaim: expected 200, got {status} {body}"
    );
}

/// One row of the read-path `Ttl` table: a token placed in the store with a LIFETIME and
/// an AGE of its own, so a read that echoed any stored lifetime would show it.
struct TtlCase {
    name: &'static str,
    /// `expires_at - created_at` — the span the MINT reports as `Ttl`.
    lifetime: Duration,
    /// How long ago the token was minted, so the rows differ in how much of the lifetime
    /// is left when the read happens.
    age: Duration,
    reads: usize,
}

/// The read path reports NO lifetime: `GetToken` answers `Ttl` as the empty string
/// whatever lifetime the token it reports carries and however late it is read, while
/// `CreateToken` — the call that grants the lifetime — reports it.
///
/// The rows differ along what the rule generalises over: the token's lifetime (this
/// service's own, a much longer one, a much shorter one), how far into that lifetime the
/// read happens, and how many times the token is read. Each row is seeded directly into
/// the shared store, which is the only way to give a token a lifetime other than the one
/// the mint grants.
#[tokio::test]
async fn the_read_path_reports_no_ttl() {
    let (router, engine) = router_with_engine();
    let app_id = "dreadpathttl01";
    register_app(&router, app_id).await;

    // The MINT half of the rule: `CreateToken` reports the lifetime it granted.
    let (status, body) = create_token(&router, app_id).await;
    assert_eq!(
        status, 200,
        "case the mint: CreateToken expected 200, got {status} {body}"
    );
    assert_eq!(
        field(&body, "ttl"),
        expected_ttl(),
        "case the mint: CreateToken reports the lifetime it granted ({:?}), got {body}",
        expected_ttl()
    );
    let minted_session = field(&body, "sessionId").to_string();
    let (status, body) = get_token(&router, app_id, &minted_session).await;
    assert_eq!(
        status, 200,
        "case the mint: GetToken expected 200, got {status} {body}"
    );
    assert_eq!(
        body.get("ttl"),
        Some(&json!(READ_PATH_TTL)),
        "case the mint: the read of a freshly minted token reports an empty ttl, got {body}"
    );

    let rows = [
        TtlCase {
            name: "this service's own lifetime, read at once",
            lifetime: CHALLENGE_TOKEN_TTL,
            age: Duration::ZERO,
            reads: 1,
        },
        TtlCase {
            name: "this service's own lifetime, nearly run out",
            lifetime: CHALLENGE_TOKEN_TTL,
            age: CHALLENGE_TOKEN_TTL - Duration::from_secs(2),
            reads: 3,
        },
        TtlCase {
            name: "an hour-long lifetime, read a minute in",
            lifetime: Duration::from_secs(3_600),
            age: Duration::from_secs(60),
            reads: 2,
        },
        TtlCase {
            name: "a thirty-second lifetime",
            lifetime: Duration::from_secs(30),
            age: Duration::ZERO,
            reads: 1,
        },
    ];

    for (index, case) in rows.iter().enumerate() {
        let session_id = format!("ttlcase-{index}-{:08x}", case.lifetime.as_secs());
        let stored = seed_token(&engine, app_id, &session_id, case.lifetime, case.age);
        // The stored record DOES carry a lifetime: the read declines to report it, it is
        // not missing from the store.
        assert_eq!(
            stored.ttl(),
            case.lifetime.as_secs().to_string(),
            "case {:?}: the seeded token carries its own lifetime, got {:?}",
            case.name,
            stored.ttl()
        );
        for read in 1..=case.reads {
            let (status, body) = get_token(&router, app_id, &session_id).await;
            assert_eq!(
                status, 200,
                "case {:?} read {read}: GetToken expected 200, got {status} {body}",
                case.name
            );
            assert_eq!(
                body.get("ttl"),
                Some(&json!(READ_PATH_TTL)),
                "case {:?} read {read}: the read reports ttl as the empty string, got {body}",
                case.name
            );
            // The identity members still round-trip: only the lifetime is withheld.
            assert_eq!(
                (
                    field(&body, "appId"),
                    field(&body, "sessionId"),
                    field(&body, "challengeCode")
                ),
                (
                    app_id,
                    session_id.as_str(),
                    stored.challenge_code.as_str()
                ),
                "case {:?} read {read}: the read round-trips the token's identity, got {body}",
                case.name
            );
        }
    }
}

/// Session ids no app in these cases holds a token for. They differ along what the rule
/// generalises over — the FORM and the SIZE of the session id — and none of them is
/// special in the handler: `SessionId` is a bare `__string` `@httpLabel` with no pattern
/// and no length in the model, so every row is a well-formed request that simply names
/// no live session.
fn unheld_session_ids() -> Vec<(&'static str, String)> {
    vec![
        (
            "the all-zero uuid",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "a uuid no mint produced",
            "9f8e7d6c-5b4a-3928-1716-050403020100".to_string(),
        ),
        (
            "a session id in the form this emulator's own mint produces",
            "emulatorabcdefghijkl-0000000000000009".to_string(),
        ),
        ("a single character", "x".to_string()),
        ("not a uuid at all", "not-a-session-id".to_string()),
        (
            "a session id at this emulator's identifier length bound",
            "s".repeat(MAX_IDENTIFIER_CHARS),
        ),
        (
            "a session id one past that bound",
            "s".repeat(MAX_IDENTIFIER_CHARS + 1),
        ),
    ]
}

/// `DeleteToken` validates app identity ONLY, so it is idempotent over session ids: a
/// session this app holds no live token for answers HTTP 200 with `IsSuccess: true`,
/// twice over, and reclaims nothing else.
///
/// The unheld rows span the forms a client can address — including one at this emulator's
/// identifier length bound and one past it, which the store's bound does not refuse
/// because this path inserts nothing.
#[tokio::test]
async fn delete_token_is_idempotent_over_session_ids() {
    let router = router();
    let app_id = "didempotent001";
    register_app(&router, app_id).await;
    // A live token of the same app, so every row also proves the no-op reclaims nothing.
    let (status, body) = create_token(&router, app_id).await;
    assert_eq!(status, 200, "seeding CreateToken expected 200, got {body}");
    let held = field(&body, "sessionId").to_string();

    for (form, session_id) in unheld_session_ids() {
        for attempt in 1..=2 {
            let (status, body) = delete_token(&router, app_id, &session_id).await;
            assert_eq!(
                (status, body.get("isSuccess").and_then(Value::as_bool)),
                (200, Some(true)),
                "case {form:?} attempt {attempt}: expected 200 isSuccess true, got {status} {body}"
            );
        }
        let (status, body) = get_token(&router, app_id, &session_id).await;
        assert_token_not_found(
            &format!("{form}: the no-op delete created nothing"),
            app_id,
            &session_id,
            status,
            &body,
        );
        let (status, body) = get_token(&router, app_id, &held).await;
        assert_eq!(
            status, 200,
            "case {form:?}: the app's live token survives the no-op delete, got {status} {body}"
        );
    }

    // The held session is the positive control: it is reclaimed once, and a repeat is the
    // same success as every unheld row.
    for attempt in 1..=2 {
        let (status, body) = delete_token(&router, app_id, &held).await;
        assert_eq!(
            (status, body.get("isSuccess").and_then(Value::as_bool)),
            (200, Some(true)),
            "case the held session attempt {attempt}: expected 200 isSuccess true, got {status} {body}"
        );
    }
    let (status, body) = get_token(&router, app_id, &held).await;
    assert_token_not_found(
        "the held session, after it was reclaimed",
        app_id,
        &held,
        status,
        &body,
    );
}

/// Every reason a challenge-token read does not resolve is ONE refusal: HTTP 404 with the
/// error code `ResourceNotFoundException` and this service's fixed prefixed message.
///
/// The rows differ in WHY the session does not resolve — never minted (in several forms),
/// deleted, expired, minted under another app — and every one of them answers the same
/// message, so which branch missed is not leaked to the caller.
#[tokio::test]
async fn a_session_that_does_not_resolve_is_one_resource_not_found() {
    let (router, engine) = router_with_engine();
    let app_id = "dnotfound00001";
    let other_app = "dotherowner001";
    register_app(&router, app_id).await;
    register_app(&router, other_app).await;

    let mut rows: Vec<(String, String)> = unheld_session_ids()
        .into_iter()
        .map(|(form, session_id)| (format!("never minted: {form}"), session_id))
        .collect();

    // Deleted: minted, then reclaimed.
    let (status, body) = create_token(&router, app_id).await;
    assert_eq!(status, 200, "seeding CreateToken expected 200, got {body}");
    let deleted = field(&body, "sessionId").to_string();
    let (status, body) = delete_token(&router, app_id, &deleted).await;
    assert_eq!(
        status, 200,
        "seeding DeleteToken expected 200, got {status} {body}"
    );
    rows.push(("deleted by a previous call".to_string(), deleted));

    // Expired: seeded with a lifetime the clock has already run through.
    let expired = "expired-session-0001".to_string();
    seed_token(
        &engine,
        app_id,
        &expired,
        CHALLENGE_TOKEN_TTL,
        CHALLENGE_TOKEN_TTL + Duration::from_secs(1),
    );
    rows.push(("past its lifetime".to_string(), expired));

    // Live, but under ANOTHER app: this app holds no token for it.
    let (status, body) = create_token(&router, other_app).await;
    assert_eq!(status, 200, "seeding CreateToken expected 200, got {body}");
    let elsewhere = field(&body, "sessionId").to_string();
    rows.push(("minted under another app".to_string(), elsewhere));

    let mut messages = BTreeSet::new();
    for (case, session_id) in &rows {
        let (status, body) = get_token(&router, app_id, session_id).await;
        assert_token_not_found(case, app_id, session_id, status, &body);
        messages.insert(field(&body, "message").to_string());
    }
    assert_eq!(
        messages.len(),
        1,
        "every unresolved session answers ONE fixed message, got {messages:?}"
    );
}
