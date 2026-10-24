// SPDX-License-Identifier: CC0-1.0

//! The CONTENT of a `GetScalingPlanResourceForecastData` answer, over the real wire path.
//!
//! One rule is pinned here: a forecast is computed from the stored scaling instruction
//! that names the requested resource — one data point per forecast interval over the
//! requested window — and the four `ForecastDataType`s report the quantities the model
//! defines, each from that instruction:
//!
//! * `LoadForecast` is the instruction's load metric, an aggregate over the resource;
//! * `CapacityForecast` is the whole capacity that holds every target tracking
//!   configuration of the instruction at its target under that load;
//! * `ScheduledActionMinCapacity` is "the larger of two values: the capacity forecast or
//!   the minimum capacity in the scaling instruction";
//! * `ScheduledActionMaxCapacity` follows the instruction's
//!   `PredictiveScalingMaxCapacityBehavior` and `PredictiveScalingMaxCapacityBuffer`;
//!
//! and an instruction that configures no predictive scaling, a stored plan holding no
//! instruction for the requested resource, and a plan this service does not hold have no
//! forecast at all.
//!
//! The RELATIONS are asserted between the four answers rather than against numbers
//! written down here: each row reads the load forecast the service computed and derives
//! what the other three have to carry from it, so the table checks the rule and not one
//! particular curve. Rows differ along what the rule generalises over: the capacity band,
//! the number of target tracking configurations and their target values, the form of the
//! load metric specification (predefined vs customized), the maximum-capacity behavior
//! and its buffer, and the predictive scaling mode.
//!
//! The harness (router, dispatch, instruction builder) is reused from
//! [`crate::scaling_plan_lifecycle`]; nothing here builds a second one.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::get_scaling_plan_resource_forecast_data::{
    FORECAST_HORIZON_DAYS, FORECAST_INTERVAL_SECONDS, FORECAST_SCALABLE_DIMENSION,
    FORECAST_SERVICE_NAMESPACE, MAX_FORECAST_HISTORY_DAYS,
};
use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::types::FORECAST_DATA_TYPE_VALUES;

use crate::scaling_plan_lifecycle::{instruction, stack_source, Emulator};

/// The operation under test.
const OPERATION: &str = "GetScalingPlanResourceForecastData";

/// Seconds in a day, for placing windows.
const DAY: i64 = 86_400;

/// The Auto Scaling group every plan here scales. `ResourceId` "consists of a prefix
/// (autoScalingGroup) followed by the name of a specified Auto Scaling group (my-asg)".
const RESOURCE: &str = "autoScalingGroup/my-asg";

/// An Auto Scaling group no plan here holds an instruction for.
const OTHER_RESOURCE: &str = "autoScalingGroup/other-asg";

/// The only scaling-plan version the model documents as valid.
const ONLY_VERSION: i64 = 1;

/// The `PredictiveScalingMaxCapacityBehavior` values, from
/// `smithy_query(query="shape",shape="PredictiveScalingMaxCapacityBehavior")`.
const BEHAVIOR_HARD_LIMIT: &str = "SetForecastCapacityToMaxCapacity";
const BEHAVIOR_TO_FORECAST: &str = "SetMaxCapacityToForecastCapacity";
const BEHAVIOR_ABOVE_FORECAST: &str = "SetMaxCapacityAboveForecastCapacity";

/// The `PredictiveScalingMode` value under which no scheduled scaling action is created.
const MODE_FORECAST_ONLY: &str = "ForecastOnly";

/// How close two doubles have to be to count as the same value, relative to their
/// magnitude: the two sides of every relation below are computed from the same double in
/// a different order, so only floating-point slack separates them.
const TOLERANCE: f64 = 1e-9;

/// Now, as the epoch seconds an `awsJson1_1` timestamp is carried as.
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a host clock at or after the epoch")
        .as_secs() as i64
}

/// The start of the forecast interval containing now.
fn current_interval_start() -> i64 {
    now_secs().div_euclid(FORECAST_INTERVAL_SECONDS) * FORECAST_INTERVAL_SECONDS
}

/// One forecast request, with every member the model marks required.
fn forecast_request(
    plan: &str,
    resource_id: &str,
    data_type: &str,
    window: (i64, i64),
) -> Value {
    json!({
        "ScalingPlanName": plan,
        "ScalingPlanVersion": ONLY_VERSION,
        "ServiceNamespace": FORECAST_SERVICE_NAMESPACE,
        "ResourceId": resource_id,
        "ScalableDimension": FORECAST_SCALABLE_DIMENSION,
        "ForecastDataType": data_type,
        "StartTime": window.0,
        "EndTime": window.1,
    })
}

/// The `(timestamp in epoch seconds, value)` pairs of one successful answer.
///
/// Both members of `Datapoint` are read: a data point that carries neither is not a
/// forecast of anything.
async fn points(
    emulator: &Emulator,
    plan: &str,
    resource_id: &str,
    data_type: &str,
    window: (i64, i64),
) -> Vec<(i64, f64)> {
    let body = emulator
        .ok(OPERATION, forecast_request(plan, resource_id, data_type, window))
        .await;
    let listed = body
        .get("Datapoints")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {data_type}: expected a Datapoints list, got {body}"))
        .clone();
    listed
        .iter()
        .map(|point| {
            let timestamp = point
                .get("Timestamp")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| panic!("case {data_type}: a data point needs a Timestamp, got {point}"));
            let value = point
                .get("Value")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| panic!("case {data_type}: a data point needs a Value, got {point}"));
            assert_eq!(
                timestamp.fract(),
                0.0,
                "case {data_type}: a forecast interval starts on a whole second, got {timestamp}"
            );
            (timestamp as i64, value)
        })
        .collect()
}

/// `value` rounded up to a whole unit of capacity, which is how capacity is counted
/// (`ResourceCapacity` is an integer in the model, and the only dimension forecast here is
/// an Auto Scaling group's desired capacity). A value already whole to within
/// floating-point slack stays where it is.
fn whole_units(value: f64) -> f64 {
    let nearest = value.round();
    if (value - nearest).abs() <= TOLERANCE * nearest.abs().max(1.0) {
        nearest
    } else {
        value.ceil()
    }
}

/// Assert two doubles are the same value to within floating-point slack.
fn assert_close(actual: f64, expected: f64, case: &str) {
    assert!(
        (actual - expected).abs() <= TOLERANCE * expected.abs().max(1.0),
        "case {case}: expected {expected}, got {actual}"
    );
}

/// How a row's load metric is specified: both forms configure predictive scaling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoadMetric {
    /// A `PredefinedLoadMetricSpecification` of this `LoadMetricType`.
    Predefined(&'static str),
    /// A `CustomizedLoadMetricSpecification` of this metric name, whose only valid
    /// statistic is `Sum`.
    Customized(&'static str),
    /// Neither — the instruction configures dynamic scaling only.
    None,
}

/// One row of the table: a stored scaling instruction, and what its forecast has to obey.
#[derive(Debug, Clone)]
struct Case {
    label: &'static str,
    min_capacity: i64,
    max_capacity: i64,
    /// `(ScalingMetricType, TargetValue)` per target tracking configuration. Each metric
    /// is distinct, which the service requires, and each metric is one the Auto Scaling
    /// group dimension supports (source: `SCALING_METRIC_TYPE_VALUES` and
    /// `smithy_query(query="doc",shape="PredefinedScalingMetricSpecification",member="PredefinedScalingMetricType")`).
    targets: &'static [(&'static str, f64)],
    load_metric: LoadMetric,
    behavior: Option<&'static str>,
    buffer: Option<i64>,
    mode: Option<&'static str>,
}

impl Case {
    /// The smallest target value of the instruction: the load one unit of capacity
    /// carries while every target tracking configuration holds its metric at its target.
    fn load_per_unit(&self) -> f64 {
        self.targets
            .iter()
            .map(|(_, target)| *target)
            .fold(f64::INFINITY, f64::min)
    }

    /// Whether the instruction configures predictive scaling, which is what a load metric
    /// specification does.
    fn is_predictive(&self) -> bool {
        self.load_metric != LoadMetric::None
    }

    /// Whether the instruction creates scheduled scaling actions from its forecast.
    fn creates_scheduled_actions(&self) -> bool {
        self.mode != Some(MODE_FORECAST_ONLY)
    }

    /// This row's instruction, as the wire carries it.
    fn instruction(&self) -> Value {
        let (first_metric, first_target) = self.targets[0];
        let mut value = instruction(
            FORECAST_SERVICE_NAMESPACE,
            RESOURCE,
            FORECAST_SCALABLE_DIMENSION,
            self.min_capacity,
            self.max_capacity,
            first_metric,
            first_target,
        );
        for (metric, target) in &self.targets[1..] {
            value["TargetTrackingConfigurations"]
                .as_array_mut()
                .expect("the instruction builder writes a list here")
                .push(json!({
                    "PredefinedScalingMetricSpecification": {
                        "PredefinedScalingMetricType": metric
                    },
                    "TargetValue": target
                }));
        }
        match self.load_metric {
            LoadMetric::Predefined(metric_type) => {
                value["PredefinedLoadMetricSpecification"] =
                    json!({ "PredefinedLoadMetricType": metric_type });
            }
            LoadMetric::Customized(metric_name) => {
                value["CustomizedLoadMetricSpecification"] = json!({
                    "MetricName": metric_name,
                    "Namespace": "AWS/EC2",
                    "Statistic": "Sum",
                });
            }
            LoadMetric::None => {}
        }
        if let Some(behavior) = self.behavior {
            value["PredictiveScalingMaxCapacityBehavior"] = json!(behavior);
        }
        if let Some(buffer) = self.buffer {
            value["PredictiveScalingMaxCapacityBuffer"] = json!(buffer);
        }
        if let Some(mode) = self.mode {
            value["PredictiveScalingMode"] = json!(mode);
        }
        value
    }

    /// The maximum capacity a scheduled scaling action of this instruction may reach,
    /// given the capacity forecast of one interval.
    fn scheduled_action_max_capacity(&self, capacity_forecast: f64) -> f64 {
        let max_capacity = self.max_capacity as f64;
        match self.behavior {
            // "may scale resource capacity higher than the maximum capacity to equal but
            // not exceed forecast capacity"
            Some(BEHAVIOR_TO_FORECAST) => max_capacity.max(capacity_forecast),
            // "higher than the maximum capacity by a specified buffer value … specified as
            // a percentage relative to the forecast capacity"
            Some(BEHAVIOR_ABOVE_FORECAST) => {
                let buffer = self.buffer.expect("this behavior requires a buffer") as f64;
                max_capacity.max(whole_units(capacity_forecast * (1.0 + buffer / 100.0)))
            }
            // `SetForecastCapacityToMaxCapacity`, the documented default: "The maximum
            // capacity is enforced as a hard limit."
            _ => max_capacity,
        }
    }
}

/// The table. Rows differ in capacity band, in how many target tracking configurations
/// they carry and with what targets, in the form of the load metric specification, in the
/// maximum-capacity behavior and buffer, and in the predictive scaling mode.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "a predefined load metric, one target, default behavior",
            min_capacity: 2,
            max_capacity: 10,
            targets: &[("ASGAverageCPUUtilization", 40.0)],
            load_metric: LoadMetric::Predefined("ASGTotalCPUUtilization"),
            behavior: None,
            buffer: None,
            mode: None,
        },
        Case {
            label: "a narrow band whose minimum capacity bites at the trough",
            min_capacity: 8,
            max_capacity: 10,
            targets: &[("ASGAverageCPUUtilization", 50.0)],
            load_metric: LoadMetric::Predefined("ASGTotalCPUUtilization"),
            behavior: Some(BEHAVIOR_HARD_LIMIT),
            buffer: None,
            mode: Some("ForecastAndScale"),
        },
        Case {
            label: "a wide band with the maximum capacity raised to the forecast",
            min_capacity: 1,
            max_capacity: 400,
            targets: &[("ASGAverageNetworkIn", 1_048_576.0)],
            load_metric: LoadMetric::Predefined("ASGTotalNetworkIn"),
            behavior: Some(BEHAVIOR_TO_FORECAST),
            buffer: None,
            mode: None,
        },
        Case {
            label: "the smallest buffer above the forecast capacity",
            min_capacity: 3,
            max_capacity: 12,
            targets: &[("ASGAverageCPUUtilization", 25.0)],
            load_metric: LoadMetric::Predefined("ASGTotalCPUUtilization"),
            behavior: Some(BEHAVIOR_ABOVE_FORECAST),
            buffer: Some(1),
            mode: None,
        },
        Case {
            label: "the largest buffer above the forecast capacity",
            min_capacity: 4,
            max_capacity: 9,
            targets: &[("ASGAverageCPUUtilization", 60.0)],
            load_metric: LoadMetric::Predefined("ASGTotalCPUUtilization"),
            behavior: Some(BEHAVIOR_ABOVE_FORECAST),
            buffer: Some(100),
            mode: None,
        },
        Case {
            label: "three target tracking configurations, the smallest target sizing the capacity",
            min_capacity: 2,
            max_capacity: 30,
            targets: &[
                ("ASGAverageCPUUtilization", 70.0),
                ("ASGAverageNetworkIn", 12.5),
                ("ASGAverageNetworkOut", 45.0),
            ],
            load_metric: LoadMetric::Predefined("ASGTotalNetworkIn"),
            behavior: Some(BEHAVIOR_ABOVE_FORECAST),
            buffer: Some(30),
            mode: None,
        },
        Case {
            label: "a customized load metric",
            min_capacity: 5,
            max_capacity: 25,
            targets: &[("ASGAverageCPUUtilization", 33.0)],
            load_metric: LoadMetric::Customized("MyQueueDepth"),
            behavior: None,
            buffer: None,
            mode: None,
        },
        Case {
            label: "a plan that forecasts but creates no scheduled scaling action",
            min_capacity: 2,
            max_capacity: 20,
            targets: &[("ASGAverageCPUUtilization", 40.0)],
            load_metric: LoadMetric::Predefined("ASGTotalCPUUtilization"),
            behavior: None,
            buffer: None,
            mode: Some(MODE_FORECAST_ONLY),
        },
        Case {
            label: "dynamic scaling only: no load metric specification, so no forecast",
            min_capacity: 2,
            max_capacity: 10,
            targets: &[("ASGAverageCPUUtilization", 40.0)],
            load_metric: LoadMetric::None,
            behavior: None,
            buffer: None,
            mode: None,
        },
    ]
}

/// An emulator holding one `Active` plan whose instructions are `instructions`.
async fn emulator_with(plan: &str, instructions: Vec<Value>) -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": plan,
                "ApplicationSource": stack_source(1),
                "ScalingInstructions": instructions,
            }),
        )
        .await;
    emulator.make_all_due();
    emulator
}

/// A window of whole forecast intervals, hour-aligned and well inside the span over which
/// forecast data exists, so which intervals fall in it does not depend on the serving
/// clock. It spans more than a day, so a forecast that varies over the day shows it.
fn settled_window() -> (i64, i64) {
    let start = current_interval_start() - 12 * FORECAST_INTERVAL_SECONDS;
    (start, start + 36 * FORECAST_INTERVAL_SECONDS)
}

/// Every forecast interval inside `window`.
fn expected_instants(window: (i64, i64)) -> Vec<i64> {
    let (start, end) = window;
    let mut instants = Vec::new();
    let mut at = start.div_euclid(FORECAST_INTERVAL_SECONDS) * FORECAST_INTERVAL_SECONDS;
    if at < start {
        at += FORECAST_INTERVAL_SECONDS;
    }
    while at < end {
        instants.push(at);
        at += FORECAST_INTERVAL_SECONDS;
    }
    instants
}

#[tokio::test]
async fn the_four_forecast_data_types_are_computed_from_the_stored_instruction() {
    let window = settled_window();
    let instants = expected_instants(window);

    for case in cases() {
        let plan = "forecast plan";
        let emulator = emulator_with(plan, vec![case.instruction()]).await;
        let load = points(&emulator, plan, RESOURCE, "LoadForecast", window).await;
        let capacity = points(&emulator, plan, RESOURCE, "CapacityForecast", window).await;
        let scheduled_min =
            points(&emulator, plan, RESOURCE, "ScheduledActionMinCapacity", window).await;
        let scheduled_max =
            points(&emulator, plan, RESOURCE, "ScheduledActionMaxCapacity", window).await;
        let label = case.label;

        if !case.is_predictive() {
            for (data_type, got) in [
                ("LoadForecast", &load),
                ("CapacityForecast", &capacity),
                ("ScheduledActionMinCapacity", &scheduled_min),
                ("ScheduledActionMaxCapacity", &scheduled_max),
            ] {
                assert!(
                    got.is_empty(),
                    "case {label:?} / {data_type}: an instruction configuring no \
                     predictive scaling has no forecast, got {got:?}"
                );
            }
            continue;
        }

        // One data point per forecast interval of the window, in order.
        for (data_type, got) in [("LoadForecast", &load), ("CapacityForecast", &capacity)] {
            let timestamps: Vec<i64> = got.iter().map(|(at, _)| *at).collect();
            assert_eq!(
                timestamps, instants,
                "case {label:?} / {data_type}: expected one data point per forecast \
                 interval of {window:?}"
            );
        }

        // The two scheduled-action capacities exist exactly where the instruction creates
        // scheduled scaling actions.
        for (data_type, got) in [
            ("ScheduledActionMinCapacity", &scheduled_min),
            ("ScheduledActionMaxCapacity", &scheduled_max),
        ] {
            let timestamps: Vec<i64> = got.iter().map(|(at, _)| *at).collect();
            let expected = if case.creates_scheduled_actions() {
                instants.clone()
            } else {
                Vec::new()
            };
            assert_eq!(
                timestamps, expected,
                "case {label:?} / {data_type}: a scheduled-action capacity is reported \
                 only where the instruction creates scheduled scaling actions"
            );
        }

        let load_per_unit = case.load_per_unit();
        for (index, (at, load_value)) in load.iter().enumerate() {
            let at_case = format!("{label:?} at {at}");
            assert!(
                *load_value > 0.0,
                "case {at_case}: a load forecast is a positive load, got {load_value}"
            );

            // The capacity forecast holds every target tracking configuration at its
            // target under the forecast load, in whole units.
            let (capacity_at, capacity_value) = capacity[index];
            assert_eq!(
                capacity_at, *at,
                "case {at_case}: the capacity forecast covers the same intervals"
            );
            let expected_capacity = whole_units(load_value / load_per_unit);
            assert_close(
                capacity_value,
                expected_capacity,
                &format!("{at_case} / CapacityForecast"),
            );

            if !case.creates_scheduled_actions() {
                continue;
            }
            // "the larger of two values: the capacity forecast or the minimum capacity in
            // the scaling instruction"
            let (min_at, min_value) = scheduled_min[index];
            assert_eq!(
                min_at, *at,
                "case {at_case}: the scheduled-action minimum covers the same intervals"
            );
            assert_close(
                min_value,
                capacity_value.max(case.min_capacity as f64),
                &format!("{at_case} / ScheduledActionMinCapacity"),
            );

            // determined by the maximum-capacity behavior of the instruction
            let (max_at, max_value) = scheduled_max[index];
            assert_eq!(
                max_at, *at,
                "case {at_case}: the scheduled-action maximum covers the same intervals"
            );
            assert_close(
                max_value,
                case.scheduled_action_max_capacity(capacity_value),
                &format!("{at_case} / ScheduledActionMaxCapacity"),
            );
        }

        // A forecast is a curve over the window, not one repeated number.
        let distinct = load
            .iter()
            .map(|(_, value)| value.to_bits())
            .collect::<std::collections::BTreeSet<u64>>();
        assert!(
            distinct.len() > 1,
            "case {label:?}: a forecast over {} intervals must vary over time, got {load:?}",
            instants.len()
        );
    }
}

#[tokio::test]
async fn the_minimum_capacity_of_the_instruction_floors_the_scheduled_action_minimum() {
    // The clamp in "the larger of two values" has to BITE somewhere, or the rule is
    // untested. Rows differ in how high the instruction's own minimum capacity sits
    // inside its band: a minimum close to the band's maximum is above the capacity
    // forecast at the quieter hours of the day, a minimum far below it never is.
    let window = settled_window();
    let cases: [(&str, i64, i64, bool); 4] = [
        ("a minimum at the top of a narrow band", 9, 10, true),
        ("a minimum one unit below the maximum", 7, 8, true),
        ("a minimum far below a wide maximum", 1, 100, false),
        ("a minimum a tenth of the maximum", 2, 20, false),
    ];

    for (label, min_capacity, max_capacity, clamp_expected) in cases {
        let case = Case {
            label,
            min_capacity,
            max_capacity,
            targets: &[("ASGAverageCPUUtilization", 40.0)],
            load_metric: LoadMetric::Predefined("ASGTotalCPUUtilization"),
            behavior: None,
            buffer: None,
            mode: None,
        };
        let plan = "floor plan";
        let emulator = emulator_with(plan, vec![case.instruction()]).await;
        let capacity = points(&emulator, plan, RESOURCE, "CapacityForecast", window).await;
        let scheduled_min =
            points(&emulator, plan, RESOURCE, "ScheduledActionMinCapacity", window).await;
        let mut clamped = 0;
        for (index, (at, capacity_value)) in capacity.iter().enumerate() {
            let (_, min_value) = scheduled_min[index];
            assert_close(
                min_value,
                capacity_value.max(min_capacity as f64),
                &format!("{label:?} at {at}"),
            );
            if min_value > *capacity_value {
                clamped += 1;
            }
        }
        assert_eq!(
            clamped > 0,
            clamp_expected,
            "case {label:?}: expected the instruction's minimum capacity to {} the \
             capacity forecast somewhere in {window:?}, and it clamped {clamped} of {} \
             intervals",
            if clamp_expected { "floor" } else { "never floor" },
            capacity.len()
        );
    }
}

#[tokio::test]
async fn a_forecast_is_reported_only_for_a_resource_a_stored_plan_instructs() {
    let window = settled_window();
    let case = Case {
        label: "a predictive plan",
        min_capacity: 2,
        max_capacity: 10,
        targets: &[("ASGAverageCPUUtilization", 40.0)],
        load_metric: LoadMetric::Predefined("ASGTotalCPUUtilization"),
        behavior: None,
        buffer: None,
        mode: None,
    };
    let plan = "held plan";
    let emulator = emulator_with(plan, vec![case.instruction()]).await;

    // Rows differ along what the lookup generalises over: whether the named plan is held,
    // and whether the plan that is held instructs the named resource.
    let misses: [(&str, &str, &str); 3] = [
        ("a plan this service does not hold", "no such plan", RESOURCE),
        (
            "a plan that instructs another resource",
            plan,
            OTHER_RESOURCE,
        ),
        (
            "neither the plan nor the resource",
            "no such plan",
            OTHER_RESOURCE,
        ),
    ];
    for data_type in FORECAST_DATA_TYPE_VALUES {
        for (label, named_plan, resource) in misses {
            let got = points(&emulator, named_plan, resource, data_type, window).await;
            assert!(
                got.is_empty(),
                "case {label:?} / {data_type}: nothing instructs this resource, so there \
                 is no forecast, got {got:?}"
            );
        }
        // …while the resource the stored plan does instruct has one.
        let got = points(&emulator, plan, RESOURCE, data_type, window).await;
        assert!(
            !got.is_empty(),
            "case the stored plan / {data_type}: the stored instruction covers this \
             resource, so its forecast is not empty"
        );
    }
}

#[tokio::test]
async fn the_forecast_follows_the_stored_instruction_and_not_the_request() {
    // Two plans differing in ONE member of the instruction forecast differently, which is
    // what "computed from state" means. Rows name the member they vary.
    let window = settled_window();
    let base = Case {
        label: "the baseline instruction",
        min_capacity: 2,
        max_capacity: 10,
        targets: &[("ASGAverageCPUUtilization", 40.0)],
        load_metric: LoadMetric::Predefined("ASGTotalCPUUtilization"),
        behavior: None,
        buffer: None,
        mode: None,
    };
    let variants: [(&str, Case); 3] = [
        (
            "MaxCapacity",
            Case {
                max_capacity: 20,
                ..base.clone()
            },
        ),
        (
            "MinCapacity",
            Case {
                min_capacity: 6,
                ..base.clone()
            },
        ),
        (
            "the target value",
            Case {
                targets: &[("ASGAverageCPUUtilization", 80.0)],
                ..base.clone()
            },
        ),
    ];

    let plan = "varied plan";
    let baseline = {
        let emulator = emulator_with(plan, vec![base.instruction()]).await;
        points(&emulator, plan, RESOURCE, "LoadForecast", window).await
    };
    assert!(
        !baseline.is_empty(),
        "the baseline instruction must have a forecast to compare against"
    );
    for (member, variant) in variants {
        let emulator = emulator_with(plan, vec![variant.instruction()]).await;
        let varied = points(&emulator, plan, RESOURCE, "LoadForecast", window).await;
        let timestamps: Vec<i64> = varied.iter().map(|(at, _)| *at).collect();
        assert_eq!(
            timestamps,
            baseline.iter().map(|(at, _)| *at).collect::<Vec<i64>>(),
            "case {member:?}: the window decides the intervals, not the instruction"
        );
        assert!(
            varied
                .iter()
                .zip(&baseline)
                .all(|((_, varied_value), (_, baseline_value))| varied_value != baseline_value),
            "case {member:?}: changing it must change the forecast, got {varied:?} for \
             both"
        );
    }
}

#[tokio::test]
async fn the_same_window_read_twice_reports_the_same_forecast() {
    let window = settled_window();
    let case = Case {
        label: "a predictive plan",
        min_capacity: 3,
        max_capacity: 15,
        targets: &[("ASGAverageCPUUtilization", 45.0)],
        load_metric: LoadMetric::Predefined("ASGTotalCPUUtilization"),
        behavior: Some(BEHAVIOR_ABOVE_FORECAST),
        buffer: Some(20),
        mode: None,
    };
    let plan = "stable plan";
    let emulator = emulator_with(plan, vec![case.instruction()]).await;
    for data_type in FORECAST_DATA_TYPE_VALUES {
        let first = points(&emulator, plan, RESOURCE, data_type, window).await;
        let second = points(&emulator, plan, RESOURCE, data_type, window).await;
        assert_eq!(
            first, second,
            "case {data_type}: a forecast is a function of stored state, so a second \
             read reports the same data points"
        );
        // A narrower window is the same curve, restricted: the hours it shares with the
        // wider one carry the same values.
        let narrower = (window.0 + FORECAST_INTERVAL_SECONDS, window.1 - DAY);
        let restricted = points(&emulator, plan, RESOURCE, data_type, narrower).await;
        let expected: Vec<(i64, f64)> = first
            .iter()
            .copied()
            .filter(|(at, _)| *at >= narrower.0 && *at < narrower.1)
            .collect();
        assert_eq!(
            restricted, expected,
            "case {data_type}: narrowing the window restricts the same forecast"
        );
    }
}

#[tokio::test]
async fn the_forecast_reaches_the_documented_horizon_and_no_further() {
    // "AWS Auto Scaling only issues forecasts for periods of two days in advance", so a
    // window reaching past that horizon is served up to it and no further — while the
    // request itself is accepted, because `EndTime` "can accept a date and time that is
    // more than two days in the future".
    let case = Case {
        label: "a predictive plan",
        min_capacity: 2,
        max_capacity: 10,
        targets: &[("ASGAverageCPUUtilization", 40.0)],
        load_metric: LoadMetric::Predefined("ASGTotalCPUUtilization"),
        behavior: None,
        buffer: None,
        mode: None,
    };
    let plan = "horizon plan";
    let emulator = emulator_with(plan, vec![case.instruction()]).await;
    let horizon_days = FORECAST_HORIZON_DAYS;

    // `(label, window relative to now in seconds, whether any data point is expected)`.
    let cases: [(&str, (i64, i64), bool); 4] = [
        (
            "a window crossing the horizon",
            (0, (horizon_days + 1) * DAY),
            true,
        ),
        (
            "a window entirely past the horizon",
            ((horizon_days + 1) * DAY, (horizon_days + 3) * DAY),
            false,
        ),
        (
            "a window inside the documented history",
            (-30 * DAY, -30 * DAY + 6 * FORECAST_INTERVAL_SECONDS),
            true,
        ),
        (
            "a window at the far end of the documented history",
            (
                -MAX_FORECAST_HISTORY_DAYS * DAY + 2 * FORECAST_INTERVAL_SECONDS,
                -MAX_FORECAST_HISTORY_DAYS * DAY + 8 * FORECAST_INTERVAL_SECONDS,
            ),
            true,
        ),
    ];

    for (label, (start_offset, end_offset), expected_any) in cases {
        let issued_at = now_secs();
        let window = (issued_at + start_offset, issued_at + end_offset);
        let got = points(&emulator, plan, RESOURCE, "CapacityForecast", window).await;
        assert_eq!(
            !got.is_empty(),
            expected_any,
            "case {label:?} ({window:?}): expected data points: {expected_any}, got {got:?}"
        );
        let horizon = issued_at + horizon_days * DAY;
        for (at, _) in &got {
            assert!(
                *at >= window.0 && *at < window.1,
                "case {label:?}: a data point sits inside the requested window, got {at}"
            );
            // The serving clock is read a few milliseconds after this request is built, so
            // the horizon is compared with one interval of slack.
            assert!(
                *at <= horizon + FORECAST_INTERVAL_SECONDS,
                "case {label:?}: no data point lies past the documented horizon \
                 ({horizon}), got {at}"
            );
        }
        if label == "a window crossing the horizon" {
            let last = got.last().map(|(at, _)| *at).unwrap_or_default();
            assert!(
                last >= horizon - 2 * FORECAST_INTERVAL_SECONDS,
                "case {label:?}: the forecast must reach up to the horizon ({horizon}), \
                 and stops at {last}"
            );
        }
    }
}
