// SPDX-License-Identifier: CC0-1.0

//! WHICH data points a `GetScalingPlanResourceForecastData` window carries when the instant
//! a data point is reported at is SHIFTED off the forecast interval it belongs to, over the
//! real wire path.
//!
//! One rule is pinned here: an answer carries exactly the data points whose own reported
//! instant lies in the half-open window the request names — `StartTime` is "the inclusive
//! start time of the time range for the forecast data to get", `EndTime` "the exclusive end
//! time", and a point is stamped with the instant it is reported at
//! (`Datapoint$Timestamp`, "The time stamp for the data point in UTC format"). The reported
//! instants of one shift are the progression `k * FORECAST_INTERVAL_SECONDS - shift`, so a
//! shift moves which members of that progression a window holds and not only where inside
//! their intervals they sit:
//!
//! * every `Timestamp` an answer carries is inside the requested range, whatever the shift —
//!   selecting any other way would answer with a time the caller did not ask about;
//! * a window whose width is a whole multiple of the forecast interval holds exactly one
//!   member per interval of that width, so the two scheduled-action data types (reported a
//!   buffer time before their interval) carry as many points as the two interval data types;
//! * a window of any other width holds one member more or one fewer under one shift than
//!   under another, in either direction, because the progressions of two shifts have
//!   different phases against that window's edges.
//!
//! [`crate::forecast_window_precision`] pins the same membership rule for endpoints inside a
//! second and [`crate::scheduled_action_buffer_time`] pins what the buffer shift does to an
//! interval-aligned window; neither drives a window whose width is not a whole number of
//! intervals, which is where the count of a shifted report parts from the interval count.
//!
//! The harness (router, dispatch, instruction builder) is reused from
//! [`crate::scaling_plan_lifecycle`], the answer is read with
//! [`crate::forecast_datapoints::points`] and the membership rule itself comes from its one
//! owner, [`crate::forecast_datapoints::expected_reported_instants`]; nothing here builds a
//! second one.

use serde_json::json;

use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::get_scaling_plan_resource_forecast_data::{
    DEFAULT_SCHEDULED_ACTION_BUFFER_SECONDS, FORECAST_INTERVAL_SECONDS,
    FORECAST_SCALABLE_DIMENSION, FORECAST_SERVICE_NAMESPACE,
};

use crate::forecast_datapoints::{expected_reported_instants, in_millis, points};
use crate::scaling_plan_lifecycle::{instruction, now_secs, stack_source, Emulator};

/// The Auto Scaling group every plan here scales. `ResourceId` "consists of a prefix
/// (autoScalingGroup) followed by the name of a specified Auto Scaling group (my-asg)".
const RESOURCE: &str = "autoScalingGroup/my-asg";

/// The plan each emulator below holds.
const PLAN: &str = "shift count plan";

/// One forecast interval in seconds, for the window arithmetic below.
const PERIOD: i64 = FORECAST_INTERVAL_SECONDS;

/// The two `ForecastDataType`s reporting a quantity OF a forecast interval, which is
/// reported at the interval itself — shift zero.
const INTERVAL_DATA_TYPES: [&str; 2] = ["LoadForecast", "CapacityForecast"];

/// The two `ForecastDataType`s reporting a capacity "for each scheduled scaling action",
/// which is reported at that action's run time — a buffer time before its interval.
const SCHEDULED_ACTION_DATA_TYPES: [&str; 2] =
    ["ScheduledActionMinCapacity", "ScheduledActionMaxCapacity"];

/// The hour-aligned instant every window below is measured from: a whole forecast period in
/// the recent past, so each window lies inside the span forecast data exists over and no
/// row's answer depends on where the serving clock sits.
fn base_secs() -> i64 {
    now_secs().div_euclid(PERIOD) * PERIOD - 12 * PERIOD
}

/// The buffer times driven, spanning the whole range the member accepts: the member absent,
/// its modelled minimum (`range: { min: 0 }`, source
/// `smithy_query(query="shape",shape="ScheduledActionBufferTime")`), one second, a minute,
/// the value the member documents as its default, ten minutes, half an interval, and the
/// largest value the documented bound accepts ("The value must be less than the forecast
/// interval duration of 3600 seconds (60 minutes)").
fn buffer_times() -> Vec<Option<i64>> {
    vec![
        None,
        Some(0),
        Some(1),
        Some(60),
        Some(DEFAULT_SCHEDULED_ACTION_BUFFER_SECONDS),
        Some(600),
        Some(PERIOD / 2),
        Some(PERIOD - 1),
    ]
}

/// The buffer the service reports a scheduled scaling action with for an instruction storing
/// `buffer`: the stored value, or — where the member is absent — the value the member
/// documents for that case ("The default is 300 seconds").
fn effective_buffer(buffer: Option<i64>) -> i64 {
    buffer.unwrap_or(DEFAULT_SCHEDULED_ACTION_BUFFER_SECONDS)
}

/// One window the rule is asserted over: where it starts relative to a forecast interval
/// boundary, and how wide it is.
#[derive(Debug, Clone, Copy)]
struct WindowCase {
    label: &'static str,
    /// Seconds after the hour-aligned [`base_secs`] the window starts at.
    start_offset: i64,
    /// The window's width in seconds, inside the documented seven-day maximum.
    width: i64,
}

impl WindowCase {
    /// The `(StartTime, EndTime)` this row asks its forecast over.
    fn window(&self, base: i64) -> (i64, i64) {
        let start = base + self.start_offset;
        (start, start + self.width)
    }

    /// Whether the width is a whole number of forecast intervals, which is the one case a
    /// window holds the same number of reported instants whatever the shift.
    fn is_interval_aligned(&self) -> bool {
        self.width.rem_euclid(PERIOD) == 0
    }
}

/// The windows driven. Rows differ along what the rule generalises over: the width (a whole
/// interval, many whole intervals, a part of one, a whole one plus or minus a second, a
/// whole one plus a buffer), where the start sits inside its interval (on the boundary, a
/// second in, half-way, a buffer short of the next boundary) and how the two combine — so
/// the table spans aligned and unaligned widths crossed with aligned and unaligned starts.
fn windows() -> Vec<WindowCase> {
    let case = |label, start_offset, width| WindowCase {
        label,
        start_offset,
        width,
    };
    vec![
        case("an aligned start and a width of one whole interval", 0, PERIOD),
        case(
            "an aligned start and a width of 26 whole intervals, more than a day",
            0,
            26 * PERIOD,
        ),
        case(
            "a start half an interval in and a width of three whole intervals",
            PERIOD / 2,
            3 * PERIOD,
        ),
        case(
            "an aligned start and a width of one interval and a buffer",
            0,
            PERIOD + 300,
        ),
        case(
            "a start a buffer short of the next boundary, width one interval and a buffer",
            PERIOD - 300,
            PERIOD + 300,
        ),
        case("an aligned start and a width of half an interval", 0, PERIOD / 2),
        case(
            "a start half an interval in and a width of half an interval",
            PERIOD / 2,
            PERIOD / 2,
        ),
        case("a start one second in and a width of one interval", 1, PERIOD),
        case(
            "a start one second short of the next boundary, width one interval",
            PERIOD - 1,
            PERIOD,
        ),
        case(
            "an aligned start and a width one second short of an interval",
            0,
            PERIOD - 1,
        ),
        case(
            "an aligned start and a width one second past an interval",
            0,
            PERIOD + 1,
        ),
        case(
            "a start a buffer in and a width of 25 intervals and three quarters",
            300,
            25 * PERIOD + 2_700,
        ),
        case(
            "a start three quarters of an interval in and a width of two intervals",
            2_700,
            2 * PERIOD,
        ),
    ]
}

/// An emulator holding one `Active` plan whose single instruction configures predictive
/// scaling with `buffer` as its `ScheduledActionBufferTime`, so all four
/// `ForecastDataType`s report data points.
async fn emulator_with(buffer: Option<i64>) -> Emulator {
    let mut scaling_instruction = instruction(
        FORECAST_SERVICE_NAMESPACE,
        RESOURCE,
        FORECAST_SCALABLE_DIMENSION,
        2,
        8,
        "ASGAverageCPUUtilization",
        40.0,
    );
    // A load metric specification is what "configur[es] predictive scaling", and the
    // instruction leaves `PredictiveScalingMode` at its documented default, so scheduled
    // scaling actions exist to be reported.
    scaling_instruction["PredefinedLoadMetricSpecification"] =
        json!({ "PredefinedLoadMetricType": "ASGTotalCPUUtilization" });
    if let Some(seconds) = buffer {
        scaling_instruction["ScheduledActionBufferTime"] = json!(seconds);
    }

    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": PLAN,
                "ApplicationSource": stack_source(1),
                "ScalingInstructions": [scaling_instruction],
            }),
        )
        .await;
    emulator.make_all_due();
    emulator
}

/// The shift a data point of `data_type` is reported with, given the instruction's
/// effective buffer: zero for a quantity of the interval itself.
fn shift_of(data_type: &str, buffer_seconds: i64) -> i64 {
    if SCHEDULED_ACTION_DATA_TYPES.contains(&data_type) {
        buffer_seconds
    } else {
        0
    }
}

/// The instants one answer carries, asserted against the membership rule for that shift and
/// against the requested range itself.
async fn reported_instants(
    emulator: &Emulator,
    case: &str,
    data_type: &str,
    window: (i64, i64),
    shift: i64,
) -> Vec<i64> {
    let expected = expected_reported_instants(in_millis(window), shift);
    let got: Vec<i64> = points(emulator, PLAN, RESOURCE, data_type, window)
        .await
        .into_iter()
        .map(|(at, _)| at)
        .collect();
    assert_eq!(
        got, expected,
        "case {case} / {data_type}: a window carries the data points whose own reported \
         instant lies in it, expected {expected:?}, got {got:?}"
    );
    for at in &got {
        assert!(
            *at >= window.0 && *at < window.1,
            "case {case} / {data_type}: every reported instant lies inside the requested \
             range [{}, {}), got {at}",
            window.0,
            window.1
        );
        assert_eq!(
            at.rem_euclid(PERIOD),
            (PERIOD - shift).rem_euclid(PERIOD),
            "case {case} / {data_type}: a point reported {shift} seconds before its \
             interval sits that far short of a boundary, got {at}"
        );
    }
    got
}

#[tokio::test]
async fn a_window_carries_the_data_points_whose_reported_instant_lies_inside_it() {
    let base = base_secs();
    for buffer in buffer_times() {
        let emulator = emulator_with(buffer).await;
        let seconds = effective_buffer(buffer);
        for row in windows() {
            let window = row.window(base);
            let case = format!(
                "{} with buffer {buffer:?} over [{}, {})",
                row.label, window.0, window.1
            );
            for data_type in INTERVAL_DATA_TYPES.iter().chain(&SCHEDULED_ACTION_DATA_TYPES) {
                reported_instants(
                    &emulator,
                    &case,
                    data_type,
                    window,
                    shift_of(data_type, seconds),
                )
                .await;
            }
        }
    }
}

#[tokio::test]
async fn a_shifted_report_matches_the_interval_count_exactly_on_an_aligned_width() {
    let base = base_secs();
    // The two directions the issue reports: on a window whose width is not a whole number
    // of intervals, a buffered report carries one point FEWER than the interval report for
    // some windows and one MORE for others. Both have to show up in the table, or the rows
    // would not be driving the case the rule is about.
    let mut carried_fewer = 0_usize;
    let mut carried_more = 0_usize;

    for buffer in buffer_times() {
        let emulator = emulator_with(buffer).await;
        let seconds = effective_buffer(buffer);
        for row in windows() {
            let window = row.window(base);
            let case = format!(
                "{} with buffer {buffer:?} over [{}, {})",
                row.label, window.0, window.1
            );
            let intervals =
                reported_instants(&emulator, &case, INTERVAL_DATA_TYPES[0], window, 0)
                    .await
                    .len() as i64;

            for data_type in SCHEDULED_ACTION_DATA_TYPES {
                let shifted = reported_instants(&emulator, &case, data_type, window, seconds)
                    .await
                    .len() as i64;
                let difference = shifted - intervals;
                if row.is_interval_aligned() {
                    assert_eq!(
                        difference, 0,
                        "case {case} / {data_type}: a width of whole forecast intervals \
                         holds one reported instant per interval whatever the shift, \
                         expected {intervals} points, got {shifted}"
                    );
                } else {
                    assert!(
                        difference.abs() <= 1,
                        "case {case} / {data_type}: two shifts of the same progression \
                         differ by at most one point over a window, expected within one of \
                         {intervals} points, got {shifted}"
                    );
                    if difference < 0 {
                        carried_fewer += 1;
                    } else if difference > 0 {
                        carried_more += 1;
                    }
                }
            }
        }
    }

    assert!(
        carried_fewer > 0,
        "the table must drive a window where a buffered report carries one point fewer \
         than the interval report, got {carried_fewer} such rows"
    );
    assert!(
        carried_more > 0,
        "the table must drive a window where a buffered report carries one point more than \
         the interval report, got {carried_more} such rows"
    );
}
