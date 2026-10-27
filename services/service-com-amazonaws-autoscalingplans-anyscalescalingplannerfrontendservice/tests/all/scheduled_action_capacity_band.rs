// SPDX-License-Identifier: CC0-1.0

//! The band a SCHEDULED SCALING ACTION of a `GetScalingPlanResourceForecastData` answer
//! provisions in.
//!
//! ONE rule is pinned here: the capacity forecast is a prediction and is bounded by neither
//! end of the instruction's capacity band, while the two capacities reported for a
//! scheduled scaling action of that forecast are — `ScheduledActionMinCapacity` is the
//! capacity forecast floored by the instruction's `MinCapacity` ("the larger of two values:
//! the capacity forecast or the minimum capacity in the scaling instruction") and held at
//! the maximum capacity that same action may reach, which the instruction's
//! `PredictiveScalingMaxCapacityBehavior` decides (under the documented default, `MaxCapacity`
//! itself, "enforced as a hard limit"). So one read of a resource never answers a
//! scheduled-action minimum above the scheduled-action maximum of the same action.
//!
//! The rows differ along what the rule generalises over: how wide the band is relative to
//! its own minimum (a band narrower than three times its minimum is one the forecast curve
//! leaves, one wider is not, and the two just either side of that ratio are both driven),
//! a band of one single capacity, the target value, and each of the three documented
//! maximum-capacity behaviors plus the behavior left unset. A second test sweeps a grid of
//! bands and behaviors and asserts only the invariant, so the ordering is checked on bands
//! no row names.
//!
//! The harness (router, dispatch, instruction builder, data-point reader) is reused from
//! [`crate::scaling_plan_lifecycle`] and [`crate::forecast_datapoints`]; nothing here builds
//! a second one.

use serde_json::{json, Value};

use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::get_scaling_plan_resource_forecast_data::{
    FORECAST_INTERVAL_SECONDS, FORECAST_SCALABLE_DIMENSION, FORECAST_SERVICE_NAMESPACE,
};

use crate::forecast_datapoints::{assert_close, points};
use crate::scaling_plan_lifecycle::{instruction, now_secs, stack_source, Emulator};

/// The Auto Scaling group every plan here scales.
const RESOURCE: &str = "autoScalingGroup/my-asg";

/// The plan every row is stored under.
const PLAN: &str = "scheduled action band plan";

/// The `PredictiveScalingMaxCapacityBehavior` values, from
/// `smithy_query(query="shape",shape="PredictiveScalingMaxCapacityBehavior")`: the
/// documented default "enforced as a hard limit", the one that lets capacity rise "to equal
/// but not exceed forecast capacity", and the one that adds "a specified buffer value".
const BEHAVIOR_HARD_LIMIT: &str = "SetForecastCapacityToMaxCapacity";
const BEHAVIOR_TO_FORECAST: &str = "SetMaxCapacityToForecastCapacity";
const BEHAVIOR_ABOVE_FORECAST: &str = "SetMaxCapacityAboveForecastCapacity";

/// How many forecast intervals every window below spans: more than a day, so a forecast
/// that varies over the day reaches both its highest and its lowest hour.
const WINDOW_PERIODS: i64 = 30;

/// How many intervals before the current one each window starts, so the whole window lies
/// inside the span forecast data exists over whatever the serving clock reads.
const WINDOW_START_PERIODS_AGO: i64 = 10;

/// How close two capacities have to be to count as the same value: both sides are computed
/// from the same double in a different order, so only floating-point slack separates them.
const TOLERANCE: f64 = 1e-9;

/// The interval-aligned window every row asks its forecast over.
fn window() -> (i64, i64) {
    let start = now_secs().div_euclid(FORECAST_INTERVAL_SECONDS) * FORECAST_INTERVAL_SECONDS
        - WINDOW_START_PERIODS_AGO * FORECAST_INTERVAL_SECONDS;
    (start, start + WINDOW_PERIODS * FORECAST_INTERVAL_SECONDS)
}

/// `value` rounded up to a whole unit of capacity, which is how capacity is counted
/// (`ResourceCapacity` is an integer in the model). A value already whole to within
/// floating-point slack stays where it is.
fn whole_units(value: f64) -> f64 {
    let nearest = value.round();
    if (value - nearest).abs() <= TOLERANCE * nearest.abs().max(1.0) {
        nearest
    } else {
        value.ceil()
    }
}

/// One stored instruction, and what its scheduled scaling actions have to report.
#[derive(Debug, Clone, Copy)]
struct Row {
    label: &'static str,
    min_capacity: i64,
    max_capacity: i64,
    /// The `TargetValue` of the single target tracking configuration.
    target: f64,
    behavior: Option<&'static str>,
    buffer: Option<i64>,
    /// Whether the capacity forecast of this band is expected to leave the band somewhere
    /// in the window: a forecast is a prediction, not a capacity the band bounds.
    forecast_leaves_band: bool,
}

impl Row {
    /// This row's instruction, as the wire carries it.
    fn instruction(&self) -> Value {
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
        if let Some(buffer) = self.buffer {
            value["PredictiveScalingMaxCapacityBuffer"] = json!(buffer);
        }
        value
    }

    /// The maximum capacity a scheduled scaling action of this instruction may reach, given
    /// the capacity forecast of its interval — "the calculation used is determined by the
    /// predictive scaling maximum capacity behavior setting in the scaling instruction".
    fn scheduled_action_max_capacity(&self, capacity_forecast: f64) -> f64 {
        let max_capacity = self.max_capacity as f64;
        match self.behavior {
            // "may scale resource capacity higher than the maximum capacity to equal but
            // not exceed forecast capacity"
            Some(BEHAVIOR_TO_FORECAST) => max_capacity.max(capacity_forecast),
            // "higher than the maximum capacity by a specified buffer value", the buffer
            // being "a percentage relative to the forecast capacity"
            Some(BEHAVIOR_ABOVE_FORECAST) => {
                let buffer = self.buffer.expect("this behavior requires a buffer") as f64;
                max_capacity.max(whole_units(capacity_forecast * (1.0 + buffer / 100.0)))
            }
            // `SetForecastCapacityToMaxCapacity`, the documented default: "The maximum
            // capacity is enforced as a hard limit."
            _ => max_capacity,
        }
    }

    /// THE RULE for the scheduled-action minimum: the capacity forecast floored by the
    /// instruction's own minimum capacity and held at the maximum capacity the same
    /// scheduled scaling action may reach.
    fn scheduled_action_min_capacity(&self, capacity_forecast: f64) -> f64 {
        capacity_forecast
            .max(self.min_capacity as f64)
            .min(self.scheduled_action_max_capacity(capacity_forecast))
    }

    /// Whether this row's maximum capacity is "enforced as a hard limit": no non-default
    /// behavior is given, so no capacity of a scheduled scaling action may exceed it.
    fn enforces_hard_limit(&self) -> bool {
        self.behavior.is_none() || self.behavior == Some(BEHAVIOR_HARD_LIMIT)
    }
}

/// The table.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "a band narrower than three times its minimum, behavior unset",
            min_capacity: 6,
            max_capacity: 10,
            target: 40.0,
            behavior: None,
            buffer: None,
            forecast_leaves_band: true,
        },
        Row {
            label: "the same band at a far larger target value",
            min_capacity: 6,
            max_capacity: 10,
            target: 85.0,
            behavior: None,
            buffer: None,
            forecast_leaves_band: true,
        },
        Row {
            label: "a narrow band naming the hard limit explicitly",
            min_capacity: 8,
            max_capacity: 10,
            target: 50.0,
            behavior: Some(BEHAVIOR_HARD_LIMIT),
            buffer: None,
            forecast_leaves_band: true,
        },
        Row {
            label: "a narrow band with the maximum raised to the forecast capacity",
            min_capacity: 9,
            max_capacity: 10,
            target: 25.0,
            behavior: Some(BEHAVIOR_TO_FORECAST),
            buffer: None,
            forecast_leaves_band: true,
        },
        Row {
            label: "a narrow band with a buffer above the forecast capacity",
            min_capacity: 7,
            max_capacity: 9,
            target: 60.0,
            behavior: Some(BEHAVIOR_ABOVE_FORECAST),
            buffer: Some(25),
            forecast_leaves_band: true,
        },
        Row {
            label: "a band of one single capacity",
            min_capacity: 4,
            max_capacity: 4,
            target: 33.0,
            behavior: None,
            buffer: None,
            forecast_leaves_band: true,
        },
        Row {
            label: "a band one unit under three times its minimum",
            min_capacity: 5,
            max_capacity: 14,
            target: 45.0,
            behavior: Some(BEHAVIOR_HARD_LIMIT),
            buffer: None,
            forecast_leaves_band: true,
        },
        Row {
            label: "a band exactly three times its minimum",
            min_capacity: 5,
            max_capacity: 15,
            target: 45.0,
            behavior: None,
            buffer: None,
            forecast_leaves_band: false,
        },
        Row {
            label: "a wide band the forecast never leaves",
            min_capacity: 1,
            max_capacity: 100,
            target: 70.0,
            behavior: Some(BEHAVIOR_ABOVE_FORECAST),
            buffer: Some(100),
            forecast_leaves_band: false,
        },
    ]
}

/// An emulator holding one `Active` plan whose single instruction is `instruction`.
async fn emulator_with(instruction: Value) -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": PLAN,
                "ApplicationSource": stack_source(1),
                "ScalingInstructions": [instruction],
            }),
        )
        .await;
    emulator.make_all_due();
    emulator
}

/// The capacity forecast and the two scheduled-action capacities of one stored instruction
/// over `window`, one triple per forecast interval.
///
/// Every row's instruction carries no `ScheduledActionBufferTime`, so a scheduled scaling
/// action is reported at the instant of the interval it provisions for and the three lists
/// line up interval by interval.
async fn forecast_triples(emulator: &Emulator, window: (i64, i64)) -> Vec<(i64, f64, f64, f64)> {
    let capacity = points(emulator, PLAN, RESOURCE, "CapacityForecast", window).await;
    let minimum = points(emulator, PLAN, RESOURCE, "ScheduledActionMinCapacity", window).await;
    let maximum = points(emulator, PLAN, RESOURCE, "ScheduledActionMaxCapacity", window).await;
    assert_eq!(
        (capacity.len(), minimum.len()),
        (maximum.len(), maximum.len()),
        "a forecast reports the same intervals for every data type, got \
         {capacity:?} / {minimum:?} / {maximum:?}"
    );
    assert!(
        !capacity.is_empty(),
        "a predictive instruction has a forecast over {window:?}, got nothing"
    );
    capacity
        .into_iter()
        .zip(minimum)
        .zip(maximum)
        .map(|(((at, capacity), (min_at, minimum)), (max_at, maximum))| {
            assert_eq!(
                (min_at, max_at),
                (at, at),
                "the scheduled-action capacities of an interval are reported at its own \
                 instant when no buffer time is stored"
            );
            (at, capacity, minimum, maximum)
        })
        .collect()
}

#[tokio::test]
async fn a_scheduled_action_capacity_stays_inside_the_band_the_instruction_allows() {
    let window = window();

    for row in rows() {
        let emulator = emulator_with(row.instruction()).await;
        let triples = forecast_triples(&emulator, window).await;
        let label = row.label;
        let max_capacity = row.max_capacity as f64;
        let min_capacity = row.min_capacity as f64;
        let mut left_band = false;

        for (at, capacity, minimum, maximum) in &triples {
            let case = format!("{label:?} at {at}");
            if *capacity > max_capacity || *capacity < min_capacity {
                left_band = true;
            }

            // The invariant: one read of one resource never answers a scheduled-action
            // minimum above the scheduled-action maximum of the same action.
            assert!(
                minimum <= maximum,
                "case {case}: expected the scheduled-action minimum at or below the \
                 scheduled-action maximum, got minimum {minimum:?} and maximum {maximum:?}"
            );
            // "the larger of two values: the capacity forecast or the minimum capacity in
            // the scaling instruction"
            assert!(
                *minimum >= min_capacity,
                "case {case}: expected the scheduled-action minimum at or above the \
                 instruction's minimum capacity {min_capacity}, got {minimum:?}"
            );
            assert_close(
                *minimum,
                row.scheduled_action_min_capacity(*capacity),
                &format!("{case} / ScheduledActionMinCapacity"),
            );
            assert_close(
                *maximum,
                row.scheduled_action_max_capacity(*capacity),
                &format!("{case} / ScheduledActionMaxCapacity"),
            );

            // With no non-default behavior given, "the maximum capacity is enforced as a
            // hard limit" on both capacities of the action.
            if row.enforces_hard_limit() {
                assert!(
                    *minimum <= max_capacity && *maximum <= max_capacity,
                    "case {case}: the maximum capacity {max_capacity} is a hard limit \
                     here, got minimum {minimum:?} and maximum {maximum:?}"
                );
            }
        }

        // …while the capacity FORECAST is a prediction the band does not bound, so a band
        // the forecast curve leaves still reports values outside it.
        assert_eq!(
            left_band,
            row.forecast_leaves_band,
            "case {label:?}: expected the capacity forecast to {} the band \
             [{min_capacity}, {max_capacity}], got {:?}",
            if row.forecast_leaves_band {
                "leave"
            } else {
                "stay inside"
            },
            triples
                .iter()
                .map(|(_, capacity, _, _)| *capacity)
                .collect::<Vec<f64>>()
        );
    }
}

#[tokio::test]
async fn the_ordering_holds_on_every_band_and_behavior_of_a_grid() {
    // The invariant on bands no row above names: every band of this grid, under each
    // documented behavior and with the behavior left unset. The minimums span the smallest
    // capacity an instruction may ask for upwards, and each maximum is a multiple of its
    // own minimum, so both sides of the ratio at which the forecast curve leaves the band
    // are covered.
    let window = window();
    let behaviors: [(Option<&str>, Option<i64>); 4] = [
        (None, None),
        (Some(BEHAVIOR_HARD_LIMIT), None),
        (Some(BEHAVIOR_TO_FORECAST), None),
        (Some(BEHAVIOR_ABOVE_FORECAST), Some(10)),
    ];

    for min_capacity in [1_i64, 2, 7, 13] {
        for factor in [1_i64, 2, 3, 5] {
            let max_capacity = min_capacity * factor;
            for (behavior, buffer) in behaviors {
                let row = Row {
                    label: "a grid band",
                    min_capacity,
                    max_capacity,
                    target: 40.0,
                    behavior,
                    buffer,
                    forecast_leaves_band: false,
                };
                let emulator = emulator_with(row.instruction()).await;
                for (at, capacity, minimum, maximum) in forecast_triples(&emulator, window).await {
                    let case =
                        format!("band [{min_capacity}, {max_capacity}] {behavior:?} at {at}");
                    assert!(
                        minimum <= maximum,
                        "case {case}: expected the scheduled-action minimum at or below the \
                         scheduled-action maximum, got minimum {minimum:?} and maximum \
                         {maximum:?}"
                    );
                    assert_close(
                        minimum,
                        row.scheduled_action_min_capacity(capacity),
                        &format!("{case} / ScheduledActionMinCapacity"),
                    );
                }
            }
        }
    }
}
