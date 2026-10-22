// SPDX-License-Identifier: CC0-1.0

//! `GetAuthorizationToken`, driven end to end over the wire.
//!
//! Every case here goes through the front door a client uses: `POST /` with
//! `X-Amz-Target: SpencerFrontendService.<Operation>` and an `awsJson1_1` body
//! (`OPERATIONS.md`, "Wire protocol"), dispatched by the multi-service router.
//!
//! The rule under test: every call MINTS one authorization token for the registry,
//! reported as a single `authorizationData` object whose `authorizationToken` is the
//! base64 of `AWS:<freshly minted password>` and whose `expiresAt` is the call's instant
//! plus the documented 12-hour validity. The operation models no input, so the class the
//! rule generalises over is the registry the call is made against and the number of
//! calls already made against it: the table below spans an empty registry, registries
//! holding one and several repositories, registries that have already issued tokens, and
//! two independent registries — every row must answer the same way, and no two tokens
//! may ever repeat.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix from the crate's `OPERATIONS.md` wire-protocol section.
const TARGET_PREFIX: &str = "SpencerFrontendService";

/// The user half of the `user:password` pair the token decodes to, as the handler's
/// `AUTHORIZATION_TOKEN_USER` documents it from `AuthorizationData$authorizationToken`.
const EXPECTED_USER: &str = "AWS";

/// The documented validity of an issued token, in seconds: `AuthorizationData$expiresAt`
/// — "Authorization tokens are valid for 12 hours."
const LIFETIME_SECONDS: f64 = 12.0 * 60.0 * 60.0;

/// How far `expiresAt` may sit from `now + 12h` and still be the same instant: the slack
/// covers only the wall-clock time the call itself takes.
const CLOCK_SLACK_SECONDS: f64 = 120.0;

/// The modelled member names of `AuthorizationData`, as the awsJson1_1 wire carries them.
const MEMBER_TOKEN: &str = "authorizationToken";
const MEMBER_EXPIRY: &str = "expiresAt";

/// A router with just this service mounted on a fresh engine — one independent registry.
fn registry() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The signature itself is not what these cases are about; the credential scope
    // below still routes the request to `ecr-public`.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ecrpublic_spencerfrontendservice::EmulatedService::new(engine),
    );
    router
}

/// One `awsJson1_1` call, returning the HTTP status and the parsed response body.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDECRPUBLIC/20260101/us-east-1\
             /ecr-public/aws4_request, SignedHeaders=host;x-amz-target, Signature=00",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&payload)
            .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
    };
    (status, parsed)
}

/// A call that must succeed, returning its body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: Value, case: &str) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} with {parsed}"
    );
    parsed
}

/// Seconds since the Unix epoch, the unit `expiresAt` is carried in.
fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the Unix epoch")
        .as_secs_f64()
}

/// One issued token, as the caller sees it: the encoded token and its expiry.
#[derive(Debug, Clone)]
struct IssuedToken {
    token: String,
    expires_at: f64,
}

/// Calls `GetAuthorizationToken` against `router` and asserts everything the rule says
/// about one issued token, bar uniqueness (which only a set of them can show).
///
/// The decoded password is a secret, so nothing here ever prints it — only its length.
async fn issue(router: &MultiServiceRouter, case: &str) -> IssuedToken {
    let before = unix_now();
    let body = ok(router, "GetAuthorizationToken", json!({}), case).await;
    let after = unix_now();

    // `GetAuthorizationTokenResponse$authorizationData` targets the `AuthorizationData`
    // STRUCTURE, so one call reports exactly one object — not the list ECR's private
    // registry API answers with.
    let data = &body["authorizationData"];
    assert!(
        data.is_object(),
        "case {case:?}: expected authorizationData to be a single object, got {body}"
    );
    let members: BTreeSet<&str> = data
        .as_object()
        .expect("checked above")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        members,
        BTreeSet::from([MEMBER_TOKEN, MEMBER_EXPIRY]),
        "case {case:?}: expected authorizationData to carry exactly the modelled \
         members, got {members:?}"
    );

    let token = data[MEMBER_TOKEN]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string authorizationToken in {body}"))
        .to_string();
    // `Base64`'s modelled pattern is `^\S+$`: no whitespace anywhere, and never empty.
    assert!(
        !token.is_empty() && !token.chars().any(char::is_whitespace),
        "case {case:?}: expected a non-empty whitespace-free token, got one of length {}",
        token.len()
    );

    let decoded = ::aws_smithy_types::base64::decode(&token).unwrap_or_else(|error| {
        panic!("case {case:?}: expected a base64 authorizationToken, got {error}")
    });
    let decoded = String::from_utf8(decoded)
        .unwrap_or_else(|_| panic!("case {case:?}: expected the token to decode to text"));
    let (user, password) = decoded
        .split_once(':')
        .unwrap_or_else(|| panic!("case {case:?}: expected a `user:password` pair once decoded"));
    assert_eq!(
        user, EXPECTED_USER,
        "case {case:?}: expected the decoded user to be {EXPECTED_USER:?}, got {user:?}"
    );
    assert!(
        !password.is_empty() && !password.chars().any(char::is_whitespace),
        "case {case:?}: expected a non-empty whitespace-free password, got one of length {}",
        password.len()
    );

    let expires_at = data[MEMBER_EXPIRY]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a numeric expiresAt in {body}"));
    assert!(
        expires_at > after,
        "case {case:?}: expected expiresAt in the future (after {after}), got {expires_at}"
    );
    let earliest = before + LIFETIME_SECONDS - CLOCK_SLACK_SECONDS;
    let latest = after + LIFETIME_SECONDS + CLOCK_SLACK_SECONDS;
    assert!(
        (earliest..=latest).contains(&expires_at),
        "case {case:?}: expected expiresAt in {earliest}..={latest} \
         (the call's instant plus 12 hours), got {expires_at}"
    );

    IssuedToken { token, expires_at }
}

#[tokio::test]
async fn every_call_mints_one_aws_user_token_valid_for_twelve_hours() {
    // Each row is a registry in a different state when the asserted tokens are issued:
    // (case, repositories to create first, tokens to issue and discard first, tokens to
    // assert). Creating a repository is not a different kind of input to this operation,
    // which models none — what differs is the state of the registry it answers for.
    let rows: [(&str, &[&str], usize, usize); 5] = [
        ("empty registry, first call", &[], 0, 1),
        ("empty registry, three prior tokens", &[], 3, 2),
        ("one repository", &["alpha"], 0, 2),
        (
            "several repositories and prior tokens",
            &["alpha", "team/app", "x1.y2_z3"],
            2,
            3,
        ),
        ("one repository, five tokens in a row", &["only"], 1, 4),
    ];

    // Across the whole table: an issued credential is never repeated, not within one
    // registry and not between two of them.
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for (case, repositories, prior_tokens, asserted_tokens) in rows {
        let router = registry();
        for name in repositories {
            ok(
                &router,
                "CreateRepository",
                json!({"repositoryName": name}),
                case,
            )
            .await;
        }
        for round in 0..prior_tokens {
            let prior = issue(&router, &format!("{case} (warm-up token {round})")).await;
            assert!(
                seen.insert(prior.token),
                "case {case:?}: warm-up token {round} repeated an already issued token"
            );
        }

        let mut expiries = Vec::new();
        for round in 0..asserted_tokens {
            let issued = issue(&router, &format!("{case} (token {round})")).await;
            expiries.push(issued.expires_at);
            assert!(
                seen.insert(issued.token),
                "case {case:?}: token {round} repeated an already issued token"
            );
        }

        // Each call carries its OWN 12-hour window measured from that call, so a later
        // token never expires before an earlier one.
        for pair in expiries.windows(2) {
            assert!(
                pair[1] >= pair[0],
                "case {case:?}: expected a later token to expire no earlier than an \
                 earlier one, got {:?} then {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    assert_eq!(
        seen.len(),
        1 + 5 + 2 + 5 + 5,
        "expected every issued token across the table to be distinct, got {} distinct \
         of the issued ones",
        seen.len()
    );
}

#[tokio::test]
async fn two_independent_registries_issue_different_tokens() {
    // The token is a credential this call mints, so two emulators that share nothing
    // must not hand out the same one.
    let first = issue(&registry(), "first registry").await;
    let second = issue(&registry(), "second registry").await;
    assert_ne!(
        first.token, second.token,
        "expected two registries to issue different tokens"
    );
}
