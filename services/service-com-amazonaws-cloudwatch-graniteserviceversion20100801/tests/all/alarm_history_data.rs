// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `AlarmHistoryItem$HistoryData`: every history item an alarm writer
//! records carries a JSON document describing the change that item reports, and
//! `DescribeAlarmHistory` serves it.
//!
//! The rule under test: `HistoryData` is "Data about the alarm, in JSON format."
//! (`smithy_query(query="shape",shape="AlarmHistoryItem")`, a `HistoryData` string with
//! `@length 1..4095`) — a configuration-update item records the configuration the call stored
//! (and the one it replaced), a state-update item records the alarm's state before and after
//! the write. Every table row below is a different input along what that rule generalises over
//! (which members the configuration carries, how many dimensions, actions and metric queries,
//! which state the alarm moved to, and the form and size of the state reason data), so no
//! assertion is keyed to one recorded request.

use crate::harness::{build_router, ok};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The modelled bound on one history document: `AlarmHistoryItem$HistoryData` targets
/// `HistoryData`, a string with `@length 1..4095`
/// (`smithy_query(query="shape",shape="AlarmHistoryItem")`).
const HISTORY_DATA_LEN: std::ops::RangeInclusive<usize> = 1..=4095;

/// The state `PutMetricAlarm` creates a metric alarm in: "When this operation creates an alarm,
/// the alarm state is immediately set to INSUFFICIENT_DATA."
const INITIAL_STATE: &str = "INSUFFICIENT_DATA";

/// A `PutMetricAlarm` request with only the members the model makes required, plus a name.
fn put_alarm(name: &str) -> Value {
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
}

/// The history of one alarm, oldest first.
async fn history(router: &MultiServiceRouter, alarm: &str) -> Vec<Value> {
    let described = ok(
        router,
        "DescribeAlarmHistory",
        &json!({"AlarmName": alarm, "ScanBy": "TimestampAscending"}),
    )
    .await;
    described
        .get("AlarmHistoryItems")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `HistoryData` of one item, parsed — this is the rule EVERY item has to satisfy: the
/// member is served, it is a JSON object, it names the document layout it uses, and it fits the
/// modelled length.
fn history_data(case: &str, item: &Value) -> Value {
    let raw = item
        .get("HistoryData")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        !raw.is_empty(),
        "case {case:?}: every history item carries HistoryData, got {item}"
    );
    let length = raw.len();
    assert!(
        HISTORY_DATA_LEN.contains(&length),
        "case {case:?}: HistoryData must fit the modelled {HISTORY_DATA_LEN:?}, got {length} \
         characters"
    );
    let parsed = serde_json::from_str::<Value>(raw);
    let Ok(document @ Value::Object(_)) = parsed else {
        panic!("case {case:?}: HistoryData must be a JSON object, got {raw:?}");
    };
    let version = document.get("version").and_then(Value::as_str);
    assert!(
        version.is_some_and(|version| !version.is_empty()),
        "case {case:?}: a history document names its layout version, got {document}"
    );
    document
}

/// Members of a `PutMetricAlarm` request and the member a configuration document records each
/// under: the model's own member name in lower camel case
/// (`smithy_query(query="shape",shape="MetricAlarm")`).
const CONFIGURATION_MEMBERS: &[(&str, &str)] = &[
    ("AlarmName", "alarmName"),
    ("AlarmDescription", "alarmDescription"),
    ("ActionsEnabled", "actionsEnabled"),
    ("AlarmActions", "alarmActions"),
    ("OKActions", "okActions"),
    ("InsufficientDataActions", "insufficientDataActions"),
    ("Namespace", "namespace"),
    ("MetricName", "metricName"),
    ("Statistic", "statistic"),
    ("ExtendedStatistic", "extendedStatistic"),
    ("Period", "period"),
    ("Unit", "unit"),
    ("EvaluationPeriods", "evaluationPeriods"),
    ("DatapointsToAlarm", "datapointsToAlarm"),
    ("Threshold", "threshold"),
    ("ComparisonOperator", "comparisonOperator"),
    ("TreatMissingData", "treatMissingData"),
    ("ThresholdMetricId", "thresholdMetricId"),
];

/// Whether a request's value and the value a document recorded for it are the same, comparing
/// numbers by value so an integral double and an integer are not reported as a difference.
fn same_value(asked: &Value, recorded: &Value) -> bool {
    match (asked.as_f64(), recorded.as_f64()) {
        (Some(asked), Some(recorded)) => (asked - recorded).abs() <= f64::EPSILON,
        _ => asked == recorded,
    }
}

/// Assert that `recorded` carries every configuration member `request` named.
fn assert_records_configuration(case: &str, request: &Value, recorded: &Value) {
    for (member, name) in CONFIGURATION_MEMBERS {
        let Some(asked) = request.get(member) else {
            continue;
        };
        let got = recorded.get(name);
        assert!(
            got.is_some_and(|got| same_value(asked, got)),
            "case {case:?}: {member} must be recorded as {name}={asked}, got {got:?} in {recorded}"
        );
    }
    if let Some(dimensions) = request.get("Dimensions").and_then(Value::as_array) {
        let got = recorded
            .get("dimensions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            got.len(),
            dimensions.len(),
            "case {case:?}: every dimension is recorded, got {recorded}"
        );
        for (asked, got) in dimensions.iter().zip(&got) {
            assert_eq!(
                (got.get("name"), got.get("value")),
                (asked.get("Name"), asked.get("Value")),
                "case {case:?}: a dimension is recorded by name and value, got {recorded}"
            );
        }
    }
    if let Some(queries) = request.get("Metrics").and_then(Value::as_array) {
        let got = recorded
            .get("metrics")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            got.len(),
            queries.len(),
            "case {case:?}: every metric query is recorded, got {recorded}"
        );
        for query in queries {
            let id = query.get("Id").and_then(Value::as_str);
            assert!(
                got.iter()
                    .any(|got| got.get("id").and_then(Value::as_str) == id),
                "case {case:?}: the query {id:?} is recorded, got {recorded}"
            );
        }
    }
}

/// A metric-math alarm's `Metrics` list: `count` queries, each with an expression `width`
/// characters long (`MetricDataQuery$Expression` is `@length 1..2048`).
fn metric_queries(count: usize, width: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| {
                let padding = " + 0".repeat(width / 4);
                json!({
                    "Id": format!("m{index}"),
                    "Expression": format!("1{padding}"),
                    "Label": format!("query {index}"),
                    "ReturnData": index == 0,
                })
            })
            .collect(),
    )
}

#[tokio::test]
async fn a_configuration_update_records_the_configuration_it_wrote() {
    let router = build_router();

    // Rows differ along the shape and size of the configuration written: which members it
    // carries, how many dimensions and actions it has, a basic statistic against an extended
    // one, and a metric-math alarm that names no single metric at all.
    let rows: Vec<(&str, Value)> = vec![
        ("only the required members", put_alarm("config-minimal")),
        (
            "a described alarm with dimensions and actions",
            json!({
                "AlarmName": "config-described",
                "AlarmDescription": "watches the queue",
                "Namespace": "MyApp",
                "MetricName": "Backlog",
                "Statistic": "Sum",
                "Unit": "Count",
                "Period": 300,
                "EvaluationPeriods": 3,
                "DatapointsToAlarm": 2,
                "Threshold": 100.5,
                "ComparisonOperator": "LessThanOrEqualToThreshold",
                "TreatMissingData": "breaching",
                "ActionsEnabled": true,
                "AlarmActions": ["arn:aws:sns:us-east-1:123456789012:page-me"],
                "OKActions": ["arn:aws:sns:us-east-1:123456789012:all-clear"],
                "InsufficientDataActions": [],
                "Dimensions": [
                    {"Name": "QueueName", "Value": "orders"},
                    {"Name": "Stage", "Value": "prod"},
                    {"Name": "Shard", "Value": "7"},
                ],
            }),
        ),
        (
            "an extended statistic",
            json!({
                "AlarmName": "config-percentile",
                "Namespace": "MyApp",
                "MetricName": "Latency",
                "ExtendedStatistic": "p99",
                "Period": 60,
                "EvaluationPeriods": 1,
                "Threshold": 250.0,
                "ComparisonOperator": "GreaterThanThreshold",
            }),
        ),
        (
            "a metric-math alarm",
            json!({
                "AlarmName": "config-metric-math",
                "EvaluationPeriods": 2,
                "Threshold": 10.0,
                "ComparisonOperator": "GreaterThanThreshold",
                "Metrics": metric_queries(3, 8),
            }),
        ),
    ];

    for (case, request) in rows {
        let name = request
            .get("AlarmName")
            .and_then(Value::as_str)
            .expect("every row names its alarm");
        ok(&router, "PutMetricAlarm", &request).await;

        let items = history(&router, name).await;
        assert_eq!(
            items.len(),
            1,
            "case {case:?}: creating an alarm records one history item, got {items:?}"
        );
        assert_eq!(
            items[0].get("HistoryItemType").and_then(Value::as_str),
            Some("ConfigurationUpdate"),
            "case {case:?}: a configuration write records a ConfigurationUpdate, got {items:?}"
        );
        let document = history_data(case, &items[0]);
        assert_eq!(
            document.get("type").and_then(Value::as_str),
            Some("Create"),
            "case {case:?}: a first write created the alarm, got {document}"
        );
        assert!(
            document.get("originalAlarm").is_none(),
            "case {case:?}: a created alarm replaced no configuration, got {document}"
        );
        let created = document
            .get("createdAlarm")
            .cloned()
            .unwrap_or(Value::Null);
        assert_records_configuration(case, &request, &created);

        // The document is computed from the alarm the store holds, not from the request: the
        // ARN it records is the one the reader reports for the same alarm.
        let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
        let arn = described.pointer("/MetricAlarms/0/AlarmArn");
        assert!(
            arn.is_some() && created.get("alarmArn") == arn,
            "case {case:?}: expected the stored ARN {arn:?}, got {created}"
        );
    }
}

#[tokio::test]
async fn overwriting_an_alarm_records_both_configurations() {
    let router = build_router();

    // Rows differ in WHAT the second write changes: a threshold, a period and evaluation
    // count, the action lists, the dimension set, and a switch to a metric-math alarm.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "a changed threshold",
            put_alarm("update-threshold"),
            json!({
                "AlarmName": "update-threshold",
                "Namespace": "MyApp",
                "MetricName": "Latency",
                "Statistic": "Average",
                "Period": 60,
                "EvaluationPeriods": 1,
                "Threshold": 42.0,
                "ComparisonOperator": "GreaterThanThreshold",
            }),
        ),
        (
            "a changed period and evaluation count",
            put_alarm("update-period"),
            json!({
                "AlarmName": "update-period",
                "Namespace": "MyApp",
                "MetricName": "Latency",
                "Statistic": "Average",
                "Period": 300,
                "EvaluationPeriods": 5,
                "DatapointsToAlarm": 4,
                "Threshold": 1.0,
                "ComparisonOperator": "GreaterThanThreshold",
            }),
        ),
        (
            "actions and dimensions added",
            put_alarm("update-actions"),
            json!({
                "AlarmName": "update-actions",
                "Namespace": "MyApp",
                "MetricName": "Latency",
                "Statistic": "Average",
                "Period": 60,
                "EvaluationPeriods": 1,
                "Threshold": 1.0,
                "ComparisonOperator": "GreaterThanThreshold",
                "AlarmActions": ["arn:aws:sns:us-east-1:123456789012:page-me"],
                "Dimensions": [{"Name": "Stage", "Value": "prod"}],
            }),
        ),
        (
            "a switch to a metric-math alarm",
            put_alarm("update-metric-math"),
            json!({
                "AlarmName": "update-metric-math",
                "EvaluationPeriods": 1,
                "Threshold": 1.0,
                "ComparisonOperator": "GreaterThanThreshold",
                "Metrics": metric_queries(2, 8),
            }),
        ),
    ];

    for (case, first, second) in rows {
        let name = first
            .get("AlarmName")
            .and_then(Value::as_str)
            .expect("every row names its alarm");
        ok(&router, "PutMetricAlarm", &first).await;
        ok(&router, "PutMetricAlarm", &second).await;

        let items = history(&router, name).await;
        assert_eq!(
            items.len(),
            2,
            "case {case:?}: two configuration writes record two items, got {items:?}"
        );
        let document = history_data(case, &items[1]);
        assert_eq!(
            document.get("type").and_then(Value::as_str),
            Some("Update"),
            "case {case:?}: overwriting an alarm records an update, got {document}"
        );
        let updated = document
            .get("updatedAlarm")
            .cloned()
            .unwrap_or(Value::Null);
        assert_records_configuration(case, &second, &updated);
        let original = document
            .get("originalAlarm")
            .cloned()
            .unwrap_or(Value::Null);
        assert_records_configuration(case, &first, &original);
        assert_ne!(
            updated, original,
            "case {case:?}: the two configurations differ, got {document}"
        );
    }
}

#[tokio::test]
async fn a_state_update_records_the_transition_it_made() {
    let router = build_router();
    let name = "transition-history";
    ok(&router, "PutMetricAlarm", &put_alarm(name)).await;

    // Rows differ along the state moved to (including moving to the state the alarm is already
    // in) and along the FORM and SIZE of the state reason data: absent, a JSON object, a JSON
    // array, one bulky payload, and then a second bulky one — at which point the transition's
    // two payloads together cannot fit the modelled 4095 and the document records neither.
    // `StateReasonData` is `@length 0..4000`
    // (`smithy_query(query="shape",shape="StateReasonData")`).
    let bulky = |marker: &str| json!({"marker": marker, "filler": "x".repeat(3_000)});
    // (case, state moved to, reason data given, whether the document can record it)
    let rows: Vec<(&str, &str, Option<Value>, bool)> = vec![
        (
            "the first transition, with no reason data",
            "ALARM",
            None,
            false,
        ),
        (
            "back to OK with a JSON object",
            "OK",
            Some(json!({
                "version": "1.0",
                "statistic": "Average",
                "period": 60,
                "recentDatapoints": [0.5, 0.25],
                "threshold": 1.0,
            })),
            true,
        ),
        (
            "to INSUFFICIENT_DATA with a JSON array",
            "INSUFFICIENT_DATA",
            Some(json!([1, 2, 3])),
            true,
        ),
        (
            "the same state again",
            "INSUFFICIENT_DATA",
            Some(json!({"repeated": true})),
            true,
        ),
        (
            "bulky reason data, which one document still fits",
            "ALARM",
            Some(bulky("first")),
            true,
        ),
        (
            "bulky reason data on both sides of the transition",
            "OK",
            Some(bulky("second")),
            false,
        ),
    ];

    let mut previous = INITIAL_STATE.to_string();
    for (case, state, reason_data, recordable) in rows {
        let reason = format!("set for {case}");
        let mut request = json!({
            "AlarmName": name,
            "StateValue": state,
            "StateReason": reason,
        });
        if let Some(data) = &reason_data {
            request["StateReasonData"] = json!(data.to_string());
        }
        ok(&router, "SetAlarmState", &request).await;

        let items = history(&router, name).await;
        let newest = items.last().cloned().unwrap_or(Value::Null);
        assert_eq!(
            newest.get("HistoryItemType").and_then(Value::as_str),
            Some("StateUpdate"),
            "case {case:?}: setting the state records a StateUpdate, got {newest}"
        );
        let document = history_data(case, &newest);
        assert_eq!(
            document.pointer("/oldState/stateValue").and_then(Value::as_str),
            Some(previous.as_str()),
            "case {case:?}: the document records the state moved out of, got {document}"
        );
        assert_eq!(
            document.pointer("/newState/stateValue").and_then(Value::as_str),
            Some(state),
            "case {case:?}: the document records the state moved to, got {document}"
        );
        assert_eq!(
            document.pointer("/newState/stateReason").and_then(Value::as_str),
            Some(reason.as_str()),
            "case {case:?}: the document records the reason given, got {document}"
        );
        match reason_data.as_ref().filter(|_| recordable) {
            // Reason data is itself JSON, so it is embedded as JSON rather than as a quoted
            // string — for as long as it fits the document's modelled length.
            Some(data) => assert_eq!(
                document.pointer("/newState/stateReasonData"),
                Some(data),
                "case {case:?}: the reason data is embedded as JSON, got {document}"
            ),
            // No reason data was given, or recording both sides of the transition would not
            // fit: it is then left out of BOTH states rather than truncated into JSON no
            // consumer can parse.
            None => assert!(
                document.pointer("/newState/stateReasonData").is_none()
                    && document.pointer("/oldState/stateReasonData").is_none(),
                "case {case:?}: reason data that cannot be recorded is left out, got {document}"
            ),
        }
        previous = state.to_string();
    }
}

#[tokio::test]
async fn a_configuration_larger_than_the_modelled_bound_stays_valid_json() {
    let router = build_router();

    // Rows differ in how far the configuration exceeds what one document may carry: a few
    // short queries fit, 20 queries of nearly the maximum expression length are far beyond
    // the modelled 4095. A document is JSON, so it is never truncated: it drops fidelity and
    // stays parseable.
    for (case, count, width) in [
        ("three short queries", 3, 8),
        ("ten medium queries", 10, 400),
        ("twenty queries of nearly the maximum width", 20, 2_040),
    ] {
        let name = format!("bulky-{count}-{width}");
        let request = json!({
            "AlarmName": name,
            "EvaluationPeriods": 1,
            "Threshold": 1.0,
            "ComparisonOperator": "GreaterThanThreshold",
            "Metrics": metric_queries(count, width),
        });
        ok(&router, "PutMetricAlarm", &request).await;
        // Overwrite it, so the document would have to carry TWO such configurations.
        ok(&router, "PutMetricAlarm", &request).await;

        let items = history(&router, &name).await;
        assert_eq!(
            items.len(),
            2,
            "case {case:?}: two writes record two items, got {items:?}"
        );
        for item in &items {
            let document = history_data(case, item);
            let recorded = document
                .get("createdAlarm")
                .or_else(|| document.get("updatedAlarm"))
                .and_then(|alarm| alarm.get("alarmName"))
                .and_then(Value::as_str);
            assert_eq!(
                recorded,
                Some(name.as_str()),
                "case {case:?}: however much it drops, the document names the alarm, got \
                 {document}"
            );
        }
    }
}

#[tokio::test]
async fn every_served_item_carries_history_data_a_consumer_can_read() {
    let router = build_router();

    // The reported defect: an item served without `HistoryData` makes a consumer that reads
    // `item["HistoryData"]` fail outright. Rows differ in the write sequence the history was
    // produced by, so every writer's items are covered.
    let states = ["ALARM", "OK", "INSUFFICIENT_DATA", "ALARM"];
    for (case, writes) in [
        ("a freshly created alarm", 0),
        ("one state transition", 1),
        ("several state transitions", states.len()),
    ] {
        let name = format!("consumer-{writes}");
        ok(&router, "PutMetricAlarm", &put_alarm(&name)).await;
        for state in states.iter().take(writes) {
            ok(
                &router,
                "SetAlarmState",
                &json!({"AlarmName": name, "StateValue": state, "StateReason": "driven"}),
            )
            .await;
        }

        let items = history(&router, &name).await;
        assert_eq!(
            items.len(),
            writes + 1,
            "case {case:?}: every write records an item, got {items:?}"
        );
        let mut reported_states: Vec<String> = Vec::new();
        for item in &items {
            let document = history_data(case, item);
            if item.get("HistoryItemType").and_then(Value::as_str) == Some("StateUpdate") {
                let new_state = document
                    .pointer("/newState/stateValue")
                    .and_then(Value::as_str);
                assert!(
                    new_state.is_some(),
                    "case {case:?}: a state update's document names the new state, got {document}"
                );
                reported_states.push(new_state.unwrap_or_default().to_string());
            }
        }
        assert_eq!(
            reported_states,
            states[..writes].to_vec(),
            "case {case:?}: the documents report the states the alarm was driven through"
        );
    }
}
