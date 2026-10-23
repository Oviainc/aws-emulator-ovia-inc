// SPDX-License-Identifier: CC0-1.0

//! `GetScalingPlanResourceForecastData` over the real wire path.
//!
//! Three rules are pinned here, each over a TABLE of requests rather than the one that
//! exposed them:
//!
//! * the forecast WINDOW is checked against its documented limits before any plan is
//!   looked up, so the same window earns the same verdict whether or not the named plan
//!   exists — `EndTime` has to be later than `StartTime`, the span may be at most
//!   `MAX_FORECAST_WINDOW_DAYS` days wide and `StartTime` at most
//!   `MAX_FORECAST_HISTORY_DAYS` days in the past;
//! * forecast data is issued for ONE kind of scalable resource — an Auto Scaling group's
//!   desired capacity — so any other `(ServiceNamespace, ScalableDimension)` pair, and a
//!   resource id that does not identify a resource of that kind, is refused with the
//!   modelled `ValidationException` (HTTP 400);
//! * an accepted request answers HTTP 200 with the required `Datapoints` member, which
//!   carries no data point: this emulator ingests no CloudWatch metric data, so no load
//!   metric has a history for a predicted value to be calculated from — for a plan it
//!   holds and for one it does not alike.
//!
//! The harness (router, dispatch, error-body readers) is reused from
//! [`crate::scaling_plan_lifecycle`]; nothing here builds a second one.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::get_scaling_plan_resource_forecast_data::{
    FORECAST_SCALABLE_DIMENSION, FORECAST_SERVICE_NAMESPACE, MAX_FORECAST_HISTORY_DAYS,
    MAX_FORECAST_WINDOW_DAYS,
};
use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::types::FORECAST_DATA_TYPE_VALUES;

use crate::scaling_plan_lifecycle::{
    error_code, instruction, simple_instructions, stack_source, Emulator,
};

/// The operation under test.
const OPERATION: &str = "GetScalingPlanResourceForecastData";

/// Seconds in a day, for building windows out of the two documented day counts.
const DAY: i64 = 86_400;

/// The Auto Scaling group the stored plan of these tests scales. Its id carries the
/// documented prefix: `ResourceId` "consists of a prefix (autoScalingGroup) followed by
/// the name of a specified Auto Scaling group (my-asg)".
const COVERED_RESOURCE: &str = "autoScalingGroup/my-asg";

/// An Auto Scaling group no stored plan of these tests covers.
const UNCOVERED_RESOURCE: &str = "autoScalingGroup/other-asg";

/// The plan the emulator holds in these tests.
const STORED_PLAN: &str = "forecast plan";

/// A plan name this service does not hold.
const ABSENT_PLAN: &str = "no such plan";

/// The only scaling-plan version this service holds.
const ONLY_VERSION: i64 = 1;

/// Now, as the epoch seconds an `awsJson1_1` timestamp is carried as.
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a host clock at or after the epoch")
        .as_secs() as i64
}

/// The coordinates of one scalable resource: `(ServiceNamespace, ResourceId,
/// ScalableDimension)`.
type Resource<'a> = (&'a str, &'a str, &'a str);

/// The Auto Scaling group resource the stored plan of these tests covers.
const ASG_RESOURCE: Resource<'static> = (
    FORECAST_SERVICE_NAMESPACE,
    COVERED_RESOURCE,
    FORECAST_SCALABLE_DIMENSION,
);

/// One forecast request, with every member the model marks required: a plan name and
/// version, a scalable resource, a forecast data type and the window `[start, end)`.
fn forecast_request(
    plan: &str,
    version: i64,
    resource: Resource<'_>,
    data_type: &str,
    window: (i64, i64),
) -> Value {
    let (namespace, resource_id, dimension) = resource;
    let (start, end) = window;
    json!({
        "ScalingPlanName": plan,
        "ScalingPlanVersion": version,
        "ServiceNamespace": namespace,
        "ResourceId": resource_id,
        "ScalableDimension": dimension,
        "ForecastDataType": data_type,
        "StartTime": start,
        "EndTime": end,
    })
}

/// A forecast request for the Auto Scaling group resource, over `[start, end)`.
fn asg_request(plan: &str, version: i64, data_type: &str, window: (i64, i64)) -> Value {
    forecast_request(plan, version, ASG_RESOURCE, data_type, window)
}

/// An emulator holding one `Active` plan that covers [`COVERED_RESOURCE`].
async fn with_stored_plan() -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": STORED_PLAN,
                "ApplicationSource": stack_source(1),
                "ScalingInstructions": simple_instructions(),
            }),
        )
        .await;
    emulator.make_all_due();
    emulator
}

/// The `Datapoints` member of a successful response, which the model marks required.
///
/// Asserted in two parts, because they are two different claims: the member is PRESENT
/// and carries the modelled list shape (an array), and separately — at each call site —
/// how many data points that list holds.
fn datapoints(body: &Value) -> &Vec<Value> {
    assert!(
        !body["Datapoints"].is_null(),
        "the required Datapoints member must be present, got {body}"
    );
    body.get("Datapoints")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected Datapoints to be a list, got {body}"))
}

#[tokio::test]
async fn the_forecast_window_is_checked_against_its_documented_limits() {
    // Rows differ along what the rule generalises over: the WIDTH of the window, its
    // POSITION relative to now (past, present, future) and the ORDER of its two
    // endpoints. The two day counts are the documented bounds, each tested at the bound
    // and one step past it.
    //
    // The 56-day history bound is measured against the service's own clock, which moves
    // on between building the request and handling it, so the row at that bound sits one
    // minute inside it and the row past it one minute outside — the step is a minute, not
    // a second, for that reason alone.
    let cases: [(&str, i64, i64, bool); 9] = [
        ("an hour-long window starting now", 0, 3_600, true),
        ("a window two days ahead", 2 * DAY, 2 * DAY + 3_600, true),
        (
            "a window exactly at the documented maximum width",
            0,
            MAX_FORECAST_WINDOW_DAYS * DAY,
            true,
        ),
        (
            "a window one second past the documented maximum width",
            0,
            MAX_FORECAST_WINDOW_DAYS * DAY + 1,
            false,
        ),
        (
            "a window two days past the documented maximum width",
            -DAY,
            (MAX_FORECAST_WINDOW_DAYS + 1) * DAY,
            false,
        ),
        (
            "a start just inside the documented history limit",
            -MAX_FORECAST_HISTORY_DAYS * DAY + 60,
            -MAX_FORECAST_HISTORY_DAYS * DAY + 60 + DAY,
            true,
        ),
        (
            "a start just outside the documented history limit",
            -MAX_FORECAST_HISTORY_DAYS * DAY - 60,
            -MAX_FORECAST_HISTORY_DAYS * DAY - 60 + DAY,
            false,
        ),
        ("an end time equal to the start time", 3_600, 3_600, false),
        ("an end time before the start time", 3_600, 1_800, false),
    ];

    let emulator = with_stored_plan().await;
    // Every row is checked twice: against the plan the emulator holds and against a name
    // it does not hold. The window verdict is the same either way, which is what "the
    // window is checked before the plan lookup" means.
    for (plan, held) in [(STORED_PLAN, "a stored plan"), (ABSENT_PLAN, "an absent plan")] {
        for (label, start_offset, end_offset, accepted) in cases {
            let base = now_secs();
            let body = asg_request(
                plan,
                ONLY_VERSION,
                "CapacityForecast",
                (base + start_offset, base + end_offset),
            );
            let (status, parsed) = emulator.call(OPERATION, &body).await;
            if accepted {
                assert_eq!(
                    status, 200,
                    "case {label:?} on {held}: expected HTTP 200, got {status} with {parsed}"
                );
                assert!(
                    datapoints(&parsed).is_empty(),
                    "case {label:?} on {held}: expected no data points, got {parsed}"
                );
            } else {
                assert_eq!(
                    (status, error_code(&parsed)),
                    (400, "ValidationException"),
                    "case {label:?} on {held}: expected HTTP 400 ValidationException, got \
                     {status} with {parsed}"
                );
            }
        }
    }
}

#[tokio::test]
async fn only_an_auto_scaling_group_desired_capacity_is_forecast() {
    // Every other `ScalableDimension` the model lists, each with a resource id of the
    // kind that dimension scales (source: `smithy_query(query="shape",
    // shape="ScalableDimension")`, mirrored as `SCALABLE_DIMENSION_VALUES`, and the
    // worked resource-id examples in `ScalingInstruction$ResourceId`'s documentation) —
    // none of which this operation forecasts for. The last three rows probe the other
    // halves of the rule: the right dimension under another namespace, an off-enum
    // dimension, and an Auto Scaling group id without the documented prefix.
    let cases: [(&str, &str, &str, &str); 10] = [
        (
            "an ECS service's desired count",
            "ecs",
            "service/default/sample-webapp",
            "ecs:service:DesiredCount",
        ),
        (
            "a Spot Fleet request's target capacity",
            "ec2",
            "spot-fleet-request/sfr-73fbd2ce-aa30-494c-8788-1cee4ea7606d",
            "ec2:spot-fleet-request:TargetCapacity",
        ),
        (
            "an Aurora replica count",
            "rds",
            "cluster:my-db-cluster",
            "rds:cluster:ReadReplicaCount",
        ),
        (
            "a DynamoDB table's read capacity",
            "dynamodb",
            "table/my-table",
            "dynamodb:table:ReadCapacityUnits",
        ),
        (
            "a DynamoDB table's write capacity",
            "dynamodb",
            "table/my-table",
            "dynamodb:table:WriteCapacityUnits",
        ),
        (
            "a DynamoDB index's read capacity",
            "dynamodb",
            "table/my-table/index/my-table-index",
            "dynamodb:index:ReadCapacityUnits",
        ),
        (
            "a DynamoDB index's write capacity",
            "dynamodb",
            "table/my-table/index/my-table-index",
            "dynamodb:index:WriteCapacityUnits",
        ),
        (
            "the Auto Scaling dimension under another service namespace",
            "dynamodb",
            COVERED_RESOURCE,
            FORECAST_SCALABLE_DIMENSION,
        ),
        (
            "a scalable dimension outside the modelled enum",
            FORECAST_SERVICE_NAMESPACE,
            COVERED_RESOURCE,
            "autoscaling:autoScalingGroup:Nonsense",
        ),
        (
            "an Auto Scaling group id without the documented prefix",
            FORECAST_SERVICE_NAMESPACE,
            "my-asg",
            FORECAST_SCALABLE_DIMENSION,
        ),
    ];

    let emulator = with_stored_plan().await;
    for (label, namespace, resource_id, dimension) in cases {
        let base = now_secs();
        let body = forecast_request(
            STORED_PLAN,
            ONLY_VERSION,
            (namespace, resource_id, dimension),
            "CapacityForecast",
            (base, base + 3_600),
        );
        let (status, parsed) = emulator.call(OPERATION, &body).await;
        assert_eq!(
            (status, error_code(&parsed)),
            (400, "ValidationException"),
            "case {label:?}: expected HTTP 400 ValidationException, got {status} with {parsed}"
        );
    }
}

#[tokio::test]
async fn a_forecast_data_type_outside_the_modelled_enum_is_refused() {
    let emulator = with_stored_plan().await;
    // Rows differ in how far they miss the enum: an empty value, a near-miss in case, a
    // value from another member's enum, and a plausible but unmodelled name.
    for label in [
        "",
        "capacityforecast",
        "ASGAverageCPUUtilization",
        "CapacityForecasts",
    ] {
        let base = now_secs();
        let body = asg_request(STORED_PLAN, ONLY_VERSION, label, (base, base + 3_600));
        let (status, parsed) = emulator.call(OPERATION, &body).await;
        assert_eq!(
            (status, error_code(&parsed)),
            (400, "ValidationException"),
            "case {label:?}: expected HTTP 400 ValidationException, got {status} with {parsed}"
        );
    }
}

#[tokio::test]
async fn every_modelled_forecast_data_type_reports_an_empty_forecast() {
    let emulator = with_stored_plan().await;
    // A second plan, covering a DIFFERENT Auto Scaling group, so a request for the
    // covered resource reaches a plan that does not cover it.
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": "other plan",
                "ApplicationSource": stack_source(2),
                "ScalingInstructions": [instruction(
                    FORECAST_SERVICE_NAMESPACE,
                    UNCOVERED_RESOURCE,
                    FORECAST_SCALABLE_DIMENSION,
                    1,
                    10,
                    "ASGAverageCPUUtilization",
                    40.0,
                )],
            }),
        )
        .await;
    emulator.make_all_due();

    // Rows differ along what the lookup generalises over: whether the plan is held, the
    // version asked for, and whether the plan that is held covers the named resource.
    let lookups: [(&str, &str, i64); 4] = [
        ("a plan that covers the resource", STORED_PLAN, ONLY_VERSION),
        ("a plan this service does not hold", ABSENT_PLAN, ONLY_VERSION),
        (
            "a version this service does not hold",
            STORED_PLAN,
            ONLY_VERSION + 1,
        ),
        ("a plan that covers another resource", "other plan", ONLY_VERSION),
    ];

    // Source: `smithy_query(query="shape",shape="ForecastDataType")`, mirrored in
    // `src/ops/types/shared_types.rs` as `FORECAST_DATA_TYPE_VALUES` — every value,
    // including the two scheduled-action ones the issue did not cite.
    for data_type in FORECAST_DATA_TYPE_VALUES {
        for (label, plan, version) in lookups {
            let base = now_secs();
            let parsed = emulator
                .ok(
                    OPERATION,
                    asg_request(plan, version, data_type, (base, base + DAY)),
                )
                .await;
            assert!(
                datapoints(&parsed).is_empty(),
                "case {label:?} / {data_type}: expected no data points, got {parsed}"
            );
        }
    }
}

#[tokio::test]
async fn a_forecast_read_leaves_the_stored_plan_as_it_was() {
    let emulator = with_stored_plan().await;
    let before = emulator
        .ok(
            "DescribeScalingPlans",
            json!({ "ScalingPlanNames": [STORED_PLAN] }),
        )
        .await;

    let base = now_secs();
    for data_type in FORECAST_DATA_TYPE_VALUES {
        emulator
            .ok(
                OPERATION,
                asg_request(STORED_PLAN, ONLY_VERSION, data_type, (base, base + DAY)),
            )
            .await;
    }

    let after = emulator
        .ok(
            "DescribeScalingPlans",
            json!({ "ScalingPlanNames": [STORED_PLAN] }),
        )
        .await;
    assert_eq!(
        after, before,
        "reading the forecast must not change what a later caller sees: {before} became \
         {after}"
    );
}
