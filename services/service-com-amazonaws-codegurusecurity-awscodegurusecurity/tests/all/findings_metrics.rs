// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListFindingsMetrics`: the per-day findings metrics of a
//! requested interval.
//!
//! Every request goes through the real router (`MultiServiceRouter::dispatch`), reusing
//! the harness of `crate::scan_lifecycle`, so the restJson1 wire shape and the shared
//! engine store are exercised exactly as a client would exercise them.
//!
//! The rule under test: the answer carries ONE `AccountFindingsMetric` per UTC day of the
//! interval `[startDate, endDate]` (each bound rounded down to the day its instant falls
//! in, both ends included), and each day reports, per severity, the findings CREATED that
//! day (`newFindings`), the findings CLOSED that day (`closedFindings`), the findings OPEN
//! as of the end of that day (`openFindings`) and the mean days-to-close of the findings
//! closed by then (`meanTimeToClose`, absent for a severity nothing of which had closed),
//! paginated at `maxResults` days per page.
//!
//! The cases vary what the rule generalises over: where the interval sits relative to the
//! stored findings, how wide it is, the severity, status, creation and close day of each
//! finding, and the page size.

use serde_json::Value;
use service_com_amazonaws_codegurusecurity_awscodegurusecurity::ops::list_findings_metrics::MAX_PAGE_SIZE;
use service_com_amazonaws_codegurusecurity_awscodegurusecurity::ops::types::Finding;
use service_com_amazonaws_codegurusecurity_awscodegurusecurity::state::{
    SEVERITY_CRITICAL, SEVERITY_HIGH, SEVERITY_INFO, SEVERITY_LOW, SEVERITY_MEDIUM, STATUS_CLOSED,
    STATUS_OPEN,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::metrics_summary::{instant_secs, iso};
use crate::scan_lifecycle::{call, harness, store_of, Answer};

/// One day in seconds.
const DAY: i64 = 24 * 60 * 60;

/// `2024-03-01T00:00:00Z`: the first day of the seeded interval.
const DAY_ZERO: i64 = 1_709_251_200;

/// The middle of a day, so every seeded instant is inside its day rather than on a
/// boundary.
const NOON: i64 = DAY / 2;

/// The start of the `n`-th day after [`DAY_ZERO`].
fn day(n: i64) -> i64 {
    DAY_ZERO + n * DAY
}

/// How far two reported `Double` values may differ and still be the same measurement.
const EPSILON: f64 = 1e-9;

/// One seeded finding: `(scanName, findingId, severity, status, createdAt, updatedAt)`.
/// `updatedAt` of a `Closed` finding is the instant it was closed.
type Seed = (
    &'static str,
    &'static str,
    Option<&'static str>,
    &'static str,
    Option<i64>,
    Option<i64>,
);

/// A finding as a scan run's analysis would have stored it. Seeded directly into the
/// store: no request path writes findings yet, so this is the only way to exercise the
/// metrics over a non-empty store.
fn seed(engine: &std::sync::Arc<engine::InMemoryEngine>, seeds: &[Seed]) {
    let store = store_of(engine);
    let mut guard = store.lock();
    for (scan_name, finding_id, severity, status, created_at, updated_at) in seeds {
        guard.findings.insert(
            ((*scan_name).to_string(), (*finding_id).to_string()),
            Finding {
                id: Some((*finding_id).to_string()),
                status: Some((*status).to_string()),
                severity: severity.map(str::to_string),
                detector_name: Some("Seeded detector".to_string()),
                created_at: created_at.map(aws_smithy_types::DateTime::from_secs),
                updated_at: updated_at.map(aws_smithy_types::DateTime::from_secs),
                title: Some(format!("seeded finding {finding_id}")),
                ..Finding::default()
            },
        );
    }
}

/// The store every aggregation case below reads: findings of every modelled severity,
/// open and closed, created on four different days, one closed the day it was created and
/// two closed days later, one of no severity at all, and one `Closed` finding that
/// records no close instant.
const SEEDED: &[Seed] = &[
    // Closed two days after it was created.
    (
        "alpha",
        "f1",
        Some(SEVERITY_CRITICAL),
        STATUS_CLOSED,
        Some(DAY_ZERO + NOON),
        Some(DAY_ZERO + 2 * DAY + NOON),
    ),
    (
        "alpha",
        "f2",
        Some(SEVERITY_HIGH),
        STATUS_OPEN,
        Some(DAY_ZERO + NOON),
        None,
    ),
    (
        "beta",
        "f3",
        Some(SEVERITY_MEDIUM),
        STATUS_OPEN,
        Some(DAY_ZERO + DAY + NOON),
        None,
    ),
    // Closed the same day it was created: zero days to close.
    (
        "beta",
        "f4",
        Some(SEVERITY_LOW),
        STATUS_CLOSED,
        Some(DAY_ZERO + DAY + NOON),
        Some(DAY_ZERO + DAY + NOON),
    ),
    (
        "beta",
        "f5",
        Some(SEVERITY_INFO),
        STATUS_OPEN,
        Some(DAY_ZERO + 3 * DAY + NOON),
        None,
    ),
    (
        "gamma",
        "f6",
        Some(SEVERITY_CRITICAL),
        STATUS_OPEN,
        Some(DAY_ZERO + 2 * DAY + NOON),
        None,
    ),
    // Closed four days after it was created.
    (
        "gamma",
        "f7",
        Some(SEVERITY_HIGH),
        STATUS_CLOSED,
        Some(DAY_ZERO + NOON),
        Some(DAY_ZERO + 4 * DAY + NOON),
    ),
    // No severity: every member of `AccountFindingsMetric` is a value per severity, so
    // this finding is counted in none of them.
    (
        "gamma",
        "f8",
        None,
        STATUS_OPEN,
        Some(DAY_ZERO + NOON),
        None,
    ),
    // `Closed` with no `updatedAt`: no close instant, so it is counted in no day's
    // closures, in no day's open findings and in no mean.
    (
        "gamma",
        "f9",
        Some(SEVERITY_CRITICAL),
        STATUS_CLOSED,
        Some(DAY_ZERO + NOON),
        None,
    ),
];

/// A per-severity expectation, in the order `FindingMetricsValuePerSeverity` declares its
/// members: `(info, low, medium, high, critical)`.
type PerSeverity = [f64; 5];

/// The same, where a severity may report NO value at all.
type PerSeverityOption = [Option<f64>; 5];

/// What one day of the seeded store adds up to.
struct DayCase {
    /// The day, as the number of days after [`DAY_ZERO`].
    index: i64,
    new_findings: PerSeverity,
    closed_findings: PerSeverity,
    open_findings: PerSeverity,
    mean_time_to_close: PerSeverityOption,
}

/// Every day of the seeded interval, as the rule adds it up.
const DAY_CASES: &[DayCase] = &[
    // Two criticals and two highs created; nothing closed yet; `f1`, `f2` and `f7` open
    // (`f9` records no close instant, so it is in no open count).
    DayCase {
        index: 0,
        new_findings: [0.0, 0.0, 0.0, 2.0, 2.0],
        closed_findings: [0.0, 0.0, 0.0, 0.0, 0.0],
        open_findings: [0.0, 0.0, 0.0, 2.0, 1.0],
        mean_time_to_close: [None, None, None, None, None],
    },
    // `f3` and `f4` created, `f4` closed the same day: a mean of zero days for Low.
    DayCase {
        index: 1,
        new_findings: [0.0, 1.0, 1.0, 0.0, 0.0],
        closed_findings: [0.0, 1.0, 0.0, 0.0, 0.0],
        open_findings: [0.0, 0.0, 1.0, 2.0, 1.0],
        mean_time_to_close: [None, Some(0.0), None, None, None],
    },
    // `f6` created and `f1` closed after two days: Critical's open count stays 1 as one
    // critical closes and another opens.
    DayCase {
        index: 2,
        new_findings: [0.0, 0.0, 0.0, 0.0, 1.0],
        closed_findings: [0.0, 0.0, 0.0, 0.0, 1.0],
        open_findings: [0.0, 0.0, 1.0, 2.0, 1.0],
        mean_time_to_close: [None, Some(0.0), None, None, Some(2.0)],
    },
    // `f5` created; nothing closed, so the means are the previous day's.
    DayCase {
        index: 3,
        new_findings: [1.0, 0.0, 0.0, 0.0, 0.0],
        closed_findings: [0.0, 0.0, 0.0, 0.0, 0.0],
        open_findings: [1.0, 0.0, 1.0, 2.0, 1.0],
        mean_time_to_close: [None, Some(0.0), None, None, Some(2.0)],
    },
    // `f7` closed after four days: High loses an open finding and gains a mean.
    DayCase {
        index: 4,
        new_findings: [0.0, 0.0, 0.0, 0.0, 0.0],
        closed_findings: [0.0, 0.0, 0.0, 1.0, 0.0],
        open_findings: [1.0, 0.0, 1.0, 1.0, 1.0],
        mean_time_to_close: [None, Some(0.0), None, Some(4.0), Some(2.0)],
    },
];

/// `GET /metrics/findings?…`, the modelled route of `ListFindingsMetrics`.
async fn list_findings_metrics(
    router: &MultiServiceRouter,
    start_secs: i64,
    end_secs: i64,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> Answer {
    let mut query = format!(
        "startDate={}&endDate={}",
        iso(start_secs),
        iso(end_secs)
    );
    if let Some(max_results) = max_results {
        query.push_str(&format!("&maxResults={max_results}"));
    }
    if let Some(token) = next_token {
        query.push_str(&format!("&nextToken={token}"));
    }
    call(router, "GET", &format!("/metrics/findings?{query}"), None).await
}

/// The `findingsMetrics` of a successful answer.
fn metrics_of<'a>(case: &str, answer: &'a Answer) -> &'a Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200, got {} {}",
        answer.status, answer.json
    );
    answer
        .get("findingsMetrics")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: findingsMetrics must be present as an array, got {}",
                answer.json
            )
        })
}

/// The `nextToken` of an answer, when it carries one.
fn token_of(answer: &Answer) -> Option<String> {
    answer.get("nextToken").as_str().map(str::to_string)
}

/// One `FindingMetricsValuePerSeverity` member of `metric`, read in the order the shape
/// declares its severities; `None` where a severity reports no value.
fn per_severity(case: &str, metric: &Value, member: &str) -> PerSeverityOption {
    let value = &metric[member];
    assert!(
        value.is_object(),
        "case {case:?}: {member} must be present as an object, got {metric}"
    );
    ["info", "low", "medium", "high", "critical"].map(|severity| {
        match &value[severity] {
            Value::Null => None,
            number => Some(number.as_f64().unwrap_or_else(|| {
                panic!("case {case:?}: {member}.{severity} must be a number, got {value}")
            })),
        }
    })
}

/// Assert that `member` of `metric` reports exactly `expected`.
fn assert_per_severity(case: &str, metric: &Value, member: &str, expected: PerSeverityOption) {
    let actual = per_severity(case, metric, member);
    for (severity, (expected, actual)) in ["info", "low", "medium", "high", "critical"]
        .into_iter()
        .zip(expected.into_iter().zip(actual))
    {
        match (expected, actual) {
            (Some(expected), Some(actual)) => assert!(
                (expected - actual).abs() < EPSILON,
                "case {case:?}: {member}.{severity} expected {expected}, got {actual} in {metric}"
            ),
            (None, None) => {}
            (expected, actual) => panic!(
                "case {case:?}: {member}.{severity} expected {expected:?}, got {actual:?} in {metric}"
            ),
        }
    }
}

/// Assert that `metric` is the day `case` describes.
fn assert_day(label: &str, metric: &Value, case: &DayCase) {
    let case_label = format!("{label}, day {}", case.index);
    assert_eq!(
        instant_secs(&case_label, &metric["date"]),
        day(case.index),
        "case {case_label:?}: each entry is reported for the start of its UTC day, got {metric}"
    );
    let present = |counts: PerSeverity| counts.map(Some);
    assert_per_severity(
        &case_label,
        metric,
        "newFindings",
        present(case.new_findings),
    );
    assert_per_severity(
        &case_label,
        metric,
        "closedFindings",
        present(case.closed_findings),
    );
    assert_per_severity(
        &case_label,
        metric,
        "openFindings",
        present(case.open_findings),
    );
    assert_per_severity(
        &case_label,
        metric,
        "meanTimeToClose",
        case.mean_time_to_close,
    );
}

#[tokio::test]
async fn each_day_of_the_interval_reports_the_accounts_findings_metrics() {
    let (engine, router) = harness();
    seed(&engine, SEEDED);

    // The instants naming the interval are deliberately NOT day boundaries: each bound
    // selects the day it falls in.
    let label = "the whole seeded interval";
    let answer = list_findings_metrics(&router, day(0) + 1, day(4) + DAY - 1, None, None).await;
    let metrics = metrics_of(label, &answer);
    assert_eq!(
        metrics.len(),
        DAY_CASES.len(),
        "case {label:?}: one entry per UTC day of the interval, both ends included, got {}",
        answer.json
    );
    for (metric, case) in metrics.iter().zip(DAY_CASES) {
        assert_day(label, metric, case);
    }
    assert_eq!(
        token_of(&answer),
        None,
        "case {label:?}: a single page of {} days carries no nextToken, got {}",
        DAY_CASES.len(),
        answer.json
    );
}

#[tokio::test]
async fn an_interval_of_any_width_or_position_reports_the_days_it_covers() {
    let (engine, router) = harness();
    seed(&engine, SEEDED);

    // Rows: where the interval sits relative to the stored findings and how wide it is,
    // against the days it must report.
    let cases: &[(&str, i64, i64, &[i64])] = &[
        ("one day, the first of the interval", 0, 0, &[0]),
        ("one day, in the middle", 2, 2, &[2]),
        ("one day, after every finding", 9, 9, &[9]),
        ("one day, before every finding", -5, -5, &[-5]),
        ("two days spanning the first finding", -1, 0, &[-1, 0]),
        ("three days inside the interval", 1, 3, &[1, 2, 3]),
        (
            "a week spanning every finding",
            -1,
            5,
            &[-1, 0, 1, 2, 3, 4, 5],
        ),
        ("an interval whose end precedes its start", 3, 1, &[]),
    ];

    for (label, start_index, end_index, expected_days) in cases {
        let answer = list_findings_metrics(
            &router,
            day(*start_index) + NOON,
            day(*end_index) + NOON,
            None,
            None,
        )
        .await;
        let metrics = metrics_of(label, &answer);
        let reported: Vec<i64> = metrics
            .iter()
            .map(|metric| instant_secs(label, &metric["date"]))
            .collect();
        let expected: Vec<i64> = expected_days.iter().map(|index| day(*index)).collect();
        assert_eq!(
            reported, expected,
            "case {label:?}: the days of the interval, in order, got {}",
            answer.json
        );
        // Each day reported is still the day the rule describes, wherever the interval
        // starts: the days inside the seeded span are checked against the table, and a
        // day outside it reports zeros and no mean.
        for metric in metrics {
            let reported_day = instant_secs(label, &metric["date"]);
            match DAY_CASES
                .iter()
                .find(|case| day(case.index) == reported_day)
            {
                Some(case) => assert_day(label, metric, case),
                None => {
                    let zero: PerSeverityOption = [Some(0.0); 5];
                    for member in ["newFindings", "closedFindings"] {
                        assert_per_severity(label, metric, member, zero);
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn a_day_before_every_finding_reports_zeros_and_no_mean() {
    let (engine, router) = harness();
    seed(&engine, SEEDED);

    let label = "three days before the first finding";
    let answer = list_findings_metrics(&router, day(-4), day(-2), None, None).await;
    let metrics = metrics_of(label, &answer);
    assert_eq!(
        metrics.len(),
        3,
        "case {label:?}: three days were asked for, got {}",
        answer.json
    );
    for metric in metrics {
        for member in ["newFindings", "closedFindings", "openFindings"] {
            assert_per_severity(label, metric, member, [Some(0.0); 5]);
        }
        assert_per_severity(label, metric, "meanTimeToClose", [None; 5]);
    }
}

#[tokio::test]
async fn the_metrics_of_an_account_with_no_findings_are_zeros() {
    let (_engine, router) = harness();

    let label = "an account with no findings";
    let answer = list_findings_metrics(&router, day(0), day(2), None, None).await;
    let metrics = metrics_of(label, &answer);
    assert_eq!(
        metrics.len(),
        3,
        "case {label:?}: one entry per day is reported even with nothing to count, got {}",
        answer.json
    );
    for metric in metrics {
        for member in ["newFindings", "closedFindings", "openFindings"] {
            assert_per_severity(label, metric, member, [Some(0.0); 5]);
        }
        assert_per_severity(label, metric, "meanTimeToClose", [None; 5]);
    }
}

#[tokio::test]
async fn paging_at_any_page_size_reports_every_day_exactly_once() {
    let (engine, router) = harness();
    seed(&engine, SEEDED);

    let unpaged = list_findings_metrics(&router, day(0), day(4), None, None).await;
    let whole = metrics_of("the unpaged interval", &unpaged).clone();

    // Rows: the page size, from one day a page up to more than the interval holds. The
    // invariant: however the answer is split, the pages concatenate to the unpaged one.
    for page_size in [1_i64, 2, 3, 5, 6] {
        let label = format!("pages of {page_size} day(s)");
        let mut collected: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let answer = list_findings_metrics(
                &router,
                day(0),
                day(4),
                Some(page_size),
                token.as_deref(),
            )
            .await;
            let page = metrics_of(&label, &answer);
            assert!(
                page.len() <= usize::try_from(page_size).unwrap_or(1),
                "case {label:?}: a page never carries more than maxResults days, got {}",
                answer.json
            );
            collected.extend(page.iter().cloned());
            pages += 1;
            assert!(
                pages <= whole.len() + 1,
                "case {label:?}: paging must terminate, got {pages} pages"
            );
            match token_of(&answer) {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            collected, whole,
            "case {label:?}: the pages concatenate to the whole interval"
        );
    }
}

#[tokio::test]
async fn max_results_is_honoured_at_its_published_bounds() {
    let (engine, router) = harness();
    seed(&engine, SEEDED);

    // The modelled range of `ListFindingsMetricsRequest$maxResults` is 1..1000, so each
    // bound is accepted and the value past each bound is refused by the framework's
    // constraint check with the modelled `ValidationException` (HTTP 400).
    let accepted: &[(&str, i64, usize)] = &[
        ("the smallest page", 1, 1),
        ("two days a page", 2, 2),
        ("the largest page", i64::from(MAX_PAGE_SIZE), 5),
    ];
    for (label, max_results, expected_len) in accepted {
        let answer =
            list_findings_metrics(&router, day(0), day(4), Some(*max_results), None).await;
        let metrics = metrics_of(label, &answer);
        assert_eq!(
            metrics.len(),
            *expected_len,
            "case {label:?}: maxResults={max_results} bounds the page, got {}",
            answer.json
        );
    }

    for (label, max_results) in [
        ("one below the minimum", 0_i64),
        ("one past the maximum", i64::from(MAX_PAGE_SIZE) + 1),
    ] {
        let answer = list_findings_metrics(&router, day(0), day(4), Some(max_results), None).await;
        assert_eq!(
            answer.status, 400,
            "case {label:?}: maxResults={max_results} is outside the modelled range, got {} {}",
            answer.status, answer.json
        );
        assert_eq!(
            answer.error_type, "ValidationException",
            "case {label:?}: a member outside its modelled range is a ValidationException, got {}",
            answer.json
        );
    }
}

#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let (engine, router) = harness();
    seed(&engine, SEEDED);

    // Rows: a token that is not the hex this service issues, and one that is hex but
    // names no day. Both satisfy the modelled `NextToken` constraints, so each reaches
    // the handler and is refused there.
    let cases: &[(&str, &str)] = &[
        ("not a token at all", "zzz-not-a-token"),
        ("hex of something that is not a day", "616c706861"),
    ];
    for (label, token) in cases {
        let answer = list_findings_metrics(&router, day(0), day(4), None, Some(token)).await;
        assert_eq!(
            answer.status, 400,
            "case {label:?}: an unissued nextToken is refused, got {} {}",
            answer.status, answer.json
        );
        assert_eq!(
            answer.error_type, "ValidationException",
            "case {label:?}: the refusal is the modelled ValidationException, got {}",
            answer.json
        );
    }

    // And the token this service DOES issue is accepted, so the refusal above is about
    // the token's origin and not about passing one at all.
    let first = list_findings_metrics(&router, day(0), day(4), Some(2), None).await;
    let token = token_of(&first).expect("a nextToken after the first of three pages");
    let second = list_findings_metrics(&router, day(0), day(4), Some(2), Some(&token)).await;
    let label = "the token of the first page";
    let metrics = metrics_of(label, &second);
    assert_eq!(
        metrics
            .iter()
            .map(|metric| instant_secs(label, &metric["date"]))
            .collect::<Vec<_>>(),
        vec![day(2), day(3)],
        "case {label:?}: the next page resumes at the first day the previous one did not report, got {}",
        second.json
    );
}

#[tokio::test]
async fn reading_the_metrics_does_not_change_what_a_later_read_reports() {
    let (engine, router) = harness();
    seed(&engine, SEEDED);

    let label = "two reads of one store";
    let first = list_findings_metrics(&router, day(0), day(4), None, None).await;
    let second = list_findings_metrics(&router, day(0), day(4), None, None).await;
    assert_eq!(
        metrics_of(label, &first),
        metrics_of(label, &second),
        "case {label:?}: a reporting operation answers the same question the same way"
    );

    let store = store_of(&engine);
    assert_eq!(
        store.lock().findings.len(),
        SEEDED.len(),
        "case {label:?}: reporting metrics must not add or drop a finding"
    );
}
