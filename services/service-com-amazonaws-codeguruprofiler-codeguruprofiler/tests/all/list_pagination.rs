// SPDX-License-Identifier: CC0-1.0

//! The ONE pagination rule this service's listings share, asserted end to end over HTTP
//! through the router this service mounts in production.
//!
//! The rule every case here asserts, for EVERY operation that reports an already-ordered
//! sequence a page at a time: the `nextToken` is this service's own cursor — the position
//! in the listing the next page starts at — so paging from no token to no token visits
//! every element of the listing exactly once and in the listing's own order; a page holds
//! at most `maxResults` elements, and the whole listing when the caller names no
//! `maxResults`; the token is present only while elements remain past the page; a cursor
//! at exactly the end of the listing is one the service issued, so it answers a final
//! empty page; and a cursor the service could not have issued — one that is not a
//! position, or one past the end — is refused with HTTP 400 and the listing's own
//! client-input error code.
//!
//! The rows span the operations that share the rule (`ListProfilingGroups`,
//! `ListFindingsReports` and `GetFindingsReportAccountSummary`, whose listings differ in
//! length and in what they are listing), the page size (every size below the listing
//! length, one equal to it, one far above it, and no page size at all) and the cursor
//! (the first position, a middle one, exactly the end, past the end, and a token that
//! names no position).

use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use service_com_amazonaws_codeguruprofiler_codeguruprofiler::state::{
    ComAmazonawsCodeguruprofilerCodeguruprofilerState, FindingsReport,
};
use smithy_server_runtime::MultiServiceRouter;

/// A signed-looking request header so the router's `can_handle` resolves this service by
/// its SigV4 credential scope, the way an SDK request does. Signature verification is off
/// in this harness, so the signature value is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTEST/20260101/us-east-1/codeguru-profiler/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// Epoch second the seeded analysis windows are laid out from (2026-01-01T00:00:00Z).
const DAY1: i64 = 1_767_225_600;

/// One day, in seconds.
const DAY: i64 = 86_400;

/// The aggregation period the seeded reports analysed, as the model's
/// `AggregationPeriod` enum spells it ("P1D", "PT1H", "PT5M").
const DAILY: &str = "P1D";

/// The groups the fixture creates, in NAME order; they are created in another order, so
/// a listing's own ordering is what the assertions see. One holds analysis, one holds
/// more of it and one holds none.
const ALPHA_GROUP: &str = "alpha-app";
const BETA_GROUP: &str = "beta-app";
const ZETA_GROUP: &str = "zeta-app";

/// The group whose own `ListFindingsReports` these cases page over.
const ANALYSED_GROUP: &str = ALPHA_GROUP;

/// Recorded findings reports, as (group, id, the analysed window's start). A
/// `FindingsReportId` is modelled
/// `[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}`.
const ALPHA_REPORTS: [(&str, i64); 3] = [
    ("aaaaaaaa-0001-4aaa-8aaa-000000000001", DAY1),
    ("aaaaaaaa-0002-4aaa-8aaa-000000000002", DAY1 + DAY),
    ("aaaaaaaa-0003-4aaa-8aaa-000000000003", DAY1 + 2 * DAY),
];
const ZETA_REPORTS: [(&str, i64); 2] = [
    ("cccccccc-0001-4ccc-8ccc-000000000001", DAY1),
    ("cccccccc-0002-4ccc-8ccc-000000000002", DAY1 + DAY),
];

/// A timestamp as the `startTime`/`endTime` query members carry it.
fn iso(secs: i64) -> String {
    DateTime::from_secs(secs)
        .fmt(Format::DateTime)
        .expect("a formattable timestamp")
}

/// One HTTP answer, reduced to what these tests assert on.
struct Answer {
    status: u16,
    body: Vec<u8>,
}

impl Answer {
    fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The modelled error code, which restJson1 carries in `__type`.
    fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .to_string()
    }

    fn message(&self) -> String {
        self.json()
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string()
    }

    fn next_token(&self) -> Option<String> {
        self.json()
            .get("nextToken")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    }
}

/// The listings that share the pagination rule: the operation, the member its page
/// arrives in, and how an element of that page is identified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Listing {
    /// `ListProfilingGroups`, paging over the stored groups in name order.
    ProfilingGroups,
    /// `ListFindingsReports`, paging over one group's recorded analysis.
    FindingsReports,
    /// `GetFindingsReportAccountSummary`, paging over every group's analysis.
    AccountSummary,
}

impl Listing {
    /// Every listing the rule covers.
    const ALL: [Listing; 3] = [
        Listing::ProfilingGroups,
        Listing::FindingsReports,
        Listing::AccountSummary,
    ];

    /// The request URI, with `page` (a `maxResults`/`nextToken` query fragment, possibly
    /// empty) added to whatever query members the operation itself requires.
    fn uri(self, page: &str) -> String {
        match self {
            Listing::ProfilingGroups => format!("/profilingGroups{}", first_query(page)),
            Listing::FindingsReports => format!(
                "/internal/profilingGroups/{ANALYSED_GROUP}/findingsReports?startTime={}&endTime={}{}",
                iso(DAY1 - 1),
                iso(DAY1 + 5 * DAY),
                next_query(page)
            ),
            Listing::AccountSummary => {
                format!("/internal/findingsReports{}", first_query(page))
            }
        }
    }

    /// The elements of a page, identified: a group by its name, a report by its id.
    fn elements(self, answer: &Answer) -> Vec<String> {
        let body = answer.json();
        match self {
            Listing::ProfilingGroups => body
                .get("profilingGroupNames")
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_else(|| {
                    panic!(
                        "no profilingGroupNames in {}",
                        String::from_utf8_lossy(&answer.body)
                    )
                })
                .iter()
                .map(|name| name.as_str().unwrap_or_default().to_string())
                .collect(),
            Listing::FindingsReports => ids_of(answer, "findingsReportSummaries"),
            Listing::AccountSummary => ids_of(answer, "reportSummaries"),
        }
    }

    /// The whole listing, in the order the rule says the pages visit it.
    fn whole(self) -> Vec<String> {
        match self {
            Listing::ProfilingGroups => [ALPHA_GROUP, BETA_GROUP, ZETA_GROUP]
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
            Listing::FindingsReports => ALPHA_REPORTS
                .iter()
                .map(|(id, _)| (*id).to_string())
                .collect(),
            // By group name, then by analysed window: the group with no analysis
            // contributes nothing.
            Listing::AccountSummary => ALPHA_REPORTS
                .iter()
                .chain(ZETA_REPORTS.iter())
                .map(|(id, _)| (*id).to_string())
                .collect(),
        }
    }
}

/// `page` as the FIRST query member of a URI that requires none of its own.
fn first_query(page: &str) -> String {
    if page.is_empty() {
        String::new()
    } else {
        format!("?{page}")
    }
}

/// `page` appended to a URI that already carries query members.
fn next_query(page: &str) -> String {
    if page.is_empty() {
        String::new()
    } else {
        format!("&{page}")
    }
}

/// `<member>[*].id`, in the order the response lists them.
fn ids_of(answer: &Answer, member: &str) -> Vec<String> {
    answer
        .json()
        .get(member)
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "no {member} in {}",
                String::from_utf8_lossy(&answer.body)
            )
        })
        .iter()
        .map(|summary| {
            summary
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The emulator, assembled exactly as the router assembles it: one shared engine behind
/// one mounted service. The engine is kept so the fixture can record analysis into the
/// store — the only way a findings report exists, since the emulator runs no
/// recommendation analysis of its own.
struct Harness {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_codeguruprofiler_codeguruprofiler::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    async fn send(&self, method: &str, uri: &str, body: Vec<u8>) -> Answer {
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "codeguru-profiler.us-east-1.amazonaws.com")
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        Answer {
            status,
            body: response.into_body().to_vec(),
        }
    }

    /// One page of `listing`, asked for with the `page` query fragment.
    async fn page(&self, listing: Listing, page: &str) -> Answer {
        self.send("GET", &listing.uri(page), Vec::new()).await
    }

    /// Create a profiling group the way a client does, so every listing below runs
    /// against groups the service itself stored.
    async fn create_group(&self, name: &str, client_token: &str) {
        let created = self
            .send(
                "POST",
                &format!("/profilingGroups?clientToken={client_token}"),
                serde_json::to_vec(&serde_json::json!({ "profilingGroupName": name }))
                    .expect("a serializable body"),
            )
            .await;
        assert_eq!(
            created.status,
            201,
            "CreateProfilingGroup {name} must answer 201, got {} {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
    }

    /// Record the seeded analysis, through the store's own writer.
    fn record_reports(&self, group: &str, reports: &[(&str, i64)]) {
        let state = self
            .engine
            .state::<ComAmazonawsCodeguruprofilerCodeguruprofilerState>();
        let mut guard = state.lock();
        for (id, start_secs) in reports {
            let recorded = guard.record_findings_report(
                group,
                FindingsReport {
                    id: (*id).to_string(),
                    profile_start_time: DateTime::from_secs(*start_secs),
                    profile_end_time: DateTime::from_secs(start_secs + DAY),
                    total_number_of_findings: 1,
                    aggregation_period: DAILY.to_string(),
                },
            );
            assert!(
                recorded,
                "recording analysis {id:?} must find the group {group:?}"
            );
        }
    }

    /// Three created groups, analysis recorded against two of them.
    async fn seeded() -> Self {
        let harness = Self::new();
        harness.create_group(ZETA_GROUP, "tok-zeta").await;
        harness.create_group(ALPHA_GROUP, "tok-alpha").await;
        harness.create_group(BETA_GROUP, "tok-beta").await;
        harness.record_reports(ALPHA_GROUP, &ALPHA_REPORTS);
        harness.record_reports(ZETA_GROUP, &ZETA_REPORTS);
        harness
    }
}

#[tokio::test]
async fn every_listing_pages_over_its_whole_listing_once() {
    let harness = Harness::seeded().await;

    for listing in Listing::ALL {
        let whole = listing.whole();
        // Page sizes inside the modelled `maxResults` range (1..1000): every size below
        // the listing length, one equal to it, one far above it — and no `maxResults` at
        // all, which the service answers with its own default page size.
        let sizes: Vec<Option<usize>> = (1..=whole.len())
            .map(Some)
            .chain([Some(1000), None])
            .collect();

        for size in sizes {
            let case = format!("{listing:?} with maxResults={size:?}");
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0usize;
            loop {
                let mut query: Vec<String> = Vec::new();
                if let Some(size) = size {
                    query.push(format!("maxResults={size}"));
                }
                if let Some(cursor) = token.as_deref() {
                    query.push(format!("nextToken={cursor}"));
                }
                let answer = harness.page(listing, &query.join("&")).await;
                assert_eq!(
                    answer.status,
                    200,
                    "case {case:?}: expected 200, got {} {}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                let elements = listing.elements(&answer);
                if let Some(size) = size {
                    assert!(
                        elements.len() <= size,
                        "case {case:?}: a page holds at most maxResults elements, got {elements:?}"
                    );
                }
                seen.extend(elements);
                pages += 1;
                assert!(
                    pages <= whole.len() + 1,
                    "case {case:?}: paging did not terminate, saw {seen:?}"
                );
                token = answer.next_token();
                if token.is_none() {
                    break;
                }
            }
            assert_eq!(
                seen, whole,
                "case {case:?}: paging must visit every element exactly once, in the \
                 listing's order, got {seen:?}"
            );
            // How many pages the rule needs: one per `maxResults`-sized slice, and a
            // single page when the caller named no size (every seeded listing is shorter
            // than the service's default page).
            let expected_pages = size.map_or(1, |size| whole.len().div_ceil(size).max(1));
            assert_eq!(
                pages, expected_pages,
                "case {case:?}: expected {expected_pages} page(s), got {pages}"
            );
        }
    }
}

#[tokio::test]
async fn every_listing_reads_a_cursor_the_same_way() {
    let harness = Harness::seeded().await;

    for listing in Listing::ALL {
        let whole = listing.whole();
        let length = whole.len();

        // (label, the cursor, the elements the rule says the page holds)
        let served: Vec<(String, String, Vec<String>)> = vec![
            (
                "the first position".to_string(),
                "0".to_string(),
                whole.clone(),
            ),
            (
                "a position inside the listing".to_string(),
                "1".to_string(),
                whole[1..].to_vec(),
            ),
            (
                "the position one before the end".to_string(),
                (length - 1).to_string(),
                whole[length - 1..].to_vec(),
            ),
            (
                // The page before it ended at the last element, so this is a cursor the
                // service DID issue: it answers a final empty page.
                "exactly the end of the listing".to_string(),
                length.to_string(),
                Vec::new(),
            ),
        ];

        for (label, cursor, expected) in served {
            let case = format!("{listing:?}: {label} (nextToken={cursor})");
            let answer = harness.page(listing, &format!("nextToken={cursor}")).await;
            assert_eq!(
                answer.status,
                200,
                "case {case:?}: expected 200, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            let elements = listing.elements(&answer);
            assert_eq!(
                elements, expected,
                "case {case:?}: wrong page, got {elements:?}"
            );
            assert_eq!(
                answer.next_token(),
                None,
                "case {case:?}: a page that reaches the end carries no nextToken, got {:?}",
                answer.next_token()
            );
        }

        // (label, the cursor, the sentence the refusal states)
        let refused: Vec<(String, String, String)> = vec![
            (
                "one position past the end".to_string(),
                (length + 1).to_string(),
                "nextToken is not a token this service issued".to_string(),
            ),
            (
                "far past the end".to_string(),
                "99".to_string(),
                "nextToken is not a token this service issued".to_string(),
            ),
            (
                "a token that names no position".to_string(),
                "not_a_cursor".to_string(),
                "nextToken is not a token this service issued: not_a_cursor".to_string(),
            ),
            (
                "a token no position could be".to_string(),
                "-1".to_string(),
                "nextToken is not a token this service issued: -1".to_string(),
            ),
        ];

        for (label, cursor, sentence) in refused {
            let case = format!("{listing:?}: {label} (nextToken={cursor})");
            let answer = harness.page(listing, &format!("nextToken={cursor}")).await;
            assert_eq!(
                answer.status,
                400,
                "case {case:?}: expected 400, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: wrong error code, got {}",
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.message(),
                sentence,
                "case {case:?}: wrong refusal sentence, got {:?}",
                answer.message()
            );
        }
    }
}

#[tokio::test]
async fn paging_changes_nothing_a_later_caller_sees() {
    let harness = Harness::seeded().await;

    for listing in Listing::ALL {
        let whole = listing.whole();
        // Page right through the listing one element at a time, twice: a reporting
        // operation consumes no cursor and moves nothing, so the second walk reports
        // exactly what the first did.
        let mut walks: Vec<Vec<String>> = Vec::new();
        for _ in 0..2usize {
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = Some("0".to_string());
            while let Some(cursor) = token {
                let answer = harness
                    .page(listing, &format!("maxResults=1&nextToken={cursor}"))
                    .await;
                assert_eq!(
                    answer.status,
                    200,
                    "case {listing:?} (cursor {cursor}): expected 200, got {} {}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                seen.extend(listing.elements(&answer));
                token = answer.next_token();
            }
            walks.push(seen);
        }
        assert_eq!(
            walks[1], walks[0],
            "case {listing:?}: a repeated walk must report the same elements, got {:?}",
            walks[1]
        );
        assert_eq!(
            walks[0], whole,
            "case {listing:?}: a walk must report the whole listing, got {:?}",
            walks[0]
        );
    }
}
