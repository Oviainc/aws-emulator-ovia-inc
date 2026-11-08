// SPDX-License-Identifier: CC0-1.0

//! `GetBackupPlan`'s `ScheduledRunsPreview`: the upcoming runs a plan read
//! reports, computed from the stored rules and the clock.
//!
//! The rules under test:
//!
//! * a plan's upcoming runs are the instants its rules' cron expressions name
//!   after the moment of the call, merged across the rules into one
//!   chronological list and cut at the number of runs the caller asked for;
//! * a rule that carries no `ScheduleExpression` is scheduled by the documented
//!   default (`cron(0 5 ? * * *)`);
//! * an expression is read in the rule's `ScheduleExpressionTimezone`, and in
//!   UTC where the rule names none;
//! * a run carries the id of the rule that schedules it and the kind of backup
//!   that rule creates — continuous where the rule enables continuous backup,
//!   snapshots where it does not;
//! * the member is reported ONLY to a caller whose `MaxScheduledRunsPreview` is
//!   greater than 0, and the model's range (0-10) bounds what may be asked for;
//! * computing a preview is part of a READ: it stores nothing, so the plan a
//!   later caller reads is the plan that was created.
//!
//! A rule whose expression names no run at all is not among the rows: a plan
//! body carrying one is refused by the create and the update
//! (`plan_and_selection_refusals.rs`), so no stored rule can have one.
//!
//! Each row asserts the SHAPE of the schedule it names (the time of day its runs
//! fall at, the spacing between them, the weekdays they may fall on) rather than
//! any one instant, so the suite pins the rule and not the day it ran on.
//!
//! The request harness (`router`, `call`, `error_code`) is the one
//! `vault_and_backup_job.rs` exposes to this test binary, so there is ONE wire
//! path for the service's suites.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::vault_and_backup_job::{call, error_code, router};

/// Seconds in a day — the unit the date arithmetic below splits an instant with.
const SECONDS_PER_DAY: u64 = 86_400;

/// Seconds in an hour.
const SECONDS_PER_HOUR: u64 = 3_600;

/// The most runs a preview may be asked for: the maximum of the model's
/// `@range` on `MaxScheduledRunsPreview` ("Valid range is 0-10").
const MAX_SCHEDULED_RUNS_PREVIEW: i64 = 10;

/// The least a preview may be asked for: the minimum of that same range, and
/// the member's default.
const MIN_SCHEDULED_RUNS_PREVIEW: i64 = 0;

/// `RuleExecutionType` of a rule that enables continuous backup (model enum
/// `RuleExecutionType`).
const CONTINUOUS: &str = "CONTINUOUS";

/// `RuleExecutionType` of a rule that does not (model enum `RuleExecutionType`).
const SNAPSHOTS: &str = "SNAPSHOTS";

/// One rule of a table row's plan: what it schedules, where, and whether its
/// backups are continuous.
struct RuleSpec {
    rule_name: &'static str,
    schedule: Option<&'static str>,
    timezone: Option<&'static str>,
    continuous: bool,
}

/// One row of the preview table.
struct PreviewCase {
    case: &'static str,
    rules: &'static [RuleSpec],
    /// What the caller asks for in `MaxScheduledRunsPreview`.
    max_runs: i64,
    /// How many runs the preview must carry.
    expected_runs: usize,
    /// The UTC second-of-day every run must fall at, where the row's schedule
    /// names one instant per day.
    second_of_day: Option<u64>,
    /// The gap between consecutive runs, where the row's schedule is evenly
    /// spaced.
    spacing: Option<u64>,
    /// The weekdays (0 = Sunday) a run may fall on, where the row's schedule
    /// restricts them.
    weekdays: Option<&'static [u64]>,
}

/// The table: the rows differ in the form of the schedule expression (wrapped
/// in `cron(…)`, bare, absent), in its period (daily, hourly, weekdays only),
/// in the time zone it is read in, in the kind of backup the rule creates, in
/// how many rules the plan carries and in how many runs the caller asks for.
const PREVIEW_CASES: &[PreviewCase] = &[
    PreviewCase {
        case: "a rule with no schedule expression runs at the documented default hour",
        rules: &[RuleSpec {
            rule_name: "defaulted",
            schedule: None,
            timezone: None,
            continuous: false,
        }],
        max_runs: 3,
        expected_runs: 3,
        second_of_day: Some(5 * SECONDS_PER_HOUR),
        spacing: Some(SECONDS_PER_DAY),
        weekdays: None,
    },
    PreviewCase {
        case: "an hourly cron, previewed to the model's maximum",
        rules: &[RuleSpec {
            rule_name: "hourly",
            schedule: Some("cron(0 * * * ? *)"),
            timezone: None,
            continuous: false,
        }],
        max_runs: MAX_SCHEDULED_RUNS_PREVIEW,
        expected_runs: MAX_SCHEDULED_RUNS_PREVIEW as usize,
        second_of_day: None,
        spacing: Some(SECONDS_PER_HOUR),
        weekdays: None,
    },
    PreviewCase {
        case: "a bare six-field expression, the spelling the model's own examples use",
        rules: &[RuleSpec {
            rule_name: "noon",
            schedule: Some("0 12 * * ? *"),
            timezone: None,
            continuous: false,
        }],
        max_runs: 4,
        expected_runs: 4,
        second_of_day: Some(12 * SECONDS_PER_HOUR),
        spacing: Some(SECONDS_PER_DAY),
        weekdays: None,
    },
    PreviewCase {
        case: "a cron restricted to weekdays",
        rules: &[RuleSpec {
            rule_name: "weekdays",
            schedule: Some("cron(30 9 ? * MON-FRI *)"),
            timezone: None,
            continuous: false,
        }],
        max_runs: 6,
        expected_runs: 6,
        second_of_day: Some(9 * SECONDS_PER_HOUR + 30 * 60),
        spacing: None,
        // Monday through Friday, counting from Sunday as 0.
        weekdays: Some(&[1, 2, 3, 4, 5]),
    },
    PreviewCase {
        case: "a schedule read in a named time zone, not in UTC",
        rules: &[RuleSpec {
            rule_name: "tokyo-morning",
            schedule: Some("cron(0 5 ? * * *)"),
            // Asia/Tokyo is a fixed +09:00 zone, so 05:00 there is 20:00 UTC
            // the day before, every day of the year.
            timezone: Some("Asia/Tokyo"),
            continuous: false,
        }],
        max_runs: 3,
        expected_runs: 3,
        second_of_day: Some(20 * SECONDS_PER_HOUR),
        spacing: Some(SECONDS_PER_DAY),
        weekdays: None,
    },
    PreviewCase {
        case: "a rule whose backups are continuous",
        rules: &[RuleSpec {
            rule_name: "pitr",
            schedule: Some("cron(15 2 ? * * *)"),
            timezone: None,
            continuous: true,
        }],
        max_runs: 2,
        expected_runs: 2,
        second_of_day: Some(2 * SECONDS_PER_HOUR + 15 * 60),
        spacing: Some(SECONDS_PER_DAY),
        weekdays: None,
    },
    PreviewCase {
        case: "three rules, whose runs are merged into one chronological preview",
        rules: &[
            RuleSpec {
                rule_name: "every-quarter-hour",
                schedule: Some("cron(0/15 * * * ? *)"),
                timezone: None,
                continuous: false,
            },
            RuleSpec {
                rule_name: "nightly",
                schedule: Some("cron(0 1 ? * * *)"),
                timezone: None,
                continuous: true,
            },
            RuleSpec {
                rule_name: "monthly",
                schedule: Some("cron(0 3 1 * ? *)"),
                timezone: None,
                continuous: false,
            },
        ],
        max_runs: MAX_SCHEDULED_RUNS_PREVIEW,
        expected_runs: MAX_SCHEDULED_RUNS_PREVIEW as usize,
        second_of_day: None,
        spacing: None,
        weekdays: None,
    },
    PreviewCase {
        case: "a preview of one run",
        rules: &[RuleSpec {
            rule_name: "single",
            schedule: Some("cron(0 7 ? * * *)"),
            timezone: None,
            continuous: false,
        }],
        max_runs: 1,
        expected_runs: 1,
        second_of_day: Some(7 * SECONDS_PER_HOUR),
        spacing: None,
        weekdays: None,
    },
];

/// `PUT /backup/plans` — `CreateBackupPlan`'s HTTP binding.
async fn create_plan(router: &MultiServiceRouter, request: &Value) -> (u16, Value) {
    call(router, "PUT", "/backup/plans", Some(&request.to_string())).await
}

/// `GET /backup/plans/{BackupPlanId}` — `GetBackupPlan`'s HTTP binding, with the
/// optional `MaxScheduledRunsPreview` query parameter.
async fn get_plan(
    router: &MultiServiceRouter,
    plan_id: &str,
    max_runs: Option<i64>,
) -> (u16, Value) {
    let path = match max_runs {
        Some(max) => format!("/backup/plans/{plan_id}?MaxScheduledRunsPreview={max}"),
        None => format!("/backup/plans/{plan_id}"),
    };
    call(router, "GET", &path, None).await
}

/// The plan body a row's rules make.
fn plan_body(name: &str, rules: &[RuleSpec]) -> Value {
    let rules: Vec<Value> = rules
        .iter()
        .map(|rule| {
            let mut body = json!({
                "RuleName": rule.rule_name,
                "TargetBackupVaultName": "preview-vault",
                "EnableContinuousBackup": rule.continuous,
            });
            if let Some(schedule) = rule.schedule {
                body["ScheduleExpression"] = json!(schedule);
            }
            if let Some(timezone) = rule.timezone {
                body["ScheduleExpressionTimezone"] = json!(timezone);
            }
            body
        })
        .collect();
    json!({"BackupPlan": {"BackupPlanName": name, "Rules": rules}})
}

/// The whole-second epoch instant of one previewed run, with the member's own
/// name reported when it is missing or not a number.
fn execution_time(run: &Value, case: &str, index: usize) -> u64 {
    let seconds = run
        .get("ExecutionTime")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| {
            panic!("case {case:?}: run {index} must carry a numeric ExecutionTime, got {run:?}")
        });
    assert!(
        seconds > 0.0,
        "case {case:?}: run {index} must be an instant after the epoch, got {seconds}"
    );
    seconds as u64
}

/// The second-of-day `instant` falls at, in UTC.
fn second_of_day(instant: u64) -> u64 {
    instant % SECONDS_PER_DAY
}

/// The weekday `instant` falls on, counting from Sunday as 0: 1970-01-01 was a
/// Thursday, so the epoch day is offset by 4.
fn weekday(instant: u64) -> u64 {
    (instant / SECONDS_PER_DAY + 4) % 7
}

/// Every row of the table: the preview a plan read reports is the upcoming runs
/// of the plan's own rules.
#[tokio::test]
async fn a_preview_reports_the_upcoming_runs_of_the_plans_rules() {
    let router = router();
    for (index, case) in PREVIEW_CASES.iter().enumerate() {
        let name = format!("preview-plan-{index}");
        let (status, created) = create_plan(&router, &plan_body(&name, case.rules)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the plan must be created, got {status} {created:?}",
            case.case
        );
        let plan_id = created
            .get("BackupPlanId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        let before = ::std::time::SystemTime::now()
            .duration_since(::std::time::SystemTime::UNIX_EPOCH)
            .expect("the test clock is after the epoch")
            .as_secs();
        let (status, body) = get_plan(&router, &plan_id, Some(case.max_runs)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the plan must be readable, got {status} {body:?}",
            case.case
        );

        let runs = body
            .get("ScheduledRunsPreview")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: a caller asking for {} runs must be reported a preview, got {body:?}",
                    case.case, case.max_runs
                )
            });
        assert_eq!(
            runs.len(),
            case.expected_runs,
            "case {:?}: expected {} previewed runs, got {}: {runs:?}",
            case.case,
            case.expected_runs,
            runs.len()
        );

        // Every run belongs to one of the plan's own rules, and reports the kind
        // of backup that rule creates.
        let rules = body
            .get("BackupPlan")
            .and_then(|plan| plan.get("Rules"))
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let mut previous: Option<u64> = None;
        for (position, run) in runs.iter().enumerate() {
            let at = execution_time(run, case.case, position);
            assert!(
                at > before,
                "case {:?}: run {position} must be in the future (after {before}), got {at}",
                case.case
            );
            if let Some(previous) = previous {
                assert!(
                    at >= previous,
                    "case {:?}: run {position} must not precede the run before it \
                     ({previous}), got {at}",
                    case.case
                );
                if let Some(spacing) = case.spacing {
                    assert_eq!(
                        at - previous,
                        spacing,
                        "case {:?}: runs must be {spacing}s apart, got {}",
                        case.case,
                        at - previous
                    );
                }
            }
            if let Some(expected) = case.second_of_day {
                assert_eq!(
                    second_of_day(at),
                    expected,
                    "case {:?}: run {position} must fall at second {expected} of the UTC day, \
                     got {} (instant {at})",
                    case.case,
                    second_of_day(at)
                );
            }
            if let Some(weekdays) = case.weekdays {
                assert!(
                    weekdays.contains(&weekday(at)),
                    "case {:?}: run {position} must fall on one of the weekdays {weekdays:?}, \
                     got {} (instant {at})",
                    case.case,
                    weekday(at)
                );
            }

            let rule_id = run
                .get("RuleId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let rule = rules
                .iter()
                .find(|stored| stored.get("RuleId").and_then(Value::as_str) == Some(&rule_id))
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: run {position} must name a rule of the plan, got {rule_id:?} \
                         against {rules:?}",
                        case.case
                    )
                });
            let continuous = rule
                .get("EnableContinuousBackup")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let expected = if continuous { CONTINUOUS } else { SNAPSHOTS };
            assert_eq!(
                run.get("RuleExecutionType").and_then(Value::as_str),
                Some(expected),
                "case {:?}: run {position} of rule {rule_id:?} must report {expected}, got {run:?}",
                case.case
            );
            previous = Some(at);
        }
    }
}

/// The preview sizes that report NO preview at all: the member is "only included
/// when MaxScheduledRunsPreview parameter is greater than 0", and the member
/// defaults to 0, so a request that names nothing is the same case.
#[tokio::test]
async fn a_caller_asking_for_no_runs_is_reported_no_preview() {
    let router = router();
    let (status, created) = create_plan(
        &router,
        &plan_body(
            "no-preview-plan",
            &[RuleSpec {
                rule_name: "daily",
                schedule: Some("cron(0 5 ? * * *)"),
                timezone: None,
                continuous: false,
            }],
        ),
    )
    .await;
    assert_eq!(status, 200, "the plan must be created, got {created:?}");
    let plan_id = created
        .get("BackupPlanId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    for (case, max_runs) in [
        ("the member left out, so it defaults to 0", None),
        (
            "the minimum of the modelled range, asked for explicitly",
            Some(MIN_SCHEDULED_RUNS_PREVIEW),
        ),
    ] {
        let (status, body) = get_plan(&router, &plan_id, max_runs).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the plan must be readable, got {status} {body:?}"
        );
        assert!(
            body.get("ScheduledRunsPreview").is_none(),
            "case {case:?}: no preview may be reported, got {:?}",
            body.get("ScheduledRunsPreview")
        );
    }
}

/// A `MaxScheduledRunsPreview` past the modelled range is refused, and the
/// bounds of that range are not.
#[tokio::test]
async fn the_preview_size_honours_the_modelled_range() {
    let router = router();
    let (status, created) = create_plan(
        &router,
        &plan_body(
            "ranged-preview-plan",
            &[RuleSpec {
                rule_name: "daily",
                schedule: Some("cron(0 5 ? * * *)"),
                timezone: None,
                continuous: false,
            }],
        ),
    )
    .await;
    assert_eq!(status, 200, "the plan must be created, got {created:?}");
    let plan_id = created
        .get("BackupPlanId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    for (case, max_runs, accepted) in [
        ("the published minimum", MIN_SCHEDULED_RUNS_PREVIEW, true),
        ("the published maximum", MAX_SCHEDULED_RUNS_PREVIEW, true),
        (
            "one past the published maximum",
            MAX_SCHEDULED_RUNS_PREVIEW + 1,
            false,
        ),
        ("below the published minimum", -1, false),
    ] {
        let (status, body) = get_plan(&router, &plan_id, Some(max_runs)).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: {max_runs} is within the modelled range, got {status} {body:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: {max_runs} is outside the modelled range, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidParameterValueException",
                "case {case:?}: an out-of-range preview size is an invalid parameter value, \
                 got {body:?}"
            );
        }
    }
}

/// Computing a preview is part of a READ: it stores nothing, so the plan the
/// next caller reads is the plan that was created — the same body, the same
/// identifiers, and a preview again.
#[tokio::test]
async fn previewing_the_runs_changes_nothing_a_later_caller_sees() {
    let router = router();
    let body = plan_body(
        "stable-preview-plan",
        &[RuleSpec {
            rule_name: "hourly",
            schedule: Some("cron(0 * * * ? *)"),
            timezone: None,
            continuous: false,
        }],
    );
    let (status, created) = create_plan(&router, &body).await;
    assert_eq!(status, 200, "the plan must be created, got {created:?}");
    let plan_id = created
        .get("BackupPlanId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let (_, first) = get_plan(&router, &plan_id, Some(MAX_SCHEDULED_RUNS_PREVIEW)).await;
    let (_, second) = get_plan(&router, &plan_id, Some(MAX_SCHEDULED_RUNS_PREVIEW)).await;
    let (_, without) = get_plan(&router, &plan_id, None).await;

    for member in [
        "BackupPlan",
        "BackupPlanId",
        "BackupPlanArn",
        "VersionId",
        "CreationDate",
    ] {
        assert_eq!(
            first.get(member),
            second.get(member),
            "member {member:?} must read back the same after a preview, got {:?} then {:?}",
            first.get(member),
            second.get(member)
        );
        assert_eq!(
            first.get(member),
            without.get(member),
            "member {member:?} must not depend on the preview, got {:?} then {:?}",
            first.get(member),
            without.get(member)
        );
    }
    assert_eq!(
        first.get("ScheduledRunsPreview"),
        second.get("ScheduledRunsPreview"),
        "an hourly schedule read twice within the same minute previews the same runs, got {:?} \
         then {:?}",
        first.get("ScheduledRunsPreview"),
        second.get("ScheduledRunsPreview")
    );
    assert!(
        first.get("LastExecutionDate").is_none(),
        "previewing a run must not record one as having run, got {:?}",
        first.get("LastExecutionDate")
    );
}
