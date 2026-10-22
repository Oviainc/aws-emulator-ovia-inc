// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch alarms a target-tracking scaling policy provisions, end to end.
//!
//! A target-tracking policy is not a document the service files away: it is driven by
//! CloudWatch alarms, and `PutScalingPolicyResponse$Alarms` is the set it minted. The rule
//! the table below asserts, over a span of namespaces, metric types, capacities and
//! periods:
//!
//! * putting a target-tracking policy materialises one alarm per enabled scaling
//!   direction on the metric the policy tracks, plus — only where the tracked metric counts
//!   capacity units consumed over the alarm's period — one more per direction on the
//!   resource's provisioned capacity;
//! * the scale-out alarm's threshold is COMPUTED, as `target% * period * provisioned units`
//!   for such a consumed-units metric and as the target value itself for a metric that
//!   already is the quantity the target names, with the scale-in alarm a fixed margin below
//!   it;
//! * a direction that is disabled (`DisableScaleIn`) or suspended (`SuspendedState`) has no
//!   alarm, and resuming it brings the alarm back;
//! * an alarm belongs to the policy that minted it, so deleting that policy retires it and
//!   leaves every sibling policy's alarms alone.

use serde_json::{json, Value};

use super::emulator::{
    alarm_kind, alarm_kinds, error_code, Emulator, ACCOUNT, ALARM_NAME_PREFIX, REGION,
};

/// The margin below the target the scale-in alarm sits at: target tracking scales in only
/// after utilisation has dropped a fifth below the target.
const SCALE_IN_FRACTION: f64 = 0.8;

/// How the resource a row scales is created in the service that owns it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resource {
    /// A PROVISIONED DynamoDB table with this many read and write capacity units.
    Table(i64),
    /// A PROVISIONED DynamoDB table with a global secondary index of that capacity.
    TableIndex(i64),
    /// A resource in a service whose store this emulator does not hold, so there is
    /// nothing to create.
    External,
}

/// One row of the alarm-provisioning table.
struct AlarmRow {
    case: &'static str,
    namespace: &'static str,
    resource_id: &'static str,
    dimension: &'static str,
    /// A value of the model's `MetricType` enum.
    metric_type: &'static str,
    target_value: f64,
    resource: Resource,
    disable_scale_in: bool,
    /// The alarm kinds the policy must own, sorted.
    expected_kinds: &'static [&'static str],
    /// The CloudWatch namespace, metric and statistic the alarms must watch.
    expected_namespace: &'static str,
    expected_metric: &'static str,
    expected_statistic: &'static str,
    /// The alarm period, in seconds.
    expected_period: i64,
    /// The scale-out alarm's threshold.
    expected_threshold: f64,
    /// The dimensions the alarm must carry, as (name, value) pairs.
    expected_dimensions: &'static [(&'static str, &'static str)],
}

/// Both scaling directions of a consumed-units metric, which also carries the two alarms
/// that watch the capacity its thresholds were computed from.
const CAPACITY_ALARMS: &[&str] = &[
    "AlarmHigh",
    "AlarmLow",
    "ProvisionedCapacityHigh",
    "ProvisionedCapacityLow",
];

/// Both scaling directions of a metric that is already the quantity the target names.
const UTILISATION_ALARMS: &[&str] = &["AlarmHigh", "AlarmLow"];

/// The catalog. Each row differs along what the rule generalises over: the namespace and
/// resource type, whether the tracked metric is a consumed-units count or a percentage, the
/// resource's provisioned capacity, the alarm period (the model documents the
/// `…HighResolution` metric types as evaluating "every 20 seconds"), and whether scale-in is
/// disabled. The first row is the case the issue cited — 40% of one provisioned read unit at
/// a 60-second period, so a threshold of 24 consumed units.
const ALARM_CATALOG: &[AlarmRow] = &[
    AlarmRow {
        case: "dynamodb read capacity at 40% of one provisioned unit",
        namespace: "dynamodb",
        resource_id: "table/orders",
        dimension: "dynamodb:table:ReadCapacityUnits",
        metric_type: "DynamoDBReadCapacityUtilization",
        target_value: 40.0,
        resource: Resource::Table(1),
        disable_scale_in: false,
        expected_kinds: CAPACITY_ALARMS,
        expected_namespace: "AWS/DynamoDB",
        expected_metric: "ConsumedReadCapacityUnits",
        expected_statistic: "Sum",
        expected_period: 60,
        expected_threshold: 24.0,
        expected_dimensions: &[("TableName", "orders")],
    },
    AlarmRow {
        case: "dynamodb write capacity at 70% of five provisioned units",
        namespace: "dynamodb",
        resource_id: "table/orders",
        dimension: "dynamodb:table:WriteCapacityUnits",
        metric_type: "DynamoDBWriteCapacityUtilization",
        target_value: 70.0,
        resource: Resource::Table(5),
        disable_scale_in: false,
        expected_kinds: CAPACITY_ALARMS,
        expected_namespace: "AWS/DynamoDB",
        expected_metric: "ConsumedWriteCapacityUnits",
        expected_statistic: "Sum",
        expected_period: 60,
        expected_threshold: 210.0,
        expected_dimensions: &[("TableName", "orders")],
    },
    AlarmRow {
        case: "a global secondary index's read capacity at 50% of two units",
        namespace: "dynamodb",
        resource_id: "table/orders/index/by-customer",
        dimension: "dynamodb:index:ReadCapacityUnits",
        metric_type: "DynamoDBReadCapacityUtilization",
        target_value: 50.0,
        resource: Resource::TableIndex(2),
        disable_scale_in: false,
        expected_kinds: CAPACITY_ALARMS,
        expected_namespace: "AWS/DynamoDB",
        expected_metric: "ConsumedReadCapacityUnits",
        expected_statistic: "Sum",
        expected_period: 60,
        expected_threshold: 60.0,
        expected_dimensions: &[
            ("TableName", "orders"),
            ("GlobalSecondaryIndexName", "by-customer"),
        ],
    },
    AlarmRow {
        case: "an ECS service's CPU utilisation is already a percentage",
        namespace: "ecs",
        resource_id: "service/prod-cluster/web-app",
        dimension: "ecs:service:DesiredCount",
        metric_type: "ECSServiceAverageCPUUtilization",
        target_value: 45.0,
        resource: Resource::External,
        disable_scale_in: false,
        expected_kinds: UTILISATION_ALARMS,
        expected_namespace: "AWS/ECS",
        expected_metric: "CPUUtilization",
        expected_statistic: "Average",
        expected_period: 60,
        expected_threshold: 45.0,
        expected_dimensions: &[("ClusterName", "prod-cluster"), ("ServiceName", "web-app")],
    },
    AlarmRow {
        case: "a high-resolution ECS metric is evaluated every 20 seconds",
        namespace: "ecs",
        resource_id: "service/prod-cluster/web-app",
        dimension: "ecs:service:DesiredCount",
        metric_type: "ECSServiceAverageMemoryUtilizationHighResolution",
        target_value: 65.0,
        resource: Resource::External,
        disable_scale_in: false,
        expected_kinds: UTILISATION_ALARMS,
        expected_namespace: "AWS/ECS",
        expected_metric: "MemoryUtilization",
        expected_statistic: "Average",
        expected_period: 20,
        expected_threshold: 65.0,
        expected_dimensions: &[("ClusterName", "prod-cluster"), ("ServiceName", "web-app")],
    },
    AlarmRow {
        case: "lambda provisioned-concurrency utilisation, a fractional target",
        namespace: "lambda",
        resource_id: "function:ingest:green",
        dimension: "lambda:function:ProvisionedConcurrency",
        metric_type: "LambdaProvisionedConcurrencyUtilization",
        target_value: 0.7,
        resource: Resource::External,
        disable_scale_in: false,
        expected_kinds: UTILISATION_ALARMS,
        expected_namespace: "AWS/Lambda",
        expected_metric: "ProvisionedConcurrencyUtilization",
        expected_statistic: "Average",
        expected_period: 60,
        expected_threshold: 0.7,
        expected_dimensions: &[("FunctionName", "ingest"), ("Resource", "ingest:green")],
    },
    AlarmRow {
        case: "DisableScaleIn withholds both scale-in alarms",
        namespace: "dynamodb",
        resource_id: "table/orders",
        dimension: "dynamodb:table:ReadCapacityUnits",
        metric_type: "DynamoDBReadCapacityUtilization",
        target_value: 40.0,
        resource: Resource::Table(1),
        disable_scale_in: true,
        expected_kinds: &["AlarmHigh", "ProvisionedCapacityHigh"],
        expected_namespace: "AWS/DynamoDB",
        expected_metric: "ConsumedReadCapacityUnits",
        expected_statistic: "Sum",
        expected_period: 60,
        expected_threshold: 24.0,
        expected_dimensions: &[("TableName", "orders")],
    },
];

/// The policy body one row puts.
fn policy_request(row: &AlarmRow, policy_name: &str) -> Value {
    let mut configuration = json!({
        "TargetValue": row.target_value,
        "PredefinedMetricSpecification": {"PredefinedMetricType": row.metric_type},
    });
    if row.disable_scale_in {
        configuration["DisableScaleIn"] = json!(true);
    }
    json!({
        "PolicyName": policy_name,
        "ServiceNamespace": row.namespace,
        "ResourceId": row.resource_id,
        "ScalableDimension": row.dimension,
        "PolicyType": "TargetTrackingScaling",
        "TargetTrackingScalingPolicyConfiguration": configuration,
    })
}

/// Create the resource a row scales, then register the scalable target on it.
async fn register(emulator: &Emulator, row: &AlarmRow) {
    match row.resource {
        Resource::Table(capacity) => {
            emulator
                .create_provisioned_table("orders", capacity, capacity)
                .await;
        }
        Resource::TableIndex(capacity) => {
            emulator
                .create_table_with_index("orders", "by-customer", "customer", capacity, capacity)
                .await;
        }
        Resource::External => {}
    }
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
        "case {:?}: expected the target to register, got {status} {body:?}",
        row.case
    );
}

/// One alarm of `alarms`, by kind.
fn alarm_of<'a>(alarms: &'a [Value], kind: &str, case: &str) -> &'a Value {
    alarms
        .iter()
        .find(|alarm| alarm_kind(alarm) == kind)
        .unwrap_or_else(|| panic!("case {case:?}: expected an alarm of kind {kind}, got {alarms:?}"))
}

#[tokio::test]
async fn a_target_tracking_policy_mints_the_alarms_that_drive_it() {
    for row in ALARM_CATALOG {
        let case = row.case;
        let emulator = Emulator::empty();
        register(&emulator, row).await;

        let (status, body) = emulator
            .call("PutScalingPolicy", policy_request(row, "tracker"))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutScalingPolicy to answer 200, got {status} {body:?}"
        );
        let policy_arn = body["PolicyARN"].as_str().unwrap_or_default().to_string();
        let reported = body["Alarms"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            alarm_kinds(&reported),
            row.expected_kinds,
            "case {case:?}: expected the response to name the alarms it minted, got {body:?}"
        );

        // The alarms are in CloudWatch, where a client finds them by prefix.
        let alarms = emulator.target_tracking_alarms().await;
        assert_eq!(
            alarm_kinds(&alarms),
            row.expected_kinds,
            "case {case:?}: expected CloudWatch to hold the policy's alarms, got {alarms:?}"
        );
        for alarm in &alarms {
            let name = alarm["AlarmName"].as_str().unwrap_or_default();
            assert!(
                name.starts_with(ALARM_NAME_PREFIX),
                "case {case:?}: expected every minted alarm under the {ALARM_NAME_PREFIX} \
                 prefix, got {name:?}"
            );
            assert_eq!(
                alarm["AlarmArn"],
                json!(format!("arn:aws:cloudwatch:{REGION}:{ACCOUNT}:alarm:{name}")),
                "case {case:?}: expected the alarm's own ARN, got {alarm:?}"
            );
            assert_eq!(
                alarm["AlarmActions"],
                json!([policy_arn]),
                "case {case:?}: expected the alarm's action to be the scaling policy, got \
                 {alarm:?}"
            );
            assert_eq!(
                alarm["Namespace"],
                json!(row.expected_namespace),
                "case {case:?}: expected the metric namespace of the tracked metric, got {alarm:?}"
            );
            let expected_dimensions: Vec<Value> = row
                .expected_dimensions
                .iter()
                .map(|(name, value)| json!({"Name": name, "Value": value}))
                .collect();
            let mut dimensions = alarm["Dimensions"].as_array().cloned().unwrap_or_default();
            dimensions.sort_by_key(|dimension| {
                dimension["Name"].as_str().unwrap_or_default().to_string()
            });
            let mut expected = expected_dimensions.clone();
            expected.sort_by_key(|dimension| {
                dimension["Name"].as_str().unwrap_or_default().to_string()
            });
            assert_eq!(
                dimensions, expected,
                "case {case:?}: expected the alarm to be dimensioned by the resource it \
                 watches, got {alarm:?}"
            );
        }

        let high = alarm_of(&alarms, "AlarmHigh", case);
        assert_eq!(
            high["MetricName"],
            json!(row.expected_metric),
            "case {case:?}: expected the tracked metric, got {high:?}"
        );
        assert_eq!(
            high["Statistic"],
            json!(row.expected_statistic),
            "case {case:?}: expected the statistic the metric is aggregated with, got {high:?}"
        );
        assert_eq!(
            high["Period"],
            json!(row.expected_period),
            "case {case:?}: expected the alarm period, got {high:?}"
        );
        assert_eq!(
            high["ComparisonOperator"],
            json!("GreaterThanThreshold"),
            "case {case:?}: expected the scale-out alarm to fire above its threshold, got {high:?}"
        );
        let threshold = high["Threshold"].as_f64().unwrap_or_default();
        assert!(
            (threshold - row.expected_threshold).abs() < 1e-9,
            "case {case:?}: expected a scale-out threshold of {}, got {threshold}",
            row.expected_threshold
        );

        if row.expected_kinds.contains(&"AlarmLow") {
            let low = alarm_of(&alarms, "AlarmLow", case);
            let expected = row.expected_threshold * SCALE_IN_FRACTION;
            let threshold = low["Threshold"].as_f64().unwrap_or_default();
            assert!(
                (threshold - expected).abs() < 1e-9,
                "case {case:?}: expected a scale-in threshold of {expected}, got {threshold}"
            );
            assert_eq!(
                low["ComparisonOperator"],
                json!("LessThanThreshold"),
                "case {case:?}: expected the scale-in alarm to fire below its threshold, got \
                 {low:?}"
            );
        }

        // The capacity pair watches the provisioned capacity the thresholds above were
        // computed from, not the tracked metric.
        if row.expected_kinds.contains(&"ProvisionedCapacityHigh") {
            let capacity = alarm_of(&alarms, "ProvisionedCapacityHigh", case);
            let expected_metric = if row.dimension.ends_with("ReadCapacityUnits") {
                "ProvisionedReadCapacityUnits"
            } else {
                "ProvisionedWriteCapacityUnits"
            };
            assert_eq!(
                capacity["MetricName"],
                json!(expected_metric),
                "case {case:?}: expected the capacity alarm to watch the provisioned capacity, \
                 got {capacity:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_threshold_is_recomputed_from_the_resources_capacity_not_copied_from_the_target() {
    // The same policy shape on two tables whose provisioned capacity differs: a threshold
    // that was the recorded target value would be equal for both.
    for (table, capacity, expected) in [("small", 1, 24.0), ("large", 10, 240.0)] {
        let emulator = Emulator::empty();
        emulator
            .create_provisioned_table(table, capacity, capacity)
            .await;
        let resource_id = format!("table/{table}");
        let (status, body) = emulator
            .call(
                "RegisterScalableTarget",
                json!({"ServiceNamespace": "dynamodb", "ResourceId": resource_id,
                       "ScalableDimension": "dynamodb:table:ReadCapacityUnits",
                       "MinCapacity": 1, "MaxCapacity": 100}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {table:?}: expected the target to register, got {status} {body:?}"
        );
        let (status, body) = emulator
            .call(
                "PutScalingPolicy",
                json!({"PolicyName": "tracker", "ServiceNamespace": "dynamodb",
                       "ResourceId": resource_id,
                       "ScalableDimension": "dynamodb:table:ReadCapacityUnits",
                       "PolicyType": "TargetTrackingScaling",
                       "TargetTrackingScalingPolicyConfiguration": {
                           "TargetValue": 40.0,
                           "PredefinedMetricSpecification": {
                               "PredefinedMetricType": "DynamoDBReadCapacityUtilization",
                           }}}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {table:?}: expected PutScalingPolicy to answer 200, got {status} {body:?}"
        );
        let alarms = emulator.target_tracking_alarms().await;
        let high = alarm_of(&alarms, "AlarmHigh", table);
        let threshold = high["Threshold"].as_f64().unwrap_or_default();
        assert!(
            (threshold - expected).abs() < 1e-9,
            "case {table:?}: expected 40% of {capacity} provisioned units over a 60 s period \
             ({expected}), got {threshold}"
        );
    }
}

#[tokio::test]
async fn suspending_a_direction_withholds_its_alarms_and_resuming_delivers_them() {
    let emulator = Emulator::empty();
    emulator.create_provisioned_table("orders", 1, 1).await;
    let target = json!({
        "ServiceNamespace": "dynamodb",
        "ResourceId": "table/orders",
        "ScalableDimension": "dynamodb:table:ReadCapacityUnits",
    });

    let mut registration = target.clone();
    registration["MinCapacity"] = json!(1);
    registration["MaxCapacity"] = json!(20);
    registration["SuspendedState"] = json!({"DynamicScalingOutSuspended": true});
    let (status, body) = emulator
        .call("RegisterScalableTarget", registration)
        .await;
    assert_eq!(
        status, 200,
        "case \"suspended at registration\": expected 200, got {status} {body:?}"
    );

    let mut policy = target.clone();
    policy["PolicyName"] = json!("tracker");
    policy["PolicyType"] = json!("TargetTrackingScaling");
    policy["TargetTrackingScalingPolicyConfiguration"] = json!({
        "TargetValue": 40.0,
        "PredefinedMetricSpecification": {"PredefinedMetricType": "DynamoDBReadCapacityUtilization"},
    });
    let (status, body) = emulator.call("PutScalingPolicy", policy).await;
    assert_eq!(
        status, 200,
        "case \"policy under a suspension\": expected 200, got {status} {body:?}"
    );
    assert_eq!(
        alarm_kinds(&emulator.target_tracking_alarms().await),
        ["AlarmLow", "ProvisionedCapacityLow"],
        "case \"scale-out suspended\": expected only the scale-in alarms to exist"
    );

    // Resuming the direction is a RegisterScalableTarget, and it delivers the alarms the
    // suspension withheld.
    let mut resume = target.clone();
    resume["SuspendedState"] = json!({"DynamicScalingOutSuspended": false});
    let (status, body) = emulator.call("RegisterScalableTarget", resume).await;
    assert_eq!(
        status, 200,
        "case \"resumed\": expected 200, got {status} {body:?}"
    );
    assert_eq!(
        alarm_kinds(&emulator.target_tracking_alarms().await),
        [
            "AlarmHigh",
            "AlarmLow",
            "ProvisionedCapacityHigh",
            "ProvisionedCapacityLow"
        ],
        "case \"resumed\": expected every alarm to be delivered again"
    );

    // And suspending scale-in withholds the other pair.
    let mut suspend_in = target.clone();
    suspend_in["SuspendedState"] = json!({"DynamicScalingInSuspended": true});
    let (status, body) = emulator.call("RegisterScalableTarget", suspend_in).await;
    assert_eq!(
        status, 200,
        "case \"scale-in suspended\": expected 200, got {status} {body:?}"
    );
    assert_eq!(
        alarm_kinds(&emulator.target_tracking_alarms().await),
        ["AlarmHigh", "ProvisionedCapacityHigh"],
        "case \"scale-in suspended\": expected only the scale-out alarms to exist"
    );
}

#[tokio::test]
async fn an_alarm_belongs_to_the_policy_that_minted_it() {
    let emulator = Emulator::empty();
    emulator.create_provisioned_table("orders", 1, 1).await;
    for dimension in [
        "dynamodb:table:ReadCapacityUnits",
        "dynamodb:table:WriteCapacityUnits",
    ] {
        let (status, body) = emulator
            .call(
                "RegisterScalableTarget",
                json!({"ServiceNamespace": "dynamodb", "ResourceId": "table/orders",
                       "ScalableDimension": dimension, "MinCapacity": 1, "MaxCapacity": 20}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {dimension:?}: expected the target to register, got {status} {body:?}"
        );
        let metric = if dimension.ends_with("ReadCapacityUnits") {
            "DynamoDBReadCapacityUtilization"
        } else {
            "DynamoDBWriteCapacityUtilization"
        };
        let (status, body) = emulator
            .call(
                "PutScalingPolicy",
                json!({"PolicyName": "tracker", "ServiceNamespace": "dynamodb",
                       "ResourceId": "table/orders", "ScalableDimension": dimension,
                       "PolicyType": "TargetTrackingScaling",
                       "TargetTrackingScalingPolicyConfiguration": {
                           "TargetValue": 40.0,
                           "PredefinedMetricSpecification": {"PredefinedMetricType": metric}}}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {dimension:?}: expected PutScalingPolicy to answer 200, got {status} {body:?}"
        );
    }
    assert_eq!(
        emulator.target_tracking_alarms().await.len(),
        8,
        "case \"two policies\": expected each policy to own its own four alarms"
    );

    // Deleting one policy retires its four alarms and leaves the other policy's alone.
    let (status, body) = emulator
        .call(
            "DeleteScalingPolicy",
            json!({"PolicyName": "tracker", "ServiceNamespace": "dynamodb",
                   "ResourceId": "table/orders",
                   "ScalableDimension": "dynamodb:table:ReadCapacityUnits"}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"delete one policy\": expected 200, got {status} {body:?}"
    );
    let left = emulator.target_tracking_alarms().await;
    assert_eq!(
        left.len(),
        4,
        "case \"delete one policy\": expected the sibling policy to keep its alarms, got {left:?}"
    );
    for alarm in &left {
        let metric = alarm["MetricName"].as_str().unwrap_or_default();
        assert!(
            metric.ends_with("WriteCapacityUnits"),
            "case \"delete one policy\": expected only the write policy's alarms to be left, \
             got {alarm:?}"
        );
    }

    // Deregistering the target takes the rest with it: "deregistering a scalable target
    // deletes the scaling policies […] that are associated with it".
    let (status, body) = emulator
        .call(
            "DeregisterScalableTarget",
            json!({"ServiceNamespace": "dynamodb", "ResourceId": "table/orders",
                   "ScalableDimension": "dynamodb:table:WriteCapacityUnits"}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"deregister\": expected 200, got {status} {body:?}"
    );
    assert_eq!(
        emulator.target_tracking_alarms().await,
        Vec::<Value>::new(),
        "case \"deregister\": expected a deregistered target to leave no alarm behind"
    );
}

/// Every alarm write a request makes happens with THIS service's store released, so a
/// handler can never park on a lock it is already holding.
///
/// Asserted as a wall-clock bound on a multi-threaded runtime: a re-entrant lock parks the
/// worker thread, so the timer (on another thread) is what notices. The sequence is the
/// one that writes to CloudWatch from every direction — put, re-put, delete, deregister.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_alarm_write_never_happens_under_this_services_lock() {
    let emulator = Emulator::empty();
    emulator.create_provisioned_table("orders", 1, 1).await;
    let target = json!({
        "ServiceNamespace": "dynamodb",
        "ResourceId": "table/orders",
        "ScalableDimension": "dynamodb:table:ReadCapacityUnits",
    });
    let mut policy = target.clone();
    policy["PolicyName"] = json!("tracker");
    policy["PolicyType"] = json!("TargetTrackingScaling");
    policy["TargetTrackingScalingPolicyConfiguration"] = json!({
        "TargetValue": 40.0,
        "PredefinedMetricSpecification": {"PredefinedMetricType": "DynamoDBReadCapacityUtilization"},
    });

    let sequence = async {
        let mut registration = target.clone();
        registration["MinCapacity"] = json!(1);
        registration["MaxCapacity"] = json!(20);
        for (case, operation, body) in [
            ("register", "RegisterScalableTarget", registration),
            ("put", "PutScalingPolicy", policy.clone()),
            ("re-put", "PutScalingPolicy", policy.clone()),
            ("suspend", "RegisterScalableTarget", {
                let mut resume = target.clone();
                resume["SuspendedState"] = json!({"DynamicScalingInSuspended": true});
                resume
            }),
            ("delete the policy", "DeleteScalingPolicy", {
                let mut delete = target.clone();
                delete["PolicyName"] = json!("tracker");
                delete
            }),
            ("deregister", "DeregisterScalableTarget", target.clone()),
        ] {
            let (status, body) = emulator.call(operation, body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {body:?}"
            );
        }
    };

    tokio::time::timeout(std::time::Duration::from_secs(30), sequence)
        .await
        .expect(
            "every alarm write must complete: a handler that held this service's store across \
             the CloudWatch write would park here instead",
        );
    assert_eq!(
        emulator.target_tracking_alarms().await,
        Vec::<Value>::new(),
        "case \"after the sequence\": expected the deregistered target to leave no alarm"
    );
}

/// Every value of the model's `MetricType` enum mints alarms, on a dimension it is
/// published FOR.
///
/// The predefined-metric catalog is what turns a `PredefinedMetricType` into a metric to
/// alarm on, and a type with no row in it would be ACCEPTED by the configuration check and
/// then mint nothing — a 200 that provisions nothing, the very symptom this issue is
/// about. So the catalog's coverage is pinned here against the model's own value list
/// rather than trusted: every metric type has to publish at least one scalable dimension,
/// and a policy for it on that dimension has to mint the alarms that drive it.
#[tokio::test]
async fn every_modelled_metric_type_mints_alarms() {
    use service_com_amazonaws_applicationautoscaling_anyscalefrontendservice::ops::types::shared_types::METRIC_TYPE_VALUES;
    use service_com_amazonaws_applicationautoscaling_anyscalefrontendservice::state::{
        predefined_metric_dimensions, predefined_metric_target_range,
    };

    let emulator = Emulator::empty();

    for metric_type in METRIC_TYPE_VALUES {
        // A metric describes the dimensions its own service publishes it for; one that
        // publishes none could never drive a policy.
        let dimensions = predefined_metric_dimensions(metric_type).unwrap_or_default();
        let dimension = *dimensions.first().unwrap_or_else(|| {
            panic!("case {metric_type:?}: expected the catalog to publish a scalable dimension")
        });
        let target = emulator.given_target(dimension, 1, 10).await;

        let mut specification = json!({"PredefinedMetricType": metric_type});
        // The one metric type that names a resource beside the scalable target, and
        // therefore cannot be put without one.
        if *metric_type == "ALBRequestCountPerTarget" {
            specification["ResourceLabel"] = json!(
                "app/my-load-balancer/50dc6c495c0c9188/targetgroup/my-targets/73e2d6bc24d8a067"
            );
        }
        // A target value inside the window the metric publishes, where it publishes one.
        let target_value = match predefined_metric_target_range(metric_type) {
            Some((low, high)) => (low + high) / 2.0,
            None => 50.0,
        };
        let mut request = target.clone();
        request["PolicyName"] = json!(format!("track-{metric_type}"));
        request["PolicyType"] = json!("TargetTrackingScaling");
        request["TargetTrackingScalingPolicyConfiguration"] = json!({
            "TargetValue": target_value,
            "PredefinedMetricSpecification": specification,
        });
        let (status, body) = emulator.call("PutScalingPolicy", request).await;
        assert_eq!(
            status, 200,
            "case {metric_type:?}: expected PutScalingPolicy to answer 200, got {status} {body:?}"
        );
        let alarms = body["Alarms"].as_array().cloned().unwrap_or_default();
        assert!(
            !alarms.is_empty(),
            "case {metric_type:?}: expected the policy to mint alarms, got {body:?}"
        );
        // Both directions are enabled, so the metric's own two alarms are the minimum.
        assert!(
            alarms.len() >= 2,
            "case {metric_type:?}: expected an alarm per scaling direction, got {alarms:?}"
        );
    }
}

/// A policy configuration that cannot be evaluated: the case, the policy type it is put
/// under, and the configuration member's value.
struct RefusedConfiguration {
    case: &'static str,
    policy_type: &'static str,
    configuration: fn() -> Value,
}

/// The refused configurations.
///
/// Each row is refused with 400 `ValidationException`: the rules are the model's own —
/// "you can specify either a predefined metric or a customized metric", "you can't specify
/// a resource label unless the metric type is ALBRequestCountPerTarget", "at least one step
/// adjustment is required" and "the upper and lower bound can't be null in the same step
/// adjustment".
const REFUSED_CONFIGURATIONS: &[RefusedConfiguration] = &[
    RefusedConfiguration {
        case: "both metric specifications at once",
        policy_type: "TargetTrackingScaling",
        configuration: || {
            json!({"TargetValue": 40.0,
                   "PredefinedMetricSpecification": {"PredefinedMetricType": "DynamoDBReadCapacityUtilization"},
                   "CustomizedMetricSpecification": {"MetricName": "Queue", "Namespace": "Mine", "Statistic": "Average"}})
        },
    },
    RefusedConfiguration {
        case: "neither metric specification",
        policy_type: "TargetTrackingScaling",
        configuration: || json!({"TargetValue": 40.0}),
    },
    RefusedConfiguration {
        case: "a request-count policy without a ResourceLabel",
        policy_type: "TargetTrackingScaling",
        configuration: || {
            json!({"TargetValue": 1000.0,
                   "PredefinedMetricSpecification": {"PredefinedMetricType": "ALBRequestCountPerTarget"}})
        },
    },
    RefusedConfiguration {
        case: "a ResourceLabel on a metric that is not per target",
        policy_type: "TargetTrackingScaling",
        configuration: || {
            json!({"TargetValue": 40.0,
                   "PredefinedMetricSpecification": {
                       "PredefinedMetricType": "DynamoDBReadCapacityUtilization",
                       "ResourceLabel": "app/my-alb/0/targetgroup/my-tg/0"}})
        },
    },
    RefusedConfiguration {
        case: "a predefined metric type outside the enum",
        policy_type: "TargetTrackingScaling",
        configuration: || {
            json!({"TargetValue": 40.0,
                   "PredefinedMetricSpecification": {"PredefinedMetricType": "NotAMetric"}})
        },
    },
    RefusedConfiguration {
        case: "a step policy with no step adjustments",
        policy_type: "StepScaling",
        configuration: || json!({"AdjustmentType": "ChangeInCapacity", "StepAdjustments": []}),
    },
    RefusedConfiguration {
        case: "a step adjustment with neither bound",
        policy_type: "StepScaling",
        configuration: || {
            json!({"AdjustmentType": "ChangeInCapacity",
                   "StepAdjustments": [{"ScalingAdjustment": 1}]})
        },
    },
    RefusedConfiguration {
        case: "two step adjustments with a null lower bound",
        policy_type: "StepScaling",
        configuration: || {
            json!({"AdjustmentType": "ChangeInCapacity",
                   "StepAdjustments": [
                       {"MetricIntervalUpperBound": 0.0, "ScalingAdjustment": -1},
                       {"MetricIntervalUpperBound": 10.0, "ScalingAdjustment": 1}]})
        },
    },
    RefusedConfiguration {
        case: "an adjustment type outside the enum",
        policy_type: "StepScaling",
        configuration: || {
            json!({"AdjustmentType": "NotAnAdjustment",
                   "StepAdjustments": [{"MetricIntervalLowerBound": 0.0, "ScalingAdjustment": 1}]})
        },
    },
];

#[tokio::test]
async fn a_policy_configuration_that_cannot_be_evaluated_is_refused_and_mints_nothing() {
    let emulator = Emulator::empty();
    emulator.create_provisioned_table("orders", 1, 1).await;
    let (status, body) = emulator
        .call(
            "RegisterScalableTarget",
            json!({"ServiceNamespace": "dynamodb", "ResourceId": "table/orders",
                   "ScalableDimension": "dynamodb:table:ReadCapacityUnits",
                   "MinCapacity": 1, "MaxCapacity": 20}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"register\": expected 200, got {status} {body:?}"
    );

    for row in REFUSED_CONFIGURATIONS {
        let case = row.case;
        let member = if row.policy_type == "StepScaling" {
            "StepScalingPolicyConfiguration"
        } else {
            "TargetTrackingScalingPolicyConfiguration"
        };
        let mut request = json!({
            "PolicyName": "tracker",
            "ServiceNamespace": "dynamodb",
            "ResourceId": "table/orders",
            "ScalableDimension": "dynamodb:table:ReadCapacityUnits",
            "PolicyType": row.policy_type,
        });
        request[member] = (row.configuration)();
        let (status, body) = emulator.call("PutScalingPolicy", request).await;
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
            emulator.target_tracking_alarms().await,
            Vec::<Value>::new(),
            "case {case:?}: expected a refused policy to mint no alarm"
        );
    }

    // A customised specification that is only a metric math query names no single series,
    // so the alarms that would drive the policy cannot be materialised: AWS accepts the
    // form, so it is answered with the scaffold's 501 rather than a 200 that provisions
    // nothing. Put on an ECS service, because a customized metric is not a signal a
    // DynamoDB capacity dimension can be scaled from at all.
    let ecs_target = emulator.given_target("ecs:service:DesiredCount", 1, 20).await;
    let mut query = ecs_target;
    query["PolicyName"] = json!("tracker");
    query["PolicyType"] = json!("TargetTrackingScaling");
    query["TargetTrackingScalingPolicyConfiguration"] = json!({
        "TargetValue": 40.0,
        "CustomizedMetricSpecification": {
            "Metrics": [{
                "Id": "m1",
                "Expression": "SUM(METRICS())",
                "ReturnData": true,
            }]},
    });
    let (status, body) = emulator.call("PutScalingPolicy", query).await;
    assert_eq!(
        status, 501,
        "case \"a metric math query\": expected HTTP 501, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "NotImplemented",
        "case \"a metric math query\": expected a NotImplemented, got {body:?}"
    );
    assert_eq!(
        emulator.target_tracking_alarms().await,
        Vec::<Value>::new(),
        "case \"a metric math query\": expected it to mint no alarm"
    );
}
