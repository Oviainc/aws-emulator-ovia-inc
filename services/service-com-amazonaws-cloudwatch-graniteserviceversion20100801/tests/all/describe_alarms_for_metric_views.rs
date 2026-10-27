// SPDX-License-Identifier: CC0-1.0

//! What the PER-METRIC alarm reader reports, end to end over the wire.
//!
//! One rule is asserted here, over TABLES whose rows differ along what the rule generalises
//! over — the configuration an alarm was written with, the dimension set that identifies its
//! metric, and which writer changed the configuration — rather than along one recorded
//! request:
//!
//! A reader that reports the alarms of one metric reports the alarm RECORDS the store holds:
//! every configuration member each alarm was written with, with the same values the
//! general alarm reader reports for the same alarm, and the value a configuration write left
//! behind on the very next read, without waiting for anything.
//!
//! The metric is identified by its namespace, its name and its dimension SET, so the order a
//! request lists the dimensions in names the same metric: every query below lists them in the
//! reverse of the order the alarm was written with.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::harness::{
    build_router, described_alarm, merged, metric_alarm_request, ok, same_json_value,
    AlarmKind as Kind,
};

/// The configuration members a metric alarm carries, as both readers must report them.
///
/// The alarm's identity and its evaluation configuration — `smithy_query(query="shape",
/// shape="MetricAlarm")` — and NOT its state members, which the evaluation the clock owes
/// moves on its own schedule.
const CONFIGURATION_MEMBERS: &[&str] = &[
    "AlarmName",
    "AlarmArn",
    "AlarmDescription",
    "ActionsEnabled",
    "OKActions",
    "AlarmActions",
    "InsufficientDataActions",
    "Namespace",
    "MetricName",
    "Dimensions",
    "Statistic",
    "ExtendedStatistic",
    "Unit",
    "Period",
    "EvaluationPeriods",
    "DatapointsToAlarm",
    "Threshold",
    "ComparisonOperator",
    "TreatMissingData",
    "EvaluateLowSampleCountPercentile",
];

/// A `PutMetricAlarm` request for `name`: the static-threshold alarm every suite starts from,
/// with `patch` merged in ([`merged`], where a member the patch carries as `null` is REMOVED,
/// which is how a row configured with `ExtendedStatistic` drops the base `Statistic`).
fn alarm_request(name: &str, patch: &Value) -> Value {
    merged(metric_alarm_request(name), patch)
}

/// A dimension list as the SET it denotes: dimension name to value, so two lists of the same
/// dimensions in different orders compare equal.
fn dimension_set(dimensions: Option<&Value>) -> BTreeMap<String, String> {
    dimensions
        .and_then(Value::as_array)
        .map(|listed| {
            listed
                .iter()
                .map(|dimension| {
                    (
                        dimension["Name"].as_str().unwrap_or_default().to_string(),
                        dimension["Value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The query that names the metric `request` configures an alarm on, listing the dimensions in
/// the REVERSE of the order the alarm was written with.
fn query_for(request: &Value) -> Value {
    let mut dimensions: Vec<Value> = request["Dimensions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    dimensions.reverse();
    json!({
        "Namespace": request["Namespace"],
        "MetricName": request["MetricName"],
        "Dimensions": dimensions,
    })
}

/// Whether `reported` carries the value `wanted` ([`same_json_value`], so a double the
/// protocol carries as `10` and one it carries as `10.0` are one value), and nothing at all is
/// never the value asked for.
fn reports(wanted: &Value, reported: Option<&Value>) -> bool {
    reported.is_some_and(|reported| same_json_value(wanted, reported))
}

/// The alarms the per-metric reader reports for the metric `query` names.
async fn alarms_for_metric(
    router: &smithy_server_runtime::MultiServiceRouter,
    query: &Value,
) -> Vec<Value> {
    let reported = ok(router, "DescribeAlarmsForMetric", query).await;
    reported["MetricAlarms"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

#[tokio::test]
async fn the_per_metric_reader_reports_the_configuration_each_alarm_was_written_with() {
    let router = build_router();

    // Rows differ in the evaluation configuration the alarm was written with: the
    // missing-data arm (every documented value — "Valid Values: breaching | notBreaching |
    // ignore | missing", `smithy_query(query="doc",shape="PutMetricAlarmInput",
    // member="TreatMissingData")`), the "M out of N" counts, the statistic and its
    // low-sample handling, the unit, the comparison, the actions, and how many dimensions
    // identify the metric.
    let rows: &[(&str, Value)] = &[
        ("the documented defaults", json!({})),
        (
            "missing data treated as breaching",
            json!({"TreatMissingData": "breaching"}),
        ),
        (
            "missing data treated as not breaching",
            json!({"TreatMissingData": "notBreaching"}),
        ),
        (
            "missing data ignored",
            json!({"TreatMissingData": "ignore"}),
        ),
        (
            "missing data treated as missing, written out",
            json!({"TreatMissingData": "missing"}),
        ),
        (
            "two of three datapoints",
            json!({"EvaluationPeriods": 3, "DatapointsToAlarm": 2}),
        ),
        (
            "a percentile statistic whose low samples are ignored",
            json!({
                "Statistic": Value::Null,
                "ExtendedStatistic": "p95",
                "EvaluateLowSampleCountPercentile": "ignore",
            }),
        ),
        (
            "a percentile statistic that is always evaluated",
            json!({
                "Statistic": Value::Null,
                "ExtendedStatistic": "p99.9",
                "EvaluateLowSampleCountPercentile": "evaluate",
            }),
        ),
        (
            "a unit, a description and the three action lists",
            json!({
                "Unit": "Count",
                "AlarmDescription": "the alarm this row configured",
                "AlarmActions": ["arn:aws:sns:us-east-1:000000000000:alarm-topic"],
                "OKActions": ["arn:aws:sns:us-east-1:000000000000:ok-topic"],
                "InsufficientDataActions": [
                    "arn:aws:sns:us-east-1:000000000000:insufficient-topic",
                ],
                "ActionsEnabled": false,
            }),
        ),
        (
            "a lower-bound comparison on a fractional threshold",
            json!({
                "ComparisonOperator": "LessThanOrEqualToThreshold",
                "Threshold": 0.5,
                "Period": 10,
            }),
        ),
        (
            "a metric identified by two dimensions",
            json!({
                "Dimensions": [
                    {"Name": "DomainName", "Value": "two-dimension-row"},
                    {"Name": "ClientId", "Value": "alpha"},
                ],
            }),
        ),
    ];

    for (index, (case, patch)) in rows.iter().enumerate() {
        let name = format!("config-row-{index}");
        let mut request = alarm_request(&name, patch);
        // Each row watches its OWN dimension set unless it states one, so every row has its
        // own metric and the rows share one emulator.
        if request.get("Dimensions").is_none() {
            request["Dimensions"] = json!([{"Name": "DomainName", "Value": name}]);
        }
        ok(&router, "PutMetricAlarm", &request).await;

        let reported = alarms_for_metric(&router, &query_for(&request)).await;
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected exactly the one alarm written on this metric, got \
             {reported:?}"
        );
        let alarm = &reported[0];

        // Every member the write carried is reported as written.
        for (member, wanted) in request.as_object().expect("the request is an object") {
            if member == "Dimensions" {
                assert_eq!(
                    dimension_set(alarm.get(member.as_str())),
                    dimension_set(Some(wanted)),
                    "case {case:?}: expected the written dimension set, got {alarm}"
                );
                continue;
            }
            assert!(
                reports(wanted, alarm.get(member.as_str())),
                "case {case:?}: expected member {member:?} to be reported as {wanted}, got \
                 {:?} in {alarm}",
                alarm.get(member.as_str())
            );
        }

        // And the record is the one the general reader reports: which call an alarm is read
        // through cannot change the configuration a client is told.
        let described = described_alarm(&router, Kind::Metric, &name)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the written alarm must be described"));
        for member in CONFIGURATION_MEMBERS {
            assert_eq!(
                alarm.get(*member),
                described.get(*member),
                "case {case:?}: both readers must report one {member:?}, got {alarm} and \
                 {described}"
            );
            // A member the write carried is reported by BOTH readers, so the comparison above
            // is not satisfied by two answers that leave it out.
            if request.get(*member).is_some() {
                assert!(
                    described.get(*member).is_some(),
                    "case {case:?}: both readers must report the written {member:?}, got \
                     {described}"
                );
            }
        }
        assert!(
            alarm
                .get("AlarmArn")
                .and_then(Value::as_str)
                .is_some_and(|arn| arn.ends_with(&format!(":alarm:{name}"))),
            "case {case:?}: expected the alarm's own ARN to be reported, got {alarm}"
        );
    }
}

#[tokio::test]
async fn the_per_metric_reader_reports_a_configuration_write_on_the_next_read() {
    let router = build_router();
    let name = "reconfigured";
    let dimensions = json!([{"Name": "DomainName", "Value": name}]);
    let request = alarm_request(
        name,
        &json!({"Dimensions": dimensions, "ActionsEnabled": true}),
    );
    ok(&router, "PutMetricAlarm", &request).await;
    let query = query_for(&request);

    // Rows differ in which writer changed the configuration and in the member it changed: the
    // two action switches, and a re-write of the alarm itself.
    let rows: &[(&str, &str, Value, &str, Value)] = &[
        (
            "the actions switched off",
            "DisableAlarmActions",
            json!({"AlarmNames": [name]}),
            "ActionsEnabled",
            json!(false),
        ),
        (
            "the actions switched back on",
            "EnableAlarmActions",
            json!({"AlarmNames": [name]}),
            "ActionsEnabled",
            json!(true),
        ),
        (
            "a new threshold",
            "PutMetricAlarm",
            alarm_request(
                name,
                &json!({"Dimensions": dimensions, "Threshold": 42.5}),
            ),
            "Threshold",
            json!(42.5),
        ),
        (
            "a new missing-data arm",
            "PutMetricAlarm",
            alarm_request(
                name,
                &json!({"Dimensions": dimensions, "TreatMissingData": "breaching"}),
            ),
            "TreatMissingData",
            json!("breaching"),
        ),
        (
            "a new datapoints-to-alarm count",
            "PutMetricAlarm",
            alarm_request(
                name,
                &json!({
                    "Dimensions": dimensions,
                    "EvaluationPeriods": 3,
                    "DatapointsToAlarm": 2,
                }),
            ),
            "DatapointsToAlarm",
            json!(2),
        ),
        (
            "another description",
            "PutMetricAlarm",
            alarm_request(
                name,
                &json!({
                    "Dimensions": dimensions,
                    "AlarmDescription": "rewritten by the next row",
                }),
            ),
            "AlarmDescription",
            json!("rewritten by the next row"),
        ),
    ];

    for (case, operation, body, member, expected) in rows {
        ok(&router, operation, body).await;

        // Read straight after the write, with no wait: a configuration member is what the
        // last write left, not something an evaluation has to come due for.
        let reported = alarms_for_metric(&router, &query).await;
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected the one alarm on this metric, got {reported:?}"
        );
        assert!(
            reports(expected, reported[0].get(*member)),
            "case {case:?}: expected member {member:?} to be reported as {expected} right \
             after the write, got {:?} in {}",
            reported[0].get(*member),
            reported[0]
        );
    }
}
