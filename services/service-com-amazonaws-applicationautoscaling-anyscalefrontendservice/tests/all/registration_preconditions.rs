// SPDX-License-Identifier: CC0-1.0

//! The preconditions this service applies to a registration and to a dimension-filtered
//! read, each over the SET of inputs its rule holds for.
//!
//! Four rules, each asserted over a table rather than over the one request an issue cited:
//!
//! * a `Describe*` that filters by `ScalableDimension` names a capacity property of ONE
//!   resource, so "if you specify a scalable dimension, you must also specify a resource ID":
//!   every one of the four reads refuses a dimension filter sent with no resource id, and
//!   serves the same filter once a resource id accompanies it;
//! * `Tags` tags a scalable target "when it is created", so a registration that carries tags
//!   for a target that ALREADY exists is refused and applies none of its other members —
//!   the registered window is left exactly as it was;
//! * `RoleARN` "is required for services that do not support service-linked roles ... if the
//!   service supports service-linked roles, Application Auto Scaling uses a service-linked
//!   role", so a target of a namespace that HAS one reports that role whatever role the
//!   request names;
//! * a predefined metric's target value must lie in the window its owning service publishes,
//!   and a value outside it is refused with the sentence naming the metric, both bounds and
//!   the value offered.

use serde_json::{json, Value};

use super::emulator::{error_code, error_message, Emulator, ACCOUNT};

/// The tables the rows of this suite scale.
const TABLES: &[&str] = &["orders", "events"];

/// Every `Describe*` of this service that takes a `ScalableDimension` filter, with the member
/// each one names the resource by: `DescribeScalableTargets` narrows by a LIST
/// (`ResourceIds`), its three siblings by one `ResourceId`.
const DIMENSION_FILTERED_READS: &[(&str, &str)] = &[
    ("DescribeScalableTargets", "ResourceIds"),
    ("DescribeScalingPolicies", "ResourceId"),
    ("DescribeScalingActivities", "ResourceId"),
    ("DescribeScheduledActions", "ResourceId"),
];

/// RULE: a dimension filter with no resource id is refused by every read that takes one, and
/// the same filter is served once a resource id accompanies it.
#[tokio::test]
async fn a_dimension_filter_needs_the_resource_it_narrows() {
    let emulator = Emulator::with_tables(TABLES).await;
    let dimension = "dynamodb:table:ReadCapacityUnits";
    let resource_id = "table/orders";
    let (status, body) = emulator
        .call(
            "RegisterScalableTarget",
            json!({
                "ServiceNamespace": "dynamodb",
                "ResourceId": resource_id,
                "ScalableDimension": dimension,
                "MinCapacity": 1,
                "MaxCapacity": 10,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"the target under test\": expected 200, got {status} {body:?}"
    );

    for (operation, member) in DIMENSION_FILTERED_READS {
        let case = format!("{operation} with a bare dimension filter");
        let (status, body) = emulator
            .call(
                operation,
                json!({"ServiceNamespace": "dynamodb", "ScalableDimension": dimension}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body:?}"
        );
        // The sentence names the member the operation reads the resource by: the plural one
        // for the read that narrows by a list, the singular for its siblings.
        let expected = if *member == "ResourceIds" {
            "Scalable dimension cannot be provided without resource IDs."
        } else {
            "Scalable dimension cannot be provided without a resource ID."
        };
        let message = error_message(&body);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );

        // The same filter, WITH the resource it narrows: served.
        let narrowed = if *member == "ResourceIds" {
            json!({
                "ServiceNamespace": "dynamodb",
                "ResourceIds": [resource_id],
                "ScalableDimension": dimension,
            })
        } else {
            json!({
                "ServiceNamespace": "dynamodb",
                "ResourceId": resource_id,
                "ScalableDimension": dimension,
            })
        };
        let case = format!("{operation} with the resource the dimension narrows");
        let (status, body) = emulator.call(operation, narrowed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the narrowed filter to be served, got {status} {body:?}"
        );
    }
}

/// One registration that carries tags for a target that already exists.
struct TaggedUpsert {
    case: &'static str,
    /// The members the second registration carries beside the identity and the tags.
    patch: fn() -> Value,
}

/// RULE: `Tags` is create-time only, so a registration carrying tags for a target that
/// already exists is refused and applies nothing else it names.
///
/// The rows differ along what else the refused call tried to change: each bound, both, the
/// suspended state, and nothing at all.
#[tokio::test]
async fn tags_on_an_already_registered_target_are_refused_and_apply_nothing() {
    let rows: &[TaggedUpsert] = &[
        TaggedUpsert {
            case: "new bounds",
            patch: || json!({"MinCapacity": 4, "MaxCapacity": 7}),
        },
        TaggedUpsert {
            case: "a new minimum alone",
            patch: || json!({"MinCapacity": 3}),
        },
        TaggedUpsert {
            case: "a new maximum alone",
            patch: || json!({"MaxCapacity": 90}),
        },
        TaggedUpsert {
            case: "a suspended state",
            patch: || json!({"SuspendedState": {"DynamicScalingInSuspended": true}}),
        },
        TaggedUpsert {
            case: "the identity and the tags alone",
            patch: || json!({}),
        },
    ];

    for row in rows {
        let emulator = Emulator::with_tables(TABLES).await;
        let case = row.case;
        let identity = json!({
            "ServiceNamespace": "dynamodb",
            "ResourceId": "table/orders",
            "ScalableDimension": "dynamodb:table:WriteCapacityUnits",
        });
        let mut first = identity.clone();
        first["MinCapacity"] = json!(1);
        first["MaxCapacity"] = json!(10);
        let (status, body) = emulator.call("RegisterScalableTarget", first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first registration to answer 200, got {status} {body:?}"
        );

        let mut tagged = identity.clone();
        for (member, value) in (row.patch)().as_object().expect("the patch is an object") {
            tagged[member] = value.clone();
        }
        tagged["Tags"] = json!({"team": "beta"});
        let (status, body) = emulator.call("RegisterScalableTarget", tagged).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body:?}"
        );
        let message = error_message(&body);
        assert_eq!(
            message,
            "The scalable target that you tried to tag already exists. To update tags on an \
             existing scalable target, use the TagResource API.",
            "case {case:?}: expected the service's own sentence, got {message:?}"
        );

        // The refused call applied NOTHING: the window is the one the first registration
        // left, and the target carries no tag.
        let (status, described) = emulator
            .call(
                "DescribeScalableTargets",
                json!({
                    "ServiceNamespace": "dynamodb",
                    "ResourceIds": ["table/orders"],
                    "ScalableDimension": "dynamodb:table:WriteCapacityUnits",
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeScalableTargets to answer 200, got {status} \
             {described:?}"
        );
        let target = &described["ScalableTargets"][0];
        assert_eq!(
            (target["MinCapacity"].clone(), target["MaxCapacity"].clone()),
            (json!(1), json!(10)),
            "case {case:?}: expected the registered window to survive the refusal, got \
             {target:?}"
        );
        let arn = target["ScalableTargetARN"].as_str().unwrap_or_default();
        let (status, tags) = emulator
            .call("ListTagsForResource", json!({"ResourceARN": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListTagsForResource to answer 200, got {status} {tags:?}"
        );
        let held = tags["Tags"].as_object().cloned().unwrap_or_default();
        assert!(
            held.is_empty(),
            "case {case:?}: expected the refused tagging to attach nothing, got {held:?}"
        );
    }
}

/// The namespaces this service scales through a service-linked role, with the resource-type
/// suffix each one's role name carries.
///
/// Source: "Service-linked roles for Application Auto Scaling" in the Application Auto
/// Scaling User Guide, one row per supported service. The rows below are the ones whose
/// resource this suite can name, which includes namespaces the issue did not cite.
const SERVICE_LINKED_ROLES: &[(&str, &str, &str)] = &[
    ("dynamodb", "dynamodb:table:ReadCapacityUnits", "DynamoDBTable"),
    ("ecs", "ecs:service:DesiredCount", "ECSService"),
    (
        "lambda",
        "lambda:function:ProvisionedConcurrency",
        "LambdaConcurrency",
    ),
    (
        "workspaces",
        "workspaces:workspacespool:DesiredUserSessions",
        "WorkSpacesPool",
    ),
];

/// RULE: a target of a namespace this service scales through a service-linked role reports
/// that role, whatever `RoleARN` the request named.
///
/// The rows differ along the namespace, and each one is registered BOTH with a customer role
/// and with none at all.
#[tokio::test]
async fn a_target_reports_the_service_linked_role_of_its_namespace() {
    for (namespace, dimension, suffix) in SERVICE_LINKED_ROLES {
        for supplied in [
            Some("arn:aws:iam::000000000000:role/aas-custom"),
            None,
        ] {
            let emulator = Emulator::empty();
            let case = format!("{namespace} registered with {supplied:?}");
            let target = emulator.given_target(dimension, 1, 10).await;
            let mut request = target.clone();
            if let Some(role) = supplied {
                request["RoleARN"] = json!(role);
            }
            request["MinCapacity"] = json!(1);
            request["MaxCapacity"] = json!(10);
            let (status, body) = emulator.call("RegisterScalableTarget", request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the registration to answer 200, got {status} {body:?}"
            );

            let (status, described) = emulator
                .call(
                    "DescribeScalableTargets",
                    json!({
                        "ServiceNamespace": namespace,
                        "ResourceIds": [target["ResourceId"].clone()],
                        "ScalableDimension": dimension,
                    }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected DescribeScalableTargets to answer 200, got {status} \
                 {described:?}"
            );
            let expected = format!(
                "arn:aws:iam::{ACCOUNT}:role/aws-service-role/\
                 {namespace}.application-autoscaling.amazonaws.com/\
                 AWSServiceRoleForApplicationAutoScaling_{suffix}"
            );
            let reported = described["ScalableTargets"][0]["RoleARN"].clone();
            assert_eq!(
                reported,
                json!(expected),
                "case {case:?}: expected RoleARN {expected:?}, got {reported:?}"
            );
        }
    }
}

/// The utilisation metrics whose target value this service bounds, and the window each one
/// publishes.
///
/// Source: the predefined-metric catalog this service scales on
/// (`crate::state::PREDEFINED_METRICS`, whose rows carry the window their owning service
/// publishes): the provisioned-throughput utilisation metrics share one window, and Lambda's
/// provisioned-concurrency utilisation — a FRACTION rather than a percentage — publishes its
/// own.
const TARGET_VALUE_WINDOWS: &[(&str, &str, f64, f64)] = &[
    (
        "dynamodb:table:ReadCapacityUnits",
        "DynamoDBReadCapacityUtilization",
        10.0,
        90.0,
    ),
    (
        "dynamodb:table:WriteCapacityUnits",
        "DynamoDBWriteCapacityUtilization",
        10.0,
        90.0,
    ),
    (
        "lambda:function:ProvisionedConcurrency",
        "LambdaProvisionedConcurrencyUtilization",
        0.1,
        0.9,
    ),
];

/// RULE: a target value inside the metric's published window is accepted — both bounds
/// included — and one outside it is refused with the sentence naming the metric type, both
/// bounds and the value offered.
///
/// Each metric is tested on both bounds, a value between them, and one just past each bound.
#[tokio::test]
async fn a_target_value_outside_the_metrics_window_is_refused_naming_both_bounds() {
    for (dimension, metric, low, high) in TARGET_VALUE_WINDOWS {
        let middle = (low + high) / 2.0;
        // (the offered value, whether it lies in the published window)
        let offers: &[(f64, bool)] = &[
            (*low, true),
            (middle, true),
            (*high, true),
            (low - low / 10.0, false),
            (high + high / 10.0, false),
        ];
        for (offered, inside) in offers {
            // One emulator per offer: a target carries at most one target-tracking policy
            // per metric specification, so each row is put on a target of its own.
            let emulator = Emulator::empty();
            let target = emulator.given_target(dimension, 1, 10).await;
            let case = format!("{metric} at {offered}");
            let mut request = target.clone();
            request["PolicyName"] = json!("track");
            request["PolicyType"] = json!("TargetTrackingScaling");
            request["TargetTrackingScalingPolicyConfiguration"] = json!({
                "TargetValue": offered,
                "PredefinedMetricSpecification": {"PredefinedMetricType": metric},
            });
            let (status, body) = emulator.call("PutScalingPolicy", request).await;
            if *inside {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected a value inside the window to be accepted, got \
                     {status} {body:?}"
                );
                continue;
            }
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body:?}"
            );
            let expected = format!(
                "For predefined metric type {metric}, target value must be between '{low:?}' \
                 and '{high:?}', but was '{offered:?}'."
            );
            let message = error_message(&body);
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }
}
