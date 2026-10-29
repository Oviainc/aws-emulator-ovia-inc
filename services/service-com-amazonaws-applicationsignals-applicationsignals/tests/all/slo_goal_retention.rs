// SPDX-License-Identifier: CC0-1.0

//! What an `UpdateServiceLevelObjective` that supplies `Goal` does to the goal members it
//! does NOT name.
//!
//! The rule under test: each member of `Goal` is a parameter in its own right — the model
//! documents each one's own behaviour for being left out
//! (`smithy_query(query="shape",shape="Goal")`) — so an update that supplies the structure
//! changes only the members it names and retains the stored value of every member it omits,
//! per the operation's "If you omit parameters, the previous values of those parameters are
//! retained"; the documented default of an omitted parameter applies only where there is no
//! stored value, which is the create case.
//!
//! The rows differ along what the rule generalises over: which of the three members the
//! update names (one, two, all three, none at all), the kind of interval stored and the kind
//! supplied, the duration unit and count, and whether the stored values were sent by the
//! create or came from the create-time defaults.
//!
//! Every test drives the real `restJson1` surface through the router production mounts,
//! through the one harness (`router`, `call`, `create`, …) `slo_lifecycle` already drives
//! this service's surface with.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::slo_lifecycle::{call, create, name_path_segment, router, sli_config};

/// 2020-01-01T00:00:00Z in epoch seconds, the anchor of the calendar intervals below.
const JAN_01_2020: i64 = 1_577_836_800;

/// 2020-01-15T00:00:00Z in epoch seconds, an instant inside the January 2020 calendar tile.
const JAN_15_2020: i64 = 1_579_046_400;

/// `Goal.Interval` when a create omits it: "If you omit this parameter, a rolling interval
/// of 7 days is used" (`smithy_query(query="shape",shape="Goal")`, member `Interval`).
fn default_interval() -> Value {
    json!({ "RollingInterval": { "DurationUnit": "DAY", "Duration": 7 } })
}

/// Apply one `Goal` patch to the SLO called `name` and return the status and body.
async fn patch_goal(router: &MultiServiceRouter, name: &str, goal: Value) -> (u16, Value) {
    call(
        router,
        "PATCH",
        &format!("/slo/{}", name_path_segment(name)),
        Some(json!({ "Goal": goal })),
    )
    .await
}

/// Assert the three goal members of `goal` against one row's expectation.
fn assert_goal(label: &str, goal: &Value, interval: &Value, attainment: f64, warning: f64) {
    assert_eq!(
        &goal["Interval"], interval,
        "case {label:?}: expected Interval {interval:?}, got {:?}",
        goal["Interval"]
    );
    assert_eq!(
        goal["AttainmentGoal"].as_f64(),
        Some(attainment),
        "case {label:?}: expected AttainmentGoal {attainment}, got {:?}",
        goal["AttainmentGoal"]
    );
    assert_eq!(
        goal["WarningThreshold"].as_f64(),
        Some(warning),
        "case {label:?}: expected WarningThreshold {warning}, got {:?}",
        goal["WarningThreshold"]
    );
}

/// An update that supplies `Goal` writes the members it names and keeps the stored value of
/// every member it omits, and a following Get answers that same goal.
#[tokio::test]
async fn an_update_retains_the_goal_members_it_omits() {
    struct Case {
        label: &'static str,
        /// The `Goal` the create sends, or `None` to let the create defaults stand.
        created_goal: Option<Value>,
        /// The `Goal` the update sends.
        patch: Value,
        /// The goal the record must carry after the update.
        interval: Value,
        attainment_goal: f64,
        warning_threshold: f64,
    }

    let full_calendar_goal = json!({
        "Interval": {
            "CalendarInterval": { "StartTime": JAN_01_2020, "DurationUnit": "MONTH", "Duration": 1 }
        },
        "AttainmentGoal": 99.9,
        "WarningThreshold": 20.0
    });
    let calendar_month = json!({
        "CalendarInterval": { "StartTime": JAN_01_2020, "DurationUnit": "MONTH", "Duration": 1 }
    });

    let cases = [
        Case {
            label: "attainment goal only, over a stored calendar month",
            created_goal: Some(full_calendar_goal.clone()),
            patch: json!({ "AttainmentGoal": 95.0 }),
            interval: calendar_month.clone(),
            attainment_goal: 95.0,
            warning_threshold: 20.0,
        },
        Case {
            label: "warning threshold only, over a stored rolling interval in hours",
            // `DurationUnit` from `smithy_query(query="shape",shape="DurationUnit")`
            // -> MINUTE | HOUR | DAY | MONTH.
            created_goal: Some(json!({
                "Interval": { "RollingInterval": { "DurationUnit": "HOUR", "Duration": 6 } },
                "AttainmentGoal": 99.5,
                "WarningThreshold": 10.0
            })),
            patch: json!({ "WarningThreshold": 1.5 }),
            interval: json!({ "RollingInterval": { "DurationUnit": "HOUR", "Duration": 6 } }),
            attainment_goal: 99.5,
            warning_threshold: 1.5,
        },
        Case {
            label: "interval only, replacing a rolling interval with a calendar one",
            created_goal: Some(json!({
                "Interval": { "RollingInterval": { "DurationUnit": "MINUTE", "Duration": 30 } },
                "AttainmentGoal": 90.0,
                "WarningThreshold": 75.0
            })),
            patch: json!({ "Interval": calendar_month.clone() }),
            interval: calendar_month.clone(),
            attainment_goal: 90.0,
            warning_threshold: 75.0,
        },
        Case {
            label: "a goal naming no parameter at all",
            created_goal: Some(full_calendar_goal.clone()),
            patch: json!({}),
            interval: calendar_month.clone(),
            attainment_goal: 99.9,
            warning_threshold: 20.0,
        },
        Case {
            label: "two of the three members",
            created_goal: Some(full_calendar_goal.clone()),
            patch: json!({ "AttainmentGoal": 99.99, "WarningThreshold": 40.0 }),
            interval: calendar_month.clone(),
            attainment_goal: 99.99,
            warning_threshold: 40.0,
        },
        Case {
            label: "all three members, nothing left to retain",
            created_goal: Some(full_calendar_goal.clone()),
            patch: json!({
                "Interval": { "RollingInterval": { "DurationUnit": "MONTH", "Duration": 3 } },
                "AttainmentGoal": 97.0,
                "WarningThreshold": 60.0
            }),
            interval: json!({ "RollingInterval": { "DurationUnit": "MONTH", "Duration": 3 } }),
            attainment_goal: 97.0,
            warning_threshold: 60.0,
        },
        Case {
            // The stored values here are the create-time defaults themselves: they are what
            // the record carries, so they are what the update retains.
            label: "stored values that came from the create defaults",
            created_goal: None,
            patch: json!({ "AttainmentGoal": 99.95 }),
            interval: default_interval(),
            attainment_goal: 99.95,
            warning_threshold: 50.0,
        },
        Case {
            label: "a create that named only the warning threshold",
            created_goal: Some(json!({ "WarningThreshold": 5.0 })),
            patch: json!({ "AttainmentGoal": 99.9 }),
            interval: default_interval(),
            attainment_goal: 99.9,
            warning_threshold: 5.0,
        },
    ];

    let router = router();
    for (index, case) in cases.iter().enumerate() {
        let name = format!("goal-retention-{index}");
        let mut body = json!({
            "Name": name,
            "SliConfig": sli_config("checkout", "GET /cart", 100.0)
        });
        if let Some(goal) = case.created_goal.clone() {
            body["Goal"] = goal;
        }
        let (status, created) = create(&router, body).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the setup create to succeed, got {status} with {created}",
            case.label
        );

        let (status, updated) = patch_goal(&router, &name, case.patch.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from UpdateServiceLevelObjective, got {status} with {updated}",
            case.label
        );
        assert_goal(
            case.label,
            &updated["Slo"]["Goal"],
            &case.interval,
            case.attainment_goal,
            case.warning_threshold,
        );

        let (status, fetched) = call(
            &router,
            "GET",
            &format!("/slo/{}", name_path_segment(&name)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from the Get after the update, got {status} with {fetched}",
            case.label
        );
        assert_eq!(
            fetched["Slo"]["Goal"], updated["Slo"]["Goal"],
            "case {:?}: a Get must answer the stored goal {:?}, got {:?}",
            case.label, updated["Slo"]["Goal"], fetched["Slo"]["Goal"]
        );
    }
}

/// Retention composes: each update in a sequence keeps what the previous ones wrote, so a
/// member written once survives every later update that does not name it.
#[tokio::test]
async fn successive_updates_each_retain_what_they_do_not_name() {
    let router = router();
    let name = "goal-retention-sequence";
    let (status, created) = create(
        &router,
        json!({
            "Name": name,
            "SliConfig": sli_config("orders", "POST /orders", 250.0),
            "Goal": { "AttainmentGoal": 99.9, "WarningThreshold": 20.0 }
        }),
    )
    .await;
    assert_eq!(status, 200, "expected the setup create to succeed, got {status} with {created}");

    // Each step names ONE member; the expectation after it carries what every earlier step
    // wrote and the create-time value of everything no step has written.
    struct Step {
        label: &'static str,
        patch: Value,
        interval: Value,
        attainment_goal: f64,
        warning_threshold: f64,
    }
    let steps = [
        Step {
            label: "after naming the attainment goal",
            patch: json!({ "AttainmentGoal": 95.0 }),
            interval: default_interval(),
            attainment_goal: 95.0,
            warning_threshold: 20.0,
        },
        Step {
            label: "after naming the interval",
            patch: json!({
                "Interval": {
                    "CalendarInterval": {
                        "StartTime": JAN_01_2020, "DurationUnit": "MONTH", "Duration": 1
                    }
                }
            }),
            interval: json!({
                "CalendarInterval": { "StartTime": JAN_01_2020, "DurationUnit": "MONTH", "Duration": 1 }
            }),
            attainment_goal: 95.0,
            warning_threshold: 20.0,
        },
        Step {
            label: "after naming the warning threshold",
            patch: json!({ "WarningThreshold": 30.0 }),
            interval: json!({
                "CalendarInterval": { "StartTime": JAN_01_2020, "DurationUnit": "MONTH", "Duration": 1 }
            }),
            attainment_goal: 95.0,
            warning_threshold: 30.0,
        },
    ];

    for step in &steps {
        let (status, updated) = patch_goal(&router, name, step.patch.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from UpdateServiceLevelObjective, got {status} with {updated}",
            step.label
        );
        assert_goal(
            step.label,
            &updated["Slo"]["Goal"],
            &step.interval,
            step.attainment_goal,
            step.warning_threshold,
        );
    }
}

/// The retained interval is what the SLO is measured over: a budget report after an update
/// that names only the attainment goal keeps the stored evaluation interval, so the reported
/// `TotalBudgetSeconds` is the share of THAT interval the new attainment goal leaves
/// unattained.
#[tokio::test]
async fn a_budget_report_after_an_update_measures_the_retained_interval() {
    let router = router();
    let name = "goal-retention-budget";
    let (status, created) = create(
        &router,
        json!({
            "Name": name,
            "SliConfig": sli_config("checkout", "GET /cart", 100.0),
            "Goal": {
                "Interval": {
                    "CalendarInterval": {
                        "StartTime": JAN_01_2020, "DurationUnit": "MONTH", "Duration": 1
                    }
                },
                "AttainmentGoal": 99.9,
                "WarningThreshold": 20.0
            }
        }),
    )
    .await;
    assert_eq!(status, 200, "expected the setup create to succeed, got {status} with {created}");

    // Each row reports over the January 2020 calendar tile (31 days = 2_678_400 s), the
    // interval the create stored and no update below names: the budget is the share of it the
    // attainment goal leaves unattained, to the nearest second.
    struct Case {
        label: &'static str,
        patch: Value,
        total_budget_seconds: i64,
    }
    let cases = [
        Case {
            label: "the stored attainment goal",
            patch: json!({ "AttainmentGoal": 99.9 }),
            // 0.1% of 2_678_400 s = 2678.4 s.
            total_budget_seconds: 2678,
        },
        Case {
            label: "a looser attainment goal",
            patch: json!({ "AttainmentGoal": 99.8 }),
            // 0.2% of 2_678_400 s = 5356.8 s.
            total_budget_seconds: 5357,
        },
        Case {
            label: "a goal naming no parameter at all",
            patch: json!({}),
            // Unchanged from the row above: nothing was named, so nothing was written.
            total_budget_seconds: 5357,
        },
    ];

    for case in &cases {
        let (status, updated) = patch_goal(&router, name, case.patch.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from UpdateServiceLevelObjective, got {status} with {updated}",
            case.label
        );

        let (status, response) = call(
            &router,
            "POST",
            "/budget-report",
            Some(json!({ "SloIds": [name], "Timestamp": JAN_15_2020 })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from the budget report, got {status} with {response}",
            case.label
        );
        let report = response["Reports"][0].clone();
        assert_eq!(
            report["TotalBudgetSeconds"].as_i64(),
            Some(case.total_budget_seconds),
            "case {:?}: expected TotalBudgetSeconds {} over the retained calendar month, got {:?}",
            case.label,
            case.total_budget_seconds,
            report["TotalBudgetSeconds"]
        );
        assert_eq!(
            report["Goal"]["WarningThreshold"].as_f64(),
            Some(20.0),
            "case {:?}: expected the report to carry the retained WarningThreshold 20.0, got {:?}",
            case.label,
            report["Goal"]["WarningThreshold"]
        );
    }
}
