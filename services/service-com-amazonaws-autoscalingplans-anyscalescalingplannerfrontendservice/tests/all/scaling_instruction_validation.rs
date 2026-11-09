// SPDX-License-Identifier: CC0-1.0

//! Two rules, each asserted over a table rather than over one recorded request.
//!
//! 1. A write that carries scaling instructions AWS cannot apply — instructions whose
//!    members contradict each other or the documented relations between them — is
//!    refused with the modelled `ValidationException` (HTTP 400) and stores nothing.
//!    The rows below each name a different documented relation; a maximum capacity
//!    below its minimum is one of them, not the rule.
//! 2. `DescribeScalingPlanResources` identifies ONE plan by its two required members but
//!    models no not-found error, so a lookup on a `ScalingPlanName` that finds no stored
//!    plan reports the empty set of scalable resources (HTTP 200,
//!    `ScalingPlanResources` = `[]`) rather than a refusal.
//!
//! The harness (`Emulator`, `instruction`, `stack_source`) is the sibling suite's: every
//! call goes over the same `awsJson1_1` wire path the router serves in production.

use serde_json::{json, Value};

use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::state::{
    ALB_LOAD_METRIC_TYPE, ALB_SCALING_METRIC_TYPE, BEHAVIOR_ABOVE_FORECAST_CAPACITY,
    MAX_CAPACITY_BUFFER_MAX, MAX_CAPACITY_BUFFER_MIN, MAX_TARGET_TRACKING_CONFIGURATIONS,
    MIN_RESOURCE_CAPACITY, TARGET_VALUE_MAX, TARGET_VALUE_MIN,
};

use crate::scaling_plan_lifecycle::{
    instruction, simple_instructions, stack_source, Emulator,
};

/// The instruction every row starts from: valid, and the one the sibling suite uses.
fn valid_instruction() -> Value {
    instruction(
        "autoscaling",
        "autoScalingGroup/my-asg",
        "autoscaling:autoScalingGroup:DesiredCapacity",
        1,
        10,
        "ASGAverageCPUUtilization",
        40.0,
    )
}

/// [`valid_instruction`] with `members` applied: a `null` value REMOVES the member, so a
/// row can drop one as easily as it can change one.
///
/// `pub(crate)` so every suite in this test target builds its instruction rows from the
/// one valid instruction rather than from a second copy of it.
pub(crate) fn with(members: &[(&str, Value)]) -> Value {
    let mut built = valid_instruction();
    let object = built.as_object_mut().expect("an instruction is an object");
    for (member, value) in members {
        if value.is_null() {
            object.remove(*member);
        } else {
            object.insert((*member).to_string(), value.clone());
        }
    }
    built
}

/// `count` target tracking configurations, each tracking a DIFFERENT customized metric,
/// so only the LIST's own length distinguishes the rows that use this.
///
/// `pub(crate)` so a suite that varies the LENGTH of this list builds it the same way
/// rather than from a second copy.
pub(crate) fn customized_configurations(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|n| {
                json!({
                    "CustomizedScalingMetricSpecification": {
                        "MetricName": format!("my-metric-{n}"),
                        "Namespace": "MyApp",
                        "Statistic": "Average"
                    },
                    "TargetValue": 50.0 + n as f64
                })
            })
            .collect::<Vec<Value>>(),
    )
}

/// One predefined-metric target tracking configuration.
fn predefined_configuration(metric: &str, target: f64) -> Value {
    json!({
        "PredefinedScalingMetricSpecification": { "PredefinedScalingMetricType": metric },
        "TargetValue": target
    })
}

/// Instruction lists AWS refuses, one per documented relation.
///
/// Enum value lists come from the model itself
/// (`smithy_query(query="shape",shape="ServiceNamespace,ScalableDimension,ScalingMetricType,LoadMetricType")`),
/// the capacity, buffer and list bounds from the member docs cited on the constants in
/// `src/state.rs`.
fn refused_instruction_lists() -> Vec<(&'static str, Value)> {
    vec![
        (
            "no scaling instruction at all",
            json!([]),
        ),
        (
            "a required capacity member missing",
            json!([with(&[("MinCapacity", Value::Null)])]),
        ),
        (
            "the required target tracking configurations member missing",
            json!([with(&[("TargetTrackingConfigurations", Value::Null)])]),
        ),
        (
            "maximum capacity below minimum capacity",
            json!([with(&[("MinCapacity", json!(10)), ("MaxCapacity", json!(2))])]),
        ),
        (
            "maximum capacity one below minimum capacity",
            json!([with(&[("MinCapacity", json!(4)), ("MaxCapacity", json!(3))])]),
        ),
        (
            "a minimum capacity one below the floor",
            json!([with(&[("MinCapacity", json!(MIN_RESOURCE_CAPACITY - 1))])]),
        ),
        (
            "a negative minimum capacity",
            json!([with(&[("MinCapacity", json!(-1))])]),
        ),
        (
            "a negative maximum capacity",
            json!([with(&[("MaxCapacity", json!(-1))])]),
        ),
        (
            "service namespace outside the modelled enum",
            json!([with(&[("ServiceNamespace", json!("lambda"))])]),
        ),
        (
            "scalable dimension outside the modelled enum",
            json!([with(&[(
                "ScalableDimension",
                json!("autoscaling:autoScalingGroup:Nonsense")
            )])]),
        ),
        (
            "scalable dimension from another service namespace",
            json!([with(&[(
                "ScalableDimension",
                json!("ecs:service:DesiredCount")
            )])]),
        ),
        (
            "resource id that is not of the dimension's resource type",
            json!([with(&[("ResourceId", json!("table/my-table"))])]),
        ),
        (
            "dynamodb index dimension addressed with a plain table id",
            json!([with(&[
                ("ServiceNamespace", json!("dynamodb")),
                ("ResourceId", json!("table/my-table")),
                ("ScalableDimension", json!("dynamodb:index:ReadCapacityUnits")),
                (
                    "TargetTrackingConfigurations",
                    json!([predefined_configuration("DynamoDBReadCapacityUtilization", 70.0)])
                ),
            ])]),
        ),
        (
            "a predefined metric type the scalable dimension does not support",
            json!([with(&[(
                "TargetTrackingConfigurations",
                json!([predefined_configuration("DynamoDBReadCapacityUtilization", 70.0)])
            )])]),
        ),
        (
            "the ALB metric type on a resource it does not apply to",
            json!([with(&[
                ("ServiceNamespace", json!("rds")),
                ("ResourceId", json!("cluster:my-db-cluster")),
                ("ScalableDimension", json!("rds:cluster:ReadReplicaCount")),
                (
                    "TargetTrackingConfigurations",
                    json!([predefined_configuration(ALB_SCALING_METRIC_TYPE, 1000.0)])
                ),
            ])]),
        ),
        (
            "no target tracking configuration",
            json!([with(&[("TargetTrackingConfigurations", json!([]))])]),
        ),
        (
            "one target tracking configuration past the documented bound",
            json!([with(&[(
                "TargetTrackingConfigurations",
                customized_configurations(MAX_TARGET_TRACKING_CONFIGURATIONS + 1)
            )])]),
        ),
        (
            "the same scaling metric tracked twice",
            json!([with(&[(
                "TargetTrackingConfigurations",
                json!([
                    predefined_configuration("ASGAverageCPUUtilization", 40.0),
                    predefined_configuration("ASGAverageCPUUtilization", 60.0)
                ])
            )])]),
        ),
        (
            "both a predefined and a customized scaling metric",
            json!([with(&[(
                "TargetTrackingConfigurations",
                json!([{
                    "PredefinedScalingMetricSpecification": {
                        "PredefinedScalingMetricType": "ASGAverageCPUUtilization"
                    },
                    "CustomizedScalingMetricSpecification": {
                        "MetricName": "my-metric",
                        "Namespace": "MyApp",
                        "Statistic": "Average"
                    },
                    "TargetValue": 40.0
                }])
            )])]),
        ),
        (
            "neither a predefined nor a customized scaling metric",
            json!([with(&[(
                "TargetTrackingConfigurations",
                json!([{ "TargetValue": 40.0 }])
            )])]),
        ),
        (
            "a resource label on a metric type that is not the ALB one",
            json!([with(&[(
                "TargetTrackingConfigurations",
                json!([{
                    "PredefinedScalingMetricSpecification": {
                        "PredefinedScalingMetricType": "ASGAverageCPUUtilization",
                        "ResourceLabel": "app/my-lb/abc/targetgroup/my-tg/def"
                    },
                    "TargetValue": 40.0
                }])
            )])]),
        ),
        (
            "a zero target value",
            json!([with(&[(
                "TargetTrackingConfigurations",
                json!([predefined_configuration("ASGAverageCPUUtilization", 0.0)])
            )])]),
        ),
        (
            "a negative target value",
            json!([with(&[(
                "TargetTrackingConfigurations",
                json!([predefined_configuration("ASGAverageCPUUtilization", -40.0)])
            )])]),
        ),
        (
            "a target value below the published lower bound",
            json!([with(&[(
                "TargetTrackingConfigurations",
                json!([predefined_configuration(
                    "ASGAverageCPUUtilization",
                    TARGET_VALUE_MIN / 2.0
                )])
            )])]),
        ),
        (
            "a target value above the published upper bound",
            json!([with(&[(
                "TargetTrackingConfigurations",
                json!([predefined_configuration(
                    "ASGAverageCPUUtilization",
                    TARGET_VALUE_MAX * 2.0
                )])
            )])]),
        ),
        (
            "a negative scale-in cooldown",
            json!([with(&[(
                "TargetTrackingConfigurations",
                json!([{
                    "PredefinedScalingMetricSpecification": {
                        "PredefinedScalingMetricType": "ASGAverageCPUUtilization"
                    },
                    "TargetValue": 40.0,
                    "ScaleInCooldown": -30
                }])
            )])]),
        ),
        (
            "both load metric specifications",
            json!([with(&[
                (
                    "PredefinedLoadMetricSpecification",
                    json!({ "PredefinedLoadMetricType": "ASGTotalNetworkIn" })
                ),
                (
                    "CustomizedLoadMetricSpecification",
                    json!({
                        "MetricName": "my-load",
                        "Namespace": "MyApp",
                        "Statistic": "Sum"
                    })
                ),
            ])]),
        ),
        (
            "a customized load metric whose statistic is not Sum",
            json!([with(&[(
                "CustomizedLoadMetricSpecification",
                json!({ "MetricName": "my-load", "Namespace": "MyApp", "Statistic": "Average" })
            )])]),
        ),
        (
            "a load metric resource label on a metric type that is not the ALB one",
            json!([with(&[(
                "PredefinedLoadMetricSpecification",
                json!({
                    "PredefinedLoadMetricType": "ASGTotalNetworkIn",
                    "ResourceLabel": "app/my-lb/abc/targetgroup/my-tg/def"
                })
            )])]),
        ),
        (
            "a max-capacity behavior without predictive scaling",
            json!([with(&[(
                "PredictiveScalingMaxCapacityBehavior",
                json!("SetForecastCapacityToMaxCapacity")
            )])]),
        ),
        (
            "a max-capacity buffer without predictive scaling",
            json!([with(&[("PredictiveScalingMaxCapacityBuffer", json!(10))])]),
        ),
        (
            "a max-capacity buffer the behavior does not allow",
            json!([with(&[
                (
                    "PredefinedLoadMetricSpecification",
                    json!({ "PredefinedLoadMetricType": "ASGTotalNetworkIn" })
                ),
                (
                    "PredictiveScalingMaxCapacityBehavior",
                    json!("SetMaxCapacityToForecastCapacity")
                ),
                ("PredictiveScalingMaxCapacityBuffer", json!(10)),
            ])]),
        ),
        (
            "the behavior that requires a buffer, without one",
            json!([with(&[
                (
                    "PredefinedLoadMetricSpecification",
                    json!({ "PredefinedLoadMetricType": "ASGTotalNetworkIn" })
                ),
                (
                    "PredictiveScalingMaxCapacityBehavior",
                    json!(BEHAVIOR_ABOVE_FORECAST_CAPACITY)
                ),
            ])]),
        ),
        (
            "a buffer one below the documented range",
            json!([with(&[
                (
                    "PredefinedLoadMetricSpecification",
                    json!({ "PredefinedLoadMetricType": "ASGTotalNetworkIn" })
                ),
                (
                    "PredictiveScalingMaxCapacityBehavior",
                    json!(BEHAVIOR_ABOVE_FORECAST_CAPACITY)
                ),
                (
                    "PredictiveScalingMaxCapacityBuffer",
                    json!(MAX_CAPACITY_BUFFER_MIN - 1)
                ),
            ])]),
        ),
        (
            "a buffer one above the documented range",
            json!([with(&[
                (
                    "PredefinedLoadMetricSpecification",
                    json!({ "PredefinedLoadMetricType": "ASGTotalNetworkIn" })
                ),
                (
                    "PredictiveScalingMaxCapacityBehavior",
                    json!(BEHAVIOR_ABOVE_FORECAST_CAPACITY)
                ),
                (
                    "PredictiveScalingMaxCapacityBuffer",
                    json!(MAX_CAPACITY_BUFFER_MAX + 1)
                ),
            ])]),
        ),
        (
            "two instructions for the same scalable resource",
            json!([valid_instruction(), valid_instruction()]),
        ),
    ]
}

/// Instruction lists AWS applies, including each published bound itself.
fn accepted_instruction_lists() -> Vec<(&'static str, Value)> {
    vec![
        ("the plain one-instruction plan", json!(simple_instructions())),
        (
            "minimum capacity equal to maximum capacity",
            json!([with(&[("MinCapacity", json!(7)), ("MaxCapacity", json!(7))])]),
        ),
        (
            "a minimum capacity at the floor",
            json!([with(&[("MinCapacity", json!(MIN_RESOURCE_CAPACITY))])]),
        ),
        (
            "exactly as many target tracking configurations as the bound allows",
            json!([with(&[(
                "TargetTrackingConfigurations",
                customized_configurations(MAX_TARGET_TRACKING_CONFIGURATIONS)
            )])]),
        ),
        (
            "the ALB metric type with the resource label it allows",
            json!([with(&[(
                "TargetTrackingConfigurations",
                json!([{
                    "PredefinedScalingMetricSpecification": {
                        "PredefinedScalingMetricType": ALB_SCALING_METRIC_TYPE,
                        "ResourceLabel": "app/my-lb/abc/targetgroup/my-tg/def"
                    },
                    "TargetValue": 1000.0
                }])
            )])]),
        ),
        (
            "the ALB metric type on an ECS service",
            json!([with(&[
                ("ServiceNamespace", json!("ecs")),
                ("ResourceId", json!("service/default/sample-webapp")),
                ("ScalableDimension", json!("ecs:service:DesiredCount")),
                (
                    "TargetTrackingConfigurations",
                    json!([predefined_configuration(ALB_SCALING_METRIC_TYPE, 1000.0)])
                ),
            ])]),
        ),
        (
            "a target value at the published lower bound",
            json!([with(&[(
                "TargetTrackingConfigurations",
                json!([predefined_configuration("ASGAverageCPUUtilization", TARGET_VALUE_MIN)])
            )])]),
        ),
        (
            "a target value at the published upper bound",
            json!([with(&[(
                "TargetTrackingConfigurations",
                json!([predefined_configuration("ASGAverageCPUUtilization", TARGET_VALUE_MAX)])
            )])]),
        ),
        (
            "a customized scaling metric with dimensions",
            json!([with(&[(
                "TargetTrackingConfigurations",
                json!([{
                    "CustomizedScalingMetricSpecification": {
                        "MetricName": "queue-depth",
                        "Namespace": "MyApp",
                        "Statistic": "Sum",
                        "Dimensions": [{ "Name": "Queue", "Value": "orders" }],
                        "Unit": "Count"
                    },
                    "TargetValue": 25.0
                }])
            )])]),
        ),
        (
            "predictive scaling with a buffer at the low bound",
            json!([with(&[
                (
                    "PredefinedLoadMetricSpecification",
                    json!({
                        "PredefinedLoadMetricType": ALB_LOAD_METRIC_TYPE,
                        "ResourceLabel": "app/my-lb/abc/targetgroup/my-tg/def"
                    })
                ),
                (
                    "PredictiveScalingMaxCapacityBehavior",
                    json!(BEHAVIOR_ABOVE_FORECAST_CAPACITY)
                ),
                (
                    "PredictiveScalingMaxCapacityBuffer",
                    json!(MAX_CAPACITY_BUFFER_MIN)
                ),
                ("ScheduledActionBufferTime", json!(0)),
            ])]),
        ),
        (
            "predictive scaling with a buffer at the high bound",
            json!([with(&[
                (
                    "CustomizedLoadMetricSpecification",
                    json!({ "MetricName": "my-load", "Namespace": "MyApp", "Statistic": "Sum" })
                ),
                (
                    "PredictiveScalingMaxCapacityBehavior",
                    json!(BEHAVIOR_ABOVE_FORECAST_CAPACITY)
                ),
                (
                    "PredictiveScalingMaxCapacityBuffer",
                    json!(MAX_CAPACITY_BUFFER_MAX)
                ),
                ("PredictiveScalingMode", json!("ForecastOnly")),
            ])]),
        ),
        (
            "two instructions for two different resources",
            json!([
                valid_instruction(),
                instruction(
                    "dynamodb",
                    "table/my-table",
                    "dynamodb:table:ReadCapacityUnits",
                    5,
                    500,
                    "DynamoDBReadCapacityUtilization",
                    70.0,
                ),
            ]),
        ),
    ]
}

#[tokio::test]
async fn create_refuses_scaling_instructions_aws_cannot_apply() {
    for (label, instructions) in refused_instruction_lists() {
        let emulator = Emulator::new();
        let (status, code) = emulator
            .err(
                "CreateScalingPlan",
                json!({
                    "ScalingPlanName": "my-plan",
                    "ApplicationSource": stack_source(1),
                    "ScalingInstructions": instructions,
                }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {label:?}: expected HTTP 400 ValidationException, got {status} / {code:?}"
        );

        // A refused create stores nothing: the plan it would have made does not exist.
        let described = emulator
            .ok("DescribeScalingPlans", json!({ "ScalingPlanNames": ["my-plan"] }))
            .await;
        assert_eq!(
            described.get("ScalingPlans"),
            Some(&json!([])),
            "case {label:?}: a refused create must store no plan, got {described}"
        );
    }
}

#[tokio::test]
async fn update_refuses_the_same_scaling_instructions_create_refuses() {
    for (label, instructions) in refused_instruction_lists() {
        let emulator = Emulator::new();
        emulator
            .ok(
                "CreateScalingPlan",
                json!({
                    "ScalingPlanName": "my-plan",
                    "ApplicationSource": stack_source(1),
                    "ScalingInstructions": simple_instructions(),
                }),
            )
            .await;
        emulator.make_all_due();

        let (status, code) = emulator
            .err(
                "UpdateScalingPlan",
                json!({
                    "ScalingPlanName": "my-plan",
                    "ScalingPlanVersion": 1,
                    "ScalingInstructions": instructions,
                }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {label:?}: expected HTTP 400 ValidationException, got {status} / {code:?}"
        );

        // The stored plan still carries what the create wrote.
        let described = emulator
            .ok("DescribeScalingPlans", json!({ "ScalingPlanNames": ["my-plan"] }))
            .await;
        assert_eq!(
            described["ScalingPlans"][0].get("ScalingInstructions"),
            Some(&json!(simple_instructions())),
            "case {label:?}: a refused update must leave the stored instructions alone, \
             got {described}"
        );
    }
}

#[tokio::test]
async fn an_applicable_instruction_list_is_created_and_read_back() {
    for (label, instructions) in accepted_instruction_lists() {
        let emulator = Emulator::new();
        let created = emulator
            .ok(
                "CreateScalingPlan",
                json!({
                    "ScalingPlanName": "my-plan",
                    "ApplicationSource": stack_source(1),
                    "ScalingInstructions": instructions,
                }),
            )
            .await;
        assert_eq!(
            created.get("ScalingPlanVersion"),
            Some(&json!(1)),
            "case {label:?}: expected version 1, got {created}"
        );

        // The round trip: what a create accepted is what a read reports.
        let described = emulator
            .ok("DescribeScalingPlans", json!({ "ScalingPlanNames": ["my-plan"] }))
            .await;
        let plan = &described["ScalingPlans"][0];
        assert_eq!(
            plan.get("ScalingInstructions"),
            Some(&instructions),
            "case {label:?}: the instructions did not round-trip, got {plan}"
        );

        // One resource per instruction, through the operation that reports them.
        let expected_resources = instructions.as_array().map_or(0, Vec::len);
        let resources = emulator
            .ok(
                "DescribeScalingPlanResources",
                json!({ "ScalingPlanName": "my-plan", "ScalingPlanVersion": 1 }),
            )
            .await;
        assert_eq!(
            resources["ScalingPlanResources"].as_array().map(Vec::len),
            Some(expected_resources),
            "case {label:?}: expected one resource per instruction, got {resources}"
        );
    }
}

#[tokio::test]
async fn create_refuses_an_application_source_that_names_no_stack() {
    // A CloudFormation stack is named by its ARN:
    // `arn:<partition>:cloudformation:<region>:<account>:stack/<name>/<id>`. Rows span
    // the ways a value can fail to be one.
    for (label, arn) in [
        ("not an arn at all", "not-an-arn"),
        ("an arn with too few fields", "arn:aws:cloudformation:us-east-1"),
        (
            "an arn of another service",
            "arn:aws:s3:us-east-1:000000000000:stack/my-stack/abc",
        ),
        (
            "an arn with no partition",
            "arn::cloudformation:us-east-1:000000000000:stack/my-stack/abc",
        ),
        (
            "an arn with no region",
            "arn:aws:cloudformation::000000000000:stack/my-stack/abc",
        ),
        (
            "an arn with no account",
            "arn:aws:cloudformation:us-east-1::stack/my-stack/abc",
        ),
        (
            "an arn naming another resource type",
            "arn:aws:cloudformation:us-east-1:000000000000:changeSet/my-change-set",
        ),
        (
            "an arn naming no stack",
            "arn:aws:cloudformation:us-east-1:000000000000:stack/",
        ),
    ] {
        let emulator = Emulator::new();
        let (status, code) = emulator
            .err(
                "CreateScalingPlan",
                json!({
                    "ScalingPlanName": "my-plan",
                    "ApplicationSource": { "CloudFormationStackARN": arn },
                    "ScalingInstructions": simple_instructions(),
                }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {label:?} ({arn}): expected HTTP 400 ValidationException, got {status} / \
             {code:?}"
        );
    }

    // Well-formed stack ARNs, differing in partition, region, account and stack id.
    for (label, arn) in [
        (
            "the commercial partition",
            "arn:aws:cloudformation:us-east-1:000000000000:stack/my-stack/abc",
        ),
        (
            "another partition and region",
            "arn:aws-cn:cloudformation:cn-north-1:111122223333:stack/other-stack/def",
        ),
        (
            "a stack with no trailing id",
            "arn:aws-us-gov:cloudformation:us-gov-west-1:444455556666:stack/bare",
        ),
    ] {
        let emulator = Emulator::new();
        let created = emulator
            .ok(
                "CreateScalingPlan",
                json!({
                    "ScalingPlanName": "my-plan",
                    "ApplicationSource": { "CloudFormationStackARN": arn },
                    "ScalingInstructions": simple_instructions(),
                }),
            )
            .await;
        assert_eq!(
            created.get("ScalingPlanVersion"),
            Some(&json!(1)),
            "case {label:?} ({arn}): expected the plan to be created, got {created}"
        );
    }
}

#[tokio::test]
async fn describe_scaling_plan_resources_reports_an_empty_set_for_a_lookup_that_finds_no_plan() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": "known",
                "ApplicationSource": stack_source(1),
                "ScalingInstructions": simple_instructions(),
            }),
        )
        .await;
    emulator.make_all_due();

    // Rows: the ways a (name, version) lookup can miss — a name nothing ever created,
    // a name whose case differs, and a name that was created and then deleted — plus a
    // bounded page size on the miss, which changes nothing: there is no page to size. The
    // VERSION half of the same coordinate misses the same way, over a table of versions in
    // [`crate::plan_version_selects_a_plan`], so every row here holds the version a created
    // plan carries.
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": "gone",
                "ApplicationSource": stack_source(2),
                "ScalingInstructions": simple_instructions(),
            }),
        )
        .await;
    emulator.make_all_due();
    emulator
        .ok(
            "DeleteScalingPlan",
            json!({ "ScalingPlanName": "gone", "ScalingPlanVersion": 1 }),
        )
        .await;
    emulator.make_all_due();

    for (label, name, version, request) in [
        (
            "a name nothing created",
            "missing",
            1,
            json!({ "ScalingPlanName": "missing", "ScalingPlanVersion": 1 }),
        ),
        (
            "a name differing only in case",
            "KNOWN",
            1,
            json!({ "ScalingPlanName": "KNOWN", "ScalingPlanVersion": 1 }),
        ),
        (
            "a deleted plan",
            "gone",
            1,
            json!({ "ScalingPlanName": "gone", "ScalingPlanVersion": 1 }),
        ),
        (
            "a miss with an explicit page size",
            "missing",
            1,
            json!({ "ScalingPlanName": "missing", "ScalingPlanVersion": 1, "MaxResults": 1 }),
        ),
    ] {
        let (status, body) = emulator
            .call("DescribeScalingPlanResources", &request)
            .await;
        assert_eq!(
            status, 200,
            "case {label:?} ({name}, {version}): expected HTTP 200, got {status} with {body}"
        );
        assert_eq!(
            body["ScalingPlanResources"],
            json!([]),
            "case {label:?} ({name}, {version}): expected an empty ScalingPlanResources \
             list, got {body}"
        );
        assert!(
            body.get("NextToken").is_none_or(Value::is_null),
            "case {label:?} ({name}, {version}): an empty page continues nowhere, got {body}"
        );
    }

    // The plan that IS held still reports its own resources, so the empty answer is about
    // the lookup and not about the operation.
    let described = emulator
        .ok(
            "DescribeScalingPlanResources",
            json!({ "ScalingPlanName": "known", "ScalingPlanVersion": 1 }),
        )
        .await;
    assert_eq!(
        described["ScalingPlanResources"].as_array().map(Vec::len),
        Some(1),
        "the stored plan must still report its one resource, got {described}"
    );
}
