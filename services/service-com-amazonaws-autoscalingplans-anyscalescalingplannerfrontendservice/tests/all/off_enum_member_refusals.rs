// SPDX-License-Identifier: CC0-1.0

//! The refusal a value outside a modelled ENUM's value set earns, wherever such a member
//! is carried.
//!
//! ONE rule is pinned here, over a TABLE spanning every enum member of every request shape
//! this service has rather than the single member that raised the question: a value the
//! member's own enum does not list is refused with HTTP 400 `ValidationException` carrying
//! the constraint-violation sentence — `1 validation error detected: Value '<value>' at
//! '<member path>' failed to satisfy constraint: Member must satisfy enum value set:
//! [<the values the model lists>]` — whichever operation received it, at whatever depth
//! the member sits, and whatever ELSE the same request gets wrong about that member. A
//! narrower rule about one of those members (a triple whose dimension is owned by another
//! service namespace describes no scalable resource; a customized load metric's statistic
//! has a single valid value) speaks about a value the enum DOES list, so it can never
//! answer for a value the enum does not list: otherwise one and the same off-enum value
//! would be refused with two different sentences depending on which operation received it.
//!
//! The rows differ along what the rule generalises over: the FORM of the miss (nothing at
//! all, a plausible unmodelled name, a listed value with a character added, with trailing
//! space, in the wrong case, a value from another member's enum), the OPERATION that
//! receives it, the member's DEPTH (a top-level member, a member of a nested structure, a
//! member of a structure inside a list) and the list POSITION the member sits at.
//!
//! The second test pins the other side of the same rule: values the enums DO list keep the
//! narrower sentences those rules own — the unsupported-resource sentence for a triple
//! that describes no one scalable resource, and the single-valid-value sentence for a load
//! metric statistic — so the enum-set sentence is reported for an off-enum value and for
//! nothing else.
//!
//! The harness (router, dispatch, error-body readers) is reused from
//! [`crate::scaling_plan_lifecycle`]; nothing here builds a second one.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::get_scaling_plan_resource_forecast_data::{
    FORECAST_SCALABLE_DIMENSION, FORECAST_SERVICE_NAMESPACE,
};
use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::types::{
    FORECAST_DATA_TYPE_VALUES, LOAD_METRIC_TYPE_VALUES, METRIC_STATISTIC_VALUES,
    PREDICTIVE_SCALING_MAX_CAPACITY_BEHAVIOR_VALUES, PREDICTIVE_SCALING_MODE_VALUES,
    SCALABLE_DIMENSION_VALUES, SCALING_METRIC_TYPE_VALUES,
    SCALING_POLICY_UPDATE_BEHAVIOR_VALUES, SERVICE_NAMESPACE_VALUES,
};
use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::state::UNSUPPORTED_RESOURCE;

use crate::scaling_plan_lifecycle::{
    error_code, error_message, now_secs, simple_instructions, stack_source, Emulator,
};

/// The plan every request below names, so no row is refused for naming no plan.
const STORED_PLAN: &str = "enum probe plan";

/// A plan name no row stores, for the create rows (which must not collide with the one
/// plan this suite holds).
const NEW_PLAN: &str = "another plan";

/// The only scaling-plan version the model documents as valid.
const ONLY_VERSION: i64 = 1;

/// The Auto Scaling group [`simple_instructions`] instructs, which is also the resource
/// the forecast rows name. `ResourceId` "consists of a prefix (autoScalingGroup) followed
/// by the name of a specified Auto Scaling group (my-asg)".
const FORECAST_RESOURCE_ID: &str = "autoScalingGroup/my-asg";

/// The forecast read, whose resource coordinates are themselves modelled enums.
const FORECAST_OPERATION: &str = "GetScalingPlanResourceForecastData";

/// The two operations that take a `ScalingInstructions` list, so every instruction member
/// below is driven through both.
const WRITE_OPERATIONS: [&str; 2] = ["CreateScalingPlan", "UpdateScalingPlan"];

/// Every modelled enum value set a REQUEST member of this service targets, so a row can
/// borrow a value that belongs to a member's enum other than its own.
///
/// Source: `src/ops/types/shared_types.rs`, which mirrors the model's own value lists
/// (`smithy_query(query="shape",shape="ServiceNamespace,ScalableDimension,
/// ForecastDataType,MetricStatistic,LoadMetricType,ScalingMetricType,PredictiveScalingMode,
/// PredictiveScalingMaxCapacityBehavior,ScalingPolicyUpdateBehavior")`).
const EVERY_VALUE_SET: [&[&str]; 9] = [
    SERVICE_NAMESPACE_VALUES,
    SCALABLE_DIMENSION_VALUES,
    FORECAST_DATA_TYPE_VALUES,
    METRIC_STATISTIC_VALUES,
    LOAD_METRIC_TYPE_VALUES,
    SCALING_METRIC_TYPE_VALUES,
    PREDICTIVE_SCALING_MODE_VALUES,
    PREDICTIVE_SCALING_MAX_CAPACITY_BEHAVIOR_VALUES,
    SCALING_POLICY_UPDATE_BEHAVIOR_VALUES,
];

/// The fixed text the sentence opens with, for `value` at `path`.
fn refusal_opening(path: &str, value: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{path}' failed to satisfy \
         constraint: Member must satisfy enum value set: ["
    )
}

/// `value` with the case of every character swapped, so a row can miss an enum by case
/// alone whatever case that enum's own values are written in.
fn flip_case(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_uppercase() {
                character.to_lowercase().collect::<String>()
            } else {
                character.to_uppercase().collect::<String>()
            }
        })
        .collect()
}

/// A value some OTHER member's enum lists and this one does not.
fn foreign_value(allowed: &[&str]) -> String {
    EVERY_VALUE_SET
        .iter()
        .flat_map(|set| set.iter())
        .find(|value| !allowed.contains(*value))
        .map(|value| (*value).to_string())
        .unwrap_or_else(|| "bogus".to_string())
}

/// How a value can miss an enum's value set: the rows every member below is driven over.
///
/// Any probe the member's own set happens to list is dropped, so no row asserts a refusal
/// for a value the model accepts.
fn off_enum_probes(allowed: &[&str]) -> Vec<(&'static str, String)> {
    let first = allowed.first().copied().unwrap_or_default();
    let mut probes = vec![
        ("an empty value", String::new()),
        ("a plausible unmodelled name", "bogus".to_string()),
        (
            "a listed value with a trailing character",
            format!("{first}X"),
        ),
        ("a listed value with a trailing space", format!("{first} ")),
        ("a case-flipped listed value", flip_case(first)),
        ("a value from another member's enum", foreign_value(allowed)),
    ];
    probes.retain(|(_, value)| !allowed.contains(&value.as_str()));
    probes
}

/// A `GetScalingPlanResourceForecastData` request with every required member, naming the
/// resource the stored plan covers over a window well inside the documented limits, so
/// only the member a row varies can refuse it.
fn forecast_request(namespace: &str, dimension: &str, data_type: &str) -> Value {
    forecast_request_for(namespace, FORECAST_RESOURCE_ID, dimension, data_type)
}

/// The same request, with the resource id under the row's control too.
fn forecast_request_for(
    namespace: &str,
    resource_id: &str,
    dimension: &str,
    data_type: &str,
) -> Value {
    let start = now_secs();
    json!({
        "ScalingPlanName": STORED_PLAN,
        "ScalingPlanVersion": ONLY_VERSION,
        "ServiceNamespace": namespace,
        "ResourceId": resource_id,
        "ScalableDimension": dimension,
        "ForecastDataType": data_type,
        "StartTime": start,
        "EndTime": start + 3_600,
    })
}

/// The enum members the forecast read carries: the member path the refusal names it by,
/// the value set the model lists, and the request carrying a given value on that member.
#[allow(clippy::type_complexity)]
const FORECAST_MEMBERS: [(&str, &[&str], fn(&str) -> Value); 3] = [
    ("serviceNamespace", SERVICE_NAMESPACE_VALUES, |value| {
        forecast_request(value, FORECAST_SCALABLE_DIMENSION, "CapacityForecast")
    }),
    ("scalableDimension", SCALABLE_DIMENSION_VALUES, |value| {
        forecast_request(FORECAST_SERVICE_NAMESPACE, value, "CapacityForecast")
    }),
    ("forecastDataType", FORECAST_DATA_TYPE_VALUES, |value| {
        forecast_request(
            FORECAST_SERVICE_NAMESPACE,
            FORECAST_SCALABLE_DIMENSION,
            value,
        )
    }),
];

/// The valid one-instruction list both write paths accept, with `member` replaced.
fn instruction_with(member: &str, value: Value) -> Value {
    let mut instruction = simple_instructions().remove(0);
    instruction[member] = value;
    json!([instruction])
}

/// A target tracking configuration naming a customized metric whose statistic is `value`.
fn customized_metric_configuration(value: &str) -> Value {
    json!({
        "CustomizedScalingMetricSpecification": {
            "MetricName": "my-metric",
            "Namespace": "MyApp",
            "Statistic": value,
        },
        "TargetValue": 40.0,
    })
}

/// The enum members a `ScalingInstruction` carries, directly or inside a metric
/// specification it nests: the member path (under the list element) the refusal names it
/// by, the value set the model lists, and the instruction list carrying a given value
/// there. The list indices AWS reports are 1-based, so the single instruction below is
/// element `1`.
#[allow(clippy::type_complexity)]
const INSTRUCTION_MEMBERS: [(&str, &[&str], fn(&str) -> Value); 9] = [
    ("serviceNamespace", SERVICE_NAMESPACE_VALUES, |value| {
        instruction_with("ServiceNamespace", json!(value))
    }),
    ("scalableDimension", SCALABLE_DIMENSION_VALUES, |value| {
        instruction_with("ScalableDimension", json!(value))
    }),
    (
        "scalingPolicyUpdateBehavior",
        SCALING_POLICY_UPDATE_BEHAVIOR_VALUES,
        |value| instruction_with("ScalingPolicyUpdateBehavior", json!(value)),
    ),
    (
        "predictiveScalingMode",
        PREDICTIVE_SCALING_MODE_VALUES,
        |value| instruction_with("PredictiveScalingMode", json!(value)),
    ),
    (
        "predictiveScalingMaxCapacityBehavior",
        PREDICTIVE_SCALING_MAX_CAPACITY_BEHAVIOR_VALUES,
        |value| instruction_with("PredictiveScalingMaxCapacityBehavior", json!(value)),
    ),
    (
        "predefinedLoadMetricSpecification.predefinedLoadMetricType",
        LOAD_METRIC_TYPE_VALUES,
        |value| {
            instruction_with(
                "PredefinedLoadMetricSpecification",
                json!({ "PredefinedLoadMetricType": value }),
            )
        },
    ),
    (
        "customizedLoadMetricSpecification.statistic",
        METRIC_STATISTIC_VALUES,
        |value| {
            instruction_with(
                "CustomizedLoadMetricSpecification",
                json!({ "MetricName": "my-load", "Namespace": "MyApp", "Statistic": value }),
            )
        },
    ),
    (
        "targetTrackingConfigurations.1.member.predefinedScalingMetricSpecification.\
         predefinedScalingMetricType",
        SCALING_METRIC_TYPE_VALUES,
        |value| {
            instruction_with(
                "TargetTrackingConfigurations",
                json!([{
                    "PredefinedScalingMetricSpecification": {
                        "PredefinedScalingMetricType": value
                    },
                    "TargetValue": 40.0,
                }]),
            )
        },
    ),
    (
        "targetTrackingConfigurations.2.member.customizedScalingMetricSpecification.\
         statistic",
        METRIC_STATISTIC_VALUES,
        |value| {
            // The off-enum member sits in the SECOND configuration, so the path the
            // refusal reports has to follow the element's position in the list.
            instruction_with(
                "TargetTrackingConfigurations",
                json!([
                    {
                        "PredefinedScalingMetricSpecification": {
                            "PredefinedScalingMetricType": "ASGAverageCPUUtilization"
                        },
                        "TargetValue": 40.0,
                    },
                    customized_metric_configuration(value),
                ]),
            )
        },
    ),
];

/// One write request carrying `instructions`, in the envelope `operation` requires.
fn write_request(operation: &str, instructions: Value) -> Value {
    if operation == "CreateScalingPlan" {
        return json!({
            "ScalingPlanName": NEW_PLAN,
            "ApplicationSource": stack_source(2),
            "ScalingInstructions": instructions,
        });
    }
    json!({
        "ScalingPlanName": STORED_PLAN,
        "ScalingPlanVersion": ONLY_VERSION,
        "ScalingInstructions": instructions,
    })
}

async fn with_one_stored_plan() -> Emulator {
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

/// Assert one request is refused with the enum-set sentence for `path`: the fixed text
/// exactly, the rejected value as the request carried it, and the values the model lists.
async fn assert_off_enum_refusal(
    emulator: &Emulator,
    case: &str,
    operation: &str,
    request: &Value,
    path: &str,
    value: &str,
    allowed: &[&str],
) {
    let (status, body) = emulator.call(operation, request).await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "ValidationException"),
        "case {case:?}: expected HTTP 400 ValidationException, got {status} with {body}"
    );
    let actual = error_message(&body);
    let opening = refusal_opening(path, value);
    assert!(
        actual.starts_with(&opening) && actual.ends_with(']'),
        "case {case:?}: expected a sentence opening {opening:?} and closing with ']', got \
         {actual:?}"
    );
    let listed: BTreeSet<&str> = actual[opening.len()..actual.len() - 1]
        .split(", ")
        .collect();
    let expected: BTreeSet<&str> = allowed.iter().copied().collect();
    assert_eq!(
        listed, expected,
        "case {case:?}: the sentence must list the values the model lists, got {actual:?}"
    );
}

#[tokio::test]
async fn every_enum_member_refuses_an_off_enum_value_with_the_one_sentence() {
    let emulator = with_one_stored_plan().await;

    for (path, allowed, build) in FORECAST_MEMBERS {
        for (miss, value) in off_enum_probes(allowed) {
            let request = build(&value);
            let case = format!("{FORECAST_OPERATION} / {path}: {miss}");
            assert_off_enum_refusal(
                &emulator,
                &case,
                FORECAST_OPERATION,
                &request,
                path,
                &value,
                allowed,
            )
            .await;
        }
    }

    // Both write paths hold a scaling instruction to the same value sets, so every
    // instruction member is driven through both.
    for (member, allowed, build) in INSTRUCTION_MEMBERS {
        for (miss, value) in off_enum_probes(allowed) {
            let path = format!("scalingInstructions.1.member.{member}");
            for operation in WRITE_OPERATIONS {
                let request = write_request(operation, build(&value));
                let case = format!("{operation} / {path}: {miss}");
                assert_off_enum_refusal(
                    &emulator, &case, operation, &request, &path, &value, allowed,
                )
                .await;
            }
        }
    }

    // The same member in the SECOND instruction of a two-instruction list: the path the
    // refusal reports follows the element's position, so the sentence names the member
    // that actually carried the off-enum value.
    let instructions = json!([
        simple_instructions().remove(0),
        {
            "ServiceNamespace": "bogus",
            "ResourceId": "service/default/sample-webapp",
            "ScalableDimension": "ecs:service:DesiredCount",
            "MinCapacity": 1,
            "MaxCapacity": 10,
            "TargetTrackingConfigurations": [{
                "PredefinedScalingMetricSpecification": {
                    "PredefinedScalingMetricType": "ECSServiceAverageCPUUtilization"
                },
                "TargetValue": 40.0,
            }],
        },
    ]);
    for operation in WRITE_OPERATIONS {
        let request = write_request(operation, instructions.clone());
        assert_off_enum_refusal(
            &emulator,
            &format!("{operation} / the second instruction's namespace"),
            operation,
            &request,
            "scalingInstructions.2.member.serviceNamespace",
            "bogus",
            SERVICE_NAMESPACE_VALUES,
        )
        .await;
    }
}

#[tokio::test]
async fn the_enum_set_refusal_outranks_the_narrower_rules_about_the_same_member() {
    let emulator = with_one_stored_plan().await;

    // An off-enum value on the forecast read's resource coordinates, beside a SECOND thing
    // the same request gets wrong about that resource — a resource id without the
    // documented prefix, or coordinates belonging to two different services. The off-enum
    // member is still what the refusal is about: a value the enum does not list is not a
    // resource triple at all, so the sentence for a triple that names no scalable resource
    // cannot answer for it.
    let combined: [(&str, Value, &str, &str, &[&str]); 4] = [
        (
            "an off-enum namespace beside a prefixless resource id",
            forecast_request_for("bogus", "my-asg", FORECAST_SCALABLE_DIMENSION, "LoadForecast"),
            "serviceNamespace",
            "bogus",
            SERVICE_NAMESPACE_VALUES,
        ),
        (
            "an off-enum namespace beside a dimension of another service",
            forecast_request(
                "bogus",
                "dynamodb:table:ReadCapacityUnits",
                "CapacityForecast",
            ),
            "serviceNamespace",
            "bogus",
            SERVICE_NAMESPACE_VALUES,
        ),
        (
            "an off-enum dimension beside a prefixless resource id",
            forecast_request_for(
                FORECAST_SERVICE_NAMESPACE,
                "my-asg",
                "autoscaling:autoScalingGroup:Nonsense",
                "CapacityForecast",
            ),
            "scalableDimension",
            "autoscaling:autoScalingGroup:Nonsense",
            SCALABLE_DIMENSION_VALUES,
        ),
        (
            "an off-enum dimension under another listed namespace",
            forecast_request("dynamodb", "dynamodb:table:Nonsense", "CapacityForecast"),
            "scalableDimension",
            "dynamodb:table:Nonsense",
            SCALABLE_DIMENSION_VALUES,
        ),
    ];
    for (case, request, path, value, allowed) in combined {
        assert_off_enum_refusal(
            &emulator,
            case,
            FORECAST_OPERATION,
            &request,
            path,
            value,
            allowed,
        )
        .await;
    }

    // The other side of the same rule: values the enums DO list keep the narrower
    // sentences those rules own — the unsupported-resource sentence for a triple that
    // describes no one scalable resource (a dimension under a namespace that does not own
    // it, or a resource id naming no resource of the dimension's kind) — so the enum-set
    // sentence is reported for an off-enum value and for nothing else. Rows differ in
    // which half of the triple contradicts the other and in the resource id's form.
    let unsupported: [(&str, Value); 4] = [
        (
            "a DynamoDB dimension under the ECS namespace",
            forecast_request_for(
                "ecs",
                "table/my-table",
                "dynamodb:table:ReadCapacityUnits",
                "CapacityForecast",
            ),
        ),
        (
            "an ECS dimension under the DynamoDB namespace",
            forecast_request_for(
                "dynamodb",
                "service/default/sample-webapp",
                "ecs:service:DesiredCount",
                "CapacityForecast",
            ),
        ),
        (
            "the forecast dimension under another listed namespace",
            forecast_request("rds", FORECAST_SCALABLE_DIMENSION, "CapacityForecast"),
        ),
        (
            "the forecast pair with a resource id lacking the documented prefix",
            forecast_request_for(
                FORECAST_SERVICE_NAMESPACE,
                "my-asg",
                FORECAST_SCALABLE_DIMENSION,
                "CapacityForecast",
            ),
        ),
    ];
    for (case, request) in unsupported {
        let (status, body) = emulator.call(FORECAST_OPERATION, &request).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} with {body}"
        );
        let actual = error_message(&body);
        assert_eq!(
            actual, UNSUPPORTED_RESOURCE,
            "case {case:?}: expected {UNSUPPORTED_RESOURCE:?}, got {actual:?}"
        );
    }

    // And a statistic the enum lists but a customized LOAD metric does not accept ("The
    // only valid value is Sum") keeps its own narrower sentence: the enum-set sentence is
    // reported for a value the enum does not list, and for nothing else.
    for statistic in METRIC_STATISTIC_VALUES
        .iter()
        .filter(|value| **value != "Sum")
    {
        let instructions = instruction_with(
            "CustomizedLoadMetricSpecification",
            json!({ "MetricName": "my-load", "Namespace": "MyApp", "Statistic": statistic }),
        );
        let request = write_request("UpdateScalingPlan", instructions);
        let (status, body) = emulator.call("UpdateScalingPlan", &request).await;
        let actual = error_message(&body);
        assert_eq!(
            (status, error_code(&body)),
            (400, "ValidationException"),
            "case a listed statistic {statistic:?} a load metric does not accept: expected \
             HTTP 400 ValidationException, got {status} with {body}"
        );
        assert!(
            !actual.contains("enum value set"),
            "case a listed statistic {statistic:?} a load metric does not accept: a value \
             the enum LISTS must not earn the enum-set sentence, got {actual:?}"
        );
    }
}
