// SPDX-License-Identifier: CC0-1.0

//! The rules a scaling policy's METRIC, a registration's CAPACITY WINDOW, its service-linked
//! ROLE and two concurrent updates of ONE target have to satisfy, end to end over the wire.
//!
//! Every suite here is a TABLE whose rows vary what the rule generalises over — the scalable
//! dimension, the predefined metric type, the target value, the capacity, the namespace — so
//! the case any one report cited is one row among many. The emulator is the shared harness in
//! `tests/all/emulator.rs`: Application Auto Scaling, DynamoDB and CloudWatch over one engine,
//! every request dispatched through the production `MultiServiceRouter`.

use std::sync::Arc;

use serde_json::{json, Value};

use super::emulator::{
    error_code, resource_id_for, service_linked_role_suffix, Emulator, ACCOUNT,
};

/// One row of the metric/dimension table: a predefined metric put on a scalable dimension,
/// and whether that dimension's own service publishes the metric for it.
struct MetricRow {
    dimension: &'static str,
    metric_type: &'static str,
    accepted: bool,
}

/// Which predefined metric describes which scalable dimension.
///
/// A predefined metric is a metric of ONE service's resources, so it only describes the
/// dimensions that service publishes it for ("Predefined metrics for target tracking scaling
/// policies" in the Application Auto Scaling User Guide), and the model itself says of the
/// one metric that spans two services that "ALBRequestCountPerTarget - This metric type
/// applies only to Spot Fleets and ECS services". The rows walk both directions of the rule
/// and span six namespaces, including pairs no report cited.
const METRIC_CATALOG: &[MetricRow] = &[
    // The capacity property matters, not just the service: a read-capacity metric does not
    // describe a write-capacity dimension.
    MetricRow { dimension: "dynamodb:table:ReadCapacityUnits", metric_type: "DynamoDBReadCapacityUtilization", accepted: true },
    MetricRow { dimension: "dynamodb:table:WriteCapacityUnits", metric_type: "DynamoDBWriteCapacityUtilization", accepted: true },
    MetricRow { dimension: "dynamodb:table:WriteCapacityUnits", metric_type: "DynamoDBReadCapacityUtilization", accepted: false },
    MetricRow { dimension: "dynamodb:table:ReadCapacityUnits", metric_type: "DynamoDBWriteCapacityUtilization", accepted: false },
    // A global secondary index is scaled on the same metrics as its table.
    MetricRow { dimension: "dynamodb:index:ReadCapacityUnits", metric_type: "DynamoDBReadCapacityUtilization", accepted: true },
    MetricRow { dimension: "dynamodb:index:WriteCapacityUnits", metric_type: "DynamoDBReadCapacityUtilization", accepted: false },
    // Amazon Keyspaces publishes its own pair, which does not describe DynamoDB either.
    MetricRow { dimension: "cassandra:table:WriteCapacityUnits", metric_type: "CassandraWriteCapacityUtilization", accepted: true },
    MetricRow { dimension: "cassandra:table:WriteCapacityUnits", metric_type: "CassandraReadCapacityUtilization", accepted: false },
    MetricRow { dimension: "cassandra:table:ReadCapacityUnits", metric_type: "DynamoDBReadCapacityUtilization", accepted: false },
    // A metric of another service entirely.
    MetricRow { dimension: "dynamodb:table:ReadCapacityUnits", metric_type: "ECSServiceAverageCPUUtilization", accepted: false },
    MetricRow { dimension: "workspaces:workspacespool:DesiredUserSessions", metric_type: "ECSServiceAverageCPUUtilization", accepted: false },
    // An ECS service is scaled on either of its own two averages.
    MetricRow { dimension: "ecs:service:DesiredCount", metric_type: "ECSServiceAverageCPUUtilization", accepted: true },
    MetricRow { dimension: "ecs:service:DesiredCount", metric_type: "ECSServiceAverageMemoryUtilization", accepted: true },
    // "This metric type applies only to Spot Fleets and ECS services".
    MetricRow { dimension: "ecs:service:DesiredCount", metric_type: "ALBRequestCountPerTarget", accepted: true },
    MetricRow { dimension: "ec2:spot-fleet-request:TargetCapacity", metric_type: "ALBRequestCountPerTarget", accepted: true },
    MetricRow { dimension: "lambda:function:ProvisionedConcurrency", metric_type: "ALBRequestCountPerTarget", accepted: false },
    MetricRow { dimension: "lambda:function:ProvisionedConcurrency", metric_type: "LambdaProvisionedConcurrencyUtilization", accepted: true },
    // Two properties of one SageMaker variant, each with its own metric.
    MetricRow { dimension: "sagemaker:variant:DesiredInstanceCount", metric_type: "SageMakerVariantInvocationsPerInstance", accepted: true },
    MetricRow { dimension: "sagemaker:variant:DesiredInstanceCount", metric_type: "SageMakerVariantProvisionedConcurrencyUtilization", accepted: false },
    MetricRow { dimension: "sagemaker:variant:DesiredProvisionedConcurrency", metric_type: "SageMakerVariantProvisionedConcurrencyUtilization", accepted: true },
    MetricRow { dimension: "workspaces:workspacespool:DesiredUserSessions", metric_type: "WorkSpacesAverageUserSessionsCapacityUtilization", accepted: true },
];

/// A target-tracking policy tracking `metric_type`, for the target `target` identifies.
fn tracking_policy(target: &Value, name: &str, metric_type: &str, target_value: f64) -> Value {
    let mut specification = json!({"PredefinedMetricType": metric_type});
    // The one metric type that is measured per target group, and so names one.
    if metric_type == "ALBRequestCountPerTarget" {
        specification["ResourceLabel"] =
            json!("app/my-load-balancer/50dc6c495c0c9188/targetgroup/my-targets/73e2d6bc24d8a067");
    }
    let mut request = target.clone();
    request["PolicyName"] = json!(name);
    request["PolicyType"] = json!("TargetTrackingScaling");
    request["TargetTrackingScalingPolicyConfiguration"] = json!({
        "TargetValue": target_value,
        "PredefinedMetricSpecification": specification,
    });
    request
}

/// The policy names a target currently carries.
async fn policy_names(emulator: &Emulator, target: &Value, case: &str) -> Vec<String> {
    let (status, body) = emulator
        .call(
            "DescribeScalingPolicies",
            json!({
                "ServiceNamespace": target["ServiceNamespace"],
                "ResourceId": target["ResourceId"],
                "ScalableDimension": target["ScalableDimension"],
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeScalingPolicies to answer 200, got {status} {body:?}"
    );
    body["ScalingPolicies"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|policy| policy["PolicyName"].as_str().map(ToString::to_string))
        .collect()
}

#[tokio::test]
async fn a_predefined_metric_is_accepted_only_on_a_dimension_it_is_published_for() {
    for row in METRIC_CATALOG {
        let case = format!("{} on {}", row.metric_type, row.dimension);
        // One emulator per row, so a refused row is judged against an empty policy set.
        let emulator = Emulator::empty();
        let target = emulator.given_target(row.dimension, 1, 20).await;
        // A target value every row's metric accepts: inside the published window of the
        // provisioned-throughput metrics, and a plain count for the rest.
        let request = tracking_policy(&target, "tracker", row.metric_type, 50.0);
        let (status, body) = emulator.call("PutScalingPolicy", request).await;

        if row.accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected PutScalingPolicy to answer 200, got {status} {body:?}"
            );
            let alarms = body["Alarms"].as_array().cloned().unwrap_or_default();
            assert!(
                alarms.len() >= 2,
                "case {case:?}: expected the policy to mint the alarms that drive it, got {body:?}"
            );
            assert_eq!(
                policy_names(&emulator, &target, &case).await,
                vec!["tracker".to_string()],
                "case {case:?}: expected the accepted policy to be stored"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {body:?}"
            );
            assert_eq!(
                policy_names(&emulator, &target, &case).await,
                Vec::<String>::new(),
                "case {case:?}: expected a refused policy to leave nothing behind"
            );
            assert_eq!(
                emulator.target_tracking_alarms().await,
                Vec::<Value>::new(),
                "case {case:?}: expected a refused policy to mint no alarm"
            );
        }
    }
}

/// One row of the target-value table: the metric, the value, and whether it is inside the
/// window that metric's own service publishes.
struct TargetValueRow {
    dimension: &'static str,
    metric_type: &'static str,
    target_value: f64,
    accepted: bool,
}

/// Target values at and past each published bound.
///
/// Both services whose scalable dimension IS a provisioned read/write throughput publish the
/// same window for a target utilisation — 20 to 90 percent (the Amazon DynamoDB Developer
/// Guide's "Managing throughput capacity automatically with DynamoDB auto scaling" and the
/// Amazon Keyspaces Developer Guide) — so each bound is tested, and one step past it. A
/// metric that is a COUNT publishes no window, and a large count is accepted.
const TARGET_VALUE_CATALOG: &[TargetValueRow] = &[
    TargetValueRow { dimension: "dynamodb:table:ReadCapacityUnits", metric_type: "DynamoDBReadCapacityUtilization", target_value: 20.0, accepted: true },
    TargetValueRow { dimension: "dynamodb:table:ReadCapacityUnits", metric_type: "DynamoDBReadCapacityUtilization", target_value: 55.5, accepted: true },
    TargetValueRow { dimension: "dynamodb:table:ReadCapacityUnits", metric_type: "DynamoDBReadCapacityUtilization", target_value: 90.0, accepted: true },
    TargetValueRow { dimension: "dynamodb:table:ReadCapacityUnits", metric_type: "DynamoDBReadCapacityUtilization", target_value: 19.9, accepted: false },
    TargetValueRow { dimension: "dynamodb:table:ReadCapacityUnits", metric_type: "DynamoDBReadCapacityUtilization", target_value: 90.1, accepted: false },
    TargetValueRow { dimension: "dynamodb:table:ReadCapacityUnits", metric_type: "DynamoDBReadCapacityUtilization", target_value: 150.0, accepted: false },
    TargetValueRow { dimension: "dynamodb:table:ReadCapacityUnits", metric_type: "DynamoDBReadCapacityUtilization", target_value: 0.0, accepted: false },
    TargetValueRow { dimension: "dynamodb:index:WriteCapacityUnits", metric_type: "DynamoDBWriteCapacityUtilization", target_value: 95.0, accepted: false },
    TargetValueRow { dimension: "cassandra:table:ReadCapacityUnits", metric_type: "CassandraReadCapacityUtilization", target_value: 90.0, accepted: true },
    TargetValueRow { dimension: "cassandra:table:ReadCapacityUnits", metric_type: "CassandraReadCapacityUtilization", target_value: 99.0, accepted: false },
    // A request count per target is not a percentage of anything, so nothing bounds it.
    TargetValueRow { dimension: "ecs:service:DesiredCount", metric_type: "ALBRequestCountPerTarget", target_value: 1000.0, accepted: true },
    TargetValueRow { dimension: "ecs:service:DesiredCount", metric_type: "ECSServiceAverageCPUUtilization", target_value: 95.0, accepted: true },
];

#[tokio::test]
async fn a_target_value_outside_the_metrics_published_window_is_refused() {
    for row in TARGET_VALUE_CATALOG {
        let case = format!("{} at {}", row.metric_type, row.target_value);
        let emulator = Emulator::empty();
        let target = emulator.given_target(row.dimension, 1, 20).await;
        let request = tracking_policy(&target, "tracker", row.metric_type, row.target_value);
        let (status, body) = emulator.call("PutScalingPolicy", request).await;

        if row.accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected PutScalingPolicy to answer 200, got {status} {body:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {body:?}"
            );
            assert_eq!(
                policy_names(&emulator, &target, &case).await,
                Vec::<String>::new(),
                "case {case:?}: expected a refused policy to leave nothing behind"
            );
        }
    }
}

/// Where a customized metric may be tracked: the dimension, and whether its owning service
/// scales from a signal of the caller's choosing.
const CUSTOMIZED_METRIC_CATALOG: &[(&str, bool)] = &[
    ("dynamodb:table:ReadCapacityUnits", false),
    ("dynamodb:table:WriteCapacityUnits", false),
    ("dynamodb:index:ReadCapacityUnits", false),
    ("cassandra:table:WriteCapacityUnits", false),
    ("ecs:service:DesiredCount", true),
    ("lambda:function:ProvisionedConcurrency", true),
    ("sagemaker:variant:DesiredInstanceCount", true),
];

#[tokio::test]
async fn a_customized_metric_is_refused_where_the_dimension_is_a_provisioned_throughput() {
    for (dimension, accepted) in CUSTOMIZED_METRIC_CATALOG {
        let case = format!("a customized metric on {dimension}");
        let emulator = Emulator::empty();
        let target = emulator.given_target(dimension, 1, 20).await;
        let mut request = target.clone();
        request["PolicyName"] = json!("tracker");
        request["PolicyType"] = json!("TargetTrackingScaling");
        request["TargetTrackingScalingPolicyConfiguration"] = json!({
            "TargetValue": 40.0,
            "CustomizedMetricSpecification": {
                "MetricName": "QueueDepth",
                "Namespace": "My/App",
                "Statistic": "Average",
                "Dimensions": [{"Name": "Queue", "Value": "ingest"}],
            },
        });
        let (status, body) = emulator.call("PutScalingPolicy", request).await;

        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected PutScalingPolicy to answer 200, got {status} {body:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {body:?}"
            );
            assert_eq!(
                policy_names(&emulator, &target, &case).await,
                Vec::<String>::new(),
                "case {case:?}: expected a refused policy to leave nothing behind"
            );
        }
    }
}

#[tokio::test]
async fn one_metric_takes_one_target_tracking_policy_per_target() {
    let emulator = Emulator::empty();
    let target = emulator
        .given_target("dynamodb:table:ReadCapacityUnits", 1, 20)
        .await;

    let (status, body) = emulator
        .call(
            "PutScalingPolicy",
            tracking_policy(&target, "first", "DynamoDBReadCapacityUtilization", 50.0),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"the first tracker\": expected 200, got {status} {body:?}"
    );
    let alarms_of_first = emulator.target_tracking_alarms().await;

    // A SECOND policy, under another name, steering the same metric: two policies would
    // compute contradictory capacity from one signal.
    let (status, body) = emulator
        .call(
            "PutScalingPolicy",
            tracking_policy(&target, "second", "DynamoDBReadCapacityUtilization", 70.0),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"a second tracker of one metric\": expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case \"a second tracker of one metric\": expected a ValidationException, got {body:?}"
    );
    assert_eq!(
        policy_names(&emulator, &target, "a second tracker of one metric").await,
        vec!["first".to_string()],
        "case \"a second tracker of one metric\": expected only the first policy to survive"
    );
    assert_eq!(
        emulator.target_tracking_alarms().await,
        alarms_of_first,
        "case \"a second tracker of one metric\": expected the refusal to disturb no alarm"
    );

    // The SAME policy name is an upsert of that one policy, not a second tracker of the
    // metric, so re-putting it with a new target value is accepted.
    let (status, body) = emulator
        .call(
            "PutScalingPolicy",
            tracking_policy(&target, "first", "DynamoDBReadCapacityUtilization", 65.0),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"re-putting one policy\": expected 200, got {status} {body:?}"
    );
    assert_eq!(
        policy_names(&emulator, &target, "re-putting one policy").await,
        vec!["first".to_string()],
        "case \"re-putting one policy\": expected one policy, not two"
    );

    // The same metric on ANOTHER target is another signal, and is accepted.
    let other = emulator
        .given_target("dynamodb:index:ReadCapacityUnits", 1, 20)
        .await;
    let (status, body) = emulator
        .call(
            "PutScalingPolicy",
            tracking_policy(&other, "first", "DynamoDBReadCapacityUtilization", 50.0),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"the same metric on another target\": expected 200, got {status} {body:?}"
    );

    // And two policies of ONE target are fine as long as they track different metrics:
    // an ECS service publishes both an average CPU and an average memory metric.
    let service = emulator.given_target("ecs:service:DesiredCount", 1, 20).await;
    for (case, metric_type) in [
        ("the CPU tracker", "ECSServiceAverageCPUUtilization"),
        ("the memory tracker", "ECSServiceAverageMemoryUtilization"),
    ] {
        let (status, body) = emulator
            .call(
                "PutScalingPolicy",
                tracking_policy(&service, case, metric_type, 60.0),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected two trackers of two metrics to be accepted, got {status} {body:?}"
        );
    }
    let mut names = policy_names(&emulator, &service, "two metrics, two policies").await;
    names.sort();
    assert_eq!(
        names,
        vec!["the CPU tracker".to_string(), "the memory tracker".to_string()],
        "case \"two metrics, two policies\": expected both policies to be stored"
    );
}

#[tokio::test]
async fn an_omitted_role_resolves_to_the_namespaces_suffixed_service_linked_role() {
    // One row per namespace this harness can name a resource for, which is every modelled
    // namespace that documents a `ResourceId` form.
    let dimensions = [
        "appstream:fleet:DesiredCapacity",
        "cassandra:table:ReadCapacityUnits",
        "dynamodb:table:ReadCapacityUnits",
        "ec2:spot-fleet-request:TargetCapacity",
        "ecs:service:DesiredCount",
        "elasticache:replication-group:Replicas",
        "elasticmapreduce:instancegroup:InstanceCount",
        "kafka:broker-storage:VolumeSize",
        "lambda:function:ProvisionedConcurrency",
        "neptune:cluster:ReadReplicaCount",
        "rds:cluster:ReadReplicaCount",
        "sagemaker:variant:DesiredInstanceCount",
        "workspaces:workspacespool:DesiredUserSessions",
    ];

    for dimension in dimensions {
        let namespace = dimension.split(':').next().unwrap_or_default();
        let case = format!("an omitted RoleARN on {dimension}");
        let emulator = Emulator::empty();
        // The floor of a provisioned-throughput dimension is one unit; every other
        // dimension accepts one too.
        let target = emulator.given_target(dimension, 1, 10).await;

        let (status, body) = emulator
            .call(
                "DescribeScalableTargets",
                json!({
                    "ServiceNamespace": namespace,
                    "ResourceIds": [target["ResourceId"]],
                    "ScalableDimension": dimension,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeScalableTargets to answer 200, got {status} {body:?}"
        );
        let expected = format!(
            "arn:aws:iam::{ACCOUNT}:role/aws-service-role/{namespace}.application-autoscaling.amazonaws.com/AWSServiceRoleForApplicationAutoScaling_{}",
            service_linked_role_suffix(namespace)
        );
        assert_eq!(
            body["ScalableTargets"][0]["RoleARN"],
            json!(expected),
            "case {case:?}: expected RoleARN {expected:?}, got {body:?}"
        );
    }
}

/// One row of the capacity-floor table: a window, and whether the dimension's owning service
/// can provision it.
struct CapacityRow {
    dimension: &'static str,
    min: i64,
    max: i64,
    accepted: bool,
}

/// Capacity windows at and below each dimension's floor.
///
/// A scalable dimension's capacity is the owning service's own provisioning unit: the
/// smallest provisionable read or write capacity is ONE unit for a DynamoDB table or index
/// (Amazon DynamoDB Developer Guide) and for an Amazon Keyspaces provisioned-throughput table
/// (Amazon Keyspaces Developer Guide), while a desired count or a replica count documents no
/// floor and may be scaled to zero.
const CAPACITY_CATALOG: &[CapacityRow] = &[
    CapacityRow { dimension: "dynamodb:table:ReadCapacityUnits", min: 1, max: 10, accepted: true },
    CapacityRow { dimension: "dynamodb:table:ReadCapacityUnits", min: 0, max: 10, accepted: false },
    CapacityRow { dimension: "dynamodb:table:WriteCapacityUnits", min: 0, max: 0, accepted: false },
    CapacityRow { dimension: "dynamodb:index:ReadCapacityUnits", min: 1, max: 1, accepted: true },
    CapacityRow { dimension: "dynamodb:index:WriteCapacityUnits", min: 0, max: 5, accepted: false },
    CapacityRow { dimension: "cassandra:table:ReadCapacityUnits", min: 1, max: 4, accepted: true },
    CapacityRow { dimension: "cassandra:table:WriteCapacityUnits", min: 0, max: 4, accepted: false },
    // A task count, a replica count and a session count all scale to zero.
    CapacityRow { dimension: "ecs:service:DesiredCount", min: 0, max: 10, accepted: true },
    CapacityRow { dimension: "rds:cluster:ReadReplicaCount", min: 0, max: 5, accepted: true },
    CapacityRow { dimension: "workspaces:workspacespool:DesiredUserSessions", min: 0, max: 7, accepted: true },
];

#[tokio::test]
async fn a_capacity_below_the_dimensions_floor_is_refused() {
    for row in CAPACITY_CATALOG {
        let case = format!("{} with {}..{}", row.dimension, row.min, row.max);
        let emulator = Emulator::empty();
        let namespace = row.dimension.split(':').next().unwrap_or_default();
        let resource_id = resource_id_for(row.dimension);
        // The DynamoDB rows name a real table, so the registration is refused (or accepted)
        // on its capacity window and not on an unresolvable resource.
        if namespace == "dynamodb" {
            let rest = resource_id.trim_start_matches("table/");
            match rest.split_once("/index/") {
                Some((table, index)) => {
                    emulator
                        .create_table_with_index(table, index, "status", 4, 4)
                        .await
                }
                None => emulator.create_provisioned_table(rest, 4, 4).await,
            }
        }
        let (status, body) = emulator
            .call(
                "RegisterScalableTarget",
                json!({
                    "ServiceNamespace": namespace,
                    "ResourceId": resource_id,
                    "ScalableDimension": row.dimension,
                    "MinCapacity": row.min,
                    "MaxCapacity": row.max,
                }),
            )
            .await;

        if row.accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected RegisterScalableTarget to answer 200, got {status} {body:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {body:?}"
            );
            let (status, listed) = emulator
                .call(
                    "DescribeScalableTargets",
                    json!({"ServiceNamespace": namespace}),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected DescribeScalableTargets to answer 200, got {listed:?}"
            );
            assert_eq!(
                listed["ScalableTargets"],
                json!([]),
                "case {case:?}: expected a refused registration to persist nothing, got {listed:?}"
            );
        }
    }
}

/// How many registrations the concurrency suite fires at once.
const CONCURRENT_CALLS: i64 = 10;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_registrations_of_one_target_either_win_or_are_refused_as_concurrent() {
    let emulator = Arc::new(Emulator::empty());
    let target = emulator
        .given_target("dynamodb:table:ReadCapacityUnits", 1, 10)
        .await;

    // Every call asks for a DIFFERENT window of the SAME target, so a lost update would be
    // visible afterwards as a window nobody asked for.
    let mut calls = Vec::new();
    for index in 0..CONCURRENT_CALLS {
        let emulator = Arc::clone(&emulator);
        let mut request = target.clone();
        request["MinCapacity"] = json!(index + 1);
        request["MaxCapacity"] = json!(100 + index);
        calls.push(tokio::spawn(async move {
            emulator.call("RegisterScalableTarget", request).await
        }));
    }

    let mut accepted = 0usize;
    let mut refused = 0usize;
    for (index, call) in calls.into_iter().enumerate() {
        let (status, body) = call.await.expect("the registration task ran");
        match status {
            200 => accepted += 1,
            500 => {
                assert_eq!(
                    error_code(&body),
                    "ConcurrentUpdateException",
                    "case {index}: expected a refused concurrent update to be a \
                     ConcurrentUpdateException, got {body:?}"
                );
                refused += 1;
            }
            other => panic!(
                "case {index}: expected 200 or a 500 ConcurrentUpdateException, got {other} {body:?}"
            ),
        }
    }
    assert_eq!(
        accepted + refused,
        CONCURRENT_CALLS as usize,
        "case \"the whole batch\": expected every call to be answered, got {accepted} accepted \
         and {refused} refused"
    );
    assert!(
        accepted >= 1,
        "case \"the whole batch\": expected at least one call to win the race"
    );
    assert!(
        refused >= 1,
        "case \"the whole batch\": expected an overlapping update to be refused as a \
         ConcurrentUpdateException, got {accepted} accepted and {refused} refused"
    );

    // Whoever won, the surviving target is ONE record with a window one of the calls asked
    // for — never a mix of two of them.
    let (status, body) = emulator
        .call(
            "DescribeScalableTargets",
            json!({"ServiceNamespace": "dynamodb"}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"after the race\": expected 200, got {status} {body:?}"
    );
    let targets = body["ScalableTargets"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        targets.len(),
        1,
        "case \"after the race\": expected exactly one scalable target, got {targets:?}"
    );
    let min = targets[0]["MinCapacity"].as_i64().unwrap_or_default();
    let max = targets[0]["MaxCapacity"].as_i64().unwrap_or_default();
    assert!(
        (1..=CONCURRENT_CALLS).contains(&min) && max == 100 + min - 1,
        "case \"after the race\": expected the window of one of the calls, got {min}..{max}"
    );

    // The in-flight marker is a span of the request, not a state the target is left in: a
    // sequential registration right afterwards still succeeds.
    let mut again = target.clone();
    again["MinCapacity"] = json!(2);
    again["MaxCapacity"] = json!(42);
    let (status, body) = emulator.call("RegisterScalableTarget", again).await;
    assert_eq!(
        status, 200,
        "case \"a later sequential update\": expected 200, got {status} {body:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_registrations_of_different_targets_all_succeed() {
    let emulator = Arc::new(Emulator::empty());
    // Each call registers a DIFFERENT target, so none of them is a concurrent update of
    // another: an update is pending for one target, not for the service.
    let dimensions = [
        "ecs:service:DesiredCount",
        "lambda:function:ProvisionedConcurrency",
        "rds:cluster:ReadReplicaCount",
        "neptune:cluster:ReadReplicaCount",
        "sagemaker:variant:DesiredInstanceCount",
        "workspaces:workspacespool:DesiredUserSessions",
    ];

    let mut calls = Vec::new();
    for dimension in dimensions {
        let emulator = Arc::clone(&emulator);
        calls.push(tokio::spawn(async move {
            let (status, body) = emulator
                .call(
                    "RegisterScalableTarget",
                    json!({
                        "ServiceNamespace": dimension.split(':').next().unwrap_or_default(),
                        "ResourceId": resource_id_for(dimension),
                        "ScalableDimension": dimension,
                        "MinCapacity": 1,
                        "MaxCapacity": 9,
                    }),
                )
                .await;
            (dimension, status, body)
        }));
    }

    for call in calls {
        let (dimension, status, body) = call.await.expect("the registration task ran");
        assert_eq!(
            status, 200,
            "case {dimension:?}: expected a registration of its own target to answer 200, got \
             {status} {body:?}"
        );
    }
}
