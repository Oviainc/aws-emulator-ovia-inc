// SPDX-License-Identifier: CC0-1.0

//! The per-request bound on the alarm-evaluation work — the EvaluationBudget — against the
//! evaluations the clock has already made due.
//!
//! The RULE asserted here: **a bound on the work ONE request may spend materialising the
//! evaluations the clock has made due only DEFERS what it stops — the evaluation stays due and
//! the next call carries it — which is only true while every pass that bound governs settles at
//! least one of them, however much of the bound was already gone when the pass began.** A pass
//! stopped before its first evaluation defers nothing; it starves it. Since part of such a
//! bound is a wall clock, and the span before a pass begins (the wait for the store's lock, and
//! whatever the scheduler did with the thread) is not the pass's to control, a bound that can be
//! spent before the first evaluation would leave an alarm reporting the state its last write
//! left behind for as long as the load lasted — not a state computed a little later, but one
//! never computed at all.
//!
//! So each row below hands a pass a budget that is ALREADY spent — its wall clock ran out
//! before the pass was entered, which is exactly what a loaded host does to a request — and
//! asserts both halves of the rule: the pass settles its first due evaluation, and it settles
//! only that one, leaving the rest due for the passes that follow. Driving the passes directly
//! is what makes "the budget was already spent when the pass began" a fact of the test rather
//! than of the host it runs on; everything a pass reads — the alarms, their data, their due
//! instants — is written through the wire and through the one store seam
//! [`crate::harness::build_router_with_engine`] exists for.
//!
//! The rows differ along what the rule generalises over: how many due evaluations are waiting,
//! how long the budget has been spent for, the state the data implies, the shape of the alarm
//! the pass evaluates (one metric, or a `Metrics` query array, whose fold is the expensive one),
//! and — for composite alarms, the second pass of the same stage — where in the alarms' name
//! order the one with an evaluation to settle sits.

use serde_json::{json, Value};
use service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;

use crate::harness::{build_router_with_engine, graded_window_stamp, now_secs, ok};

/// The state an alarm's write leaves behind, which every row seeds its alarms with: "the alarm
/// state is immediately set to INSUFFICIENT_DATA" (`PutMetricAlarm`). A pass that settles an
/// evaluation moves the alarm off it; a pass that starves one leaves it here.
const SEED: &str = "INSUFFICIENT_DATA";

/// The namespace every series of this suite is published under.
const NAMESPACE: &str = "MyApp/BudgetProgress";

/// The metric every row publishes one datapoint of.
const METRIC: &str = "Load";

/// The width of every row's window, in seconds, and the span the published datapoint is
/// graded in.
const PERIOD: i32 = 60;

/// How the alarms of one row are configured: the two shapes a metric alarm's evaluation folds,
/// because the one evaluation a pass is owed has to be a WHOLE one whichever of them it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// The alarm names one metric, folded by its own `Statistic`.
    OneMetric,
    /// The alarm names a `Metrics` query array, whose returning query is an expression over
    /// the metric the array reads.
    QueryArray,
}

/// One row of the table.
struct Row {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// How many alarms have an evaluation the clock has already made due.
    due: usize,
    /// How the row's alarms are configured.
    shape: Shape,
    /// The value published into the row's series, one datapoint in the current window.
    published: f64,
    /// The alarm's `Threshold`.
    threshold: f64,
    /// The alarm's `ComparisonOperator`.
    comparison: &'static str,
    /// The state the published datapoint puts the row's alarms in.
    state: &'static str,
    /// How long the budget each pass is given has ALREADY been spent for when the pass begins.
    spent_for: std::time::Duration,
}

/// How many alarms the longest row holds, so the names sort in one order whatever the count.
const NAME_DIGITS: usize = 2;

/// The name of the row's `index`th alarm. Zero-padded, so the order a pass visits them in is
/// the order the indexes read in.
fn alarm_name(index: usize) -> String {
    format!("budget-alarm-{index:0NAME_DIGITS$}")
}

/// The dimension set the row's series is identified by: its own, so a row's alarms grade the
/// data that row published.
fn dimensions(case: &str) -> Value {
    json!([{"Name": "Row", "Value": case}])
}

/// The `PutMetricAlarm` request for one of the row's alarms, in the shape the row names.
fn alarm_request(row: &Row, name: &str) -> Value {
    let mut request = json!({
        "AlarmName": name,
        "EvaluationPeriods": 1,
        "Threshold": row.threshold,
        "ComparisonOperator": row.comparison,
        "TreatMissingData": "missing",
        // No action is configured, so what a transition owes is a history item and nothing to
        // deliver: this suite is about the evaluation, not about the actions it fires.
        "ActionsEnabled": false,
    });
    match row.shape {
        Shape::OneMetric => {
            request["Namespace"] = json!(NAMESPACE);
            request["MetricName"] = json!(METRIC);
            request["Dimensions"] = dimensions(row.case);
            request["Statistic"] = json!("Maximum");
            request["Period"] = json!(PERIOD);
        }
        Shape::QueryArray => {
            request["Metrics"] = json!([
                {
                    "Id": "m1",
                    "ReturnData": false,
                    "MetricStat": {
                        "Metric": {
                            "Namespace": NAMESPACE,
                            "MetricName": METRIC,
                            "Dimensions": dimensions(row.case),
                        },
                        "Period": PERIOD,
                        "Stat": "Maximum",
                    },
                },
                {"Id": "e1", "Expression": "m1 * 2", "ReturnData": true},
            ]);
        }
    }
    request
}

/// The value the row's alarms grade: the published datapoint, doubled where the row's array
/// computes it.
fn graded(row: &Row) -> f64 {
    match row.shape {
        Shape::OneMetric => row.published,
        Shape::QueryArray => row.published * 2.0,
    }
}

/// The table: one row per shape of the question "what does a pass do with a budget that is
/// already spent?".
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one due evaluation and a budget just spent",
            due: 1,
            shape: Shape::OneMetric,
            published: 211.0,
            threshold: 50.0,
            comparison: "GreaterThanThreshold",
            state: "ALARM",
            spent_for: std::time::Duration::from_millis(60),
        },
        Row {
            case: "eight due evaluations and a budget long spent",
            due: 8,
            shape: Shape::OneMetric,
            published: 211.0,
            threshold: 50.0,
            comparison: "GreaterThanThreshold",
            state: "ALARM",
            spent_for: std::time::Duration::from_millis(80),
        },
        Row {
            case: "two due evaluations the data puts in OK",
            due: 2,
            shape: Shape::OneMetric,
            published: 2.0,
            threshold: 50.0,
            comparison: "GreaterThanThreshold",
            state: "OK",
            spent_for: std::time::Duration::from_millis(200),
        },
        Row {
            case: "a query array, whose fold is the expensive one",
            due: 1,
            shape: Shape::QueryArray,
            published: 60.0,
            threshold: 100.0,
            comparison: "GreaterThanThreshold",
            state: "ALARM",
            spent_for: std::time::Duration::from_millis(300),
        },
        Row {
            case: "three query arrays, graded below their threshold",
            due: 3,
            shape: Shape::QueryArray,
            published: 10.0,
            threshold: 100.0,
            comparison: "GreaterThanThreshold",
            state: "OK",
            spent_for: std::time::Duration::from_millis(120),
        },
    ]
}

/// Seed every metric alarm of the store with the state its write left behind, and make its
/// evaluation DUE at the epoch — the clock has already owed it for as long as there has been a
/// clock.
///
/// A pass re-arms the alarm it evaluates for its own next evaluation, so a due instant that is
/// still the epoch afterwards is an evaluation the pass did not settle: that is what the
/// assertions below count.
fn make_every_evaluation_due(engine: &engine::InMemoryEngine) {
    let state = cloudwatch::state::service_state(engine).expect("the CloudWatch store exists");
    let mut store = state.lock();
    for stored in store.alarms.values_mut() {
        stored.alarm.state_value = Some(SEED.to_string());
        stored.evaluation_due_secs = 0.0;
    }
}

/// How the store reports each of the row's alarms: its state value, and whether its evaluation
/// has been settled (its due instant re-armed off the epoch).
fn reported(engine: &engine::InMemoryEngine, row: &Row) -> Vec<(String, bool)> {
    let state = cloudwatch::state::service_state(engine).expect("the CloudWatch store exists");
    let store = state.lock();
    (0..row.due)
        .map(|index| {
            let name = alarm_name(index);
            let stored = store
                .alarms
                .get(&name)
                .unwrap_or_else(|| panic!("case {:?}: the store holds the alarm {name}", row.case));
            (
                stored.alarm.state_value.clone().unwrap_or_default(),
                stored.evaluation_due_secs > 0.0,
            )
        })
        .collect()
}

/// Run ONE pass over the metric alarms with a budget whose wall clock ran out `spent_for`
/// before the pass began, the way a loaded host hands one to a request.
fn pass_with_a_spent_budget(engine: &engine::InMemoryEngine, spent_for: std::time::Duration) {
    let mut budget = cloudwatch::state::alarm_evaluation::EvaluationBudget::for_one_request();
    // A BLOCKING sleep, with no await between the budget and the pass. The handlers above
    // started this store's alarm clock, which settles the same evaluations whenever the runtime
    // gets to run it: blocking the one worker of this test's runtime is what keeps it from
    // running here, so the pass below is the only thing that can settle anything and what it
    // settles is this row's assertion rather than a race with that task.
    std::thread::sleep(spent_for);
    let state = cloudwatch::state::service_state(engine).expect("the CloudWatch store exists");
    let mut store = state.lock();
    store.evaluate_metric_alarms(now_secs(), &mut budget);
}

#[tokio::test]
async fn a_pass_given_a_spent_budget_settles_the_first_due_evaluation_and_defers_the_rest() {
    for row in rows() {
        let case = row.case;
        // A fresh emulator per row, so the alarms a pass visits can only be this row's.
        let (router, engine) = build_router_with_engine();
        ok(
            &router,
            "PutMetricData",
            &json!({
                "Namespace": NAMESPACE,
                "MetricData": [{
                    "MetricName": METRIC,
                    "Dimensions": dimensions(case),
                    "Value": row.published,
                    // The newest window an evaluation GRADES is the newest one that has closed,
                    // so the datapoint a row's alarms grade is stamped inside that one.
                    "Timestamp": graded_window_stamp(f64::from(PERIOD)),
                }],
            }),
        )
        .await;
        for index in 0..row.due {
            ok(
                &router,
                "PutMetricAlarm",
                &alarm_request(&row, &alarm_name(index)),
            )
            .await;
        }
        make_every_evaluation_due(&engine);

        // One pass per request: each settles the first evaluation still due and leaves the
        // others for the next, so after the Nth pass every one of them is settled — the
        // bound DEFERS, and a deferred evaluation is one a later pass really does.
        for settled in 1..=row.due {
            pass_with_a_spent_budget(&engine, row.spent_for);
            let reported = reported(&engine, &row);
            assert_eq!(
                reported.iter().filter(|(_, settled)| *settled).count(),
                settled,
                "case {case:?}: pass {settled} of a spent budget must settle one more due \
                 evaluation and no more, got {reported:?}"
            );
            for (index, (state, was_settled)) in reported.iter().enumerate() {
                let expected = if *was_settled { row.state } else { SEED };
                assert_eq!(
                    state, expected,
                    "case {case:?}: after pass {settled}, the alarm {} must report {expected:?} \
                     over the value {} it grades, got {reported:?}",
                    alarm_name(index),
                    graded(&row)
                );
            }
        }
    }
}

/// One row of the composite table: where in the alarms' name order the one with an evaluation
/// to settle sits.
struct CompositeRow {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The composite alarms the store holds, in the order their names sort, each beside
    /// whether the clock has made its evaluation due.
    alarms: &'static [(&'static str, bool)],
}

/// The composite table: the pass's one owed evaluation has to be spent on an alarm that HAS one
/// to settle, wherever in the name order that alarm sits.
const COMPOSITE_ROWS: &[CompositeRow] = &[
    CompositeRow {
        case: "the only composite alarm is due",
        alarms: &[("aa-due", true)],
    },
    CompositeRow {
        case: "the due composite alarm sorts after one that is not due",
        alarms: &[("aa-not-due", false), ("zz-due", true)],
    },
    CompositeRow {
        case: "the due composite alarm sorts first, between two that are not",
        alarms: &[
            ("aa-due", true),
            ("mm-not-due", false),
            ("zz-not-due", false),
        ],
    },
];

/// How far in the future a composite alarm's evaluation is owed for the rows that hold it NOT
/// due: long enough that no pass of this suite can reach it.
const NOT_DUE_SECS: f64 = 3_600.0;

#[tokio::test]
async fn a_composite_pass_given_a_spent_budget_settles_an_alarm_that_has_an_evaluation_due() {
    for row in COMPOSITE_ROWS {
        let case = row.case;
        let (router, engine) = build_router_with_engine();
        // The child the rules read, forced into ALARM so the rule over it computes a state
        // that differs from the one a composite alarm's write leaves behind.
        ok(
            &router,
            "PutMetricAlarm",
            &json!({
                "AlarmName": "rule-child",
                "Namespace": NAMESPACE,
                "MetricName": METRIC,
                "Dimensions": dimensions(case),
                "Statistic": "Maximum",
                "Period": PERIOD,
                "EvaluationPeriods": 1,
                "Threshold": 1.0,
                "ComparisonOperator": "GreaterThanThreshold",
                "TreatMissingData": "ignore",
            }),
        )
        .await;
        ok(
            &router,
            "SetAlarmState",
            &json!({
                "AlarmName": "rule-child",
                "StateValue": "ALARM",
                "StateReason": "driven by the test",
            }),
        )
        .await;
        for (name, _due) in row.alarms {
            ok(
                &router,
                "PutCompositeAlarm",
                &json!({
                    "AlarmName": name,
                    "AlarmRule": "ALARM(\"rule-child\")",
                    "ActionsEnabled": false,
                }),
            )
            .await;
        }

        // The state each composite alarm's write left behind, and the due instants this row is
        // about: the epoch for the one the clock owes an evaluation, far ahead for the rest.
        let now = now_secs();
        let state =
            cloudwatch::state::service_state(&*engine).expect("the CloudWatch store exists");
        {
            let mut store = state.lock();
            for (name, due) in row.alarms {
                let stored = store
                    .composite_alarms
                    .get_mut(*name)
                    .unwrap_or_else(|| panic!("case {case:?}: the store holds {name}"));
                stored.alarm.state_value = Some(SEED.to_string());
                stored.evaluation_due_secs = if *due { 0.0 } else { now + NOT_DUE_SECS };
            }
        }

        let mut budget = cloudwatch::state::alarm_evaluation::EvaluationBudget::for_one_request();
        // Blocking, for the same reason as the metric table above: it holds this store's alarm
        // clock off while the budget's wall clock runs out.
        std::thread::sleep(std::time::Duration::from_millis(120));
        {
            let mut store = state.lock();
            store.evaluate_composite_alarms(now_secs(), &mut budget);
        }

        let store = state.lock();
        for (name, due) in row.alarms {
            let stored = store
                .composite_alarms
                .get(*name)
                .unwrap_or_else(|| panic!("case {case:?}: the store holds {name}"));
            // The rule reads a child in ALARM, so the alarm the pass settled reports ALARM and
            // one whose evaluation is not due yet still reports the state its write left.
            let expected = if *due { "ALARM" } else { SEED };
            assert_eq!(
                stored.alarm.state_value.as_deref(),
                Some(expected),
                "case {case:?}: with a spent budget, {name} must report {expected:?}, got {:?}",
                stored.alarm.state_value
            );
        }
    }
}
