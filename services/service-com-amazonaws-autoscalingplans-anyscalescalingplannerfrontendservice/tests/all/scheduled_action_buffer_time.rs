// SPDX-License-Identifier: CC0-1.0

//! What `ScalingInstruction$ScheduledActionBufferTime` does to a forecast answer.
//!
//! ONE rule is pinned here: a scheduled scaling action runs the stored instruction's
//! buffer time BEFORE the forecast interval it provisions for ("if the forecast says to
//! add capacity at 10:00 AM, and the buffer time is 5 minutes, then the run time of the
//! corresponding scheduled scaling action will be 9:55 AM"), and the two scheduled-action
//! `ForecastDataType`s report a capacity "for each scheduled scaling action" — so each of
//! their data points is stamped with that action's run time, while the interval's own
//! quantities (`LoadForecast` and `CapacityForecast`) stay on the interval boundaries. The
//! value a shifted data point carries is still the capacity of the interval it provisions
//! for, so the buffer moves WHEN a scheduled-action capacity is reported and never WHAT it
//! is.
//!
//! The buffer an instruction that stores no buffer time is read with is the one the member
//! documents for that case ("The default is 300 seconds"), so an absent member and that
//! value describe the same instruction and are answered the same way, while
//! `DescribeScalingPlans` still echoes the member exactly as stored.
//!
//! Rows differ along what the rule generalises over: the buffer time, over the whole range
//! the member accepts and with the member absent, crossed with instructions differing in
//! capacity band, target value and maximum-capacity behavior.
//!
//! The harness (router, dispatch, instruction builder) is reused from
//! [`crate::scaling_plan_lifecycle`], and the answer is read with
//! [`crate::forecast_datapoints::points`]; nothing here builds a second one.

use serde_json::{json, Value};

use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::get_scaling_plan_resource_forecast_data::{
    DEFAULT_SCHEDULED_ACTION_BUFFER_SECONDS, FORECAST_INTERVAL_SECONDS,
    FORECAST_SCALABLE_DIMENSION, FORECAST_SERVICE_NAMESPACE,
};

use crate::forecast_datapoints::{
    assert_close, expected_instants, expected_reported_instants, in_millis, points,
};
use crate::scaling_plan_lifecycle::{instruction, now_secs, stack_source, Emulator};

/// The Auto Scaling group every plan here scales. `ResourceId` "consists of a prefix
/// (autoScalingGroup) followed by the name of a specified Auto Scaling group (my-asg)".
const RESOURCE: &str = "autoScalingGroup/my-asg";

/// The `PredictiveScalingMaxCapacityBehavior` values driven below, from
/// `smithy_query(query="shape",shape="PredictiveScalingMaxCapacityBehavior")`: the
/// documented default, under which "the maximum capacity is enforced as a hard limit", and
/// the one that lets capacity rise "to equal but not exceed forecast capacity".
const BEHAVIOR_HARD_LIMIT: &str = "SetForecastCapacityToMaxCapacity";
const BEHAVIOR_TO_FORECAST: &str = "SetMaxCapacityToForecastCapacity";

/// The two `ForecastDataType`s that report a capacity "for each scheduled scaling action",
/// and so are stamped with that action's run time.
const SCHEDULED_ACTION_DATA_TYPES: [&str; 2] =
    ["ScheduledActionMinCapacity", "ScheduledActionMaxCapacity"];

/// How many forecast intervals the window below spans: more than a day, so the forecast
/// varies over it and the rule is checked on many intervals rather than one.
const WINDOW_PERIODS: i64 = 26;

/// How many intervals before the current one the window starts: the whole span it reaches
/// over lies inside the span forecast data exists for, so which intervals fall in it does
/// not depend on the serving clock.
const WINDOW_START_PERIODS_AGO: i64 = 12;

/// The start of the forecast interval containing now.
fn current_interval_start() -> i64 {
    now_secs().div_euclid(FORECAST_INTERVAL_SECONDS) * FORECAST_INTERVAL_SECONDS
}

/// The interval-aligned window every row below asks its forecast over.
fn window() -> (i64, i64) {
    let start = current_interval_start() - WINDOW_START_PERIODS_AGO * FORECAST_INTERVAL_SECONDS;
    (start, start + WINDOW_PERIODS * FORECAST_INTERVAL_SECONDS)
}

/// The same window extended by one interval, so the capacity forecast of EVERY interval a
/// scheduled action inside [`window`] provisions for can be read back — the last such
/// action runs inside the window while the interval it provisions for starts at its end.
fn covering_window(window: (i64, i64)) -> (i64, i64) {
    (window.0, window.1 + FORECAST_INTERVAL_SECONDS)
}

/// The forecast interval boundaries inside the half-open window `[start, end)`: where the
/// quantities OF an interval — the load and capacity forecasts — are reported.
fn interval_boundaries(window: (i64, i64)) -> Vec<i64> {
    expected_instants(in_millis(window))
}

/// THE RULE, written from the documented sentence and nothing else: the scheduled scaling
/// action of a forecast interval runs `buffer` seconds before that interval, so the run
/// times reported inside the half-open window `[start, end)` are the ones that land in it —
/// which is the membership rule [`crate::forecast_datapoints::expected_reported_instants`]
/// owns, driven with that buffer as the shift.
fn expected_run_times(window: (i64, i64), buffer: i64) -> Vec<i64> {
    expected_reported_instants(in_millis(window), buffer)
}

/// The buffer times driven, spanning the whole range the member accepts: the member absent,
/// its modelled minimum (`range: { min: 0 }`, source
/// `smithy_query(query="shape",shape="ScheduledActionBufferTime")`), one second, a minute,
/// the value the member documents as its default, the ten minutes the issue reproduced
/// with, half an interval, and the largest value the documented bound accepts ("The value
/// must be less than the forecast interval duration of 3600 seconds (60 minutes)").
fn buffer_times() -> Vec<Option<i64>> {
    vec![
        None,
        Some(0),
        Some(1),
        Some(60),
        Some(DEFAULT_SCHEDULED_ACTION_BUFFER_SECONDS),
        Some(600),
        Some(FORECAST_INTERVAL_SECONDS / 2),
        Some(FORECAST_INTERVAL_SECONDS - 1),
    ]
}

/// The buffer the service uses for an instruction storing `buffer`: the stored value, or —
/// where the member is absent — the value the member documents for that case ("The default
/// is 300 seconds"), which is what an absent member asks the service to buffer by.
fn effective_buffer(buffer: Option<i64>) -> i64 {
    buffer.unwrap_or(DEFAULT_SCHEDULED_ACTION_BUFFER_SECONDS)
}

/// One instruction shape: the capacity rules the scheduled-action capacities follow from,
/// which the buffer time must leave alone.
#[derive(Debug, Clone, Copy)]
struct Shape {
    label: &'static str,
    min_capacity: i64,
    max_capacity: i64,
    target: f64,
    behavior: Option<&'static str>,
}

impl Shape {
    /// This shape's instruction with `buffer` as its `ScheduledActionBufferTime`, carrying
    /// a load metric specification so predictive scaling is configured — which is what
    /// makes the instruction forecast at all, and what the buffer member requires.
    fn instruction(&self, buffer: Option<i64>) -> Value {
        let mut value = instruction(
            FORECAST_SERVICE_NAMESPACE,
            RESOURCE,
            FORECAST_SCALABLE_DIMENSION,
            self.min_capacity,
            self.max_capacity,
            "ASGAverageCPUUtilization",
            self.target,
        );
        value["PredefinedLoadMetricSpecification"] =
            json!({ "PredefinedLoadMetricType": "ASGTotalCPUUtilization" });
        if let Some(behavior) = self.behavior {
            value["PredictiveScalingMaxCapacityBehavior"] = json!(behavior);
        }
        if let Some(seconds) = buffer {
            value["ScheduledActionBufferTime"] = json!(seconds);
        }
        value
    }

    /// "The minimum capacity for each scheduled scaling action … calculated as the larger
    /// of two values: the capacity forecast or the minimum capacity in the scaling
    /// instruction" — inside the band that scheduled scaling action provisions in, so never
    /// above the maximum capacity the same action may reach.
    fn scheduled_action_min_capacity(&self, capacity_forecast: f64) -> f64 {
        capacity_forecast
            .max(self.min_capacity as f64)
            .min(self.scheduled_action_max_capacity(capacity_forecast))
    }

    /// The maximum capacity for each scheduled scaling action, by the instruction's
    /// `PredictiveScalingMaxCapacityBehavior`.
    fn scheduled_action_max_capacity(&self, capacity_forecast: f64) -> f64 {
        let max_capacity = self.max_capacity as f64;
        match self.behavior {
            Some(BEHAVIOR_TO_FORECAST) => max_capacity.max(capacity_forecast),
            _ => max_capacity,
        }
    }

    /// What this shape's scheduled scaling action for one interval reports for `data_type`,
    /// given that interval's capacity forecast.
    fn expected_value(&self, data_type: &str, capacity_forecast: f64) -> f64 {
        if data_type == SCHEDULED_ACTION_DATA_TYPES[0] {
            self.scheduled_action_min_capacity(capacity_forecast)
        } else {
            self.scheduled_action_max_capacity(capacity_forecast)
        }
    }
}

/// The instruction shapes. Rows differ in capacity band, in target value and in
/// maximum-capacity behavior, so the shift is asserted on instructions whose
/// scheduled-action capacities are computed three different ways.
fn shapes() -> Vec<Shape> {
    vec![
        Shape {
            label: "a wide band at the default maximum-capacity behavior",
            min_capacity: 2,
            max_capacity: 10,
            target: 40.0,
            behavior: None,
        },
        Shape {
            label: "a narrow band whose minimum capacity floors the forecast",
            min_capacity: 9,
            max_capacity: 10,
            target: 50.0,
            behavior: Some(BEHAVIOR_HARD_LIMIT),
        },
        Shape {
            label: "a band with the maximum raised to the forecast capacity",
            min_capacity: 1,
            max_capacity: 4,
            target: 25.0,
            behavior: Some(BEHAVIOR_TO_FORECAST),
        },
    ]
}

/// An emulator holding one `Active` plan whose single instruction is `instruction`.
async fn emulator_with(plan: &str, instruction: Value) -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": plan,
                "ApplicationSource": stack_source(1),
                "ScalingInstructions": [instruction],
            }),
        )
        .await;
    emulator.make_all_due();
    emulator
}

/// The capacity forecast of each interval of `window`, keyed by the interval's instant.
async fn capacity_by_interval(
    emulator: &Emulator,
    plan: &str,
    window: (i64, i64),
) -> std::collections::BTreeMap<i64, f64> {
    points(emulator, plan, RESOURCE, "CapacityForecast", window)
        .await
        .into_iter()
        .collect()
}

#[tokio::test]
async fn the_buffer_time_moves_every_scheduled_action_run_time_ahead_of_its_interval() {
    let window = window();
    let boundaries = interval_boundaries(window);
    let plan = "buffer plan";

    for shape in shapes() {
        for buffer in buffer_times() {
            let case = format!("{} with buffer {buffer:?}", shape.label);
            let emulator = emulator_with(plan, shape.instruction(buffer)).await;
            let seconds = effective_buffer(buffer);

            // The quantities OF an interval stay on the interval boundaries: the buffer
            // time moves a scheduled scaling action, not the forecast itself.
            for data_type in ["LoadForecast", "CapacityForecast"] {
                let got: Vec<i64> = points(&emulator, plan, RESOURCE, data_type, window)
                    .await
                    .into_iter()
                    .map(|(at, _)| at)
                    .collect();
                assert_eq!(
                    got, boundaries,
                    "case {case:?} / {data_type}: an interval's own forecast is reported at \
                     the interval, expected {boundaries:?}, got {got:?}"
                );
            }

            let capacity = capacity_by_interval(&emulator, plan, covering_window(window)).await;
            let expected_runs = expected_run_times(window, seconds);
            assert_eq!(
                expected_runs.len(),
                boundaries.len(),
                "case {case:?}: an INTERVAL-ALIGNED window carries one scheduled scaling \
                 action per forecast interval whatever the buffer, expected {} run times, \
                 got {expected_runs:?}",
                boundaries.len()
            );

            for data_type in SCHEDULED_ACTION_DATA_TYPES {
                let got = points(&emulator, plan, RESOURCE, data_type, window).await;
                let run_times: Vec<i64> = got.iter().map(|(at, _)| *at).collect();
                assert_eq!(
                    run_times, expected_runs,
                    "case {case:?} / {data_type}: each data point is stamped with the run \
                     time of the scheduled scaling action of one interval, expected \
                     {expected_runs:?}, got {run_times:?}"
                );

                for (at, value) in got {
                    // The run time sits that many seconds before the interval it
                    // provisions for — the documented 10:00 AM -> 9:55 AM shift, stated as
                    // where inside its interval the reported instant falls.
                    let into_interval = at.rem_euclid(FORECAST_INTERVAL_SECONDS);
                    let expected_offset =
                        (FORECAST_INTERVAL_SECONDS - seconds).rem_euclid(FORECAST_INTERVAL_SECONDS);
                    assert_eq!(
                        into_interval, expected_offset,
                        "case {case:?} / {data_type} at {at}: a run time sits {seconds} \
                         seconds before an interval boundary, so it falls \
                         {expected_offset} seconds into an interval, got {into_interval}"
                    );

                    // The value is still the capacity of the interval the action
                    // provisions for, which starts `seconds` after the run time.
                    let interval = at + seconds;
                    let forecast = *capacity.get(&interval).unwrap_or_else(|| {
                        panic!(
                            "case {case:?} / {data_type} at {at}: no capacity forecast read \
                             for interval {interval}, got {capacity:?}"
                        )
                    });
                    assert_close(
                        value,
                        shape.expected_value(data_type, forecast),
                        &format!("{case:?} / {data_type} at {at}"),
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn two_plans_differing_only_in_buffer_time_report_the_same_capacities_at_other_times() {
    // The observable difference the buffer makes: two plans whose instructions differ in
    // nothing but this member report the SAME capacity for each forecast interval, each at
    // its own scheduled scaling action's run time.
    let window = window();
    let shape = shapes()[0];
    let buffered = "buffered";
    let defaulted = "defaulted";
    let seconds = 600;
    // The instruction that leaves the member out is buffered by the value the member
    // documents for that case, which differs from the one spelled out above — so the two
    // answers are stamped with different run times for the same intervals.
    let default_seconds = effective_buffer(None);

    let emulator = Emulator::new();
    for (index, (plan, buffer)) in [(buffered, Some(seconds)), (defaulted, None)]
        .into_iter()
        .enumerate()
    {
        emulator
            .ok(
                "CreateScalingPlan",
                json!({
                    "ScalingPlanName": plan,
                    // One plan per application source, so each plan brings its own.
                    "ApplicationSource": stack_source(index + 1),
                    "ScalingInstructions": [shape.instruction(buffer)],
                }),
            )
            .await;
    }
    emulator.make_all_due();

    // The member round-trips: the plan that was created with it carries it back, and the
    // plan created without it carries nothing.
    let described = emulator
        .ok("DescribeScalingPlans", json!({ "ScalingPlanNames": [buffered, defaulted] }))
        .await;
    for (plan, expected) in [(buffered, Some(seconds)), (defaulted, None)] {
        let stored = described["ScalingPlans"]
            .as_array()
            .unwrap_or_else(|| panic!("case {plan}: expected a ScalingPlans list, got {described}"))
            .iter()
            .find(|held| held["ScalingPlanName"] == json!(plan))
            .unwrap_or_else(|| panic!("case {plan}: expected the plan to be held, got {described}"))
            .clone();
        let echoed = stored["ScalingInstructions"][0]["ScheduledActionBufferTime"].as_i64();
        assert_eq!(
            echoed, expected,
            "case {plan}: expected ScheduledActionBufferTime {expected:?}, got {echoed:?}"
        );
    }

    for data_type in SCHEDULED_ACTION_DATA_TYPES {
        let with_buffer = points(&emulator, buffered, RESOURCE, data_type, window).await;
        let with_default = points(&emulator, defaulted, RESOURCE, data_type, window).await;

        // Not one instant in common: the two plans buffer by different amounts, so each
        // interval's action runs at a different instant on each plan.
        for (at, _) in &with_buffer {
            assert!(
                !with_default.iter().any(|(other, _)| other == at),
                "case {data_type}: a run time {at} buffered by {seconds} seconds must \
                 differ from every run time buffered by {default_seconds}, got \
                 {with_default:?}"
            );
        }

        // And the capacity of each interval is the same on both plans: keyed by the
        // interval each action provisions for, the two answers agree wherever they overlap.
        let defaulted_by_interval: std::collections::BTreeMap<i64, f64> = with_default
            .iter()
            .map(|(at, value)| (at + default_seconds, *value))
            .collect();
        let mut compared = 0;
        for (at, value) in &with_buffer {
            if let Some(other) = defaulted_by_interval.get(&(at + seconds)) {
                assert_close(*value, *other, &format!("{data_type} for interval {}", at + seconds));
                compared += 1;
            }
        }
        assert!(
            compared > 1,
            "case {data_type}: the two plans must share more than one forecast interval to \
             compare, compared {compared}"
        );
    }
}
