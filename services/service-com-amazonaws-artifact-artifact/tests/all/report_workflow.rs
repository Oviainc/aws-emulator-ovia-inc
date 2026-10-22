// SPDX-License-Identifier: CC0-1.0

//! The AWS Artifact report-retrieval workflow, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test are the ones AWS Artifact's report surface states:
//!
//! * the report catalog is SERVICE-owned and persistent, so every read of it answers from
//!   the same records — two calls report the same ids, arns, versions and periods, and
//!   the projection one operation returns agrees with the projection another returns for
//!   the same report version;
//! * a paginated report operation cuts a page at `maxResults`, hands back a `nextToken`
//!   exactly when items remain, and resumes where the previous page ended — so walking
//!   the pages at ANY admissible page size enumerates the whole collection once;
//! * a report document is served only under a term grant this service issued for that
//!   same report version, so a token nobody was given, or one accepted for another
//!   version, yields a modelled failure rather than a URL.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;
use service_com_amazonaws_artifact_artifact::state::TERM_GRANT_CAPACITY;
use service_com_amazonaws_artifact_artifact::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
/// The bounds `MaxResultsAttribute` declares (`@range(min: 1, max: 300)`), spelled out
/// here rather than imported from the crate so a wrong bound in `src/state.rs` fails.
const MAX_RESULTS_FLOOR: i32 = 1;
/// See [`MAX_RESULTS_FLOOR`].
const MAX_RESULTS_CEILING: i32 = 300;

/// A SigV4 credential scope naming this service. It is what routes a request to the
/// artifact service, and its region is what a report ARN is built for.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/artifact/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The region [`SIGNED_FOR_EU_WEST_1`] signs for.
const REGION: &str = "eu-west-1";

/// A credential scope whose region field does not have an AWS region's shape. It still
/// names the `artifact` signing name, so the request routes and the handler runs — and
/// has to choose the region a report ARN names without letting a header shape it.
const SIGNED_FOR_A_MALFORMED_REGION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/not_a_region/artifact/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The region a report ARN names when the request's scope offers none usable.
const FALLBACK_REGION: &str = "us-east-1";

/// A credential scope for `region`, in the shape SigV4 declares.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/artifact/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// One row of the service-owned catalog this emulator publishes.
struct Expected {
    id: &'static str,
    name: &'static str,
    series: &'static str,
    /// One of `AcceptanceType`'s values (`EXPLICIT` | `PASSTHROUGH`).
    acceptance_type: &'static str,
    /// Every published version of this report, ascending.
    versions: &'static [i64],
}

/// The catalog the emulator serves, in the ascending report-id order `ListReports` uses.
///
/// AWS Artifact has no `CreateReport`: the catalog is service-owned, so these rows are
/// the emulator's published set. They are spelled out here, independently of the seed in
/// `src/state.rs`, so a changed or fabricated catalog fails this suite rather than
/// quietly redefining what it asserts. The rows differ along everything the rules below
/// generalise over: report series, `AcceptanceType`, and how many versions a report has
/// (one, two and three).
const CATALOG: &[Expected] = &[
    Expected {
        id: "report-cloudsecalliance",
        name: "Cloud Security Alliance CAIQ",
        series: "CSA",
        acceptance_type: "PASSTHROUGH",
        versions: &[1],
    },
    Expected {
        id: "report-iso270012022cert",
        name: "ISO 27001 2022 Certification",
        series: "ISO",
        acceptance_type: "PASSTHROUGH",
        versions: &[1],
    },
    Expected {
        id: "report-pcidss4aoc000001",
        name: "PCI DSS v4 AOC",
        series: "PCI",
        acceptance_type: "EXPLICIT",
        versions: &[1, 2],
    },
    Expected {
        id: "report-soc1type2report1",
        name: "SOC 1 Type 2",
        series: "SOC",
        acceptance_type: "EXPLICIT",
        versions: &[1, 2, 3],
    },
    Expected {
        id: "report-soc2type2report1",
        name: "SOC 2 Type 2",
        series: "SOC",
        acceptance_type: "EXPLICIT",
        versions: &[1, 2],
    },
];

/// `ReportId`'s `^report-[a-zA-Z0-9]{16}$`, checked here rather than through the crate's
/// own predicate so a wrong predicate cannot make this test pass.
fn matches_report_id_pattern(value: &str) -> bool {
    let Some(suffix) = value.strip_prefix("report-") else {
        return false;
    };
    suffix.len() == 16 && suffix.chars().all(|c| c.is_ascii_alphanumeric())
}

/// `GetReport`'s `termToken` member admits `^[a-zA-Z0-9_\-\s]*$` within 1..256 bytes, so
/// a token this service mints has to satisfy it or its own `GetReport` would refuse it.
fn matches_term_token_pattern(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c.is_whitespace())
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The `reports` list a paginated report operation returns.
    fn reports(&self) -> &[Value] {
        self.body["reports"].as_array().map_or(&[], |items| items)
    }

    /// The `nextToken` the answer carries, if any.
    fn next_token(&self) -> Option<String> {
        self.body["nextToken"].as_str().map(str::to_string)
    }
}

async fn get(router: &MultiServiceRouter, uri: &str) -> Answer {
    get_signed(router, uri, SIGNED_FOR_EU_WEST_1).await
}

async fn get_signed(router: &MultiServiceRouter, uri: &str, authorization: &str) -> Answer {
    let request = ::http::Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", "artifact.eu-west-1.amazonaws.com")
        .header("authorization", authorization)
        .body(Bytes::new())
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "GET {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

async fn list_reports(router: &MultiServiceRouter, query: &str) -> Answer {
    get(router, &format!("/v1/report/list{query}")).await
}

async fn list_report_versions(router: &MultiServiceRouter, query: &str) -> Answer {
    get(router, &format!("/v1/report/listVersions{query}")).await
}

async fn get_report_metadata(router: &MultiServiceRouter, query: &str) -> Answer {
    get(router, &format!("/v1/report/getMetadata{query}")).await
}

async fn get_term_for_report(router: &MultiServiceRouter, query: &str) -> Answer {
    get(router, &format!("/v1/report/getTermForReport{query}")).await
}

async fn get_report(router: &MultiServiceRouter, query: &str) -> Answer {
    get(router, &format!("/v1/report/get{query}")).await
}

/// Walk every page of `ListReports` at `page_size`, returning the concatenated summaries.
///
/// Asserts the token rule as it goes: a page is never longer than the requested size,
/// and a `nextToken` only ever arrives with a full page.
async fn walk_reports(router: &MultiServiceRouter, page_size: i32) -> Vec<Value> {
    let mut collected = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..=CATALOG.len() {
        let query = match token.as_deref() {
            None => format!("?maxResults={page_size}"),
            Some(value) => format!("?maxResults={page_size}&nextToken={value}"),
        };
        let answer = list_reports(router, &query).await;
        assert_eq!(
            answer.status, 200,
            "case maxResults={page_size} page {page}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        let items = answer.reports();
        assert!(
            items.len() <= usize::try_from(page_size).expect("a positive page size"),
            "case maxResults={page_size} page {page}: a page must hold at most {page_size} \
             reports, got {}",
            items.len()
        );
        collected.extend(items.iter().cloned());
        match answer.next_token() {
            None => return collected,
            Some(next) => {
                assert_eq!(
                    items.len(),
                    usize::try_from(page_size).expect("a positive page size"),
                    "case maxResults={page_size} page {page}: a nextToken must only arrive \
                     with a full page, got {} items",
                    items.len()
                );
                token = Some(next);
            }
        }
    }
    panic!("case maxResults={page_size}: pagination did not terminate over {} reports", CATALOG.len());
}

/// The instant a `TimestampAttribute` member names, in seconds since the Unix epoch.
///
/// The member is `@timestampFormat("date-time")`, so the wire form is an RFC 3339 STRING:
/// anything else (a JSON number of epoch seconds, say) fails the case here rather than
/// being quietly compared.
fn instant(case: &str, member: &str, value: &Value) -> i64 {
    let text = value.as_str().unwrap_or_else(|| {
        panic!("case {case:?}: {member} must be an RFC 3339 date-time string, got {value:?}")
    });
    ::aws_smithy_types::DateTime::from_str(text, ::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|error| {
            panic!("case {case:?}: {member} must parse as a date-time ({error}), got {text:?}")
        })
        .secs()
}

#[tokio::test]
async fn list_reports_serves_the_persisted_catalog_identically_on_every_call() {
    let router = emulator();

    let first = list_reports(&router, "").await;
    assert_eq!(
        first.status, 200,
        "expected HTTP 200 from ListReports, got {} with {:?}",
        first.status, first.body
    );
    let second = list_reports(&router, "").await;
    assert_eq!(
        first.reports(),
        second.reports(),
        "two successive ListReports calls must report the same records; first {:?}, second {:?}",
        first.body,
        second.body
    );

    let items = first.reports();
    assert_eq!(
        items.len(),
        CATALOG.len(),
        "ListReports must report every catalog entry, got {:?}",
        first.body
    );
    assert_eq!(
        first.next_token(),
        None,
        "a page holding every report must carry no nextToken, got {:?}",
        first.body
    );

    for (expected, actual) in CATALOG.iter().zip(items) {
        let case = expected.id;
        assert_eq!(
            actual["id"].as_str(),
            Some(expected.id),
            "case {case:?}: expected id {:?}, got {:?}",
            expected.id,
            actual
        );
        assert!(
            matches_report_id_pattern(actual["id"].as_str().unwrap_or_default()),
            "case {case:?}: id must match ^report-[a-zA-Z0-9]{{16}}$, got {:?}",
            actual["id"]
        );
        assert_eq!(
            actual["name"].as_str(),
            Some(expected.name),
            "case {case:?}: expected name {:?}, got {:?}",
            expected.name,
            actual
        );
        assert_eq!(
            actual["series"].as_str(),
            Some(expected.series),
            "case {case:?}: expected series {:?}, got {:?}",
            expected.series,
            actual
        );
        assert_eq!(
            actual["acceptanceType"].as_str(),
            Some(expected.acceptance_type),
            "case {case:?}: expected acceptanceType {:?}, got {:?}",
            expected.acceptance_type,
            actual
        );
        // A report ARN is service-owned: the region the request was signed for, and an
        // empty account id.
        assert_eq!(
            actual["arn"].as_str(),
            Some(format!("arn:aws:artifact:{REGION}::report/{}", expected.id).as_str()),
            "case {case:?}: expected a service-owned ARN for region {REGION}, got {:?}",
            actual
        );
        // ListReports reports one entry per report, at its LATEST published version.
        let latest = expected
            .versions
            .iter()
            .copied()
            .next_back()
            .expect("every catalog row publishes a version");
        assert_eq!(
            actual["version"].as_i64(),
            Some(latest),
            "case {case:?}: expected the latest version {latest}, got {:?}",
            actual
        );
        assert_eq!(
            actual["state"].as_str(),
            Some("PUBLISHED"),
            "case {case:?}: a catalog entry must be PUBLISHED, got {:?}",
            actual
        );
        assert_eq!(
            actual["uploadState"].as_str(),
            Some("COMPLETE"),
            "case {case:?}: a catalog entry's document upload must be COMPLETE, got {:?}",
            actual
        );
        for member in ["description", "category", "companyName", "productName"] {
            assert!(
                actual[member].is_string(),
                "case {case:?}: {member} must be reported, got {:?}",
                actual
            );
        }
        // The reporting period is the stored window, and it is a window: start strictly
        // before end. Both members target `TimestampAttribute`
        // (`@timestampFormat("date-time")`), so they arrive as RFC 3339 STRINGS and are
        // compared as the instants they name.
        let (start, end) = (
            instant(case, "periodStart", &actual["periodStart"]),
            instant(case, "periodEnd", &actual["periodEnd"]),
        );
        assert!(
            start > 0 && start < end,
            "case {case:?}: expected a reporting period with start before end, got {:?}",
            actual
        );
    }
}

#[tokio::test]
async fn list_reports_pages_at_every_admissible_page_size() {
    let router = emulator();
    let whole = list_reports(&router, "").await;
    let expected = whole.reports().to_vec();

    // The rule generalises over page size, so the rows are page sizes: every size that
    // cuts the catalog into more than one page, a size that holds it exactly, one past
    // it, and both published bounds of `@range(min: 1, max: 300)`.
    let sizes = [
        MAX_RESULTS_FLOOR,
        2,
        3,
        4,
        i32::try_from(CATALOG.len()).expect("a small catalog"),
        i32::try_from(CATALOG.len() + 1).expect("a small catalog"),
        MAX_RESULTS_CEILING,
    ];
    for page_size in sizes {
        let walked = walk_reports(&router, page_size).await;
        assert_eq!(
            walked, expected,
            "case maxResults={page_size}: walking the pages must enumerate the catalog once, \
             in order; got {walked:?}"
        );
        let ids: BTreeSet<&str> = walked
            .iter()
            .map(|report| report["id"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            ids.len(),
            walked.len(),
            "case maxResults={page_size}: no report id may repeat across pages, got {walked:?}"
        );
    }
}

#[tokio::test]
async fn max_results_outside_its_modelled_range_is_refused() {
    let router = emulator();
    // One step past each published bound of `@range(min: 1, max: 300)`.
    for page_size in [MAX_RESULTS_FLOOR - 1, MAX_RESULTS_CEILING + 1] {
        for (operation, answer) in [
            (
                "ListReports",
                list_reports(&router, &format!("?maxResults={page_size}")).await,
            ),
            (
                "ListReportVersions",
                list_report_versions(
                    &router,
                    &format!("?reportId={}&maxResults={page_size}", CATALOG[0].id),
                )
                .await,
            ),
        ] {
            assert_eq!(
                answer.status, 400,
                "case {operation} maxResults={page_size}: a value outside 1..=300 must be \
                 refused, got {} with {:?}",
                answer.status, answer.body
            );
        }
    }
    // And both bounds themselves are served.
    for page_size in [MAX_RESULTS_FLOOR, MAX_RESULTS_CEILING] {
        let answer = list_reports(&router, &format!("?maxResults={page_size}")).await;
        assert_eq!(
            answer.status, 200,
            "case maxResults={page_size}: a published bound must be served, got {} with {:?}",
            answer.status, answer.body
        );
    }
}

#[tokio::test]
async fn every_listed_report_is_fetchable_by_id_and_by_version() {
    let router = emulator();
    let listed = list_reports(&router, "").await;

    for (expected, summary) in CATALOG.iter().zip(listed.reports()) {
        let case = expected.id;

        // No `reportVersion`: the metadata of the LATEST version, agreeing member for
        // member with what ListReports reported for the same report.
        let latest = get_report_metadata(&router, &format!("?reportId={}", expected.id)).await;
        assert_eq!(
            latest.status, 200,
            "case {case:?}: expected HTTP 200 from GetReportMetadata, got {} with {:?}",
            latest.status, latest.body
        );
        let detail = &latest.body["reportDetails"];
        for member in [
            "id",
            "name",
            "arn",
            "version",
            "state",
            "uploadState",
            "description",
            "series",
            "category",
            "companyName",
            "productName",
            "periodStart",
            "periodEnd",
            "acceptanceType",
        ] {
            assert_eq!(
                detail[member], summary[member],
                "case {case:?}: GetReportMetadata and ListReports must report the same \
                 {member}; got {:?} and {:?}",
                detail[member], summary[member]
            );
        }
        // `ReportDetail` adds members the summary has no room for; they come from the
        // same stored version.
        assert_eq!(
            detail["termArn"].as_str(),
            Some(
                format!(
                    "arn:aws:artifact:{REGION}::term/term-{}-v{}",
                    expected.id.trim_start_matches("report-"),
                    expected.versions.iter().copied().next_back().unwrap_or(1)
                )
                .as_str()
            ),
            "case {case:?}: expected a service-owned term ARN, got {:?}",
            detail
        );
        assert!(
            detail["sequenceNumber"].as_i64().unwrap_or_default() >= 1,
            "case {case:?}: sequenceNumber is @range(min: 1), got {:?}",
            detail
        );
        assert!(
            !detail["createdAt"].is_null() && !detail["lastModifiedAt"].is_null(),
            "case {case:?}: a published version reports when it was created, got {:?}",
            detail
        );
        assert!(
            detail["deletedAt"].is_null(),
            "case {case:?}: a report in the catalog is not deleted, got {:?}",
            detail
        );

        // Each stored version is fetchable by number, and reports that number.
        for version in expected.versions {
            let answer = get_report_metadata(
                &router,
                &format!("?reportId={}&reportVersion={version}", expected.id),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?} v{version}: expected HTTP 200, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.body["reportDetails"]["version"].as_i64(),
                Some(*version),
                "case {case:?} v{version}: expected that version back, got {:?}",
                answer.body
            );
        }

        // One past the newest version does not exist.
        let past = expected.versions.iter().copied().next_back().unwrap_or(0) + 1;
        let missing = get_report_metadata(
            &router,
            &format!("?reportId={}&reportVersion={past}", expected.id),
        )
        .await;
        assert_eq!(
            missing.status, 404,
            "case {case:?} v{past}: an unpublished version must answer 404, got {} with {:?}",
            missing.status, missing.body
        );
        assert_eq!(
            missing.error_code(),
            "ResourceNotFoundException",
            "case {case:?} v{past}: expected ResourceNotFoundException, got {:?}",
            missing.body
        );
    }
}

#[tokio::test]
async fn list_report_versions_returns_every_stored_version_of_one_report() {
    let router = emulator();

    for expected in CATALOG {
        let case = expected.id;
        let answer = list_report_versions(&router, &format!("?reportId={}", expected.id)).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200 from ListReportVersions, got {} with {:?}",
            answer.status, answer.body
        );
        let versions: Vec<i64> = answer
            .reports()
            .iter()
            .map(|report| report["version"].as_i64().unwrap_or_default())
            .collect();
        assert_eq!(
            versions, expected.versions,
            "case {case:?}: expected one summary per stored version in ascending order, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.reports().len(),
            BTreeSet::from_iter(versions.iter()).len(),
            "case {case:?}: every reported version must be distinct, got {:?}",
            answer.body
        );
        for report in answer.reports() {
            assert_eq!(
                report["id"].as_str(),
                Some(expected.id),
                "case {case:?}: every version belongs to the requested report, got {:?}",
                report
            );
        }

        // Paging one version at a time reaches the same set, in the same order.
        let mut walked = Vec::new();
        let mut token: Option<String> = None;
        for page in 0..=expected.versions.len() {
            let query = match token.as_deref() {
                None => format!("?reportId={}&maxResults=1", expected.id),
                Some(value) => {
                    format!("?reportId={}&maxResults=1&nextToken={value}", expected.id)
                }
            };
            let page_answer = list_report_versions(&router, &query).await;
            assert_eq!(
                page_answer.status, 200,
                "case {case:?} page {page}: expected HTTP 200, got {} with {:?}",
                page_answer.status, page_answer.body
            );
            walked.extend(
                page_answer
                    .reports()
                    .iter()
                    .map(|report| report["version"].as_i64().unwrap_or_default()),
            );
            match page_answer.next_token() {
                None => break,
                Some(next) => token = Some(next),
            }
        }
        assert_eq!(
            walked, expected.versions,
            "case {case:?}: walking one version per page must enumerate every version once, \
             got {walked:?}"
        );
    }
}

#[tokio::test]
async fn a_report_downloads_only_under_a_term_grant_issued_for_it() {
    let router = emulator();
    let mut issued: Vec<(String, i64, String)> = Vec::new();

    // Every (report, version) pair in the catalog: accept the term, then download.
    for expected in CATALOG {
        for version in expected.versions {
            let case = format!("{} v{version}", expected.id);
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
            assert!(
                matches_term_token_pattern(&token),
                "case {case:?}: a minted termToken must satisfy GetReport's own \
                 ^[a-zA-Z0-9_\\-\\s]*$ within 1..256, got {token:?}"
            );
            assert!(
                term.body["documentPresignedUrl"]
                    .as_str()
                    .unwrap_or_default()
                    .starts_with("https://"),
                "case {case:?}: expected a term document URL, got {:?}",
                term.body
            );

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
                "case {case:?}: a token issued for this version must yield the document, got \
                 {} with {:?}",
                download.status, download.body
            );
            let url = download.body["documentPresignedUrl"]
                .as_str()
                .unwrap_or_default();
            assert!(
                url.starts_with("https://") && url.contains(expected.id) && url.contains(&token),
                "case {case:?}: the document URL must locate this report version under the \
                 accepted grant, got {url:?}"
            );

            issued.push((expected.id.to_string(), *version, token));
        }
    }

    // A token is minted per call: no two acceptances share one.
    let tokens: BTreeSet<&str> = issued
        .iter()
        .map(|(_, _, token)| token.as_str())
        .collect();
    assert_eq!(
        tokens.len(),
        issued.len(),
        "every GetTermForReport call must mint its own token, got {issued:?}"
    );

    // A token this service never issued is not a token: no URL, a modelled 400.
    for bogus in ["bogus-token", "term-0000000000000000", "never_issued"] {
        let refused = get_report(
            &router,
            &format!("?reportId={}&termToken={bogus}", CATALOG[0].id),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {bogus:?}: an unissued termToken must be refused, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationException",
            "case {bogus:?}: expected ValidationException, got {:?}",
            refused.body
        );
        assert!(
            refused.body["documentPresignedUrl"].is_null(),
            "case {bogus:?}: a refused request must carry no document URL, got {:?}",
            refused.body
        );
    }

    // A token issued for a DIFFERENT report version does not authorise this one.
    for (report_id, version, token) in &issued {
        let Some((other_id, other_version, _)) = issued
            .iter()
            .find(|(id, other, _)| id != report_id || other != version)
        else {
            continue;
        };
        let case = format!("{token} accepted for {report_id} v{version}");
        let refused = get_report(
            &router,
            &format!("?reportId={other_id}&reportVersion={other_version}&termToken={token}"),
        )
        .await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: must not authorise {other_id} v{other_version}, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {:?}",
            refused.body
        );
        assert!(
            refused.body["documentPresignedUrl"].is_null(),
            "case {case:?}: a refused request must carry no document URL, got {:?}",
            refused.body
        );
    }

    // Reading a report does not consume its grant: the first token still downloads.
    let (report_id, version, token) = issued.first().expect("the catalog publishes a version");
    let again = get_report(
        &router,
        &format!("?reportId={report_id}&reportVersion={version}&termToken={token}"),
    )
    .await;
    assert_eq!(
        again.status, 200,
        "a grant is not consumed by a download, got {} with {:?}",
        again.status, again.body
    );
}

#[tokio::test]
async fn a_report_id_naming_nothing_is_unauthorized_on_every_report_operation() {
    let router = emulator();
    // `^report-[a-zA-Z0-9]{16}$` is satisfied, so these reach the handler: the only
    // reason they fail is that the catalog holds no such report — and the catalog IS
    // the account's entitlement, so the answer is the front door's authorization
    // refusal, not the modelled not-found. The whole rule, over both kinds of
    // identifier and both front-door refusals, is pinned in `front_door_errors.rs`.
    let unseeded = ["report-0000000000000000", "report-zzzzzzzzzzzzzzzz"];
    for id in unseeded {
        for (operation, answer) in [
            (
                "GetReportMetadata",
                get_report_metadata(&router, &format!("?reportId={id}")).await,
            ),
            (
                "ListReportVersions",
                list_report_versions(&router, &format!("?reportId={id}")).await,
            ),
            (
                "GetTermForReport",
                get_term_for_report(&router, &format!("?reportId={id}")).await,
            ),
            (
                "GetReport",
                get_report(&router, &format!("?reportId={id}&termToken=anything")).await,
            ),
        ] {
            assert_eq!(
                answer.status, 401,
                "case {operation} {id:?}: an unseeded report must answer 401, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "UnauthorizedException",
                "case {operation} {id:?}: expected UnauthorizedException, got {:?}",
                answer.body
            );
        }
    }

    // A refused GetTermForReport leaves no grant behind, so nothing it "minted" can be
    // spent: the catalog's own report still needs a real acceptance.
    let refused = get_report(
        &router,
        &format!("?reportId={}&termToken=anything", CATALOG[0].id),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "a failed acceptance must leave no usable grant, got {} with {:?}",
        refused.status, refused.body
    );
}

#[tokio::test]
async fn a_malformed_report_id_or_version_is_refused_before_the_catalog_is_consulted() {
    let router = emulator();
    // Each row breaks ONE modelled constraint: the id pattern (prefix, length, alphabet)
    // or `reportVersion`'s `@range(min: 1)`. The two are refused by different layers,
    // which is the row's expectation: a `reportId` that is not a well-formed id names no
    // report to authorize, so the front door refuses it (401 `UnauthorizedException`)
    // before the modelled constraint is reached, while a `reportVersion` is not a
    // resource identifier at all and is refused by the service's own validation (400
    // `ValidationException`).
    let cases = [
        (
            "missing prefix",
            "?reportId=0123456789abcdef".to_string(),
            401,
            "UnauthorizedException",
        ),
        (
            "short suffix",
            "?reportId=report-abc".to_string(),
            401,
            "UnauthorizedException",
        ),
        (
            "long suffix",
            "?reportId=report-0123456789abcdef0".to_string(),
            401,
            "UnauthorizedException",
        ),
        (
            "suffix outside the alphabet",
            "?reportId=report-0123456789abcde_".to_string(),
            401,
            "UnauthorizedException",
        ),
        (
            "version below the range",
            format!("?reportId={}&reportVersion=0", CATALOG[0].id),
            400,
            "ValidationException",
        ),
        (
            "negative version",
            format!("?reportId={}&reportVersion=-1", CATALOG[0].id),
            400,
            "ValidationException",
        ),
    ];
    for (case, query, status, code) in cases {
        for (operation, answer) in [
            ("GetReportMetadata", get_report_metadata(&router, &query).await),
            ("GetTermForReport", get_term_for_report(&router, &query).await),
        ] {
            assert_eq!(
                answer.status, status,
                "case {operation} {case:?}: expected HTTP {status}, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                code,
                "case {operation} {case:?}: expected {code}, got {:?}",
                answer.body
            );
        }
    }
}

#[tokio::test]
async fn a_next_token_this_service_never_issued_is_refused() {
    let router = emulator();
    // A pagination token is opaque, but it is still one of THIS operation's tokens: a
    // ListReports token is a report id, a ListReportVersions token is a version number.
    for token in ["not-a-token", "report-short", "0", "-3", ""] {
        let reports = list_reports(&router, &format!("?nextToken={token}")).await;
        assert_eq!(
            reports.status, 400,
            "case ListReports {token:?}: an unissued nextToken must be refused, got {} with {:?}",
            reports.status, reports.body
        );
        assert_eq!(
            reports.error_code(),
            "ValidationException",
            "case ListReports {token:?}: expected ValidationException, got {:?}",
            reports.body
        );
    }
    for token in ["not-a-number", "0", "-3", "1.5", "", "report-cloudsecalliance:x"] {
        let versions = list_report_versions(
            &router,
            &format!("?reportId={}&nextToken={token}", CATALOG[0].id),
        )
        .await;
        assert_eq!(
            versions.status, 400,
            "case ListReportVersions {token:?}: an unissued nextToken must be refused, got {} \
             with {:?}",
            versions.status, versions.body
        );
    }

    // A ListReportVersions token is bound to the report it was cut for: spending it on
    // another report is not a resume offset, it is a token that was never issued there.
    let multi_version = CATALOG
        .iter()
        .find(|row| row.versions.len() > 1)
        .expect("the catalog publishes a report with several versions");
    let other = CATALOG
        .iter()
        .find(|row| row.id != multi_version.id)
        .expect("the catalog publishes more than one report");
    let first_page = list_report_versions(
        &router,
        &format!("?reportId={}&maxResults=1", multi_version.id),
    )
    .await;
    let issued = first_page
        .next_token()
        .expect("a report with several versions must page");
    let misused = list_report_versions(
        &router,
        &format!("?reportId={}&nextToken={issued}", other.id),
    )
    .await;
    assert_eq!(
        misused.status, 400,
        "a token issued for {} must not resume {}, got {} with {:?}",
        multi_version.id, other.id, misused.status, misused.body
    );
    assert_eq!(
        misused.error_code(),
        "ValidationException",
        "expected ValidationException, got {:?}",
        misused.body
    );
}

#[tokio::test]
async fn a_report_arn_names_the_region_the_request_was_signed_for() {
    let router = emulator();
    // The rule generalises over the signing region, so the rows are regions: the usual
    // two-word form, a three-word one, a region whose number is more than one digit, and
    // a scope whose region field is not a region at all — which must not be allowed to
    // shape the ARN, because the ARN is a value a client parses.
    let cases: Vec<(String, String)> = vec![
        (signed_for(REGION), REGION.to_string()),
        (signed_for("us-west-2"), "us-west-2".to_string()),
        (signed_for("ap-southeast-3"), "ap-southeast-3".to_string()),
        (signed_for("us-gov-east-1"), "us-gov-east-1".to_string()),
        (
            SIGNED_FOR_A_MALFORMED_REGION.to_string(),
            FALLBACK_REGION.to_string(),
        ),
    ];
    for (authorization, expected_region) in cases {
        let answer = get_signed(
            &router,
            &format!("/v1/report/getMetadata?reportId={}", CATALOG[0].id),
            &authorization,
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {expected_region:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["reportDetails"]["arn"].as_str(),
            Some(
                format!(
                    "arn:aws:artifact:{expected_region}::report/{}",
                    CATALOG[0].id
                )
                .as_str()
            ),
            "case {expected_region:?}: expected the ARN to name that region, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn the_term_grant_store_stays_bounded_however_many_terms_are_accepted() {
    let router = emulator();
    let report = CATALOG[0].id;

    // One acceptance past the emulator's own bound on the grant store.
    let mut first = String::new();
    let mut newest = String::new();
    for index in 0..=TERM_GRANT_CAPACITY {
        let term = get_term_for_report(&router, &format!("?reportId={report}")).await;
        assert_eq!(
            term.status, 200,
            "acceptance {index} of {TERM_GRANT_CAPACITY} must be served, got {} with {:?}",
            term.status, term.body
        );
        newest = term.body["termToken"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if index == 0 {
            first = newest.clone();
        }
    }

    // The newest grant still downloads: the bound releases old grants, it does not stop
    // issuing them.
    let served = get_report(&router, &format!("?reportId={report}&termToken={newest}")).await;
    assert_eq!(
        served.status, 200,
        "the newest grant must still download, got {} with {:?}",
        served.status, served.body
    );

    // And the oldest was released, which is what keeps the store bounded.
    let released = get_report(&router, &format!("?reportId={report}&termToken={first}")).await;
    assert_eq!(
        released.status, 400,
        "the oldest grant must have been released once the bound was reached, got {} with {:?}",
        released.status, released.body
    );
    assert!(
        released.body["documentPresignedUrl"].is_null(),
        "a released grant must not download, got {:?}",
        released.body
    );
}
