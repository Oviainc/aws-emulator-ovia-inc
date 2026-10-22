// SPDX-License-Identifier: CC0-1.0

//! `ListFindingsReports`, end to end over HTTP through the router this service mounts
//! in production.
//!
//! The rule every case here asserts: a `ListFindingsReports` call reports the findings
//! reports RECORDED against the named stored profiling group whose analysed profile
//! window overlaps the requested `[startTime, endTime]` range — restricted to reports
//! over daily profiles when `dailyReportsOnly` is set — in a stable order, one
//! `maxResults`-sized page at a time; a name no group is stored under is a 404, and
//! nothing about the listing changes what a later caller sees.
//!
//! Rows vary what the rule generalises over: the number of recorded reports, their
//! analysed windows (including windows that only touch a range bound and a range that
//! sits inside a window), their aggregation period, the `dailyReportsOnly` flag, the
//! page size and the cursor. The recorded corpus case — a group with no recorded
//! analysis, which must list nothing — is one row among them.

use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use service_com_amazonaws_codeguruprofiler_codeguruprofiler::state::{
    ComAmazonawsCodeguruprofilerCodeguruprofilerState, FindingsReport,
};
use smithy_server_runtime::MultiServiceRouter;

/// A signed-looking request header so the router's `can_handle` resolves this service
/// by its SigV4 credential scope, the way an SDK request does. Signature verification
/// is off in this harness, so the signature value is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTEST/20260101/us-east-1/codeguru-profiler/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The group the seeded reports belong to.
const ANALYSED_GROUP: &str = "analysed-app";

/// A group created the same way but with no analysis recorded against it — the shape
/// of the recorded corpus unit `list-findings-reports-empty-for-new-group`.
const FRESH_GROUP: &str = "brand-new-group";

/// Epoch second the seeded windows are laid out from (2026-01-01T00:00:00Z).
const DAY1: i64 = 1_767_225_600;

/// One day, in seconds.
const DAY: i64 = 86_400;

/// Aggregation periods, as the model's `AggregationPeriod` enum spells them
/// (`shared_types::AGGREGATION_PERIOD_VALUES` — "P1D", "PT1H", "PT5M"). `P1D` is the
/// daily one `dailyReportsOnly` selects.
const DAILY: &str = "P1D";
const HOURLY: &str = "PT1H";
const FIVE_MINUTES: &str = "PT5M";

/// `startTime`/`endTime` as the model binds them: an ISO 8601 `date-time` query value.
fn iso(secs: i64) -> String {
    DateTime::from_secs(secs)
        .fmt(Format::DateTime)
        .expect("a formattable timestamp")
}

/// The instant a response timestamp carries, as epoch seconds — read from whichever
/// wire form the protocol serializer emits for a `Timestamp` member (an epoch number
/// or an ISO 8601 string), so these cases assert the INSTANT rather than its spelling.
fn timestamp_secs(value: Option<&serde_json::Value>) -> Option<f64> {
    match value? {
        serde_json::Value::Number(number) => number.as_f64(),
        serde_json::Value::String(text) => DateTime::from_str(text, Format::DateTime)
            .ok()
            .map(|parsed| parsed.as_secs_f64()),
        _ => None,
    }
}

/// One recorded findings report seeded into the store.
#[derive(Debug, Clone)]
struct ReportCase {
    /// A `FindingsReportId`: the modelled pattern is
    /// `[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}`.
    id: &'static str,
    start_secs: i64,
    end_secs: i64,
    findings: i32,
    period: &'static str,
}

impl ReportCase {
    fn stored(&self) -> FindingsReport {
        FindingsReport {
            id: self.id.to_string(),
            profile_start_time: DateTime::from_secs(self.start_secs),
            profile_end_time: DateTime::from_secs(self.end_secs),
            total_number_of_findings: self.findings,
            aggregation_period: self.period.to_string(),
        }
    }
}

/// The reports recorded against [`ANALYSED_GROUP`]: windows of three different
/// aggregation periods, one of them a day apart from the others, deliberately NOT in
/// window order so the listing's own ordering is what the assertions see.
fn report_cases() -> Vec<ReportCase> {
    vec![
        ReportCase {
            id: "22222222-2222-4222-8222-222222222222",
            start_secs: DAY1 + 3_600,
            end_secs: DAY1 + 7_200,
            findings: 0,
            period: HOURLY,
        },
        ReportCase {
            id: "11111111-1111-4111-8111-111111111111",
            start_secs: DAY1,
            end_secs: DAY1 + DAY,
            findings: 3,
            period: DAILY,
        },
        ReportCase {
            id: "44444444-4444-4444-8444-444444444444",
            start_secs: DAY1 + 3 * DAY,
            end_secs: DAY1 + 4 * DAY,
            findings: 12,
            period: DAILY,
        },
        ReportCase {
            id: "33333333-3333-4333-8333-333333333333",
            start_secs: DAY1 + 7_200,
            end_secs: DAY1 + 7_500,
            findings: 7,
            period: FIVE_MINUTES,
        },
    ]
}

/// The ids in the order the rule says a listing reports them: by analysed window,
/// oldest first.
const IN_WINDOW_ORDER: [&str; 4] = [
    "11111111-1111-4111-8111-111111111111",
    "22222222-2222-4222-8222-222222222222",
    "33333333-3333-4333-8333-333333333333",
    "44444444-4444-4444-8444-444444444444",
];

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

    /// `findingsReportSummaries[*].id`, in the order the response lists them.
    fn report_ids(&self) -> Vec<String> {
        self.json()
            .get("findingsReportSummaries")
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| {
                panic!(
                    "no findingsReportSummaries in {}",
                    String::from_utf8_lossy(&self.body)
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

    fn next_token(&self) -> Option<String> {
        self.json()
            .get("nextToken")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    }
}

/// The emulator, assembled exactly as the router assembles it: one shared engine
/// behind one mounted service. The engine is kept so a test can record analysis into
/// the store — the only way a findings report exists, since the emulator runs no
/// recommendation analysis of its own — and assert the listing did not change it.
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

    /// Create a profiling group the way a client does, so every listing below runs
    /// against a group the service itself stored.
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

    async fn list(&self, group: &str, query: &str) -> Answer {
        self.send(
            "GET",
            &format!("/internal/profilingGroups/{group}/findingsReports{query}"),
            Vec::new(),
        )
        .await
    }

    /// List `[start, end]`, with the optional `dailyReportsOnly` flag and page
    /// parameters a row asks for.
    async fn list_range(
        &self,
        group: &str,
        start_secs: i64,
        end_secs: i64,
        daily_only: Option<bool>,
        extra: &str,
    ) -> Answer {
        let mut query = format!(
            "?startTime={}&endTime={}",
            iso(start_secs),
            iso(end_secs)
        );
        if let Some(daily) = daily_only {
            query.push_str(&format!("&dailyReportsOnly={daily}"));
        }
        query.push_str(extra);
        self.list(group, &query).await
    }

    fn state(&self) -> engine::ServiceState<ComAmazonawsCodeguruprofilerCodeguruprofilerState> {
        self.engine
            .state::<ComAmazonawsCodeguruprofilerCodeguruprofilerState>()
    }

    /// Record the seeded analysis against [`ANALYSED_GROUP`], through the store's own
    /// writer.
    fn record_reports(&self, cases: &[ReportCase]) {
        let state = self.state();
        let mut guard = state.lock();
        for case in cases {
            assert!(
                guard.record_findings_report(ANALYSED_GROUP, case.stored()),
                "case {case:?}: recording analysis against {ANALYSED_GROUP} must find the group"
            );
        }
    }

    /// How many reports the store holds for one group right now.
    fn stored_report_count(&self, group: &str) -> usize {
        let state = self.state();
        let guard = state.lock();
        guard
            .groups
            .get(group)
            .unwrap_or_else(|| panic!("no stored group named {group}"))
            .findings_reports
            .len()
    }

    /// The whole fixture: two created groups, analysis recorded against one of them.
    async fn seeded() -> Self {
        let harness = Self::new();
        harness.create_group(ANALYSED_GROUP, "tok-analysed").await;
        harness.create_group(FRESH_GROUP, "tok-fresh").await;
        harness.record_reports(&report_cases());
        harness
    }
}

/// One listing request and the ids it must report.
#[derive(Debug)]
struct RangeCase {
    label: &'static str,
    group: &'static str,
    start_secs: i64,
    end_secs: i64,
    daily_only: Option<bool>,
    expected: Vec<&'static str>,
}

/// The table: each row varies the range, the flag or the group, and names the ids the
/// rule says the listing reports.
fn range_cases() -> Vec<RangeCase> {
    vec![
        RangeCase {
            label: "a range around every window lists them all, in window order",
            group: ANALYSED_GROUP,
            start_secs: DAY1 - 1,
            end_secs: DAY1 + 5 * DAY,
            daily_only: None,
            expected: IN_WINDOW_ORDER.to_vec(),
        },
        RangeCase {
            label: "dailyReportsOnly=true keeps only the reports over daily profiles",
            group: ANALYSED_GROUP,
            start_secs: DAY1 - 1,
            end_secs: DAY1 + 5 * DAY,
            daily_only: Some(true),
            expected: vec![
                "11111111-1111-4111-8111-111111111111",
                "44444444-4444-4444-8444-444444444444",
            ],
        },
        RangeCase {
            label: "dailyReportsOnly=false lists the smaller aggregation windows too",
            group: ANALYSED_GROUP,
            start_secs: DAY1 - 1,
            end_secs: DAY1 + 5 * DAY,
            daily_only: Some(false),
            expected: IN_WINDOW_ORDER.to_vec(),
        },
        RangeCase {
            label: "a range starting exactly where a window ends still overlaps it",
            group: ANALYSED_GROUP,
            start_secs: DAY1 + DAY,
            end_secs: DAY1 + 2 * DAY,
            daily_only: None,
            expected: vec!["11111111-1111-4111-8111-111111111111"],
        },
        RangeCase {
            label: "a range ending exactly where a window starts still overlaps it",
            group: ANALYSED_GROUP,
            start_secs: DAY1 + 2 * DAY,
            end_secs: DAY1 + 3 * DAY,
            daily_only: None,
            expected: vec!["44444444-4444-4444-8444-444444444444"],
        },
        RangeCase {
            label: "a range in the gap between two windows lists nothing",
            group: ANALYSED_GROUP,
            start_secs: DAY1 + DAY + 1,
            end_secs: DAY1 + 3 * DAY - 1,
            daily_only: None,
            expected: Vec::new(),
        },
        RangeCase {
            label: "a range INSIDE one window lists that window's report",
            group: ANALYSED_GROUP,
            start_secs: DAY1 + 100,
            end_secs: DAY1 + 200,
            daily_only: None,
            expected: vec!["11111111-1111-4111-8111-111111111111"],
        },
        RangeCase {
            label: "a range inside one window, dailyReportsOnly=true",
            group: ANALYSED_GROUP,
            start_secs: DAY1 + 4_000,
            end_secs: DAY1 + 4_100,
            daily_only: Some(true),
            expected: vec!["11111111-1111-4111-8111-111111111111"],
        },
        RangeCase {
            label: "an instant range (startTime == endTime) lists every window over it",
            group: ANALYSED_GROUP,
            start_secs: DAY1 + 7_300,
            end_secs: DAY1 + 7_300,
            daily_only: None,
            expected: vec![
                "11111111-1111-4111-8111-111111111111",
                "33333333-3333-4333-8333-333333333333",
            ],
        },
        RangeCase {
            label: "a group with no recorded analysis lists nothing",
            group: FRESH_GROUP,
            start_secs: DAY1 - 5 * DAY,
            end_secs: DAY1 + 5 * DAY,
            daily_only: None,
            expected: Vec::new(),
        },
        RangeCase {
            label: "a group with no recorded analysis, dailyReportsOnly=true",
            group: FRESH_GROUP,
            start_secs: DAY1 - 5 * DAY,
            end_secs: DAY1 + 5 * DAY,
            daily_only: Some(true),
            expected: Vec::new(),
        },
    ]
}

#[tokio::test]
async fn a_listing_reports_the_recorded_reports_its_range_overlaps() {
    let harness = Harness::seeded().await;

    for case in range_cases() {
        let label = case.label;
        let answer = harness
            .list_range(
                case.group,
                case.start_secs,
                case.end_secs,
                case.daily_only,
                "",
            )
            .await;
        assert_eq!(
            answer.status,
            200,
            "case {label:?} ({case:?}): expected 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let ids = answer.report_ids();
        assert_eq!(
            ids,
            case.expected,
            "case {label:?} ({case:?}): wrong reports listed, got {ids:?}"
        );
        assert_eq!(
            answer.next_token(),
            None,
            "case {label:?}: a listing that fits one page must carry no nextToken, got {:?}",
            answer.next_token()
        );
    }
}

#[tokio::test]
async fn every_summary_is_built_from_the_stored_report() {
    let harness = Harness::seeded().await;

    let answer = harness
        .list_range(ANALYSED_GROUP, DAY1 - 1, DAY1 + 5 * DAY, None, "")
        .await;
    assert_eq!(
        answer.status,
        200,
        "expected 200, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    let summaries = answer
        .json()
        .get("findingsReportSummaries")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        summaries.len(),
        report_cases().len(),
        "expected every recorded report, got {summaries:?}"
    );

    for case in report_cases() {
        let summary = summaries
            .iter()
            .find(|summary| {
                summary.get("id").and_then(serde_json::Value::as_str) == Some(case.id)
            })
            .unwrap_or_else(|| panic!("case {case:?}: not listed, got {summaries:?}"));
        assert_eq!(
            summary
                .get("profilingGroupName")
                .and_then(serde_json::Value::as_str),
            Some(ANALYSED_GROUP),
            "case {case:?}: wrong profilingGroupName, got {summary:?}"
        );
        assert_eq!(
            summary
                .get("totalNumberOfFindings")
                .and_then(serde_json::Value::as_i64),
            Some(i64::from(case.findings)),
            "case {case:?}: wrong totalNumberOfFindings, got {summary:?}"
        );
        assert_eq!(
            timestamp_secs(summary.get("profileStartTime")),
            Some(DateTime::from_secs(case.start_secs).as_secs_f64()),
            "case {case:?}: wrong profileStartTime, got {summary:?}"
        );
        assert_eq!(
            timestamp_secs(summary.get("profileEndTime")),
            Some(DateTime::from_secs(case.end_secs).as_secs_f64()),
            "case {case:?}: wrong profileEndTime, got {summary:?}"
        );
        // `aggregationPeriod` is not a member of `FindingsReportSummary`: it is stored
        // to answer `dailyReportsOnly` and never reported.
        assert!(
            summary.get("aggregationPeriod").is_none(),
            "case {case:?}: FindingsReportSummary models no aggregationPeriod, got {summary:?}"
        );
    }
}

#[tokio::test]
async fn paging_visits_every_report_exactly_once() {
    let harness = Harness::seeded().await;

    // Page sizes inside the modelled `maxResults` range (1..1000): every size below
    // the report count, one equal to it, and one far above.
    for page_size in [1usize, 2, 3, 4, 1000] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let cursor = token
                .as_deref()
                .map(|value| format!("&nextToken={value}"))
                .unwrap_or_default();
            let answer = harness
                .list_range(
                    ANALYSED_GROUP,
                    DAY1 - 1,
                    DAY1 + 5 * DAY,
                    None,
                    &format!("&maxResults={page_size}{cursor}"),
                )
                .await;
            assert_eq!(
                answer.status,
                200,
                "page size {page_size}: expected 200, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            let ids = answer.report_ids();
            assert!(
                ids.len() <= page_size,
                "page size {page_size}: a page must hold at most maxResults reports, got {ids:?}"
            );
            seen.extend(ids);
            pages += 1;
            assert!(
                pages <= IN_WINDOW_ORDER.len() + 1,
                "page size {page_size}: paging did not terminate, saw {seen:?}"
            );
            token = answer.next_token();
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen,
            IN_WINDOW_ORDER.to_vec(),
            "page size {page_size}: paging must visit every report exactly once, in window order"
        );
    }
}

#[tokio::test]
async fn a_listing_refuses_what_it_cannot_answer() {
    let harness = Harness::seeded().await;

    // (label, group, query, expected status, expected error code)
    let cases: Vec<(&str, &str, String, u16, &str)> = vec![
        (
            "a name no group is stored under",
            "no-such-group",
            format!("?startTime={}&endTime={}", iso(DAY1), iso(DAY1 + DAY)),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a group with no recorded analysis answers an empty listing, not an error",
            FRESH_GROUP,
            format!("?startTime={}&endTime={}", iso(DAY1), iso(DAY1 + DAY)),
            200,
            "",
        ),
        (
            "a range that ends before it starts",
            ANALYSED_GROUP,
            format!("?startTime={}&endTime={}", iso(DAY1 + DAY), iso(DAY1)),
            400,
            "ValidationException",
        ),
        (
            "a nextToken this service never issued",
            ANALYSED_GROUP,
            format!(
                "?startTime={}&endTime={}&nextToken=not_a_cursor",
                iso(DAY1 - 1),
                iso(DAY1 + 5 * DAY)
            ),
            400,
            "ValidationException",
        ),
        (
            "a nextToken past the end of the listing",
            ANALYSED_GROUP,
            format!(
                "?startTime={}&endTime={}&nextToken=99",
                iso(DAY1 - 1),
                iso(DAY1 + 5 * DAY)
            ),
            400,
            "ValidationException",
        ),
        (
            "a maxResults below the modelled range",
            ANALYSED_GROUP,
            format!(
                "?startTime={}&endTime={}&maxResults=0",
                iso(DAY1 - 1),
                iso(DAY1 + 5 * DAY)
            ),
            400,
            "ValidationException",
        ),
        (
            "a maxResults above the modelled range",
            ANALYSED_GROUP,
            format!(
                "?startTime={}&endTime={}&maxResults=1001",
                iso(DAY1 - 1),
                iso(DAY1 + 5 * DAY)
            ),
            400,
            "ValidationException",
        ),
    ];

    for (label, group, query, status, code) in cases {
        let answer = harness.list(group, &query).await;
        assert_eq!(
            answer.status,
            status,
            "case {label:?}: expected {status}, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        if !code.is_empty() {
            assert_eq!(
                answer.error_code(),
                code,
                "case {label:?}: wrong error code, got {}",
                String::from_utf8_lossy(&answer.body)
            );
        }
    }
}

#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let harness = Harness::seeded().await;
    let before = harness.stored_report_count(ANALYSED_GROUP);

    let queries = [
        ("the whole range", None, ""),
        ("daily only", Some(true), ""),
        ("one report per page", None, "&maxResults=1"),
    ];
    let mut bodies: Vec<(usize, Vec<String>)> = Vec::new();
    for round in 0..2usize {
        for (label, daily_only, extra) in &queries {
            let answer = harness
                .list_range(
                    ANALYSED_GROUP,
                    DAY1 - 1,
                    DAY1 + 5 * DAY,
                    *daily_only,
                    extra,
                )
                .await;
            assert_eq!(
                answer.status,
                200,
                "case {label:?} (round {round}): expected 200, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            bodies.push((round, answer.report_ids()));
        }
    }

    let (first_round, second_round) = bodies.split_at(queries.len());
    for (index, (_, ids)) in second_round.iter().enumerate() {
        assert_eq!(
            ids, &first_round[index].1,
            "case {:?}: a repeated listing must report the same reports, got {ids:?}",
            queries[index].0
        );
    }
    assert_eq!(
        harness.stored_report_count(ANALYSED_GROUP),
        before,
        "listing must not add or consume stored reports"
    );
}
