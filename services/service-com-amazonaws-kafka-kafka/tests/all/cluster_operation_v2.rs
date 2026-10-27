// SPDX-License-Identifier: CC0-1.0

//! `DescribeClusterOperationV2`, driven end to end over the same
//! `MultiServiceRouter` the emulator serves from, through the shared harness in
//! `tests/all/kafka_harness.rs`.
//!
//! RULE under test: the cluster-operation ARN an accepted cluster update hands
//! back names a stored operation, and `DescribeClusterOperationV2` reports THAT
//! record in the V2 shape — the cluster it acts on, the kind of cluster that is,
//! when it started, what kind of operation it is, how far along it is at the
//! reading instant, and the per-kind detail arm the cluster kind names (the
//! provisioned arm's source and target attributes for a provisioned cluster) —
//! so it agrees member for member with what `DescribeClusterOperation` reports
//! for the same ARN. An operation still running carries no end time; one the
//! clock has finished carries one, and reading it neither starts nor finishes
//! anything. A value that is not a cluster-operation ARN is a
//! `BadRequestException`, and a well-formed one this endpoint's region holds no
//! operation for is a `NotFoundException`.
//!
//! The rows differ along what the rule generalises over: which update started
//! the operation (storage, broker storage, broker type, connectivity — every
//! update this service accepts), so the operation type, the attributes it
//! reports and the ARN it is addressed by all differ per row, and each row is
//! read both while the operation is in flight and after the clock has finished
//! it.

use serde_json::{json, Value};

use crate::kafka_harness::{
    pct, sleep_past_activation, sleep_past_update, ClusterRequest, Emulator, Reply, ACCOUNT, REGION,
};

/// The wire values this suite expects, written out rather than imported from
/// the implementation, so an assertion cannot agree with a constant that is
/// wrong. The operation types are the SCREAMING_SNAKE_CASE names Amazon MSK
/// reports for these update APIs, and the states are the ones it reports while
/// an update runs and once it has finished.
const OPERATION_STATE_IN_PROGRESS: &str = "UPDATE_IN_PROGRESS";
const OPERATION_STATE_COMPLETE: &str = "UPDATE_COMPLETE";
/// `ClusterOperationV2$ClusterType` for an operation on a provisioned cluster,
/// from the model's `ClusterType` enum (`PROVISIONED | SERVERLESS`).
const CLUSTER_TYPE_PROVISIONED: &str = "PROVISIONED";

/// `DescribeClusterOperationV2`: `GET /api/v2/operations/{ClusterOperationArn}`,
/// per the operation's `http` binding in the model.
async fn describe_v2(emulator: &Emulator, arn: &str) -> Reply {
    describe_v2_in(emulator, REGION, arn).await
}

/// The same read, as the endpoint of `region` serves it.
async fn describe_v2_in(emulator: &Emulator, region: &str, arn: &str) -> Reply {
    emulator
        .call_in(
            region,
            "GET",
            &format!("/api/v2/operations/{}", pct(arn)),
            None,
        )
        .await
}

/// `DescribeClusterOperation`: `GET /v1/operations/{ClusterOperationArn}` — the
/// V1 view of the same stored record, which the V2 view must agree with.
async fn describe_v1(emulator: &Emulator, arn: &str) -> Reply {
    emulator
        .call("GET", &format!("/v1/operations/{}", pct(arn)), None)
        .await
}

/// One row of the table: an update call, and what the operation it starts must
/// report.
struct Case {
    /// The cluster name, which is also this row's case name.
    name: &'static str,
    /// The request URI's path suffix under the cluster, from the update
    /// operation's `http` binding.
    path: &'static str,
    /// The update body's members beside `currentVersion`.
    body: fn() -> Value,
    /// The `operationType` the stored operation must report.
    operation_type: &'static str,
    /// JSON pointers into `provisioned.sourceClusterInfo` /
    /// `provisioned.targetClusterInfo` that must differ between the two: the
    /// attribute this update changes, reported as it was and as the update
    /// leaves it.
    changed: &'static str,
    /// The value `changed` must hold in the target attributes.
    target: fn() -> Value,
}

/// The table: one row per update this service accepts, so the rule is asserted
/// over every path that files a cluster operation rather than over one of them.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "storage-update",
            path: "storage",
            body: || json!({ "volumeSizeGB": 200 }),
            operation_type: "UPDATE_STORAGE",
            changed: "/brokerEBSVolumeInfo/0/volumeSizeGB",
            target: || json!(200),
        },
        Case {
            name: "broker-storage-update",
            path: "nodes/storage",
            body: || {
                json!({
                    "targetBrokerEBSVolumeInfo": [
                        { "kafkaBrokerNodeId": "ALL", "volumeSizeGB": 500 }
                    ]
                })
            },
            operation_type: "UPDATE_BROKER_STORAGE",
            changed: "/brokerEBSVolumeInfo/0/volumeSizeGB",
            target: || json!(500),
        },
        Case {
            name: "broker-type-update",
            path: "nodes/type",
            body: || json!({ "targetInstanceType": "kafka.m5.2xlarge" }),
            operation_type: "UPDATE_BROKER_TYPE",
            changed: "/instanceType",
            target: || json!("kafka.m5.2xlarge"),
        },
        Case {
            name: "connectivity-update",
            path: "connectivity",
            body: || {
                json!({
                    "connectivityInfo": {
                        "vpcConnectivity": {
                            "clientAuthentication": { "sasl": { "iam": { "enabled": true } } }
                        }
                    }
                })
            },
            operation_type: "UPDATE_CONNECTIVITY",
            changed: "/connectivityInfo/vpcConnectivity/clientAuthentication/sasl/iam/enabled",
            target: || json!(true),
        },
        Case {
            name: "zookeeper-access-update",
            path: "connectivity",
            body: || json!({ "zookeeperAccess": { "enabled": false } }),
            operation_type: "UPDATE_CONNECTIVITY",
            changed: "/zookeeperAccess/enabled",
            target: || json!(false),
        },
    ]
}

/// A member of a reply body, or `Value::Null` when it is absent.
fn member(body: &Value, pointer: &str) -> Value {
    body.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// Every row: the operation the update filed is reported in the V2 shape, in
/// flight and then finished, and the V1 reader of the same ARN agrees with it.
#[tokio::test(flavor = "current_thread")]
async fn describes_the_operation_each_update_files() {
    let emulator = Emulator::new();
    for case in cases() {
        let cluster_arn = emulator
            .create_ok(&ClusterRequest::named(case.name))
            .await;
        sleep_past_activation();
        let version = emulator
            .describe(&cluster_arn)
            .await
            .at("/clusterInfo/currentVersion")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let mut body = (case.body)();
        body["currentVersion"] = json!(version);
        let update = emulator
            .call(
                "PUT",
                &format!("/v1/clusters/{}/{}", pct(&cluster_arn), case.path),
                Some(body),
            )
            .await;
        assert_eq!(
            update.status, 200,
            "case {}: expected the update to be accepted, got {} {}",
            case.name, update.status, update.body
        );
        let operation_arn = update
            .at("/clusterOperationArn")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            operation_arn.starts_with(&format!(
                "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster-operation/{}/",
                case.name
            )),
            "case {}: expected the update to answer with a cluster-operation ARN of its own \
             cluster, got {operation_arn:?}",
            case.name
        );

        // In flight: the operation is reported running, with no end time,
        // because the clock has not reached its completion yet.
        let running = describe_v2(&emulator, &operation_arn).await;
        assert_eq!(
            running.status, 200,
            "case {}: expected DescribeClusterOperationV2 to return 200 for the ARN the \
             update answered with, got {} {}",
            case.name, running.status, running.body
        );
        let info = running.at("/clusterOperationInfo").clone();
        assert_eq!(
            member(&info, "/operationArn"),
            json!(operation_arn),
            "case {}: expected the reported operationArn to be the one read, got {:?}",
            case.name,
            member(&info, "/operationArn")
        );
        assert_eq!(
            member(&info, "/clusterArn"),
            json!(cluster_arn),
            "case {}: expected the operation to name its own cluster, got {:?}",
            case.name,
            member(&info, "/clusterArn")
        );
        assert_eq!(
            member(&info, "/clusterType"),
            json!(CLUSTER_TYPE_PROVISIONED),
            "case {}: expected clusterType {CLUSTER_TYPE_PROVISIONED}, got {:?}",
            case.name,
            member(&info, "/clusterType")
        );
        assert_eq!(
            member(&info, "/operationType"),
            json!(case.operation_type),
            "case {}: expected operationType {}, got {:?}",
            case.name,
            case.operation_type,
            member(&info, "/operationType")
        );
        assert_eq!(
            member(&info, "/operationState"),
            json!(OPERATION_STATE_IN_PROGRESS),
            "case {}: expected operationState {OPERATION_STATE_IN_PROGRESS} while the update \
             is in flight, got {:?}",
            case.name,
            member(&info, "/operationState")
        );
        assert!(
            member(&info, "/startTime")
                .as_f64()
                .is_some_and(|epoch| epoch > 0.0),
            "case {}: expected the operation to report a startTime, got {:?}",
            case.name,
            member(&info, "/startTime")
        );
        assert_eq!(
            member(&info, "/endTime"),
            Value::Null,
            "case {}: expected no endTime while the operation runs, got {:?}",
            case.name,
            member(&info, "/endTime")
        );
        assert_eq!(
            member(&info, "/errorInfo"),
            Value::Null,
            "case {}: expected no errorInfo for an operation nothing failed, got {:?}",
            case.name,
            member(&info, "/errorInfo")
        );
        // The per-kind arm is the one this operation's cluster kind names: the
        // provisioned detail, carrying the attributes the update changes as
        // they were and as it leaves them.
        assert_eq!(
            member(&info, "/serverless"),
            Value::Null,
            "case {}: expected no serverless arm for an operation on a provisioned cluster, \
             got {:?}",
            case.name,
            member(&info, "/serverless")
        );
        let source = member(&info, &format!("/provisioned/sourceClusterInfo{}", case.changed));
        let target = member(&info, &format!("/provisioned/targetClusterInfo{}", case.changed));
        assert_eq!(
            target,
            (case.target)(),
            "case {}: expected the target attributes to carry the submitted {}, got {target:?}",
            case.name,
            case.changed
        );
        assert_ne!(
            source, target,
            "case {}: expected the source attributes to report {} as it was before the \
             update, got {source:?}",
            case.name, case.changed
        );

        // The V1 reader of the same ARN reports the same stored record, so the
        // two views cannot disagree about one operation.
        let v1 = describe_v1(&emulator, &operation_arn).await;
        assert_eq!(
            v1.status, 200,
            "case {}: expected DescribeClusterOperation to return 200 for the same ARN, got \
             {} {}",
            case.name, v1.status, v1.body
        );
        let v1_info = v1.at("/clusterOperationInfo").clone();
        for pointer in ["/operationArn", "/clusterArn", "/operationType", "/startTime"] {
            // The V1 shape dates an operation from `creationTime`, so the two
            // views are compared on the member each one names.
            let v1_pointer = if pointer == "/startTime" {
                "/creationTime"
            } else {
                pointer
            };
            assert_eq!(
                member(&info, pointer),
                member(&v1_info, v1_pointer),
                "case {}: expected the V1 and V2 views to agree on {pointer}, got {:?} and {:?}",
                case.name,
                member(&info, pointer),
                member(&v1_info, v1_pointer)
            );
        }
        for attributes in ["sourceClusterInfo", "targetClusterInfo"] {
            assert_eq!(
                member(&info, &format!("/provisioned/{attributes}")),
                member(&v1_info, &format!("/{attributes}")),
                "case {}: expected the V1 and V2 views to report the same {attributes}, got \
                 {:?} and {:?}",
                case.name,
                member(&info, &format!("/provisioned/{attributes}")),
                member(&v1_info, &format!("/{attributes}"))
            );
        }

        // Finished: the clock, not the reader, is what moves the operation on,
        // and a finished operation reports when it ended.
        sleep_past_update();
        let finished = describe_v2(&emulator, &operation_arn).await;
        let info = finished.at("/clusterOperationInfo").clone();
        assert_eq!(
            member(&info, "/operationState"),
            json!(OPERATION_STATE_COMPLETE),
            "case {}: expected operationState {OPERATION_STATE_COMPLETE} once the update span \
             has passed, got {:?}",
            case.name,
            member(&info, "/operationState")
        );
        assert!(
            member(&info, "/endTime")
                .as_f64()
                .is_some_and(|epoch| epoch > 0.0),
            "case {}: expected a finished operation to report an endTime, got {:?}",
            case.name,
            member(&info, "/endTime")
        );
        // Reading an operation changes nothing a later caller sees: the same
        // read answers the same way.
        let again = describe_v2(&emulator, &operation_arn).await;
        assert_eq!(
            again.body, finished.body,
            "case {}: expected a second read of the operation to report the same record, got \
             {} then {}",
            case.name, finished.body, again.body
        );
    }
}

/// A value that is not a cluster-operation ARN is refused as a bad request,
/// whatever it is; a well-formed one this region holds no operation for is a
/// resource that does not exist. The rows differ along how the value fails:
/// the wrong resource type, a truncated resource path, a foreign service, and
/// something that is no ARN at all.
#[tokio::test(flavor = "current_thread")]
async fn refuses_a_value_that_names_no_operation() {
    let emulator = Emulator::new();
    let malformed = [
        (
            "cluster-arn",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/a-cluster/11111111-2222-3333-4444-555555555555-1"),
        ),
        (
            "missing-operation-id",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster-operation/a-cluster/11111111-2222-3333-4444-555555555555-1"),
        ),
        (
            "other-service",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:cluster-operation/a-cluster/cluster-id/operation-id"),
        ),
        ("not-an-arn", "a-cluster-operation".to_string()),
    ];
    for (case, value) in malformed {
        let reply = describe_v2(&emulator, &value).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: expected 400 for {value:?}, got {} {}",
            reply.status, reply.body
        );
        assert!(
            reply.error_code().ends_with("BadRequestException"),
            "case {case}: expected a BadRequestException for {value:?}, got {:?}",
            reply.error_code()
        );
    }

    // Well formed, and held by nobody: the operation does not exist. The second
    // row is a real operation read through ANOTHER region's endpoint, which
    // knows only its own region's operations.
    let cluster_arn = emulator
        .create_ok(&ClusterRequest::named("elsewhere"))
        .await;
    sleep_past_activation();
    let version = emulator
        .describe(&cluster_arn)
        .await
        .at("/clusterInfo/currentVersion")
        .as_str()
        .unwrap_or_default()
        .to_string();
    let update = emulator
        .call(
            "PUT",
            &format!("/v1/clusters/{}/storage", pct(&cluster_arn)),
            Some(json!({ "currentVersion": version, "volumeSizeGB": 300 })),
        )
        .await;
    assert_eq!(
        update.status, 200,
        "case elsewhere: expected the update to be accepted, got {} {}",
        update.status, update.body
    );
    let operation_arn = update
        .at("/clusterOperationArn")
        .as_str()
        .unwrap_or_default()
        .to_string();
    let unknown = format!(
        "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster-operation/no-such-cluster/\
         11111111-2222-3333-4444-555555555555-1/66666666-7777-8888-9999-000000000000"
    );
    for (case, value, region) in [
        ("unheld-operation", unknown, REGION),
        ("another-region", operation_arn, "eu-central-1"),
    ] {
        let reply = describe_v2_in(&emulator, region, &value).await;
        assert_eq!(
            reply.status, 404,
            "case {case}: expected 404 for {value:?} at the {region} endpoint, got {} {}",
            reply.status, reply.body
        );
        assert!(
            reply.error_code().ends_with("NotFoundException"),
            "case {case}: expected a NotFoundException for {value:?}, got {:?}",
            reply.error_code()
        );
    }
}
