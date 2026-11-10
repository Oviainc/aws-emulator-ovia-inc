// SPDX-License-Identifier: CC0-1.0

//! What `GetReport` requires of a `termToken`, over HTTP.
//!
//! ONE rule, asserted over a SET of (report, token) pairs rather than any single one: **a
//! report version is served only under a term grant this service issued for the term
//! governing that report; any other value of `termToken` is refused with the modelled 400
//! `ValidationException`, `x-amzn-errortype: ValidationException`, the fixed message `The
//! provided token is invalid.` and the `invalidToken` reason — whatever the report's
//! `acceptanceType` says.**
//!
//! WHY: the member is `@required` and documented as the "Unique download token provided by
//! GetTermForReport API" (`smithy_query` operation `GetReport`), so a value no acceptance
//! of this service ever minted is not a receipt on any report. `AcceptanceType` says
//! whether a client has to put the term document in front of a person before asking for
//! that receipt; it does not say which receipts this service issued.
//!
//! The rows differ along what the rule generalises over: the acceptance type of the report,
//! how many versions it publishes, which version is asked for (named and unnamed), and
//! where the token came from — never issued, issued for another report's term, or issued
//! for this report's own term, in both the shortest and the longest shapes the member
//! admits.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes.

use crate::harness::{
    emulator, follow_announced_url, get_report, get_term_for_report, Answer, ACCEPTANCE_EXPLICIT,
    ACCEPTANCE_PASSTHROUGH, CATALOG,
};

/// The refusal a token this service did not issue is answered with. Spelled out here rather
/// than imported from the crate so a changed constant fails this suite instead of
/// redefining what it asserts.
const INVALID_TOKEN: (u16, &str, &str) = (400, "ValidationException", "The provided token is invalid.");

/// `ValidationExceptionReason`'s value this refusal reports. Source:
/// `smithy_query(query="shape",shape="ValidationExceptionReason")`.
const INVALID_TOKEN_REASON: &str = "invalidToken";

/// Values no acceptance of this service ever minted, each satisfying `termToken`'s own
/// constraints (`ShortStringAttribute`, `@length(1..256)`, `^[a-zA-Z0-9_\-\s]*$`, as
/// `smithy_query` reports them) so every row reaches the handler rather than the boundary
/// validator.
fn never_issued() -> Vec<(&'static str, String)> {
    vec![
        ("a word", "unaccepted".to_string()),
        ("the shortest the member admits", "a".to_string()),
        ("the longest the member admits", "z".repeat(256)),
        (
            "shaped like this service's counter-minted ids",
            "term-0000000000000001".to_string(),
        ),
        (
            "a run of percent-encoded spaces, which the pattern admits",
            "%20%20%20".to_string(),
        ),
    ]
}

/// The term grant one acceptance of `report_id` mints, read off `GetTermForReport`'s own
/// answer.
async fn accept(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    report_id: &str,
    version: Option<i64>,
) -> String {
    let query = match version {
        Some(version) => format!("?reportId={report_id}&reportVersion={version}"),
        None => format!("?reportId={report_id}"),
    };
    let term = get_term_for_report(router, &query).await;
    assert_eq!(
        term.status, 200,
        "case {case:?}: the acceptance must be served, got {} with {:?}",
        term.status, term.body
    );
    term.body["termToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the acceptance carries a token, got {:?}", term.body))
        .to_string()
}

/// Assert `answer` is the invalid-token refusal, and that it announces no document.
fn assert_invalid_token(case: &str, answer: &Answer) {
    let (status, code, message) = INVALID_TOKEN;
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected HTTP {status}, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        code,
        "case {case:?}: expected the modelled {code}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.body["message"].as_str(),
        Some(message),
        "case {case:?}: expected the fixed message {message:?}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.body["reason"].as_str(),
        Some(INVALID_TOKEN_REASON),
        "case {case:?}: expected reason {INVALID_TOKEN_REASON}, got {:?}",
        answer.body
    );
    assert!(
        answer.body["documentPresignedUrl"].is_null(),
        "case {case:?}: a refused download must announce no URL, got {:?}",
        answer.body
    );
}

#[tokio::test]
async fn a_term_token_no_acceptance_minted_is_refused_on_every_report() {
    let router = emulator();

    // The rows are the whole published catalog crossed with the token shapes above: both
    // acceptance types, reports publishing one version and reports publishing several, and
    // the first and the newest version of each.
    let mut explicit = 0usize;
    let mut passthrough = 0usize;
    for expected in CATALOG {
        match expected.acceptance_type {
            ACCEPTANCE_EXPLICIT => explicit += 1,
            ACCEPTANCE_PASSTHROUGH => passthrough += 1,
            other => panic!("{}: unmodelled acceptanceType {other:?}", expected.id),
        }
        let versions = [
            expected.versions.first().copied(),
            Some(expected.latest()),
            // A request naming no version resolves to the newest published one, and the
            // receipt rule applies to it just the same.
            None,
        ];
        for version in versions {
            for (shape, token) in never_issued() {
                let case = format!(
                    "{} {} version={version:?} with {shape}",
                    expected.id, expected.acceptance_type
                );
                let query = match version {
                    Some(version) => format!(
                        "?reportId={}&reportVersion={version}&termToken={token}",
                        expected.id
                    ),
                    None => format!("?reportId={}&termToken={token}", expected.id),
                };
                assert_invalid_token(&case, &get_report(&router, &query).await);
            }
        }
    }
    // The table spans both values of the attribute the old rule keyed on, so this suite
    // cannot pass by covering only one of them.
    assert!(
        explicit > 0 && passthrough > 0,
        "the catalog must publish reports of both acceptance types, got {explicit} explicit \
         and {passthrough} passthrough"
    );
}

#[tokio::test]
async fn a_grant_minted_for_another_report_is_no_receipt_for_this_one() {
    let router = emulator();

    // One acceptance per report, presented on the NEXT report in the catalog: a receipt is
    // the acceptance of ONE term document, so it authorises nothing else, whatever either
    // report's acceptance type is.
    for (position, expected) in CATALOG.iter().enumerate() {
        let other = &CATALOG[(position + 1) % CATALOG.len()];
        if other.id == expected.id {
            continue;
        }
        let case = format!(
            "{} {} under the grant for {} {}",
            other.id, other.acceptance_type, expected.id, expected.acceptance_type
        );
        let token = accept(&router, &case, expected.id, None).await;
        assert_invalid_token(
            &case,
            &get_report(&router, &format!("?reportId={}&termToken={token}", other.id)).await,
        );
    }
}

#[tokio::test]
async fn the_receipt_its_own_acceptance_minted_serves_every_version_of_that_report() {
    let router = emulator();

    // The positive control for the rule, over the same catalog: once the term is accepted,
    // the receipt downloads every version the report publishes — one term document governs
    // them all — and the URL it announces resolves, which is what announcing one is for.
    for expected in CATALOG {
        let accepted_through = expected.versions.first().copied();
        let case = format!("{} {}", expected.id, expected.acceptance_type);
        let token = accept(&router, &case, expected.id, accepted_through).await;

        for version in expected.versions {
            let case = format!("{case} v{version}");
            let served = get_report(
                &router,
                &format!(
                    "?reportId={}&reportVersion={version}&termToken={token}",
                    expected.id
                ),
            )
            .await;
            assert_eq!(
                served.status, 200,
                "case {case:?}: the receipt its own acceptance minted must serve the report, \
                 got {} with {:?}",
                served.status, served.body
            );
            let url = served.body["documentPresignedUrl"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a URL, got {:?}", served.body))
                .to_string();
            let document = follow_announced_url(&router, &case, &url).await;
            assert_eq!(
                document.status, 200,
                "case {case:?}: the announced URL must serve its document, got {}",
                document.status
            );
            assert!(
                document.bytes.starts_with(b"%PDF-"),
                "case {case:?}: expected the stored report document, got {:?}",
                String::from_utf8_lossy(&document.bytes[..document.bytes.len().min(16)])
            );
        }
    }
}
