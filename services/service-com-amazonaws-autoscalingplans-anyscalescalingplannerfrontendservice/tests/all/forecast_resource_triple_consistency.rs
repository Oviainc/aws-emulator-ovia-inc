// SPDX-License-Identifier: CC0-1.0

//! What a forecast read's resource coordinates are refused for, and what they are served
//! for, over the real wire path.
//!
//! ONE rule is pinned here, over a TABLE spanning every scalable resource kind the model
//! lists rather than the one kind this operation's member prose works through: a
//! `GetScalingPlanResourceForecastData` request is refused on its coordinates exactly when
//! the `(ServiceNamespace, ResourceId, ScalableDimension)` triple describes no one scalable
//! resource — the dimension is owned by another service namespace, or the resource id
//! identifies no resource of the kind that dimension scales — and every triple that DOES
//! describe one scalable resource is answered by the normal lookup: HTTP 200 carrying the
//! required `Datapoints` list, empty where no stored scaling instruction names that
//! resource, and computed from the instruction where one does.
//!
//! That is the same question the two write paths ask of a scaling instruction's own triple
//! (`crate::resource_id_identifies_a_resource` and `crate::scaling_instruction_validation`
//! pin the write side), and this operation binds no not-found error at all
//! (`smithy_query(query="operation",operation="GetScalingPlanResourceForecastData")` binds
//! only `InternalServiceException` and `ValidationException`), so a resource this service
//! holds no forecast for is the empty list and not a refusal.
//!
//! Rows differ along what the rule generalises over: the service NAMESPACE, the scalable
//! DIMENSION, the resource TYPE and the FORM of its identifier (one segment, two segments,
//! a colon separator, an index addressed through its table), and — for the refusals — which
//! half of the triple contradicts the other and whether the named plan exists.
//!
//! The harness (router, dispatch, error-body readers) is reused from
//! [`crate::scaling_plan_lifecycle`]; nothing here builds a second one.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::get_scaling_plan_resource_forecast_data::FORECAST_INTERVAL_SECONDS;
use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::types::{
    FORECAST_DATA_TYPE_VALUES, SCALABLE_DIMENSION_VALUES, SERVICE_NAMESPACE_VALUES,
};
use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::state::UNSUPPORTED_RESOURCE;

use crate::scaling_plan_lifecycle::{
    error_code, error_message, instruction, now_secs, stack_source, Emulator,
};

/// The operation under test.
const OPERATION: &str = "GetScalingPlanResourceForecastData";

/// One forecast period in seconds: the granularity forecast data exists at, so the window
/// below is measured in whole periods.
const HOUR: i64 = FORECAST_INTERVAL_SECONDS;

/// The only scaling-plan version the model documents as valid.
const ONLY_VERSION: i64 = 1;

/// A plan name this service never holds, so a row read against it reaches the lookup with
/// nothing to find.
const ABSENT_PLAN: &str = "no such plan";

/// The plan the "nothing instructs this resource" rows read, which instructs one Auto
/// Scaling group no row below names.
const UNRELATED_PLAN: &str = "unrelated plan";

/// The Auto Scaling group that plan instructs.
const UNRELATED_RESOURCE: &str = "autoScalingGroup/other-asg";

/// The capacity band every instruction this suite writes declares, so the forecast of one
/// row is comparable with the forecast of every other row.
const MIN_CAPACITY: i64 = 2;
const MAX_CAPACITY: i64 = 10;

/// The target value every instruction this suite writes tracks its metric at.
const TARGET_VALUE: f64 = 40.0;

/// The load metric specification that configures predictive scaling, which is what gives
/// an instruction a forecast at all
/// (`smithy_query(query="doc",shape="ScalingInstruction",member="PredefinedLoadMetricSpecification")`:
/// "This parameter or a CustomizedLoadMetricSpecification is required when configuring
/// predictive scaling"). The member's own enum does not depend on the scalable dimension
/// (source: `smithy_query(query="shape",shape="LoadMetricType")`, mirrored as
/// `LOAD_METRIC_TYPE_VALUES`), so every row below configures predictive scaling with the
/// same one.
const LOAD_METRIC_TYPE: &str = "ASGTotalCPUUtilization";

/// How many whole forecast periods the window of the served rows spans.
const WINDOW_PERIODS: i64 = 3;

/// One scalable resource: the coordinates that describe it, plus a predefined scaling
/// metric that measures a resource of that kind.
struct Row {
    label: &'static str,
    namespace: &'static str,
    resource_id: &'static str,
    dimension: &'static str,
    /// Source: `smithy_query(query="doc",shape="PredefinedScalingMetricSpecification",member="PredefinedScalingMetricType")`,
    /// each metric named for the kind of resource it measures.
    metric: &'static str,
}

/// Every scalable resource kind the model lists, each with the worked resource-id example
/// from `ScalingInstruction$ResourceId`'s own documentation
/// (`smithy_query(query="doc",shape="ScalingInstruction",member="ResourceId")`), covering
/// every value of `ScalableDimension`
/// (`smithy_query(query="shape",shape="ScalableDimension")`).
const ROWS: [Row; 8] = [
    Row {
        label: "an Auto Scaling group's desired capacity",
        namespace: "autoscaling",
        resource_id: "autoScalingGroup/my-asg",
        dimension: "autoscaling:autoScalingGroup:DesiredCapacity",
        metric: "ASGAverageCPUUtilization",
    },
    Row {
        label: "an ECS service's desired count",
        namespace: "ecs",
        resource_id: "service/default/sample-webapp",
        dimension: "ecs:service:DesiredCount",
        metric: "ECSServiceAverageCPUUtilization",
    },
    Row {
        label: "a Spot Fleet request's target capacity",
        namespace: "ec2",
        resource_id: "spot-fleet-request/sfr-73fbd2ce-aa30-494c-8788-1cee4EXAMPLE",
        dimension: "ec2:spot-fleet-request:TargetCapacity",
        metric: "EC2SpotFleetRequestAverageCPUUtilization",
    },
    Row {
        label: "an Aurora DB cluster's read replica count",
        namespace: "rds",
        resource_id: "cluster:my-db-cluster",
        dimension: "rds:cluster:ReadReplicaCount",
        metric: "RDSReaderAverageCPUUtilization",
    },
    Row {
        label: "a DynamoDB table's read capacity",
        namespace: "dynamodb",
        resource_id: "table/my-table",
        dimension: "dynamodb:table:ReadCapacityUnits",
        metric: "DynamoDBReadCapacityUtilization",
    },
    Row {
        label: "a DynamoDB table's write capacity",
        namespace: "dynamodb",
        resource_id: "table/my-table",
        dimension: "dynamodb:table:WriteCapacityUnits",
        metric: "DynamoDBWriteCapacityUtilization",
    },
    Row {
        label: "a DynamoDB index's read capacity",
        namespace: "dynamodb",
        resource_id: "table/my-table/index/my-table-index",
        dimension: "dynamodb:index:ReadCapacityUnits",
        metric: "DynamoDBReadCapacityUtilization",
    },
    Row {
        label: "a DynamoDB index's write capacity",
        namespace: "dynamodb",
        resource_id: "table/my-table/index/my-table-index",
        dimension: "dynamodb:index:WriteCapacityUnits",
        metric: "DynamoDBWriteCapacityUtilization",
    },
];

/// The start of the clock hour containing now, so each row's window is a whole number of
/// forecast periods and the data points of two rows are reported at the same instants.
fn current_hour_start() -> i64 {
    now_secs().div_euclid(HOUR) * HOUR
}

/// One forecast request with every member the model marks required, over a window of
/// [`WINDOW_PERIODS`] whole forecast periods starting at the top of the current hour —
/// well inside the documented window limits, so nothing but the coordinates can refuse it.
fn forecast_request(
    plan: &str,
    namespace: &str,
    resource_id: &str,
    dimension: &str,
    data_type: &str,
) -> Value {
    let start = current_hour_start();
    json!({
        "ScalingPlanName": plan,
        "ScalingPlanVersion": ONLY_VERSION,
        "ServiceNamespace": namespace,
        "ResourceId": resource_id,
        "ScalableDimension": dimension,
        "ForecastDataType": data_type,
        "StartTime": start,
        "EndTime": start + WINDOW_PERIODS * HOUR,
    })
}

/// The `Datapoints` member of a successful answer, which the model marks required.
fn datapoints(body: &Value) -> &Vec<Value> {
    body.get("Datapoints")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected the required Datapoints list, got {body}"))
}

/// One instruction naming `row`'s resource and configuring predictive scaling, so the
/// resource has a forecast to report.
fn predictive_instruction(row: &Row) -> Value {
    let mut value = instruction(
        row.namespace,
        row.resource_id,
        row.dimension,
        MIN_CAPACITY,
        MAX_CAPACITY,
        row.metric,
        TARGET_VALUE,
    );
    value["PredefinedLoadMetricSpecification"] =
        json!({ "PredefinedLoadMetricType": LOAD_METRIC_TYPE });
    // A scheduled scaling action's run time is buffered ahead of the interval it
    // provisions for, so every row pins the buffer at the same value and the instants two
    // rows report are the same ones.
    value["ScheduledActionBufferTime"] = json!(0);
    value
}

/// An emulator holding one `Active` plan whose single instruction names
/// [`UNRELATED_RESOURCE`] — a resource no row below names.
async fn with_an_unrelated_plan() -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": UNRELATED_PLAN,
                "ApplicationSource": stack_source(1),
                "ScalingInstructions": [instruction(
                    "autoscaling",
                    UNRELATED_RESOURCE,
                    "autoscaling:autoScalingGroup:DesiredCapacity",
                    MIN_CAPACITY,
                    MAX_CAPACITY,
                    "ASGAverageCPUUtilization",
                    TARGET_VALUE,
                )],
            }),
        )
        .await;
    emulator.make_all_due();
    emulator
}

#[test]
fn the_table_covers_every_scalable_dimension_the_model_lists() {
    let covered: BTreeSet<&str> = ROWS.iter().map(|row| row.dimension).collect();
    let modelled: BTreeSet<&str> = SCALABLE_DIMENSION_VALUES.iter().copied().collect();
    assert_eq!(
        covered, modelled,
        "case the table's coverage: every modelled ScalableDimension must be driven, \
         covered {covered:?} of {modelled:?}"
    );
}

#[tokio::test]
async fn a_consistent_triple_no_instruction_names_is_answered_with_an_empty_list() {
    let emulator = with_an_unrelated_plan().await;

    // Two lookups per row, because neither is a refusal: a plan this service does not
    // hold, and a plan it does hold that instructs another resource.
    let lookups: [(&str, &str); 2] = [
        ("a plan this service does not hold", ABSENT_PLAN),
        ("a plan that instructs another resource", UNRELATED_PLAN),
    ];

    // Source: `smithy_query(query="shape",shape="ForecastDataType")`, mirrored as
    // `FORECAST_DATA_TYPE_VALUES` — every value, so no data type refuses a triple the
    // others serve.
    for row in &ROWS {
        for data_type in FORECAST_DATA_TYPE_VALUES {
            for (held, plan) in lookups {
                let request = forecast_request(
                    plan,
                    row.namespace,
                    row.resource_id,
                    row.dimension,
                    data_type,
                );
                let (status, body) = emulator.call(OPERATION, &request).await;
                let case = format!("{} / {data_type} / {held}", row.label);
                assert_eq!(
                    status, 200,
                    "case {case:?}: a triple describing one scalable resource must be \
                     served, got {status} with {body}"
                );
                assert!(
                    datapoints(&body).is_empty(),
                    "case {case:?}: nothing instructs this resource, so it has no \
                     forecast, got {body}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_consistent_triple_its_plan_instructs_is_forecast_from_that_instruction() {
    // Every row's instruction declares the same band, the same target value and the same
    // load metric specification, so the forecast is a function of the INSTRUCTION and not
    // of which kind of resource it names: every row must report the data points the Auto
    // Scaling group row reports. The values themselves are pinned, instruction by
    // instruction, in [`crate::forecast_datapoints`].
    let mut expected: Vec<(&str, Vec<Value>)> = Vec::new();

    for (position, row) in ROWS.iter().enumerate() {
        // One plan per row, each with its own name and application source, so no row's
        // plan is refused for colliding with another's.
        let plan = format!("plan {position}");
        let emulator = Emulator::new();
        emulator
            .ok(
                "CreateScalingPlan",
                json!({
                    "ScalingPlanName": plan,
                    "ApplicationSource": stack_source(position),
                    "ScalingInstructions": [predictive_instruction(row)],
                }),
            )
            .await;
        emulator.make_all_due();

        // Round trip: the instruction the create stored reaches the resource read, which
        // is what the forecast below is issued from.
        let resources = emulator
            .ok(
                "DescribeScalingPlanResources",
                json!({ "ScalingPlanName": plan, "ScalingPlanVersion": ONLY_VERSION }),
            )
            .await;
        let stored: Vec<(String, String, String)> = resources["ScalingPlanResources"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(crate::scaling_plan_lifecycle::resource_identity)
            .collect();
        assert_eq!(
            stored,
            vec![(
                row.namespace.to_string(),
                row.resource_id.to_string(),
                row.dimension.to_string()
            )],
            "case {:?}: the created plan must cover exactly the resource its instruction \
             names, got {stored:?}",
            row.label
        );

        for data_type in FORECAST_DATA_TYPE_VALUES {
            let request = forecast_request(
                &plan,
                row.namespace,
                row.resource_id,
                row.dimension,
                data_type,
            );
            let body = emulator.ok(OPERATION, request.clone()).await;
            let case = format!("{} / {data_type}", row.label);
            let points = datapoints(&body).clone();
            assert_eq!(
                points.len(),
                WINDOW_PERIODS as usize,
                "case {case:?}: a window of {WINDOW_PERIODS} forecast periods over an \
                 instructed resource must carry one data point per period, got {body}"
            );
            for point in &points {
                let value = point["Value"].as_f64().unwrap_or_else(|| {
                    panic!("case {case:?}: expected a numeric data point value, got {point}")
                });
                assert!(
                    value > 0.0,
                    "case {case:?}: a forecast computed from a band of \
                     {MIN_CAPACITY}-{MAX_CAPACITY} units must be positive, got {value}"
                );
                assert!(
                    point["Timestamp"].is_number(),
                    "case {case:?}: every data point must carry the instant it is \
                     reported at, got {point}"
                );
            }

            // A read reports, so reading the same window again answers the same thing.
            let again = emulator.ok(OPERATION, request).await;
            assert_eq!(
                datapoints(&again),
                &points,
                "case {case:?}: a second read of the same window must not contradict the \
                 first, got {again}"
            );

            // The forecast follows the instruction, not the resource kind.
            match expected
                .iter()
                .find(|(kind, _)| *kind == *data_type)
                .map(|(_, points)| points.clone())
            {
                Some(first_rows_points) => assert_eq!(
                    points, first_rows_points,
                    "case {case:?}: an identical instruction must forecast identically \
                     whatever resource it names, expected {first_rows_points:?}, got \
                     {points:?}"
                ),
                None => expected.push((data_type, points)),
            }
        }
    }
}

#[tokio::test]
async fn a_dimension_owned_by_another_namespace_is_refused() {
    let emulator = with_an_unrelated_plan().await;

    // Every dimension against every namespace that does NOT own it: a dimension's own
    // namespace is its prefix (source: `smithy_query(query="shape",
    // shape="ScalableDimension")`), and the namespaces are every value the other member's
    // enum lists (`SERVICE_NAMESPACE_VALUES`). Driven against a plan this service holds
    // and a name it does not, because the refusal is about the request alone.
    for row in &ROWS {
        for namespace in SERVICE_NAMESPACE_VALUES {
            if *namespace == row.namespace {
                continue;
            }
            for plan in [UNRELATED_PLAN, ABSENT_PLAN] {
                let request = forecast_request(
                    plan,
                    namespace,
                    row.resource_id,
                    row.dimension,
                    "LoadForecast",
                );
                let case = format!("{} under the {namespace} namespace / {plan}", row.label);
                assert_refused(&emulator, &case, &request).await;
            }
        }
    }
}

#[tokio::test]
async fn a_resource_id_of_another_resource_type_is_refused() {
    let emulator = with_an_unrelated_plan().await;

    // The id carries no resource type at all, and the id of a resource of ANOTHER kind:
    // either way it identifies no resource of the kind the row's dimension scales. The
    // second spelling is the Auto Scaling group example, which every non-autoscaling row
    // is driven with; the Auto Scaling group row is driven with a DynamoDB table id.
    for row in &ROWS {
        let foreign = if row.namespace == "autoscaling" {
            "table/my-table"
        } else {
            "autoScalingGroup/my-asg"
        };
        for (spelling, resource_id) in [
            ("an id with no resource type", "my-resource"),
            ("an id of another resource type", foreign),
        ] {
            let request = forecast_request(
                UNRELATED_PLAN,
                row.namespace,
                resource_id,
                row.dimension,
                "CapacityForecast",
            );
            let case = format!("{} with {spelling} ({resource_id})", row.label);
            assert_refused(&emulator, &case, &request).await;
        }
    }
}

/// Assert one request is refused the way a triple that describes no scalable resource is:
/// HTTP 400 `ValidationException` carrying the one sentence this service words it with.
async fn assert_refused(emulator: &Emulator, case: &str, request: &Value) {
    let (status, body) = emulator.call(OPERATION, request).await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "ValidationException"),
        "case {case:?}: expected HTTP 400 ValidationException, got {status} with {body}"
    );
    let message = error_message(&body);
    assert_eq!(
        message, UNSUPPORTED_RESOURCE,
        "case {case:?}: expected {UNSUPPORTED_RESOURCE:?}, got {message:?}"
    );
}
