// SPDX-License-Identifier: CC0-1.0

//! The OAuth 2.0 token operations' own acceptance of a request, end to end over HTTP.
//!
//! One rule, in two halves, and both are about WHO judges the request — the operation,
//! never the framework's generic constraint walk:
//!
//! * `RevokeOAuth2TokenWithIAM` owns acceptance of its token: revocation is
//!   idempotent-safe, so ANY value — the empty string, an access-token-shaped value, a
//!   value no Sign-In token could have — is answered 200 with an empty output and
//!   revokes only a refresh token the store actually holds. The model says exactly this:
//!   "revoking an already-revoked, expired, or otherwise invalid token still returns
//!   200 OK with an empty body", and "only the refresh_token type is accepted".
//! * `CreateOAuth2TokenWithIAM` and `IntrospectOAuth2TokenWithIAM` own rejection of
//!   theirs: each refuses with `ValidationException` naming the SINGLE condition that
//!   failed — the OAuth 2.0 `invalid_request` description the model documents for
//!   `INVALID_REQUEST`, or the token's own `token is required` / `token has invalid
//!   format` — never the framework's multi-constraint report.
//!
//! What a minted token then reports, and what a revocation does to a chain, is
//! `oauth2_token_lifecycle.rs`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::signin_harness::{emulator, post, Answer};

/// The description an OAuth 2.0 request is refused with.
///
/// Spelled out here rather than imported from the crate, so a wrong message in `src`
/// fails this test. Source: the model documents the `INVALID_REQUEST` member of
/// `OAuth2ErrorCode` as "The request is missing a required parameter, includes an
/// invalid parameter value, or is otherwise malformed" — RFC 6749 §5.2's own
/// description of `invalid_request`.
const INVALID_REQUEST_MESSAGE: &str = "The request is missing a required parameter, \
     includes an invalid parameter value, or is otherwise malformed.";

/// `IntrospectOAuth2TokenWithIAM`'s refusal of a request with no token value.
const TOKEN_REQUIRED_MESSAGE: &str = "token is required";

/// `IntrospectOAuth2TokenWithIAM`'s refusal of a value that is not shaped like a
/// Sign-In token.
const TOKEN_INVALID_FORMAT_MESSAGE: &str = "token has invalid format";

/// The `error` member every modelled `ValidationException` of this service carries:
/// "Will be INVALID_REQUEST for validation errors".
const VALIDATION_ERROR_CODE: &str = "INVALID_REQUEST";

/// The grant `CreateOAuth2TokenWithIAM` implements: `ClientCredentialsGrantType` is
/// `^client_credentials$`.
const CLIENT_CREDENTIALS: &str = "client_credentials";

/// A text the framework's generic constraint reporter puts in every message it writes.
/// No message under test may contain it: that is the difference this suite pins.
const FRAMEWORK_CONSTRAINT_REPORT: &str = "failed to satisfy constraint";

async fn create_token(router: &MultiServiceRouter, request: Value) -> Answer {
    post(router, "/v1/token?x-amz-client-auth-method=iam", request).await
}

async fn introspect_token(router: &MultiServiceRouter, request: Value) -> Answer {
    post(router, "/v1/introspect?x-amz-client-auth-method=iam", request).await
}

async fn revoke_token(router: &MultiServiceRouter, request: Value) -> Answer {
    post(router, "/v1/revoke?x-amz-client-auth-method=iam", request).await
}

/// Mint one access token for `resource`, asserting the mint succeeded, and report it.
async fn mint(router: &MultiServiceRouter, case: &str, resource: &str) -> String {
    let answer = create_token(
        router,
        json!({"grant_type": CLIENT_CREDENTIALS, "resource": resource}),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: a client-credentials mint must be accepted, got {} with {:?}",
        answer.status, answer.body
    );
    let token = answer.body["access_token"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !token.is_empty(),
        "case {case:?}: the mint must report an access token, got {:?}",
        answer.body
    );
    token
}

/// Revocation accepts every token value, whatever its shape, as a 200 with an empty
/// body.
///
/// The rows differ along what the revocation shape generalises over: the empty value, a
/// well-formed refresh token, an access-token-shaped value, a value whose prefix is
/// wrong, one whose alphabet is wrong, and values at and past `RevocationToken`'s
/// modelled 4096-character ceiling.
#[tokio::test]
async fn revocation_accepts_every_token_value_as_a_no_op() {
    let cases: Vec<(&str, String)> = vec![
        ("the empty string", String::new()),
        (
            "a refresh-token-shaped value the store never minted",
            format!("ASOR{}", "a".repeat(32)),
        ),
        (
            "an access-token-shaped value, which revocation does not accept",
            format!("ASOA{}", "b".repeat(32)),
        ),
        ("a value with no Sign-In prefix", "not-a-signin-token-at-all".to_string()),
        ("the prefix alone, with no body", "ASOR".to_string()),
        ("a lowercase prefix", format!("asor{}", "c".repeat(32))),
        (
            "a body outside the token alphabet",
            "ASOR not a token!".to_string(),
        ),
        (
            "a value at the modelled 4096-character ceiling",
            format!("ASOR{}", "d".repeat(4092)),
        ),
        (
            "a value one character past that ceiling",
            format!("ASOR{}", "e".repeat(4093)),
        ),
        ("a non-ASCII value", "ASOR✗token".to_string()),
    ];

    let router = emulator();
    for (case, token) in cases {
        let answer = revoke_token(&router, json!({"token": token})).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: revocation is idempotent-safe and must answer 200, got {} \
             with {:?}",
            answer.status, answer.body
        );
        // The operation models an empty output, so the restJson1 envelope for it is
        // either no body at all or the empty JSON document — never an error payload.
        let body = String::from_utf8_lossy(&answer.raw_body).to_string();
        assert!(
            body.is_empty() || body == "{}",
            "case {case:?}: revocation must answer an empty output, got {body:?}"
        );
    }
}

/// Revocation only ever revokes a refresh token: the access token a mint returned stays
/// active after its own value is presented for revocation, and after any other value
/// is.
#[tokio::test]
async fn revocation_leaves_an_access_token_active() {
    let router = emulator();
    let token = mint(&router, "an access token presented for revocation", "aws-mcp.amazonaws.com").await;

    for (case, presented) in [
        ("the access token itself", token.clone()),
        ("a refresh-token-shaped value", format!("ASOR{}", "f".repeat(32))),
        ("the empty string", String::new()),
    ] {
        let revoked = revoke_token(&router, json!({"token": presented})).await;
        assert_eq!(
            revoked.status, 200,
            "case {case:?}: revocation must answer 200, got {} with {:?}",
            revoked.status, revoked.body
        );
        let introspected = introspect_token(&router, json!({"token": token.clone()})).await;
        assert_eq!(
            (introspected.status, introspected.body["active"].as_bool()),
            (200, Some(true)),
            "case {case:?}: the minted access token must still be active, got {} with \
             {:?}",
            introspected.status,
            introspected.body
        );
    }
}

/// A mint refuses a request the operation does not accept with its own single-condition
/// description, and accepts every request it does.
///
/// The rows differ along both members the operation judges: `grantType` away from
/// `client_credentials` (another OAuth grant name, the wrong case, the empty value) and
/// `resource` outside its modelled `@length(min: 1, max: 2048)` — tested at each
/// published bound and one character past the ceiling.
#[tokio::test]
async fn a_mint_judges_its_own_request() {
    let refused: Vec<(&str, Value)> = vec![
        (
            "the authorization-code grant",
            json!({"grant_type": "authorization_code", "resource": "aws-mcp.amazonaws.com"}),
        ),
        (
            "the refresh-token grant",
            json!({"grant_type": "refresh_token", "resource": "aws-mcp.amazonaws.com"}),
        ),
        (
            "the right grant in the wrong case",
            json!({"grant_type": "CLIENT_CREDENTIALS", "resource": "aws-mcp.amazonaws.com"}),
        ),
        (
            "an empty grant type",
            json!({"grant_type": "", "resource": "aws-mcp.amazonaws.com"}),
        ),
        (
            "the grant name with trailing text",
            json!({"grant_type": "client_credentials_extra", "resource": "aws-mcp.amazonaws.com"}),
        ),
        (
            "an empty resource, below the modelled length floor",
            json!({"grant_type": CLIENT_CREDENTIALS, "resource": ""}),
        ),
        (
            "a resource one character past the modelled ceiling",
            json!({"grant_type": CLIENT_CREDENTIALS, "resource": "r".repeat(2049)}),
        ),
    ];
    let accepted: Vec<(&str, Value)> = vec![
        (
            "the documented example resource",
            json!({"grant_type": CLIENT_CREDENTIALS, "resource": "aws-mcp.amazonaws.com"}),
        ),
        (
            "a one-character resource, at the modelled length floor",
            json!({"grant_type": CLIENT_CREDENTIALS, "resource": "r"}),
        ),
        (
            "a resource at the modelled 2048-character ceiling",
            json!({"grant_type": CLIENT_CREDENTIALS, "resource": "r".repeat(2048)}),
        ),
    ];

    let router = emulator();
    for (case, request) in refused {
        let answer = create_token(&router, request).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} with {:?}",
            answer.status,
            answer.body
        );
        assert_eq!(
            (answer.error_member(), answer.message()),
            (VALIDATION_ERROR_CODE, INVALID_REQUEST_MESSAGE),
            "case {case:?}: expected the operation's own invalid_request description, \
             got {:?}",
            answer.body
        );
        assert!(
            !answer.message().contains(FRAMEWORK_CONSTRAINT_REPORT),
            "case {case:?}: the message must come from the operation, not the generic \
             constraint reporter, got {:?}",
            answer.message()
        );
    }

    for (case, request) in accepted {
        let answer = create_token(&router, request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the mint must be accepted, got {} with {:?}",
            answer.status, answer.body
        );
    }
}

/// Introspection refuses a token value the operation does not accept by naming the one
/// condition it failed: no value at all is `token is required`, and anything not shaped
/// like a Sign-In token is `token has invalid format`.
#[tokio::test]
async fn introspection_names_the_one_condition_a_token_failed() {
    // (case, token, the message the operation states).
    let cases: Vec<(&str, String, &str)> = vec![
        ("no value at all", String::new(), TOKEN_REQUIRED_MESSAGE),
        (
            "a value no Sign-In token could have",
            "not-a-signin-token-at-all".to_string(),
            TOKEN_INVALID_FORMAT_MESSAGE,
        ),
        (
            "a prefix one letter short",
            "ASO1234567890".to_string(),
            TOKEN_INVALID_FORMAT_MESSAGE,
        ),
        (
            "a Sign-In prefix with the wrong kind letter",
            format!("ASOX{}", "a".repeat(32)),
            TOKEN_INVALID_FORMAT_MESSAGE,
        ),
        (
            "a lowercase prefix",
            format!("asoa{}", "a".repeat(32)),
            TOKEN_INVALID_FORMAT_MESSAGE,
        ),
        (
            "an access-token prefix with no body",
            "ASOA".to_string(),
            TOKEN_INVALID_FORMAT_MESSAGE,
        ),
        (
            "a body outside the token alphabet",
            "ASOA token with spaces".to_string(),
            TOKEN_INVALID_FORMAT_MESSAGE,
        ),
        (
            "a value one character past the modelled 4096 ceiling",
            format!("ASOA{}", "a".repeat(4093)),
            TOKEN_INVALID_FORMAT_MESSAGE,
        ),
    ];

    let router = emulator();
    for (case, token, expected) in cases {
        let answer = introspect_token(&router, json!({"token": token})).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} with {:?}",
            answer.status,
            answer.body
        );
        assert_eq!(
            (answer.error_member(), answer.message()),
            (VALIDATION_ERROR_CODE, expected),
            "case {case:?}: expected the operation's own message {expected:?}, got {:?}",
            answer.body
        );
        assert!(
            !answer.message().contains(FRAMEWORK_CONSTRAINT_REPORT),
            "case {case:?}: the message must come from the operation, not the generic \
             constraint reporter, got {:?}",
            answer.message()
        );
    }
}

/// The mint accepts every request it judges well formed, and what the token it returns
/// then reports is `oauth2_token_lifecycle.rs`' subject, not this suite's.
#[tokio::test]
async fn a_mint_returns_its_own_token_for_each_accepted_request() {
    // The rows differ in the resource the token is scoped to, including both published
    // bounds of `resource`'s `@length(min: 1, max: 2048)`.
    let resources = [
        (
            "the documented example resource",
            "aws-mcp.amazonaws.com".to_string(),
        ),
        ("a one-character resource", "r".to_string()),
        (
            "another service resource",
            "lakeformation.amazonaws.com".to_string(),
        ),
        ("a resource at the length ceiling", "r".repeat(2048)),
    ];

    let router = emulator();
    let mut minted = Vec::new();
    for (case, resource) in &resources {
        let token = mint(&router, case, resource).await;
        assert!(
            token.starts_with("ASOA") && token.len() > 4,
            "case {case:?}: an access token carries the documented ASOA prefix, got \
             {token:?}"
        );
        // The token it reports is one introspection accepts and reports active.
        let answer = introspect_token(&router, json!({"token": &token})).await;
        assert_eq!(
            (answer.status, answer.body["active"].as_bool()),
            (200, Some(true)),
            "case {case:?}: the minted token must introspect as active, got {} with {:?}",
            answer.status,
            answer.body
        );
        minted.push(token);
    }

    minted.sort();
    let distinct = {
        let mut values = minted.clone();
        values.dedup();
        values.len()
    };
    assert_eq!(
        distinct,
        minted.len(),
        "each mint must return its own token, got {minted:?}"
    );
}
