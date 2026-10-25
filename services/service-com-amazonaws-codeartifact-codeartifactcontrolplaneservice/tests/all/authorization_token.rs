// SPDX-License-Identifier: CC0-1.0

//! `GetAuthorizationToken` end to end over the wire.
//!
//! Every case drives the production front door — a `MultiServiceRouter` with this service
//! mounted, addressed by a `codeartifact` endpoint host, speaking restJson1 — so what is
//! asserted is the status, error code and body a CodeArtifact client sees.
//!
//! The rules under test, each over a TABLE rather than one example:
//!
//! 1. A token request for a domain this service holds answers 200 with a non-empty,
//!    freshly generated `authorizationToken` and an `expiration` at the end of the
//!    authorization period the request asked for — the documented 12 hours when it named
//!    none, and the requested number of seconds when it did.
//! 2. The accepted `durationSeconds` set is `0` plus 900..=43200: a value strictly inside
//!    the modelled range but outside that set is `ValidationException` (400), as is one
//!    past the modelled bound.
//! 3. The token is domain-scoped, so a domain the service does not hold — including a held
//!    domain pinned to another `domainOwner` — is `ResourceNotFoundException` (404) and no
//!    token is issued.
//! 4. Each call mints its OWN token: no two calls answer the same secret.
//! 5. Issuing a token changes nothing a later caller sees: the domain record and the
//!    repositories it holds read back unchanged.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::domain_repository_lifecycle::create_repository;
use crate::harness::{call, create_domain, emulator, Answer, ACCOUNT};

/// The documented default authorization period: "CodeArtifact authorization tokens are
/// valid for a period of 12 hours" (`GetAuthorizationToken`'s own documentation).
const DEFAULT_PERIOD_SECONDS: f64 = 12.0 * 60.0 * 60.0;

/// The period a `durationSeconds=0` token gets: the life of the caller's temporary
/// credentials, whose documented default is `AssumeRole`'s 3600 seconds.
const CALLER_CREDENTIAL_PERIOD_SECONDS: f64 = 3_600.0;

/// How far an asserted `expiration` may sit from `now + period`: the wall-clock cost of
/// the call itself, which is a handler invocation rather than a sleep.
const CLOCK_SLACK_SECONDS: f64 = 60.0;

/// `GetAuthorizationToken`, with the optional query parameters sent only when named.
async fn get_token(
    router: &MultiServiceRouter,
    domain: &str,
    duration_seconds: Option<i64>,
    domain_owner: Option<&str>,
) -> Answer {
    let mut uri = format!("/v1/authorization-token?domain={domain}");
    if let Some(duration) = duration_seconds {
        uri.push_str(&format!("&duration={duration}"));
    }
    if let Some(owner) = domain_owner {
        uri.push_str(&format!("&domain-owner={owner}"));
    }
    call(router, "POST", &uri, None).await
}

/// The `authorizationToken` of an answer, or a marker naming what was missing.
fn token_of(answer: &Answer) -> String {
    answer
        .body
        .get("authorizationToken")
        .and_then(Value::as_str)
        .unwrap_or("<missing authorizationToken>")
        .to_string()
}

/// The `expiration` of an answer as epoch seconds (restJson1 serialises a `Timestamp` as
/// an epoch-seconds number).
fn expiration_of(answer: &Answer) -> Option<f64> {
    answer.body.get("expiration").and_then(Value::as_f64)
}

/// Now, as epoch seconds.
fn now_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// `DescribeDomain`, for the "nothing changed" rule.
async fn describe_domain(router: &MultiServiceRouter, domain: &str) -> Answer {
    call(router, "GET", &format!("/v1/domain?domain={domain}"), None).await
}

/// `DeleteDomain`, for the row whose domain is no longer held.
async fn delete_domain(router: &MultiServiceRouter, domain: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/v1/domain?domain={domain}"),
        None,
    )
    .await
}

/// `ListRepositoriesInDomain`, for the same rule.
async fn list_repositories(router: &MultiServiceRouter, domain: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/domain/repositories?domain={domain}"),
        Some(json!({})),
    )
    .await
}

/// RULES 1 and 4: every accepted `durationSeconds` answers a fresh token expiring at the
/// end of the period it asked for.
#[tokio::test]
async fn every_accepted_duration_answers_a_fresh_token_expiring_at_the_end_of_its_period() {
    let router = emulator();
    let domain = "tokens";
    assert_eq!(
        create_domain(&router, domain, None).await.status,
        200,
        "CreateDomain must succeed before a token is requested"
    );

    // Rows: no durationSeconds (the documented default), the caller-credential value, the
    // published minimum and maximum of the accepted interval, and two values inside it.
    let cases: [(&str, Option<i64>, f64); 6] = [
        ("no durationSeconds", None, DEFAULT_PERIOD_SECONDS),
        (
            "the caller-credential value",
            Some(0),
            CALLER_CREDENTIAL_PERIOD_SECONDS,
        ),
        ("the published minimum", Some(900), 900.0),
        ("fifteen minutes past the minimum", Some(1_800), 1_800.0),
        ("two hours", Some(7_200), 7_200.0),
        ("the published maximum", Some(43_200), 43_200.0),
    ];

    let mut issued: Vec<String> = Vec::new();
    for (case, duration, expected_period) in cases {
        let before = now_seconds();
        let answer = get_token(&router, domain, duration, None).await;
        let after = now_seconds();

        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {answer:?}"
        );
        let token = token_of(&answer);
        assert!(
            !token.is_empty() && !token.starts_with('<'),
            "case {case:?}: expected a non-empty authorizationToken, got {token:?}"
        );
        let expiration = expiration_of(&answer);
        let Some(expiration) = expiration else {
            panic!("case {case:?}: expected an epoch-seconds expiration, got {answer:?}");
        };
        let lower = before + expected_period - CLOCK_SLACK_SECONDS;
        let upper = after + expected_period + CLOCK_SLACK_SECONDS;
        assert!(
            expiration >= lower && expiration <= upper,
            "case {case:?}: expected an expiration in {lower:?}..={upper:?}, got {expiration:?}"
        );

        // RULE 4: the token is minted per call, never reused.
        assert!(
            !issued.contains(&token),
            "case {case:?}: expected a token no earlier call was given, got {token:?}"
        );
        issued.push(token);
    }
}

/// RULE 2: the accepted `durationSeconds` set is `0` plus 900..=43200 — a value in the gap
/// below 900, or past the modelled ceiling, is refused rather than served a shorter token.
#[tokio::test]
async fn a_duration_outside_the_published_set_is_refused() {
    let router = emulator();
    let domain = "durations";
    assert_eq!(create_domain(&router, domain, None).await.status, 200);

    // Rows: each end of the gap between 0 and the minimum, a value in its middle, and one
    // past each published bound.
    let cases: [(&str, i64); 6] = [
        ("one second", 1),
        ("one below the published minimum", 899),
        ("inside the gap", 300),
        ("one past the published maximum", 43_201),
        ("far past the published maximum", 86_400),
        ("negative", -1),
    ];

    for (case, duration) in cases {
        let answer = get_token(&router, domain, Some(duration), None).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {answer:?}"
        );
        assert_eq!(
            answer.code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        assert!(
            answer.body.get("authorizationToken").is_none(),
            "case {case:?}: a refusal must carry no token, got {answer:?}"
        );
    }

    // The control rows: each published bound of the accepted set IS served.
    for (case, duration) in [
        ("the published minimum", 900),
        ("the published maximum", 43_200),
    ] {
        let answer = get_token(&router, domain, Some(duration), None).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {answer:?}"
        );
    }
}

/// RULE 3: a token is scoped to a domain, so a domain the service does not hold has none.
#[tokio::test]
async fn a_domain_the_service_does_not_hold_has_no_token() {
    let router = emulator();
    let domain = "held";
    assert_eq!(create_domain(&router, domain, None).await.status, 200);

    // A domain that WAS held and is not any more: the scope goes away with it.
    let removed = "removed";
    assert_eq!(create_domain(&router, removed, None).await.status, 200);
    assert_eq!(
        delete_domain(&router, removed).await.status,
        200,
        "DeleteDomain must succeed before the removed-domain row runs"
    );

    // (case, domain, domainOwner)
    let cases: [(&str, &str, Option<&str>); 4] = [
        ("no such domain", "absent", None),
        (
            "no such domain pinned to its own account",
            "absent",
            Some(ACCOUNT),
        ),
        (
            "a held domain pinned to another account",
            domain,
            Some("210987654321"),
        ),
        ("a domain that was deleted", removed, None),
    ];

    for (case, case_domain, owner) in cases {
        let answer = get_token(&router, case_domain, None, owner).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {answer:?}"
        );
        assert_eq!(
            answer.code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        assert!(
            answer.body.get("authorizationToken").is_none(),
            "case {case:?}: a refusal must carry no token, got {answer:?}"
        );
    }

    // The control rows: the held domain serves, whether or not the request pins it to the
    // account that owns it.
    for (case, owner) in [
        ("unpinned", None),
        ("pinned to the owning account", Some(ACCOUNT)),
    ] {
        let answer = get_token(&router, domain, None, owner).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {answer:?}"
        );
    }
}

/// RULE 5: issuing a token leaves every later read unchanged — the domain record and the
/// repositories it holds.
#[tokio::test]
async fn issuing_a_token_changes_nothing_a_later_caller_sees() {
    let router = emulator();
    let domain = "stable";
    assert_eq!(create_domain(&router, domain, None).await.status, 200);
    for repository in ["alpha", "beta"] {
        assert_eq!(
            create_repository(&router, domain, repository, None, &[])
                .await
                .status,
            200,
            "CreateRepository {repository:?} must succeed"
        );
    }

    let domain_before = describe_domain(&router, domain).await.body;
    let repositories_before = list_repositories(&router, domain).await.body;

    // Several issues, over the whole accepted duration set.
    for duration in [None, Some(0), Some(900), Some(43_200)] {
        let answer = get_token(&router, domain, duration, None).await;
        assert_eq!(
            answer.status, 200,
            "case {duration:?}: expected 200, got {answer:?}"
        );
    }

    let domain_after = describe_domain(&router, domain).await.body;
    let repositories_after = list_repositories(&router, domain).await.body;
    assert_eq!(
        domain_after, domain_before,
        "issuing a token must not change the domain record: {domain_before:?} became {domain_after:?}"
    );
    assert_eq!(
        repositories_after, repositories_before,
        "issuing a token must not change the domain's repositories: \
         {repositories_before:?} became {repositories_after:?}"
    );
}
