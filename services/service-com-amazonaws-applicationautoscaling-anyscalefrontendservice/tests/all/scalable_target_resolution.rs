// SPDX-License-Identifier: CC0-1.0

//! What a scalable target may be registered on, and what a refusal says.
//!
//! Three rules, each asserted over a table rather than over the one case the issue cited:
//!
//! * `ResourceId` and `ScalableDimension` describe ONE resource between them — the
//!   dimension is `<namespace>:<resource type>:<property>` and the resource id "consists of
//!   the resource type and unique identifier" — so a resource id that is not of the form
//!   that resource type documents names no resource, and is refused with 400
//!   `ValidationException`;
//! * a scalable target is a capacity dimension OF A RESOURCE, so registering one resolves
//!   that resource in the service that owns it: a DynamoDB table that does not exist, one
//!   in on-demand billing mode (no provisioned capacity to scale) and a global secondary
//!   index the table does not carry are all refused;
//! * deregistering a target RETIRES it, so registering the same identity again mints a new
//!   target — a new `ScalableTargetARN`, with none of the retired target's tags or
//!   policies.
//!
//! The refusal messages the suite pins are the ones built from the request: they name the
//! resource the request asked to scale, which is what a caller fixes the request from. What
//! each of them SAYS is pinned by the `refusal_messages` suite, and the refusals of the
//! service's OTHER rules by `refusal_texts`.

use serde_json::json;

use super::emulator::{error_code, error_message, Emulator, REGION};

/// What the backing service holds before the registration under test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Setup {
    /// Nothing: the resource the registration names does not exist.
    Nothing,
    /// A PROVISIONED DynamoDB table named `orders`.
    ProvisionedTable,
    /// A PAY_PER_REQUEST DynamoDB table named `orders`.
    OnDemandTable,
    /// A PROVISIONED table `orders` carrying the global secondary index `by-customer`.
    TableWithIndex,
}

/// One registration, and whether it names a resource at all.
struct ResourceRow {
    case: &'static str,
    namespace: &'static str,
    resource_id: &'static str,
    dimension: &'static str,
    setup: Setup,
    /// `true` when the registration must be accepted.
    accepted: bool,
}

/// The catalog. The accepted rows walk the `ResourceId` forms the model's own documentation
/// gives for a span of resource types — the ECS service, the Lambda function alias, the
/// Keyspaces table, the SageMaker variant, the RDS cluster, the custom resource with no
/// documented form at all — and the refused rows pair each with a resource id of the wrong
/// shape, plus the three DynamoDB resources that cannot carry the target.
const RESOURCE_CATALOG: &[ResourceRow] = &[
    ResourceRow {
        case: "a provisioned table carries a table capacity dimension",
        namespace: "dynamodb",
        resource_id: "table/orders",
        dimension: "dynamodb:table:ReadCapacityUnits",
        setup: Setup::ProvisionedTable,
        accepted: true,
    },
    ResourceRow {
        case: "a table that does not exist",
        namespace: "dynamodb",
        resource_id: "table/orders",
        dimension: "dynamodb:table:ReadCapacityUnits",
        setup: Setup::Nothing,
        accepted: false,
    },
    ResourceRow {
        case: "an on-demand table has no provisioned capacity to scale",
        namespace: "dynamodb",
        resource_id: "table/orders",
        dimension: "dynamodb:table:WriteCapacityUnits",
        setup: Setup::OnDemandTable,
        accepted: false,
    },
    ResourceRow {
        case: "a table resource id paired with an index dimension",
        namespace: "dynamodb",
        resource_id: "table/orders",
        dimension: "dynamodb:index:ReadCapacityUnits",
        setup: Setup::ProvisionedTable,
        accepted: false,
    },
    ResourceRow {
        case: "an index resource id paired with a table dimension",
        namespace: "dynamodb",
        resource_id: "table/orders/index/by-customer",
        dimension: "dynamodb:table:ReadCapacityUnits",
        setup: Setup::TableWithIndex,
        accepted: false,
    },
    ResourceRow {
        case: "a global secondary index the table does not carry",
        namespace: "dynamodb",
        resource_id: "table/orders/index/by-nothing",
        dimension: "dynamodb:index:ReadCapacityUnits",
        setup: Setup::TableWithIndex,
        accepted: false,
    },
    ResourceRow {
        case: "the global secondary index the table does carry",
        namespace: "dynamodb",
        resource_id: "table/orders/index/by-customer",
        dimension: "dynamodb:index:WriteCapacityUnits",
        setup: Setup::TableWithIndex,
        accepted: true,
    },
    ResourceRow {
        case: "an ECS service names its cluster and its service",
        namespace: "ecs",
        resource_id: "service/prod-cluster/web-app",
        dimension: "ecs:service:DesiredCount",
        setup: Setup::Nothing,
        accepted: true,
    },
    ResourceRow {
        case: "an ECS service id without its cluster",
        namespace: "ecs",
        resource_id: "service/web-app",
        dimension: "ecs:service:DesiredCount",
        setup: Setup::Nothing,
        accepted: false,
    },
    ResourceRow {
        case: "a Lambda function with a qualifier",
        namespace: "lambda",
        resource_id: "function:ingest:green",
        dimension: "lambda:function:ProvisionedConcurrency",
        setup: Setup::Nothing,
        accepted: true,
    },
    ResourceRow {
        case: "a Lambda function without a qualifier",
        namespace: "lambda",
        resource_id: "function:ingest",
        dimension: "lambda:function:ProvisionedConcurrency",
        setup: Setup::Nothing,
        accepted: false,
    },
    ResourceRow {
        case: "a Keyspaces table names its keyspace",
        namespace: "cassandra",
        resource_id: "keyspace/events/table/clicks",
        dimension: "cassandra:table:ReadCapacityUnits",
        setup: Setup::Nothing,
        accepted: true,
    },
    ResourceRow {
        case: "a Keyspaces table in the DynamoDB form",
        namespace: "cassandra",
        resource_id: "table/clicks",
        dimension: "cassandra:table:ReadCapacityUnits",
        setup: Setup::Nothing,
        accepted: false,
    },
    ResourceRow {
        case: "a SageMaker endpoint variant",
        namespace: "sagemaker",
        resource_id: "endpoint/my-endpoint/variant/KMeansClustering",
        dimension: "sagemaker:variant:DesiredInstanceCount",
        setup: Setup::Nothing,
        accepted: true,
    },
    ResourceRow {
        case: "a SageMaker variant without its endpoint",
        namespace: "sagemaker",
        resource_id: "variant/KMeansClustering",
        dimension: "sagemaker:variant:DesiredInstanceCount",
        setup: Setup::Nothing,
        accepted: false,
    },
    ResourceRow {
        case: "an RDS cluster, which is colon-separated",
        namespace: "rds",
        resource_id: "cluster:my-db-cluster",
        dimension: "rds:cluster:ReadReplicaCount",
        setup: Setup::Nothing,
        accepted: true,
    },
    ResourceRow {
        case: "an RDS cluster written with a slash",
        namespace: "rds",
        resource_id: "cluster/my-db-cluster",
        dimension: "rds:cluster:ReadReplicaCount",
        setup: Setup::Nothing,
        accepted: false,
    },
    ResourceRow {
        case: "an MSK cluster is named by its ARN",
        namespace: "kafka",
        resource_id: "arn:aws:kafka:us-east-1:000000000000:cluster/demo/6357e0b2-1",
        dimension: "kafka:broker-storage:VolumeSize",
        setup: Setup::Nothing,
        accepted: true,
    },
    ResourceRow {
        case: "an MSK cluster named by its bare name",
        namespace: "kafka",
        resource_id: "cluster/demo",
        dimension: "kafka:broker-storage:VolumeSize",
        setup: Setup::Nothing,
        accepted: false,
    },
    ResourceRow {
        case: "a custom resource has no documented form, so any id is its own",
        namespace: "custom-resource",
        resource_id: "whatever-the-provider-says",
        dimension: "custom-resource:ResourceType:Property",
        setup: Setup::Nothing,
        accepted: true,
    },
];

/// Create what `setup` describes.
async fn prepare(emulator: &Emulator, setup: Setup) {
    match setup {
        Setup::Nothing => {}
        Setup::ProvisionedTable => emulator.create_provisioned_table("orders", 1, 1).await,
        Setup::OnDemandTable => emulator.create_on_demand_table("orders").await,
        Setup::TableWithIndex => {
            emulator
                .create_table_with_index("orders", "by-customer", "customer", 2, 2)
                .await;
        }
    }
}

#[tokio::test]
async fn a_target_is_registered_only_on_a_resource_that_can_carry_it() {
    for row in RESOURCE_CATALOG {
        let case = row.case;
        let emulator = Emulator::empty();
        prepare(&emulator, row.setup).await;

        let (status, body) = emulator
            .call(
                "RegisterScalableTarget",
                json!({
                    "ServiceNamespace": row.namespace,
                    "ResourceId": row.resource_id,
                    "ScalableDimension": row.dimension,
                    "MinCapacity": 1,
                    "MaxCapacity": 10,
                }),
            )
            .await;

        if row.accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the registration to be accepted, got {status} {body:?}"
            );
            assert!(
                body["ScalableTargetARN"].as_str().is_some(),
                "case {case:?}: expected a ScalableTargetARN, got {body:?}"
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
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
        let message = error_message(&body);
        assert!(
            message.contains(row.resource_id),
            "case {case:?}: expected the message to name the resource id the request \
             supplied, got {message:?}"
        );
        // The refusal is the service's own sentence: never the boundary validator's
        // constraint template, and never a value the caller did not supply (the region the
        // request happens to be signed for).
        assert!(
            !message.contains("failed to satisfy constraint") && !message.contains(REGION),
            "case {case:?}: expected the service's own refusal text, got {message:?}"
        );
        // And nothing was persisted.
        let (status, described) = emulator
            .call(
                "DescribeScalableTargets",
                json!({"ServiceNamespace": row.namespace}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeScalableTargets to answer 200, got {described:?}"
        );
        assert_eq!(
            described["ScalableTargets"],
            json!([]),
            "case {case:?}: expected a refused registration to persist nothing, got {described:?}"
        );
    }
}

#[tokio::test]
async fn re_registering_after_a_deregister_mints_a_new_target() {
    let emulator = Emulator::empty();
    emulator.create_provisioned_table("orders", 1, 1).await;
    let identity = json!({
        "ServiceNamespace": "dynamodb",
        "ResourceId": "table/orders",
        "ScalableDimension": "dynamodb:table:ReadCapacityUnits",
    });

    let mut first = identity.clone();
    first["MinCapacity"] = json!(1);
    first["MaxCapacity"] = json!(20);
    first["Tags"] = json!({"owner": "orders-team"});
    let (status, body) = emulator.call("RegisterScalableTarget", first).await;
    assert_eq!(
        status, 200,
        "case \"first registration\": expected 200, got {status} {body:?}"
    );
    let old_arn = body["ScalableTargetARN"]
        .as_str()
        .expect("a ScalableTargetARN")
        .to_string();

    let mut policy = identity.clone();
    policy["PolicyName"] = json!("tracker");
    policy["PolicyType"] = json!("TargetTrackingScaling");
    policy["TargetTrackingScalingPolicyConfiguration"] = json!({
        "TargetValue": 40.0,
        "PredefinedMetricSpecification": {"PredefinedMetricType": "DynamoDBReadCapacityUtilization"},
    });
    let (status, body) = emulator.call("PutScalingPolicy", policy).await;
    assert_eq!(
        status, 200,
        "case \"policy on the first target\": expected 200, got {status} {body:?}"
    );

    let (status, body) = emulator
        .call("DeregisterScalableTarget", identity.clone())
        .await;
    assert_eq!(
        status, 200,
        "case \"deregister\": expected 200, got {status} {body:?}"
    );

    let mut again = identity.clone();
    again["MinCapacity"] = json!(2);
    again["MaxCapacity"] = json!(30);
    let (status, body) = emulator.call("RegisterScalableTarget", again).await;
    assert_eq!(
        status, 200,
        "case \"re-registration\": expected 200, got {status} {body:?}"
    );
    let new_arn = body["ScalableTargetARN"]
        .as_str()
        .expect("a ScalableTargetARN")
        .to_string();
    assert_ne!(
        new_arn, old_arn,
        "case \"re-registration\": expected a new scalable target to carry a new ARN"
    );

    // The new target has none of the retired target's associations.
    let (status, body) = emulator
        .call(
            "DescribeScalingPolicies",
            json!({"ServiceNamespace": "dynamodb"}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"policies after re-registration\": expected 200, got {status} {body:?}"
    );
    assert_eq!(
        body["ScalingPolicies"],
        json!([]),
        "case \"policies after re-registration\": expected the retired target's policy to be \
         gone, got {body:?}"
    );
    let (status, body) = emulator
        .call(
            "DescribeScalableTargets",
            json!({"ServiceNamespace": "dynamodb"}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"targets after re-registration\": expected 200, got {status} {body:?}"
    );
    let targets = body["ScalableTargets"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        targets.len(),
        1,
        "case \"targets after re-registration\": expected exactly the new target, got {targets:?}"
    );
    assert_eq!(
        targets[0]["ScalableTargetARN"],
        json!(new_arn),
        "case \"targets after re-registration\": expected the new ARN to be what Describe \
         reports, got {targets:?}"
    );
    assert_eq!(
        targets[0]["MinCapacity"],
        json!(2),
        "case \"targets after re-registration\": expected the new target's own bounds, got \
         {targets:?}"
    );
}

#[tokio::test]
async fn a_refusal_names_the_member_the_request_got_wrong() {
    let emulator = Emulator::empty();
    emulator.create_provisioned_table("orders", 1, 1).await;

    // A lookup of a target that was never registered names the whole identity it looked
    // for, so the caller can see WHICH target is missing.
    let (status, body) = emulator
        .call(
            "PutScalingPolicy",
            json!({"PolicyName": "tracker", "ServiceNamespace": "dynamodb",
                   "ResourceId": "table/orders",
                   "ScalableDimension": "dynamodb:table:ReadCapacityUnits",
                   "PolicyType": "TargetTrackingScaling",
                   "TargetTrackingScalingPolicyConfiguration": {
                       "TargetValue": 40.0,
                       "PredefinedMetricSpecification": {
                           "PredefinedMetricType": "DynamoDBReadCapacityUtilization"}}}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"policy on an unregistered target\": expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ObjectNotFoundException",
        "case \"policy on an unregistered target\": expected an ObjectNotFoundException, got \
         {body:?}"
    );
    assert_eq!(
        error_message(&body),
        "No scalable target found for service namespace: dynamodb, resource ID: \
         table/orders, scalable dimension: dynamodb:table:ReadCapacityUnits",
        "case \"policy on an unregistered target\": expected the message to name the identity \
         it looked for, got {body:?}"
    );

    // A first registration must carry both bounds, and the refusal names the one that is
    // missing rather than both.
    for (case, request, expected) in [
        (
            "neither bound",
            json!({"ServiceNamespace": "dynamodb", "ResourceId": "table/orders",
                   "ScalableDimension": "dynamodb:table:ReadCapacityUnits"}),
            "Minimum capacity must be specified",
        ),
        (
            "no MinCapacity",
            json!({"ServiceNamespace": "dynamodb", "ResourceId": "table/orders",
                   "ScalableDimension": "dynamodb:table:ReadCapacityUnits", "MaxCapacity": 10}),
            "Minimum capacity must be specified",
        ),
        (
            "no MaxCapacity",
            json!({"ServiceNamespace": "dynamodb", "ResourceId": "table/orders",
                   "ScalableDimension": "dynamodb:table:ReadCapacityUnits", "MinCapacity": 1}),
            "Maximum capacity must be specified",
        ),
    ] {
        let (status, body) = emulator.call("RegisterScalableTarget", request).await;
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
            error_message(&body),
            expected,
            "case {case:?}: expected the refusal to name the missing member, got {body:?}"
        );
    }
}

#[tokio::test]
async fn a_global_secondary_index_round_trips_through_describe_table() {
    // The index a `dynamodb:index:*` target is registered on is real state in DynamoDB,
    // which is what makes the index check above a resolution rather than a refusal.
    let emulator = Emulator::empty();
    emulator
        .create_table_with_index("orders", "by-customer", "customer", 3, 4)
        .await;
    let (status, body) = emulator
        .call_dynamodb("DescribeTable", json!({"TableName": "orders"}))
        .await;
    assert_eq!(
        status, 200,
        "case \"describe\": expected DescribeTable to answer 200, got {status} {body:?}"
    );
    let indexes = body["Table"]["GlobalSecondaryIndexes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        indexes.len(),
        1,
        "case \"describe\": expected the index the table was created with, got {body:?}"
    );
    let index = &indexes[0];
    assert_eq!(
        index["IndexName"],
        json!("by-customer"),
        "case \"describe\": expected the index name, got {index:?}"
    );
    assert_eq!(
        index["KeySchema"],
        json!([{"AttributeName": "customer", "KeyType": "HASH"}]),
        "case \"describe\": expected the index key schema, got {index:?}"
    );
    assert_eq!(
        index["Projection"],
        json!({"ProjectionType": "ALL"}),
        "case \"describe\": expected the projection, got {index:?}"
    );
    assert_eq!(
        index["ProvisionedThroughput"]["ReadCapacityUnits"],
        json!(3),
        "case \"describe\": expected the index's own read capacity, got {index:?}"
    );
    assert_eq!(
        index["ProvisionedThroughput"]["WriteCapacityUnits"],
        json!(4),
        "case \"describe\": expected the index's own write capacity, got {index:?}"
    );
    assert_eq!(
        index["ItemCount"],
        json!(0),
        "case \"describe\": expected an empty index to hold no item, got {index:?}"
    );
    let arn = body["Table"]["TableArn"].as_str().unwrap_or_default();
    assert_eq!(
        index["IndexArn"],
        json!(format!("{arn}/index/by-customer")),
        "case \"describe\": expected the index ARN under the table's, got {index:?}"
    );

    // A refused index is refused on the same reading: an index name the table does carry
    // resolves, one it does not is a ValidationException naming it.
    let (status, body) = emulator
        .call(
            "RegisterScalableTarget",
            json!({"ServiceNamespace": "dynamodb",
                   "ResourceId": "table/orders/index/by-nothing",
                   "ScalableDimension": "dynamodb:index:ReadCapacityUnits",
                   "MinCapacity": 1, "MaxCapacity": 10}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"missing index\": expected HTTP 400, got {status} {body:?}"
    );
    assert!(
        error_message(&body).contains("by-nothing"),
        "case \"missing index\": expected the message to name the index, got {body:?}"
    );
}

/// What `ItemCount` reports is the METERED figure, not a live count: "DynamoDB updates this
/// value approximately every six hours. Recent changes might not be reflected in this value"
/// (`TableDescription$ItemCount`, and the same sentence on an index's `IndexSizeBytes`). The
/// projection rule those figures are computed from — an item is in an index only when it
/// carries the index's key attribute — is pinned by the DynamoDB crate's own
/// `table_description_members` suite, which drives the refresh the clock owes.
#[tokio::test]
async fn the_item_figures_a_table_reports_are_the_metered_ones() {
    let emulator = Emulator::empty();
    emulator
        .create_table_with_index("orders", "by-customer", "customer", 1, 1)
        .await;
    // "You can perform read and write operations only on an ACTIVE table": a new table is
    // CREATING for the span the clock owes it.
    std::thread::sleep(
        service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION
            + std::time::Duration::from_millis(200),
    );
    for (case, item) in [
        ("with the index key", json!({"pk": {"S": "1"}, "customer": {"S": "ann"}})),
        ("without the index key", json!({"pk": {"S": "2"}})),
    ] {
        let (status, body) = emulator
            .call_dynamodb("PutItem", json!({"TableName": "orders", "Item": item}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected PutItem to answer 200, got {status} {body:?}"
        );
    }

    // The figures a description reports are the METERED ones, which those writes do not
    // move: "DynamoDB updates this value approximately every six hours. Recent changes
    // might not be reflected in this value" (the `TableDescription$ItemCount` doc).
    let (status, body) = emulator
        .call_dynamodb("DescribeTable", json!({"TableName": "orders"}))
        .await;
    assert_eq!(
        status, 200,
        "case \"describe\": expected DescribeTable to answer 200, got {status} {body:?}"
    );
    assert_eq!(
        body["Table"]["ItemCount"],
        json!(0),
        "case \"describe\": expected the metered ItemCount, which the two writes do not \
         move, got {body:?}"
    );
    assert_eq!(
        body["Table"]["GlobalSecondaryIndexes"][0]["ItemCount"],
        json!(0),
        "case \"describe\": expected the index's metered ItemCount, got {body:?}"
    );
}
