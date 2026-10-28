// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the instant a metric reported "as of" a day is read at:
//! `GetMetricsSummary` over one day, cross-checked against `ListFindingsMetrics` over the
//! interval that contains it.
//!
//! Every request goes through the real router (`MultiServiceRouter::dispatch`), reusing
//! the harness of `crate::scan_lifecycle`, so the restJson1 wire shape and the shared
//! engine store are exercised exactly as a client would exercise them.
//!
//! The rule under test: a metric reported AS OF a day is a reading of the account at the
//! END of that day, so each stored finding is placed in time by the instants it records
//! and not by the status it carries now — it counts as open as of a day when it existed by
//! then (`createdAt`) and had not been closed by then (a closed finding's close instant is
//! its `updatedAt`). Two operations of this one findings store therefore report the same
//! open-finding counts for the same day.
//!
//! The cases vary what the rule generalises over: where a finding's close instant falls
//! relative to the day asked about (days before it, the same day, its first and last
//! second, days after it, never), whether it records a creation instant at all, its
//! severity (including none), its detector (including none) and its scan — read through
//! every member the counts reach (`openFindings`, `categoriesWithMostFindings`,
//! `scansWithMostOpenFindings`, `scansWithMostOpenCriticalFindings`).

use serde_json::Value;
use service_com_amazonaws_codegurusecurity_awscodegurusecurity::ops::types::Finding;
use service_com_amazonaws_codegurusecurity_awscodegurusecurity::state::{
    SEVERITY_CRITICAL, SEVERITY_HIGH, SEVERITY_INFO, SEVERITY_LOW, SEVERITY_MEDIUM, STATUS_CLOSED,
    STATUS_OPEN,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::metrics_summary::{
    get_metrics_summary, instant_secs, iso, open_findings, ranking, summary_of,
};
use crate::scan_lifecycle::{call, harness, store_of, Answer};

/// One day in seconds.
const DAY: i64 = 24 * 60 * 60;

/// `2024-03-04T00:00:00Z`: the day the seeded findings were created. Fixed, because every
/// instant this suite asks about is relative to it and nothing in the rule depends on how
/// old the day is.
const DAY_ZERO: i64 = 1_709_510_400;

/// The middle of a day, so a seeded instant sits inside its day rather than on a boundary.
const NOON: i64 = DAY / 2;

/// The start of the `n`-th day after [`DAY_ZERO`]; `n` may be negative, for a day that
/// precedes every seeded finding.
fn day(n: i64) -> i64 {
    DAY_ZERO + n * DAY
}

/// One seeded finding: `(scanName, findingId, severity, detectorName, status, createdAt,
/// updatedAt)`, each instant in epoch seconds. The `updatedAt` of a `Closed` finding is
/// the instant it was closed.
type Seed = (
    &'static str,
    &'static str,
    Option<&'static str>,
    Option<&'static str>,
    &'static str,
    Option<i64>,
    Option<i64>,
);

/// Put `seeds` in the store both metrics handlers read. Seeded directly, so the metrics
/// are exercised over chosen creation and close instants without one upload and one run
/// per row.
fn seed(engine: &std::sync::Arc<engine::InMemoryEngine>, seeds: &[Seed]) {
    let store = store_of(engine);
    let mut guard = store.lock();
    for (scan_name, finding_id, severity, detector_name, status, created_at, updated_at) in seeds {
        guard.findings.insert(
            ((*scan_name).to_string(), (*finding_id).to_string()),
            Finding {
                id: Some((*finding_id).to_string()),
                status: Some((*status).to_string()),
                severity: severity.map(str::to_string),
                detector_name: detector_name.map(str::to_string),
                created_at: created_at.map(aws_smithy_types::DateTime::from_secs),
                updated_at: updated_at.map(aws_smithy_types::DateTime::from_secs),
                title: Some(format!("seeded finding {finding_id}")),
                ..Finding::default()
            },
        );
    }
}

/// The store every case below reads: one finding per way a close instant can fall
/// relative to a day asked about, spread over five scans and five detectors so the
/// rankings move with the counts.
const SEEDED: &[Seed] = &[
    // Closed two days after it was created: open as of the two days before that.
    (
        "alpha",
        "a1",
        Some(SEVERITY_CRITICAL),
        Some("SQL injection"),
        STATUS_CLOSED,
        Some(DAY_ZERO + NOON),
        Some(DAY_ZERO + 2 * DAY + NOON),
    ),
    // Never closed: open as of every day from the one it was created on.
    (
        "alpha",
        "a2",
        Some(SEVERITY_HIGH),
        Some("Hardcoded credentials"),
        STATUS_OPEN,
        Some(DAY_ZERO + NOON),
        None,
    ),
    // Created and closed within one day: open as of no day at all.
    (
        "alpha",
        "a3",
        Some(SEVERITY_HIGH),
        Some("Hardcoded credentials"),
        STATUS_CLOSED,
        Some(DAY_ZERO + DAY + NOON),
        Some(DAY_ZERO + DAY + NOON),
    ),
    // Closed at the LAST second of day 3: closed by that day, open as of day 2.
    (
        "beta",
        "b1",
        Some(SEVERITY_CRITICAL),
        Some("SQL injection"),
        STATUS_CLOSED,
        Some(DAY_ZERO + NOON),
        Some(DAY_ZERO + 4 * DAY - 1),
    ),
    // `Closed` recording no `updatedAt`: no close instant, so no day can be placed either
    // side of it and it is open as of none.
    (
        "beta",
        "b2",
        Some(SEVERITY_MEDIUM),
        Some("Insecure hashing"),
        STATUS_CLOSED,
        Some(DAY_ZERO + NOON),
        None,
    ),
    // Created two days in: open as of no earlier day.
    (
        "gamma",
        "g1",
        Some(SEVERITY_LOW),
        Some("Resource leak"),
        STATUS_OPEN,
        Some(DAY_ZERO + 2 * DAY + NOON),
        None,
    ),
    // Closed four days after it was created.
    (
        "gamma",
        "g2",
        Some(SEVERITY_INFO),
        Some("Log injection"),
        STATUS_CLOSED,
        Some(DAY_ZERO + NOON),
        Some(DAY_ZERO + 4 * DAY + NOON),
    ),
    // Closed at the FIRST instant of day 1: open as of day 0 and no later day.
    (
        "delta",
        "d1",
        Some(SEVERITY_CRITICAL),
        Some("SQL injection"),
        STATUS_CLOSED,
        Some(DAY_ZERO + NOON),
        Some(DAY_ZERO + DAY),
    ),
    // No creation instant: nothing places it after a day, so it is open as of every day
    // up to the one it was closed on.
    (
        "epsilon",
        "e1",
        Some(SEVERITY_MEDIUM),
        Some("Insecure hashing"),
        STATUS_CLOSED,
        None,
        Some(DAY_ZERO + 2 * DAY + NOON),
    ),
    // Neither a severity nor a detector: counted against its scan only, and closed five
    // days in.
    (
        "zeta",
        "z1",
        None,
        None,
        STATUS_CLOSED,
        Some(DAY_ZERO + NOON),
        Some(DAY_ZERO + 5 * DAY + NOON),
    ),
];

/// What the seeded store adds up to as of the end of one day.
struct DayCase {
    /// The day, as the number of days after [`DAY_ZERO`].
    index: i64,
    /// `openFindings` as `(info, low, medium, high, critical)`.
    open: [f64; 5],
    categories: &'static [(&'static str, i64)],
    scans: &'static [(&'static str, i64)],
    critical_scans: &'static [(&'static str, i64)],
}

/// Every day from before the first finding to after the last close, as the rule adds the
/// seeded store up. Equal counts are ranked by name, so each row's order is the one the
/// handler reports.
const DAY_CASES: &[DayCase] = &[
    // Before anything was created, only the finding that records NO creation instant can
    // be open — it was not closed until day 2.
    DayCase {
        index: -1,
        open: [0.0, 0.0, 1.0, 0.0, 0.0],
        categories: &[("Insecure hashing", 1)],
        scans: &[("epsilon", 1)],
        critical_scans: &[],
    },
    // The day the findings were created: the one closed at the first instant of the next
    // day still counts, the one that records no close does not.
    DayCase {
        index: 0,
        open: [1.0, 0.0, 1.0, 1.0, 3.0],
        categories: &[
            ("SQL injection", 3),
            ("Hardcoded credentials", 1),
            ("Insecure hashing", 1),
            ("Log injection", 1),
        ],
        scans: &[("alpha", 2), ("beta", 1), ("delta", 1)],
        critical_scans: &[("alpha", 1), ("beta", 1), ("delta", 1)],
    },
    // One critical closed at the start of this day drops out; the finding created and
    // closed within this day was never open as of any day.
    DayCase {
        index: 1,
        open: [1.0, 0.0, 1.0, 1.0, 2.0],
        categories: &[
            ("SQL injection", 2),
            ("Hardcoded credentials", 1),
            ("Insecure hashing", 1),
            ("Log injection", 1),
        ],
        scans: &[("alpha", 2), ("beta", 1), ("epsilon", 1)],
        critical_scans: &[("alpha", 1), ("beta", 1)],
    },
    // Two findings close during this day and one is created on it.
    DayCase {
        index: 2,
        open: [1.0, 1.0, 0.0, 1.0, 1.0],
        categories: &[
            ("Hardcoded credentials", 1),
            ("Log injection", 1),
            ("Resource leak", 1),
            ("SQL injection", 1),
        ],
        scans: &[("gamma", 2), ("alpha", 1), ("beta", 1)],
        critical_scans: &[("beta", 1)],
    },
    // The last critical closes at the last second of this day, so no scan has an open
    // critical finding as of it.
    DayCase {
        index: 3,
        open: [1.0, 1.0, 0.0, 1.0, 0.0],
        categories: &[
            ("Hardcoded credentials", 1),
            ("Log injection", 1),
            ("Resource leak", 1),
        ],
        scans: &[("gamma", 2), ("alpha", 1), ("zeta", 1)],
        critical_scans: &[],
    },
    DayCase {
        index: 4,
        open: [0.0, 1.0, 0.0, 1.0, 0.0],
        categories: &[("Hardcoded credentials", 1), ("Resource leak", 1)],
        scans: &[("alpha", 1), ("gamma", 1), ("zeta", 1)],
        critical_scans: &[],
    },
    // Only the two findings that were never closed are open as of this day.
    DayCase {
        index: 5,
        open: [0.0, 1.0, 0.0, 1.0, 0.0],
        categories: &[("Hardcoded credentials", 1), ("Resource leak", 1)],
        scans: &[("alpha", 1), ("gamma", 1)],
        critical_scans: &[],
    },
];

/// `rows` as the ranking helper reports a ranking.
fn expected_ranking(rows: &[(&str, i64)]) -> Vec<(String, i64)> {
    rows.iter()
        .map(|(name, count)| ((*name).to_string(), *count))
        .collect()
}

/// `case.open` as the per-severity reader reports `openFindings`.
fn expected_open(case: &DayCase) -> Vec<(&'static str, f64)> {
    vec![
        (SEVERITY_INFO, case.open[0]),
        (SEVERITY_LOW, case.open[1]),
        (SEVERITY_MEDIUM, case.open[2]),
        (SEVERITY_HIGH, case.open[3]),
        (SEVERITY_CRITICAL, case.open[4]),
    ]
}

#[tokio::test]
async fn the_summary_counts_a_finding_closed_after_the_day_it_asks_about() {
    let (engine, router) = harness();
    seed(&engine, SEEDED);

    for case in DAY_CASES {
        let label = format!("as of day {}", case.index);
        let at_secs = day(case.index) + NOON;
        let answer = get_metrics_summary(&router, at_secs).await;
        let summary = summary_of(&label, &answer);

        assert_eq!(
            instant_secs(&label, &summary["date"]),
            day(case.index),
            "case {label:?}: the summary is reported for the UTC day the date falls in, got {summary}"
        );
        assert_eq!(
            open_findings(&label, summary),
            expected_open(case),
            "case {label:?}: openFindings counts what was open as of the end of that day, got {summary}"
        );
        assert_eq!(
            ranking(
                &label,
                summary,
                "categoriesWithMostFindings",
                "categoryName"
            ),
            expected_ranking(case.categories),
            "case {label:?}: the categories rank what was open as of that day, got {summary}"
        );
        assert_eq!(
            ranking(&label, summary, "scansWithMostOpenFindings", "scanName"),
            expected_ranking(case.scans),
            "case {label:?}: the scans rank what was open as of that day, got {summary}"
        );
        assert_eq!(
            ranking(
                &label,
                summary,
                "scansWithMostOpenCriticalFindings",
                "scanName"
            ),
            expected_ranking(case.critical_scans),
            "case {label:?}: the scans rank the criticals open as of that day, got {summary}"
        );
    }
}

/// The per-day metrics of the interval that spans [`DAY_CASES`], as one page.
async fn findings_metrics_over_the_cases(router: &MultiServiceRouter) -> Vec<Value> {
    let first = DAY_CASES
        .first()
        .expect("the day table has at least one row")
        .index;
    let last = DAY_CASES
        .last()
        .expect("the day table has at least one row")
        .index;
    let answer: Answer = call(
        router,
        "GET",
        &format!(
            "/metrics/findings?startDate={}&endDate={}",
            iso(day(first) + NOON),
            iso(day(last) + NOON)
        ),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "the interval that spans the day table is answered, got {} {}",
        answer.status, answer.json
    );
    answer
        .get("findingsMetrics")
        .as_array()
        .unwrap_or_else(|| panic!("findingsMetrics must be an array, got {}", answer.json))
        .clone()
}

#[tokio::test]
async fn both_metrics_operations_report_the_same_open_findings_for_one_day() {
    let (engine, router) = harness();
    seed(&engine, SEEDED);

    let per_day = findings_metrics_over_the_cases(&router).await;
    assert_eq!(
        per_day.len(),
        DAY_CASES.len(),
        "case \"one metric per day\": expected {} days, got {}",
        DAY_CASES.len(),
        per_day.len()
    );

    for (case, metric) in DAY_CASES.iter().zip(&per_day) {
        let label = format!("both operations, day {}", case.index);
        assert_eq!(
            instant_secs(&label, &metric["date"]),
            day(case.index),
            "case {label:?}: each day of the interval is reported in order, got {metric}"
        );

        let answer = get_metrics_summary(&router, day(case.index) + NOON).await;
        let summary = summary_of(&label, &answer);
        // Both members answer "the number of open findings of each severity as of the
        // specified date" over one store, so one day cannot have two answers.
        assert_eq!(
            open_findings(&label, summary),
            open_findings(&label, metric),
            "case {label:?}: the one-day summary and the per-day metric must agree, got {summary} and {metric}"
        );
        assert_eq!(
            open_findings(&label, metric),
            expected_open(case),
            "case {label:?}: the per-day metric counts what was open as of that day, got {metric}"
        );
    }
}
