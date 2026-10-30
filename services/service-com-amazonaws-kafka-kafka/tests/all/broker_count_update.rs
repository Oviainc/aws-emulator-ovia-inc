// SPDX-License-Identifier: CC0-1.0

//! `UpdateBrokerCount` end to end, over the same `MultiServiceRouter` the emulator
//! serves from, through the harness in `tests/all/kafka_harness.rs`.
//!
//! The rules under test, each asserted over a TABLE rather than one request:
//!
//! 1. an accepted update is applied ASYNCHRONOUSLY: it records a cluster
//!    operation, leaves the cluster `UPDATING` with that operation as its
//!    `ActiveOperationArn`, and the broker nodes it creates or deletes — with the
//!    new `NumberOfBrokerNodes`, the node listing, the bootstrap endpoints and a
//!    new `CurrentVersion` that follow from them — land only once the clock owes
//!    the operation its completion;
//! 2. `TargetNumberOfBrokerNodes` outside the modelled 1..15 range is refused with
//!    a client-input error, and the cluster is left exactly as it was;
//! 3. the cluster the URI names must be a provisioned one this endpoint holds, the
//!    submitted `CurrentVersion` must be the one the cluster is at, and the cluster
//!    must be `ACTIVE` — so a second update while one is in flight is refused;
//! 4. each accepted update mints its own operation and its own version, and the
//!    cluster ends holding the broker nodes of the last target it accepted.
//!
//! The rows differ along what each rule generalises over: the broker count the
//! cluster starts at, the DIRECTION and size of the change (up, down, to the
//! published bounds and to the count the cluster already has), how many client
//! subnets the nodes are placed across, the kind of ARN, the kind of stale version
//! and the state the cluster is in.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_kafka_kafka::state::{
    MAX_BROKER_NODES_PER_CLUSTER, OPERATION_STATE_COMPLETE, OPERATION_STATE_IN_PROGRESS,
    OPERATION_TYPE_UPDATE_BROKER_COUNT, PROVISIONED_ONLY_MESSAGE, RESOURCE_NOT_FOUND_MESSAGE,
};

use crate::kafka_harness::{
    pct, sleep_past_activation, sleep_past_update, ClusterRequest, Emulator, Reply, ACCOUNT, REGION,
};

/// `PUT /v1/clusters/{ClusterArn}/nodes/count` — the route this suite drives.
async fn update_broker_count(
    emulator: &Emulator,
    cluster_arn: &str,
    current_version: &str,
    target: i64,
) -> Reply {
    emulator
        .call(
            "PUT",
            &format!("/v1/clusters/{}/nodes/count", pct(cluster_arn)),
            Some(json!({
                "currentVersion": current_version,
                "targetNumberOfBrokerNodes": target,
            })),
        )
        .await
}

/// `DescribeClusterOperation`: `GET /v1/operations/{ClusterOperationArn}` — the
/// reader that reports the operation an update started.
async fn describe_operation(emulator: &Emulator, operation_arn: &str) -> Reply {
    emulator
        .call("GET", &format!("/v1/operations/{}", pct(operation_arn)), None)
        .await
}

/// A cluster created with `brokers` nodes across `subnets` and waited into
/// `ACTIVE`, as (ARN, `CurrentVersion`): the state an update is accepted from.
async fn active_cluster(
    emulator: &Emulator,
    name: &str,
    brokers: i64,
    subnets: Vec<&'static str>,
) -> (String, String) {
    let request = ClusterRequest {
        brokers,
        subnets,
        ..ClusterRequest::named(name)
    };
    let arn = emulator.create_ok(&request).await;
    sleep_past_activation();
    let described = emulator.describe(&arn).await;
    assert_eq!(
        described.at("/clusterInfo/state").as_str(),
        Some("ACTIVE"),
        "case {name}: expected the new cluster to settle ACTIVE, got {}",
        described.body
    );
    let version = described
        .at("/clusterInfo/currentVersion")
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: DescribeCluster reported no currentVersion"))
        .to_string();
    (arn, version)
}

/// The broker ids `ListNodes` reports for the cluster, in the order it reports
/// them: the nodes the cluster actually holds, which is what an applied
/// broker-count change has to move.
async fn listed_broker_ids(emulator: &Emulator, cluster_arn: &str, case: &str) -> Vec<i64> {
    let reply = emulator
        .call(
            "GET",
            &format!("/v1/clusters/{}/nodes", pct(cluster_arn)),
            None,
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected ListNodes to return 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .at("/nodeInfoList")
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: ListNodes reported no nodeInfoList: {}", reply.body))
        .iter()
        .map(|node| {
            node.pointer("/brokerNodeInfo/brokerId")
                .and_then(Value::as_f64)
                .map(|id| id as i64)
                .unwrap_or_else(|| panic!("case {case}: a node reported no brokerId: {node}"))
        })
        .collect()
}

/// Every broker host a `GetBootstrapBrokers` answer names, with the per-path port
/// stripped: the endpoints a client would connect to, rendered from the same
/// stored nodes the listing reports.
async fn bootstrap_hosts(emulator: &Emulator, cluster_arn: &str, case: &str) -> BTreeSet<String> {
    let reply = emulator.bootstrap_brokers(cluster_arn).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected GetBootstrapBrokers to return 200, got {} {}",
        reply.status, reply.body
    );
    let mut hosts = BTreeSet::new();
    let members = reply
        .body
        .as_object()
        .unwrap_or_else(|| panic!("case {case}: GetBootstrapBrokers returned {}", reply.body));
    for value in members.values() {
        let Some(text) = value.as_str() else { continue };
        for endpoint in text.split(',') {
            let (host, _port) = endpoint
                .rsplit_once(':')
                .unwrap_or_else(|| panic!("case {case}: {endpoint:?} carries no port"));
            hosts.insert(host.to_string());
        }
    }
    hosts
}

/// `arn:aws:kafka:<region>:<account>:cluster-operation/<name>/<cluster-id>/<operation-id>`:
/// a cluster operation hangs off the resource tail of the cluster it belongs to.
fn assert_operation_arn(operation_arn: &str, cluster_arn: &str, case: &str) {
    let cluster_tail = cluster_arn
        .rsplit(':')
        .next()
        .and_then(|resource| resource.strip_prefix("cluster/"))
        .unwrap_or_else(|| panic!("case {case}: {cluster_arn:?} is not a cluster ARN"));
    let prefix = format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster-operation/{cluster_tail}/");
    assert!(
        operation_arn.starts_with(&prefix),
        "case {case}: expected an operation ARN starting {prefix:?}, got {operation_arn:?}"
    );
}

/// The broker ids a value of `BrokerCountUpdateInfo`'s two `__listOf__double`
/// members carries, as whole numbers.
fn update_info_ids(value: &Value, case: &str, member: &str) -> Vec<i64> {
    value
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: the operation reported no {member}: {value}"))
        .iter()
        .map(|id| {
            id.as_f64()
                .map(|id| id as i64)
                .unwrap_or_else(|| panic!("case {case}: {member} carries {id}, not a number"))
        })
        .collect()
}

/// RULE: an accepted `UpdateBrokerCount` does not rewrite the cluster where it
/// stands — it starts a cluster operation, so until the clock owes that operation
/// its completion the cluster reads `UPDATING` with the operation as its
/// `ActiveOperationArn`, at its old broker count, nodes and version; once it lands
/// the cluster is `ACTIVE` again holding exactly the target number of broker
/// nodes — reported by `NumberOfBrokerNodes`, by the node listing and by the
/// bootstrap endpoints alike — at a NEW version, with every other member of the
/// record unchanged, and the operation itself reports the count it moved from and
/// to plus the ids of the nodes it created or deleted.
#[tokio::test]
async fn an_accepted_update_lands_the_broker_nodes_once_the_clock_owes_it() {
    // The rows differ along what the rule generalises over: the count the cluster
    // starts at, the DIRECTION and size of the change (up by one, up to the
    // published maximum, down, down to the published minimum, and to the count the
    // cluster already has), and how many client subnets the nodes are spread over.
    let rows: &[(&str, i64, i64, Vec<&'static str>)] = &[
        ("up-by-one", 3, 4, vec!["subnet-a", "subnet-b", "subnet-c"]),
        (
            "up-to-the-maximum",
            2,
            i64::from(MAX_BROKER_NODES_PER_CLUSTER),
            vec!["subnet-a", "subnet-b"],
        ),
        ("down", 6, 2, vec!["subnet-a", "subnet-b", "subnet-c"]),
        ("down-to-the-minimum", 4, 1, vec!["subnet-a"]),
        ("same-count", 3, 3, vec!["subnet-a", "subnet-b", "subnet-c"]),
        ("up-from-one", 1, 2, vec!["subnet-a", "subnet-b"]),
    ];

    let emulator = Emulator::new();
    for (case, initial, target, subnets) in rows {
        let (arn, version) = active_cluster(&emulator, case, *initial, subnets.clone()).await;
        let before = emulator.describe(&arn).await;
        let before = before.at("/clusterInfo").clone();
        let nodes_before = listed_broker_ids(&emulator, &arn, case).await;
        assert_eq!(
            nodes_before,
            (1..=*initial).collect::<Vec<i64>>(),
            "case {case}: expected the created cluster to hold brokers 1..={initial}, got \
             {nodes_before:?}"
        );

        let reply = update_broker_count(&emulator, &arn, &version, *target).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected UpdateBrokerCount to return 200, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.body["clusterArn"].as_str(),
            Some(arn.as_str()),
            "case {case}: UpdateBrokerCount reported the wrong clusterArn, got {}",
            reply.body["clusterArn"]
        );
        let operation_arn = reply.body["clusterOperationArn"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case}: UpdateBrokerCount returned no clusterOperationArn")
            })
            .to_string();
        assert_operation_arn(&operation_arn, &arn, case);

        // Still rolling: the operation is the cluster's active one and nothing it
        // will change has changed yet.
        let rolling = emulator.describe(&arn).await;
        assert_eq!(
            rolling.at("/clusterInfo/state").as_str(),
            Some("UPDATING"),
            "case {case}: expected the cluster to read UPDATING, got {}",
            rolling.body
        );
        assert_eq!(
            rolling.at("/clusterInfo/activeOperationArn").as_str(),
            Some(operation_arn.as_str()),
            "case {case}: expected activeOperationArn {operation_arn:?}, got {}",
            rolling.at("/clusterInfo/activeOperationArn")
        );
        assert_eq!(
            rolling.at("/clusterInfo/numberOfBrokerNodes").as_i64(),
            Some(*initial),
            "case {case}: the broker count must not change before the operation lands, got {}",
            rolling.at("/clusterInfo/numberOfBrokerNodes")
        );
        assert_eq!(
            listed_broker_ids(&emulator, &arn, case).await,
            nodes_before,
            "case {case}: the nodes must not change before the operation lands"
        );
        assert_eq!(
            rolling.at("/clusterInfo/currentVersion").as_str(),
            Some(version.as_str()),
            "case {case}: the version must not change before the operation lands, got {}",
            rolling.at("/clusterInfo/currentVersion")
        );

        // The operation record reports what it is changing — the counts, and the
        // ids of the nodes it creates or deletes to get there — and that it is
        // still running.
        let operation = describe_operation(&emulator, &operation_arn).await;
        assert_eq!(
            operation.status, 200,
            "case {case}: expected DescribeClusterOperation to return 200, got {} {}",
            operation.status, operation.body
        );
        assert_eq!(
            operation.at("/clusterOperationInfo/operationType").as_str(),
            Some(OPERATION_TYPE_UPDATE_BROKER_COUNT),
            "case {case}: expected the broker-count operation type, got {}",
            operation.at("/clusterOperationInfo/operationType")
        );
        assert_eq!(
            operation
                .at("/clusterOperationInfo/operationState")
                .as_str(),
            Some(OPERATION_STATE_IN_PROGRESS),
            "case {case}: expected an in-progress operation, got {}",
            operation.at("/clusterOperationInfo/operationState")
        );
        assert_eq!(
            operation
                .at("/clusterOperationInfo/sourceClusterInfo/numberOfBrokerNodes")
                .as_i64(),
            Some(*initial),
            "case {case}: expected the source broker count, got {}",
            operation.at("/clusterOperationInfo/sourceClusterInfo/numberOfBrokerNodes")
        );
        assert_eq!(
            operation
                .at("/clusterOperationInfo/targetClusterInfo/numberOfBrokerNodes")
                .as_i64(),
            Some(*target),
            "case {case}: expected the target broker count, got {}",
            operation.at("/clusterOperationInfo/targetClusterInfo/numberOfBrokerNodes")
        );
        let created = update_info_ids(
            operation.at(
                "/clusterOperationInfo/targetClusterInfo/brokerCountUpdateInfo/createdBrokerIds",
            ),
            case,
            "createdBrokerIds",
        );
        let deleted = update_info_ids(
            operation.at(
                "/clusterOperationInfo/targetClusterInfo/brokerCountUpdateInfo/deletedBrokerIds",
            ),
            case,
            "deletedBrokerIds",
        );
        assert_eq!(
            created,
            (initial + 1..=*target).collect::<Vec<i64>>(),
            "case {case}: expected the ids of the nodes the update adds, got {created:?}"
        );
        assert_eq!(
            deleted,
            (target + 1..=*initial).collect::<Vec<i64>>(),
            "case {case}: expected the ids of the nodes the update removes, got {deleted:?}"
        );

        sleep_past_update();
        let after = emulator.describe(&arn).await;
        assert_eq!(
            after.at("/clusterInfo/state").as_str(),
            Some("ACTIVE"),
            "case {case}: expected the cluster to settle ACTIVE again, got {}",
            after.body
        );
        assert_eq!(
            after.at("/clusterInfo/activeOperationArn"),
            &Value::Null,
            "case {case}: a landed operation is no longer active, got {}",
            after.at("/clusterInfo/activeOperationArn")
        );
        assert_eq!(
            after.at("/clusterInfo/numberOfBrokerNodes").as_i64(),
            Some(*target),
            "case {case}: expected the target broker count, got {}",
            after.at("/clusterInfo/numberOfBrokerNodes")
        );

        // The NODES moved with the count: the listing reports one per broker the
        // cluster now holds, and the bootstrap endpoints name exactly those nodes.
        let nodes_after = listed_broker_ids(&emulator, &arn, case).await;
        assert_eq!(
            nodes_after,
            (1..=*target).collect::<Vec<i64>>(),
            "case {case}: expected the cluster to hold brokers 1..={target}, got {nodes_after:?}"
        );
        let hosts = bootstrap_hosts(&emulator, &arn, case).await;
        assert_eq!(
            hosts.len(),
            usize::try_from(*target).expect("a broker count fits a usize"),
            "case {case}: expected one bootstrap host per broker node, got {hosts:?}"
        );
        for id in 1..=*target {
            assert!(
                hosts.iter().any(|host| host.starts_with(&format!("b-{id}."))),
                "case {case}: expected a bootstrap endpoint for broker {id}, got {hosts:?}"
            );
        }

        let new_version = after
            .at("/clusterInfo/currentVersion")
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: DescribeCluster reported no currentVersion"))
            .to_string();
        assert_ne!(
            new_version, version,
            "case {case}: a landed update must leave the cluster at a new version, got \
             {new_version:?}"
        );

        // And the operation has finished, with the end time it finished at.
        let operation = describe_operation(&emulator, &operation_arn).await;
        assert_eq!(
            operation
                .at("/clusterOperationInfo/operationState")
                .as_str(),
            Some(OPERATION_STATE_COMPLETE),
            "case {case}: expected a completed operation, got {}",
            operation.at("/clusterOperationInfo/operationState")
        );
        assert!(
            !operation.at("/clusterOperationInfo/endTime").is_null(),
            "case {case}: a completed operation must report its endTime, got {}",
            operation.body
        );

        // Everything the operation does not change is still what it was: the
        // update adds or removes broker nodes, it does not rewrite the record.
        let after = after.at("/clusterInfo").clone();
        for member in [
            "clusterName",
            "clusterArn",
            "creationTime",
            "encryptionInfo",
            "enhancedMonitoring",
            "storageMode",
            "zookeeperConnectString",
            "currentBrokerSoftwareInfo",
            "brokerNodeGroupInfo",
        ] {
            assert_eq!(
                after.get(member),
                before.get(member),
                "case {case}: {member} must survive the update unchanged, got {:?}",
                after.get(member)
            );
        }
    }
}

/// RULE: `TargetNumberOfBrokerNodes` becomes the cluster's own
/// `NumberOfBrokerNodes`, so a value outside the modelled 1..15 range is refused
/// with a client-input error and the cluster is left exactly as it was, while each
/// published bound is accepted.
#[tokio::test]
async fn a_target_outside_the_modelled_range_is_refused() {
    // The bounds come from the `range` on
    // `UpdateBrokerCountRequest$TargetNumberOfBrokerNodes` (`__integerMin1Max15`,
    // 1..15), which the service crate exports as MAX_BROKER_NODES_PER_CLUSTER:
    // each published bound is accepted and each value one past it refused.
    let maximum = i64::from(MAX_BROKER_NODES_PER_CLUSTER);
    let rows: &[(i64, bool)] = &[
        (1, true),
        (maximum, true),
        (0, false),
        (-1, false),
        (maximum + 1, false),
        (i64::from(i32::MAX), false),
    ];

    let emulator = Emulator::new();
    for (index, (target, accepted)) in rows.iter().enumerate() {
        let case = format!("target={target}");
        let (arn, version) = active_cluster(
            &emulator,
            &format!("range-row-{index}"),
            3,
            vec!["subnet-a", "subnet-b", "subnet-c"],
        )
        .await;

        let reply = update_broker_count(&emulator, &arn, &version, *target).await;
        if *accepted {
            assert_eq!(
                reply.status, 200,
                "case {case}: expected 200 for a count within the published bounds, got {} {}",
                reply.status, reply.body
            );
            sleep_past_update();
            let after = emulator.describe(&arn).await;
            assert_eq!(
                after.at("/clusterInfo/numberOfBrokerNodes").as_i64(),
                Some(*target),
                "case {case}: expected the accepted target to land, got {}",
                after.at("/clusterInfo/numberOfBrokerNodes")
            );
            continue;
        }

        assert_eq!(
            reply.status, 400,
            "case {case}: expected 400 for a count outside the range, got {} {}",
            reply.status, reply.body
        );
        assert!(
            matches!(
                reply.error_code(),
                "BadRequestException" | "ValidationException"
            ),
            "case {case}: expected a client-input refusal, got {}",
            reply.body
        );

        // A refused update changes nothing: no operation, no new version, no new
        // broker count.
        let after = emulator.describe(&arn).await;
        assert_eq!(
            after.at("/clusterInfo/state").as_str(),
            Some("ACTIVE"),
            "case {case}: a refused update must leave the cluster ACTIVE, got {}",
            after.body
        );
        assert_eq!(
            after.at("/clusterInfo/activeOperationArn"),
            &Value::Null,
            "case {case}: a refused update must start no operation, got {}",
            after.at("/clusterInfo/activeOperationArn")
        );
        assert_eq!(
            after.at("/clusterInfo/numberOfBrokerNodes").as_i64(),
            Some(3),
            "case {case}: a refused update must not change the broker count, got {}",
            after.at("/clusterInfo/numberOfBrokerNodes")
        );
        assert_eq!(
            after.at("/clusterInfo/currentVersion").as_str(),
            Some(version.as_str()),
            "case {case}: a refused update must not change the version, got {}",
            after.at("/clusterInfo/currentVersion")
        );
    }
}

/// RULE: the cluster an update names must be a provisioned cluster this endpoint
/// holds — a value that is not an MSK cluster ARN is `BadRequestException`, a
/// well-formed ARN no live cluster of this region holds is `NotFoundException`,
/// and a serverless cluster has no broker nodes of its own to scale.
#[tokio::test]
async fn an_update_to_a_cluster_this_endpoint_does_not_hold_is_refused() {
    let emulator = Emulator::new();
    let (live_arn, live_version) = active_cluster(
        &emulator,
        "reachable",
        3,
        vec!["subnet-a", "subnet-b", "subnet-c"],
    )
    .await;

    // A cluster that was deleted, and one that lives in another region, are both
    // ARNs this endpoint cannot resolve — exactly like one that never existed.
    let deleted_arn = emulator
        .create_ok(&ClusterRequest::named("deleted-cluster"))
        .await;
    sleep_past_activation();
    let deleted = emulator
        .call("DELETE", &format!("/v1/clusters/{}", pct(&deleted_arn)), None)
        .await;
    assert_eq!(
        deleted.status, 200,
        "expected DeleteCluster to return 200, got {} {}",
        deleted.status, deleted.body
    );
    let other_region_arn = emulator
        .create_ok_in("eu-central-1", &ClusterRequest::named("elsewhere"))
        .await;

    // The rows differ along the KIND of ARN: not an ARN at all, an ARN of another
    // service, the right service with the wrong resource type, a cluster resource
    // missing its id, a well-formed ARN of no cluster, a deleted cluster and a
    // cluster of another region.
    let unknown = format!(
        "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/ghost/11111111-2222-4333-8444-555555555555-9"
    );
    let rows: &[(&str, &str, u16, &str)] = &[
        ("not-an-arn", "demo-cluster", 400, "BadRequestException"),
        (
            "wrong-service",
            "arn:aws:s3:::my-bucket",
            400,
            "BadRequestException",
        ),
        (
            "wrong-resource-type",
            "arn:aws:kafka:us-west-2:000000000000:configuration/demo/abc-1",
            400,
            "BadRequestException",
        ),
        (
            "no-cluster-id",
            "arn:aws:kafka:us-west-2:000000000000:cluster/demo",
            400,
            "BadRequestException",
        ),
        ("unknown-cluster", &unknown, 404, "NotFoundException"),
        ("deleted-cluster", &deleted_arn, 404, "NotFoundException"),
        ("another-region", &other_region_arn, 404, "NotFoundException"),
    ];

    for (case, arn, status, code) in rows {
        let reply = update_broker_count(&emulator, arn, &live_version, 4).await;
        assert_eq!(
            reply.status, *status,
            "case {case}: expected {status} for {arn:?}, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            *code,
            "case {case}: expected {code} for {arn:?}, got {}",
            reply.body
        );
        if *status == 404 {
            // A missing resource is reported with the service's own constant
            // message, the same one for every ARN it cannot resolve.
            assert_eq!(
                reply.error_message(),
                RESOURCE_NOT_FOUND_MESSAGE,
                "case {case}: expected the service's not-found message, got {}",
                reply.body
            );
        }
    }

    // A SERVERLESS cluster resolves, and is refused on its kind: its brokers are
    // the service's, so there is no broker count for a caller to set.
    let serverless_arn = emulator
        .create_serverless_ok("serverless-count", &[vec!["subnet-a", "subnet-b"]], None)
        .await;
    sleep_past_activation();
    let described = emulator.describe_v2(&serverless_arn).await;
    let serverless_version = described
        .at("/clusterInfo/currentVersion")
        .as_str()
        .expect("DescribeClusterV2 reported no currentVersion")
        .to_string();
    let reply = update_broker_count(&emulator, &serverless_arn, &serverless_version, 4).await;
    assert_eq!(
        reply.status, 400,
        "case serverless: expected 400 for a serverless cluster, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        "BadRequestException",
        "case serverless: expected BadRequestException, got {}",
        reply.body
    );
    assert_eq!(
        reply.error_message(),
        PROVISIONED_ONLY_MESSAGE,
        "case serverless: expected the service's provisioned-only message, got {}",
        reply.body
    );

    // And the cluster this endpoint DOES hold still updates, so the refusals above
    // are about the cluster named and not about the service being unable to scale.
    let reply = update_broker_count(&emulator, &live_arn, &live_version, 4).await;
    assert_eq!(
        reply.status, 200,
        "expected the live cluster to accept the same update, got {} {}",
        reply.status, reply.body
    );
}

/// RULE: an update applies to the cluster version the caller submits, so a
/// `CurrentVersion` other than the one the cluster is at is refused
/// `BadRequestException` — which makes a replayed update a refusal once the first
/// one has landed.
#[tokio::test]
async fn a_current_version_other_than_the_clusters_is_refused() {
    let emulator = Emulator::new();
    let (arn, version) = active_cluster(
        &emulator,
        "versioned",
        3,
        vec!["subnet-a", "subnet-b", "subnet-c"],
    )
    .await;
    let (_, other_cluster_version) = active_cluster(
        &emulator,
        "another-cluster",
        3,
        vec!["subnet-a", "subnet-b", "subnet-c"],
    )
    .await;

    // The rows differ along the KIND of wrong version: empty, a token of the right
    // shape that is not this cluster's, another cluster's actual version, and
    // arbitrary text.
    let rows: Vec<(&str, String)> = vec![
        ("empty", String::new()),
        ("well-shaped-but-wrong", "K3AEGXETFSTCSX".to_string()),
        ("another-clusters", other_cluster_version),
        ("not-a-version", "2".to_string()),
    ];
    for (case, stale) in &rows {
        assert_ne!(
            stale, &version,
            "case {case}: the row must differ from the cluster's own version"
        );
        let reply = update_broker_count(&emulator, &arn, stale, 4).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: expected 400 for currentVersion {stale:?}, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "BadRequestException",
            "case {case}: expected BadRequestException, got {}",
            reply.body
        );
        assert_eq!(
            reply.body["invalidParameter"].as_str(),
            Some("currentVersion"),
            "case {case}: expected the refusal to name currentVersion, got {}",
            reply.body
        );
        let after = emulator.describe(&arn).await;
        assert_eq!(
            after.at("/clusterInfo/numberOfBrokerNodes").as_i64(),
            Some(3),
            "case {case}: a refused update must not change the broker count, got {}",
            after.at("/clusterInfo/numberOfBrokerNodes")
        );
    }

    // The cluster's own version is accepted — and the SAME request replayed after
    // the update landed is refused, because the version it names is no longer the
    // cluster's.
    let accepted = update_broker_count(&emulator, &arn, &version, 4).await;
    assert_eq!(
        accepted.status, 200,
        "expected the cluster's own version to be accepted, got {} {}",
        accepted.status, accepted.body
    );
    sleep_past_update();
    let replayed = update_broker_count(&emulator, &arn, &version, 5).await;
    assert_eq!(
        replayed.status, 400,
        "expected a replayed update to be refused once the first landed, got {} {}",
        replayed.status, replayed.body
    );
    assert_eq!(
        replayed.error_code(),
        "BadRequestException",
        "expected BadRequestException for a replayed update, got {}",
        replayed.body
    );
}

/// RULE: a cluster reconfigures one change at a time, so an update is refused
/// `BadRequestException` unless the cluster is `ACTIVE` — whether it is still
/// provisioning or already rolling an update of its own — and the refusal starts
/// nothing.
#[tokio::test]
async fn an_update_is_refused_unless_the_cluster_is_active() {
    let emulator = Emulator::new();

    // CREATING: the cluster has not finished provisioning.
    let creating_arn = emulator
        .create_ok(&ClusterRequest::named("still-creating"))
        .await;
    let creating = emulator.describe(&creating_arn).await;
    let creating_version = creating
        .at("/clusterInfo/currentVersion")
        .as_str()
        .expect("DescribeCluster reported no currentVersion")
        .to_string();
    let reply = update_broker_count(&emulator, &creating_arn, &creating_version, 4).await;
    assert_eq!(
        reply.status, 400,
        "case CREATING: expected 400 while the cluster is provisioning, got {} {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_code(),
        "BadRequestException",
        "case CREATING: expected BadRequestException, got {}",
        reply.body
    );
    let described = emulator.describe(&creating_arn).await;
    assert_eq!(
        described.at("/clusterInfo/activeOperationArn"),
        &Value::Null,
        "case CREATING: a refused update must start no operation, got {}",
        described.at("/clusterInfo/activeOperationArn")
    );

    // UPDATING: an operation of its own is already in flight. The first update's
    // target is the one that lands; every attempt made while it rolls is refused,
    // and refused as a CLIENT fault — the caller has asked for something the
    // cluster cannot do yet.
    let (arn, version) = active_cluster(
        &emulator,
        "already-updating",
        3,
        vec!["subnet-a", "subnet-b", "subnet-c"],
    )
    .await;
    let first = update_broker_count(&emulator, &arn, &version, 4).await;
    assert_eq!(
        first.status, 200,
        "case UPDATING: expected the first update to be accepted, got {} {}",
        first.status, first.body
    );
    let first_operation = first.body["clusterOperationArn"]
        .as_str()
        .expect("UpdateBrokerCount returned no clusterOperationArn")
        .to_string();
    // Several attempts while the first rolls, differing in the target they ask
    // for: each one is refused the same way, so a caller retrying in a loop sees a
    // client fault rather than a server error.
    for target in [5, 6, 4] {
        let again = update_broker_count(&emulator, &arn, &version, target).await;
        assert_eq!(
            again.status, 400,
            "case UPDATING target={target}: expected 400 while an operation is in flight, got {} {}",
            again.status, again.body
        );
        assert_eq!(
            again.error_code(),
            "BadRequestException",
            "case UPDATING target={target}: expected BadRequestException, got {}",
            again.body
        );
        let rolling = emulator.describe(&arn).await;
        assert_eq!(
            rolling.at("/clusterInfo/activeOperationArn").as_str(),
            Some(first_operation.as_str()),
            "case UPDATING target={target}: the first operation must still be the active one, \
             got {}",
            rolling.at("/clusterInfo/activeOperationArn")
        );
    }
    sleep_past_update();
    let after = emulator.describe(&arn).await;
    assert_eq!(
        after.at("/clusterInfo/numberOfBrokerNodes").as_i64(),
        Some(4),
        "case UPDATING: only the accepted update may land, got {}",
        after.at("/clusterInfo/numberOfBrokerNodes")
    );
    assert_eq!(
        listed_broker_ids(&emulator, &arn, "UPDATING").await,
        vec![1, 2, 3, 4],
        "case UPDATING: only the accepted update's nodes may exist"
    );
}

/// RULE: every accepted update is its own operation — a new operation ARN and a
/// new cluster version each time — and the cluster ends holding exactly the broker
/// nodes of the last target it accepted, whichever direction each step moved in.
#[tokio::test]
async fn consecutive_updates_each_mint_their_own_operation() {
    // A chain rather than a single call: the rule is about successive updates, so
    // the row count and the direction of each step are what it generalises over —
    // up, down, and back up past the count a step removed.
    let targets = [5, 2, 6];

    let emulator = Emulator::new();
    let (arn, mut version) = active_cluster(
        &emulator,
        "chained",
        3,
        vec!["subnet-a", "subnet-b", "subnet-c"],
    )
    .await;
    let mut operations: Vec<String> = Vec::new();
    let mut versions: Vec<String> = vec![version.clone()];

    for target in targets {
        let case = format!("target={target}");
        let reply = update_broker_count(&emulator, &arn, &version, target).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected 200, got {} {}",
            reply.status, reply.body
        );
        let operation_arn = reply.body["clusterOperationArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: no clusterOperationArn"))
            .to_string();
        assert_operation_arn(&operation_arn, &arn, &case);
        operations.push(operation_arn);

        sleep_past_update();
        let after = emulator.describe(&arn).await;
        assert_eq!(
            after.at("/clusterInfo/numberOfBrokerNodes").as_i64(),
            Some(target),
            "case {case}: expected the target to land, got {}",
            after.at("/clusterInfo/numberOfBrokerNodes")
        );
        assert_eq!(
            listed_broker_ids(&emulator, &arn, &case).await,
            (1..=target).collect::<Vec<i64>>(),
            "case {case}: expected the cluster to hold brokers 1..={target}"
        );
        version = after
            .at("/clusterInfo/currentVersion")
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: no currentVersion"))
            .to_string();
        versions.push(version.clone());
    }

    let mut distinct = operations.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        operations.len(),
        "every update must mint its own operation ARN, got {operations:?}"
    );
    let mut distinct_versions = versions.clone();
    distinct_versions.sort();
    distinct_versions.dedup();
    assert_eq!(
        distinct_versions.len(),
        versions.len(),
        "every landed update must leave the cluster at a new version, got {versions:?}"
    );

    // Every operation the chain minted is still readable, each reporting the step
    // it applied: a cluster's history keeps one record per accepted update.
    for (index, operation_arn) in operations.iter().enumerate() {
        let operation = describe_operation(&emulator, operation_arn).await;
        assert_eq!(
            operation.status, 200,
            "operation {index}: expected DescribeClusterOperation to return 200, got {} {}",
            operation.status, operation.body
        );
        assert_eq!(
            operation
                .at("/clusterOperationInfo/targetClusterInfo/numberOfBrokerNodes")
                .as_i64(),
            Some(targets[index]),
            "operation {index}: expected target {}, got {}",
            targets[index],
            operation.at("/clusterOperationInfo/targetClusterInfo/numberOfBrokerNodes")
        );
    }
}
