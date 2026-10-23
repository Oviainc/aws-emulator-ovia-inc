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

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_amplifybackend_amplifybackend::state::{
    ChallengeToken, ComAmazonawsAmplifybackendAmplifybackendState, CHALLENGE_CODE_CHARS,
    CHALLENGE_TOKEN_TTL, LIMIT_TYPE_CHALLENGE_TOKENS, MAX_CHALLENGE_TOKENS_PER_APP,
    MAX_IDENTIFIER_CHARS, RESOURCE_TYPE_CHALLENGE_TOKEN,
};
use service_com_amazonaws_amplifybackend_amplifybackend::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The endpoint host an AmplifyBackend request addresses; its first label is the routing
/// key the router claims the request by.
const HOST: &str = "amplifybackend.us-east-1.amazonaws.com";

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

/// The `Ttl` every token reports: [`CHALLENGE_TOKEN_TTL`] in whole seconds, as a string.
fn expected_ttl() -> String {
    CHALLENGE_TOKEN_TTL.as_secs().to_string()
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

/// `CreateToken` mints a token, stores it, and `GetToken` reports the SAME token back;
/// `DeleteToken` reclaims it, after which both answer the modelled `NotFoundException`.
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
        // changes nothing a later caller sees.
        for token in &minted {
            for read in 1..=2 {
                let (status, body) = get_token(&router, &app_id, &token.session_id).await;
                assert_eq!(
                    status, 200,
                    "case {name:?} read {read}: GetToken expected 200, got {status} {body}"
                );
                assert_eq!(
                    body, token.body,
                    "case {name:?} read {read}: GetToken reports the minted token, expected {}, got {body}",
                    token.body
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
        for (operation, (status, body)) in [
            (
                "GetToken",
                get_token(&router, &app_id, &reclaimed.session_id).await,
            ),
            (
                "DeleteToken",
                delete_token(&router, &app_id, &reclaimed.session_id).await,
            ),
        ] {
            assert_eq!(
                status, 404,
                "case {name:?}: {operation} on a deleted token expected 404, got {status} {body}"
            );
            assert_eq!(
                field(&body, "resourceType"),
                RESOURCE_TYPE_CHALLENGE_TOKEN,
                "case {name:?}: {operation} names the resource type, got {body}"
            );
            assert!(
                field(&body, "message").contains(&reclaimed.session_id),
                "case {name:?}: {operation}'s message names the session asked for, got {body}"
            );
        }
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

/// A token is addressed by (app id, session id): a session minted under one app is not
/// found under another, and the mismatched lookup leaves it alone.
#[tokio::test]
async fn a_token_lookup_is_scoped_to_its_app() {
    let router = router();
    let (owner, other) = ("downerapp0001", "dotherapp0002");
    // BOTH apps are registered: the lookup under `other` must fail on the TOKEN, which
    // is what this case is about, not on the app's identity.
    register_app(&router, owner).await;
    register_app(&router, other).await;
    let (status, body) = create_token(&router, owner).await;
    assert_eq!(status, 200, "seeding CreateToken expected 200, got {body}");
    let session_id = field(&body, "sessionId").to_string();

    for (operation, (status, body)) in [
        ("GetToken", get_token(&router, other, &session_id).await),
        (
            "DeleteToken",
            delete_token(&router, other, &session_id).await,
        ),
    ] {
        assert_eq!(
            status, 404,
            "case {operation:?} under the wrong app: expected 404, got {status} {body}"
        );
        assert_eq!(
            field(&body, "resourceType"),
            RESOURCE_TYPE_CHALLENGE_TOKEN,
            "case {operation:?} under the wrong app: names the resource type, got {body}"
        );
    }
    let (status, body) = get_token(&router, owner, &session_id).await;
    assert_eq!(
        status, 200,
        "the owner's token survives a lookup under another app, got {status} {body}"
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
        let (status, body) = delete_token(&router, app_id, &session_id).await;
        assert_eq!(
            status, expected,
            "case {case:?}: DeleteToken expected {expected}, got {status} {body}"
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
