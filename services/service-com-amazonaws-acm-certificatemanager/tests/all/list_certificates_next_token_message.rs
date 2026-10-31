// SPDX-License-Identifier: CC0-1.0

//! How `ListCertificates` WORDS its refusal of a `NextToken` it did not issue.
//!
//! THE RULE under test: a `NextToken` this operation cannot read as one of its own is
//! refused with the modelled `InvalidArgsException` (HTTP 400) whose message is the
//! fixed text `Invalid Next Token received ` followed by the token the request
//! carried, exactly as it carried it. The refusal therefore names the one value it
//! could not read and nothing else — no position out of the store, and no account's
//! certificate.
//!
//! The rows span the ways a token can fail to be one this operation issued (it hands
//! out the hex of a certificate ARN joined to that certificate's creation instant):
//! not hex at all, hex that is not UTF-8, hex of a value with no position in it, a
//! position no instant can hold, and a token issued by this operation then mutated.
//! Each row's value satisfies `NextToken`'s modelled 1..10000 length and
//! `^[\u0009\u000A\u000D\u0020-\u00FF]*$` pattern
//! (`smithy_query(query="operation",operation="ListCertificates")`), so every one of
//! them reaches the handler's decode rather than the boundary.
//!
//! A token the operation DID issue is in the table too, as the row that must be
//! served: a message assertion over refusals alone could pass against an operation
//! that refused every token.
//!
//! The second table spans the VALUE the refusal has to echo rather than the way the
//! token fails: every character class `NextToken`'s modelled pattern admits (its
//! whitespace characters, spaces and punctuation, and its highest code points) and
//! both published length bounds plus one value past the upper one — so a message that
//! quoted, trimmed, escaped or truncated the token it names would fail some row.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::certificate_lifecycle::{
    call, error_code, error_message, request_certificate, router, ACCOUNT, REGION,
};

/// The sentence ACM refuses an unreadable `ListCertificates` page token with, as a
/// caller composes it from the token it sent.
fn expected_message(token: &str) -> String {
    format!("Invalid Next Token received {token}")
}

/// One `ListCertificates` call.
async fn list(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "ListCertificates", body).await
}

/// Assert that `token` is refused by the operation itself — the modelled
/// `InvalidArgsException` at HTTP 400 — and that the refusal's message is ACM's
/// sentence carrying that very token.
///
/// Both tables below assert exactly this, so they assert it through one helper and
/// report a failure the same way.
async fn assert_refused_with_the_token(router: &MultiServiceRouter, case: &str, token: &str) {
    let (status, response) = list(router, &json!({ "NextToken": token })).await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "InvalidArgsException"),
        "case {case:?}: expected 400 InvalidArgsException for a token of {} character(s), \
         got {status} {response}",
        token.chars().count()
    );
    let stated = error_message(&response);
    let expected = expected_message(token);
    assert_eq!(
        stated, expected,
        "case {case:?}: expected the message {expected:?}, got {stated:?}"
    );
}

#[tokio::test]
async fn an_unreadable_next_token_is_refused_with_the_token_it_carried() {
    let router = router();
    // Three certificates, so a one-item page leaves something beyond it and the
    // operation really issues a token.
    for domain in ["one.example.com", "two.example.com", "three.example.com"] {
        request_certificate(
            &router,
            &json!({ "DomainName": domain, "ValidationMethod": "DNS" }),
        )
        .await;
    }

    let (status, page) = list(&router, &json!({ "MaxItems": 1 })).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected a MaxItems=1 page to succeed, got {status} {page}",
        "the issued token"
    );
    let issued = page["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a page that stopped short must issue a NextToken: {page}"))
        .to_string();

    let certificate_arn = format!("arn:aws:acm:{REGION}:{ACCOUNT}:certificate/abc");
    let rows: Vec<(&str, String)> = vec![
        ("not hex at all", "not-a-valid-next-token".to_string()),
        ("hex that is not UTF-8", "ff".to_string()),
        (
            "hex of a certificate ARN with no position",
            hex::encode(&certificate_arn),
        ),
        (
            "hex of a value that is not a certificate ARN",
            hex::encode("not-an-arn\u{1f}0.000000000"),
        ),
        (
            "a position with out-of-range nanoseconds",
            hex::encode(format!("{certificate_arn}\u{1f}0.1000000000")),
        ),
        (
            "a position whose seconds overflow every instant",
            hex::encode(format!(
                "{certificate_arn}\u{1f}18446744073709551615.000000000"
            )),
        ),
        (
            "a position that is not a number",
            hex::encode(format!("{certificate_arn}\u{1f}later.000000000")),
        ),
        (
            "the issued token with one hex digit dropped",
            issued[..issued.len() - 1].to_string(),
        ),
        ("the empty token", String::new()),
    ];

    for (case, token) in rows {
        // The empty token is outside `NextToken`'s modelled 1..10000 length, so the
        // boundary refuses it before the handler: what the row pins is that it is
        // refused, not which of the two sentences it carries.
        if token.is_empty() {
            let (status, response) = list(&router, &json!({ "NextToken": token })).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the empty token to be refused, got {status} {response}"
            );
            continue;
        }
        assert_refused_with_the_token(&router, case, &token).await;
    }

    // The token this operation issued is read, not refused — so the rule above is
    // about tokens it cannot read, not about every token.
    let (status, resumed) = list(&router, &json!({ "MaxItems": 1, "NextToken": issued })).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected the issued token to resume the walk, got {status} {resumed}",
        "the issued token"
    );
    assert_eq!(
        resumed["CertificateSummaryList"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        1,
        "case {:?}: expected the resumed page to carry one summary, got {resumed}",
        "the issued token"
    );
}

/// The refusal copies the token out of the request as it arrived, whatever the member
/// admits.
///
/// The rows walk `NextToken`'s modelled character set and its published length bounds
/// (`NextToken` @length(min 1, max 10000) @pattern `^[\u0009\u000A\u000D\u0020-\u00FF]*$`,
/// from `smithy_query(query="operation",operation="ListCertificates")`): the three
/// whitespace characters the pattern names, a value with spaces and punctuation, its
/// highest admitted code points, the shortest and the longest admissible token, and one
/// value past the upper bound. None of them is a token this operation issued, so each
/// one the boundary admits must come back with the token itself in the message —
/// unquoted, untrimmed and whole.
#[tokio::test]
async fn the_refusal_echoes_the_token_verbatim_across_the_member_it_accepts() {
    let router = router();
    request_certificate(
        &router,
        &json!({ "DomainName": "echo.example.com", "ValidationMethod": "DNS" }),
    )
    .await;

    /// The modelled maximum length of `NextToken`.
    const MAX_TOKEN_LEN: usize = 10_000;

    let rows: Vec<(&str, String)> = vec![
        ("the shortest admissible token", "z".to_string()),
        (
            "the whitespace characters the pattern admits",
            "stale\u{9}cursor\u{a}from\u{d}a-paginator".to_string(),
        ),
        (
            "spaces and punctuation",
            "page 2 of 7 (cursor: \"stale\")".to_string(),
        ),
        (
            "the highest code points the pattern admits",
            "¡cursor÷caducadoÿ".to_string(),
        ),
        ("the longest admissible token", "z".repeat(MAX_TOKEN_LEN)),
    ];

    for (case, token) in rows {
        assert_refused_with_the_token(&router, case, &token).await;
    }

    // One character past the modelled maximum: outside the member, so the boundary
    // refuses it before the handler reads it. The row pins that it is refused, not
    // which of the two sentences it carries.
    let case = "one character past the longest admissible token";
    let (status, response) = list(
        &router,
        &json!({ "NextToken": "z".repeat(MAX_TOKEN_LEN + 1) }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a token longer than {MAX_TOKEN_LEN} characters to be \
         refused, got {status} {response}"
    );
}
