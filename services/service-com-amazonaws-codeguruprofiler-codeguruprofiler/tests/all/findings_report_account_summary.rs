// SPDX-License-Identifier: CC0-1.0

//! `GetFindingsReportAccountSummary`, end to end over HTTP through the router this
//! service mounts in production.
//!
//! The rule every case here asserts: a `GetFindingsReportAccountSummary` call reports
//! the findings reports RECORDED against EVERY profiling group the store holds — the
//! operation names no time range, so a report is reported whatever window it analysed —
//! restricted to reports over daily profiles when `dailyReportsOnly` is set, in a
//! stable order (by profiling-group name, then by analysed window and report id), one
//! `maxResults`-sized page at a time; and nothing about the summary changes what a
//! later caller sees.
//!
//! Rows vary what the rule generalises over: the number of groups, how many reports
//! each holds (including none at all, and an account with no groups at all), the
//! analysed windows (including two reports of one group that start at the same instant,
//! so the id tiebreak is what orders them), the aggregation period, the
//! `dailyReportsOnly` flag, the page size and the cursor.

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

/// The groups the fixture creates: two with analysis recorded against them and one
/// with none. Deliberately created in an order that is NOT their name order, so the
/// summary's own ordering is what the assertions see.
const ALPHA_GROUP: &str = "alpha-app";
const BETA_GROUP: &str = "beta-app";
const ZETA_GROUP: &str = "zeta-app";

/// One recorded findings report seeded into the store, and the group it belongs to.
#[derive(Debug, Clone)]
struct ReportCase {
    group: &'static str,
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

    fn is_daily(&self) -> bool {
        self.period == DAILY
    }
}

const ALPHA_DAILY: &str = "aaaaaaaa-0001-4aaa-8aaa-000000000001";
const ALPHA_SAME_START: &str = "aaaaaaaa-0003-4aaa-8aaa-000000000003";
const ALPHA_HOURLY: &str = "aaaaaaaa-0002-4aaa-8aaa-000000000002";
const ZETA_FIVE_MINUTES: &str = "cccccccc-0002-4ccc-8ccc-000000000002";
const ZETA_DAILY: &str = "cccccccc-0001-4ccc-8ccc-000000000001";

/// The reports recorded into the store, in the order they are recorded — neither group
/// order nor window order, so neither the map's iteration order nor the insertion order
/// can pass for the rule's order.
fn report_cases() -> Vec<ReportCase> {
    vec![
        ReportCase {
            group: ZETA_GROUP,
            id: ZETA_DAILY,
            start_secs: DAY1 + 2 * DAY,
            end_secs: DAY1 + 3 * DAY,
            findings: 0,
            period: DAILY,
        },
        ReportCase {
            group: ALPHA_GROUP,
            id: ALPHA_HOURLY,
            start_secs: DAY1 + 3_600,
            end_secs: DAY1 + 7_200,
            findings: 2,
            period: HOURLY,
        },
        ReportCase {
            group: ALPHA_GROUP,
            // The same analysed START as ALPHA_DAILY, so the report id is what breaks
            // the tie — and this one is recorded FIRST of the two.
            id: ALPHA_SAME_START,
            start_secs: DAY1,
            end_secs: DAY1 + 3_600,
            findings: 1,
            period: HOURLY,
        },
        ReportCase {
            group: ZETA_GROUP,
            id: ZETA_FIVE_MINUTES,
            start_secs: DAY1 + 100,
            end_secs: DAY1 + 400,
            findings: 9,
            period: FIVE_MINUTES,
        },
        ReportCase {
            group: ALPHA_GROUP,
            id: ALPHA_DAILY,
            start_secs: DAY1,
            end_secs: DAY1 + DAY,
            findings: 5,
            period: DAILY,
        },
    ]
}

/// The ids in the order the rule says the summary reports them: by group name, then by
/// analysed window (oldest first), then by report id.
const IN_ACCOUNT_ORDER: [&str; 5] = [
    ALPHA_DAILY,
    ALPHA_SAME_START,
    ALPHA_HOURLY,
    ZETA_FIVE_MINUTES,
    ZETA_DAILY,
];

/// The same order, restricted to reports over daily profiles.
const DAILY_ONLY_ORDER: [&str; 2] = [ALPHA_DAILY, ZETA_DAILY];

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

    fn summaries(&self) -> Vec<serde_json::Value> {
        self.json()
            .get("reportSummaries")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "no reportSummaries in {}",
                    String::from_utf8_lossy(&self.body)
                )
            })
    }

    /// `reportSummaries[*].id`, in the order the response lists them.
    fn report_ids(&self) -> Vec<String> {
        self.summaries()
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

/// The emulator, assembled exactly as the router assembles it: one shared engine
/// behind one mounted service. The engine is kept so a test can record analysis into
/// the store — the only way a findings report exists, since the emulator runs no
/// recommendation analysis of its own — and assert the summary did not change it.
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

    /// Create a profiling group the way a client does, so every summary below runs
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

    async fn summary(&self, query: &str) -> Answer {
        self.send("GET", &format!("/internal/findingsReports{query}"), Vec::new())
            .await
    }

    /// The account summary with the optional `dailyReportsOnly` flag and whatever page
    /// parameters a row asks for.
    async fn summary_of(&self, daily_only: Option<bool>, extra: &str) -> Answer {
        let mut query = String::new();
        if let Some(daily) = daily_only {
            query.push_str(&format!("?dailyReportsOnly={daily}"));
        }
        if !extra.is_empty() {
            query.push(if query.is_empty() { '?' } else { '&' });
            query.push_str(extra);
        }
        self.summary(&query).await
    }

    fn state(&self) -> engine::ServiceState<ComAmazonawsCodeguruprofilerCodeguruprofilerState> {
        self.engine
            .state::<ComAmazonawsCodeguruprofilerCodeguruprofilerState>()
    }

    /// Record the seeded analysis, through the store's own writer.
    fn record_reports(&self, cases: &[ReportCase]) {
        let state = self.state();
        let mut guard = state.lock();
        for case in cases {
            assert!(
                guard.record_findings_report(case.group, case.stored()),
                "case {case:?}: recording analysis must find the group"
            );
        }
    }

    /// How many reports the store holds, across every group, right now.
    fn stored_report_count(&self) -> usize {
        let state = self.state();
        let guard = state.lock();
        guard
            .groups
            .values()
            .map(|group| group.findings_reports.len())
            .sum()
    }

    /// The whole fixture: three created groups, analysis recorded against two of them.
    async fn seeded() -> Self {
        let harness = Self::new();
        harness.create_group(ZETA_GROUP, "tok-zeta").await;
        harness.create_group(ALPHA_GROUP, "tok-alpha").await;
        harness.create_group(BETA_GROUP, "tok-beta").await;
        harness.record_reports(&report_cases());
        harness
    }

    /// Groups but no analysis at all.
    async fn groups_without_analysis() -> Self {
        let harness = Self::new();
        harness.create_group(ALPHA_GROUP, "tok-alpha").await;
        harness.create_group(BETA_GROUP, "tok-beta").await;
        harness
    }
}

#[tokio::test]
async fn the_summary_reports_every_recorded_report_of_every_group() {
    // (label, dailyReportsOnly, the ids the rule says are reported)
    let flag_cases: Vec<(&str, Option<bool>, Vec<&str>)> = vec![
        (
            "no dailyReportsOnly reports every recorded report, in account order",
            None,
            IN_ACCOUNT_ORDER.to_vec(),
        ),
        (
            "dailyReportsOnly=false restricts nothing",
            Some(false),
            IN_ACCOUNT_ORDER.to_vec(),
        ),
        (
            "dailyReportsOnly=true keeps only the reports over daily profiles",
            Some(true),
            DAILY_ONLY_ORDER.to_vec(),
        ),
    ];

    let harness = Harness::seeded().await;
    for (label, daily_only, expected) in flag_cases {
        let answer = harness.summary_of(daily_only, "").await;
        assert_eq!(
            answer.status,
            200,
            "case {label:?}: expected 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let ids = answer.report_ids();
        assert_eq!(
            ids, expected,
            "case {label:?}: wrong reports summarised, got {ids:?}"
        );
        assert_eq!(
            answer.next_token(),
            None,
            "case {label:?}: a summary that fits one page must carry no nextToken, got {:?}",
            answer.next_token()
        );
    }
}

#[tokio::test]
async fn an_account_with_nothing_recorded_summarises_nothing() {
    // (label, the harness, the ids expected)
    let empty_cases: Vec<(&str, Harness)> = vec![
        ("no profiling groups at all", Harness::new()),
        (
            "groups that have had no analysis recorded",
            Harness::groups_without_analysis().await,
        ),
    ];

    for (label, harness) in empty_cases {
        for daily_only in [None, Some(false), Some(true)] {
            let answer = harness.summary_of(daily_only, "").await;
            assert_eq!(
                answer.status,
                200,
                "case {label:?} (dailyReportsOnly={daily_only:?}): expected 200, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            let ids = answer.report_ids();
            assert!(
                ids.is_empty(),
                "case {label:?} (dailyReportsOnly={daily_only:?}): expected no summaries, got {ids:?}"
            );
            assert_eq!(
                answer.next_token(),
                None,
                "case {label:?}: an empty summary must carry no nextToken, got {:?}",
                answer.next_token()
            );
        }
    }
}

#[tokio::test]
async fn every_summary_is_built_from_the_stored_report() {
    let harness = Harness::seeded().await;

    let answer = harness.summary_of(None, "").await;
    assert_eq!(
        answer.status,
        200,
        "expected 200, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    let summaries = answer.summaries();
    assert_eq!(
        summaries.len(),
        report_cases().len(),
        "expected every recorded report, got {summaries:?}"
    );

    for case in report_cases() {
        let summary = summaries
            .iter()
            .find(|summary| summary.get("id").and_then(serde_json::Value::as_str) == Some(case.id))
            .unwrap_or_else(|| panic!("case {case:?}: not summarised, got {summaries:?}"));
        assert_eq!(
            summary
                .get("profilingGroupName")
                .and_then(serde_json::Value::as_str),
            Some(case.group),
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
        assert_eq!(
            case.is_daily(),
            DAILY_ONLY_ORDER.contains(&case.id),
            "case {case:?}: the daily-only expectation table and the stored period disagree"
        );
    }
}

#[tokio::test]
async fn paging_visits_every_report_exactly_once() {
    let harness = Harness::seeded().await;

    // Page sizes inside the modelled `maxResults` range (1..1000): every size below
    // the report count, one equal to it, and one far above.
    for page_size in [1usize, 2, 3, 4, 5, 1000] {
        for (label, daily_only, expected) in [
            ("every report", None, IN_ACCOUNT_ORDER.to_vec()),
            ("daily only", Some(true), DAILY_ONLY_ORDER.to_vec()),
        ] {
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0usize;
            loop {
                let cursor = token
                    .as_deref()
                    .map(|value| format!("&nextToken={value}"))
                    .unwrap_or_default();
                let answer = harness
                    .summary_of(daily_only, &format!("maxResults={page_size}{cursor}"))
                    .await;
                assert_eq!(
                    answer.status,
                    200,
                    "case {label:?} (page size {page_size}): expected 200, got {} {}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                let ids = answer.report_ids();
                assert!(
                    ids.len() <= page_size,
                    "case {label:?} (page size {page_size}): a page must hold at most \
                     maxResults reports, got {ids:?}"
                );
                seen.extend(ids);
                pages += 1;
                assert!(
                    pages <= IN_ACCOUNT_ORDER.len() + 1,
                    "case {label:?} (page size {page_size}): paging did not terminate, \
                     saw {seen:?}"
                );
                token = answer.next_token();
                if token.is_none() {
                    break;
                }
            }
            assert_eq!(
                seen, expected,
                "case {label:?} (page size {page_size}): paging must visit every report \
                 exactly once, in account order"
            );
        }
    }
}

#[tokio::test]
async fn the_summary_refuses_what_it_cannot_answer() {
    let harness = Harness::seeded().await;

    // (label, query, expected status, expected error code)
    let cases: Vec<(&str, &str, u16, &str)> = vec![
        (
            "a nextToken this service never issued",
            "?nextToken=not_a_cursor",
            400,
            "ValidationException",
        ),
        (
            "a nextToken past the end of the summary",
            "?nextToken=99",
            400,
            "ValidationException",
        ),
        (
            "a nextToken past the end of a dailyReportsOnly summary",
            "?dailyReportsOnly=true&nextToken=3",
            400,
            "ValidationException",
        ),
        (
            "a maxResults below the modelled range",
            "?maxResults=0",
            400,
            "ValidationException",
        ),
        (
            "a maxResults above the modelled range",
            "?maxResults=1001",
            400,
            "ValidationException",
        ),
        (
            "a nextToken at exactly the end of the summary is one this service issued",
            "?maxResults=1&nextToken=5",
            200,
            "",
        ),
    ];

    for (label, query, status, code) in cases {
        let answer = harness.summary(query).await;
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
async fn summarising_changes_nothing_a_later_caller_sees() {
    let harness = Harness::seeded().await;
    let before = harness.stored_report_count();

    let queries: [(&str, Option<bool>, &str); 3] = [
        ("the whole account", None, ""),
        ("daily only", Some(true), ""),
        ("one report per page", None, "maxResults=1"),
    ];
    let mut rounds: Vec<Vec<String>> = Vec::new();
    for round in 0..2usize {
        for (label, daily_only, extra) in &queries {
            let answer = harness.summary_of(*daily_only, extra).await;
            assert_eq!(
                answer.status,
                200,
                "case {label:?} (round {round}): expected 200, got {} {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            rounds.push(answer.report_ids());
        }
    }

    let (first_round, second_round) = rounds.split_at(queries.len());
    for (index, ids) in second_round.iter().enumerate() {
        assert_eq!(
            ids, &first_round[index],
            "case {:?}: a repeated summary must report the same reports, got {ids:?}",
            queries[index].0
        );
    }
    assert_eq!(
        harness.stored_report_count(),
        before,
        "summarising must not add or consume stored reports"
    );
}

#[tokio::test]
async fn the_summary_agrees_with_each_group_s_own_listing() {
    let harness = Harness::seeded().await;

    let answer = harness.summary_of(None, "").await;
    assert_eq!(
        answer.status,
        200,
        "expected 200, got {} {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    let account_ids = answer.report_ids();

    // Every group's own `ListFindingsReports` over a range that covers every seeded
    // window must report exactly the reports the account summary attributes to it, in
    // the same order: both read the same stored reports.
    for group in [ALPHA_GROUP, BETA_GROUP, ZETA_GROUP] {
        let start = DateTime::from_secs(DAY1 - DAY)
            .fmt(Format::DateTime)
            .expect("a formattable timestamp");
        let end = DateTime::from_secs(DAY1 + 10 * DAY)
            .fmt(Format::DateTime)
            .expect("a formattable timestamp");
        let listed = harness
            .send(
                "GET",
                &format!(
                    "/internal/profilingGroups/{group}/findingsReports?startTime={start}&endTime={end}"
                ),
                Vec::new(),
            )
            .await;
        assert_eq!(
            listed.status,
            200,
            "case {group:?}: ListFindingsReports expected 200, got {} {}",
            listed.status,
            String::from_utf8_lossy(&listed.body)
        );
        let listed_ids: Vec<String> = listed
            .json()
            .get("findingsReportSummaries")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|summary| {
                summary
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();

        let attributed: Vec<String> = answer
            .summaries()
            .iter()
            .filter(|summary| {
                summary
                    .get("profilingGroupName")
                    .and_then(serde_json::Value::as_str)
                    == Some(group)
            })
            .map(|summary| {
                summary
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            attributed, listed_ids,
            "case {group:?}: the account summary and the group's own listing must report \
             the same reports, got {attributed:?} vs {listed_ids:?} (account: {account_ids:?})"
        );
    }
}
