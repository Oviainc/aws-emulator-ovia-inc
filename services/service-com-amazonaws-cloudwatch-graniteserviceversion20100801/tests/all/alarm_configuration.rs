// SPDX-License-Identifier: CC0-1.0

//! The configuration rules `PutMetricAlarm` enforces across its members, and the action lists
//! every described alarm reports.
//!
//! Each rule is asserted over a TABLE whose rows differ along what the rule generalises over —
//! the period, the number of evaluation periods, the shape of the query array — rather than
//! along a name:
//!
//! 1. An alarm is evaluated on a `Period` of 10, 20 or 30 seconds, or a multiple of 60; any
//!    other value is refused with HTTP 400 `ValidationError` and stores nothing.
//! 2. An alarm's total evaluation period (`Period` x `EvaluationPeriods`) may cover at most
//!    one day when the period is shorter than an hour, and at most seven days otherwise.
//! 3. An alarm whose `EvaluationWindow` aligns its evaluated range to clock boundaries is
//!    evaluated on a period a clock boundary matches: one minute, five minutes, an hour, a day
//!    or a week.
//! 4. A `Metrics` query array defines the single time series the alarm watches: every id an
//!    expression refers to is the `Id` of a query in the SAME request, no expression calls
//!    `SEARCH`, and the expression the alarm watches does not return more than one time
//!    series.
//! 5. A described alarm always reports `OKActions`, `AlarmActions` and
//!    `InsufficientDataActions` — the empty list when the request configured no action.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{build_router, call, error_code, ok};

/// The wire error code CloudWatch refuses a configuration it would not create with.
const REFUSAL_CODE: &str = "ValidationError";

/// The three action lists of an alarm, each a `ResourceList` of `length 0..5`
/// (`smithy_query(query="operation",operation="PutMetricAlarm")`).
const ACTION_MEMBERS: [&str; 3] = ["OKActions", "AlarmActions", "InsufficientDataActions"];

/// A static-threshold `PutMetricAlarm` request for `name`, with `members` merged over it.
fn metric_alarm(name: &str, members: Value) -> Value {
    let mut request = json!({
        "AlarmName": name,
        "Namespace": "Custom/Alarms",
        "MetricName": "Latency",
        "Statistic": "Average",
        "Period": 60,
        "EvaluationPeriods": 1,
        "Threshold": 10.0,
        "ComparisonOperator": "GreaterThanThreshold",
    });
    merge(&mut request, members);
    request
}

/// A metric-math `PutMetricAlarm` request for `name`: the alarm watches the result of
/// `queries` rather than one named metric, so it carries no `Namespace` / `MetricName` /
/// `Statistic` / `Period`.
fn math_alarm(name: &str, queries: Value) -> Value {
    json!({
        "AlarmName": name,
        "Metrics": queries,
        "EvaluationPeriods": 1,
        "Threshold": 10.0,
        "ComparisonOperator": "GreaterThanThreshold",
    })
}

/// A `MetricStat` query under `id` that retrieves one series, the kind an expression refers to.
fn stat_query(id: &str) -> Value {
    json!({
        "Id": id,
        "ReturnData": false,
        "MetricStat": {
            "Metric": {"Namespace": "Custom/Alarms", "MetricName": "Latency"},
            "Period": 60,
            "Stat": "Average",
        },
    })
}

/// Merge `members` over `request`, which both have to be JSON objects.
fn merge(request: &mut Value, members: Value) {
    let (Value::Object(target), Value::Object(extra)) = (request, members) else {
        panic!("both the request and the overrides are JSON objects");
    };
    for (member, value) in extra {
        target.insert(member, value);
    }
}

/// The one metric alarm `DescribeAlarms` reports for `name`, or `None` when it reports none.
async fn described(router: &MultiServiceRouter, name: &str) -> Option<Value> {
    let body = ok(router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
    body.get("MetricAlarms")
        .and_then(Value::as_array)
        .and_then(|alarms| alarms.first().cloned())
}

/// Assert that `request` is accepted and the alarm it configures is stored.
async fn assert_accepted(router: &MultiServiceRouter, case: &str, name: &str, request: &Value) {
    let (status, body) = call(router, "PutMetricAlarm", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the configuration to be accepted with HTTP 200, got {status} \
         with body {body}"
    );
    let stored = described(router, name).await;
    assert!(
        stored.is_some(),
        "case {case:?}: an accepted configuration must be stored, got {stored:?}"
    );
}

/// Assert that `request` is refused with HTTP 400 `ValidationError` and stores nothing.
async fn assert_refused(router: &MultiServiceRouter, case: &str, name: &str, request: &Value) {
    let (status, body) = call(router, "PutMetricAlarm", request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the configuration to be refused with HTTP 400, got {status} \
         with body {body}"
    );
    assert_eq!(
        error_code(&body),
        REFUSAL_CODE,
        "case {case:?}: expected the error code {REFUSAL_CODE}, got {body}"
    );
    let stored = described(router, name).await;
    assert_eq!(
        stored, None,
        "case {case:?}: a refused configuration must store nothing, got {stored:?}"
    );
}

#[tokio::test]
async fn an_alarm_is_created_only_on_a_period_it_can_be_evaluated_at() {
    // (case, Period, whether CloudWatch creates the alarm)
    //
    // "Valid values are 10, 20, 30, and any multiple of 60."
    // (`smithy_query(query="doc",shape="PutMetricAlarmInput",member="Period")`) — the three
    // sub-minute cadences, multiples of a minute up to an hour and a day, and, refused, the
    // sub-minute values that are not among the three (including 1 and 5, which a metric may
    // be READ at), a value just under and just over a minute, and one that is a multiple of
    // neither.
    let rows: [(&str, i64, bool); 15] = [
        ("the shortest high-resolution period", 10, true),
        ("twenty seconds", 20, true),
        ("thirty seconds", 30, true),
        ("one minute", 60, true),
        ("two minutes", 120, true),
        ("five minutes", 300, true),
        ("one hour", 3_600, true),
        ("one day", 86_400, true),
        ("one second", 1, false),
        ("five seconds", 5, false),
        ("fifteen seconds", 15, false),
        ("forty-five seconds", 45, false),
        ("one second under a minute", 59, false),
        ("one second over a minute", 61, false),
        ("ninety seconds", 90, false),
    ];

    let router = build_router();
    for (case, period, accepted) in rows {
        let name = format!("period-{period}");
        // An hour and a day are only within the horizon ceiling at one evaluation period.
        let request = metric_alarm(&name, json!({"Period": period, "EvaluationPeriods": 1}));
        if accepted {
            assert_accepted(&router, case, &name, &request).await;
            let stored = described(&router, &name).await.unwrap_or_default();
            assert_eq!(
                stored.get("Period").and_then(Value::as_i64),
                Some(period),
                "case {case:?}: the stored alarm must carry the period it was created with, \
                 got {stored}"
            );
        } else {
            assert_refused(&router, case, &name, &request).await;
        }
    }
}

#[tokio::test]
async fn an_alarms_total_evaluation_period_may_not_exceed_the_published_ceiling() {
    // (case, Period, EvaluationPeriods, whether CloudWatch creates the alarm)
    //
    // "An alarm's total current evaluation period can be no longer than seven days, so Period
    // multiplied by EvaluationPeriods can't be more than 604,800 seconds. For alarms with a
    // period of less than one hour (3,600 seconds), the total evaluation period can't be
    // longer than one day (86,400 seconds)."
    // (`smithy_query(query="doc",shape="PutMetricAlarmInput",member="Period")`) — each
    // published bound exactly, and one evaluation period past it.
    let rows: [(&str, i64, i64, bool); 10] = [
        ("one day of one-minute periods", 60, 1_440, true),
        ("one period past one day at one minute", 60, 1_441, false),
        ("one day of ten-second periods", 10, 8_640, true),
        ("one period past one day at ten seconds", 10, 8_641, false),
        ("one day of five-minute periods", 300, 288, true),
        ("one period past one day at five minutes", 300, 289, false),
        ("seven days of one-hour periods", 3_600, 168, true),
        ("one period past seven days at one hour", 3_600, 169, false),
        ("seven days of one-day periods", 86_400, 7, true),
        ("one period past seven days at one day", 86_400, 8, false),
    ];

    let router = build_router();
    for (case, period, periods, accepted) in rows {
        let name = format!("horizon-{period}-{periods}");
        let request = metric_alarm(
            &name,
            json!({"Period": period, "EvaluationPeriods": periods}),
        );
        if accepted {
            assert_accepted(&router, case, &name, &request).await;
        } else {
            assert_refused(&router, case, &name, &request).await;
        }
    }
}

#[tokio::test]
async fn an_alarm_aligned_to_clock_boundaries_is_created_only_on_a_period_a_boundary_matches() {
    // (case, Period, whether the alarm aligns its window to clock boundaries, whether the
    //  alarm is created)
    //
    // "When you use a wall clock window, the alarm's period must be 1 minute (60 seconds), 5
    // minutes (300 seconds), 1 hour (3,600 seconds), 1 day (86,400 seconds), or 1 week
    // (604,800 seconds). Other period values aren't supported with a wall clock window."
    // (`smithy_query(query="doc",shape="WallClockWindow")`) — every published period, and
    // refused, the periods that are valid for an alarm but match no clock boundary. The same
    // periods in the default sliding window stay acceptable.
    let rows: [(&str, i64, bool, bool); 14] = [
        ("one minute, aligned", 60, true, true),
        ("five minutes, aligned", 300, true, true),
        ("one hour, aligned", 3_600, true, true),
        ("one day, aligned", 86_400, true, true),
        ("one week, aligned", 604_800, true, true),
        ("ten seconds, aligned", 10, true, false),
        ("twenty seconds, aligned", 20, true, false),
        ("thirty seconds, aligned", 30, true, false),
        ("two minutes, aligned", 120, true, false),
        ("ten minutes, aligned", 600, true, false),
        ("two hours, aligned", 7_200, true, false),
        ("two minutes, sliding", 120, false, true),
        ("ten minutes, sliding", 600, false, true),
        ("ten seconds, sliding", 10, false, true),
    ];

    let router = build_router();
    for (case, period, aligned, accepted) in rows {
        let name = format!("window-{period}-{aligned}");
        let window = if aligned {
            json!({"WallClockWindow": {"Timezone": "America/New_York"}})
        } else {
            json!({"SlidingWindow": {}})
        };
        // A warm-up configuration beside the window: the warm-up delays the evaluation and
        // does not change which periods the window can be aligned to.
        let request = metric_alarm(
            &name,
            json!({
                "Period": period,
                "EvaluationPeriods": 1,
                "EvaluationWindow": window,
                "WarmUpConfiguration": {"WarmUpPeriodDurationInMinutes": 5},
            }),
        );
        if accepted {
            assert_accepted(&router, case, &name, &request).await;
        } else {
            assert_refused(&router, case, &name, &request).await;
        }
    }
}

#[tokio::test]
async fn a_metric_math_alarm_is_created_only_when_its_queries_resolve_to_one_series() {
    // (case, the Metrics query array, whether CloudWatch creates the alarm)
    //
    // "A math expression can use the Id of the other metrics or queries to refer to those
    // metrics" and "Any expression used in a PutMetricAlarm operation must return a single
    // time series." (`smithy_query(query="shape",shape="MetricDataQuery")`) — rows differ in
    // whether the referred-to id is defined, how many queries the array holds, which function
    // the watched expression calls, how deeply a search is wrapped, whether a multi-series
    // call is reduced by an enclosing function, and whether the expression in question is the
    // one the alarm watches.
    let rows: [(&str, Value, bool); 12] = [
        (
            "an expression over a defined query",
            json!([
                stat_query("m1"),
                {"Id": "e1", "Expression": "m1 / 1000", "ReturnData": true},
            ]),
            true,
        ),
        (
            "an expression over two defined queries",
            json!([
                stat_query("m1"),
                stat_query("m2"),
                {"Id": "e1", "Expression": "IF(m1 > 0, m1 + m2, 0)", "ReturnData": true},
            ]),
            true,
        ),
        (
            "an expression over another expression",
            json!([
                stat_query("m1"),
                {"Id": "e1", "Expression": "m1 * 2", "ReturnData": false},
                {"Id": "e2", "Expression": "e1 + 1", "ReturnData": true},
            ]),
            true,
        ),
        (
            "a constant expression that refers to nothing",
            json!([{"Id": "e1", "Expression": "100", "ReturnData": true}]),
            true,
        ),
        (
            "every metric of the request reduced to one series",
            json!([
                stat_query("m1"),
                {"Id": "e1", "Expression": "SUM(METRICS())", "ReturnData": true},
            ]),
            true,
        ),
        (
            "a bare search expression",
            json!([{
                "Id": "e1",
                "Expression": "SEARCH('{Custom/Alarms,Host} MetricName=\"Latency\"', 'Average', 300)",
                "ReturnData": true,
            }]),
            false,
        ),
        (
            "a bare search expression the request adds to",
            json!([{
                "Id": "e1",
                "Expression": "SEARCH('{Custom/Alarms,Host} MetricName=\"Latency\"', 'Average', 300) + 1",
                "ReturnData": true,
            }]),
            false,
        ),
        (
            "a search wrapped in a function that reduces it to one series",
            json!([{
                "Id": "e1",
                "Expression": "SUM(SEARCH('{Custom/Alarms,Host} MetricName=\"Latency\"', 'Average', 300))",
                "ReturnData": true,
            }]),
            false,
        ),
        (
            "a search the alarm does not watch, feeding the expression it does",
            json!([
                {
                    "Id": "e1",
                    "Expression": "SEARCH('{Custom/Alarms,Host} MetricName=\"Latency\"', 'Average', 300)",
                    "ReturnData": false,
                },
                {"Id": "e2", "Expression": "SUM(e1)", "ReturnData": true},
            ]),
            false,
        ),
        (
            "every metric of the request, unreduced",
            json!([
                stat_query("m1"),
                {"Id": "e1", "Expression": "METRICS()", "ReturnData": true},
            ]),
            false,
        ),
        (
            "an expression over an id no query defines",
            json!([{"Id": "e1", "Expression": "m1 / 1000", "ReturnData": true}]),
            false,
        ),
        (
            "an expression over an id another request would define",
            json!([
                stat_query("m1"),
                {"Id": "e1", "Expression": "m1 + other_total", "ReturnData": true},
            ]),
            false,
        ),
    ];

    let router = build_router();
    for (index, (case, queries, accepted)) in rows.into_iter().enumerate() {
        let name = format!("math-{index}");
        let request = math_alarm(&name, queries);
        if accepted {
            assert_accepted(&router, case, &name, &request).await;
        } else {
            assert_refused(&router, case, &name, &request).await;
        }
    }
}

#[tokio::test]
async fn a_multi_series_expression_the_alarm_does_not_watch_is_accepted() {
    // "specify true for the one expression result to use as the alarm. For all other metrics
    // and expressions in the same PutMetricAlarm operation, specify ReturnData as False."
    // (`smithy_query(query="shape",shape="MetricDataQuery")`) — only the expression the alarm
    // WATCHES has to return a single time series, so a multi-series expression feeding the one
    // it watches is part of a configuration CloudWatch creates.
    let router = build_router();
    let request = math_alarm(
        "metrics-fed",
        json!([
            stat_query("m1"),
            {"Id": "e1", "Expression": "METRICS()", "ReturnData": false},
            {"Id": "e2", "Expression": "SUM(e1)", "ReturnData": true},
        ]),
    );
    assert_accepted(
        &router,
        "every metric of the request feeding the watched expression",
        "metrics-fed",
        &request,
    )
    .await;
}

#[tokio::test]
async fn every_described_alarm_reports_its_three_action_lists() {
    // (case, the actions the request configures, what each list must come back as)
    //
    // The three members are `ResourceList`s of `length 0..5`, so no action is the empty list
    // rather than no list at all: a caller iterating a described alarm's actions never has to
    // tell one from the other.
    let configured = "arn:aws:sns:us-east-1:000000000000:oncall";
    let rows: [(&str, Value, [Value; 3]); 4] = [
        (
            "no action at all",
            json!({}),
            [json!([]), json!([]), json!([])],
        ),
        (
            "one alarm action",
            json!({"AlarmActions": [configured]}),
            [json!([]), json!([configured]), json!([])],
        ),
        (
            "an action in every list",
            json!({
                "OKActions": [configured],
                "AlarmActions": [configured],
                "InsufficientDataActions": [configured],
            }),
            [
                json!([configured]),
                json!([configured]),
                json!([configured]),
            ],
        ),
        (
            "an explicitly empty list",
            json!({"AlarmActions": []}),
            [json!([]), json!([]), json!([])],
        ),
    ];

    let router = build_router();
    for (index, (case, actions, expected)) in rows.into_iter().enumerate() {
        let name = format!("actions-{index}");
        ok(
            &router,
            "PutMetricAlarm",
            &metric_alarm(&name, actions.clone()),
        )
        .await;
        let alarm = described(&router, &name).await.unwrap_or_default();
        for (member, expected) in ACTION_MEMBERS.iter().zip(&expected) {
            assert_eq!(
                alarm.get(member),
                Some(expected),
                "case {case:?}: expected {member} to be reported as {expected}, got {alarm}"
            );
        }

        // The same rule on a composite alarm, which `DescribeAlarms` reports beside the
        // metric alarms through the same three members.
        let composite = format!("composite-{index}");
        let mut request = json!({
            "AlarmName": composite,
            "AlarmRule": format!("ALARM(\"{name}\")"),
        });
        merge(&mut request, actions);
        ok(&router, "PutCompositeAlarm", &request).await;
        let body = ok(
            &router,
            "DescribeAlarms",
            &json!({"AlarmNames": [composite], "AlarmTypes": ["CompositeAlarm"]}),
        )
        .await;
        let alarm = body
            .get("CompositeAlarms")
            .and_then(Value::as_array)
            .and_then(|alarms| alarms.first().cloned())
            .unwrap_or_default();
        for (member, expected) in ACTION_MEMBERS.iter().zip(&expected) {
            assert_eq!(
                alarm.get(member),
                Some(expected),
                "case {case:?}: expected the composite alarm's {member} to be reported as \
                 {expected}, got {alarm}"
            );
        }
    }
}
