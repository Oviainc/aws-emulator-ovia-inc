// SPDX-License-Identifier: CC0-1.0

//! One definition of which scalable resource a scaling instruction applies to, asked by
//! the two paths that each used to decide it for themselves.
//!
//! "Each scaling instruction applies to one resource"
//! (`smithy_query(query="doc",shape="ScalingInstruction")`), and that resource is named by
//! the `(ServiceNamespace, ResourceId, ScalableDimension)` triple the instruction carries —
//! compared WHOLE and part for part, no part trimmed, case-folded or matched by prefix,
//! because each part is caller text the model constrains only by its own value list (the
//! two enums) or not at all (`ResourceId`). Two paths of this service turn on that one
//! identity:
//!
//! * the WRITE path (`CreateScalingPlan` / `UpdateScalingPlan`) holds at most one
//!   instruction per identity, and `DescribeScalingPlanResources` reports the resources a
//!   plan covers one per identity;
//! * the READ path (`GetScalingPlanResourceForecastData`) issues the forecast of a resource
//!   from the stored instruction whose identity is the triple the request names, and has no
//!   forecast at all for a resource no instruction of the plan names.
//!
//! So the rows below drive BOTH paths from the same triples and assert they agree about
//! which instruction names which resource. Rows differ along what the identity generalises
//! over: the VALUE, the CASE, the LENGTH, the embedded separators and the trailing
//! whitespace of a `ResourceId`, whether one resource id is a prefix of another, and which
//! PART of the triple two instructions differ in.
//!
//! Nothing here builds a second harness: the router and error-body readers (`Emulator`,
//! `error_code`, `error_message`, `stack_source`), the instruction builder (`instruction`),
//! the identity reader (`resource_identity`), the create-and-settle helper and resource
//! envelope reader (`applied_plan`, `resources`), and the forecast request and data-point
//! readers (`points`, `assert_close`, `current_hour_start`) all come from the sibling
//! suites.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::get_scaling_plan_resource_forecast_data::{
    FORECAST_INTERVAL_SECONDS, FORECAST_SCALABLE_DIMENSION, FORECAST_SERVICE_NAMESPACE,
};
use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::types::FORECAST_DATA_TYPE_VALUES;

use crate::applied_policies_and_status_narratives::{applied_plan, resources};
use crate::forecast_data::current_hour_start;
use crate::forecast_datapoints::{assert_close, points};
use crate::scaling_plan_lifecycle::{
    error_code, error_message, instruction, resource_identity, stack_source, Emulator,
    ResourceIdentity,
};

/// The plan every case stores.
const PLAN: &str = "identity plan";

/// A `ScalingMetricType` the Auto Scaling group dimension supports, and a target value
/// inside the documented range (source: `SCALING_METRIC_TYPE_VALUES` and
/// `smithy_query(query="doc",shape="PredefinedScalingMetricSpecification",member="PredefinedScalingMetricType")`).
const SCALING_METRIC: &str = "ASGAverageCPUUtilization";
const TARGET_VALUE: f64 = 40.0;

/// A `LoadMetricType` of the same dimension (source:
/// `smithy_query(query="shape",shape="LoadMetricType")`). A load metric specification is
/// what configures predictive scaling, which is what a forecast is issued for.
const LOAD_METRIC: &str = "ASGTotalCPUUtilization";

/// The `ForecastDataType` whose value is read straight off the instruction under the
/// documented default `PredictiveScalingMaxCapacityBehavior`
/// (`SetForecastCapacityToMaxCapacity`, "The maximum capacity is enforced as a hard
/// limit"): the maximum capacity of the instruction the forecast was issued from, so it
/// says WHICH instruction answered.
const DATA_TYPE_SCHEDULED_ACTION_MAX: &str = "ScheduledActionMaxCapacity";

/// The `ForecastDataType` carrying the capacity forecast itself, which scales with the
/// capacity band of the instruction it came from.
const DATA_TYPE_CAPACITY: &str = "CapacityForecast";

/// The fragment the service refuses two instructions for one scalable resource with.
const ONE_PER_RESOURCE_FRAGMENT: &str =
    "a scaling plan can specify only one scaling instruction per scalable resource";

/// One row: an Auto Scaling group the stored plan instructs, and the capacity band its
/// instruction declares.
///
/// Every row's capacity band is its own, and the rows are ordered by the MIDDLE of that
/// band, each row's being at least twice the previous row's — so the forecast of one row
/// cannot be mistaken for the forecast of any other.
struct Row {
    label: &'static str,
    resource_id: &'static str,
    min_capacity: i64,
    max_capacity: i64,
}

/// The table. Every resource id carries the prefix an
/// `autoscaling:autoScalingGroup:DesiredCapacity` resource id is documented to carry —
/// "consists of a prefix (autoScalingGroup) followed by the name of a specified Auto
/// Scaling group (my-asg)"
/// (`smithy_query(query="doc",shape="ScalingInstruction",member="ResourceId")`) — and the
/// rows differ in everything an identity comparison could wrongly fold together.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "a plain group name",
            resource_id: "autoScalingGroup/asg",
            min_capacity: 1,
            max_capacity: 19,
        },
        Row {
            label: "a name the first row's name is a prefix of",
            resource_id: "autoScalingGroup/asg-two",
            min_capacity: 2,
            max_capacity: 38,
        },
        Row {
            label: "a name differing from the first only in case",
            resource_id: "autoScalingGroup/ASG",
            min_capacity: 4,
            max_capacity: 76,
        },
        Row {
            label: "a name differing from the first only by a trailing space",
            resource_id: "autoScalingGroup/asg ",
            min_capacity: 8,
            max_capacity: 152,
        },
        Row {
            label: "a name carrying further separators",
            resource_id: "autoScalingGroup/asg/two/three",
            min_capacity: 16,
            max_capacity: 304,
        },
        Row {
            label: "a long name",
            resource_id: "autoScalingGroup/asg-with-a-long-and-detailed-group-name-0123456789",
            min_capacity: 32,
            max_capacity: 608,
        },
    ]
}

/// Resource ids NEAR a row's but not equal to it: no instruction of the stored plan names
/// any of them, so the plan has no forecast for them.
fn near_misses() -> Vec<(&'static str, &'static str)> {
    vec![
        ("a prefix of a covered name", "autoScalingGroup/as"),
        ("a covered name with a character appended", "autoScalingGroup/asg-"),
        ("a third casing of a covered name", "autoScalingGroup/Asg"),
        ("a covered name with one space too many", "autoScalingGroup/asg  "),
    ]
}

/// This row's scaling instruction, as the wire carries it.
fn row_instruction(row: &Row) -> Value {
    let mut value = instruction(
        FORECAST_SERVICE_NAMESPACE,
        row.resource_id,
        FORECAST_SCALABLE_DIMENSION,
        row.min_capacity,
        row.max_capacity,
        SCALING_METRIC,
        TARGET_VALUE,
    );
    value["PredefinedLoadMetricSpecification"] = json!({ "PredefinedLoadMetricType": LOAD_METRIC });
    // The buffer time moves a scheduled scaling action's run time ahead of the interval it
    // provisions for; pinned at zero here so which intervals the window holds does not
    // depend on it. The shift itself is pinned by `crate::scheduled_action_buffer_time`.
    value["ScheduledActionBufferTime"] = json!(0);
    value
}

/// The identity the resource of this row is named by.
fn row_identity(row: &Row) -> ResourceIdentity {
    (
        FORECAST_SERVICE_NAMESPACE.to_string(),
        row.resource_id.to_string(),
        FORECAST_SCALABLE_DIMENSION.to_string(),
    )
}

/// A window of whole forecast intervals, hour-aligned and inside the span forecast data
/// exists over, so which intervals fall in it does not depend on the serving clock.
fn settled_window() -> (i64, i64) {
    let start = current_hour_start() - 2 * FORECAST_INTERVAL_SECONDS;
    (start, start + 4 * FORECAST_INTERVAL_SECONDS)
}

/// The identities `DescribeScalingPlanResources` reports for [`PLAN`].
async fn reported_identities(emulator: &Emulator) -> BTreeSet<ResourceIdentity> {
    resources(emulator, PLAN)
        .await
        .iter()
        .map(resource_identity)
        .collect()
}

/// The body of a `CreateScalingPlan` call storing `instructions` as [`PLAN`].
fn create_body(instructions: Value) -> Value {
    json!({
        "ScalingPlanName": PLAN,
        "ApplicationSource": stack_source(1),
        "ScalingInstructions": instructions,
    })
}

#[tokio::test]
async fn the_forecast_read_and_the_plan_resource_walk_name_a_resource_by_the_same_triple() {
    let rows = rows();
    let emulator = Emulator::new();
    applied_plan(&emulator, PLAN, rows.iter().map(row_instruction).collect()).await;

    // The write path's view of the same identities: one resource per instruction, every
    // row its own resource.
    let reported = reported_identities(&emulator).await;
    let expected: BTreeSet<ResourceIdentity> = rows.iter().map(row_identity).collect();
    assert_eq!(
        reported, expected,
        "case every row is its own scalable resource: expected {expected:?}, got {reported:?}"
    );

    // The read path's view: each row's forecast is issued from that row's instruction.
    let window = settled_window();
    let mut capacity_curves: Vec<(&'static str, Vec<(i64, f64)>)> = Vec::new();
    for row in &rows {
        let case = row.label;
        let scheduled_max = points(
            &emulator,
            PLAN,
            row.resource_id,
            DATA_TYPE_SCHEDULED_ACTION_MAX,
            window,
        )
        .await;
        assert!(
            !scheduled_max.is_empty(),
            "case {case:?}: the plan instructs this resource, so it has a forecast over \
             {window:?}, got {scheduled_max:?}"
        );
        for (at, value) in &scheduled_max {
            assert_close(
                *value,
                row.max_capacity as f64,
                &format!("{case:?} at {at} / {DATA_TYPE_SCHEDULED_ACTION_MAX}"),
            );
        }

        let capacity = points(&emulator, PLAN, row.resource_id, DATA_TYPE_CAPACITY, window).await;
        assert!(
            !capacity.is_empty(),
            "case {case:?}: the plan instructs this resource, so it has a capacity forecast \
             over {window:?}, got {capacity:?}"
        );
        capacity_curves.push((case, capacity));
    }

    // The rows are ordered by the middle of their capacity band, so each row's capacity
    // forecast sits strictly above the previous row's at the same instant: a read that
    // resolved the identity loosely would answer one of its neighbours' curves instead.
    for pair in capacity_curves.windows(2) {
        let (lower_case, lower) = &pair[0];
        let (higher_case, higher) = &pair[1];
        assert_eq!(
            lower.len(),
            higher.len(),
            "case {lower_case:?} against {higher_case:?}: one window carries one count of \
             data points, got {} and {}",
            lower.len(),
            higher.len()
        );
        for (index, (at, lower_value)) in lower.iter().enumerate() {
            let (higher_at, higher_value) = higher[index];
            assert_eq!(
                higher_at, *at,
                "case {lower_case:?} against {higher_case:?}: the two forecasts cover the \
                 same intervals, got {higher_at} against {at}"
            );
            assert!(
                *lower_value < higher_value,
                "case {lower_case:?} against {higher_case:?} at {at}: a wider band forecasts \
                 more capacity, got {lower_value} against {higher_value}"
            );
        }
    }

    // A resource id near a covered one but not equal to it is a resource the plan does not
    // instruct, so it has no forecast of any type.
    for (label, resource_id) in near_misses() {
        // Source: `smithy_query(query="shape",shape="ForecastDataType")`, mirrored as
        // `FORECAST_DATA_TYPE_VALUES` — every modelled forecast data type.
        for data_type in FORECAST_DATA_TYPE_VALUES {
            let case = format!("{label} / {data_type}");
            let got = points(&emulator, PLAN, resource_id, data_type, window).await;
            assert!(
                got.is_empty(),
                "case {case:?}: no instruction of the plan names {resource_id:?}, so there is \
                 no forecast for it, got {got:?}"
            );
        }
    }
}

/// One pair of instructions whose triples differ in exactly the part named.
struct Pair {
    label: &'static str,
    left: Value,
    right: Value,
}

/// The pairs. Every `ScalableDimension` value is prefixed with the service namespace that
/// owns it and names one resource type
/// (`smithy_query(query="shape",shape="ScalableDimension")`), so the namespace part cannot
/// vary on its own: the row that varies it varies the dimension that names it, and the
/// resource id that dimension's resource type requires, with it.
fn pairs() -> Vec<Pair> {
    vec![
        Pair {
            label: "the ResourceId",
            left: instruction(
                FORECAST_SERVICE_NAMESPACE,
                "autoScalingGroup/asg",
                FORECAST_SCALABLE_DIMENSION,
                1,
                10,
                SCALING_METRIC,
                TARGET_VALUE,
            ),
            right: instruction(
                FORECAST_SERVICE_NAMESPACE,
                "autoScalingGroup/asg-two",
                FORECAST_SCALABLE_DIMENSION,
                1,
                10,
                SCALING_METRIC,
                TARGET_VALUE,
            ),
        },
        Pair {
            label: "the ScalableDimension, on one resource id",
            left: instruction(
                "dynamodb",
                "table/my-table",
                "dynamodb:table:ReadCapacityUnits",
                1,
                10,
                "DynamoDBReadCapacityUtilization",
                TARGET_VALUE,
            ),
            right: instruction(
                "dynamodb",
                "table/my-table",
                "dynamodb:table:WriteCapacityUnits",
                1,
                10,
                "DynamoDBWriteCapacityUtilization",
                TARGET_VALUE,
            ),
        },
        Pair {
            label: "the ServiceNamespace, with the dimension and resource type that follow it",
            left: instruction(
                FORECAST_SERVICE_NAMESPACE,
                "autoScalingGroup/asg",
                FORECAST_SCALABLE_DIMENSION,
                1,
                10,
                SCALING_METRIC,
                TARGET_VALUE,
            ),
            right: instruction(
                "ecs",
                "service/default/sample-webapp",
                "ecs:service:DesiredCount",
                1,
                10,
                "ECSServiceAverageCPUUtilization",
                TARGET_VALUE,
            ),
        },
    ]
}

#[tokio::test]
async fn two_instructions_are_one_resource_only_when_the_whole_triple_is_the_same() {
    for pair in pairs() {
        let case = pair.label;

        // Twice the same triple is twice the same resource, which the write path refuses.
        let emulator = Emulator::new();
        let (status, body) = emulator
            .call(
                "CreateScalingPlan",
                &create_body(json!([pair.left, pair.left])),
            )
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "ValidationException"),
            "case {case:?}: two instructions for one scalable resource are refused, got \
             {status} with {body}"
        );
        let message = error_message(&body);
        assert!(
            message.contains(ONE_PER_RESOURCE_FRAGMENT),
            "case {case:?}: expected a message containing {ONE_PER_RESOURCE_FRAGMENT:?}, got \
             {message:?}"
        );

        // Differing in that one part makes them two resources, both of which the plan
        // then covers.
        let emulator = Emulator::new();
        applied_plan(&emulator, PLAN, vec![pair.left.clone(), pair.right.clone()]).await;
        let reported = reported_identities(&emulator).await;
        let expected: BTreeSet<ResourceIdentity> =
            [resource_identity(&pair.left), resource_identity(&pair.right)]
                .into_iter()
                .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: two triples differing in this part are two resources, expected \
             {expected:?}, got {reported:?}"
        );
    }
}
