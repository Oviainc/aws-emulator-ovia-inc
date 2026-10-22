// SPDX-License-Identifier: CC0-1.0

//! The alarms a target-tracking policy mints for a CUSTOMIZED metric specification.
//!
//! A customized specification describes the series the policy tracks completely — "the
//! namespace of the metric", "the name of the metric", "the dimensions of the metric", "the
//! statistic of the metric" and "the unit of the metric" — so the alarms that drive the
//! policy watch THAT series: all five members are carried onto both alarms, because an
//! alarm that aggregated the series differently, or read it in another unit, would measure
//! a different signal from the one the policy declared.
//!
//! The table below spans the statistics the model's `MetricStatistic` enum publishes and a
//! span of CloudWatch's own units, including a specification that names no unit at all.

use serde_json::{json, Value};

use super::emulator::{alarm_kinds, Emulator};

/// A customized metric is a signal a service scales from only where its capacity can be
/// driven by an arbitrary series; an ECS service is such a resource.
const DIMENSION: &str = "ecs:service:DesiredCount";

/// The customer's own series every row tracks.
const NAMESPACE: &str = "sim2real/behavioral";
const METRIC_NAME: &str = "Backlog";
const DIMENSIONS: &[(&str, &str)] = &[("Queue", "orders"), ("Shard", "7")];

/// One row: the statistic and unit the specification declares.
struct SpecRow {
    case: &'static str,
    /// A value of the model's `MetricStatistic` enum (`METRIC_STATISTIC_VALUES`:
    /// Average | Maximum | Minimum | SampleCount | Sum).
    statistic: &'static str,
    /// A value of CloudWatch's `StandardUnit` enum (that service's
    /// `STANDARD_UNIT_VALUES`), or `None` for a specification that names no unit.
    unit: Option<&'static str>,
}

/// The table: every statistic the model publishes, each with a different unit — and the
/// two cases a unit can be absent in.
const SPEC_TABLE: &[SpecRow] = &[
    SpecRow {
        case: "a summed count",
        statistic: "Sum",
        unit: Some("Count"),
    },
    SpecRow {
        case: "an averaged percentage",
        statistic: "Average",
        unit: Some("Percent"),
    },
    SpecRow {
        case: "a maximum in milliseconds",
        statistic: "Maximum",
        unit: Some("Milliseconds"),
    },
    SpecRow {
        case: "a minimum in bytes",
        statistic: "Minimum",
        unit: Some("Bytes"),
    },
    SpecRow {
        case: "a sample count in a rate unit",
        statistic: "SampleCount",
        unit: Some("Count/Second"),
    },
    SpecRow {
        case: "the explicit None unit",
        statistic: "Sum",
        unit: Some("None"),
    },
    SpecRow {
        case: "no unit at all",
        statistic: "Sum",
        unit: None,
    },
];

#[tokio::test]
async fn customized_metric_alarms_watch_the_customers_series() {
    for row in SPEC_TABLE {
        let case = row.case;
        let emulator = Emulator::empty();
        let target = emulator.given_target(DIMENSION, 1, 20).await;

        let mut specification = json!({
            "MetricName": METRIC_NAME,
            "Namespace": NAMESPACE,
            "Dimensions": DIMENSIONS.iter()
                .map(|(name, value)| json!({"Name": name, "Value": value}))
                .collect::<Vec<Value>>(),
            "Statistic": row.statistic,
        });
        if let Some(unit) = row.unit {
            specification["Unit"] = json!(unit);
        }
        let mut request = target.clone();
        request["PolicyName"] = json!("tracker");
        request["PolicyType"] = json!("TargetTrackingScaling");
        request["TargetTrackingScalingPolicyConfiguration"] = json!({
            "TargetValue": 100.0,
            "CustomizedMetricSpecification": specification.clone(),
        });
        let (status, body) = emulator.call("PutScalingPolicy", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body:?}"
        );

        // A customized metric is already the quantity the target names, so there is no
        // provisioned-capacity pair: one alarm per scaling direction.
        let alarms = emulator.target_tracking_alarms().await;
        assert_eq!(
            alarm_kinds(&alarms),
            vec!["AlarmHigh".to_string(), "AlarmLow".to_string()],
            "case {case:?}: expected a high and a low alarm, got {alarms:?}"
        );

        for alarm in &alarms {
            assert_eq!(
                alarm["Namespace"].as_str(),
                Some(NAMESPACE),
                "case {case:?}: expected the alarm to watch namespace {NAMESPACE}, got {alarm:?}"
            );
            assert_eq!(
                alarm["MetricName"].as_str(),
                Some(METRIC_NAME),
                "case {case:?}: expected the alarm to watch metric {METRIC_NAME}, got {alarm:?}"
            );
            assert_eq!(
                alarm["Dimensions"], specification["Dimensions"],
                "case {case:?}: expected the alarm to carry the specification's dimensions {:?}, \
                 got {alarm:?}",
                specification["Dimensions"]
            );
            assert_eq!(
                alarm["Statistic"].as_str(),
                Some(row.statistic),
                "case {case:?}: expected the alarm to aggregate with {}, got {alarm:?}",
                row.statistic
            );
            assert_eq!(
                alarm["Unit"].as_str(),
                row.unit,
                "case {case:?}: expected the alarm's unit to be {:?}, got {alarm:?}",
                row.unit
            );
        }
    }
}
