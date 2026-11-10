// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the replica AUTO SCALING surface:
//! `UpdateTableReplicaAutoScaling` and `DescribeTableReplicaAutoScaling`.
//!
//! The rule these tests pin:
//!
//! > an auto scaling update writes the capacity window and target-tracking policy each of
//! > its members names — the WRITE capacity it states for the table to every replica of
//! > the replication group, and the READ capacity a `ReplicaUpdates` entry states to the
//! > Region that entry names, with the same split for each global secondary index — a
//! > member it leaves out keeps the setting it had, and the describing operation reports
//! > exactly what is stored for every replica of the group, reporting no settings at all
//! > for a capacity nothing has configured.
//!
//! The rows of each table below differ along what the rule generalises over: which
//! capacity an update names (the table's read or write, an index's read or write), the
//! Region it addresses, the window and target values it carries, and whether it names the
//! policy or leaves the service to name it.
//!
//! Every test drives the operations over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call_in, emulator, error_code, message, ok_in, REGION};

/// The Region the replica of every global table below lives in.
const REPLICA_REGION: &str = "eu-west-1";

/// The global secondary index every table below carries.
const INDEX: &str = "ByStatus";

/// Wait out one transient span, so whatever the clock owes a table or a replica is due.
async fn settle() {
    tokio::time::sleep(
        service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION
            + std::time::Duration::from_millis(150),
    )
    .await;
}

/// One provisioned table with a global secondary index, created in [`REGION`] — the
/// capacities an auto scaling policy steers are provisioned ones.
async fn create_table(router: &MultiServiceRouter, name: &str) -> Value {
    ok_in(
        router,
        REGION,
        "CreateTable",
        json!({
            "TableName": name,
            "KeySchema": [{"AttributeName": "Id", "KeyType": "HASH"}],
            "AttributeDefinitions": [
                {"AttributeName": "Id", "AttributeType": "S"},
                {"AttributeName": "Status", "AttributeType": "S"},
            ],
            "BillingMode": "PROVISIONED",
            "ProvisionedThroughput": {"ReadCapacityUnits": 5, "WriteCapacityUnits": 5},
            "GlobalSecondaryIndexes": [{
                "IndexName": INDEX,
                "KeySchema": [{"AttributeName": "Status", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "ALL"},
                "ProvisionedThroughput": {"ReadCapacityUnits": 5, "WriteCapacityUnits": 5},
            }],
        }),
    )
    .await
}

/// The same table, replicated into [`REPLICA_REGION`] so its group spans two Regions.
async fn create_global_table(router: &MultiServiceRouter, name: &str) {
    create_table(router, name).await;
    settle().await;
    ok_in(
        router,
        REGION,
        "UpdateTable",
        json!({
            "TableName": name,
            "ReplicaUpdates": [{"Create": {"RegionName": REPLICA_REGION}}],
        }),
    )
    .await;
    settle().await;
}

/// One `AutoScalingSettingsUpdate` body: the capacity window, and the target-tracking
/// policy that steers the capacity inside it.
fn settings_update(minimum: i64, maximum: i64, target: f64, policy_name: Option<&str>) -> Value {
    let mut policy = json!({
        "TargetTrackingScalingPolicyConfiguration": {
            "TargetValue": target,
            "ScaleInCooldown": 60,
            "ScaleOutCooldown": 120,
            "DisableScaleIn": false,
        },
    });
    if let Some(policy_name) = policy_name {
        policy["PolicyName"] = json!(policy_name);
    }
    json!({
        "MinimumUnits": minimum,
        "MaximumUnits": maximum,
        "ScalingPolicyUpdate": policy,
    })
}

/// The `TableAutoScalingDescription` the describing operation reports for `name`.
async fn described(router: &MultiServiceRouter, name: &str) -> Value {
    ok_in(
        router,
        REGION,
        "DescribeTableReplicaAutoScaling",
        json!({"TableName": name}),
    )
    .await["TableAutoScalingDescription"]
        .clone()
}

/// The `Replicas` entry a description carries for `region`, or `Null` when it carries
/// none.
fn replica_in(description: &Value, region: &str) -> Value {
    description["Replicas"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|replica| replica["RegionName"] == json!(region))
        .unwrap_or(Value::Null)
}

/// The settings one replica reports for one capacity: the table's own under `member`, or
/// the index's when `index` names one.
fn reported_settings(
    description: &Value,
    region: &str,
    index: Option<&str>,
    member: &str,
) -> Value {
    let replica = replica_in(description, region);
    match index {
        None => replica[member].clone(),
        Some(index) => replica["GlobalSecondaryIndexes"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|entry| entry["IndexName"] == json!(index))
            .map(|entry| entry[member].clone())
            .unwrap_or(Value::Null),
    }
}

/// The window, target value and policy name one settings report states, in the compact
/// form every row below is asserted in.
fn stated(settings: &Value) -> Value {
    json!([
        settings["MinimumUnits"].clone(),
        settings["MaximumUnits"].clone(),
        settings["ScalingPolicies"][0]["TargetTrackingScalingPolicyConfiguration"]["TargetValue"]
            .clone(),
        settings["ScalingPolicies"][0]["PolicyName"].clone(),
    ])
}

/// The member of a replica report that carries each capacity's settings, as the model
/// names them (`ReplicaAutoScalingDescription` /
/// `ReplicaGlobalSecondaryIndexAutoScalingDescription`).
const REPLICA_READ: &str = "ReplicaProvisionedReadCapacityAutoScalingSettings";
const REPLICA_WRITE: &str = "ReplicaProvisionedWriteCapacityAutoScalingSettings";
const INDEX_READ: &str = "ProvisionedReadCapacityAutoScalingSettings";
const INDEX_WRITE: &str = "ProvisionedWriteCapacityAutoScalingSettings";

/// One capacity an update names, as a row of the table below: the request that scales it,
/// where the report carries it back (Region, index, member), whether the settings reach
/// the WHOLE replication group, and what the report must then state.
struct ScaledCapacity {
    case: &'static str,
    request: Value,
    region: &'static str,
    index: Option<&'static str>,
    member: &'static str,
    whole_group: bool,
    expected: Value,
}

#[tokio::test]
async fn an_update_writes_the_capacities_it_names_and_the_describe_reports_them() {
    let router = emulator();
    let name = "Scaled.capacities";
    create_global_table(&router, name).await;

    // Each row names ONE capacity: the request that scales it, where the report carries
    // it back (Region, index, member), whether the settings reach the WHOLE group, and
    // the window, target and policy name the report must then state. The policy name a
    // row leaves out is the one the service manages the capacity's policy under:
    // `DynamoDB<Read|Write>CapacityUtilization:table/<table>[/index/<index>]`, the
    // default policy name DynamoDB auto scaling publishes for a table and for an index.
    let rows = vec![
        ScaledCapacity {
            case: "the table's write capacity, for the whole group",
            request: json!({
                "TableName": name,
                "ProvisionedWriteCapacityAutoScalingUpdate":
                    settings_update(1, 10, 70.0, None),
            }),
            region: REGION,
            index: None,
            member: REPLICA_WRITE,
            whole_group: true,
            expected: json!([
                1,
                10,
                70.0,
                format!("DynamoDBWriteCapacityUtilization:table/{name}")
            ]),
        },
        ScaledCapacity {
            case: "the local replica's read capacity, with a policy the request names",
            request: json!({
                "TableName": name,
                "ReplicaUpdates": [{
                    "RegionName": REGION,
                    "ReplicaProvisionedReadCapacityAutoScalingUpdate":
                        settings_update(2, 200, 42.5, Some("reads-of-the-local-replica")),
                }],
            }),
            region: REGION,
            index: None,
            member: REPLICA_READ,
            whole_group: false,
            expected: json!([2, 200, 42.5, "reads-of-the-local-replica"]),
        },
        ScaledCapacity {
            case: "the other Region's read capacity",
            request: json!({
                "TableName": name,
                "ReplicaUpdates": [{
                    "RegionName": REPLICA_REGION,
                    "ReplicaProvisionedReadCapacityAutoScalingUpdate":
                        settings_update(5, 50, 80.0, None),
                }],
            }),
            region: REPLICA_REGION,
            index: None,
            member: REPLICA_READ,
            whole_group: false,
            expected: json!([
                5,
                50,
                80.0,
                format!("DynamoDBReadCapacityUtilization:table/{name}")
            ]),
        },
        ScaledCapacity {
            case: "the index's write capacity, for the whole group",
            request: json!({
                "TableName": name,
                "GlobalSecondaryIndexUpdates": [{
                    "IndexName": INDEX,
                    "ProvisionedWriteCapacityAutoScalingUpdate":
                        settings_update(3, 30, 55.0, None),
                }],
            }),
            region: REGION,
            index: Some(INDEX),
            member: INDEX_WRITE,
            whole_group: true,
            expected: json!([
                3,
                30,
                55.0,
                format!("DynamoDBWriteCapacityUtilization:table/{name}/index/{INDEX}")
            ]),
        },
        ScaledCapacity {
            case: "the other Region's index read capacity",
            request: json!({
                "TableName": name,
                "ReplicaUpdates": [{
                    "RegionName": REPLICA_REGION,
                    "ReplicaGlobalSecondaryIndexUpdates": [{
                        "IndexName": INDEX,
                        "ProvisionedReadCapacityAutoScalingUpdate":
                            settings_update(4, 40, 65.0, None),
                    }],
                }],
            }),
            region: REPLICA_REGION,
            index: Some(INDEX),
            member: INDEX_READ,
            whole_group: false,
            expected: json!([
                4,
                40,
                65.0,
                format!("DynamoDBReadCapacityUtilization:table/{name}/index/{INDEX}")
            ]),
        },
    ];

    for ScaledCapacity {
        case,
        request,
        region,
        index,
        member,
        whole_group,
        expected,
    } in rows
    {
        let updated = ok_in(&router, REGION, "UpdateTableReplicaAutoScaling", request).await
            ["TableAutoScalingDescription"]
            .clone();
        let settings = reported_settings(&updated, region, index, member);
        assert_eq!(
            stated(&settings),
            expected,
            "case {case:?}: the update must answer the settings it wrote, got {updated}"
        );
        // Read back through the describing operation: what the update answered is what
        // the store holds.
        let read_back = described(&router, name).await;
        assert_eq!(
            stated(&reported_settings(&read_back, region, index, member)),
            expected,
            "case {case:?}: the describe must report the settings the update wrote, got \
             {read_back}"
        );
        // A capacity the group shares is scaled on every replica; one a replica entry
        // addressed is scaled on that Region alone.
        let other = if region == REGION { REPLICA_REGION } else { REGION };
        let on_the_other = reported_settings(&read_back, other, index, member);
        if whole_group {
            assert_eq!(
                stated(&on_the_other),
                expected,
                "case {case:?}: a capacity of the global table is scaled on every replica, got \
                 {read_back}"
            );
        } else {
            assert_ne!(
                stated(&on_the_other),
                expected,
                "case {case:?}: a capacity one replica entry addressed is not scaled in {other} \
                 by it, got {read_back}"
            );
        }
    }

    // Every capacity the rows configured is still reported after the last of them: an
    // update writes what it names and leaves every other capacity exactly as it was.
    let final_report = described(&router, name).await;
    assert_eq!(
        (
            stated(&reported_settings(&final_report, REGION, None, REPLICA_WRITE)),
            stated(&reported_settings(&final_report, REGION, None, REPLICA_READ)),
            stated(&reported_settings(
                &final_report,
                REPLICA_REGION,
                None,
                REPLICA_READ
            )),
            stated(&reported_settings(
                &final_report,
                REGION,
                Some(INDEX),
                INDEX_WRITE
            )),
        ),
        (
            json!([1, 10, 70.0, format!("DynamoDBWriteCapacityUtilization:table/{name}")]),
            json!([2, 200, 42.5, "reads-of-the-local-replica"]),
            json!([5, 50, 80.0, format!("DynamoDBReadCapacityUtilization:table/{name}")]),
            json!([
                3,
                30,
                55.0,
                format!("DynamoDBWriteCapacityUtilization:table/{name}/index/{INDEX}")
            ]),
        ),
        "case every capacity configured: each keeps the settings its own update wrote, got \
         {final_report}"
    );
}

#[tokio::test]
async fn an_update_modifies_only_the_members_it_names() {
    let router = emulator();
    let name = "Scaled.modified";
    create_global_table(&router, name).await;
    ok_in(
        &router,
        REGION,
        "UpdateTableReplicaAutoScaling",
        json!({
            "TableName": name,
            "ProvisionedWriteCapacityAutoScalingUpdate":
                settings_update(2, 20, 50.0, Some("writes-of-the-group")),
        }),
    )
    .await;

    // Each row is a follow-up update that names SOME of the members, and what the whole
    // settings report must then state: the members it named, and the ones it left out
    // unchanged.
    let rows = vec![
        (
            "a window-only update keeps the policy the earlier one configured",
            json!({"MinimumUnits": 4, "MaximumUnits": 400}),
            json!([4, 400, 50.0, "writes-of-the-group"]),
        ),
        (
            "a policy-only update keeps the window",
            json!({
                "ScalingPolicyUpdate": {
                    "PolicyName": "writes-of-the-group",
                    "TargetTrackingScalingPolicyConfiguration": {"TargetValue": 81.5},
                },
            }),
            json!([4, 400, 81.5, "writes-of-the-group"]),
        ),
        (
            "an update that only turns scaling off keeps both",
            json!({"AutoScalingDisabled": true}),
            json!([4, 400, 81.5, "writes-of-the-group"]),
        ),
    ];

    for (case, update, expected) in rows {
        ok_in(
            &router,
            REGION,
            "UpdateTableReplicaAutoScaling",
            json!({
                "TableName": name,
                "ProvisionedWriteCapacityAutoScalingUpdate": update,
            }),
        )
        .await;
        let report = described(&router, name).await;
        let settings = reported_settings(&report, REGION, None, REPLICA_WRITE);
        assert_eq!(
            stated(&settings),
            expected,
            "case {case:?}: got {report}"
        );
    }

    // The last row turned scaling OFF, which the report states rather than forgetting the
    // settings it was turned off with.
    let report = described(&router, name).await;
    assert_eq!(
        reported_settings(&report, REGION, None, REPLICA_WRITE)["AutoScalingDisabled"],
        json!(true),
        "case scaling turned off: the report states that it is off, got {report}"
    );
}

#[tokio::test]
async fn a_capacity_nothing_scales_is_reported_with_no_settings() {
    let router = emulator();
    let name = "Scaled.unconfigured";
    create_table(&router, name).await;
    settle().await;

    // A table that belongs to no replication group is still a regional table of its own,
    // so it reports exactly one replica: its own Region, with no capacity configured and
    // its index listed.
    let report = described(&router, name).await;
    let replica = replica_in(&report, REGION);
    assert_eq!(
        (
            report["TableName"].clone(),
            report["TableStatus"].clone(),
            report["Replicas"].as_array().map(Vec::len),
            replica["ReplicaStatus"].clone(),
            replica[REPLICA_READ].clone(),
            replica[REPLICA_WRITE].clone(),
            replica["GlobalSecondaryIndexes"][0]["IndexName"].clone(),
            replica["GlobalSecondaryIndexes"][0][INDEX_READ].clone(),
        ),
        (
            json!(name),
            json!("ACTIVE"),
            Some(1),
            json!("ACTIVE"),
            Value::Null,
            Value::Null,
            json!(INDEX),
            Value::Null,
        ),
        "case a table nothing scales: every capacity is reported as configured by nothing, got \
         {report}"
    );

    // Configuring ONE capacity leaves the others reported by nothing still.
    ok_in(
        &router,
        REGION,
        "UpdateTableReplicaAutoScaling",
        json!({
            "TableName": name,
            "ReplicaUpdates": [{
                "RegionName": REGION,
                "ReplicaProvisionedReadCapacityAutoScalingUpdate":
                    settings_update(1, 11, 60.0, None),
            }],
        }),
    )
    .await;
    let report = described(&router, name).await;
    let replica = replica_in(&report, REGION);
    assert_eq!(
        (
            replica[REPLICA_READ]["MaximumUnits"].clone(),
            replica[REPLICA_WRITE].clone(),
            replica["GlobalSecondaryIndexes"][0][INDEX_READ].clone(),
        ),
        (json!(11), Value::Null, Value::Null),
        "case one capacity configured: the others are still configured by nothing, got {report}"
    );
}

/// One request the surface cannot apply, as a row of the refusal table: the operation it
/// is sent to, and the HTTP status and error code it is refused with.
struct Refusal {
    case: &'static str,
    operation: &'static str,
    request: Value,
    status: u16,
    code: &'static str,
}

#[tokio::test]
async fn the_replica_auto_scaling_surface_refuses_what_it_cannot_address() {
    let router = emulator();
    let name = "Scaled.refusals";
    create_global_table(&router, name).await;

    // Each row is a request that cannot apply, and the HTTP status and error code it is
    // refused with: a table or index the request addresses that is not there is the
    // `ResourceNotFoundException` both operations model, and a Region the group does not
    // hold is a contradiction in the request itself.
    let rows = vec![
        Refusal {
            case: "describing a table that is not there",
            operation: "DescribeTableReplicaAutoScaling",
            request: json!({"TableName": "Scaled.absent"}),
            status: 400,
            code: "ResourceNotFoundException",
        },
        Refusal {
            case: "scaling a table that is not there",
            operation: "UpdateTableReplicaAutoScaling",
            request: json!({
                "TableName": "Scaled.absent",
                "ProvisionedWriteCapacityAutoScalingUpdate": settings_update(1, 2, 70.0, None),
            }),
            status: 400,
            code: "ResourceNotFoundException",
        },
        Refusal {
            case: "scaling an index the table does not carry",
            operation: "UpdateTableReplicaAutoScaling",
            request: json!({
                "TableName": name,
                "GlobalSecondaryIndexUpdates": [{
                    "IndexName": "NoSuchIndex",
                    "ProvisionedWriteCapacityAutoScalingUpdate": settings_update(1, 2, 70.0, None),
                }],
            }),
            status: 400,
            code: "ResourceNotFoundException",
        },
        Refusal {
            case: "scaling a replica index the table does not carry",
            operation: "UpdateTableReplicaAutoScaling",
            request: json!({
                "TableName": name,
                "ReplicaUpdates": [{
                    "RegionName": REPLICA_REGION,
                    "ReplicaGlobalSecondaryIndexUpdates": [{
                        "IndexName": "NoSuchIndex",
                        "ProvisionedReadCapacityAutoScalingUpdate":
                            settings_update(1, 2, 70.0, None),
                    }],
                }],
            }),
            status: 400,
            code: "ResourceNotFoundException",
        },
        Refusal {
            case: "scaling a Region the replication group does not hold",
            operation: "UpdateTableReplicaAutoScaling",
            request: json!({
                "TableName": name,
                "ReplicaUpdates": [{
                    "RegionName": "ap-south-1",
                    "ReplicaProvisionedReadCapacityAutoScalingUpdate":
                        settings_update(1, 2, 70.0, None),
                }],
            }),
            status: 400,
            code: "ValidationException",
        },
        Refusal {
            case: "scaling an index entry that names no index",
            operation: "UpdateTableReplicaAutoScaling",
            request: json!({
                "TableName": name,
                "GlobalSecondaryIndexUpdates": [{
                    "ProvisionedWriteCapacityAutoScalingUpdate": settings_update(1, 2, 70.0, None),
                }],
            }),
            status: 400,
            code: "ValidationException",
        },
    ];

    for Refusal {
        case,
        operation,
        request,
        status,
        code,
    } in rows
    {
        let (answered, refusal) = call_in(&router, REGION, operation, request).await;
        assert_eq!(
            (answered, error_code(&refusal).as_str()),
            (status, code),
            "case {case:?}: expected HTTP {status} {code}, got {answered} with {refusal}"
        );
    }

    // A refused update leaves every capacity exactly as it was: nothing is written until
    // every Region and index the request names has been resolved.
    let report = described(&router, name).await;
    assert_eq!(
        (
            reported_settings(&report, REGION, None, REPLICA_WRITE),
            reported_settings(&report, REPLICA_REGION, None, REPLICA_READ),
        ),
        (Value::Null, Value::Null),
        "case refusals leave the settings alone: no refused update configured a capacity, got \
         {report}"
    );
}

#[tokio::test]
async fn a_capacity_is_scaled_through_the_service_linked_role_alone() {
    let router = emulator();
    let name = "Scaled.roles";
    create_global_table(&router, name).await;

    // The role a replica's capacities are scaled through, which every configured capacity
    // reports whether or not the request named it.
    let in_force =
        service_com_amazonaws_dynamodb_dynamodb_20120810::state::auto_scaling_service_linked_role_arn();
    let mut named_in_force = settings_update(1, 10, 70.0, None);
    named_in_force["AutoScalingRoleArn"] = json!(in_force);
    let mut named_another = settings_update(1, 10, 70.0, None);
    named_another["AutoScalingRoleArn"] = json!(::engine::emulator_arn(
        "iam",
        "",
        "role/a-role-of-the-caller-s-own"
    ));

    // Each row names a role, or leaves the member out, and says whether the update is
    // served: the role in force is the one the service assumes on its own behalf, so a
    // request naming any other asks for a role to be passed to it, which nothing
    // authorizes the caller to do.
    let rows = vec![
        ("no role at all", settings_update(1, 10, 70.0, None), true),
        ("the role the scaling is performed by", named_in_force, true),
        ("a role of the caller's own", named_another, false),
    ];

    for (case, update, served) in rows {
        let (status, body) = call_in(
            &router,
            REGION,
            "UpdateTableReplicaAutoScaling",
            json!({
                "TableName": name,
                "ProvisionedWriteCapacityAutoScalingUpdate": update,
            }),
        )
        .await;
        if served {
            assert_eq!(
                (
                    status,
                    body["TableAutoScalingDescription"]["Replicas"][0]
                        [REPLICA_WRITE]["AutoScalingRoleArn"]
                        .clone()
                ),
                (200, json!(in_force)),
                "case {case:?}: the capacity is scaled through the role in force, got {status} \
                 with {body}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected HTTP 400 ValidationException, got {status} with {body}"
            );
            assert!(
                message(&body).contains("iam:PassRole"),
                "case {case:?}: the refusal states the pass of the role it could not perform, \
                 got {}",
                message(&body)
            );
        }
    }

    // The refused role configured nothing: the capacity still reports the settings the
    // served rows wrote, under the role in force.
    let report = described(&router, name).await;
    assert_eq!(
        stated(&reported_settings(&report, REPLICA_REGION, None, REPLICA_WRITE)),
        json!([1, 10, 70.0, format!("DynamoDBWriteCapacityUtilization:table/{name}")]),
        "case the group's write capacity: the served updates wrote it, got {report}"
    );
}

#[tokio::test]
async fn a_replica_the_clock_is_still_creating_is_reported_as_creating() {
    let router = emulator();
    let name = "Scaled.creating";
    create_table(&router, name).await;
    settle().await;
    let updated = ok_in(
        &router,
        REGION,
        "UpdateTable",
        json!({
            "TableName": name,
            "ReplicaUpdates": [{"Create": {"RegionName": REPLICA_REGION}}],
        }),
    )
    .await;
    assert_eq!(
        updated["TableDescription"]["TableStatus"],
        json!("UPDATING"),
        "case a replica being created: the table is updating, got {updated}"
    );

    // Inside the span the create takes, the auto scaling report carries the replica with
    // the status the clock holds for it — the same status its replica record reports.
    let report = described(&router, name).await;
    assert_eq!(
        (
            replica_in(&report, REPLICA_REGION)["ReplicaStatus"].clone(),
            report["TableStatus"].clone(),
        ),
        (json!("CREATING"), json!("UPDATING")),
        "case a replica being created: the report carries the clock's statuses, got {report}"
    );

    settle().await;
    let report = described(&router, name).await;
    assert_eq!(
        (
            replica_in(&report, REPLICA_REGION)["ReplicaStatus"].clone(),
            replica_in(&report, REGION)["ReplicaStatus"].clone(),
            report["TableStatus"].clone(),
        ),
        (json!("ACTIVE"), json!("ACTIVE"), json!("ACTIVE")),
        "case the create has settled: every replica is active, got {report}"
    );
}
