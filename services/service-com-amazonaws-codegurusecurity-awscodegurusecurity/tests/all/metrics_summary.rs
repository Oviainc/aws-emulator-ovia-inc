// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `GetMetricsSummary`: the one-day summary the account's
//! stored findings add up to.
//!
//! Every request goes through the real router (`MultiServiceRouter::dispatch`), reusing
//! the harness of `crate::scan_lifecycle`, so the restJson1 wire shape and the shared
//! engine store are exercised exactly as a client would exercise them.
//!
//! The rule under test: a `date` inside the window the model documents for it is
//! answered with the summary of the UTC day that instant falls in — the account's OPEN
//! findings as of the END of that day, counted per severity, per finding category (the
//! detector that found it) and per scan, with the top 5 categories and the top 3 scans by
//! count — while a `date` outside that window fails a documented input constraint and is
//! refused with the modelled `ValidationException` (HTTP 400). Each case varies what the
//! rule generalises over: where in (or outside) the window the requested instant falls,
//! how many categories and scans the store holds, the severity and status of each
//! finding, and when each finding was created.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::Value;
use service_com_amazonaws_codegurusecurity_awscodegurusecurity::ops::get_metrics_summary::{
    CATEGORIES_WITH_MOST_FINDINGS, METRICS_SUMMARY_WINDOW_DAYS, SCANS_WITH_MOST_OPEN_FINDINGS,
};
use service_com_amazonaws_codegurusecurity_awscodegurusecurity::ops::types::Finding;
use service_com_amazonaws_codegurusecurity_awscodegurusecurity::state::{
    now_millis, start_of_utc_day_millis, MILLIS_PER_DAY, SEVERITY_CRITICAL, SEVERITY_HIGH,
    SEVERITY_INFO, SEVERITY_LOW, SEVERITY_MEDIUM, STATUS_OPEN,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::scan_lifecycle::{call, harness, store_of, Answer};

/// One day in seconds.
const DAY: i64 = 24 * 60 * 60;

/// The start of today's UTC day, in epoch seconds: the latest day
/// `GetMetricsSummaryRequest$date` may name. Every fixture day below is expressed
/// relative to it, so the suite keeps asking about days inside the window the handler
/// accepts however long after it was written it runs.
fn today_secs() -> i64 {
    start_of_utc_day_millis(now_millis()) / 1_000
}

/// The day the seeded store's first findings were created: thirty days before today, far
/// enough inside the window that the cases below can also ask about the day either side
/// of it.
fn day_a() -> i64 {
    today_secs() - 30 * DAY
}

/// Five days after [`day_a`], when one more finding was created.
fn day_b() -> i64 {
    day_a() + 5 * DAY
}

/// How close to a UTC midnight [`off_a_day_boundary`] refuses to run a case: the test
/// reads the clock to build its bounds and the handler reads it again, so a day that
/// rolls over between the two reads would move the window under an exact-bound case.
const DAY_ROLLOVER_MARGIN_MILLIS: i64 = 5_000;

/// Wait out a UTC midnight that is about to happen, so every case of this test computes
/// its bounds from the same day the handler will read. Waits at most
/// [`DAY_ROLLOVER_MARGIN_MILLIS`] and usually not at all.
async fn off_a_day_boundary() {
    let until_midnight = MILLIS_PER_DAY - now_millis().rem_euclid(MILLIS_PER_DAY);
    if until_midnight <= DAY_ROLLOVER_MARGIN_MILLIS {
        let wait = u64::try_from(until_midnight).unwrap_or(0) + 100;
        tokio::time::sleep(std::time::Duration::from_millis(wait)).await;
    }
}

/// A seeded finding created on [`day_a`], as the seed table spells a creation instant:
/// seconds after that day's start.
const AT_DAY_A: i64 = 0;

/// A seeded finding created on [`day_b`]; see [`AT_DAY_A`].
const AT_DAY_B: i64 = 5 * DAY;

/// The `Closed` status, from `types::shared_types::STATUS_VALUES`
/// (`All | Closed | Open`): a finding the summary must never count.
const STATUS_CLOSED: &str = "Closed";

/// An instant as the ISO 8601 form the modelled timestamp query members use. Shared with
/// the sibling suite of the other metrics operation (`crate::findings_metrics`).
pub(crate) fn iso(at_secs: i64) -> String {
    DateTime::from_secs(at_secs)
        .fmt(Format::DateTime)
        .unwrap_or_else(|error| panic!("unformattable instant {at_secs}: {error}"))
}

/// `GET /metrics/summary?date=<iso>`, the modelled route of `GetMetricsSummary`.
async fn get_metrics_summary(router: &MultiServiceRouter, at_secs: i64) -> Answer {
    call(
        router,
        "GET",
        &format!("/metrics/summary?date={}", iso(at_secs)),
        None,
    )
    .await
}

/// The `metricsSummary` of a successful answer.
fn summary_of<'a>(case: &str, answer: &'a Answer) -> &'a Value {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200, got {} {}",
        answer.status, answer.json
    );
    let summary = answer.get("metricsSummary");
    assert!(
        summary.is_object(),
        "case {case:?}: metricsSummary must be present as an object, got {}",
        answer.json
    );
    summary
}

/// The instant a reported `Timestamp` member carries, as epoch seconds.
///
/// restJson1 carries a `Timestamp` member either as epoch seconds or in the ISO 8601
/// form depending on its `timestampFormat`, so both renderings are read the same way.
/// Shared with `crate::findings_metrics`, which reads the same kind of member.
pub(crate) fn instant_secs(case: &str, value: &Value) -> i64 {
    match value {
        Value::String(text) => DateTime::from_str(text, Format::DateTime)
            .unwrap_or_else(|error| panic!("case {case:?}: unparseable instant {text:?}: {error}"))
            .as_secs_f64() as i64,
        Value::Number(seconds) => seconds
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: unparseable instant {seconds:?}")) as i64,
        other => panic!("case {case:?}: {other} is neither a timestamp string nor epoch seconds"),
    }
}

/// `openFindings` as `(severity, count)` pairs, in the order
/// `FindingMetricsValuePerSeverity` declares its members.
fn open_findings(case: &str, summary: &Value) -> Vec<(&'static str, f64)> {
    let open = &summary["openFindings"];
    assert!(
        open.is_object(),
        "case {case:?}: openFindings must be present as an object, got {summary}"
    );
    [
        ("info", SEVERITY_INFO),
        ("low", SEVERITY_LOW),
        ("medium", SEVERITY_MEDIUM),
        ("high", SEVERITY_HIGH),
        ("critical", SEVERITY_CRITICAL),
    ]
    .into_iter()
    .map(|(member, severity)| {
        let count = open[member].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: openFindings.{member} must be a number, got {open}")
        });
        (severity, count)
    })
    .collect()
}

/// A ranking member (`categoriesWithMostFindings`, `scansWithMostOpenFindings`,
/// `scansWithMostOpenCriticalFindings`) as `(name, findingNumber)` pairs, in the order
/// the answer listed them.
fn ranking(case: &str, summary: &Value, member: &str, name_key: &str) -> Vec<(String, i64)> {
    let entries = summary[member].as_array().unwrap_or_else(|| {
        panic!("case {case:?}: {member} must be present as an array, got {summary}")
    });
    entries
        .iter()
        .map(|entry| {
            let name = entry[name_key].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: {member} entry has no {name_key}: {entry}")
            });
            let count = entry["findingNumber"].as_i64().unwrap_or_else(|| {
                panic!("case {case:?}: {member} entry has no findingNumber: {entry}")
            });
            (name.to_string(), count)
        })
        .collect()
}

/// A finding as a scan run's analysis would have stored it. Seeded directly into the
/// store: no request path writes findings yet, so this is the only way to exercise the
/// metrics over a non-empty store.
fn seeded_finding(
    finding_id: &str,
    status: &str,
    severity: Option<&str>,
    detector_name: Option<&str>,
    created_at_secs: i64,
) -> Finding {
    Finding {
        id: Some(finding_id.to_string()),
        status: Some(status.to_string()),
        severity: severity.map(str::to_string),
        detector_name: detector_name.map(str::to_string),
        created_at: Some(DateTime::from_secs(created_at_secs)),
        title: Some(format!("seeded finding {finding_id}")),
        ..Finding::default()
    }
}

/// One seeded row: `(scanName, findingId, status, severity, detectorName,
/// secondsAfterDayA)` — the creation instant is relative to [`day_a`], so the fixture
/// ages with the clock.
type Seed = (
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
    Option<&'static str>,
    i64,
);

/// Put `seeds` in the store the handler reads.
fn seed(engine: &std::sync::Arc<engine::InMemoryEngine>, seeds: &[Seed]) {
    let store = store_of(engine);
    let mut guard = store.lock();
    for (scan_name, finding_id, status, severity, detector_name, after_day_a) in seeds {
        guard.findings.insert(
            ((*scan_name).to_string(), (*finding_id).to_string()),
            seeded_finding(
                finding_id,
                status,
                *severity,
                *detector_name,
                day_a() + *after_day_a,
            ),
        );
    }
}

/// The seeded store every aggregation case below reads: more categories than the
/// summary reports (8 > 5) and more scans than it ranks (5 > 3), both `Open` and
/// `Closed` findings, every modelled severity, and one finding created five days after
/// the others.
const SEEDED: &[Seed] = &[
    (
        "alpha",
        "a1",
        STATUS_OPEN,
        Some(SEVERITY_CRITICAL),
        Some("SQL injection"),
        AT_DAY_A,
    ),
    (
        "alpha",
        "a2",
        STATUS_OPEN,
        Some(SEVERITY_CRITICAL),
        Some("SQL injection"),
        AT_DAY_A,
    ),
    (
        "alpha",
        "a3",
        STATUS_OPEN,
        Some(SEVERITY_HIGH),
        Some("Hardcoded credentials"),
        AT_DAY_A,
    ),
    (
        "alpha",
        "a4",
        STATUS_CLOSED,
        Some(SEVERITY_CRITICAL),
        Some("SQL injection"),
        AT_DAY_A,
    ),
    (
        "alpha",
        "a5",
        STATUS_OPEN,
        Some(SEVERITY_MEDIUM),
        Some("Insecure hashing"),
        AT_DAY_B,
    ),
    (
        "beta",
        "b1",
        STATUS_OPEN,
        Some(SEVERITY_CRITICAL),
        Some("Cross-site scripting"),
        AT_DAY_A,
    ),
    (
        "beta",
        "b2",
        STATUS_OPEN,
        Some(SEVERITY_LOW),
        Some("Insecure hashing"),
        AT_DAY_A,
    ),
    (
        "beta",
        "b3",
        STATUS_OPEN,
        Some(SEVERITY_INFO),
        Some("Log injection"),
        AT_DAY_A,
    ),
    (
        "gamma",
        "g1",
        STATUS_OPEN,
        Some(SEVERITY_HIGH),
        Some("SQL injection"),
        AT_DAY_A,
    ),
    (
        "gamma",
        "g2",
        STATUS_OPEN,
        Some(SEVERITY_MEDIUM),
        Some("Weak crypto"),
        AT_DAY_A,
    ),
    (
        "delta",
        "d1",
        STATUS_OPEN,
        Some(SEVERITY_LOW),
        Some("Resource leak"),
        AT_DAY_A,
    ),
    (
        "epsilon",
        "e1",
        STATUS_OPEN,
        Some(SEVERITY_INFO),
        Some("Path traversal"),
        AT_DAY_A,
    ),
];

/// One aggregation case: the instant asked about, and the summary the seeded store adds
/// up to as of the end of that day.
struct Case {
    label: &'static str,
    /// The instant the request names (not necessarily a day boundary).
    at_secs: i64,
    /// `(info, low, medium, high, critical)`.
    open: (f64, f64, f64, f64, f64),
    categories: &'static [(&'static str, i64)],
    scans: &'static [(&'static str, i64)],
    critical_scans: &'static [(&'static str, i64)],
}

#[tokio::test]
async fn metrics_summary_counts_the_stored_findings_as_of_the_requested_day() {
    off_a_day_boundary().await;
    let (engine, router) = harness();
    seed(&engine, SEEDED);

    let cases = &[
        // Nothing existed yet: every count is zero and every ranking empty.
        Case {
            label: "the day before the first finding",
            at_secs: day_a() - DAY + DAY / 2,
            open: (0.0, 0.0, 0.0, 0.0, 0.0),
            categories: &[],
            scans: &[],
            critical_scans: &[],
        },
        // The day the first findings were created, asked about from the middle of that
        // day: the `Closed` finding is counted nowhere, the one created five days later
        // did not exist yet, the 8 categories are cut to the top 5 and the 5 scans to
        // the top 3, equal counts ordered by name.
        Case {
            label: "the day the first findings were created",
            at_secs: day_a() + 45_296,
            open: (2.0, 2.0, 1.0, 2.0, 3.0),
            categories: &[
                ("SQL injection", 3),
                ("Cross-site scripting", 1),
                ("Hardcoded credentials", 1),
                ("Insecure hashing", 1),
                ("Log injection", 1),
            ],
            scans: &[("alpha", 3), ("beta", 3), ("gamma", 2)],
            critical_scans: &[("alpha", 2), ("beta", 1)],
        },
        // Five days on, at the last second of the day: the medium finding created that
        // day joins its severity, its category and its scan.
        Case {
            label: "the day the fifth finding was created",
            at_secs: day_b() + DAY - 1,
            open: (2.0, 2.0, 2.0, 2.0, 3.0),
            categories: &[
                ("SQL injection", 3),
                ("Insecure hashing", 2),
                ("Cross-site scripting", 1),
                ("Hardcoded credentials", 1),
                ("Log injection", 1),
            ],
            scans: &[("alpha", 4), ("beta", 3), ("gamma", 2)],
            critical_scans: &[("alpha", 2), ("beta", 1)],
        },
        // The latest day the window reaches, long after every finding was created,
        // reports the same summary: the cutoff is a lower bound on what has been
        // created, not a second window.
        Case {
            label: "today, the latest day the window reaches",
            at_secs: today_secs() + DAY / 3,
            open: (2.0, 2.0, 2.0, 2.0, 3.0),
            categories: &[
                ("SQL injection", 3),
                ("Insecure hashing", 2),
                ("Cross-site scripting", 1),
                ("Hardcoded credentials", 1),
                ("Log injection", 1),
            ],
            scans: &[("alpha", 4), ("beta", 3), ("gamma", 2)],
            critical_scans: &[("alpha", 2), ("beta", 1)],
        },
    ];

    for case in cases {
        let answer = get_metrics_summary(&router, case.at_secs).await;
        let summary = summary_of(case.label, &answer);

        let expected_day = case.at_secs - case.at_secs.rem_euclid(DAY);
        assert_eq!(
            instant_secs(case.label, &summary["date"]),
            expected_day,
            "case {:?}: the summary is reported for the UTC day the date falls in, got {summary}",
            case.label
        );

        let expected_open = vec![
            (SEVERITY_INFO, case.open.0),
            (SEVERITY_LOW, case.open.1),
            (SEVERITY_MEDIUM, case.open.2),
            (SEVERITY_HIGH, case.open.3),
            (SEVERITY_CRITICAL, case.open.4),
        ];
        assert_eq!(
            open_findings(case.label, summary),
            expected_open,
            "case {:?}: openFindings counts the open findings of each severity, got {summary}",
            case.label
        );

        let expected = |rows: &[(&str, i64)]| -> Vec<(String, i64)> {
            rows.iter()
                .map(|(name, count)| ((*name).to_string(), *count))
                .collect()
        };
        assert_eq!(
            ranking(
                case.label,
                summary,
                "categoriesWithMostFindings",
                "categoryName"
            ),
            expected(case.categories),
            "case {:?}: the top {CATEGORIES_WITH_MOST_FINDINGS} categories by open findings, got {summary}",
            case.label
        );
        assert_eq!(
            ranking(case.label, summary, "scansWithMostOpenFindings", "scanName"),
            expected(case.scans),
            "case {:?}: the top {SCANS_WITH_MOST_OPEN_FINDINGS} scans by open findings, got {summary}",
            case.label
        );
        assert_eq!(
            ranking(
                case.label,
                summary,
                "scansWithMostOpenCriticalFindings",
                "scanName"
            ),
            expected(case.critical_scans),
            "case {:?}: the top {SCANS_WITH_MOST_OPEN_FINDINGS} scans by open critical findings, got {summary}",
            case.label
        );
    }
}

/// One open finding on [`day_a`], so a case also pins what the account held as of the
/// day it asks about.
fn seed_one_open_finding(engine: &std::sync::Arc<engine::InMemoryEngine>) {
    seed(
        engine,
        &[(
            "alpha",
            "a1",
            STATUS_OPEN,
            Some(SEVERITY_HIGH),
            Some("SQL injection"),
            AT_DAY_A,
        )],
    );
}

/// The earliest day `GetMetricsSummaryRequest$date` may name, in epoch seconds: the
/// published lower bound of the window the model documents ("The date must be within the
/// past two years", `METRICS_SUMMARY_WINDOW_DAYS` days back from today).
fn earliest_day_secs() -> i64 {
    today_secs() - METRICS_SUMMARY_WINDOW_DAYS * DAY
}

#[tokio::test]
async fn metrics_summary_answers_any_day_inside_the_documented_window() {
    off_a_day_boundary().await;
    let (engine, router) = harness();
    seed_one_open_finding(&engine);

    // Rows: where inside the window the requested instant falls — both published bounds
    // (the earliest day the window reaches and today), a day just inside each, and three
    // instants inside ONE day (its first second, the middle, its last second) which must
    // all report that same day.
    let cases: &[(&str, i64, f64)] = &[
        ("the earliest day of the window", earliest_day_secs(), 0.0),
        (
            "one day inside the earliest bound",
            earliest_day_secs() + DAY,
            0.0,
        ),
        ("the day before the findings", day_a() - DAY, 0.0),
        ("the first second of the day", day_a(), 1.0),
        ("the middle of the day", day_a() + DAY / 2, 1.0),
        ("the last second of the day", day_a() + DAY - 1, 1.0),
        ("the day the fifth finding exists", day_b(), 1.0),
        ("yesterday", today_secs() - DAY, 1.0),
        ("the first second of today", today_secs(), 1.0),
        ("the last second of today", today_secs() + DAY - 1, 1.0),
    ];

    for (label, at_secs, expected_high) in cases {
        let answer = get_metrics_summary(&router, *at_secs).await;
        let summary = summary_of(label, &answer);
        assert_eq!(
            instant_secs(label, &summary["date"]),
            at_secs - at_secs.rem_euclid(DAY),
            "case {label:?}: a date inside the window is answered for the UTC day it falls in, got {summary}"
        );
        let high = open_findings(label, summary)
            .into_iter()
            .find(|(severity, _)| *severity == SEVERITY_HIGH)
            .map(|(_, count)| count)
            .unwrap_or_default();
        assert_eq!(
            high, *expected_high,
            "case {label:?}: openFindings.high counts what existed as of that day, got {summary}"
        );
    }
}

#[tokio::test]
async fn metrics_summary_refuses_a_date_outside_the_documented_window() {
    off_a_day_boundary().await;
    let (engine, router) = harness();
    seed_one_open_finding(&engine);

    // Rows: how the requested date leaves the window — one day past each published
    // bound, then far outside it on either side, including the epoch itself (what a
    // client that sent an unset or a seconds-vs-milliseconds timestamp names).
    let cases: &[(&str, i64)] = &[
        ("one day before the earliest bound", earliest_day_secs() - DAY),
        (
            "the last second before the earliest bound",
            earliest_day_secs() - 1,
        ),
        ("tomorrow", today_secs() + DAY),
        (
            "the last second of tomorrow",
            today_secs() + 2 * DAY - 1,
        ),
        ("five years in the future", today_secs() + 5 * 365 * DAY),
        ("the epoch", 0),
        ("twenty years ago", today_secs() - 20 * 365 * DAY),
    ];

    for (label, at_secs) in cases {
        let answer = get_metrics_summary(&router, *at_secs).await;
        assert_eq!(
            answer.status, 400,
            "case {label:?}: a date outside the documented window is refused, got {} {}",
            answer.status, answer.json
        );
        assert!(
            answer.error_type.contains("ValidationException"),
            "case {label:?}: the refusal is the modelled ValidationException, got {:?}",
            answer.error_type
        );
        assert_eq!(
            answer.json["reason"].as_str(),
            Some("fieldValidationFailed"),
            "case {label:?}: an input constraint failed, got {}",
            answer.json
        );
        let fields = answer.json["fieldList"].as_array().cloned().unwrap_or_default();
        assert!(
            fields
                .iter()
                .any(|field| field["name"].as_str() == Some("date")),
            "case {label:?}: the refusal names the member that failed, got {}",
            answer.json
        );
        assert!(
            answer.json["metricsSummary"].is_null(),
            "case {label:?}: a refused request reports no summary, got {}",
            answer.json
        );
    }
}

#[tokio::test]
async fn metrics_summary_of_an_account_with_no_findings_reports_zeros() {
    let (_engine, router) = harness();

    let answer = get_metrics_summary(&router, day_a()).await;
    let label = "an account with no findings";
    let summary = summary_of(label, &answer);
    assert_eq!(
        open_findings(label, summary),
        vec![
            (SEVERITY_INFO, 0.0),
            (SEVERITY_LOW, 0.0),
            (SEVERITY_MEDIUM, 0.0),
            (SEVERITY_HIGH, 0.0),
            (SEVERITY_CRITICAL, 0.0),
        ],
        "case {label:?}: every severity is reported, as zero, got {summary}"
    );
    for (member, name_key) in [
        ("categoriesWithMostFindings", "categoryName"),
        ("scansWithMostOpenFindings", "scanName"),
        ("scansWithMostOpenCriticalFindings", "scanName"),
    ] {
        assert!(
            ranking(label, summary, member, name_key).is_empty(),
            "case {label:?}: {member} has nothing to rank, got {summary}"
        );
    }
}

#[tokio::test]
async fn a_finding_with_no_severity_or_detector_is_counted_only_against_its_scan() {
    let (engine, router) = harness();
    // Both members are optional on the modelled `Finding`, so a stored finding may
    // carry neither: it is an open finding of its scan, but of no severity bucket and
    // of no category ("A finding category is determined by the detector that detected
    // the finding", `CategoryWithFindingNum$categoryName`).
    seed(
        &engine,
        &[
            ("zeta", "z1", STATUS_OPEN, None, None, AT_DAY_A),
            ("zeta", "z2", STATUS_OPEN, None, None, AT_DAY_A),
            (
                "zeta",
                "z3",
                STATUS_CLOSED,
                Some(SEVERITY_CRITICAL),
                Some("SQL injection"),
                AT_DAY_A,
            ),
        ],
    );

    let label = "findings with neither severity nor detector";
    let answer = get_metrics_summary(&router, day_a()).await;
    let summary = summary_of(label, &answer);
    assert_eq!(
        open_findings(label, summary),
        vec![
            (SEVERITY_INFO, 0.0),
            (SEVERITY_LOW, 0.0),
            (SEVERITY_MEDIUM, 0.0),
            (SEVERITY_HIGH, 0.0),
            (SEVERITY_CRITICAL, 0.0),
        ],
        "case {label:?}: a finding of no modelled severity is in no severity count, got {summary}"
    );
    assert!(
        ranking(label, summary, "categoriesWithMostFindings", "categoryName").is_empty(),
        "case {label:?}: a finding with no detector belongs to no category, got {summary}"
    );
    assert_eq!(
        ranking(label, summary, "scansWithMostOpenFindings", "scanName"),
        vec![("zeta".to_string(), 2)],
        "case {label:?}: both open findings are counted against their scan, got {summary}"
    );
    assert!(
        ranking(
            label,
            summary,
            "scansWithMostOpenCriticalFindings",
            "scanName"
        )
        .is_empty(),
        "case {label:?}: the only critical finding is closed, got {summary}"
    );
}

#[tokio::test]
async fn reading_the_summary_does_not_change_what_a_later_read_reports() {
    let (engine, router) = harness();
    seed(&engine, SEEDED);

    let label = "two reads of one store";
    let first = get_metrics_summary(&router, day_b()).await;
    let second = get_metrics_summary(&router, day_b()).await;
    assert_eq!(
        summary_of(label, &first),
        summary_of(label, &second),
        "case {label:?}: a reporting operation answers the same question the same way"
    );

    // And the day asked about is still a whole day on the millisecond scale the store
    // keeps its instants on.
    let reported = instant_secs(label, &summary_of(label, &second)["date"]);
    assert!(
        reported.saturating_mul(1_000) % MILLIS_PER_DAY == 0,
        "case {label:?}: the reported date is the start of a UTC day, got {reported}"
    );

    let store = store_of(&engine);
    assert_eq!(
        store.lock().findings.len(),
        SEEDED.len(),
        "case {label:?}: the summary must not add or drop a finding"
    );
}
