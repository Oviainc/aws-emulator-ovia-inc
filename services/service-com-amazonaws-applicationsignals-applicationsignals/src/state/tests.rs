// SPDX-License-Identifier: CC0-1.0

//! Unit tests for this service's state helpers.

use super::*;
use crate::ops::types::{CalendarInterval, ServiceLevelIndicator};

/// 2020-01-01T00:00:00Z, the anchor most rows below tile from.
const JAN_01_2020: i64 = 1_577_836_800;
/// 2020-01-15T00:00:00Z, a mid-month anchor.
const JAN_15_2020: i64 = 1_579_046_400;
/// 2020-01-31T00:00:00Z, an anchor on a day February does not have.
const JAN_31_2020: i64 = 1_580_428_800;
/// 2020-02-01T00:00:00Z.
const FEB_01_2020: i64 = 1_580_515_200;
/// 2020-02-13T00:00:00Z, a rolling week before [`FEB_20_2020`].
const FEB_13_2020: i64 = FEB_20_2020 - 7 * 86_400;
/// 2020-02-15T00:00:00Z, inside the rolling week ending at [`FEB_20_2020`].
const FEB_15_2020: i64 = FEB_20_2020 - 5 * 86_400;
/// 2020-02-20T00:00:00Z.
const FEB_20_2020: i64 = 1_582_156_800;
/// 2020-02-29T00:00:00Z, the last day of a leap February.
const FEB_29_2020: i64 = 1_582_934_400;
/// 2020-03-01T00:00:00Z.
const MAR_01_2020: i64 = 1_583_020_800;
/// 2021-01-01T00:00:00Z.
const JAN_01_2021: i64 = 1_609_459_200;
/// 2021-02-15T00:00:00Z.
const FEB_15_2021: i64 = 1_613_347_200;
/// 2021-02-20T00:00:00Z.
const FEB_20_2021: i64 = 1_613_779_200;
/// 2021-03-15T00:00:00Z.
const MAR_15_2021: i64 = 1_615_766_400;
/// 2021-04-01T00:00:00Z.
const APR_01_2021: i64 = 1_617_235_200;
/// 2021-05-10T00:00:00Z.
const MAY_10_2021: i64 = 1_620_604_800;
/// 2021-05-11T00:00:00Z.
const MAY_11_2021: i64 = 1_620_691_200;
/// 2021-07-01T00:00:00Z.
const JUL_01_2021: i64 = 1_625_097_600;
/// 2030-01-01T00:00:00Z, an anchor after every instant asked about below.
const JAN_01_2030: i64 = 1_893_456_000;

/// 1% of a rolling week, the budget an attainment goal of 99 leaves over the default
/// interval.
const WEEKLY_BUDGET_SECONDS: i32 = (7 * 86_400) / 100;

fn rolling(duration_unit: &str, duration: i32) -> Interval {
    Interval::RollingInterval(RollingInterval {
        duration_unit: duration_unit.to_string(),
        duration,
    })
}

fn calendar(duration_unit: &str, duration: i32, start_time: i64) -> Interval {
    Interval::CalendarInterval(CalendarInterval {
        start_time: DateTime::from_secs(start_time),
        duration_unit: duration_unit.to_string(),
        duration,
    })
}

/// One stored objective, as the create path would have written it: created at `created`,
/// carrying the period-based indicator unless `period_based` is false, and evaluated
/// against `goal`.
fn stored_slo(name: &str, goal: Goal, created: i64, period_based: bool) -> ServiceLevelObjective {
    ServiceLevelObjective {
        arn: format!("arn:aws:{SIGNING_NAME}:eu-west-2:000000000000:slo/{name}"),
        name: name.to_string(),
        description: None,
        created_time: DateTime::from_secs(created),
        last_updated_time: DateTime::from_secs(created),
        sli: period_based.then(ServiceLevelIndicator::default),
        request_based_sli: None,
        evaluation_type: None,
        goal,
        burn_rate_configurations: None,
        metric_source_type: None,
        auto_investigation_enabled: None,
    }
}

/// A `Goal` stating exactly the members a row varies; the stored record carries what the
/// request sent, and [`goal_with_defaults`] fills the rest in where the budget is derived.
fn goal(
    interval: Option<Interval>,
    attainment_goal: Option<f64>,
    warning_threshold: Option<f64>,
) -> Goal {
    Goal {
        interval,
        attainment_goal,
        warning_threshold,
    }
}

/// The `BudgetStatus` rule is the member's own — the remaining share of the budget
/// against the SLO's warning threshold — over EVERY share, including the spent budgets
/// no read of this emulator reaches while nothing is observed in breach.
#[test]
fn budget_status_follows_the_remaining_share() {
    let cases = [
        (3600, 3600, Some(50.0), BUDGET_STATUS_OK),
        // Exactly AT the threshold is not below it.
        (3600, 1800, Some(50.0), BUDGET_STATUS_OK),
        (3600, 1799, Some(50.0), BUDGET_STATUS_WARNING),
        (3600, 1, Some(50.0), BUDGET_STATUS_WARNING),
        // A stricter threshold warns earlier, a laxer one later.
        (3600, 3000, Some(90.0), BUDGET_STATUS_WARNING),
        (3600, 360, Some(5.0), BUDGET_STATUS_OK),
        // Nothing left of a budget that had something is the exhausted one.
        (3600, 0, Some(50.0), BUDGET_STATUS_BREACHED),
        (3600, -60, Some(50.0), BUDGET_STATUS_BREACHED),
        // A goal that tolerates no unhealthy time has no share to compare, and is
        // intact while nothing has been spent.
        (0, 0, Some(50.0), BUDGET_STATUS_OK),
        (0, -1, Some(50.0), BUDGET_STATUS_BREACHED),
        // No threshold configured: nothing to be below.
        (3600, 1, None, BUDGET_STATUS_OK),
    ];
    for (total, remaining, warning_threshold, expected) in cases {
        let status = budget_status_of(total, remaining, warning_threshold);
        assert_eq!(
            status, expected,
            "case {:?}: expected {expected}, got {status}",
            (total, remaining, warning_threshold)
        );
    }
}

/// One row of the interval table: what it states, the interval, the instant asked
/// about, and the `(start, end)` the interval resolves to.
struct IntervalCase {
    label: &'static str,
    interval: Interval,
    at: i64,
    resolved: Option<(i64, i64)>,
}

/// The evaluation interval of an instant: the rolling span ending at it, or the calendar
/// tile containing it — months stepped by the calendar, so a tile is as long as the
/// months it covers.
#[test]
fn evaluation_interval_spans_rolling_windows_and_calendar_tiles() {
    let cases = [
        IntervalCase {
            label: "a rolling week ends at the instant asked about",
            interval: rolling("DAY", 7),
            at: FEB_20_2020,
            resolved: Some((FEB_13_2020, FEB_20_2020)),
        },
        IntervalCase {
            label: "a rolling month is the 30-day month the doc measures a budget in",
            interval: rolling("MONTH", 1),
            at: FEB_20_2020,
            resolved: Some((FEB_20_2020 - 30 * 86_400, FEB_20_2020)),
        },
        IntervalCase {
            label: "a rolling span of minutes",
            interval: rolling("MINUTE", 90),
            at: FEB_20_2020,
            resolved: Some((FEB_20_2020 - 5_400, FEB_20_2020)),
        },
        IntervalCase {
            label: "a duration unit the service does not know spans nothing",
            interval: rolling("WEEK", 1),
            at: FEB_20_2020,
            resolved: None,
        },
        IntervalCase {
            label: "a monthly tile is the calendar month, 29 days in a leap February",
            interval: calendar("MONTH", 1, JAN_01_2020),
            at: FEB_20_2020,
            resolved: Some((FEB_01_2020, MAR_01_2020)),
        },
        IntervalCase {
            label: "a mid-month anchor tiles from its own day of the month",
            interval: calendar("MONTH", 1, JAN_15_2020),
            at: FEB_20_2021,
            resolved: Some((FEB_15_2021, MAR_15_2021)),
        },
        IntervalCase {
            label: "an anchor on the 31st ends on the last day the next month has",
            interval: calendar("MONTH", 1, JAN_31_2020),
            at: FEB_20_2020,
            resolved: Some((JAN_31_2020, FEB_29_2020)),
        },
        IntervalCase {
            label: "a three-month tile",
            interval: calendar("MONTH", 3, JAN_01_2021),
            at: MAY_10_2021,
            resolved: Some((APR_01_2021, JUL_01_2021)),
        },
        IntervalCase {
            label: "a daily tile, asked about at its own boundary",
            interval: calendar("DAY", 1, JAN_01_2021),
            at: MAY_10_2021,
            resolved: Some((MAY_10_2021, MAY_11_2021)),
        },
        IntervalCase {
            label: "the anchor instant itself falls in the first tile",
            interval: calendar("MONTH", 1, JAN_01_2020),
            at: JAN_01_2020,
            resolved: Some((JAN_01_2020, FEB_01_2020)),
        },
        IntervalCase {
            label: "an instant before the first tile falls in no interval",
            interval: calendar("MONTH", 1, JAN_01_2030),
            at: FEB_20_2020,
            resolved: None,
        },
    ];

    for case in &cases {
        let resolved = evaluation_interval(&case.interval, case.at);
        assert_eq!(
            resolved, case.resolved,
            "case {:?}: expected {:?}, got {resolved:?}",
            case.label, case.resolved
        );
    }
}

/// One row of the assessment table: the stored record a read asks about, the period it
/// asks about, the instant the read is served at, and the assessment that follows.
struct AssessmentCase {
    label: &'static str,
    goal: Goal,
    period_based: bool,
    created: i64,
    /// The start of the period asked about, for a read that states one; `None` is the
    /// read that asks about an instant.
    period_start: Option<i64>,
    at: i64,
    now: i64,
    expected: BudgetAssessment,
}

/// The assessment of an evaluated window: the whole budget remains, so the status is the
/// one the remaining share earns against the goal's warning threshold.
fn intact(total: i32, budget_status: &'static str) -> BudgetAssessment {
    BudgetAssessment {
        budget_status,
        attainment: Some(FULL_ATTAINMENT),
        total_budget_seconds: Some(total),
        budget_seconds_remaining: Some(total),
    }
}

/// The assessment of a window nothing was evaluated in: `total` is the budget the goal
/// allows over the interval, which is a fact about the goal even so.
fn unevaluated(total: Option<i32>) -> BudgetAssessment {
    BudgetAssessment {
        budget_status: BUDGET_STATUS_INSUFFICIENT_DATA,
        attainment: None,
        total_budget_seconds: total,
        budget_seconds_remaining: None,
    }
}

/// The error budget of a stored objective follows its goal, its evaluation interval, its
/// creation and the elapsed part of the period asked about — and nothing else.
///
/// The rows differ along the interval kind and unit, the attainment goal, the warning
/// threshold, the indicator the objective carries, where its creation falls relative to
/// the period, where the period starts, and whether the read is served before the instant
/// it asks about.
#[test]
fn budget_assessment_follows_the_goal_and_the_evaluated_window() {
    let cases = [
        AssessmentCase {
            label: "the documented default goal over an elapsed week",
            goal: goal(None, None, None),
            period_based: true,
            created: JAN_01_2020,
            period_start: None,
            at: FEB_20_2020,
            now: MAR_01_2020,
            expected: intact(WEEKLY_BUDGET_SECONDS, BUDGET_STATUS_OK),
        },
        AssessmentCase {
            label: "a period beginning inside the interval still has an elapsed part",
            goal: goal(None, None, None),
            period_based: true,
            created: JAN_01_2020,
            period_start: Some(FEB_15_2020),
            at: FEB_20_2020,
            now: MAR_01_2020,
            expected: intact(WEEKLY_BUDGET_SECONDS, BUDGET_STATUS_OK),
        },
        AssessmentCase {
            label: "a period that begins where it ends has no elapsed part",
            goal: goal(None, None, None),
            period_based: true,
            created: JAN_01_2020,
            period_start: Some(FEB_20_2020),
            at: FEB_20_2020,
            now: MAR_01_2020,
            expected: unevaluated(Some(WEEKLY_BUDGET_SECONDS)),
        },
        AssessmentCase {
            label: "a period entirely before the objective was created",
            goal: goal(None, None, None),
            period_based: true,
            created: FEB_20_2020,
            period_start: Some(JAN_01_2020),
            at: JAN_15_2020,
            now: MAR_01_2020,
            expected: unevaluated(Some(WEEKLY_BUDGET_SECONDS)),
        },
        AssessmentCase {
            label: "a period ending at the instant the objective was created",
            goal: goal(None, None, None),
            period_based: true,
            created: FEB_20_2020,
            period_start: Some(JAN_01_2020),
            at: FEB_20_2020,
            now: MAR_01_2020,
            expected: unevaluated(Some(WEEKLY_BUDGET_SECONDS)),
        },
        AssessmentCase {
            label: "nothing after the instant the read is served at has elapsed",
            goal: goal(None, None, None),
            period_based: true,
            created: FEB_13_2020,
            period_start: None,
            at: MAR_01_2020,
            now: FEB_13_2020,
            expected: unevaluated(Some(WEEKLY_BUDGET_SECONDS)),
        },
        AssessmentCase {
            label: "a warning threshold above the whole budget warns on it",
            goal: goal(None, None, Some(150.0)),
            period_based: true,
            created: JAN_01_2020,
            period_start: Some(JAN_15_2020),
            at: FEB_20_2020,
            now: MAR_01_2020,
            expected: intact(WEEKLY_BUDGET_SECONDS, BUDGET_STATUS_WARNING),
        },
        AssessmentCase {
            label: "a threshold exactly at the whole budget is not below it",
            goal: goal(None, None, Some(100.0)),
            period_based: true,
            created: JAN_01_2020,
            period_start: Some(JAN_15_2020),
            at: FEB_20_2020,
            now: MAR_01_2020,
            expected: intact(WEEKLY_BUDGET_SECONDS, BUDGET_STATUS_OK),
        },
        AssessmentCase {
            label: "a goal tolerating no unhealthy time allows no budget at all",
            goal: goal(None, Some(100.0), None),
            period_based: true,
            created: JAN_01_2020,
            period_start: Some(JAN_15_2020),
            at: FEB_20_2020,
            now: MAR_01_2020,
            expected: intact(0, BUDGET_STATUS_OK),
        },
        AssessmentCase {
            label: "an hourly interval and a goal of 95 leave 5% of the hour",
            goal: goal(Some(rolling("HOUR", 1)), Some(95.0), None),
            period_based: true,
            created: JAN_01_2020,
            period_start: Some(JAN_15_2020),
            at: FEB_20_2020,
            now: MAR_01_2020,
            expected: intact(180, BUDGET_STATUS_OK),
        },
        AssessmentCase {
            label: "a calendar day tile, its budget the share of that day",
            goal: goal(Some(calendar("DAY", 1, JAN_01_2020)), Some(99.0), None),
            period_based: true,
            created: JAN_01_2020,
            period_start: Some(JAN_15_2020),
            at: FEB_20_2020 + 3_600,
            now: MAR_01_2020,
            expected: intact(864, BUDGET_STATUS_OK),
        },
        AssessmentCase {
            label: "a calendar interval whose first tile has not begun",
            goal: goal(Some(calendar("MONTH", 1, JAN_01_2030)), Some(99.0), None),
            period_based: true,
            created: JAN_01_2020,
            period_start: Some(JAN_15_2020),
            at: FEB_20_2020,
            now: MAR_01_2020,
            expected: unevaluated(None),
        },
        AssessmentCase {
            label: "a duration unit the service does not know spans no interval",
            goal: goal(Some(rolling("WEEK", 1)), Some(99.0), None),
            period_based: true,
            created: JAN_01_2020,
            period_start: Some(JAN_15_2020),
            at: FEB_20_2020,
            now: MAR_01_2020,
            expected: unevaluated(None),
        },
        AssessmentCase {
            label: "a request-based objective counts requests nothing reported",
            goal: goal(None, None, None),
            period_based: false,
            created: JAN_01_2020,
            period_start: Some(JAN_15_2020),
            at: FEB_20_2020,
            now: MAR_01_2020,
            expected: unevaluated(None),
        },
    ];

    for case in &cases {
        let slo = stored_slo(
            "assessed-objective",
            case.goal.clone(),
            case.created,
            case.period_based,
        );
        let period_start = case.period_start.map(DateTime::from_secs);
        let assessment = budget_assessment(
            &slo,
            period_start.as_ref(),
            &DateTime::from_secs(case.at),
            &DateTime::from_secs(case.now),
        );
        assert_eq!(
            assessment, case.expected,
            "case {:?}: expected {:?}, got {assessment:?}",
            case.label, case.expected
        );
    }
}

/// The SLO auditor's result states the SAME assessment the budget rule reaches for the
/// same record over the same period: its `BudgetStatus` entry is that status, and its
/// severity and description are of that status rather than of a fixed verdict.
///
/// The rows differ along the goal and the indicator, so they reach three of the modelled
/// budget statuses, and along where the audited period falls relative to the objective.
#[test]
fn the_slo_auditor_reports_the_assessed_budget_status() {
    // Each row: the label, the goal, whether the objective is period-based, the period
    // audited, and the status and severity the result must state.
    let cases = [
        (
            "an elapsed period of a period-based objective",
            goal(None, None, None),
            true,
            (JAN_01_2020, FEB_20_2020),
            BUDGET_STATUS_OK,
            SEVERITY_NONE,
        ),
        (
            "a warning threshold above the whole budget",
            goal(None, None, Some(150.0)),
            true,
            (JAN_01_2020, FEB_20_2020),
            BUDGET_STATUS_WARNING,
            SEVERITY_WARNING,
        ),
        (
            "a period audited before the objective was created",
            goal(None, None, None),
            true,
            (JAN_01_2020, JAN_15_2020),
            BUDGET_STATUS_INSUFFICIENT_DATA,
            SEVERITY_NONE,
        ),
        (
            "a request-based objective, whose requests nothing reported",
            goal(None, None, None),
            false,
            (JAN_01_2020, FEB_20_2020),
            BUDGET_STATUS_INSUFFICIENT_DATA,
            SEVERITY_NONE,
        ),
    ];

    for (label, row_goal, period_based, (period_start, period_end), status, severity) in cases {
        // Created between the two periods the rows audit, so one covers an elapsed part
        // of this objective's life and the other ends before it existed.
        let slo = stored_slo("audited-objective", row_goal, FEB_01_2020, period_based);
        let window_start = DateTime::from_secs(period_start);
        let window_end = DateTime::from_secs(period_end);
        let now = DateTime::from_secs(MAR_01_2020);

        let result = slo_auditor_result(&slo, &window_start, &window_end, &now);
        let data = result.data.clone().unwrap_or_default();
        let assessed = budget_assessment(&slo, Some(&window_start), &window_end, &now);

        assert_eq!(
            data.get(AUDITOR_DATA_BUDGET_STATUS).map(String::as_str),
            Some(status),
            "case {label:?}: expected budget status {status}, got {:?}",
            data.get(AUDITOR_DATA_BUDGET_STATUS)
        );
        assert_eq!(
            data.get(AUDITOR_DATA_BUDGET_STATUS).map(String::as_str),
            Some(assessed.budget_status),
            "case {label:?}: expected the auditor to report the assessed status {}, got {:?}",
            assessed.budget_status,
            data.get(AUDITOR_DATA_BUDGET_STATUS)
        );
        assert_eq!(
            result.severity.as_deref(),
            Some(severity),
            "case {label:?}: expected severity {severity}, got {:?}",
            result.severity
        );
        assert_eq!(
            result.description.as_deref(),
            Some(assessed_description(&slo.name, status).as_str()),
            "case {label:?}: expected the description of status {status}, got {:?}",
            result.description
        );
        assert_eq!(
            data.get(AUDITOR_DATA_SLO_NAME).map(String::as_str),
            Some(slo.name.as_str()),
            "case {label:?}: expected the assessed objective's name, got {:?}",
            data.get(AUDITOR_DATA_SLO_NAME)
        );
        assert_eq!(
            data.get(AUDITOR_DATA_SLO_ARN).map(String::as_str),
            Some(slo.arn.as_str()),
            "case {label:?}: expected the assessed objective's ARN, got {:?}",
            data.get(AUDITOR_DATA_SLO_ARN)
        );
    }
}

/// Every modelled `ServiceLevelObjectiveBudgetStatus` earns the severity and the wording
/// of its own verdict: an issue is reported as one, and a status that reports no issue
/// claims none.
#[test]
fn each_budget_status_earns_the_severity_and_wording_of_its_verdict() {
    let cases = [
        (BUDGET_STATUS_OK, SEVERITY_NONE, "above the warning threshold"),
        (
            BUDGET_STATUS_WARNING,
            SEVERITY_WARNING,
            "below the warning threshold",
        ),
        (
            BUDGET_STATUS_BREACHED,
            SEVERITY_BREACHED,
            "has exhausted its error budget",
        ),
        (
            BUDGET_STATUS_INSUFFICIENT_DATA,
            SEVERITY_NONE,
            "No attainment datapoint is recorded",
        ),
    ];

    for (status, severity, wording) in cases {
        let assessed = assessed_severity(status);
        assert_eq!(
            assessed, severity,
            "case {status:?}: expected severity {severity}, got {assessed}"
        );
        let description = assessed_description("assessed-objective", status);
        assert!(
            description.contains(wording) && description.contains("assessed-objective"),
            "case {status:?}: expected a description naming the objective and stating \
                 {wording:?}, got {description:?}"
        );
    }
}
