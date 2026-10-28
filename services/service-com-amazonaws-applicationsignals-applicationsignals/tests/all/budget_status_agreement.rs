// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the ONE budget status a stored service level objective has.
//!
//! The rule under test: the budget status of one stored objective over one period is a single
//! derived quantity, so every read that reports it — `BatchGetServiceLevelObjectiveBudgetReport`
//! as `BudgetStatus`, and `ListAuditFindings` as the SLO auditor's `BudgetStatus` data entry —
//! reports the same value, derived from the objective's goal, its evaluation interval, its
//! creation and the elapsed part of the period asked about.
//!
//! Every row drives both reads over the SAME objective and the SAME instant and compares their
//! answers; the rows differ along the interval kind and span, the attainment goal, the warning
//! threshold and the indicator the objective carries, so they span three of the modelled
//! `ServiceLevelObjectiveBudgetStatus` values.
//!
//! Every objective is created over HTTP first, so each reported status is a round trip through
//! the stored record.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::slo_lifecycle::{call, router};

/// The modelled `ServiceLevelObjectiveBudgetStatus` values a read of this emulator reaches
/// (`smithy_query(shape="ServiceLevelObjectiveBudgetStatus")` -> `["OK", "WARNING",
/// "BREACHED", "INSUFFICIENT_DATA"]`).
const STATUS_OK: &str = "OK";
/// [`STATUS_OK`], for a remaining budget under the objective's warning threshold.
const STATUS_WARNING: &str = "WARNING";
/// [`STATUS_OK`], for a period nothing was evaluated in.
const STATUS_INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";

/// The `Severity` each status earns (`smithy_query(shape="Severity")` -> `["CRITICAL",
/// "HIGH", "MEDIUM", "LOW", "NONE"]`): the modelled value for no issue, and the one a warning
/// earns.
const SEVERITY_NONE: &str = "NONE";
/// [`SEVERITY_NONE`], for the warning the objective's own goal asked to be told about.
const SEVERITY_WARNING: &str = "MEDIUM";

/// Seconds in a day, the unit the rows state their anchors in.
const DAY_SECONDS: i64 = 86_400;

/// How far ahead of now the reads ask about, so the part of the period that has elapsed is
/// the one from the objective's creation to now rather than an empty one.
const AHEAD_SECONDS: i64 = 3_600;

/// Epoch seconds now.
fn now_seconds() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_secs(),
    )
    .expect("epoch seconds fit an i64")
}

/// A rolling `Goal.Interval`.
fn rolling(duration_unit: &str, duration: i64) -> Value {
    json!({ "RollingInterval": { "DurationUnit": duration_unit, "Duration": duration } })
}

/// A calendar `Goal.Interval`, anchored at an epoch-seconds `StartTime`.
fn calendar(duration_unit: &str, duration: i64, start_time: i64) -> Value {
    json!({
        "CalendarInterval": {
            "StartTime": start_time,
            "DurationUnit": duration_unit,
            "Duration": duration
        }
    })
}

/// A period-based `SliConfig` on an Application Signals service operation.
fn sli_config(name: &str) -> Value {
    json!({
        "SliMetricConfig": {
            "KeyAttributes": { "Type": "Service", "Name": name, "Environment": "prod" },
            "OperationName": "GET /orders",
            "MetricType": "LATENCY"
        },
        "MetricThreshold": 250.0,
        "ComparisonOperator": "LessThan"
    })
}

/// A request-based `RequestBasedSliConfig` on the same kind of entity.
fn request_based_sli_config(name: &str) -> Value {
    json!({
        "RequestBasedSliMetricConfig": {
            "KeyAttributes": { "Type": "Service", "Name": name, "Environment": "prod" },
            "MetricType": "AVAILABILITY"
        }
    })
}

/// The `BudgetStatus` and `Attainment` the budget report states for `name` as of `timestamp`.
async fn reported_budget(
    router: &MultiServiceRouter,
    label: &str,
    name: &str,
    timestamp: i64,
) -> (String, Value) {
    let (status, answer) = call(
        router,
        "POST",
        "/budget-report",
        Some(json!({ "SloIds": [name], "Timestamp": timestamp })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected the budget report to answer 200, got {status} with {answer:?}"
    );
    let report = &answer["Reports"][0];
    assert_eq!(
        report["Name"].as_str(),
        Some(name),
        "case {label:?}: expected a report for {name}, got {answer:?}"
    );
    (
        report["BudgetStatus"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: expected a BudgetStatus, got {report:?}"))
            .to_string(),
        report["Attainment"].clone(),
    )
}

/// The SLO auditor's result for `name` over the period `(start, end)`.
async fn audited_result(
    router: &MultiServiceRouter,
    label: &str,
    name: &str,
    (start, end): (i64, i64),
) -> Value {
    let (status, answer) = call(
        router,
        "POST",
        &format!("/auditFindings?StartTime={start}&EndTime={end}"),
        Some(json!({
            "AuditTargets": [{ "Type": "slo", "Data": { "Slo": { "SloName": name } } }],
            "Auditors": ["slo"]
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected the audit to answer 200, got {status} with {answer:?}"
    );
    let result = answer["AuditFindings"][0]["AuditorResults"][0].clone();
    assert_eq!(
        result["Data"]["SloName"].as_str(),
        Some(name),
        "case {label:?}: expected a finding assessing {name}, got {answer:?}"
    );
    result
}

/// One row: an objective to store, and the one budget status both reads must report for it.
struct Case {
    /// The objective's name, which is also the case label.
    name: &'static str,
    /// The `Goal` the create states, or `None` to send none at all — the documented defaults
    /// then apply (a rolling interval of 7 days, an attainment goal of 99).
    goal: Option<Value>,
    /// Whether the objective carries the period-based indicator rather than the
    /// request-based one.
    period_based: bool,
    /// The `BudgetStatus` both reads must report.
    budget_status: &'static str,
    /// The `Severity` the audit result must carry for that status.
    severity: &'static str,
}

/// The rows, built against `now` because a calendar anchor is placed relative to it.
fn cases(now: i64) -> Vec<Case> {
    vec![
        Case {
            name: "default-goal",
            goal: None,
            period_based: true,
            budget_status: STATUS_OK,
            severity: SEVERITY_NONE,
        },
        Case {
            name: "rolling-ninety-minutes",
            goal: Some(json!({ "Interval": rolling("MINUTE", 90), "AttainmentGoal": 99.9 })),
            period_based: true,
            budget_status: STATUS_OK,
            severity: SEVERITY_NONE,
        },
        Case {
            name: "rolling-month",
            goal: Some(json!({ "Interval": rolling("MONTH", 1), "AttainmentGoal": 95.0 })),
            period_based: true,
            budget_status: STATUS_OK,
            severity: SEVERITY_NONE,
        },
        // An attainment goal that tolerates no unhealthy time leaves no budget to compare
        // against the threshold, and nothing of it has been spent.
        Case {
            name: "no-unhealthy-time",
            goal: Some(json!({ "AttainmentGoal": 100.0 })),
            period_based: true,
            budget_status: STATUS_OK,
            severity: SEVERITY_NONE,
        },
        // A threshold above the whole budget is one the intact budget is already below.
        Case {
            name: "threshold-above-the-budget",
            goal: Some(json!({ "AttainmentGoal": 99.0, "WarningThreshold": 150.0 })),
            period_based: true,
            budget_status: STATUS_WARNING,
            severity: SEVERITY_WARNING,
        },
        // A calendar tile that began well before now: the anchor is 40 days back, so the
        // tile containing the instant asked about began around ten days ago.
        Case {
            name: "calendar-tile-under-way",
            goal: Some(json!({
                "Interval": calendar("MONTH", 1, now - 40 * DAY_SECONDS),
                "AttainmentGoal": 99.0
            })),
            period_based: true,
            budget_status: STATUS_OK,
            severity: SEVERITY_NONE,
        },
        // A calendar interval whose first tile begins after the instant asked about: no
        // interval has begun, so nothing was evaluated.
        Case {
            name: "calendar-not-begun",
            goal: Some(json!({
                "Interval": calendar("MONTH", 1, now + 30 * DAY_SECONDS),
                "AttainmentGoal": 99.0
            })),
            period_based: true,
            budget_status: STATUS_INSUFFICIENT_DATA,
            severity: SEVERITY_NONE,
        },
        // A request-based objective's attainment is a quotient of request counts nothing has
        // reported.
        Case {
            name: "request-based",
            goal: None,
            period_based: false,
            budget_status: STATUS_INSUFFICIENT_DATA,
            severity: SEVERITY_NONE,
        },
    ]
}

/// The budget report and the SLO auditor report ONE budget status for one stored objective at
/// one instant, and it is the one the objective's goal, interval, creation and the elapsed
/// part of the period asked about earn.
#[tokio::test]
async fn the_audit_and_the_budget_report_state_one_budget_status() {
    let now = now_seconds();
    // Both reads ask about the same instant: the audit's period ends there, and the report is
    // asked for there.
    let asked_about = now + AHEAD_SECONDS;
    let period = (now - AHEAD_SECONDS, asked_about);

    for case in cases(now) {
        let router = router();
        let mut body = json!({ "Name": case.name });
        if case.period_based {
            body["SliConfig"] = sli_config(case.name);
        } else {
            body["RequestBasedSliConfig"] = request_based_sli_config(case.name);
        }
        if let Some(goal) = case.goal.clone() {
            body["Goal"] = goal;
        }
        let (status, created) = call(&router, "POST", "/slo", Some(body)).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the create to answer 200, got {status} with {created:?}",
            case.name
        );

        let (reported, attainment) =
            reported_budget(&router, case.name, case.name, asked_about).await;
        let audited = audited_result(&router, case.name, case.name, period).await;
        let assessed = audited["Data"]["BudgetStatus"].as_str();

        assert_eq!(
            reported, case.budget_status,
            "case {:?}: expected the budget report to state {}, got {reported:?}",
            case.name, case.budget_status
        );
        assert_eq!(
            assessed,
            Some(case.budget_status),
            "case {:?}: expected the audit to state {}, got {assessed:?}",
            case.name,
            case.budget_status
        );
        assert_eq!(
            assessed,
            Some(reported.as_str()),
            "case {:?}: expected both reads to state one budget status, got {reported:?} from \
             the report and {assessed:?} from the audit",
            case.name
        );
        assert_eq!(
            audited["Severity"].as_str(),
            Some(case.severity),
            "case {:?}: expected severity {} for status {}, got {:?}",
            case.name,
            case.severity,
            case.budget_status,
            audited["Severity"]
        );
        // An attainment is reported exactly where one was evaluated, which is where the
        // status is not the one for missing attainment data.
        assert_eq!(
            attainment.is_null(),
            case.budget_status == STATUS_INSUFFICIENT_DATA,
            "case {:?}: expected the report's Attainment to be absent={}, got {attainment:?}",
            case.name,
            case.budget_status == STATUS_INSUFFICIENT_DATA
        );
        // The description states the verdict the status reached, so no part of the result
        // contradicts another: only the status for missing attainment data says none was
        // recorded.
        let description = audited["Description"].as_str().unwrap_or_default();
        assert_eq!(
            description.contains("No attainment datapoint is recorded"),
            case.budget_status == STATUS_INSUFFICIENT_DATA,
            "case {:?}: expected the description of status {} not to contradict it, got \
             {description:?}",
            case.name,
            case.budget_status
        );
        assert!(
            description.contains(case.name),
            "case {:?}: expected the description to name the assessed objective, got \
             {description:?}",
            case.name
        );
    }
}

/// The audited status follows the PERIOD the request asks about: with one stored objective,
/// a period whose end is within its interval's reach of now is assessed on its budget, while a
/// period ending further ahead than that interval spans has no elapsed part to assess — and
/// the budget report asked for the same two instants answers the same way.
///
/// One stored objective, two instants: the finding is not a constant.
#[tokio::test]
async fn the_audited_budget_status_follows_the_period_asked_about() {
    let router = router();
    let now = now_seconds();
    // A one-minute rolling interval: the interval is the minute ending at the instant asked
    // about, so an instant far enough ahead of now puts the whole interval in the future.
    let body = json!({
        "Name": "tiled-objective",
        "SliConfig": sli_config("orders"),
        "Goal": { "Interval": rolling("MINUTE", 1), "AttainmentGoal": 99.0 }
    });
    let (status, created) = call(&router, "POST", "/slo", Some(body)).await;
    assert_eq!(
        status, 200,
        "expected the create to answer 200, got {status} with {created:?}"
    );

    let rows = [
        (
            "a period ending within the interval's reach of now",
            now + 30,
            STATUS_OK,
        ),
        (
            "a period ending further ahead than the interval spans",
            now + AHEAD_SECONDS,
            STATUS_INSUFFICIENT_DATA,
        ),
    ];

    for (label, asked_about, expected) in rows {
        let audited =
            audited_result(&router, label, "tiled-objective", (now - AHEAD_SECONDS, asked_about))
                .await;
        let assessed = audited["Data"]["BudgetStatus"].as_str();
        assert_eq!(
            assessed,
            Some(expected),
            "case {label:?}: expected budget status {expected}, got {assessed:?}"
        );
        let (reported, _) = reported_budget(&router, label, "tiled-objective", asked_about).await;
        assert_eq!(
            assessed,
            Some(reported.as_str()),
            "case {label:?}: expected both reads to state one budget status, got {reported:?} \
             from the report and {assessed:?} from the audit"
        );
    }
}
