// SPDX-License-Identifier: CC0-1.0

//! The scalable-target + scaling-policy lifecycle, end to end over the wire.
//!
//! Every case drives the production `MultiServiceRouter` with a buffered awsJson1_1
//! request (`POST /`, `X-Amz-Target: AnyScaleFrontendService.<Operation>`), so what is
//! asserted is what a signing AWS client would see — routing, deserialization,
//! constraint validation, the handler and serialization included.
//!
//! The suites are tables rather than single calls: each row varies the thing the rule
//! under test generalises over (the namespace/dimension pair, which members a partial
//! update names, the policy type, the page size), and the case the bootstrap issue cited
//! is one row among them.
//!
//! The emulator itself, and the helpers every suite in this target shares, live in
//! `tests/all/emulator.rs`.

use serde_json::{json, Value};

use super::emulator::{error_code, service_linked_role_suffix, Emulator, ACCOUNT, REGION};

/// The DynamoDB tables this suite's `dynamodb` rows register a scalable target on.
///
/// A scalable target is a capacity dimension OF A RESOURCE, so registering one resolves
/// that resource in the service that owns it: these exist before the targets do.
const TABLES: &[&str] = &[
    "my-table", "alpha", "bravo", "charlie", "delta", "echo", "row-a", "row-b", "row-c",
    "row-d", "row-e", "row-f", "row-g", "row-h",
];

/// The single `ScalableTarget` a namespace holds, for a case that registered exactly one.
async fn only_target(emulator: &Emulator, namespace: &str, case: &str) -> Value {
    let (status, body) = emulator
        .call(
            "DescribeScalableTargets",
            json!({"ServiceNamespace": namespace}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeScalableTargets to answer 200, got {status} {body:?}"
    );
    let targets = body["ScalableTargets"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ScalableTargets list, got {body:?}"));
    assert_eq!(
        targets.len(),
        1,
        "case {case:?}: expected exactly one scalable target, got {targets:?}"
    );
    targets[0].clone()
}

/// A row of the registration catalog.
///
/// The namespace/dimension pairs come from the model's own `ServiceNamespace` and
/// `ScalableDimension` value sets (rendered into `src/ops/types/shared_types.rs` as
/// `SERVICE_NAMESPACE_VALUES` / `SCALABLE_DIMENSION_VALUES`). The first row is the case
/// the bootstrap issue cited; the others are entries it did not, including a namespace
/// whose minimum capacity is 0 and one whose window is a single value.
struct TargetRow {
    namespace: &'static str,
    resource_id: &'static str,
    dimension: &'static str,
    min: i64,
    max: i64,
    /// `None` exercises the service-linked-role fallback.
    role: Option<&'static str>,
}

const TARGET_CATALOG: &[TargetRow] = &[
    TargetRow {
        namespace: "dynamodb",
        resource_id: "table/my-table",
        dimension: "dynamodb:table:ReadCapacityUnits",
        min: 1,
        max: 10,
        role: Some("arn:aws:iam::000000000000:role/aas"),
    },
    TargetRow {
        namespace: "ecs",
        resource_id: "service/prod-cluster/web-app",
        dimension: "ecs:service:DesiredCount",
        min: 0,
        max: 100,
        role: None,
    },
    TargetRow {
        namespace: "lambda",
        resource_id: "function:ingest:green",
        dimension: "lambda:function:ProvisionedConcurrency",
        min: 5,
        max: 5,
        role: None,
    },
    TargetRow {
        namespace: "cassandra",
        resource_id: "keyspace/events/table/clicks",
        dimension: "cassandra:table:WriteCapacityUnits",
        min: 2,
        max: 7,
        role: Some("arn:aws:iam::000000000000:role/keyspaces-aas"),
    },
    TargetRow {
        namespace: "workspaces",
        resource_id: "workspacespool/wspool-9f8e7d",
        dimension: "workspaces:workspacespool:DesiredUserSessions",
        min: 3,
        max: 9,
        role: None,
    },
];

/// The predefined metric a scalable dimension's own service publishes for it, for the rows
/// that put a target-tracking policy on a dimension they choose.
fn predefined_metric_of(dimension: &str) -> &'static str {
    match dimension {
        "dynamodb:table:ReadCapacityUnits" | "dynamodb:index:ReadCapacityUnits" => {
            "DynamoDBReadCapacityUtilization"
        }
        "dynamodb:table:WriteCapacityUnits" | "dynamodb:index:WriteCapacityUnits" => {
            "DynamoDBWriteCapacityUtilization"
        }
        other => panic!("case {other:?}: this suite publishes no metric for it"),
    }
}

#[tokio::test]
async fn a_registered_target_is_described_from_what_was_persisted() {
    let emulator = Emulator::with_tables(TABLES).await;

    for row in TARGET_CATALOG {
        let case = format!("{}/{}", row.namespace, row.dimension);
        let mut request = json!({
            "ServiceNamespace": row.namespace,
            "ResourceId": row.resource_id,
            "ScalableDimension": row.dimension,
            "MinCapacity": row.min,
            "MaxCapacity": row.max,
        });
        if let Some(role) = row.role {
            request["RoleARN"] = json!(role);
        }
        // One row also carries create-time tags, to prove they do not disturb the
        // response; `resource_tags.rs` is where the tag surface itself is asserted.
        if row.namespace == "cassandra" {
            request["Tags"] = json!({"team": "search"});
        }

        emulator.given_resource(row.namespace, row.resource_id, row.min).await;
        let (status, body) = emulator.call("RegisterScalableTarget", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected RegisterScalableTarget to answer 200, got {status} {body:?}"
        );
        let arn = body["ScalableTargetARN"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a ScalableTargetARN, got {body:?}"))
            .to_string();
        let expected_prefix =
            format!("arn:aws:application-autoscaling:{REGION}:{ACCOUNT}:scalable-target/");
        assert!(
            arn.starts_with(&expected_prefix),
            "case {case:?}: expected a ScalableTargetARN under {expected_prefix:?}, got {arn:?}"
        );

        let target = only_target(&emulator, row.namespace, &case).await;
        assert_eq!(
            target["ResourceId"],
            json!(row.resource_id),
            "case {case:?}: expected the stored ResourceId, got {target:?}"
        );
        assert_eq!(
            target["ScalableDimension"],
            json!(row.dimension),
            "case {case:?}: expected the stored ScalableDimension, got {target:?}"
        );
        assert_eq!(
            target["MinCapacity"],
            json!(row.min),
            "case {case:?}: expected the stored MinCapacity, got {target:?}"
        );
        assert_eq!(
            target["MaxCapacity"],
            json!(row.max),
            "case {case:?}: expected the stored MaxCapacity, got {target:?}"
        );
        assert_eq!(
            target["ScalableTargetARN"],
            json!(arn),
            "case {case:?}: expected Describe to return the ARN Register minted, got {target:?}"
        );

        // An omitted RoleARN resolves to the service-linked role for the namespace, whose
        // name carries the resource type that namespace scales.
        let expected_role = row.role.map(ToString::to_string).unwrap_or_else(|| {
            format!(
                "arn:aws:iam::{ACCOUNT}:role/aws-service-role/{}.application-autoscaling.amazonaws.com/AWSServiceRoleForApplicationAutoScaling_{}",
                row.namespace,
                service_linked_role_suffix(row.namespace)
            )
        });
        assert_eq!(
            target["RoleARN"],
            json!(expected_role),
            "case {case:?}: expected RoleARN {expected_role:?}, got {target:?}"
        );

        let creation_time = target["CreationTime"].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: expected a numeric CreationTime, got {target:?}")
        });
        assert!(
            creation_time > 0.0,
            "case {case:?}: expected a CreationTime after the epoch, got {creation_time}"
        );

        // A read must not change what the next reader sees.
        let again = only_target(&emulator, row.namespace, &case).await;
        assert_eq!(
            again, target,
            "case {case:?}: expected a second Describe to answer identically, got {again:?}"
        );
    }
}

/// The roles the upsert suite registers with, as consts so a table row's closure stays
/// non-capturing.
const FIRST_ROLE: &str = "arn:aws:iam::000000000000:role/aas-first";
/// See [`FIRST_ROLE`].
const SECOND_ROLE: &str = "arn:aws:iam::000000000000:role/aas-second";

/// A partial update, and the full state it must leave behind.
struct UpdateRow {
    case: &'static str,
    patch: fn() -> Value,
    min: i64,
    max: i64,
    role: &'static str,
    suspended: fn() -> Value,
}

#[tokio::test]
async fn register_updates_only_the_members_the_request_names() {
    let emulator = Emulator::with_tables(TABLES).await;
    let namespace = "dynamodb";
    let resource_id = "table/my-table";
    let dimension = "dynamodb:table:ReadCapacityUnits";
    let first_role = FIRST_ROLE;
    let second_role = SECOND_ROLE;

    let (status, body) = emulator
        .call(
            "RegisterScalableTarget",
            json!({
                "ServiceNamespace": namespace,
                "ResourceId": resource_id,
                "ScalableDimension": dimension,
                "MinCapacity": 1,
                "MaxCapacity": 10,
                "RoleARN": first_role,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"baseline\": expected 200, got {status} {body:?}"
    );
    let baseline = only_target(&emulator, namespace, "baseline").await;
    let baseline_arn = baseline["ScalableTargetARN"].clone();
    let baseline_creation = baseline["CreationTime"].clone();

    let rows: &[UpdateRow] = &[
        UpdateRow {
            case: "MinCapacity only",
            patch: || json!({"MinCapacity": 4}),
            min: 4,
            max: 10,
            role: first_role,
            suspended: || json!({"DynamicScalingInSuspended": false, "DynamicScalingOutSuspended": false, "ScheduledScalingSuspended": false}),
        },
        UpdateRow {
            case: "MaxCapacity only",
            patch: || json!({"MaxCapacity": 40}),
            min: 4,
            max: 40,
            role: first_role,
            suspended: || json!({"DynamicScalingInSuspended": false, "DynamicScalingOutSuspended": false, "ScheduledScalingSuspended": false}),
        },
        UpdateRow {
            case: "RoleARN only",
            patch: || json!({"RoleARN": SECOND_ROLE}),
            min: 4,
            max: 40,
            role: second_role,
            suspended: || json!({"DynamicScalingInSuspended": false, "DynamicScalingOutSuspended": false, "ScheduledScalingSuspended": false}),
        },
        UpdateRow {
            case: "one suspended-scaling attribute",
            patch: || json!({"SuspendedState": {"DynamicScalingInSuspended": true}}),
            min: 4,
            max: 40,
            role: second_role,
            suspended: || json!({"DynamicScalingInSuspended": true, "DynamicScalingOutSuspended": false, "ScheduledScalingSuspended": false}),
        },
        UpdateRow {
            case: "a second suspended-scaling attribute",
            patch: || json!({"SuspendedState": {"ScheduledScalingSuspended": true}}),
            min: 4,
            max: 40,
            role: second_role,
            suspended: || {
                json!({"DynamicScalingInSuspended": true, "DynamicScalingOutSuspended": false, "ScheduledScalingSuspended": true})
            },
        },
        UpdateRow {
            case: "the key alone",
            patch: || json!({}),
            min: 4,
            max: 40,
            role: second_role,
            suspended: || {
                json!({"DynamicScalingInSuspended": true, "DynamicScalingOutSuspended": false, "ScheduledScalingSuspended": true})
            },
        },
    ];

    for row in rows {
        let case = row.case;
        let mut request = json!({
            "ServiceNamespace": namespace,
            "ResourceId": resource_id,
            "ScalableDimension": dimension,
        });
        for (member, value) in (row.patch)().as_object().expect("the patch is an object") {
            request[member] = value.clone();
        }

        let (status, body) = emulator.call("RegisterScalableTarget", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the update to answer 200, got {status} {body:?}"
        );
        assert_eq!(
            body["ScalableTargetARN"], baseline_arn,
            "case {case:?}: expected the ARN to stay stable across re-registration, got {body:?}"
        );

        let target = only_target(&emulator, namespace, case).await;
        assert_eq!(
            target["MinCapacity"],
            json!(row.min),
            "case {case:?}: expected MinCapacity {}, got {target:?}",
            row.min
        );
        assert_eq!(
            target["MaxCapacity"],
            json!(row.max),
            "case {case:?}: expected MaxCapacity {}, got {target:?}",
            row.max
        );
        assert_eq!(
            target["RoleARN"],
            json!(row.role),
            "case {case:?}: expected RoleARN {:?}, got {target:?}",
            row.role
        );
        let expected_suspended = (row.suspended)();
        let actual_suspended = target
            .get("SuspendedState")
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            actual_suspended, expected_suspended,
            "case {case:?}: expected SuspendedState {expected_suspended:?}, got {actual_suspended:?}"
        );
        assert_eq!(
            target["CreationTime"], baseline_creation,
            "case {case:?}: expected CreationTime to survive re-registration, got {target:?}"
        );
    }
}

#[tokio::test]
async fn a_registration_outside_the_contract_is_refused_and_persists_nothing() {
    let emulator = Emulator::with_tables(TABLES).await;

    let rows: &[(&str, Value)] = &[
        (
            "no MinCapacity on a first registration",
            json!({"ServiceNamespace": "dynamodb", "ResourceId": "table/row-a", "ScalableDimension": "dynamodb:table:ReadCapacityUnits", "MaxCapacity": 10}),
        ),
        (
            "no MaxCapacity on a first registration",
            json!({"ServiceNamespace": "dynamodb", "ResourceId": "table/row-b", "ScalableDimension": "dynamodb:table:ReadCapacityUnits", "MinCapacity": 1}),
        ),
        (
            "neither capacity on a first registration",
            json!({"ServiceNamespace": "dynamodb", "ResourceId": "table/row-c", "ScalableDimension": "dynamodb:table:ReadCapacityUnits"}),
        ),
        (
            "an inverted capacity window",
            json!({"ServiceNamespace": "dynamodb", "ResourceId": "table/row-d", "ScalableDimension": "dynamodb:table:ReadCapacityUnits", "MinCapacity": 5, "MaxCapacity": 2}),
        ),
        (
            "a negative MinCapacity",
            json!({"ServiceNamespace": "dynamodb", "ResourceId": "table/row-e", "ScalableDimension": "dynamodb:table:ReadCapacityUnits", "MinCapacity": -1, "MaxCapacity": 10}),
        ),
        (
            "a negative MaxCapacity",
            json!({"ServiceNamespace": "dynamodb", "ResourceId": "table/row-f", "ScalableDimension": "dynamodb:table:ReadCapacityUnits", "MinCapacity": 0, "MaxCapacity": -3}),
        ),
        (
            "a namespace outside the enum",
            json!({"ServiceNamespace": "not-a-namespace", "ResourceId": "table/row-g", "ScalableDimension": "dynamodb:table:ReadCapacityUnits", "MinCapacity": 1, "MaxCapacity": 10}),
        ),
        (
            "a dimension outside the enum",
            json!({"ServiceNamespace": "dynamodb", "ResourceId": "table/row-h", "ScalableDimension": "dynamodb:table:NotACapacity", "MinCapacity": 1, "MaxCapacity": 10}),
        ),
    ];

    for (case, request) in rows {
        let (status, body) = emulator
            .call("RegisterScalableTarget", request.clone())
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
    }

    let (status, body) = emulator
        .call(
            "DescribeScalableTargets",
            json!({"ServiceNamespace": "dynamodb"}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"nothing persisted\": expected 200, got {status} {body:?}"
    );
    assert_eq!(
        body["ScalableTargets"],
        json!([]),
        "case \"nothing persisted\": expected a refused registration to store nothing, got {body:?}"
    );
}

/// A policy type, a configuration member and a value for it.
struct PolicyRow {
    case: &'static str,
    namespace: &'static str,
    resource_id: &'static str,
    dimension: &'static str,
    policy_type: &'static str,
    config_member: &'static str,
    config: fn() -> Value,
}

/// One row per modelled `PolicyType` (`POLICY_TYPE_VALUES` in `shared_types.rs`), each
/// with the configuration structure that type requires.
const POLICY_CATALOG: &[PolicyRow] = &[
    PolicyRow {
        case: "target tracking",
        namespace: "dynamodb",
        resource_id: "table/my-table",
        dimension: "dynamodb:table:ReadCapacityUnits",
        policy_type: "TargetTrackingScaling",
        config_member: "TargetTrackingScalingPolicyConfiguration",
        config: || {
            json!({
                "TargetValue": 50.0,
                "PredefinedMetricSpecification": {
                    "PredefinedMetricType": "DynamoDBReadCapacityUtilization"
                },
                "ScaleInCooldown": 120,
                "DisableScaleIn": false,
            })
        },
    },
    PolicyRow {
        case: "step scaling",
        namespace: "ec2",
        resource_id: "spot-fleet-request/sfr-1234abcd",
        dimension: "ec2:spot-fleet-request:TargetCapacity",
        policy_type: "StepScaling",
        config_member: "StepScalingPolicyConfiguration",
        config: || {
            json!({
                "AdjustmentType": "ChangeInCapacity",
                "StepAdjustments": [
                    {"MetricIntervalLowerBound": 0.0, "ScalingAdjustment": 2},
                    {"MetricIntervalUpperBound": 0.0, "ScalingAdjustment": -1},
                ],
                "Cooldown": 60,
                "MetricAggregationType": "Average",
            })
        },
    },
    PolicyRow {
        case: "predictive scaling",
        namespace: "ecs",
        resource_id: "service/prod-cluster/web-app",
        dimension: "ecs:service:DesiredCount",
        policy_type: "PredictiveScaling",
        config_member: "PredictiveScalingPolicyConfiguration",
        config: || {
            json!({
                "MetricSpecifications": [{
                    "TargetValue": 40.0,
                    "PredefinedMetricPairSpecification": {
                        "PredefinedMetricType": "ECSServiceCPUUtilization"
                    },
                }],
                "Mode": "ForecastOnly",
                "MaxCapacityBreachBehavior": "HonorMaxCapacity",
            })
        },
    },
];

#[tokio::test]
async fn a_policy_round_trips_for_every_modelled_policy_type() {
    let emulator = Emulator::with_tables(TABLES).await;

    for row in POLICY_CATALOG {
        let case = row.case;
        let policy_name = format!("{}-policy", row.policy_type);

        emulator.given_resource(row.namespace, row.resource_id, 1).await;
        let (status, body) = emulator
            .call(
                "RegisterScalableTarget",
                json!({
                    "ServiceNamespace": row.namespace,
                    "ResourceId": row.resource_id,
                    "ScalableDimension": row.dimension,
                    "MinCapacity": 1,
                    "MaxCapacity": 20,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the target to register, got {status} {body:?}"
        );

        let mut request = json!({
            "PolicyName": policy_name,
            "ServiceNamespace": row.namespace,
            "ResourceId": row.resource_id,
            "ScalableDimension": row.dimension,
            "PolicyType": row.policy_type,
        });
        request[row.config_member] = (row.config)();

        let (status, body) = emulator.call("PutScalingPolicy", request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutScalingPolicy to answer 200, got {status} {body:?}"
        );
        let policy_arn = body["PolicyARN"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a PolicyARN, got {body:?}"))
            .to_string();
        let expected_prefix = format!("arn:aws:autoscaling:{REGION}:{ACCOUNT}:scalingPolicy:");
        assert!(
            policy_arn.starts_with(&expected_prefix),
            "case {case:?}: expected a PolicyARN under {expected_prefix:?}, got {policy_arn:?}"
        );
        assert!(
            policy_arn.ends_with(&format!(
                ":resource/{}/{}:policyName/{policy_name}",
                row.namespace, row.resource_id
            )),
            "case {case:?}: expected the PolicyARN to name its resource and policy, got {policy_arn:?}"
        );
        // Only a target-tracking policy is alarm-driven, so only it mints alarms; a step
        // or predictive policy's `Alarms` list is the alarms the CALLER attached, and this
        // row attached none.
        let minted = body["Alarms"].as_array().cloned().unwrap_or_default();
        if row.policy_type == "TargetTrackingScaling" {
            assert!(
                !minted.is_empty(),
                "case {case:?}: expected a target-tracking policy to mint alarms, got {body:?}"
            );
        } else {
            assert_eq!(
                body["Alarms"],
                json!([]),
                "case {case:?}: expected no alarms for a {} policy, got {body:?}",
                row.policy_type
            );
        }

        // A repeated Put for the same identity is an update, not a second policy.
        let (status, again) = emulator.call("PutScalingPolicy", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the repeated Put to answer 200, got {status} {again:?}"
        );
        assert_eq!(
            again["PolicyARN"],
            json!(policy_arn),
            "case {case:?}: expected the PolicyARN to stay stable across Puts, got {again:?}"
        );

        let (status, described) = emulator
            .call(
                "DescribeScalingPolicies",
                json!({
                    "ServiceNamespace": row.namespace,
                    "PolicyNames": [policy_name.clone()],
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeScalingPolicies to answer 200, got {status} {described:?}"
        );
        let policies = described["ScalingPolicies"].as_array().unwrap_or_else(|| {
            panic!("case {case:?}: expected a ScalingPolicies list, got {described:?}")
        });
        assert_eq!(
            policies.len(),
            1,
            "case {case:?}: expected exactly one policy, got {policies:?}"
        );
        let policy = &policies[0];
        assert_eq!(
            policy["PolicyARN"],
            json!(policy_arn),
            "case {case:?}: expected the stored PolicyARN, got {policy:?}"
        );
        assert_eq!(
            policy["PolicyName"],
            json!(policy_name),
            "case {case:?}: expected the stored PolicyName, got {policy:?}"
        );
        assert_eq!(
            policy["PolicyType"],
            json!(row.policy_type),
            "case {case:?}: expected the stored PolicyType, got {policy:?}"
        );
        assert_eq!(
            policy["ResourceId"],
            json!(row.resource_id),
            "case {case:?}: expected the stored ResourceId, got {policy:?}"
        );
        assert_eq!(
            policy["ScalableDimension"],
            json!(row.dimension),
            "case {case:?}: expected the stored ScalableDimension, got {policy:?}"
        );
        assert_eq!(
            policy[row.config_member],
            (row.config)(),
            "case {case:?}: expected the configuration as submitted, got {policy:?}"
        );
        assert_eq!(
            policy["Alarms"],
            json!(minted),
            "case {case:?}: expected Describe to report the alarms Put minted, got {policy:?}"
        );
        assert!(
            policy["CreationTime"].as_f64().is_some_and(|at| at > 0.0),
            "case {case:?}: expected a CreationTime after the epoch, got {policy:?}"
        );
        // Only the configuration belonging to the policy's type is reported.
        for other in POLICY_CATALOG
            .iter()
            .map(|r| r.config_member)
            .filter(|member| *member != row.config_member)
        {
            assert!(
                policy.get(other).is_none(),
                "case {case:?}: expected no {other} on a {} policy, got {policy:?}",
                row.policy_type
            );
        }
    }
}

#[tokio::test]
async fn a_policy_without_a_registered_target_is_refused() {
    let emulator = Emulator::with_tables(TABLES).await;

    // Register one target, so the refusal is about the identity and not about an empty
    // store.
    let (status, body) = emulator
        .call(
            "RegisterScalableTarget",
            json!({
                "ServiceNamespace": "dynamodb",
                "ResourceId": "table/my-table",
                "ScalableDimension": "dynamodb:table:ReadCapacityUnits",
                "MinCapacity": 1,
                "MaxCapacity": 10,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"baseline\": expected 200, got {status} {body:?}"
    );

    let rows: &[(&str, &str, &str, &str)] = &[
        (
            "an unregistered resource id",
            "dynamodb",
            "table/absent",
            "dynamodb:table:ReadCapacityUnits",
        ),
        (
            "a different dimension of a registered resource",
            "dynamodb",
            "table/my-table",
            "dynamodb:table:WriteCapacityUnits",
        ),
        (
            "an unregistered namespace",
            "ecs",
            "service/prod-cluster/web-app",
            "ecs:service:DesiredCount",
        ),
    ];

    for (case, namespace, resource_id, dimension) in rows {
        let (status, body) = emulator
            .call(
                "PutScalingPolicy",
                json!({
                    "PolicyName": "my-policy",
                    "ServiceNamespace": namespace,
                    "ResourceId": resource_id,
                    "ScalableDimension": dimension,
                    "PolicyType": "TargetTrackingScaling",
                    "TargetTrackingScalingPolicyConfiguration": {
                        "TargetValue": 50.0,
                        "PredefinedMetricSpecification": {
                            "PredefinedMetricType": "DynamoDBReadCapacityUtilization"
                        },
                    },
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ObjectNotFoundException"),
            "case {case:?}: expected an ObjectNotFoundException, got {body:?}"
        );
    }
}

#[tokio::test]
async fn a_policy_missing_the_configuration_its_type_requires_is_refused() {
    let emulator = Emulator::with_tables(TABLES).await;
    emulator
        .given_resource("ecs", "service/prod-cluster/web-app", 1)
        .await;
    let (status, body) = emulator
        .call(
            "RegisterScalableTarget",
            json!({
                "ServiceNamespace": "ecs",
                "ResourceId": "service/prod-cluster/web-app",
                "ScalableDimension": "ecs:service:DesiredCount",
                "MinCapacity": 1,
                "MaxCapacity": 10,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"baseline\": expected 200, got {status} {body:?}"
    );

    // Each row names a policy type and the configuration member it is missing; the last
    // row names no type at all, which creation requires.
    let rows: &[(&str, Value)] = &[
        ("TargetTrackingScaling", json!({"PolicyType": "TargetTrackingScaling"})),
        ("StepScaling", json!({"PolicyType": "StepScaling"})),
        ("PredictiveScaling", json!({"PolicyType": "PredictiveScaling"})),
        ("no PolicyType on creation", json!({})),
    ];

    for (case, patch) in rows {
        let mut request = json!({
            "PolicyName": format!("policy-for-{case}"),
            "ServiceNamespace": "ecs",
            "ResourceId": "service/prod-cluster/web-app",
            "ScalableDimension": "ecs:service:DesiredCount",
        });
        for (member, value) in patch.as_object().expect("the patch is an object") {
            request[member] = value.clone();
        }

        let (status, body) = emulator.call("PutScalingPolicy", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
    }

    let (status, body) = emulator
        .call("DescribeScalingPolicies", json!({"ServiceNamespace": "ecs"}))
        .await;
    assert_eq!(
        status, 200,
        "case \"nothing persisted\": expected 200, got {status} {body:?}"
    );
    assert_eq!(
        body["ScalingPolicies"],
        json!([]),
        "case \"nothing persisted\": expected a refused Put to store nothing, got {body:?}"
    );
}

#[tokio::test]
async fn describe_pages_through_every_target_in_key_order() {
    let emulator = Emulator::with_tables(TABLES).await;
    let namespace = "dynamodb";
    let dimension = "dynamodb:table:ReadCapacityUnits";
    let resource_ids = [
        "table/alpha",
        "table/bravo",
        "table/charlie",
        "table/delta",
        "table/echo",
    ];

    for resource_id in resource_ids {
        let (status, body) = emulator
            .call(
                "RegisterScalableTarget",
                json!({
                    "ServiceNamespace": namespace,
                    "ResourceId": resource_id,
                    "ScalableDimension": dimension,
                    "MinCapacity": 1,
                    "MaxCapacity": 10,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {resource_id:?}: expected 200, got {status} {body:?}"
        );
    }

    // Every page size from one item to one past the whole set: each must walk the same
    // ordered sequence, and only the final page may omit a NextToken.
    for page_size in [1_i64, 2, 3, 5, 6] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;

        loop {
            let mut request = json!({"ServiceNamespace": namespace, "MaxResults": page_size});
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let (status, body) = emulator.call("DescribeScalableTargets", request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 on page {pages}, got {status} {body:?}"
            );
            let targets = body["ScalableTargets"].as_array().unwrap_or_else(|| {
                panic!("case {case:?}: expected a ScalableTargets list, got {body:?}")
            });
            assert!(
                targets.len() as i64 <= page_size,
                "case {case:?}: expected at most {page_size} items on a page, got {targets:?}"
            );
            for target in targets {
                seen.push(
                    target["ResourceId"]
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("case {case:?}: expected a ResourceId, got {target:?}")
                        })
                        .to_string(),
                );
            }
            next_token = body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            pages += 1;
            assert!(
                pages <= resource_ids.len() + 1,
                "case {case:?}: expected pagination to terminate, got {pages} pages"
            );
            if next_token.is_none() {
                break;
            }
        }

        assert_eq!(
            seen,
            resource_ids.map(ToString::to_string).to_vec(),
            "case {case:?}: expected every target exactly once in key order, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn describe_refuses_a_token_it_did_not_issue_and_a_page_size_out_of_range() {
    let emulator = Emulator::with_tables(TABLES).await;
    let (status, body) = emulator
        .call(
            "RegisterScalableTarget",
            json!({
                "ServiceNamespace": "dynamodb",
                "ResourceId": "table/my-table",
                "ScalableDimension": "dynamodb:table:ReadCapacityUnits",
                "MinCapacity": 1,
                "MaxCapacity": 10,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"baseline\": expected 200, got {status} {body:?}"
    );

    // A token is opaque, so the only tokens that resume a page are the ones this service
    // issued; anything else is refused rather than read as "from the start".
    for (case, token) in [
        ("plain garbage", "garbage"),
        ("a non-hex payload", "not-hex-at-all!!"),
        ("hex carrying too few fields", "6162"),
        ("an empty token", ""),
    ] {
        for operation in ["DescribeScalableTargets", "DescribeScalingPolicies"] {
            let (status, body) = emulator
                .call(
                    operation,
                    json!({"ServiceNamespace": "dynamodb", "NextToken": token}),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case:?} on {operation}: expected HTTP 400, got {status} {body:?}"
            );
            assert!(
                error_code(&body).contains("InvalidNextTokenException"),
                "case {case:?} on {operation}: expected an InvalidNextTokenException, got {body:?}"
            );
        }
    }

    // `MaxResults` "can be between 1 and 50": both published bounds are accepted and one
    // past each is refused.
    for (case, max_results, expected_status) in [
        ("the lower bound", 1_i64, 200_u16),
        ("the upper bound", 50, 200),
        ("one below the lower bound", 0, 400),
        ("one past the upper bound", 51, 400),
    ] {
        for operation in ["DescribeScalableTargets", "DescribeScalingPolicies"] {
            let (status, body) = emulator
                .call(
                    operation,
                    json!({"ServiceNamespace": "dynamodb", "MaxResults": max_results}),
                )
                .await;
            assert_eq!(
                status, expected_status,
                "case {case:?} on {operation}: expected HTTP {expected_status}, got {status} {body:?}"
            );
            if expected_status == 400 {
                assert!(
                    error_code(&body).contains("ValidationException"),
                    "case {case:?} on {operation}: expected a ValidationException, got {body:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn describe_policies_pages_and_filters_by_resource_and_dimension() {
    let emulator = Emulator::with_tables(TABLES).await;
    let namespace = "dynamodb";
    let resources = ["table/alpha", "table/bravo"];
    let dimensions = [
        "dynamodb:table:ReadCapacityUnits",
        "dynamodb:table:WriteCapacityUnits",
    ];

    for resource_id in resources {
        for dimension in dimensions {
            let (status, body) = emulator
                .call(
                    "RegisterScalableTarget",
                    json!({
                        "ServiceNamespace": namespace,
                        "ResourceId": resource_id,
                        "ScalableDimension": dimension,
                        "MinCapacity": 1,
                        "MaxCapacity": 10,
                    }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {resource_id:?}/{dimension:?}: expected 200, got {status} {body:?}"
            );
            // Two policies per target. AWS allows more than one, but not two that track
            // the same metric, so one is the tracker of the metric this dimension
            // publishes and the other is a step policy.
            let tracker = json!({
                "PolicyName": "p-one",
                "ServiceNamespace": namespace,
                "ResourceId": resource_id,
                "ScalableDimension": dimension,
                "PolicyType": "TargetTrackingScaling",
                "TargetTrackingScalingPolicyConfiguration": {
                    "TargetValue": 50.0,
                    "PredefinedMetricSpecification": {
                        "PredefinedMetricType": predefined_metric_of(dimension)
                    },
                },
            });
            let stepper = json!({
                "PolicyName": "p-two",
                "ServiceNamespace": namespace,
                "ResourceId": resource_id,
                "ScalableDimension": dimension,
                "PolicyType": "StepScaling",
                "StepScalingPolicyConfiguration": {
                    "AdjustmentType": "ChangeInCapacity",
                    "StepAdjustments": [
                        {"MetricIntervalLowerBound": 0.0, "ScalingAdjustment": 1},
                    ],
                },
            });
            for (policy_name, request) in [("p-one", tracker), ("p-two", stepper)] {
                let (status, body) = emulator.call("PutScalingPolicy", request).await;
                assert_eq!(
                    status, 200,
                    "case {resource_id:?}/{dimension:?}/{policy_name:?}: expected 200, got {status} {body:?}"
                );
            }
        }
    }

    // Each filter combination, and the number of policies it must select out of the
    // eight stored (2 resources x 2 dimensions x 2 names).
    let rows: &[(&str, Value, usize)] = &[
        ("no filter", json!({}), 8),
        ("by resource id", json!({"ResourceId": "table/alpha"}), 4),
        (
            "by dimension",
            json!({"ScalableDimension": "dynamodb:table:WriteCapacityUnits"}),
            4,
        ),
        (
            "by resource id and dimension",
            json!({"ResourceId": "table/bravo", "ScalableDimension": "dynamodb:table:ReadCapacityUnits"}),
            2,
        ),
        ("by policy name", json!({"PolicyNames": ["p-one"]}), 4),
        ("by an empty name list", json!({"PolicyNames": []}), 8),
        (
            "by a name no policy carries",
            json!({"PolicyNames": ["absent"]}),
            0,
        ),
    ];

    for (case, filter, expected) in rows {
        // Walk every page, so the count is of the whole filtered set and not of one page.
        let mut total = 0;
        let mut next_token: Option<String> = None;
        loop {
            let mut request = json!({"ServiceNamespace": namespace, "MaxResults": 3});
            for (member, value) in filter.as_object().expect("the filter is an object") {
                request[member] = value.clone();
            }
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let (status, body) = emulator.call("DescribeScalingPolicies", request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {body:?}"
            );
            total += body["ScalingPolicies"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a ScalingPolicies list, got {body:?}")
                })
                .len();
            next_token = body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            total, *expected,
            "case {case:?}: expected {expected} policies, got {total}"
        );
    }
}

#[tokio::test]
async fn every_modelled_dimension_names_the_namespace_it_belongs_to() {
    use service_com_amazonaws_applicationautoscaling_anyscalefrontendservice::ops::types::{
        SCALABLE_DIMENSION_VALUES, SERVICE_NAMESPACE_VALUES,
    };

    // The invariant the pair check rests on, asserted over the WHOLE model catalog
    // rather than over the few rows the suites above use: a `ScalableDimension` is
    // "<service namespace>:<resource type>:<scaling property>", so its first segment is
    // always one of the modelled `ServiceNamespace` values.
    for dimension in SCALABLE_DIMENSION_VALUES {
        let owner = dimension.split(':').next().unwrap_or_default();
        assert!(
            SERVICE_NAMESPACE_VALUES.contains(&owner),
            "case {dimension:?}: expected its first segment to be a modelled ServiceNamespace, got {owner:?}"
        );
    }

    let emulator = Emulator::with_tables(TABLES).await;

    // A namespace and a dimension that are each legal but do not belong together: which
    // refusal they get follows what the operation DOES with them. One that takes the pair as
    // part of a REGISTRATION it writes, or as a filter it narrows a read with, refuses the
    // pair itself; one that depends on an existing scalable target resolves the triple as
    // that target's identity, and a pair that cannot belong together names no registered
    // target — which is the answer it gets.
    let mismatches: &[(&str, &str)] = &[
        ("ecs", "dynamodb:table:ReadCapacityUnits"),
        ("dynamodb", "ecs:service:DesiredCount"),
        ("lambda", "sagemaker:variant:DesiredInstanceCount"),
        ("rds", "neptune:cluster:ReadReplicaCount"),
    ];

    for (namespace, dimension) in mismatches {
        let case = format!("{namespace} with {dimension}");
        let requests: &[(&str, Value, &str)] = &[
            (
                "RegisterScalableTarget",
                json!({"ServiceNamespace": namespace, "ResourceId": "res/one", "ScalableDimension": dimension, "MinCapacity": 1, "MaxCapacity": 10}),
                "ValidationException",
            ),
            (
                "DeregisterScalableTarget",
                json!({"ServiceNamespace": namespace, "ResourceId": "res/one", "ScalableDimension": dimension}),
                "ObjectNotFoundException",
            ),
            (
                "PutScalingPolicy",
                json!({"PolicyName": "p", "ServiceNamespace": namespace, "ResourceId": "res/one", "ScalableDimension": dimension, "PolicyType": "TargetTrackingScaling", "TargetTrackingScalingPolicyConfiguration": {"TargetValue": 50.0, "PredefinedMetricSpecification": {"PredefinedMetricType": "DynamoDBReadCapacityUtilization"}}}),
                "ObjectNotFoundException",
            ),
            (
                "DeleteScalingPolicy",
                json!({"PolicyName": "p", "ServiceNamespace": namespace, "ResourceId": "res/one", "ScalableDimension": dimension}),
                "ObjectNotFoundException",
            ),
            (
                "DescribeScalableTargets",
                json!({"ServiceNamespace": namespace, "ScalableDimension": dimension}),
                "ValidationException",
            ),
            (
                "DescribeScalingPolicies",
                json!({"ServiceNamespace": namespace, "ScalableDimension": dimension}),
                "ValidationException",
            ),
        ];

        for (operation, request, expected) in requests {
            let (status, body) = emulator.call(operation, request.clone()).await;
            assert_eq!(
                status, 400,
                "case {case:?} on {operation}: expected HTTP 400, got {status} {body:?}"
            );
            assert!(
                error_code(&body).contains(expected),
                "case {case:?} on {operation}: expected a {expected}, got {body:?}"
            );
        }
    }

    // And the matching pair of the same namespaces still registers — each with the
    // `ResourceId` form its own resource type takes.
    for (namespace, dimension, resource_id) in [
        ("ecs", "ecs:service:DesiredCount", "service/prod/web"),
        (
            "dynamodb",
            "dynamodb:table:ReadCapacityUnits",
            "table/my-table",
        ),
        (
            "lambda",
            "lambda:function:ProvisionedConcurrency",
            "function:ingest:green",
        ),
        ("rds", "rds:cluster:ReadReplicaCount", "cluster:my-db"),
    ] {
        let case = format!("{namespace} with {dimension}");
        emulator.given_resource(namespace, resource_id, 1).await;
        let (status, body) = emulator
            .call(
                "RegisterScalableTarget",
                json!({"ServiceNamespace": namespace, "ResourceId": resource_id, "ScalableDimension": dimension, "MinCapacity": 1, "MaxCapacity": 10}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a matching pair to register, got {status} {body:?}"
        );
    }
}

#[tokio::test]
async fn delete_and_deregister_are_observable_and_refuse_a_second_time() {
    let emulator = Emulator::with_tables(TABLES).await;
    let namespace = "dynamodb";
    let resource_id = "table/my-table";
    let dimension = "dynamodb:table:ReadCapacityUnits";
    let target = json!({
        "ServiceNamespace": namespace,
        "ResourceId": resource_id,
        "ScalableDimension": dimension,
    });

    let mut registration = target.clone();
    registration["MinCapacity"] = json!(1);
    registration["MaxCapacity"] = json!(10);
    let (status, body) = emulator
        .call("RegisterScalableTarget", registration.clone())
        .await;
    assert_eq!(
        status, 200,
        "case \"register\": expected 200, got {status} {body:?}"
    );

    // Two policies on one target. AWS allows more than one, but not two that track the
    // SAME metric, and this dimension publishes exactly one target-tracking metric — so
    // the second policy is a step policy rather than a second tracker of that metric.
    let mut tracker = target.clone();
    tracker["PolicyName"] = json!("keep-me");
    tracker["PolicyType"] = json!("TargetTrackingScaling");
    tracker["TargetTrackingScalingPolicyConfiguration"] = json!({
        "TargetValue": 50.0,
        "PredefinedMetricSpecification": {
            "PredefinedMetricType": "DynamoDBReadCapacityUtilization"
        },
    });
    let mut stepper = target.clone();
    stepper["PolicyName"] = json!("delete-me");
    stepper["PolicyType"] = json!("StepScaling");
    stepper["StepScalingPolicyConfiguration"] = json!({
        "AdjustmentType": "ChangeInCapacity",
        "StepAdjustments": [{"MetricIntervalLowerBound": 0.0, "ScalingAdjustment": 1}],
    });
    for (policy_name, request) in [("keep-me", tracker), ("delete-me", stepper)] {
        let (status, body) = emulator.call("PutScalingPolicy", request).await;
        assert_eq!(
            status, 200,
            "case {policy_name:?}: expected 200, got {status} {body:?}"
        );
    }

    let mut delete = target.clone();
    delete["PolicyName"] = json!("delete-me");
    let (status, body) = emulator.call("DeleteScalingPolicy", delete.clone()).await;
    assert_eq!(
        status, 200,
        "case \"delete a policy\": expected 200, got {status} {body:?}"
    );

    let (status, body) = emulator
        .call(
            "DescribeScalingPolicies",
            json!({"ServiceNamespace": namespace}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"after delete\": expected 200, got {status} {body:?}"
    );
    let names: Vec<&str> = body["ScalingPolicies"]
        .as_array()
        .expect("a ScalingPolicies list")
        .iter()
        .filter_map(|policy| policy["PolicyName"].as_str())
        .collect();
    assert_eq!(
        names,
        vec!["keep-me"],
        "case \"after delete\": expected only the surviving policy, got {names:?}"
    );

    let (status, body) = emulator.call("DeleteScalingPolicy", delete).await;
    assert_eq!(
        status, 400,
        "case \"delete twice\": expected HTTP 400, got {status} {body:?}"
    );
    assert!(
        error_code(&body).contains("ObjectNotFoundException"),
        "case \"delete twice\": expected an ObjectNotFoundException, got {body:?}"
    );

    let (status, body) = emulator
        .call("DeregisterScalableTarget", target.clone())
        .await;
    assert_eq!(
        status, 200,
        "case \"deregister\": expected 200, got {status} {body:?}"
    );

    let (status, body) = emulator
        .call(
            "DescribeScalableTargets",
            json!({"ServiceNamespace": namespace}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"after deregister\": expected 200, got {status} {body:?}"
    );
    assert_eq!(
        body["ScalableTargets"],
        json!([]),
        "case \"after deregister\": expected no scalable targets, got {body:?}"
    );

    // Deregistering cascades: the policies that hung off the target are gone too.
    let (status, body) = emulator
        .call(
            "DescribeScalingPolicies",
            json!({"ServiceNamespace": namespace}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"cascade\": expected 200, got {status} {body:?}"
    );
    assert_eq!(
        body["ScalingPolicies"],
        json!([]),
        "case \"cascade\": expected the target's policies to be deleted with it, got {body:?}"
    );

    let (status, body) = emulator
        .call("DeregisterScalableTarget", target.clone())
        .await;
    assert_eq!(
        status, 400,
        "case \"deregister twice\": expected HTTP 400, got {status} {body:?}"
    );
    assert!(
        error_code(&body).contains("ObjectNotFoundException"),
        "case \"deregister twice\": expected an ObjectNotFoundException, got {body:?}"
    );

    // Registering the identity again is a fresh first registration, not a resurrection
    // of the deregistered record.
    let (status, body) = emulator.call("RegisterScalableTarget", registration).await;
    assert_eq!(
        status, 200,
        "case \"re-register\": expected 200, got {status} {body:?}"
    );
    let (status, body) = emulator
        .call(
            "DescribeScalingPolicies",
            json!({"ServiceNamespace": namespace}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"re-register\": expected 200, got {status} {body:?}"
    );
    assert_eq!(
        body["ScalingPolicies"],
        json!([]),
        "case \"re-register\": expected a re-registered target to carry no policies, got {body:?}"
    );
}
