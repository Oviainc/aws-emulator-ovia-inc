// SPDX-License-Identifier: CC0-1.0

//! `GetPredictiveScalingForecast`, end to end.
//!
//! A forecast is not a document the service files away: it is COMPUTED, from the load the
//! policy's metric specification names and from the scalable target's capacity window. The
//! rules the tables below assert, over a span of metric specification forms, statistics,
//! windows, target values and capacity windows:
//!
//! * a forecast reports one slot per forecast interval (3600 seconds): every hour boundary
//!   inside the requested range, from the inclusive `StartTime` up to the exclusive
//!   `EndTime`, and the capacity forecast reports at the same instants as the load
//!   forecast;
//! * each load slot is the hourly load observed in CloudWatch for the specification's load
//!   series at that hour OF THE DAY, falling back to the mean over every observed hour;
//! * a slot the analysis has no observed load for carries NO VALUE at all — the
//!   not-a-number double, which the awsJson protocols put on the wire as its name — and so
//!   does the capacity slot computed from it;
//! * each capacity slot is the load divided by the target utilisation, rounded up, held
//!   inside the target's `MinCapacity`/`MaxCapacity` window — and `MaxCapacity` gives way
//!   instead under `IncreaseMaxCapacity`;
//! * a policy that is not a `PredictiveScaling` policy (and a policy name that names
//!   nothing) has no forecast, and a window outside the documented bounds is refused; the
//!   refusal for a policy of another type reports that no predictive scaling policy answers
//!   to the name the request named, and nothing about the policy stored under it.

use serde_json::{json, Value};

use super::emulator::{error_code, error_message, Emulator};

/// The forecast interval: the model documents "the forecast interval duration of 3600
/// seconds (60 minutes)" (`PredictiveScalingPolicyConfiguration$SchedulingBufferTime`).
const INTERVAL: i64 = 3_600;

/// The ECS service every row scales, and the CloudWatch dimensions its metrics carry:
/// an `AWS/ECS` service metric is dimensioned by cluster and service name.
const DIMENSION: &str = "ecs:service:DesiredCount";
const ECS_DIMENSIONS: &[(&str, &str)] = &[("ClusterName", "prod-cluster"), ("ServiceName", "web-app")];

/// The policy name every row puts its policy under.
const POLICY: &str = "forecaster";

/// How many past hours a row publishes its observations into.
const PUBLISHED_HOURS: i64 = 5;

/// The namespace prefix AWS reserves for its own services' metrics, which no client may
/// publish into (`PutMetricDataInput$Namespace`).
const RESERVED_NAMESPACE_PREFIX: &str = "AWS/";

/// One row of the forecast table.
struct ForecastRow {
    case: &'static str,
    /// The `PredictiveScalingPolicyConfiguration` the policy is put with.
    configuration: fn() -> Value,
    /// The CloudWatch series the row publishes into: namespace, metric name, dimensions.
    series: (&'static str, &'static str, &'static [(&'static str, &'static str)]),
    /// The values published in EACH of the last [`PUBLISHED_HOURS`] hours. Empty publishes
    /// nothing at all.
    values_per_hour: &'static [f64],
    /// The target's capacity window.
    min_capacity: i64,
    max_capacity: i64,
    /// How many hours past the current hour the window starts, and how many hours it spans.
    start_hours_from_now: i64,
    window_hours: i64,
    /// Whether `StartTime` is pushed 17 minutes past the hour, so the first slot is the
    /// NEXT hour boundary — the first hourly data point inside the requested range.
    start_off_the_hour: bool,
    /// The hourly load every slot must forecast, and the capacity that load needs.
    expected_load: f64,
    expected_capacity: f64,
}

/// A metric-pair specification: the pair "determines the appropriate scaling metric and
/// load metric to use", and the ECS CPU pair's load metric is the service's total CPU
/// utilisation, summed over the interval.
fn metric_pair_configuration() -> Value {
    json!({"MetricSpecifications": [{
        "TargetValue": 40.0,
        "PredefinedMetricPairSpecification": {"PredefinedMetricType": "ECSServiceCPUUtilization"},
    }]})
}

/// A load metric named on its own, as a TOTAL (summed) memory metric.
fn predefined_load_configuration() -> Value {
    json!({"MetricSpecifications": [{
        "TargetValue": 30.0,
        "PredefinedLoadMetricSpecification": {"PredefinedMetricType": "ECSServiceTotalMemoryUtilization"},
        "PredefinedScalingMetricSpecification": {"PredefinedMetricType": "ECSServiceAverageMemoryUtilization"},
    }]})
}

/// A load metric named on its own, as an AVERAGE metric: the hourly load is then the mean
/// of that hour's observations rather than their sum.
fn predefined_average_load_configuration() -> Value {
    json!({"MetricSpecifications": [{
        "TargetValue": 40.0,
        "PredefinedLoadMetricSpecification": {"PredefinedMetricType": "ECSServiceAverageCPUUtilization"},
        "PredefinedScalingMetricSpecification": {"PredefinedMetricType": "ECSServiceAverageCPUUtilization"},
    }]})
}

/// A customized load metric: a series of the caller's own choosing, aggregated with the
/// statistic the query names.
fn customized_load_configuration() -> Value {
    json!({"MetricSpecifications": [{
        "TargetValue": 5.0,
        "PredefinedScalingMetricSpecification": {"PredefinedMetricType": "ECSServiceAverageCPUUtilization"},
        "CustomizedLoadMetricSpecification": {"MetricDataQueries": [{
            "Id": "load",
            "MetricStat": {
                "Metric": {
                    "Namespace": "sim2real/forecast",
                    "MetricName": "Backlog",
                    "Dimensions": [{"Name": "Queue", "Value": "orders"}],
                },
                "Stat": "Sum",
            },
        }]},
    }]})
}

/// The same metric pair, with the maximum allowed to give way to the forecast instead of
/// capping it: "Defines the behavior that should be applied if the forecast capacity
/// approaches or exceeds the maximum capacity."
fn increase_max_capacity_configuration() -> Value {
    json!({"MetricSpecifications": [{
        "TargetValue": 1.0,
        "PredefinedMetricPairSpecification": {"PredefinedMetricType": "ECSServiceCPUUtilization"},
    }],
    "MaxCapacityBreachBehavior": "IncreaseMaxCapacity",
    "MaxCapacityBuffer": 10})
}

/// The table. Each row differs along what the rule generalises over: the form the load
/// metric is named in, the statistic its hourly load is aggregated with, how many
/// observations an hour carries, the length and alignment of the requested window, the
/// target utilisation and the capacity window the forecast capacity is held in.
const FORECAST_TABLE: &[ForecastRow] = &[
    ForecastRow {
        case: "a metric pair, one observation an hour, a one-hour window",
        configuration: metric_pair_configuration,
        series: ("AWS/ECS", "CPUUtilization", ECS_DIMENSIONS),
        values_per_hour: &[240.0],
        min_capacity: 1,
        max_capacity: 20,
        start_hours_from_now: 0,
        window_hours: 1,
        start_off_the_hour: false,
        // 240 of load at a target utilisation of 40 needs 6 capacity units.
        expected_load: 240.0,
        expected_capacity: 6.0,
    },
    ForecastRow {
        case: "a metric pair, a two-day window starting off the hour",
        configuration: metric_pair_configuration,
        series: ("AWS/ECS", "CPUUtilization", ECS_DIMENSIONS),
        values_per_hour: &[100.0, 140.0],
        min_capacity: 1,
        max_capacity: 20,
        start_hours_from_now: 0,
        window_hours: 48,
        start_off_the_hour: true,
        // Summed: the hourly load is both observations of that hour.
        expected_load: 240.0,
        expected_capacity: 6.0,
    },
    ForecastRow {
        case: "a predefined total load metric of another metric",
        configuration: predefined_load_configuration,
        series: ("AWS/ECS", "MemoryUtilization", ECS_DIMENSIONS),
        values_per_hour: &[100.0],
        min_capacity: 1,
        max_capacity: 20,
        start_hours_from_now: 2,
        window_hours: 3,
        start_off_the_hour: false,
        // 100 of load at a target of 30 needs 3.33 units, so 4 whole units.
        expected_load: 100.0,
        expected_capacity: 4.0,
    },
    ForecastRow {
        case: "a predefined AVERAGE load metric, two observations an hour",
        configuration: predefined_average_load_configuration,
        series: ("AWS/ECS", "CPUUtilization", ECS_DIMENSIONS),
        values_per_hour: &[30.0, 50.0],
        min_capacity: 1,
        max_capacity: 20,
        start_hours_from_now: 0,
        window_hours: 4,
        start_off_the_hour: false,
        // Averaged, not summed: the hourly load is the mean of the two observations.
        expected_load: 40.0,
        expected_capacity: 1.0,
    },
    ForecastRow {
        case: "a customized load metric, held at the target's floor",
        configuration: customized_load_configuration,
        series: (
            "sim2real/forecast",
            "Backlog",
            &[("Queue", "orders")],
        ),
        values_per_hour: &[12.0],
        min_capacity: 4,
        max_capacity: 20,
        start_hours_from_now: 1,
        window_hours: 6,
        start_off_the_hour: false,
        // 12 of load at a target of 5 needs 2.4 units, but the target is never below its
        // MinCapacity of 4.
        expected_load: 12.0,
        expected_capacity: 4.0,
    },
    ForecastRow {
        case: "a load the capacity window caps",
        configuration: metric_pair_configuration,
        series: ("AWS/ECS", "CPUUtilization", ECS_DIMENSIONS),
        values_per_hour: &[4_000.0],
        min_capacity: 1,
        max_capacity: 20,
        start_hours_from_now: 0,
        window_hours: 2,
        start_off_the_hour: false,
        // 4000 at a target of 40 needs 100 units, which HonorMaxCapacity caps at 20.
        expected_load: 4_000.0,
        expected_capacity: 20.0,
    },
    ForecastRow {
        case: "a load past the maximum, with IncreaseMaxCapacity",
        configuration: increase_max_capacity_configuration,
        series: ("AWS/ECS", "CPUUtilization", ECS_DIMENSIONS),
        values_per_hour: &[57.0],
        min_capacity: 1,
        max_capacity: 20,
        start_hours_from_now: 0,
        window_hours: 2,
        start_off_the_hour: false,
        // The maximum gives way instead of capping the forecast.
        expected_load: 57.0,
        expected_capacity: 57.0,
    },
];

#[tokio::test]
async fn a_forecast_reports_hourly_slots_computed_from_the_observed_load() {
    for row in FORECAST_TABLE {
        let case = row.case;
        let emulator = Emulator::empty();
        let target = emulator
            .given_target(DIMENSION, row.min_capacity, row.max_capacity)
            .await;
        put_predictive_policy(&emulator, &target, (row.configuration)()).await;

        let now = now_secs();
        let current_hour = now - now.rem_euclid(INTERVAL);
        let (namespace, metric_name, dimensions) = row.series;
        for hour in 1..=PUBLISHED_HOURS {
            publish(
                &emulator,
                namespace,
                metric_name,
                dimensions,
                current_hour - hour * INTERVAL,
                row.values_per_hour,
            )
            .await;
        }

        let window_start = current_hour + row.start_hours_from_now * INTERVAL;
        let start = window_start + if row.start_off_the_hour { 17 * 60 } else { 0 };
        let end = window_start + row.window_hours * INTERVAL;
        let mut request = target.clone();
        request["PolicyName"] = json!(POLICY);
        request["StartTime"] = json!(start);
        request["EndTime"] = json!(end);
        let (status, body) = emulator
            .call("GetPredictiveScalingForecast", request)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body:?}"
        );

        // One load forecast, carrying back the specification it was computed for.
        let forecasts = body["LoadForecast"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            forecasts.len(),
            1,
            "case {case:?}: expected one load forecast, got {:?}",
            body["LoadForecast"]
        );
        let submitted = (row.configuration)()["MetricSpecifications"][0].clone();
        assert_eq!(
            canonical(&forecasts[0]["MetricSpecification"]),
            canonical(&submitted),
            "case {case:?}: expected the forecast to name the policy's metric specification \
             {submitted:?}, got {:?}",
            forecasts[0]["MetricSpecification"]
        );

        // The slots: every hour boundary inside the requested range, which for a start
        // time past the hour begins at the next one.
        let first_slot = window_start + if row.start_off_the_hour { INTERVAL } else { 0 };
        let expected_slots: Vec<i64> = ::std::iter::successors(Some(first_slot), |slot| {
            Some(slot + INTERVAL).filter(|next| *next < end)
        })
        .collect();
        let slot_count = expected_slots.len();
        let load_timestamps = numbers(&forecasts[0]["Timestamps"]);
        assert_eq!(
            load_timestamps,
            expected_slots.iter().map(|slot| *slot as f64).collect::<Vec<f64>>(),
            "case {case:?}: expected hourly slots {expected_slots:?}, got {load_timestamps:?}"
        );
        let capacity_timestamps = numbers(&body["CapacityForecast"]["Timestamps"]);
        assert_eq!(
            capacity_timestamps, load_timestamps,
            "case {case:?}: expected the capacity forecast at the same instants as the load \
             forecast {load_timestamps:?}, got {capacity_timestamps:?}"
        );

        // The values: the hourly load, and the capacity that load needs.
        let load_values = numbers(&forecasts[0]["Values"]);
        assert_eq!(
            load_values,
            vec![row.expected_load; slot_count],
            "case {case:?}: expected every slot to forecast a load of {}, got {load_values:?}",
            row.expected_load
        );
        let capacity_values = numbers(&body["CapacityForecast"]["Values"]);
        assert_eq!(
            capacity_values,
            vec![row.expected_capacity; slot_count],
            "case {case:?}: expected every slot to forecast a capacity of {}, got \
             {capacity_values:?}",
            row.expected_capacity
        );

        // "The time the forecast was made": this call made it.
        let update_time = body["UpdateTime"].as_f64().unwrap_or_default();
        assert!(
            (update_time - now as f64).abs() <= 60.0,
            "case {case:?}: expected an UpdateTime at the time of the call ({now}), got \
             {update_time}"
        );
    }
}

#[tokio::test]
async fn a_forecast_follows_the_hour_of_day_trend_of_the_observed_load() {
    let emulator = Emulator::empty();
    let target = emulator.given_target(DIMENSION, 1, 20).await;
    put_predictive_policy(
        &emulator,
        &target,
        json!({"MetricSpecifications": [{
            "TargetValue": 100.0,
            "PredefinedMetricPairSpecification": {"PredefinedMetricType": "ECSServiceCPUUtilization"},
        }]}),
    )
    .await;

    let now = now_secs();
    let current_hour = now - now.rem_euclid(INTERVAL);
    // Yesterday's load at this time of day, and at an hour later: the forecast of each hour
    // of the day is the load that hour of the day was observed at.
    publish(
        &emulator,
        "AWS/ECS",
        "CPUUtilization",
        ECS_DIMENSIONS,
        current_hour - 24 * INTERVAL,
        &[600.0],
    )
    .await;
    publish(
        &emulator,
        "AWS/ECS",
        "CPUUtilization",
        ECS_DIMENSIONS,
        current_hour - 23 * INTERVAL,
        &[200.0],
    )
    .await;

    let mut request = target.clone();
    request["PolicyName"] = json!(POLICY);
    request["StartTime"] = json!(current_hour);
    request["EndTime"] = json!(current_hour + 3 * INTERVAL);
    let (status, body) = emulator
        .call("GetPredictiveScalingForecast", request)
        .await;
    assert_eq!(
        status, 200,
        "case \"hour-of-day trend\": expected HTTP 200, got {status} {body:?}"
    );

    // This hour of the day was observed at 600 and the next at 200; the third hour of the
    // day was never observed, so it falls back to the mean over the observed hours.
    let load_values = numbers(&body["LoadForecast"][0]["Values"]);
    assert_eq!(
        load_values,
        vec![600.0, 200.0, 400.0],
        "case \"hour-of-day trend\": expected the load of each hour of the day, got {load_values:?}"
    );
    let capacity_values = numbers(&body["CapacityForecast"]["Values"]);
    assert_eq!(
        capacity_values,
        vec![6.0, 2.0, 4.0],
        "case \"hour-of-day trend\": expected the capacity each hourly load needs at a target of \
         100, got {capacity_values:?}"
    );
}

/// One row of the no-observed-load table.
struct NoHistoryRow {
    case: &'static str,
    /// The `PredictiveScalingPolicyConfiguration` the policy is put with.
    configuration: fn() -> Value,
    /// The CloudWatch series the row publishes into: namespace, metric name, dimensions.
    series: (&'static str, &'static str, &'static [(&'static str, &'static str)]),
    /// How many hours before the current hour the row publishes ONE observation at. Empty
    /// publishes nothing at all.
    published_hours_ago: &'static [i64],
    /// The value each of those observations carries.
    value_published: f64,
    /// The target's capacity window.
    min_capacity: i64,
    max_capacity: i64,
    /// How many hours past the current hour the requested window spans.
    window_hours: i64,
    /// Whether `StartTime` is pushed 17 minutes past the hour, so the first slot is the
    /// NEXT hour boundary.
    start_off_the_hour: bool,
    /// The load and the capacity every slot must forecast, or `None` where every slot must
    /// carry no value at all.
    expected: Option<(f64, f64)>,
}

/// How long before now the historical analysis reads: "A minimum of 24 hours of data is
/// required to create the initial forecasts. However, having a full 14 days of historical
/// data results in more accurate forecasts"
/// (`smithy_query(query="doc",shape="com.amazonaws.applicationautoscaling#GetPredictiveScalingForecast")`),
/// so an observation older than 14 days is outside the window the forecast is computed from.
const LOOKBACK_HOURS: i64 = 14 * 24;

/// The table. Each row differs along what the rule generalises over: the form the load
/// metric is named in, the series it names, whether anything was ever published for it and
/// how far back, the length and alignment of the requested window, the target utilisation
/// and the capacity window — including the one behaviour that lets `MaxCapacity` give way,
/// which must not turn a value-less slot into a number either.
const NO_HISTORY_TABLE: &[NoHistoryRow] = &[
    NoHistoryRow {
        case: "a metric pair whose series nothing was ever published for",
        configuration: metric_pair_configuration,
        series: ("AWS/ECS", "CPUUtilization", ECS_DIMENSIONS),
        published_hours_ago: &[],
        value_published: 0.0,
        min_capacity: 2,
        max_capacity: 20,
        window_hours: 3,
        start_off_the_hour: false,
        expected: None,
    },
    NoHistoryRow {
        case: "a predefined load metric whose series nothing was ever published for",
        configuration: predefined_load_configuration,
        series: ("AWS/ECS", "MemoryUtilization", ECS_DIMENSIONS),
        published_hours_ago: &[],
        value_published: 0.0,
        min_capacity: 1,
        max_capacity: 5,
        window_hours: 1,
        start_off_the_hour: false,
        expected: None,
    },
    NoHistoryRow {
        case: "a customized load metric whose series nothing was ever published for, over a \
               window starting off the hour",
        configuration: customized_load_configuration,
        series: ("sim2real/forecast", "Backlog", &[("Queue", "orders")]),
        published_hours_ago: &[],
        value_published: 0.0,
        min_capacity: 4,
        max_capacity: 20,
        window_hours: 6,
        start_off_the_hour: true,
        expected: None,
    },
    NoHistoryRow {
        case: "a series published to only OUTSIDE the analysis window",
        configuration: metric_pair_configuration,
        series: ("AWS/ECS", "CPUUtilization", ECS_DIMENSIONS),
        published_hours_ago: &[LOOKBACK_HOURS + 1, LOOKBACK_HOURS + 6 * 24],
        value_published: 240.0,
        min_capacity: 1,
        max_capacity: 20,
        window_hours: 2,
        start_off_the_hour: false,
        expected: None,
    },
    NoHistoryRow {
        case: "a series nothing was published for, with MaxCapacity allowed to give way",
        configuration: increase_max_capacity_configuration,
        series: ("AWS/ECS", "CPUUtilization", ECS_DIMENSIONS),
        published_hours_ago: &[],
        value_published: 0.0,
        min_capacity: 1,
        max_capacity: 20,
        window_hours: 2,
        start_off_the_hour: false,
        expected: None,
    },
    NoHistoryRow {
        case: "a series published to just INSIDE the analysis window",
        configuration: metric_pair_configuration,
        series: ("AWS/ECS", "CPUUtilization", ECS_DIMENSIONS),
        published_hours_ago: &[LOOKBACK_HOURS - 24],
        value_published: 240.0,
        min_capacity: 1,
        max_capacity: 20,
        window_hours: 2,
        start_off_the_hour: false,
        // 240 of load at a target utilisation of 40 needs 6 capacity units: the hour of the
        // day the observation fell in forecasts it, and the other hours the mean over the
        // observed hours, which is the same single value.
        expected: Some((240.0, 6.0)),
    },
];

#[tokio::test]
async fn a_forecast_with_no_observed_load_carries_no_values() {
    for row in NO_HISTORY_TABLE {
        let case = row.case;
        let emulator = Emulator::empty();
        let target = emulator
            .given_target(DIMENSION, row.min_capacity, row.max_capacity)
            .await;
        put_predictive_policy(&emulator, &target, (row.configuration)()).await;

        let now = now_secs();
        let current_hour = now - now.rem_euclid(INTERVAL);
        let (namespace, metric_name, dimensions) = row.series;
        for hours_ago in row.published_hours_ago {
            publish(
                &emulator,
                namespace,
                metric_name,
                dimensions,
                current_hour - hours_ago * INTERVAL,
                &[row.value_published],
            )
            .await;
        }

        let start = current_hour + if row.start_off_the_hour { 17 * 60 } else { 0 };
        let end = current_hour + row.window_hours * INTERVAL;
        let mut request = target.clone();
        request["PolicyName"] = json!(POLICY);
        request["StartTime"] = json!(start);
        request["EndTime"] = json!(end);
        let (status, body) = emulator
            .call("GetPredictiveScalingForecast", request)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body:?}"
        );

        // However the slots come out, the load and the capacity forecast report at the same
        // ones, and a value-less slot is still a slot: the window is answered.
        let expected_slots = usize::try_from(row.window_hours - i64::from(row.start_off_the_hour))
            .expect("a window of whole hours");
        let load_values = entries(&body["LoadForecast"][0]["Values"]);
        let capacity_values = entries(&body["CapacityForecast"]["Values"]);
        assert_eq!(
            load_values.len(),
            expected_slots,
            "case {case:?}: expected {expected_slots} load slots, got {load_values:?}"
        );
        assert_eq!(
            capacity_values.len(),
            expected_slots,
            "case {case:?}: expected {expected_slots} capacity slots, got {capacity_values:?}"
        );

        match row.expected {
            None => {
                assert!(
                    load_values.iter().all(no_value),
                    "case {case:?}: expected every load slot to carry no value, got \
                     {load_values:?}"
                );
                assert!(
                    capacity_values.iter().all(no_value),
                    "case {case:?}: expected every capacity slot to carry no value, got \
                     {capacity_values:?}"
                );
            }
            Some((load, capacity)) => {
                assert!(
                    load_values.iter().all(|value| value.as_f64() == Some(load)),
                    "case {case:?}: expected every load slot to forecast {load}, got \
                     {load_values:?}"
                );
                assert!(
                    capacity_values
                        .iter()
                        .all(|value| value.as_f64() == Some(capacity)),
                    "case {case:?}: expected every capacity slot to forecast {capacity}, got \
                     {capacity_values:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_forecast_refusal_for_a_policy_of_another_type_reports_only_the_requested_name() {
    // The longest `PolicyName` the model admits (`PolicyName`, length 1-256), and one
    // carrying the punctuation its pattern (^\p{Print}+$) admits: the refusal is built from
    // whatever name the request named.
    let long_name = "p".repeat(256);
    let step_configuration = json!({"AdjustmentType": "ChangeInCapacity",
                                    "StepAdjustments": [{"MetricIntervalLowerBound": 0.0,
                                                         "ScalingAdjustment": 1}]});
    let tracking_configuration = json!({"TargetValue": 40.0,
                                        "PredefinedMetricSpecification": {"PredefinedMetricType": "ECSServiceAverageCPUUtilization"}});
    // (case, policy name, policy type, the configuration member that type carries, it)
    let rows: Vec<(&str, String, &str, &str, Value)> = vec![
        (
            "a step scaling policy",
            "stepper".to_string(),
            "StepScaling",
            "StepScalingPolicyConfiguration",
            step_configuration.clone(),
        ),
        (
            "a target tracking policy under a punctuated name",
            "night shift (v2)".to_string(),
            "TargetTrackingScaling",
            "TargetTrackingScalingPolicyConfiguration",
            tracking_configuration,
        ),
        (
            "a step scaling policy under the longest name the model admits",
            long_name,
            "StepScaling",
            "StepScalingPolicyConfiguration",
            step_configuration,
        ),
    ];

    for (case, policy_name, policy_type, member, configuration) in rows {
        let emulator = Emulator::empty();
        let target = emulator.given_target(DIMENSION, 1, 20).await;

        let mut put = target.clone();
        put["PolicyName"] = json!(policy_name);
        put["PolicyType"] = json!(policy_type);
        put[member] = configuration;
        let (status, body) = emulator.call("PutScalingPolicy", put).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the policy to be put, got {status} {body:?}"
        );

        let mut request = target.clone();
        request["PolicyName"] = json!(policy_name);
        request["StartTime"] = json!(now_secs());
        request["EndTime"] = json!(now_secs() + INTERVAL);
        let (status, body) = emulator
            .call("GetPredictiveScalingForecast", request)
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
        let expected = format!(
            "No predictive scaling policy with the name {policy_name} found. Verify the policy \
             name and try again."
        );
        let message = error_message(&body);
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
        assert!(
            !message.contains(policy_type),
            "case {case:?}: expected the refusal to report nothing about the policy stored \
             under that name, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_forecast_is_refused_for_a_policy_that_is_not_predictive() {
    let emulator = Emulator::empty();
    let target = emulator.given_target(DIMENSION, 1, 20).await;

    // One policy of every other type, each a valid policy of that type.
    let policies: &[(&str, &str, &str, Value)] = &[
        (
            "a step scaling policy",
            "stepper",
            "StepScalingPolicyConfiguration",
            json!({"AdjustmentType": "ChangeInCapacity",
                   "StepAdjustments": [{"MetricIntervalLowerBound": 0.0, "ScalingAdjustment": 1}]}),
        ),
        (
            "a target tracking policy",
            "tracker",
            "TargetTrackingScalingPolicyConfiguration",
            json!({"TargetValue": 40.0,
                   "PredefinedMetricSpecification": {"PredefinedMetricType": "ECSServiceAverageCPUUtilization"}}),
        ),
    ];
    for (case, policy_name, member, configuration) in policies {
        let mut request = target.clone();
        request["PolicyName"] = json!(policy_name);
        request["PolicyType"] = json!(if *member == "StepScalingPolicyConfiguration" {
            "StepScaling"
        } else {
            "TargetTrackingScaling"
        });
        request[*member] = configuration.clone();
        let (status, body) = emulator.call("PutScalingPolicy", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the policy to be put, got {status} {body:?}"
        );
    }

    let cases: &[(&str, &str)] = &[
        ("a step scaling policy", "stepper"),
        ("a target tracking policy", "tracker"),
        ("a policy name that names nothing", "absent"),
    ];
    for (case, policy_name) in cases {
        let mut request = target.clone();
        request["PolicyName"] = json!(policy_name);
        request["StartTime"] = json!(now_secs());
        request["EndTime"] = json!(now_secs() + INTERVAL);
        let (status, body) = emulator
            .call("GetPredictiveScalingForecast", request)
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
    }
}

#[tokio::test]
async fn a_forecast_window_outside_the_documented_bounds_is_refused() {
    let emulator = Emulator::empty();
    let target = emulator.given_target(DIMENSION, 1, 20).await;
    put_predictive_policy(&emulator, &target, metric_pair_configuration()).await;

    let now = now_secs();
    let day = 24 * INTERVAL;
    // Each bound the model publishes, and one step past it: "the maximum time duration
    // between the start and end time is 30 days" and "at most, the date and time can be one
    // year before the current date and time".
    let rows: &[(&str, i64, i64, u16)] = &[
        ("a window of exactly 30 days", now, now + 30 * day, 200),
        ("a window one hour past 30 days", now, now + 30 * day + INTERVAL, 400),
        ("a start just inside one year ago", now - 365 * day + INTERVAL, now - 365 * day + 2 * INTERVAL, 200),
        ("a start one day past one year ago", now - 366 * day, now - 366 * day + INTERVAL, 400),
        ("an end equal to the start", now, now, 400),
        ("an end before the start", now, now - INTERVAL, 400),
    ];
    for (case, start, end, expected) in rows {
        let mut request = target.clone();
        request["PolicyName"] = json!(POLICY);
        request["StartTime"] = json!(start);
        request["EndTime"] = json!(end);
        let (status, body) = emulator
            .call("GetPredictiveScalingForecast", request)
            .await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected HTTP {expected}, got {status} {body:?}"
        );
        if *expected == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_predictive_configuration_names_exactly_one_metric_specification() {
    let emulator = Emulator::empty();
    let target = emulator.given_target(DIMENSION, 1, 20).await;

    // The model states the count in prose: "This is an array, but we currently only support
    // a single metric specification." None describes no load to forecast, and two is not a
    // configuration this service supports.
    let one = metric_pair_configuration()["MetricSpecifications"][0].clone();
    let rows: &[(&str, Value, u16)] = &[
        ("no metric specification", json!([]), 400),
        ("one metric specification", json!([one]), 200),
        (
            "two metric specifications",
            json!([
                metric_pair_configuration()["MetricSpecifications"][0].clone(),
                predefined_load_configuration()["MetricSpecifications"][0].clone(),
            ]),
            400,
        ),
    ];
    for (case, specifications, expected) in rows {
        let mut request = target.clone();
        request["PolicyName"] = json!("counted");
        request["PolicyType"] = json!("PredictiveScaling");
        request["PredictiveScalingPolicyConfiguration"] =
            json!({"MetricSpecifications": specifications});
        let (status, body) = emulator.call("PutScalingPolicy", request).await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected HTTP {expected}, got {status} {body:?}"
        );
        if *expected == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_predictive_configuration_takes_only_the_modelled_enum_values() {
    let emulator = Emulator::empty();
    let target = emulator.given_target(DIMENSION, 1, 20).await;

    // Both members are closed enums — `PredictiveScalingMode` (ForecastOnly |
    // ForecastAndScale) and `PredictiveScalingMaxCapacityBreachBehavior` (HonorMaxCapacity |
    // IncreaseMaxCapacity) — and the forecast READS the breach behaviour, so a value outside
    // the enum cannot be stored to be resolved later.
    let rows: &[(&str, &str, &str, u16)] = &[
        ("the other mode", "Mode", "ForecastAndScale", 200),
        ("a mode outside the enum", "Mode", "ForecastAndHope", 400),
        (
            "the other breach behaviour",
            "MaxCapacityBreachBehavior",
            "IncreaseMaxCapacity",
            200,
        ),
        (
            "a breach behaviour outside the enum",
            "MaxCapacityBreachBehavior",
            "RaiseTheRoof",
            400,
        ),
    ];
    for (case, member, value, expected) in rows {
        let mut configuration = metric_pair_configuration();
        configuration[*member] = json!(value);
        let mut request = target.clone();
        request["PolicyName"] = json!("enumerated");
        request["PolicyType"] = json!("PredictiveScaling");
        request["PredictiveScalingPolicyConfiguration"] = configuration;
        let (status, body) = emulator.call("PutScalingPolicy", request).await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected HTTP {expected}, got {status} {body:?}"
        );
        if *expected == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {body:?}"
            );
        }
    }
}

/// Put the one predictive scaling policy a row forecasts.
async fn put_predictive_policy(emulator: &Emulator, target: &Value, configuration: Value) {
    let mut request = target.clone();
    request["PolicyName"] = json!(POLICY);
    request["PolicyType"] = json!("PredictiveScaling");
    request["PredictiveScalingPolicyConfiguration"] = configuration;
    let (status, body) = emulator.call("PutScalingPolicy", request).await;
    assert_eq!(
        status, 200,
        "expected PutScalingPolicy to answer 200, got {status} {body:?}"
    );
}

/// Publish `values` into one CloudWatch series, all stamped at `at`.
///
/// A series of the AWS-reserved namespace space is one its own AWS service authors, and no
/// client may publish into it (`PutMetricData` refuses a `Namespace` beginning with `AWS/`),
/// so such a row's observations are seeded into the metric store the way that service writes
/// them. A customer-namespace series goes over the wire, through the handler.
async fn publish(
    emulator: &Emulator,
    namespace: &str,
    metric_name: &str,
    dimensions: &[(&str, &str)],
    at: i64,
    values: &[f64],
) {
    if values.is_empty() {
        return;
    }
    if namespace.starts_with(RESERVED_NAMESPACE_PREFIX) {
        #[allow(clippy::cast_precision_loss)]
        emulator.seed_metric_data(namespace, metric_name, dimensions, at as f64, values);
        return;
    }
    let dimensions: Vec<Value> = dimensions
        .iter()
        .map(|(name, value)| json!({"Name": name, "Value": value}))
        .collect();
    let data: Vec<Value> = values
        .iter()
        .map(|value| {
            json!({"MetricName": metric_name, "Dimensions": dimensions,
                   "Value": value, "Timestamp": at})
        })
        .collect();
    let (status, body) = emulator
        .call_cloudwatch(
            "PutMetricData",
            json!({"Namespace": namespace, "MetricData": data}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {metric_name:?}: expected PutMetricData to answer 200, got {status} {body:?}"
    );
}

/// The current instant, as whole epoch seconds.
fn now_secs() -> i64 {
    ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as i64
}

/// A JSON array as its entries, so a forecast's slots can be read one by one whether or not
/// every one of them is a number.
fn entries(value: &Value) -> Vec<Value> {
    value.as_array().cloned().unwrap_or_default()
}

/// Whether a forecast slot carries NO value.
///
/// The awsJson protocols put a double that is not a finite number on the wire as its NAME
/// rather than as a JSON number (`PutMetricData` refuses exactly such a value, "NaN", on the
/// way in), so a slot with no value arrives as that name; a protocol that rendered it as a
/// bare literal is accepted here too.
fn no_value(entry: &Value) -> bool {
    match entry {
        Value::String(name) => name == "NaN",
        Value::Number(number) => number.as_f64().is_some_and(f64::is_nan),
        _ => false,
    }
}

/// A JSON array of numbers as a vector, so a forecast's slots and values compare as one.
fn numbers(value: &Value) -> Vec<f64> {
    value
        .as_array()
        .map(|values| {
            values
                .iter()
                .map(|entry| entry.as_f64().unwrap_or(f64::NAN))
                .collect()
        })
        .unwrap_or_default()
}

/// `value` with every number read as a double, so a round trip compares by VALUE rather
/// than by how the protocol happened to render a modelled double.
fn canonical(value: &Value) -> Value {
    match value {
        Value::Number(number) => json!(number.as_f64().unwrap_or(f64::NAN)),
        Value::Array(entries) => Value::Array(entries.iter().map(canonical).collect()),
        Value::Object(members) => Value::Object(
            members
                .iter()
                .map(|(member, entry)| (member.clone(), canonical(entry)))
                .collect(),
        ),
        other => other.clone(),
    }
}
