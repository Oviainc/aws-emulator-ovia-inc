// SPDX-License-Identifier: CC0-1.0

//! WHICH data points a `GetScalingPlanResourceForecastData` window carries when its
//! endpoints sit inside a second, over the real wire path.
//!
//! One rule is pinned here: the forecast window is half-open at the full precision the
//! wire carries, so the forecast reports the data point of a forecast interval exactly
//! when that interval's instant is at or after `StartTime` and strictly before `EndTime`.
//! An `awsJson1_1` `TimestampType` is epoch SECONDS, so a fractional number is an endpoint
//! INSIDE a second: such an endpoint moves the window's edge within its own second, and
//! the whole-second instants on either side of that edge fall where they really sit
//! relative to it — the interval boundary before a sub-second `EndTime` is inside the
//! window, and the one at a sub-second `StartTime`'s own second is outside it.
//!
//! [`crate::forecast_data`] pins that the sub-second part of an endpoint changes no
//! VERDICT (the documented day bounds are measured in whole forecast periods) and
//! [`crate::forecast_datapoints`] pins what an accepted whole-second window carries; this
//! suite pins the membership of the window itself, which neither of those reads.
//!
//! Each row's expected points come from the ONE place that owns the membership rule —
//! [`crate::forecast_datapoints::expected_instants`], which takes the window in
//! milliseconds — and their values are taken from one wide whole-second answer, so a row
//! asserts both which instants come back and that a narrower window reports the same
//! forecast at them.
//!
//! The harness (router, dispatch, instruction builder) is reused from
//! [`crate::scaling_plan_lifecycle`] and the request builder and response reader from
//! [`crate::forecast_data`]; nothing here builds a second one.

use serde_json::{json, Value};

use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::get_scaling_plan_resource_forecast_data::FORECAST_INTERVAL_SECONDS;
use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::types::FORECAST_DATA_TYPE_VALUES;

use crate::forecast_data::{
    asg_request, current_hour_start, datapoints, epoch_seconds, MILLIS_PER_SECOND, ONLY_VERSION,
};
// The membership rule itself has one owner, measured in milliseconds; this suite drives it
// with endpoints inside a second.
use crate::forecast_datapoints::expected_instants;
use crate::scaling_plan_lifecycle::{instruction, stack_source, Emulator};

/// The operation under test.
const OPERATION: &str = "GetScalingPlanResourceForecastData";

/// One forecast period in seconds: the granularity forecast data exists at, so the
/// instants a window may carry a point at are the multiples of it.
const HOUR: i64 = FORECAST_INTERVAL_SECONDS;

/// The plan these tests hold.
const STORED_PLAN: &str = "forecast plan";

/// The Auto Scaling group that plan scales, as `crate::forecast_data` names it.
const RESOURCE: &str = "autoScalingGroup/my-asg";

/// An emulator holding one `Active` plan whose instruction configures PREDICTIVE scaling,
/// so every window that carries an interval boundary carries a data point at it.
///
/// A load metric specification is what "configur[es] predictive scaling"
/// (`smithy_query(query="doc",shape="ScalingInstruction",member="PredefinedLoadMetricSpecification")`),
/// and the instruction leaves `PredictiveScalingMode` and
/// `PredictiveScalingMaxCapacityBehavior` at their documented defaults, so all four
/// `ForecastDataType`s report a value.
async fn with_predictive_plan() -> Emulator {
    let mut scaling_instruction = instruction(
        "autoscaling",
        RESOURCE,
        "autoscaling:autoScalingGroup:DesiredCapacity",
        2,
        10,
        "ASGAverageCPUUtilization",
        40.0,
    );
    scaling_instruction["PredefinedLoadMetricSpecification"] =
        json!({ "PredefinedLoadMetricType": "ASGTotalCPUUtilization" });

    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": STORED_PLAN,
                "ApplicationSource": stack_source(1),
                "ScalingInstructions": [scaling_instruction],
            }),
        )
        .await;
    emulator.make_all_due();
    emulator
}

/// The `(instant in whole epoch seconds, value)` pairs of one successful answer.
async fn points<T: Into<Value>>(
    emulator: &Emulator,
    data_type: &str,
    window: (T, T),
    case: &str,
) -> Vec<(i64, f64)> {
    let request = asg_request(STORED_PLAN, ONLY_VERSION, data_type, window);
    let body = emulator.ok(OPERATION, request).await;
    datapoints(&body)
        .iter()
        .map(|point| {
            let timestamp = point
                .get("Timestamp")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| panic!("case {case}: a data point needs a Timestamp, got {point}"));
            let value = point
                .get("Value")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| panic!("case {case}: a data point needs a Value, got {point}"));
            assert_eq!(
                timestamp.fract(),
                0.0,
                "case {case}: a forecast interval starts on a whole second, got {timestamp}"
            );
            (timestamp as i64, value)
        })
        .collect()
}

/// The hour-aligned instant every window below is measured from: a whole forecast period
/// in the recent past, so the whole span these rows reach over lies inside the span the
/// service has forecast data for and no row's answer depends on the serving clock.
fn base_secs() -> i64 {
    current_hour_start() - 12 * HOUR
}

/// How many forecast periods the reference window spans: wide enough to contain every
/// instant a row below asks for, and inside the documented maximum window width.
const REFERENCE_PERIODS: i64 = 48;

/// Assert one window, its endpoints given as offsets from [`base_secs`] in MILLISECONDS,
/// carries exactly the data points the rule puts in it — the same forecast values a wide
/// whole-second window reports at those instants.
async fn assert_window(
    emulator: &Emulator,
    label: &str,
    data_type: &str,
    offsets_millis: (i64, i64),
    reference: &[(i64, f64)],
) {
    let base = base_secs() * MILLIS_PER_SECOND;
    let window_millis = (base + offsets_millis.0, base + offsets_millis.1);
    let instants = expected_instants(window_millis);
    let expected: Vec<(i64, f64)> = reference
        .iter()
        .filter(|(at, _)| instants.contains(at))
        .copied()
        .collect();
    assert_eq!(
        expected.len(),
        instants.len(),
        "case {label:?} / {data_type}: the reference window must cover every instant of \
         {window_millis:?}, expected {instants:?}"
    );

    let window = (
        epoch_seconds(window_millis.0),
        epoch_seconds(window_millis.1),
    );
    let case = format!("{label:?} / {data_type} over [{}, {})", window.0, window.1);
    let got = points(emulator, data_type, window, &case).await;
    assert_eq!(
        got, expected,
        "case {case}: expected the forecast of {instants:?}, got {got:?}"
    );
}

#[tokio::test]
async fn a_window_carries_every_forecast_interval_inside_its_half_open_span() {
    let period = HOUR * MILLIS_PER_SECOND;
    // Rows differ along what the rule generalises over: where inside its second each
    // endpoint sits (on the second, a millisecond in, a quarter, half and all but a
    // millisecond of a second in), where inside its forecast period it sits, how many
    // forecast periods the window spans (none, one, a few, more than a day), and which
    // endpoint carries the sub-second part. A row whose span contains no interval
    // boundary at all carries no data point.
    //
    // `(label, start offset from the base instant in millis, end offset in millis)`.
    let cases: [(&str, i64, i64); 13] = [
        ("a whole-second window of one forecast period", 0, period),
        ("an end a millisecond past a period boundary", 0, period + 1),
        (
            "an end a quarter second past a period boundary",
            0,
            period + 250,
        ),
        ("an end half a second past a period boundary", 0, period + 500),
        (
            "an end all but a millisecond past a period boundary",
            0,
            period + 999,
        ),
        (
            "an end a millisecond short of a period boundary",
            0,
            period - 1,
        ),
        ("a start a millisecond past a period boundary", 1, 2 * period),
        (
            "a start half a second past a boundary, end half a second past a later one",
            500,
            3 * period + 500,
        ),
        (
            "both endpoints all but a millisecond past a boundary",
            999,
            25 * period + 999,
        ),
        (
            "an end half a second past a boundary more than a day on",
            0,
            25 * period + 500,
        ),
        ("a whole-second window spanning a day", 0, 24 * period),
        ("a sub-second window containing no boundary", 250, 750),
        (
            "a window inside a period, ending just short of its far boundary",
            period - 500,
            period - 100,
        ),
    ];

    let emulator = with_predictive_plan().await;
    for data_type in FORECAST_DATA_TYPE_VALUES {
        let base = base_secs();
        let reference = points(
            &emulator,
            data_type,
            (base, base + REFERENCE_PERIODS * HOUR),
            "the reference window",
        )
        .await;
        assert_eq!(
            reference.len() as i64,
            REFERENCE_PERIODS,
            "case the reference window / {data_type}: expected one data point per \
             forecast period, got {reference:?}"
        );
        for (label, start, end) in cases {
            assert_window(&emulator, label, data_type, (start, end), &reference).await;
        }
    }
}

#[tokio::test]
async fn the_sub_second_part_of_an_endpoint_moves_the_edge_within_its_own_second() {
    // The same rule swept over every combination of a sub-second part on each endpoint and
    // a width in whole forecast periods: the sub-second part of a start excludes the
    // boundary in its own second, the sub-second part of an end includes the boundary
    // before it, and nothing else about the answer changes.
    let subseconds = [0_i64, 1, 250, 500, 750, 999];
    let widths = [1_i64, 2, 5, 26];

    let emulator = with_predictive_plan().await;
    let data_type = "CapacityForecast";
    let base = base_secs();
    let reference = points(
        &emulator,
        data_type,
        (base, base + REFERENCE_PERIODS * HOUR),
        "the reference window",
    )
    .await;

    for start_subsecond in subseconds {
        for end_subsecond in subseconds {
            for width in widths {
                let label = format!(
                    "a {width}-period window starting {start_subsecond}ms and ending \
                     {end_subsecond}ms into a second"
                );
                let start = start_subsecond;
                let end = width * HOUR * MILLIS_PER_SECOND + end_subsecond;
                assert_window(&emulator, &label, data_type, (start, end), &reference).await;
            }
        }
    }
}
