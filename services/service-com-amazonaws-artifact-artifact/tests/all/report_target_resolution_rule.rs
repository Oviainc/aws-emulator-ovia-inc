// SPDX-License-Identifier: CC0-1.0

//! The ONE rule every report-reading operation resolves its target by, asserted over all
//! four of them at once.
//!
//! A request naming a report (and perhaps a version of it) is resolved in two stages, and
//! which refusal a caller sees depends only on the stage that failed — never on which
//! operation asked:
//!
//! 1. the account's catalog IS its entitlement, so a `reportId` the catalog does not
//!    publish names no report this account may read and is answered with the front door's
//!    401 `UnauthorizedException`, whatever else the request carried;
//! 2. a report that IS published and simply never published the version asked for — or,
//!    when the request named none, published none at all — is the modelled 404
//!    `ResourceNotFoundException`, naming the report as the resource;
//! 3. otherwise the request resolves to the version it named, or to the newest published
//!    one when it named none, and every operation reports that same record.
//!
//! The rows span what the rule generalises over: both `AcceptanceType`s, reports
//! publishing one, two, three and seven versions, a request that names a version and one
//! that names none, and versions just past the newest and far above it. Each refusal is
//! compared ACROSS the operations as well as against its expected shape, so the four
//! handlers cannot start disagreeing about the same `reportId`/`reportVersion` pair.

use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    emulator, get_report, get_report_metadata, get_term_for_report, list_report_versions, Answer,
    CATALOG,
};

/// Ids that satisfy `ReportId`'s `^report-[a-zA-Z0-9]{16}$` (so each one reaches a
/// handler, rather than being refused at the boundary) and that the catalog does not
/// publish. They span the alphabet the pattern admits: digits, lower case, upper case.
const UNSEEDED: &[&str] = &[
    "report-0000000000000000",
    "report-zzzzzzzzzzzzzzzz",
    "report-aaaaAAAA00009999",
];

/// `ResourceNotFoundException$resourceType` for a report, as the service names it.
const REPORT_RESOURCE_TYPE: &str = "Report";

/// A `termToken` nobody was issued, satisfying `GetReport`'s own
/// `^[a-zA-Z0-9_\-\s]*$`: a request whose target does not resolve is refused before any
/// token is looked at, so this value must not change which refusal comes back.
const UNISSUED_TOKEN: &str = "not-a-token";

/// The `&reportVersion=` fragment for a target, empty when the request names no version.
fn version_query(version: Option<i64>) -> String {
    version.map_or_else(String::new, |value| format!("&reportVersion={value}"))
}

/// One term grant for `report_id`, minted through `GetTermForReport` — the receipt a
/// report whose `AcceptanceType` requires the explicit acceptance of its term downloads
/// under. A grant covers the report's TERM, so one token serves every version of it.
async fn term_token(router: &MultiServiceRouter, report_id: &str) -> String {
    let answer = get_term_for_report(router, &format!("?reportId={report_id}")).await;
    assert_eq!(
        answer.status, 200,
        "case {report_id:?}: a published report must issue a term grant, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["termToken"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {report_id:?}: a term grant must carry a token, got {:?}", answer.body)
        })
        .to_string()
}

/// What each operation that resolves ONE report version answers for `report_id` at
/// `version`, keyed by the operation's name.
async fn version_resolving_answers(
    router: &MultiServiceRouter,
    report_id: &str,
    version: Option<i64>,
    term_token: &str,
) -> Vec<(&'static str, Answer)> {
    let query = format!("?reportId={report_id}{}", version_query(version));
    vec![
        (
            "GetReportMetadata",
            get_report_metadata(router, &query).await,
        ),
        (
            "GetTermForReport",
            get_term_for_report(router, &query).await,
        ),
        (
            "GetReport",
            get_report(router, &format!("{query}&termToken={term_token}")).await,
        ),
    ]
}

#[tokio::test]
async fn an_id_the_catalog_does_not_publish_is_the_front_door_refusal_on_every_report_operation() {
    let router = emulator();

    // Stage one is decided before any version is looked at, so the same id is refused the
    // same way whether the request names no version, a version the catalog's reports do
    // publish, or one far above every published version.
    for id in UNSEEDED {
        for version in [None, Some(1), Some(7), Some(9_007_199_254_740_991)] {
            let query = format!("?reportId={id}{}", version_query(version));
            let answers = [
                (
                    "GetReportMetadata",
                    get_report_metadata(&router, &query).await,
                ),
                (
                    "ListReportVersions",
                    list_report_versions(&router, &format!("?reportId={id}")).await,
                ),
                (
                    "GetTermForReport",
                    get_term_for_report(&router, &query).await,
                ),
                (
                    "GetReport",
                    get_report(&router, &format!("{query}&termToken={UNISSUED_TOKEN}")).await,
                ),
            ];
            for (operation, answer) in &answers {
                let case = format!("{operation} {id:?} version={version:?}");
                assert_eq!(
                    answer.status, 401,
                    "case {case:?}: an id the catalog does not publish must answer 401, got \
                     {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    "UnauthorizedException",
                    "case {case:?}: expected UnauthorizedException, got {:?}",
                    answer.body
                );
            }
            // Every operation states the SAME refusal, body included: one rule, not four.
            let (first_operation, first) = &answers[0];
            for (operation, answer) in &answers[1..] {
                assert_eq!(
                    answer.body, first.body,
                    "case {operation} {id:?} version={version:?}: must state the same refusal \
                     as {first_operation}, got {:?} against {:?}",
                    answer.body, first.body
                );
            }
        }
    }
}

#[tokio::test]
async fn a_version_a_published_report_never_had_is_the_modelled_not_found_on_every_operation() {
    let router = emulator();

    for expected in CATALOG {
        let latest = expected.latest();
        // Just past the newest version, far past it, and at the top of the range
        // `VersionAttribute` (`@range(min: 1)`, no ceiling) admits — none of which this
        // report published.
        for version in [latest + 1, latest + 1_000, 9_007_199_254_740_991] {
            let answers = version_resolving_answers(
                &router,
                expected.id,
                Some(version),
                // The target does not resolve, so the download receipt is never reached:
                // an unissued token must not turn this 404 into a token refusal.
                UNISSUED_TOKEN,
            )
            .await;
            for (operation, answer) in &answers {
                let case = format!("{operation} {} v{version}", expected.id);
                assert_eq!(
                    answer.status, 404,
                    "case {case:?}: a version the report never published must answer 404, got \
                     {} with {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    "ResourceNotFoundException",
                    "case {case:?}: expected ResourceNotFoundException, got {:?}",
                    answer.body
                );
                assert_eq!(
                    answer.body["resourceId"].as_str(),
                    Some(expected.id),
                    "case {case:?}: the missing resource is the report, got {:?}",
                    answer.body
                );
                assert_eq!(
                    answer.body["resourceType"].as_str(),
                    Some(REPORT_RESOURCE_TYPE),
                    "case {case:?}: expected the report resource type, got {:?}",
                    answer.body
                );
                let message = answer.body["message"].as_str().unwrap_or_default();
                assert!(
                    message.contains(expected.id) && message.contains(&version.to_string()),
                    "case {case:?}: the message names the report and the version asked for, \
                     got {message:?}"
                );
            }
            let (first_operation, first) = &answers[0];
            for (operation, answer) in &answers[1..] {
                assert_eq!(
                    answer.body, first.body,
                    "case {operation} {} v{version}: must state the same not-found as \
                     {first_operation}, got {:?} against {:?}",
                    expected.id, answer.body, first.body
                );
            }
        }
    }
}

#[tokio::test]
async fn every_published_version_resolves_to_the_same_record_on_every_report_operation() {
    let router = emulator();

    for expected in CATALOG {
        let case = expected.id;
        // Stage one alone, through the operation that resolves no single version: the
        // report is reachable, so its versions are reported.
        let listed = list_report_versions(&router, &format!("?reportId={}", expected.id)).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: a published report must list its versions, got {} with {:?}",
            listed.status, listed.body
        );
        assert_eq!(
            listed.reports().len(),
            expected.versions.len(),
            "case {case:?}: expected one summary per published version, got {:?}",
            listed.body
        );

        // A request naming no version resolves to the newest published one; a request
        // naming a published version resolves to exactly that one.
        let targets = std::iter::once(None).chain(expected.versions.iter().copied().map(Some));
        for target in targets {
            let resolved = target.unwrap_or_else(|| expected.latest());
            let token = term_token(&router, expected.id).await;
            for (operation, answer) in
                version_resolving_answers(&router, expected.id, target, &token).await
            {
                let case = format!("{operation} {} version={target:?}", expected.id);
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: a published version must be served, got {} with {:?}",
                    answer.status, answer.body
                );
                match operation {
                    "GetReportMetadata" => assert_eq!(
                        answer.body["reportDetails"]["version"].as_i64(),
                        Some(resolved),
                        "case {case:?}: expected version {resolved} back, got {:?}",
                        answer.body
                    ),
                    // Both document-serving operations announce a URL naming the version
                    // the request resolved to, so a target that resolved elsewhere fails
                    // here rather than being quietly served.
                    _ => {
                        let url = answer.body["documentPresignedUrl"].as_str().unwrap_or_default();
                        assert!(
                            url.contains(&format!("/{}/v{resolved}/", expected.id)),
                            "case {case:?}: the URL must name the resolved version, got {url:?}"
                        );
                    }
                }
            }
        }
    }
}
