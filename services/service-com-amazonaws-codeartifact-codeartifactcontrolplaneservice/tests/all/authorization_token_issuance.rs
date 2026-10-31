// SPDX-License-Identifier: CC0-1.0

//! The INVARIANTS of `GetAuthorizationToken`, over the wire.
//!
//! `tests/all/authorization_token.rs` pins the value each accepted `durationSeconds`
//! answers — the period, the status and the error code of each row. This suite pins the
//! properties that hold ACROSS rows, which no single row can show:
//!
//! 1. ORDERING: the `expiration` an answer carries is ordered by the authorization period
//!    the request asked for. Two requests served moments apart for different periods come
//!    back in the period's order, and two requests for the SAME period come back at the
//!    same instant (within the wall-clock cost of the calls).
//! 2. OPACITY: the token is a freshly generated secret, so it is a function of nothing in
//!    the request — two calls with identical arguments answer different tokens, and no
//!    token carries the domain it was issued for or the account that owns it.
//! 3. STATE-DERIVED SCOPE: whether a token is issued follows the domains the service
//!    HOLDS at the moment of the call, so the same request is refused before a domain is
//!    created, served while it is held, and refused again once it is gone — and issuing
//!    for one domain grants nothing for another.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, create_domain, emulator, Answer, ACCOUNT};

/// The documented default authorization period: "CodeArtifact authorization tokens are
/// valid for a period of 12 hours" (`GetAuthorizationToken`'s own documentation).
const DEFAULT_PERIOD_SECONDS: f64 = 12.0 * 60.0 * 60.0;

/// The period a `durationSeconds=0` token gets when the caller presented no session: 0
/// ties the token to the expiration of the caller's own temporary credentials, and a
/// credential with no session behind it carries none, so the token gets the full
/// documented authorization period.
const CALLER_CREDENTIAL_PERIOD_SECONDS: f64 = DEFAULT_PERIOD_SECONDS;

/// How far two expirations that must be the SAME instant may sit apart, and the slack
/// allowed when comparing one against `now + period`: the wall-clock cost of the calls
/// themselves, which are handler invocations rather than sleeps.
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

/// `DeleteDomain`, for the row whose domain stops being held.
async fn delete_domain(router: &MultiServiceRouter, domain: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/v1/domain?domain={domain}"),
        None,
    )
    .await
}

/// The `authorizationToken` of a successful answer.
fn token_of(answer: &Answer) -> String {
    answer
        .body
        .get("authorizationToken")
        .and_then(Value::as_str)
        .unwrap_or("<missing authorizationToken>")
        .to_string()
}

/// The `expiration` of a successful answer as epoch seconds (restJson1 serialises a
/// `Timestamp` as an epoch-seconds number).
fn expiration_of(answer: &Answer) -> f64 {
    answer
        .body
        .get("expiration")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("no epoch-seconds expiration in {answer:?}"))
}

/// RULE 1: the expirations of tokens issued moments apart are ordered by the period each
/// request asked for, and equal periods land on the same instant.
#[tokio::test]
async fn expirations_are_ordered_by_the_period_each_request_asked_for() {
    let router = emulator();
    let domain = "ordering";
    assert_eq!(
        create_domain(&router, domain, None).await.status,
        200,
        "CreateDomain must succeed before a token is requested"
    );

    // Rows in ascending period order. "No durationSeconds", the published maximum and the
    // caller-credential value share a period — the documented default IS 12 hours, which is
    // also the ceiling and the period a token tied to a credential with no session behind
    // it gets.
    let cases: [(&str, Option<i64>, f64); 6] = [
        ("the published minimum", Some(900), 900.0),
        ("an hour", Some(3_600), 3_600.0),
        ("two hours", Some(7_200), 7_200.0),
        ("the published maximum", Some(43_200), 43_200.0),
        ("no durationSeconds", None, DEFAULT_PERIOD_SECONDS),
        (
            "the caller-credential value",
            Some(0),
            CALLER_CREDENTIAL_PERIOD_SECONDS,
        ),
    ];

    let mut answered: Vec<(&str, f64, f64)> = Vec::new();
    for (case, duration, period) in cases {
        let answer = get_token(&router, domain, duration, None).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {answer:?}"
        );
        answered.push((case, period, expiration_of(&answer)));
    }

    for (case, period, expiration) in &answered {
        for (other_case, other_period, other_expiration) in &answered {
            let difference = other_expiration - expiration;
            if (other_period - period).abs() <= CLOCK_SLACK_SECONDS {
                // Same period: the two answers name the same instant, up to the cost of
                // the calls between them.
                assert!(
                    difference.abs() <= CLOCK_SLACK_SECONDS,
                    "case {case:?} vs {other_case:?}: equal periods {period:?} and \
                     {other_period:?} must expire together, got a gap of {difference:?}"
                );
            } else if other_period > period {
                // A longer period expires strictly later, by about the difference between
                // the two periods.
                let expected = other_period - period;
                assert!(
                    (difference - expected).abs() <= CLOCK_SLACK_SECONDS,
                    "case {case:?} vs {other_case:?}: expected the expirations to differ \
                     by about {expected:?} seconds, got {difference:?}"
                );
            }
        }
    }
}

/// RULE 2: the token is generated, not derived — identical requests answer different
/// tokens and no token carries anything the request named.
#[tokio::test]
async fn each_call_answers_a_fresh_token_that_carries_nothing_from_the_request() {
    let router = emulator();
    // Two domains, so a token can be checked against a name it was NOT issued for too.
    let domains = ["mint-domain-first", "mint-domain-second"];
    for domain in domains {
        assert_eq!(
            create_domain(&router, domain, None).await.status,
            200,
            "CreateDomain {domain:?} must succeed"
        );
    }

    // (case, domain, durationSeconds, domainOwner) — including the same arguments twice,
    // which is the row that shows the token is not a function of them.
    let cases: [(&str, &str, Option<i64>, Option<&str>); 6] = [
        ("the first domain, default period", domains[0], None, None),
        (
            "the first domain, default period again",
            domains[0],
            None,
            None,
        ),
        (
            "the first domain, pinned to its owner",
            domains[0],
            None,
            Some(ACCOUNT),
        ),
        (
            "the first domain, the published minimum",
            domains[0],
            Some(900),
            None,
        ),
        (
            "the second domain, the caller-credential value",
            domains[1],
            Some(0),
            None,
        ),
        (
            "the second domain, the published maximum",
            domains[1],
            Some(43_200),
            None,
        ),
    ];

    let mut issued: Vec<(&str, String)> = Vec::new();
    for (case, domain, duration, owner) in cases {
        let answer = get_token(&router, domain, duration, owner).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {answer:?}"
        );
        let token = token_of(&answer);
        assert!(
            !token.is_empty() && !token.starts_with('<'),
            "case {case:?}: expected a non-empty authorizationToken, got {token:?}"
        );

        // Neither domain name nor the account that owns them is recoverable from the
        // token: it is a secret the credential store generated, not a rendering of the
        // request. (Each needle is at least twelve characters, so a generated token
        // carrying one by chance is not a case this assertion can hit.)
        for (what, needle) in [
            ("the domain it was issued for", domains[0]),
            ("the other domain", domains[1]),
            ("the owning account", ACCOUNT),
        ] {
            assert!(
                !token.contains(needle),
                "case {case:?}: the token must not carry {what} ({needle:?}), got {token:?}"
            );
        }

        for (earlier_case, earlier) in &issued {
            assert!(
                *earlier != token,
                "case {case:?}: expected a token {earlier_case:?} was not given, got {token:?}"
            );
        }
        issued.push((case, token));
    }
}

/// RULE 3: issuance follows the domains the service holds at the moment of the call.
#[tokio::test]
async fn issuance_follows_the_domains_the_service_holds_right_now() {
    let router = emulator();
    let domain = "lifecycle";
    let neighbour = "neighbour";
    assert_eq!(
        create_domain(&router, neighbour, None).await.status,
        200,
        "CreateDomain {neighbour:?} must succeed"
    );

    // Before the domain exists, the same request its held self will serve is refused —
    // and holding its neighbour grants nothing for it.
    let before = get_token(&router, domain, None, None).await;
    assert_eq!(
        before.status, 404,
        "case before CreateDomain: expected 404, got {before:?}"
    );
    assert_eq!(
        before.code(),
        "ResourceNotFoundException",
        "case before CreateDomain: expected ResourceNotFoundException, got {before:?}"
    );

    assert_eq!(
        get_token(&router, neighbour, None, None).await.status,
        200,
        "the neighbour domain must serve a token of its own"
    );

    // Created: the identical request now serves.
    assert_eq!(create_domain(&router, domain, None).await.status, 200);
    let now = now_seconds();
    let during = get_token(&router, domain, None, None).await;
    assert_eq!(
        during.status, 200,
        "case while held: expected 200, got {during:?}"
    );
    let expiration = expiration_of(&during);
    let lower = now + DEFAULT_PERIOD_SECONDS - CLOCK_SLACK_SECONDS;
    let upper = now + DEFAULT_PERIOD_SECONDS + CLOCK_SLACK_SECONDS;
    assert!(
        expiration >= lower && expiration <= upper,
        "case while held: expected an expiration in {lower:?}..={upper:?}, got {expiration:?}"
    );

    // Gone again: refused once more, while the neighbour is untouched by any of it.
    assert_eq!(
        delete_domain(&router, domain).await.status,
        200,
        "DeleteDomain must succeed"
    );
    let after = get_token(&router, domain, None, None).await;
    assert_eq!(
        after.status, 404,
        "case after DeleteDomain: expected 404, got {after:?}"
    );
    assert_eq!(
        after.code(),
        "ResourceNotFoundException",
        "case after DeleteDomain: expected ResourceNotFoundException, got {after:?}"
    );
    assert!(
        after.body.get("authorizationToken").is_none(),
        "case after DeleteDomain: a refusal must carry no token, got {after:?}"
    );
    assert_eq!(
        get_token(&router, neighbour, None, None).await.status,
        200,
        "the neighbour domain must still serve after its sibling is deleted"
    );
}

/// Now, as epoch seconds.
fn now_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}
