// SPDX-License-Identifier: CC0-1.0

//! How the artifact report read path PROJECTS a stored report onto the wire.
//!
//! The workflow suite next door asserts that the catalog is persistent and that the
//! term/download sequence holds. This one asserts the shape of what comes back, over the
//! whole catalog rather than one row:
//!
//! * RULE T — every projected member whose Smithy target carries
//!   `@timestampFormat("date-time")` serializes as an ISO-8601 date-time STRING, on every
//!   operation that projects it. (`smithy_query(query="shape",shape="ReportSummary")` and
//!   `…shape="ReportDetail"` report `"timestampFormat": "date-time"` on `periodStart`,
//!   `periodEnd`, `createdAt`, `lastModifiedAt` and `deletedAt`, all of which target
//!   `TimestampAttribute`.)
//! * RULE P — a report projection reports the report's own published identity: a catalog
//!   entry carries exactly the published-report members, and the document-upload progress
//!   members belong to the single-report metadata read alone.
//! * RULE A — a report ARN names the region the request was signed for, an empty account
//!   id, the report and the VERSION the projection was taken from.
//! * RULE K — a `nextToken` is an OPAQUE continuation value: it is none of the resource
//!   identifiers of the collection it pages, handing it back resumes exactly where the
//!   page ended, and it is refused by the operation that did not issue it.

use std::collections::{BTreeMap, BTreeSet};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    emulator, get_report_metadata, get_signed, list_report_versions, list_reports, signed_for,
    Answer, CATALOG, FALLBACK_REGION, REGION, SIGNED_FOR_A_MALFORMED_REGION,
};

/// The members `ReportSummary` projects for a published catalog report, exactly.
///
/// Source: `smithy_query(query="shape",shape="ReportSummary")` lists `id`, `name`,
/// `state`, `arn`, `version`, `uploadState`, `description`, `periodStart`, `periodEnd`,
/// `series`, `category`, `companyName`, `productName`, `statusMessage` and
/// `acceptanceType`; `visibility` is reported by the live API and absent from that
/// snapshot. `uploadState` and `statusMessage` describe the ingestion of a report's
/// document, which only the metadata read reports — see [`DETAIL_ONLY_MEMBERS`].
const SUMMARY_MEMBERS: &[&str] = &[
    "acceptanceType",
    "arn",
    "category",
    "companyName",
    "description",
    "id",
    "name",
    "periodEnd",
    "periodStart",
    "productName",
    "series",
    "state",
    "version",
    "visibility",
];

/// The members only `ReportDetail` projects — the document-upload progress pair.
const DETAIL_ONLY_MEMBERS: &[&str] = &["uploadState", "statusMessage"];

/// Every `ReportSummary` member whose target carries `@timestampFormat("date-time")`.
const SUMMARY_DATE_TIME_MEMBERS: &[&str] = &["periodStart", "periodEnd"];

/// Every `ReportDetail` member whose target carries `@timestampFormat("date-time")` and
/// that a report in the catalog carries a value for. `deletedAt` is the fifth such
/// member; a catalog report is not deleted, so it is asserted ABSENT instead.
const DETAIL_DATE_TIME_MEMBERS: &[&str] = &[
    "periodStart",
    "periodEnd",
    "createdAt",
    "lastModifiedAt",
];

/// The visibility a report AWS publishes in the Artifact catalog carries.
const PUBLIC: &str = "PUBLIC";

/// Assert `value` is an ISO-8601 date-time STRING: a string the `date-time` format parses,
/// and one that is reproduced by re-formatting the instant it denotes (so a number, an
/// epoch-seconds string, or a truncated rendering all fail).
fn assert_date_time(case: &str, member: &str, value: &Value) {
    let Some(text) = value.as_str() else {
        panic!(
            "case {case:?}: {member} is @timestampFormat date-time, so it must serialize as \
             an ISO-8601 string, got {value:?}"
        );
    };
    let parsed = DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: {member} must parse as an ISO-8601 date-time ({error}), got {text:?}")
    });
    let reformatted = parsed
        .fmt(Format::DateTime)
        .expect("a parsed instant re-formats");
    assert_eq!(
        reformatted, text,
        "case {case:?}: {member} must round-trip through the date-time format, got {text:?}"
    );
    assert!(
        text.ends_with('Z') && text.contains('T'),
        "case {case:?}: {member} must be a UTC ISO-8601 date-time, got {text:?}"
    );
}

/// The member names an object carries, as a set.
fn members(object: &Value) -> BTreeSet<String> {
    object
        .as_object()
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default()
}

/// Every summary `ListReports` and `ListReportVersions` project, keyed by the case they
/// came from, so one table can span both operations.
async fn every_projected_summary(router: &MultiServiceRouter) -> BTreeMap<String, Value> {
    let mut collected = BTreeMap::new();

    let listed = list_reports(router, "").await;
    assert_eq!(
        listed.status, 200,
        "expected HTTP 200 from ListReports, got {} with {:?}",
        listed.status, listed.body
    );
    for summary in listed.reports() {
        let id = summary["id"].as_str().unwrap_or_default();
        collected.insert(format!("ListReports {id}"), summary.clone());
    }

    for expected in CATALOG {
        let answer = list_report_versions(router, &format!("?reportId={}", expected.id)).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: expected HTTP 200 from ListReportVersions, got {} with {:?}",
            expected.id, answer.status, answer.body
        );
        for summary in answer.reports() {
            let version = summary["version"].as_i64().unwrap_or_default();
            collected.insert(
                format!("ListReportVersions {} v{version}", expected.id),
                summary.clone(),
            );
        }
    }

    collected
}

#[tokio::test]
async fn every_date_time_member_of_every_projection_is_an_iso_8601_string() {
    let router = emulator();

    // Rows: every (operation, report, version) a summary is projected for — every report
    // in the catalog, of every version count it publishes, over both paginated operations.
    for (case, summary) in every_projected_summary(&router).await {
        for member in SUMMARY_DATE_TIME_MEMBERS {
            assert_date_time(&case, member, &summary[*member]);
        }
    }

    // And the metadata read, for every report AND every one of its versions.
    for expected in CATALOG {
        for version in expected.versions {
            let case = format!("GetReportMetadata {} v{version}", expected.id);
            let answer = get_report_metadata(
                &router,
                &format!("?reportId={}&reportVersion={version}", expected.id),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200, got {} with {:?}",
                answer.status, answer.body
            );
            let detail = &answer.body["reportDetails"];
            for member in DETAIL_DATE_TIME_MEMBERS {
                assert_date_time(&case, member, &detail[*member]);
            }
            assert!(
                detail["deletedAt"].is_null(),
                "case {case:?}: a report in the catalog is not deleted, got {:?}",
                detail["deletedAt"]
            );
        }
    }
}

#[tokio::test]
async fn a_catalog_entry_projects_the_published_report_members_and_nothing_else() {
    let router = emulator();
    let expected_members: BTreeSet<String> =
        SUMMARY_MEMBERS.iter().map(|m| (*m).to_string()).collect();

    for (case, summary) in every_projected_summary(&router).await {
        assert_eq!(
            members(&summary),
            expected_members,
            "case {case:?}: a catalog entry projects exactly the published-report members, \
             got {summary:?}"
        );
        assert_eq!(
            summary["visibility"].as_str(),
            Some(PUBLIC),
            "case {case:?}: a report AWS publishes in the catalog is {PUBLIC}, got {summary:?}"
        );
        for member in DETAIL_ONLY_MEMBERS {
            assert!(
                summary[*member].is_null(),
                "case {case:?}: {member} belongs to the metadata read, not to a listing, \
                 got {summary:?}"
            );
        }
    }

    // The metadata read projects the same published-report members, plus the pair above.
    for expected in CATALOG {
        let case = format!("GetReportMetadata {}", expected.id);
        let answer = get_report_metadata(&router, &format!("?reportId={}", expected.id)).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200 from GetReportMetadata, got {} with {:?}",
            answer.status, answer.body
        );
        let detail = &answer.body["reportDetails"];
        let reported = members(detail);
        for member in SUMMARY_MEMBERS.iter().chain(DETAIL_ONLY_MEMBERS) {
            assert!(
                reported.contains(*member),
                "case {case:?}: the metadata read must report {member}, got {detail:?}"
            );
        }
        assert_eq!(
            detail["visibility"].as_str(),
            Some(PUBLIC),
            "case {case:?}: expected visibility {PUBLIC}, got {detail:?}"
        );
        assert_eq!(
            detail["category"].as_str(),
            Some(expected.category),
            "case {case:?}: expected category {:?}, got {detail:?}",
            expected.category
        );
        assert_eq!(
            detail["series"].as_str(),
            Some(expected.series),
            "case {case:?}: expected series {:?}, got {detail:?}",
            expected.series
        );
    }
}

#[tokio::test]
async fn a_report_arn_names_the_version_the_projection_was_taken_from() {
    let router = emulator();

    // Rows: every version of every report, read back through the operation that projects
    // one version at a time, plus the regions a request can be signed for.
    for expected in CATALOG {
        let answer = list_report_versions(&router, &format!("?reportId={}", expected.id)).await;
        let mut seen = BTreeSet::new();
        for summary in answer.reports() {
            let version = summary["version"].as_i64().unwrap_or_default();
            let case = format!("{} v{version}", expected.id);
            let arn = summary["arn"].as_str().unwrap_or_default();
            assert_eq!(
                arn,
                format!("arn:aws:artifact:{REGION}::report/{}:{version}", expected.id),
                "case {case:?}: expected a service-owned ARN naming that version, got {arn:?}"
            );
            assert!(
                seen.insert(arn.to_string()),
                "case {case:?}: two versions must not share an ARN, got {arn:?}"
            );
        }
        assert_eq!(
            seen.len(),
            expected.versions.len(),
            "case {:?}: one ARN per published version, got {seen:?}",
            expected.id
        );
    }

    // The region comes from the request's credential scope, and an unusable scope falls
    // back rather than letting a header shape the ARN.
    let regions: Vec<(String, String)> = vec![
        (signed_for(REGION), REGION.to_string()),
        (signed_for("us-west-2"), "us-west-2".to_string()),
        (signed_for("ap-southeast-3"), "ap-southeast-3".to_string()),
        (
            SIGNED_FOR_A_MALFORMED_REGION.to_string(),
            FALLBACK_REGION.to_string(),
        ),
    ];
    for (authorization, region) in regions {
        for expected in CATALOG {
            let case = format!("{} in {region}", expected.id);
            let answer = get_signed(
                &router,
                &format!("/v1/report/getMetadata?reportId={}", expected.id),
                &authorization,
            )
            .await;
            assert_eq!(
                answer.body["reportDetails"]["arn"].as_str(),
                Some(
                    format!(
                        "arn:aws:artifact:{region}::report/{}:{}",
                        expected.id,
                        expected.latest()
                    )
                    .as_str()
                ),
                "case {case:?}: expected the ARN to name that region and the latest version, \
                 got {:?}",
                answer.body
            );
        }
    }
}

/// Every value a client could mistake for a pagination token if the service published one:
/// the collection's own identifiers.
fn resource_identifiers() -> Vec<String> {
    let mut values = Vec::new();
    for expected in CATALOG {
        values.push(expected.id.to_string());
        for version in expected.versions {
            values.push(version.to_string());
            values.push(format!("{}:{version}", expected.id));
            values.push(format!(
                "arn:aws:artifact:{REGION}::report/{}:{version}",
                expected.id
            ));
        }
    }
    values
}

/// Assert `token` is opaque: it is none of the collection's identifiers in `identifiers`,
/// and it does not carry one unencoded. The identifier list is built once per test and
/// passed in, rather than rebuilt per token inside the paging loops.
fn assert_opaque(case: &str, token: &str, identifiers: &[String]) {
    assert!(
        !token.is_empty() && token.len() <= 2048,
        "case {case:?}: NextTokenAttribute is @length(1..2048), got {} bytes",
        token.len()
    );
    for identifier in identifiers {
        assert_ne!(
            token, identifier,
            "case {case:?}: a pagination token must not be a resource identifier, got {token:?}"
        );
        // A bare version number is one or two characters, so finding one inside an encoded
        // token says nothing; the substring check is for the identifiers long enough for a
        // match to mean the token carries them literally.
        if identifier.len() < "report-".len() {
            continue;
        }
        assert!(
            !token.contains(identifier.as_str()),
            "case {case:?}: a pagination token must not carry the identifier {identifier:?} \
             unencoded, got {token:?}"
        );
    }
}

#[tokio::test]
async fn a_pagination_token_is_opaque_and_resumes_exactly_where_the_page_ended() {
    let router = emulator();
    let whole_catalog: Vec<Value> = list_reports(&router, "").await.reports().to_vec();
    let identifiers = resource_identifiers();

    // Rows: page sizes that cut each collection in different places, including the floor
    // (1) and the ceiling (300) of `@range(min: 1, max: 300)` and a size larger than the
    // collection.
    for page_size in [1, 2, 3, 5, 7, 300] {
        let mut walked: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let query = match token.as_deref() {
                None => format!("?maxResults={page_size}"),
                Some(value) => format!("?maxResults={page_size}&nextToken={value}"),
            };
            let answer: Answer = list_reports(&router, &query).await;
            assert_eq!(
                answer.status, 200,
                "case ListReports maxResults={page_size}: expected HTTP 200, got {} with {:?}",
                answer.status, answer.body
            );
            walked.extend(answer.reports().iter().cloned());
            match answer.next_token() {
                None => break,
                Some(next) => {
                    assert_opaque(
                        &format!("ListReports maxResults={page_size}"),
                        &next,
                        &identifiers,
                    );
                    token = Some(next);
                }
            }
            assert!(
                walked.len() <= whole_catalog.len(),
                "case ListReports maxResults={page_size}: pagination did not terminate"
            );
        }
        assert_eq!(
            walked, whole_catalog,
            "case ListReports maxResults={page_size}: walking the pages must enumerate the \
             catalog once, in order"
        );
    }

    // Same rule, same rows, on the other paginated operation — whose collection is one
    // report's versions.
    for expected in CATALOG {
        for page_size in [1, 2, 3, 5, 7, 300] {
            let case = format!("ListReportVersions {} maxResults={page_size}", expected.id);
            let mut walked: Vec<i64> = Vec::new();
            let mut token: Option<String> = None;
            loop {
                let query = match token.as_deref() {
                    None => format!("?reportId={}&maxResults={page_size}", expected.id),
                    Some(value) => format!(
                        "?reportId={}&maxResults={page_size}&nextToken={value}",
                        expected.id
                    ),
                };
                let answer = list_report_versions(&router, &query).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: expected HTTP 200, got {} with {:?}",
                    answer.status, answer.body
                );
                walked.extend(
                    answer
                        .reports()
                        .iter()
                        .map(|report| report["version"].as_i64().unwrap_or_default()),
                );
                match answer.next_token() {
                    None => break,
                    Some(next) => {
                        assert_opaque(&case, &next, &identifiers);
                        token = Some(next);
                    }
                }
                assert!(
                    walked.len() <= expected.versions.len(),
                    "case {case:?}: pagination did not terminate"
                );
            }
            assert_eq!(
                walked, expected.versions,
                "case {case:?}: walking the pages must enumerate every version once, in order"
            );
        }
    }
}

#[tokio::test]
async fn a_token_issued_by_one_paginated_operation_is_refused_by_the_other() {
    let router = emulator();

    let reports_token = list_reports(&router, "?maxResults=1")
        .await
        .next_token()
        .expect("a catalog of several reports must page");
    let multi_version = CATALOG
        .iter()
        .find(|row| row.versions.len() > 1)
        .expect("the catalog publishes a report with several versions");
    let versions_token = list_report_versions(
        &router,
        &format!("?reportId={}&maxResults=1", multi_version.id),
    )
    .await
    .next_token()
    .expect("a report with several versions must page");

    // Rows: each operation handed the other's token, and each handed a token issued for a
    // different report.
    let other = CATALOG
        .iter()
        .find(|row| row.id != multi_version.id)
        .expect("the catalog publishes more than one report");
    let cases = vec![
        (
            "ListReports given a ListReportVersions token",
            list_reports(&router, &format!("?nextToken={versions_token}")).await,
        ),
        (
            "ListReportVersions given a ListReports token",
            list_report_versions(
                &router,
                &format!("?reportId={}&nextToken={reports_token}", multi_version.id),
            )
            .await,
        ),
        (
            "ListReportVersions given another report's token",
            list_report_versions(
                &router,
                &format!("?reportId={}&nextToken={versions_token}", other.id),
            )
            .await,
        ),
    ];
    for (case, answer) in cases {
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a token this operation did not issue must be refused, got {} with \
             {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );
    }
}
