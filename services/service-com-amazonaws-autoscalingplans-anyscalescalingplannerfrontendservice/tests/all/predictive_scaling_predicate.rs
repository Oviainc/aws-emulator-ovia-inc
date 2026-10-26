// SPDX-License-Identifier: CC0-1.0

//! One question, asked by two paths, answered the same way.
//!
//! "Configuring predictive scaling" is a load metric specification of either kind being
//! present on a `ScalingInstruction`
//! (`ScalingInstruction$PredefinedLoadMetricSpecification`: "This parameter or a
//! CustomizedLoadMetricSpecification is required when configuring predictive scaling, and
//! cannot be used otherwise"), and two paths of this service turn on that one answer:
//!
//! * the WRITE path (`CreateScalingPlan` / `UpdateScalingPlan`) accepts a member
//!   documented "Only valid when configuring predictive scaling" exactly when the
//!   instruction configures it, and refuses it otherwise with the modelled
//!   `ValidationException` (HTTP 400);
//! * the READ path (`GetScalingPlanResourceForecastData`) has a forecast to report exactly
//!   when the stored instruction configures it, and an empty `Datapoints` list otherwise.
//!
//! So the table below drives BOTH paths from the same row and asserts they agree: a row
//! the write path accepts the conditional members of is a row the forecast path reports
//! data points for, and a row it refuses them on is a row with no forecast at all. Rows
//! differ along what the rule generalises over: which load metric specification carries
//! the instruction into predictive scaling, and which form and value it takes.
//!
//! Nothing here builds a second harness or a second builder: the router and error-body
//! readers (`Emulator`, `error_code`, `error_message`, `stack_source`), the instruction
//! builder (`with`), the forecast request and its `Datapoints` reader (`asg_request`,
//! `datapoints`, `current_hour_start`, `ONLY_VERSION`) and the refusal wording
//! (`predictive_only_fragment`) all come from the sibling suites.

use serde_json::{json, Value};

use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::get_scaling_plan_resource_forecast_data::FORECAST_INTERVAL_SECONDS;
use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::types::{
    FORECAST_DATA_TYPE_VALUES, LOAD_METRIC_TYPE_VALUES,
};

use crate::forecast_data::{asg_request, current_hour_start, datapoints, ONLY_VERSION};
use crate::scaling_instruction_validation::with;
use crate::scaling_mode_conditional_members::predictive_only_fragment;
use crate::scaling_plan_lifecycle::{error_code, error_message, stack_source, Emulator};

/// The read path under test.
const FORECAST_OPERATION: &str = "GetScalingPlanResourceForecastData";

/// The plan every row stores. The instruction builder scales an Auto Scaling group's
/// desired capacity, which is the one resource kind forecast data is issued for.
const PLAN: &str = "predictive plan";

/// One row: the load metric members it puts on the instruction, and whether those members
/// configure predictive scaling.
struct Row {
    label: String,
    load_members: Vec<(&'static str, Value)>,
    configures_predictive_scaling: bool,
}

/// The table. Every `LoadMetricType` the model carries is driven through the predefined
/// form (source: `smithy_query(query="shape",shape="LoadMetricType")`, mirrored as
/// `LOAD_METRIC_TYPE_VALUES`), the customized form is driven twice, and the row carrying
/// no load metric specification at all is the control.
fn rows() -> Vec<Row> {
    let mut rows = vec![Row {
        label: "no load metric specification at all".to_string(),
        load_members: Vec::new(),
        configures_predictive_scaling: false,
    }];

    for metric_type in LOAD_METRIC_TYPE_VALUES {
        rows.push(Row {
            label: format!("a predefined load metric of type {metric_type}"),
            load_members: vec![(
                "PredefinedLoadMetricSpecification",
                json!({ "PredefinedLoadMetricType": metric_type }),
            )],
            configures_predictive_scaling: true,
        });
    }

    // The customized form, whose only documented statistic is `Sum`: once bare, once with
    // dimensions, so the row differs in the shape of the specification and not only in
    // which member holds it.
    rows.push(Row {
        label: "a customized load metric".to_string(),
        load_members: vec![(
            "CustomizedLoadMetricSpecification",
            json!({ "MetricName": "my-load", "Namespace": "MyApp", "Statistic": "Sum" }),
        )],
        configures_predictive_scaling: true,
    });
    rows.push(Row {
        label: "a customized load metric with dimensions".to_string(),
        load_members: vec![(
            "CustomizedLoadMetricSpecification",
            json!({
                "MetricName": "my-load",
                "Namespace": "MyApp",
                "Statistic": "Sum",
                "Dimensions": [{ "Name": "AutoScalingGroupName", "Value": "my-asg" }],
            }),
        )],
        configures_predictive_scaling: true,
    });

    rows
}

/// Members documented "Only valid when configuring predictive scaling", each of which the
/// write path decides by the same question the forecast path asks. Each value is inside
/// the bound documented for its member, so only the predictive-scaling rule can refuse it.
fn predictive_scaling_only_members() -> Vec<(&'static str, Value)> {
    vec![
        // "The default is 300 seconds", well below the forecast interval duration.
        ("ScheduledActionBufferTime", json!(300)),
        // A modelled `PredictiveScalingMaxCapacityBehavior` value.
        (
            "PredictiveScalingMaxCapacityBehavior",
            json!("SetMaxCapacityToForecastCapacity"),
        ),
    ]
}

/// This row's instruction, with `extra` members appended to its load metric members.
fn instruction_of(row: &Row, extra: &[(&'static str, Value)]) -> Value {
    let mut members = row.load_members.clone();
    members.extend(extra.iter().cloned());
    with(&members)
}

/// The body of a `CreateScalingPlan` call storing `instructions` as [`PLAN`].
fn create_body(instructions: &Value) -> Value {
    json!({
        "ScalingPlanName": PLAN,
        "ApplicationSource": stack_source(1),
        "ScalingInstructions": instructions,
    })
}

/// The body of an `UpdateScalingPlan` call replacing [`PLAN`]'s instructions.
fn update_body(instructions: &Value) -> Value {
    json!({
        "ScalingPlanName": PLAN,
        "ScalingPlanVersion": ONLY_VERSION,
        "ScalingInstructions": instructions,
    })
}

/// A window of whole forecast intervals, hour-aligned and inside the span forecast data
/// exists over, so which intervals fall in it does not depend on the serving clock.
fn settled_window() -> (i64, i64) {
    let start = current_hour_start() - 2 * FORECAST_INTERVAL_SECONDS;
    (start, start + 4 * FORECAST_INTERVAL_SECONDS)
}

/// How many data points the stored plan reports for `data_type` over [`settled_window`].
async fn datapoint_count(emulator: &Emulator, data_type: &str) -> usize {
    let body = emulator
        .ok(
            FORECAST_OPERATION,
            asg_request(PLAN, ONLY_VERSION, data_type, settled_window()),
        )
        .await;
    datapoints(&body).len()
}

/// An emulator holding one settled plan whose instruction list is `instructions`.
async fn emulator_holding(instructions: &Value) -> Emulator {
    let emulator = Emulator::new();
    emulator.ok("CreateScalingPlan", create_body(instructions)).await;
    emulator.make_all_due();
    emulator
}

#[tokio::test]
async fn the_write_path_and_the_forecast_path_agree_on_configuring_predictive_scaling() {
    for row in rows() {
        let label = &row.label;
        let configures = row.configures_predictive_scaling;

        // The WRITE path: a member valid only when predictive scaling is configured is
        // accepted exactly where this row configures it, through both write operations.
        for (member, value) in predictive_scaling_only_members() {
            let instructions = json!([instruction_of(&row, &[(member, value)])]);
            let case = format!("{label} with {member}");
            let emulator = Emulator::new();
            let (status, body) = emulator.call("CreateScalingPlan", &create_body(&instructions)).await;
            if configures {
                assert_eq!(
                    (status, body.get("ScalingPlanVersion")),
                    (200, Some(&json!(ONLY_VERSION))),
                    "case {case:?}: expected the create to be accepted, got {status} with {body}"
                );
                emulator.make_all_due();
                let (status, body) = emulator
                    .call("UpdateScalingPlan", &update_body(&instructions))
                    .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the update to be accepted, got {status} with {body}"
                );
            } else {
                let expected =
                    predictive_only_fragment(&format!("ScalingInstructions[0].{member}"));
                assert_eq!(
                    (status, error_code(&body)),
                    (400, "ValidationException"),
                    "case {case:?}: expected HTTP 400 ValidationException, got {status} \
                     with {body}"
                );
                let actual = error_message(&body);
                assert!(
                    actual.contains(&expected),
                    "case {case:?}: expected a message containing {expected:?}, got \
                     {actual:?}"
                );

                // The same list is refused by the other write path, on a plan that holds
                // instructions the create accepted.
                let emulator = emulator_holding(&json!([with(&[])])).await;
                let (status, body) = emulator
                    .call("UpdateScalingPlan", &update_body(&instructions))
                    .await;
                assert_eq!(
                    (status, error_code(&body)),
                    (400, "ValidationException"),
                    "case {case:?}: expected the update to be refused too, got {status} \
                     with {body}"
                );
                let actual = error_message(&body);
                assert!(
                    actual.contains(&expected),
                    "case {case:?}: expected a message containing {expected:?}, got \
                     {actual:?}"
                );
            }
        }

        // The READ path, on the same row without any conditional member: a forecast exists
        // exactly where the write path accepts those members.
        let emulator = emulator_holding(&json!([instruction_of(&row, &[])])).await;
        // Source: `smithy_query(query="shape",shape="ForecastDataType")`, mirrored as
        // `FORECAST_DATA_TYPE_VALUES` — every modelled forecast data type.
        for data_type in FORECAST_DATA_TYPE_VALUES {
            let case = format!("{label} / {data_type}");
            let points = datapoint_count(&emulator, data_type).await;
            if configures {
                assert!(
                    points > 0,
                    "case {case:?}: an instruction configuring predictive scaling has a \
                     forecast, got {points} data points"
                );
            } else {
                assert_eq!(
                    points, 0,
                    "case {case:?}: an instruction configuring no predictive scaling has \
                     no forecast, got {points} data points"
                );
            }
        }
    }
}
