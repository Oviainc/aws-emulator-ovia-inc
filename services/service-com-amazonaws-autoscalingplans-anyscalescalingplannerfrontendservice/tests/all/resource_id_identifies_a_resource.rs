// SPDX-License-Identifier: CC0-1.0

//! A `ResourceId` names a scalable resource only when its resource type is followed by a
//! unique identifier.
//!
//! `ResourceId` "consists of the resource type and unique identifier"
//! (`smithy_query(query="doc",shape="ScalingInstruction",member="ResourceId")`, and the
//! forecast read's own member: "a prefix (`autoScalingGroup`) followed by the name of a
//! specified Auto Scaling group"), so an id that carries nothing but the resource type of
//! its `ScalableDimension` identifies nothing and is refused — and where the identifier
//! addresses a resource THROUGH a parent (a DynamoDB global secondary index, whose
//! identifier is its table then its index, `table/my-table/index/my-table-index`),
//! neither part may be empty either.
//!
//! One predicate decides this for the whole crate, so the SAME table of `ResourceId`
//! spellings is driven through every path that asks: `CreateScalingPlan` and
//! `UpdateScalingPlan` on a scaling instruction's id, and
//! `GetScalingPlanResourceForecastData` on its own. Rows differ along what the rule
//! generalises over: the resource TYPE (every `ScalableDimension` the model lists), the
//! LENGTH of the identifier (absent, one character, a worked example), which PART of a
//! two-part identifier is empty, whether the identifier carries a separator of its own,
//! and whether the type prefix is there at all.
//!
//! The harness (router, dispatch, error-body readers) is reused from
//! [`crate::scaling_plan_lifecycle`]; nothing here builds a second one.

use serde_json::{json, Value};

use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::get_scaling_plan_resource_forecast_data::{
    FORECAST_INTERVAL_SECONDS, FORECAST_SCALABLE_DIMENSION,
};
use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::state::UNSUPPORTED_RESOURCE;

use crate::scaling_plan_lifecycle::{
    error_code, error_message, now_secs, resource_identity, simple_instructions, stack_source,
    Emulator,
};

/// The plan the write-path tests create.
const PLAN: &str = "resource-id-plan";

/// The only scaling-plan version the model documents as valid.
const ONLY_VERSION: i64 = 1;

/// One `ResourceId` spelling, with the scalable resource coordinates it is used under.
struct Row {
    label: &'static str,
    namespace: &'static str,
    resource_id: &'static str,
    dimension: &'static str,
    /// A scaling metric that measures this kind of resource, so only the `ResourceId`
    /// decides the row's verdict. Source: `ScalingMetricType`
    /// (`smithy_query(query="shape",shape="ScalingMetricType")`).
    metric: &'static str,
    /// Whether the id identifies a resource of the kind the dimension scales.
    identifies: bool,
    /// Whether a WRITE path refuses this row on this rule's own sentence. An id the
    /// model's own length constraint on `ScalingInstruction$ResourceId` (1-1600,
    /// `smithy_query(query="shape",shape="ScalingInstruction")`) already refuses never
    /// reaches the rule there — while the forecast read's `ResourceId` is an `XmlString`
    /// under no such minimum, so the rule is what refuses it.
    on_the_rule_when_written: bool,
}

impl Row {
    /// This row as the one `ScalingInstruction` a write carries.
    fn instruction(&self) -> Value {
        json!({
            "ServiceNamespace": self.namespace,
            "ResourceId": self.resource_id,
            "ScalableDimension": self.dimension,
            "MinCapacity": 1,
            "MaxCapacity": 10,
            "TargetTrackingConfigurations": [{
                "PredefinedScalingMetricSpecification": {
                    "PredefinedScalingMetricType": self.metric
                },
                "TargetValue": 40.0
            }]
        })
    }
}

/// A row that names a resource, so every path serves it.
fn names(
    label: &'static str,
    namespace: &'static str,
    resource_id: &'static str,
    dimension: &'static str,
    metric: &'static str,
) -> Row {
    Row {
        label,
        namespace,
        resource_id,
        dimension,
        metric,
        identifies: true,
        on_the_rule_when_written: true,
    }
}

/// A row that names no resource, so every path refuses it with the one sentence.
fn names_nothing(
    label: &'static str,
    namespace: &'static str,
    resource_id: &'static str,
    dimension: &'static str,
    metric: &'static str,
) -> Row {
    Row {
        label,
        namespace,
        resource_id,
        dimension,
        metric,
        identifies: false,
        on_the_rule_when_written: true,
    }
}

/// The table every test in this suite spans.
///
/// The resource types, their separators and the worked identifier examples come from the
/// model's own documentation of the member
/// (`smithy_query(query="doc",shape="ScalingInstruction",member="ResourceId")`), covering
/// every value of `ScalableDimension`
/// (`smithy_query(query="shape",shape="ScalableDimension")`).
fn rows() -> Vec<Row> {
    vec![
        // A bare resource type, once per kind of resource the model lists.
        names_nothing(
            "an Auto Scaling group id that is only the resource type",
            "autoscaling",
            "autoScalingGroup/",
            FORECAST_SCALABLE_DIMENSION,
            "ASGAverageCPUUtilization",
        ),
        names_nothing(
            "an ECS service id that is only the resource type",
            "ecs",
            "service/",
            "ecs:service:DesiredCount",
            "ECSServiceAverageCPUUtilization",
        ),
        names_nothing(
            "a Spot Fleet request id that is only the resource type",
            "ec2",
            "spot-fleet-request/",
            "ec2:spot-fleet-request:TargetCapacity",
            "EC2SpotFleetRequestAverageCPUUtilization",
        ),
        names_nothing(
            "an Aurora DB cluster id that is only the resource type",
            "rds",
            "cluster:",
            "rds:cluster:ReadReplicaCount",
            "RDSReaderAverageCPUUtilization",
        ),
        names_nothing(
            "a DynamoDB table read-capacity id that is only the resource type",
            "dynamodb",
            "table/",
            "dynamodb:table:ReadCapacityUnits",
            "DynamoDBReadCapacityUtilization",
        ),
        names_nothing(
            "a DynamoDB table write-capacity id that is only the resource type",
            "dynamodb",
            "table/",
            "dynamodb:table:WriteCapacityUnits",
            "DynamoDBWriteCapacityUtilization",
        ),
        // A two-part identifier with either part, or both, empty.
        names_nothing(
            "an index id naming neither a table nor an index",
            "dynamodb",
            "table/index/",
            "dynamodb:index:ReadCapacityUnits",
            "DynamoDBReadCapacityUtilization",
        ),
        names_nothing(
            "an index id whose table name is empty",
            "dynamodb",
            "table//index/my-table-index",
            "dynamodb:index:ReadCapacityUnits",
            "DynamoDBReadCapacityUtilization",
        ),
        names_nothing(
            "an index id whose index name is empty",
            "dynamodb",
            "table/my-table/index/",
            "dynamodb:index:WriteCapacityUnits",
            "DynamoDBWriteCapacityUtilization",
        ),
        // The other halves of the rule: no separator, the wrong resource type, and an id
        // with no text at all.
        names_nothing(
            "the resource type without its separator",
            "autoscaling",
            "autoScalingGroup",
            FORECAST_SCALABLE_DIMENSION,
            "ASGAverageCPUUtilization",
        ),
        names_nothing(
            "a DynamoDB table id under the Auto Scaling group dimension",
            "autoscaling",
            "table/my-table",
            FORECAST_SCALABLE_DIMENSION,
            "ASGAverageCPUUtilization",
        ),
        Row {
            label: "an id with no text at all",
            namespace: "autoscaling",
            resource_id: "",
            dimension: FORECAST_SCALABLE_DIMENSION,
            metric: "ASGAverageCPUUtilization",
            identifies: false,
            on_the_rule_when_written: false,
        },
        // Ids that DO name a resource, so the rule refuses no more than it should.
        names(
            "the documented Auto Scaling group example",
            "autoscaling",
            "autoScalingGroup/my-asg",
            FORECAST_SCALABLE_DIMENSION,
            "ASGAverageCPUUtilization",
        ),
        names(
            "the shortest Auto Scaling group name",
            "autoscaling",
            "autoScalingGroup/a",
            FORECAST_SCALABLE_DIMENSION,
            "ASGAverageCPUUtilization",
        ),
        names(
            "an Auto Scaling group name carrying a separator of its own",
            "autoscaling",
            "autoScalingGroup/team/my-asg",
            FORECAST_SCALABLE_DIMENSION,
            "ASGAverageCPUUtilization",
        ),
        names(
            "the documented ECS service example",
            "ecs",
            "service/default/sample-webapp",
            "ecs:service:DesiredCount",
            "ECSServiceAverageCPUUtilization",
        ),
        names(
            "the documented Spot Fleet request example",
            "ec2",
            "spot-fleet-request/sfr-73fbd2ce-aa30-494c-8788-1cee4EXAMPLE",
            "ec2:spot-fleet-request:TargetCapacity",
            "EC2SpotFleetRequestAverageCPUUtilization",
        ),
        names(
            "the documented Aurora DB cluster example",
            "rds",
            "cluster:my-db-cluster",
            "rds:cluster:ReadReplicaCount",
            "RDSReaderAverageCPUUtilization",
        ),
        names(
            "the documented DynamoDB table example",
            "dynamodb",
            "table/my-table",
            "dynamodb:table:ReadCapacityUnits",
            "DynamoDBReadCapacityUtilization",
        ),
        names(
            "the documented DynamoDB index example",
            "dynamodb",
            "table/my-table/index/my-table-index",
            "dynamodb:index:ReadCapacityUnits",
            "DynamoDBReadCapacityUtilization",
        ),
        names(
            "an index id whose two parts are one character each",
            "dynamodb",
            "table/t/index/i",
            "dynamodb:index:WriteCapacityUnits",
            "DynamoDBWriteCapacityUtilization",
        ),
    ]
}

/// Assert one refusal: HTTP 400 `ValidationException`, carrying the one sentence this
/// service refuses a triple naming no scalable resource with when the refusal is on this
/// rule.
fn assert_refused(label: &str, path: &str, on_the_rule: bool, status: u16, body: &Value) {
    assert_eq!(
        (status, error_code(body)),
        (400, "ValidationException"),
        "case {label:?} on {path}: expected HTTP 400 ValidationException, got {status} \
         with {body}"
    );
    if on_the_rule {
        assert_eq!(
            error_message(body),
            UNSUPPORTED_RESOURCE,
            "case {label:?} on {path}: expected the refusal for a triple that names no \
             scalable resource, got {body}"
        );
    }
}

#[tokio::test]
async fn create_stores_only_a_resource_id_that_identifies_a_resource() {
    for row in rows() {
        let label = row.label;
        let emulator = Emulator::new();
        let (status, body) = emulator
            .call(
                "CreateScalingPlan",
                &json!({
                    "ScalingPlanName": PLAN,
                    "ApplicationSource": stack_source(1),
                    "ScalingInstructions": [row.instruction()],
                }),
            )
            .await;

        if !row.identifies {
            assert_refused(
                label,
                "CreateScalingPlan",
                row.on_the_rule_when_written,
                status,
                &body,
            );
            // A refused create stores nothing: no plan, hence no resource.
            let described = emulator
                .ok("DescribeScalingPlans", json!({ "ScalingPlanNames": [PLAN] }))
                .await;
            assert_eq!(
                described.get("ScalingPlans"),
                Some(&json!([])),
                "case {label:?}: a refused create must store no plan, got {described}"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {label:?}: the id names a resource, so the create must be accepted, \
             got {status} with {body}"
        );
        // Round trip: the stored instruction reaches the resource read, which mints the
        // scalable resource it names.
        emulator.make_all_due();
        let resources = emulator
            .ok(
                "DescribeScalingPlanResources",
                json!({ "ScalingPlanName": PLAN, "ScalingPlanVersion": ONLY_VERSION }),
            )
            .await;
        let identities: Vec<_> = resources["ScalingPlanResources"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(resource_identity)
            .collect();
        assert_eq!(
            identities,
            vec![(
                row.namespace.to_string(),
                row.resource_id.to_string(),
                row.dimension.to_string()
            )],
            "case {label:?}: the created plan must cover exactly the resource its \
             instruction names, got {identities:?}"
        );
    }
}

#[tokio::test]
async fn update_applies_the_same_rule_create_applies() {
    for row in rows() {
        let label = row.label;
        let emulator = Emulator::new();
        emulator
            .ok(
                "CreateScalingPlan",
                json!({
                    "ScalingPlanName": PLAN,
                    "ApplicationSource": stack_source(1),
                    "ScalingInstructions": simple_instructions(),
                }),
            )
            .await;
        emulator.make_all_due();

        let (status, body) = emulator
            .call(
                "UpdateScalingPlan",
                &json!({
                    "ScalingPlanName": PLAN,
                    "ScalingPlanVersion": ONLY_VERSION,
                    "ScalingInstructions": [row.instruction()],
                }),
            )
            .await;

        if !row.identifies {
            assert_refused(
                label,
                "UpdateScalingPlan",
                row.on_the_rule_when_written,
                status,
                &body,
            );
            let described = emulator
                .ok("DescribeScalingPlans", json!({ "ScalingPlanNames": [PLAN] }))
                .await;
            assert_eq!(
                described["ScalingPlans"][0].get("ScalingInstructions"),
                Some(&json!(simple_instructions())),
                "case {label:?}: a refused update must leave the stored instructions \
                 alone, got {described}"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {label:?}: the id names a resource, so the update must be accepted, \
             got {status} with {body}"
        );
        emulator.make_all_due();
        let described = emulator
            .ok("DescribeScalingPlans", json!({ "ScalingPlanNames": [PLAN] }))
            .await;
        assert_eq!(
            described["ScalingPlans"][0].get("ScalingInstructions"),
            Some(&json!([row.instruction()])),
            "case {label:?}: the update must store the instruction it was given, got \
             {described}"
        );
    }
}

#[tokio::test]
async fn the_forecast_read_applies_the_same_rule_to_its_own_resource_id() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": PLAN,
                "ApplicationSource": stack_source(1),
                "ScalingInstructions": simple_instructions(),
            }),
        )
        .await;
    emulator.make_all_due();

    for row in rows() {
        let label = row.label;
        // The read refuses a triple that describes no scalable resource and nothing else,
        // so the verdict turns on the resource id alone — whatever kind of resource the
        // row's dimension scales. Every row's namespace already owns its dimension.
        let forecast_served = row.identifies;
        let start = now_secs();
        let (status, body) = emulator
            .call(
                "GetScalingPlanResourceForecastData",
                &json!({
                    "ScalingPlanName": PLAN,
                    "ScalingPlanVersion": ONLY_VERSION,
                    "ServiceNamespace": row.namespace,
                    "ResourceId": row.resource_id,
                    "ScalableDimension": row.dimension,
                    "ForecastDataType": "CapacityForecast",
                    "StartTime": start,
                    "EndTime": start + FORECAST_INTERVAL_SECONDS,
                }),
            )
            .await;

        if !forecast_served {
            assert_refused(
                label,
                "GetScalingPlanResourceForecastData",
                true,
                status,
                &body,
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {label:?} on GetScalingPlanResourceForecastData: the id names a \
             resource of the kind its dimension scales, so the read must be served, got \
             {status} with {body}"
        );
        assert!(
            body["Datapoints"].is_array(),
            "case {label:?} on GetScalingPlanResourceForecastData: expected the required \
             Datapoints list, got {body}"
        );
    }
}
