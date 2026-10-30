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
        let (status, response) = list(&router, &json!({ "NextToken": token })).await;
        // The empty token is outside `NextToken`'s modelled 1..10000 length, so the
        // boundary refuses it before the handler: what the row pins is that it is
        // refused, not which of the two sentences it carries.
        if token.is_empty() {
            assert_eq!(
                status, 400,
                "case {case:?}: expected the empty token to be refused, got {status} {response}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "InvalidArgsException"),
            "case {case:?}: expected 400 InvalidArgsException for {token:?}, got {status} \
             {response}"
        );
        let stated = error_message(&response);
        let expected = expected_message(&token);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected the message {expected:?}, got {stated:?}"
        );
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
