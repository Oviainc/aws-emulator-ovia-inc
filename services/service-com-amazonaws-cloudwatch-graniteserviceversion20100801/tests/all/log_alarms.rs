// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the log-alarm lifecycle: `PutLogAlarm` writing an alarm whose
//! configuration is the CloudWatch Logs scheduled query it evaluates, and `DescribeAlarms` /
//! `DescribeAlarmHistory` / `DeleteAlarms` reading and removing it.
//!
//! The rules under test are the ones `PutLogAlarm` documents, and each table's rows differ
//! along what the rule generalises over rather than repeating one recorded request:
//!
//! * a stored log alarm round-trips every member the request carried, whatever values it
//!   carried, and the service adds the members it owns (the alarm ARN, the managed scheduled
//!   query's `QueryARN`, the configuration timestamp, the documented defaults);
//! * a newly configured alarm is `INSUFFICIENT_DATA` — nothing has evaluated it yet — and a
//!   call that replaces an existing alarm overwrites its configuration and leaves its state;
//! * a request that breaks one of the operation's documented value rules is refused with
//!   HTTP 400 `ValidationError`, and the published bounds themselves are accepted.

use crate::harness::{build_router, call, error_code, ok};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A role ARN of the form the alarm's two role members name
/// (`ScheduledQueryConfiguration$ScheduledQueryRoleARN`).
const QUERY_ROLE: &str = "arn:aws:iam::000000000000:role/cwl-scheduled-query";

/// The role an action notification's log lines are retrieved with
/// (`PutLogAlarmInput$ActionLogLineRoleArn`).
const LOG_LINE_ROLE: &str = "arn:aws:iam::000000000000:role/cwl-action-log-lines";

/// The smallest request `PutLogAlarm` accepts: every required member and nothing else.
fn minimal(name: &str) -> Value {
    json!({
        "AlarmName": name,
        "QueryResultsToEvaluate": 3,
        "QueryResultsToAlarm": 2,
        "Threshold": 10.0,
        "ComparisonOperator": "GreaterThanThreshold",
        "ScheduledQueryConfiguration": {
            "QueryString": "fields @message | filter @message like /ERROR/",
            "ScheduledQueryRoleARN": QUERY_ROLE,
            "AggregationExpression": "count(*)",
            "ScheduleConfiguration": {
                "ScheduleExpression": "rate(5 minutes)",
                "StartTimeOffset": 360,
            },
        },
    })
}

/// A request built from [`minimal`] with `members` merged over it.
fn with(name: &str, members: Value) -> Value {
    let mut request = minimal(name);
    let (Value::Object(request_members), Value::Object(extra)) = (&mut request, members) else {
        panic!("both the request and the overrides are JSON objects");
    };
    for (member, value) in extra {
        request_members.insert(member, value);
    }
    request
}

/// Every log alarm `DescribeAlarms` reports for the given request.
async fn described_log_alarms(router: &MultiServiceRouter, request: &Value) -> Vec<Value> {
    let body = ok(router, "DescribeAlarms", request).await;
    body.get("LogAlarms")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The one log alarm `DescribeAlarms` reports for `name`.
async fn log_alarm(router: &MultiServiceRouter, name: &str) -> Value {
    let alarms = described_log_alarms(
        router,
        &json!({"AlarmNames": [name], "AlarmTypes": ["LogAlarm"]}),
    )
    .await;
    assert_eq!(
        alarms.len(),
        1,
        "DescribeAlarms: expected exactly the log alarm {name}, got {alarms:?}"
    );
    alarms[0].clone()
}

#[tokio::test]
async fn the_log_alarm_configuration_round_trips_through_describe_alarms() {
    let router = build_router();
    let request = with(
        "error-rate-high",
        json!({
            "AlarmDescription": "too many errors in the application log",
            "ActionsEnabled": false,
            "AlarmActions": ["arn:aws:sns:us-east-1:000000000000:page-oncall"],
            "OKActions": ["arn:aws:sns:us-east-1:000000000000:recovered"],
            "InsufficientDataActions": ["arn:aws:sns:us-east-1:000000000000:no-data"],
            "TreatMissingData": "notBreaching",
            "ActionLogLineCount": 10,
            "ActionLogLineRoleArn": LOG_LINE_ROLE,
            "WarmUpConfiguration": {
                "WarmUpPeriodDurationInMinutes": 15,
                "OnlyStartEvaluatingAfterWarmUpPeriodEnds": true,
            },
            "ActionConfiguration": {
                "AlarmTagsPropagation": {"Enabled": true, "IncludedTagKeys": ["team"]},
            },
            "Tags": [{"Key": "team", "Value": "platform"}],
            "ScheduledQueryConfiguration": {
                "QueryString": "fields @message | filter @message like /ERROR/",
                "ScheduledQueryRoleARN": QUERY_ROLE,
                "AggregationExpression": "count(*) by host",
                "LogGroupIdentifiers": ["/app/api", "/app/worker"],
                "ScheduleConfiguration": {
                    "ScheduleExpression": "rate(5 minutes)",
                    "StartTimeOffset": 600,
                    "EndTimeOffset": 60,
                },
            },
        }),
    );
    ok(&router, "PutLogAlarm", &request).await;

    let alarm = log_alarm(&router, "error-rate-high").await;

    // Every member the request carried comes back as it was given.
    for member in [
        "AlarmDescription",
        "ActionsEnabled",
        "AlarmActions",
        "OKActions",
        "InsufficientDataActions",
        "TreatMissingData",
        "ActionLogLineCount",
        "ActionLogLineRoleArn",
        "WarmUpConfiguration",
        "ActionConfiguration",
        "QueryResultsToEvaluate",
        "QueryResultsToAlarm",
        "Threshold",
        "ComparisonOperator",
    ] {
        assert_eq!(
            alarm.get(member),
            request.get(member),
            "member {member}: expected the stored alarm to report what the request carried, \
             got {:?}",
            alarm.get(member)
        );
    }

    // The scheduled query configuration round-trips too, plus the ARN of the managed query
    // the service created for the alarm.
    let stored_query = alarm
        .get("ScheduledQueryConfiguration")
        .cloned()
        .unwrap_or(Value::Null);
    let requested_query = request
        .get("ScheduledQueryConfiguration")
        .cloned()
        .unwrap_or(Value::Null);
    for member in [
        "QueryString",
        "ScheduledQueryRoleARN",
        "AggregationExpression",
        "LogGroupIdentifiers",
        "ScheduleConfiguration",
    ] {
        assert_eq!(
            stored_query.get(member),
            requested_query.get(member),
            "ScheduledQueryConfiguration.{member}: expected what the request carried, got {:?}",
            stored_query.get(member)
        );
    }
    assert_eq!(
        stored_query.get("QueryARN").and_then(Value::as_str),
        Some("arn:aws:logs:us-east-1:000000000000:scheduled-query:error-rate-high"),
        "QueryARN: expected the ARN of the managed scheduled query, got {stored_query}"
    );

    // The members the SERVICE owns.
    assert_eq!(
        alarm.get("AlarmName").and_then(Value::as_str),
        Some("error-rate-high"),
        "AlarmName: got {alarm}"
    );
    assert_eq!(
        alarm.get("AlarmArn").and_then(Value::as_str),
        Some("arn:aws:cloudwatch:us-east-1:000000000000:alarm:error-rate-high"),
        "AlarmArn: got {alarm}"
    );
    assert!(
        alarm.get("AlarmConfigurationUpdatedTimestamp").is_some(),
        "AlarmConfigurationUpdatedTimestamp: expected the write's own instant, got {alarm}"
    );
    // Nothing has evaluated the alarm yet, so it carries neither a state nor an evaluation
    // state a query execution would have produced.
    assert_eq!(
        alarm.get("StateValue").and_then(Value::as_str),
        Some("INSUFFICIENT_DATA"),
        "StateValue: expected a newly configured alarm to be INSUFFICIENT_DATA, got {alarm}"
    );
    assert_eq!(
        alarm.get("EvaluationState"),
        None,
        "EvaluationState: expected none before any evaluation, got {:?}",
        alarm.get("EvaluationState")
    );
}

#[tokio::test]
async fn the_documented_defaults_are_applied_when_the_request_omits_them() {
    let router = build_router();
    ok(&router, "PutLogAlarm", &minimal("defaults")).await;

    let alarm = log_alarm(&router, "defaults").await;
    // "The default is true." (`ActionsEnabled`), "If this parameter is omitted, the default
    // behavior of missing is used." (`TreatMissingData`), "The default is 0, which means no
    // log lines are included." (`ActionLogLineCount`).
    for (member, expected) in [
        ("ActionsEnabled", json!(true)),
        ("TreatMissingData", json!("missing")),
        ("ActionLogLineCount", json!(0)),
    ] {
        assert_eq!(
            alarm.get(member),
            Some(&expected),
            "member {member}: expected the documented default {expected}, got {:?}",
            alarm.get(member)
        );
    }
}

#[tokio::test]
async fn every_accepted_value_of_each_member_round_trips() {
    let router = build_router();

    // The rows differ along what each rule generalises over: the M and the N of the M-of-N
    // evaluation (including the published upper bound of 100 and M == N), the comparison
    // operator (all four a log alarm accepts, per
    // `smithy_query(query="doc",shape="PutLogAlarmInput",member="ComparisonOperator")`), the
    // missing-data behaviour (all four documented values), the number of action log lines
    // (both published bounds), the schedule expression's form, and the query time range.
    let cases: Vec<(&str, Value)> = vec![
        ("the smallest m-of-n", json!({"QueryResultsToEvaluate": 1, "QueryResultsToAlarm": 1})),
        ("m equal to n", json!({"QueryResultsToEvaluate": 4, "QueryResultsToAlarm": 4})),
        (
            "the largest n",
            json!({"QueryResultsToEvaluate": 100, "QueryResultsToAlarm": 7}),
        ),
        ("greater than", json!({"ComparisonOperator": "GreaterThanThreshold"})),
        (
            "greater than or equal to",
            json!({"ComparisonOperator": "GreaterThanOrEqualToThreshold"}),
        ),
        ("less than", json!({"ComparisonOperator": "LessThanThreshold"})),
        (
            "less than or equal to",
            json!({"ComparisonOperator": "LessThanOrEqualToThreshold"}),
        ),
        ("a negative threshold", json!({"Threshold": -2.5})),
        ("a zero threshold", json!({"Threshold": 0.0})),
        ("missing data breaches", json!({"TreatMissingData": "breaching"})),
        ("missing data does not breach", json!({"TreatMissingData": "notBreaching"})),
        ("missing data is ignored", json!({"TreatMissingData": "ignore"})),
        ("missing data is missing", json!({"TreatMissingData": "missing"})),
        ("no action log lines", json!({"ActionLogLineCount": 0})),
        (
            "the most action log lines",
            json!({"ActionLogLineCount": 50, "ActionLogLineRoleArn": LOG_LINE_ROLE}),
        ),
        (
            "a rate in minutes",
            json!({"ScheduledQueryConfiguration": scheduled_query("rate(1 minute)", 60, None)}),
        ),
        (
            "a rate in hours",
            json!({"ScheduledQueryConfiguration": scheduled_query("rate(2 hours)", 7200, None)}),
        ),
        (
            "a rate in days",
            json!({"ScheduledQueryConfiguration": scheduled_query("rate(1 day)", 86400, None)}),
        ),
        (
            "a five-field cron",
            json!({"ScheduledQueryConfiguration": scheduled_query("cron(0/5 * * * ?)", 300, None)}),
        ),
        (
            "a six-field cron",
            json!({
                "ScheduledQueryConfiguration":
                    scheduled_query("cron(0 12 * * ? *)", 3600, None)
            }),
        ),
        (
            "a query time range that ends before the execution",
            json!({"ScheduledQueryConfiguration": scheduled_query("rate(5 minutes)", 600, Some(599))}),
        ),
        (
            "a query time range that ends at the execution",
            json!({"ScheduledQueryConfiguration": scheduled_query("rate(5 minutes)", 600, Some(0))}),
        ),
    ];

    for (index, (case, members)) in cases.iter().enumerate() {
        let name = format!("accepted-{index}");
        let request = with(&name, members.clone());
        let (status, body) = call(&router, "PutLogAlarm", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );

        let alarm = log_alarm(&router, &name).await;
        let Value::Object(expected) = members else {
            panic!("case {case:?}: the overrides are a JSON object");
        };
        for (member, value) in expected {
            let reported = alarm.get(member).cloned().unwrap_or(Value::Null);
            let reported = match member.as_str() {
                // The service adds the managed query's ARN to the configuration it stores, so
                // the comparison is against the configuration WITHOUT it.
                "ScheduledQueryConfiguration" => {
                    let mut without = reported;
                    if let Value::Object(members) = &mut without {
                        members.remove("QueryARN");
                    }
                    without
                }
                _ => reported,
            };
            assert_eq!(
                &reported, value,
                "case {case:?}: member {member} did not round-trip, got {reported}"
            );
        }
    }
}

/// A scheduled query configuration with the given schedule and time-range offsets.
fn scheduled_query(schedule: &str, start_time_offset: i64, end_time_offset: Option<i64>) -> Value {
    let mut configuration = json!({
        "QueryString": "fields @message",
        "ScheduledQueryRoleARN": QUERY_ROLE,
        "AggregationExpression": "count(*)",
        "ScheduleConfiguration": {
            "ScheduleExpression": schedule,
            "StartTimeOffset": start_time_offset,
        },
    });
    if let (Some(end), Some(schedule_members)) = (
        end_time_offset,
        configuration
            .get_mut("ScheduleConfiguration")
            .and_then(Value::as_object_mut),
    ) {
        schedule_members.insert("EndTimeOffset".to_string(), json!(end));
    }
    configuration
}

#[tokio::test]
async fn a_request_that_breaks_a_documented_rule_is_refused_and_stores_nothing() {
    let router = build_router();

    // One row per documented rule, each naming the member it breaks.
    let cases: Vec<(&str, Value)> = vec![
        (
            "more results to evaluate than the published maximum",
            json!({"QueryResultsToEvaluate": 101, "QueryResultsToAlarm": 1}),
        ),
        (
            "more breaches required than there are results",
            json!({"QueryResultsToEvaluate": 3, "QueryResultsToAlarm": 4}),
        ),
        (
            "a comparison operator only a band alarm can use",
            json!({"ComparisonOperator": "GreaterThanUpperThreshold"}),
        ),
        (
            "another band comparison operator",
            json!({"ComparisonOperator": "LessThanLowerOrGreaterThanUpperThreshold"}),
        ),
        (
            "a missing-data behaviour the alarm does not know",
            json!({"TreatMissingData": "Breaching"}),
        ),
        (
            "more action log lines than the published maximum",
            json!({"ActionLogLineCount": 51, "ActionLogLineRoleArn": LOG_LINE_ROLE}),
        ),
        (
            "a negative number of action log lines",
            json!({"ActionLogLineCount": -1}),
        ),
        (
            "action log lines with no role to retrieve them",
            json!({"ActionLogLineCount": 5}),
        ),
        (
            "an action log line role that is not a role ARN",
            json!({"ActionLogLineCount": 5, "ActionLogLineRoleArn": "cwl-action-log-lines"}),
        ),
        (
            "a query role that is not a role ARN",
            json!({
                "ScheduledQueryConfiguration": {
                    "QueryString": "fields @message",
                    "ScheduledQueryRoleARN": "arn:aws:iam::000000000000:user/not-a-role",
                    "AggregationExpression": "count(*)",
                    "ScheduleConfiguration": {
                        "ScheduleExpression": "rate(5 minutes)",
                        "StartTimeOffset": 300,
                    },
                }
            }),
        ),
        (
            "a schedule that is neither a rate nor a cron",
            json!({"ScheduledQueryConfiguration": scheduled_query("every 5 minutes", 300, None)}),
        ),
        (
            "a rate of zero",
            json!({"ScheduledQueryConfiguration": scheduled_query("rate(0 minutes)", 300, None)}),
        ),
        (
            "a rate in a unit no schedule uses",
            json!({"ScheduledQueryConfiguration": scheduled_query("rate(2 fortnights)", 300, None)}),
        ),
        (
            "a cron with too few fields",
            json!({"ScheduledQueryConfiguration": scheduled_query("cron(0/5 * * *)", 300, None)}),
        ),
        (
            "a cron with too many fields",
            json!({"ScheduledQueryConfiguration": scheduled_query("cron(0 12 * * ? * *)", 300, None)}),
        ),
        (
            "a query time range that ends when it begins",
            json!({"ScheduledQueryConfiguration": scheduled_query("rate(5 minutes)", 300, Some(300))}),
        ),
        (
            "a query time range that ends after it begins",
            json!({"ScheduledQueryConfiguration": scheduled_query("rate(5 minutes)", 300, Some(301))}),
        ),
    ];

    for (index, (case, members)) in cases.iter().enumerate() {
        let name = format!("refused-{index}");
        let (status, body) = call(&router, "PutLogAlarm", &with(&name, members.clone())).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationError"),
            "case {case:?}: expected HTTP 400 ValidationError, got {status} with {body}"
        );
        let alarms = described_log_alarms(
            &router,
            &json!({"AlarmNames": [&name], "AlarmTypes": ["LogAlarm"]}),
        )
        .await;
        assert!(
            alarms.is_empty(),
            "case {case:?}: a refused request stored an alarm anyway: {alarms:?}"
        );
    }
}

#[tokio::test]
async fn a_second_call_replaces_the_configuration_and_leaves_the_state() {
    let router = build_router();
    ok(
        &router,
        "PutLogAlarm",
        &with(
            "replaced",
            json!({
                "AlarmDescription": "the first configuration",
                "AlarmActions": ["arn:aws:sns:us-east-1:000000000000:first"],
                "Tags": [{"Key": "round", "Value": "first"}],
            }),
        ),
    )
    .await;
    let first = log_alarm(&router, "replaced").await;

    ok(
        &router,
        "PutLogAlarm",
        &with(
            "replaced",
            json!({
                "QueryResultsToEvaluate": 10,
                "QueryResultsToAlarm": 9,
                "Threshold": 99.0,
                "ComparisonOperator": "LessThanThreshold",
                "Tags": [{"Key": "round", "Value": "second"}],
            }),
        ),
    )
    .await;
    let second = log_alarm(&router, "replaced").await;

    // "the operation replaces the previous configuration of that alarm" — the members the
    // second call did not carry are gone rather than merged.
    assert_eq!(
        second.get("AlarmDescription"),
        None,
        "AlarmDescription: expected the replaced configuration to drop it, got {second}"
    );
    assert_eq!(
        second.get("AlarmActions"),
        None,
        "AlarmActions: expected the replaced configuration to drop it, got {second}"
    );
    for (member, expected) in [
        ("QueryResultsToEvaluate", json!(10)),
        ("QueryResultsToAlarm", json!(9)),
        ("Threshold", json!(99.0)),
        ("ComparisonOperator", json!("LessThanThreshold")),
    ] {
        assert_eq!(
            second.get(member),
            Some(&expected),
            "member {member}: expected the second call's value {expected}, got {:?}",
            second.get(member)
        );
    }
    // The alarm's state is the evaluation's, not the configuration write's, so it is the one
    // the first call left.
    assert_eq!(
        second.get("StateValue"),
        first.get("StateValue"),
        "StateValue: expected an update to leave the state alone, got {:?} after {:?}",
        second.get("StateValue"),
        first.get("StateValue")
    );
    assert_eq!(
        second.get("StateUpdatedTimestamp"),
        first.get("StateUpdatedTimestamp"),
        "StateUpdatedTimestamp: expected an update to leave it alone, got {:?} after {:?}",
        second.get("StateUpdatedTimestamp"),
        first.get("StateUpdatedTimestamp")
    );

    // Only ONE alarm exists under the name, whichever call wrote it.
    let alarms = described_log_alarms(&router, &json!({"AlarmTypes": ["LogAlarm"]})).await;
    assert_eq!(
        alarms.len(),
        1,
        "DescribeAlarms: expected the second call to replace the alarm, got {alarms:?}"
    );
}

#[tokio::test]
async fn each_write_is_recorded_in_the_alarms_history() {
    let router = build_router();
    ok(&router, "PutLogAlarm", &minimal("historied")).await;
    ok(&router, "PutLogAlarm", &minimal("historied")).await;

    let history = ok(
        &router,
        "DescribeAlarmHistory",
        &json!({"AlarmName": "historied", "AlarmTypes": ["LogAlarm"]}),
    )
    .await;
    let items = history
        .get("AlarmHistoryItems")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        items.len(),
        2,
        "DescribeAlarmHistory: expected one item per write, got {items:?}"
    );
    for item in &items {
        assert_eq!(
            item.get("AlarmType").and_then(Value::as_str),
            Some("LogAlarm"),
            "AlarmType: expected the item to report the alarm's kind, got {item}"
        );
        assert_eq!(
            item.get("HistoryItemType").and_then(Value::as_str),
            Some("ConfigurationUpdate"),
            "HistoryItemType: expected a configuration update, got {item}"
        );
        let data: Value = item
            .get("HistoryData")
            .and_then(Value::as_str)
            .map(|raw| serde_json::from_str(raw).expect("HistoryData is JSON"))
            .unwrap_or(Value::Null);
        assert!(
            data.get("createdAlarm").is_some() || data.get("updatedAlarm").is_some(),
            "HistoryData: expected the configuration this write stored, got {data}"
        );
    }
    // The CREATE comes first and the replacement records the configuration it replaced.
    let newest: Value = items
        .iter()
        .filter_map(|item| item.get("HistoryData").and_then(Value::as_str))
        .filter_map(|raw| serde_json::from_str(raw).ok())
        .find(|data: &Value| data.get("updatedAlarm").is_some())
        .unwrap_or(Value::Null);
    assert!(
        newest.get("originalAlarm").is_some(),
        "HistoryData: expected the replacement to record the configuration it replaced, got \
         {newest}"
    );
}

#[tokio::test]
async fn one_name_names_one_alarm_whatever_kind_it_is() {
    let router = build_router();
    ok(&router, "PutLogAlarm", &minimal("taken-by-a-log-alarm")).await;

    let metric_alarm = |name: &str| {
        json!({
            "AlarmName": name,
            "Namespace": "MyApp",
            "MetricName": "Latency",
            "Statistic": "Average",
            "Period": 60,
            "EvaluationPeriods": 1,
            "Threshold": 1.0,
            "ComparisonOperator": "GreaterThanThreshold",
        })
    };

    // A writer of another kind cannot take a name a log alarm holds.
    for (case, operation, request) in [
        (
            "a metric alarm over a log alarm's name",
            "PutMetricAlarm",
            metric_alarm("taken-by-a-log-alarm"),
        ),
        (
            "a composite alarm over a log alarm's name",
            "PutCompositeAlarm",
            json!({"AlarmName": "taken-by-a-log-alarm", "AlarmRule": "TRUE"}),
        ),
    ] {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationError"),
            "case {case:?}: expected HTTP 400 ValidationError, got {status} with {body}"
        );
    }

    // And a log alarm cannot take a name another kind holds: that conflicts with the current
    // state of the alarm already there, which is the modelled `ResourceConflict`.
    ok(&router, "PutMetricAlarm", &metric_alarm("taken-by-a-metric-alarm")).await;
    ok(
        &router,
        "PutCompositeAlarm",
        &json!({"AlarmName": "taken-by-a-composite-alarm", "AlarmRule": "TRUE"}),
    )
    .await;
    for (case, name) in [
        ("a metric alarm's name", "taken-by-a-metric-alarm"),
        ("a composite alarm's name", "taken-by-a-composite-alarm"),
    ] {
        let (status, body) = call(&router, "PutLogAlarm", &minimal(name)).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (409, "ResourceConflict"),
            "case {case:?}: expected HTTP 409 ResourceConflict, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn describe_alarms_reports_a_log_alarm_only_when_the_request_asks_for_one() {
    let router = build_router();
    ok(
        &router,
        "PutLogAlarm",
        &with(
            "svc-errors",
            json!({"AlarmActions": ["arn:aws:sns:us-east-1:000000000000:page-oncall"]}),
        ),
    )
    .await;

    // "If you omit this parameter, only metric alarms are returned, even if composite alarms
    // or log alarms exist in the account." (`DescribeAlarmsInput$AlarmTypes`), and every other
    // filter applies to a log alarm exactly as it does to the other kinds.
    let cases: Vec<(&str, Value, usize)> = vec![
        ("no AlarmTypes at all", json!({}), 0),
        ("metric alarms only", json!({"AlarmTypes": ["MetricAlarm"]}), 0),
        ("log alarms", json!({"AlarmTypes": ["LogAlarm"]}), 1),
        (
            "log alarms among several types",
            json!({"AlarmTypes": ["MetricAlarm", "CompositeAlarm", "LogAlarm"]}),
            1,
        ),
        (
            "a matching name prefix",
            json!({"AlarmTypes": ["LogAlarm"], "AlarmNamePrefix": "svc-"}),
            1,
        ),
        (
            "a prefix that matches nothing",
            json!({"AlarmTypes": ["LogAlarm"], "AlarmNamePrefix": "db-"}),
            0,
        ),
        (
            "the state the alarm is in",
            json!({"AlarmTypes": ["LogAlarm"], "StateValue": "INSUFFICIENT_DATA"}),
            1,
        ),
        (
            "a state the alarm is not in",
            json!({"AlarmTypes": ["LogAlarm"], "StateValue": "ALARM"}),
            0,
        ),
        (
            "an action prefix the alarm uses",
            json!({"AlarmTypes": ["LogAlarm"], "ActionPrefix": "arn:aws:sns:"}),
            1,
        ),
        (
            "an action prefix the alarm does not use",
            json!({"AlarmTypes": ["LogAlarm"], "ActionPrefix": "arn:aws:lambda:"}),
            0,
        ),
    ];

    for (case, request, expected) in cases {
        let alarms = described_log_alarms(&router, &request).await;
        assert_eq!(
            alarms.len(),
            expected,
            "case {case:?}: expected {expected} log alarm(s), got {alarms:?}"
        );
    }
}

#[tokio::test]
async fn delete_alarms_removes_a_log_alarm_but_never_in_a_batch() {
    let router = build_router();
    ok(&router, "PutLogAlarm", &minimal("deleted")).await;
    ok(&router, "PutLogAlarm", &minimal("also-deleted")).await;
    ok(
        &router,
        "PutMetricAlarm",
        &json!({
            "AlarmName": "latency-high",
            "Namespace": "MyApp",
            "MetricName": "Latency",
            "Statistic": "Average",
            "Period": 60,
            "EvaluationPeriods": 1,
            "Threshold": 1.0,
            "ComparisonOperator": "GreaterThanThreshold",
        }),
    )
    .await;

    // "Log alarms cannot be batch deleted." — the rows differ along what the log alarm is
    // batched with.
    for (case, names) in [
        ("two log alarms", vec!["deleted", "also-deleted"]),
        ("a log alarm and a metric alarm", vec!["deleted", "latency-high"]),
        ("a log alarm and a name nothing holds", vec!["deleted", "absent"]),
    ] {
        let (status, body) = call(&router, "DeleteAlarms", &json!({"AlarmNames": names})).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationError"),
            "case {case:?}: expected HTTP 400 ValidationError, got {status} with {body}"
        );
        let alarms = described_log_alarms(&router, &json!({"AlarmTypes": ["LogAlarm"]})).await;
        assert_eq!(
            alarms.len(),
            2,
            "case {case:?}: a refused delete removed an alarm anyway, leaving {alarms:?}"
        );
    }

    // Named on its own, the log alarm is deleted.
    ok(&router, "DeleteAlarms", &json!({"AlarmNames": ["deleted"]})).await;
    let alarms = described_log_alarms(&router, &json!({"AlarmTypes": ["LogAlarm"]})).await;
    let names: Vec<Option<&str>> = alarms
        .iter()
        .map(|alarm| alarm.get("AlarmName").and_then(Value::as_str))
        .collect();
    assert_eq!(
        names,
        vec![Some("also-deleted")],
        "DescribeAlarms: expected only the alarm that was not deleted, got {alarms:?}"
    );

    // A name the request repeats names one alarm: the call deletes it and does not report it
    // as missing on the second mention.
    ok(
        &router,
        "DeleteAlarms",
        &json!({"AlarmNames": ["also-deleted", "also-deleted"]}),
    )
    .await;
    let alarms = described_log_alarms(&router, &json!({"AlarmTypes": ["LogAlarm"]})).await;
    assert!(
        alarms.is_empty(),
        "DescribeAlarms: expected both log alarms to be gone, got {alarms:?}"
    );

    // A name nothing holds any more is reported as missing rather than silently accepted.
    let (status, body) = call(&router, "DeleteAlarms", &json!({"AlarmNames": ["deleted"]})).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (404, "ResourceNotFound"),
        "expected HTTP 404 ResourceNotFound for an alarm already deleted, got {status} with \
         {body}"
    );
}
