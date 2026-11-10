// SPDX-License-Identifier: CC0-1.0

//! What a report's `AcceptanceType` decides, over HTTP.
//!
//! ONE rule, asserted over the whole published catalog rather than over any single
//! report: **every report downloads only under a term grant this service issued for the
//! term governing it, whatever its `acceptanceType` — that attribute says whether a client
//! has to put the term document in front of a person before asking for the receipt, and it
//! decides nothing about which receipts this service issued.** The rows read the attribute
//! back from the API rather than assuming it, because the claim is about what it does NOT
//! change.
//!
//! The rows differ along what the rule generalises over: the acceptance type, the report,
//! how many versions it publishes, which version is asked for, and the shape of the token
//! presented (a word, a string shaped like one of this service's own tokens, the shortest
//! the member admits, the longest it admits, and a grant minted under another report's
//! term).
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes.

use std::collections::BTreeMap;

use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    emulator, get_report, get_report_metadata, get_term_for_report, list_reports,
    ACCEPTANCE_EXPLICIT, ACCEPTANCE_PASSTHROUGH, CATALOG,
};

/// The fixed text this service answers for a token it did not issue, and the `reason` it
/// reports beside it. Spelled out here rather than imported from the crate so a changed
/// constant fails this suite instead of redefining what it asserts.
const INVALID_TOKEN: &str = "The provided token is invalid.";
/// See [`INVALID_TOKEN`].
const INVALID_TOKEN_REASON: &str = "invalidToken";

/// Tokens no acceptance ever minted, each satisfying `GetReport`'s own `termToken`
/// constraints (`ShortStringAttribute`, `@length(1..256)`, `^[a-zA-Z0-9_\-\s]*$`, as
/// `smithy_query` reports them for this model) so every row reaches the handler.
fn never_issued_tokens() -> Vec<(String, String)> {
    vec![
        ("a word".to_string(), "unaccepted".to_string()),
        (
            "shaped like this service's counter-minted store ids".to_string(),
            "term-0000000000000000".to_string(),
        ),
        (
            "the first value such a counter mints".to_string(),
            "term-0000000000000001".to_string(),
        ),
        ("the shortest admitted".to_string(), "a".to_string()),
        ("the longest admitted".to_string(), "z".repeat(256)),
    ]
}

/// The `acceptanceType` the catalog reports for every report it publishes, read through
/// `ListReports`: the attribute the download decision has to follow.
async fn reported_acceptance_types(router: &MultiServiceRouter) -> BTreeMap<String, String> {
    let listed = list_reports(router, "?maxResults=300").await;
    assert_eq!(
        listed.status, 200,
        "ListReports must serve the catalog, got {} with {:?}",
        listed.status, listed.body
    );
    let mut reported = BTreeMap::new();
    for summary in listed.reports() {
        let id = summary["id"]
            .as_str()
            .unwrap_or_else(|| panic!("every summary reports its id, got {summary:?}"))
            .to_string();
        let acceptance = summary["acceptanceType"]
            .as_str()
            .unwrap_or_else(|| panic!("every summary reports its acceptanceType, got {summary:?}"))
            .to_string();
        reported.insert(id, acceptance);
    }
    assert_eq!(
        reported.len(),
        CATALOG.len(),
        "the catalog must report every published report, got {reported:?}"
    );
    reported
}

#[tokio::test]
async fn no_acceptance_type_waives_the_term_grant_a_download_needs() {
    let router = emulator();
    let reported = reported_acceptance_types(&router).await;

    for expected in CATALOG {
        let acceptance = reported
            .get(expected.id)
            .unwrap_or_else(|| panic!("the catalog must publish {}", expected.id));
        assert_eq!(
            acceptance, expected.acceptance_type,
            "case {:?}: expected acceptanceType {:?}, got {acceptance:?}",
            expected.id, expected.acceptance_type
        );
        // Only the two values the model declares exist, and neither of them waives the
        // receipt the download needs.
        assert!(
            acceptance == ACCEPTANCE_EXPLICIT || acceptance == ACCEPTANCE_PASSTHROUGH,
            "case {:?}: acceptanceType must be one of the modelled values, got {acceptance:?}",
            expected.id
        );

        for version in expected.versions {
            for (shape, token) in never_issued_tokens() {
                let case = format!("{} v{version} {acceptance} with {shape}", expected.id);
                let answer = get_report(
                    &router,
                    &format!(
                        "?reportId={}&reportVersion={version}&termToken={token}",
                        expected.id
                    ),
                )
                .await;
                assert_eq!(
                    answer.status, 400,
                    "case {case:?}: a token no acceptance minted must be refused whatever the \
                     acceptance type, got {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    "ValidationException",
                    "case {case:?}: expected ValidationException, got {:?}",
                    answer.body
                );
                assert_eq!(
                    answer.body["message"].as_str(),
                    Some(INVALID_TOKEN),
                    "case {case:?}: expected the fixed invalid-token text, got {:?}",
                    answer.body
                );
                assert_eq!(
                    answer.body["reason"].as_str(),
                    Some(INVALID_TOKEN_REASON),
                    "case {case:?}: expected the invalidToken reason, got {:?}",
                    answer.body
                );
                assert!(
                    answer.body["documentPresignedUrl"].is_null(),
                    "case {case:?}: a refused request must carry no document URL, got {:?}",
                    answer.body
                );
            }
        }
    }
}

#[tokio::test]
async fn accepting_a_term_serves_every_report_whatever_its_acceptance_type() {
    let router = emulator();
    let reported = reported_acceptance_types(&router).await;

    // The acceptance type decides whether a grant is REQUIRED, never whether one works:
    // a grant minted for a report downloads it under either value, so the rows are the
    // whole catalog.
    for expected in CATALOG {
        let version = expected.latest();
        let case = format!(
            "{} v{version} {}",
            expected.id,
            reported
                .get(expected.id)
                .map(String::as_str)
                .unwrap_or_default()
        );
        let term = get_term_for_report(
            &router,
            &format!("?reportId={}&reportVersion={version}", expected.id),
        )
        .await;
        assert_eq!(
            term.status, 200,
            "case {case:?}: expected HTTP 200 from GetTermForReport, got {} with {:?}",
            term.status, term.body
        );
        let token = term.body["termToken"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let download = get_report(
            &router,
            &format!(
                "?reportId={}&reportVersion={version}&termToken={token}",
                expected.id
            ),
        )
        .await;
        assert_eq!(
            download.status, 200,
            "case {case:?}: a grant minted for this report must download it, got {} with {:?}",
            download.status, download.body
        );

        // And the attribute the decision followed is the one the single-report read
        // reports, so the two cannot disagree about what this report requires.
        let metadata =
            get_report_metadata(&router, &format!("?reportId={}", expected.id)).await;
        assert_eq!(
            metadata.body["reportDetails"]["acceptanceType"].as_str(),
            Some(expected.acceptance_type),
            "case {case:?}: expected acceptanceType {:?}, got {:?}",
            expected.acceptance_type,
            metadata.body
        );
    }
}

#[tokio::test]
async fn a_grant_minted_under_another_term_authorises_nothing_it_does_not_cover() {
    let router = emulator();
    let source = CATALOG
        .first()
        .expect("the catalog publishes a report to accept");

    // One acceptance, of ONE report's term: a grant that covers that term and no other.
    let term = get_term_for_report(&router, &format!("?reportId={}", source.id)).await;
    assert_eq!(
        term.status, 200,
        "expected HTTP 200 from GetTermForReport, got {} with {:?}",
        term.status, term.body
    );
    let token = term.body["termToken"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Presented on every OTHER report, it is a token like any other the service did not
    // mint for them: refused, whatever that report's acceptance type says, because the
    // receipt is the acceptance of ONE term document.
    for expected in CATALOG.iter().filter(|row| row.id != source.id) {
        let case = format!(
            "{} {} under the grant for {}",
            expected.id, expected.acceptance_type, source.id
        );
        let answer = get_report(
            &router,
            &format!("?reportId={}&termToken={token}", expected.id),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a grant under another term must not authorise it, got {} \
             with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["message"].as_str(),
            Some(INVALID_TOKEN),
            "case {case:?}: expected the fixed invalid-token text, got {:?}",
            answer.body
        );
        assert!(
            answer.body["documentPresignedUrl"].is_null(),
            "case {case:?}: a refused request must carry no document URL, got {:?}",
            answer.body
        );
    }
}
