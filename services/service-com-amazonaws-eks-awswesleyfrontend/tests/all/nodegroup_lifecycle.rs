// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the EKS managed node group slice: `CreateNodegroup` and
//! `DescribeNodegroup`, driven over the wire through the same `MultiServiceRouter`
//! the production binary mounts.
//!
//! Every suite here asserts a RULE over a TABLE of inputs rather than one
//! recorded request: the rows differ along what the rule generalises over (which
//! optional members are supplied, AMI family, scaling triple, list length,
//! region, cluster). Each assertion names its case and prints the actual value.
//!
//! The wire harness (`Answer`, `emulator`, `call`, `encode_label`) is the shared
//! one in `tests/all/wire.rs` — reused rather than written again.

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    DEFAULT_AMI_TYPE, DEFAULT_CAPACITY_TYPE, DEFAULT_DISK_SIZE_GIB, DEFAULT_INSTANCE_TYPE,
    DEFAULT_MAX_UNAVAILABLE, DEFAULT_SCALING_DESIRED_SIZE, DEFAULT_SCALING_MAX_SIZE,
    DEFAULT_SCALING_MIN_SIZE, DEFAULT_WINDOWS_DISK_SIZE_GIB, MAX_INSTANCE_TYPES,
    MAX_NODEGROUPS_PER_CLUSTER, MIN_DISK_SIZE_GIB, NODEGROUP_ACTIVATION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::REGION;
use crate::wire::{call, emulator, encode_label, Answer};

/// The `nodegroup` document an answer carries.
fn nodegroup(answer: &Answer) -> &serde_json::Value {
    &answer.body["nodegroup"]
}

/// The same document with `status` dropped, which is the one member a read may
/// legitimately differ on: the clock owes the node group a `CREATING` ->
/// `ACTIVE` move between the create and a later read.
fn without_status(answer: &Answer) -> serde_json::Value {
    let mut document = nodegroup(answer).clone();
    if let Some(object) = document.as_object_mut() {
        object.remove("status");
    }
    document
}

/// The smallest request `CreateCluster` accepts, so a node group has a cluster.
fn cluster_request(name: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "roleArn": "arn:aws:iam::000000000000:role/eksClusterRole",
        "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
    })
}

async fn create_cluster_in(router: &MultiServiceRouter, region: &str, name: &str) -> Answer {
    call(
        router,
        region,
        "POST",
        "/clusters",
        &cluster_request(name).to_string(),
    )
    .await
}

/// Create a cluster in [`REGION`] and answer the Kubernetes version it reports,
/// which is the only `version` a node group of it may name.
pub(crate) async fn create_cluster(router: &MultiServiceRouter, name: &str) -> String {
    let answer = create_cluster_in(router, REGION, name).await;
    assert_eq!(
        answer.status, 200,
        "the cluster {name:?} a node group needs must be created, got {answer:?}"
    );
    answer.body["cluster"]["version"]
        .as_str()
        .expect("CreateCluster reports the cluster's Kubernetes version")
        .to_owned()
}

pub(crate) async fn create_nodegroup_in(
    router: &MultiServiceRouter,
    region: &str,
    cluster: &str,
    body: &serde_json::Value,
) -> Answer {
    call(
        router,
        region,
        "POST",
        &format!("/clusters/{}/node-groups", encode_label(cluster)),
        &body.to_string(),
    )
    .await
}

async fn create_nodegroup(
    router: &MultiServiceRouter,
    cluster: &str,
    body: &serde_json::Value,
) -> Answer {
    create_nodegroup_in(router, REGION, cluster, body).await
}

async fn describe_nodegroup_in(
    router: &MultiServiceRouter,
    region: &str,
    cluster: &str,
    name: &str,
) -> Answer {
    call(
        router,
        region,
        "GET",
        &format!(
            "/clusters/{}/node-groups/{}",
            encode_label(cluster),
            encode_label(name)
        ),
        "",
    )
    .await
}

async fn describe_nodegroup(router: &MultiServiceRouter, cluster: &str, name: &str) -> Answer {
    describe_nodegroup_in(router, REGION, cluster, name).await
}

/// The smallest request `CreateNodegroup` accepts: the three required members.
pub(crate) fn minimal_request(name: &str) -> serde_json::Value {
    serde_json::json!({
        "nodegroupName": name,
        "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
        "subnets": ["subnet-aaaa1111", "subnet-bbbb2222"],
    })
}

// ---------------------------------------------------------------------------
// RULE: a node group read back through `DescribeNodegroup` is the record
// `CreateNodegroup` wrote — every member the request supplied, plus the
// server-generated ones — and only its clock-owed status may have moved.
// ---------------------------------------------------------------------------

/// The table spans what the rule generalises over: which optional members are
/// supplied at all, the AMI family (which switches the default disk size), the
/// scaling triple, the taint/label/tag map shapes and the launch-template form
/// (which switches three members to `null`).
#[tokio::test]
async fn a_created_nodegroup_reads_back_exactly_as_stored() {
    let router = emulator();
    let cluster_version = create_cluster(&router, "roundtrip-cluster").await;

    let cases: Vec<(&str, serde_json::Value)> = vec![
        ("only the required members", minimal_request("minimal")),
        (
            "every optional member this slice stores",
            serde_json::json!({
                "nodegroupName": "fully-specified",
                "nodeRole": "arn:aws:iam::000000000000:role/other",
                "subnets": ["subnet-1", "subnet-2", "subnet-3"],
                "amiType": "AL2023_ARM_64_STANDARD",
                "capacityType": "SPOT",
                "instanceTypes": ["m6g.large", "m6g.xlarge"],
                "diskSize": 100,
                "scalingConfig": { "minSize": 2, "maxSize": 10, "desiredSize": 4 },
                "updateConfig": { "maxUnavailablePercentage": 25, "updateStrategy": "MINIMAL" },
                "nodeRepairConfig": { "enabled": true, "maxUnhealthyNodeThresholdPercentage": 20 },
                "remoteAccess": { "ec2SshKey": "my-key", "sourceSecurityGroups": ["sg-1", "sg-2"] },
                "labels": { "role": "worker", "tier": "batch" },
                "taints": [{ "key": "dedicated", "value": "batch", "effect": "NO_SCHEDULE" }],
                "tags": { "env": "dev", "team": "platform" },
                "warmPoolConfig": { "enabled": true, "minSize": 1, "poolState": "STOPPED" },
                "clientRequestToken": "token-fully-specified",
                "releaseVersion": "1.31.0-20250101",
                "version": cluster_version,
            }),
        ),
        (
            "a Windows AMI family, where the default disk size differs",
            serde_json::json!({
                "nodegroupName": "windows_nodes",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": ["subnet-1"],
                "amiType": "WINDOWS_CORE_2022_x86_64",
            }),
        ),
        (
            "a launch template, which carries the instance type and disk itself",
            serde_json::json!({
                "nodegroupName": "LaunchTemplated9",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": ["subnet-1", "subnet-2"],
                "launchTemplate": { "id": "lt-0123456789abcdef0", "version": "3" },
            }),
        ),
        (
            "a single tag and a single label, the low end of the modelled maps",
            serde_json::json!({
                "nodegroupName": "one-of-each",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": ["subnet-1"],
                "tags": { "only": "one" },
                "labels": { "only": "one" },
                "scalingConfig": { "minSize": 0, "maxSize": 1, "desiredSize": 0 },
            }),
        ),
    ];

    for (case, request) in &cases {
        let name = request["nodegroupName"]
            .as_str()
            .expect("the case names its node group");
        let created = create_nodegroup(&router, "roundtrip-cluster", request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateNodegroup, got {created:?}"
        );

        // The echo-back of everything the request supplied.
        for member in [
            "nodegroupName",
            "nodeRole",
            "subnets",
            "capacityType",
            "amiType",
            "labels",
            "taints",
            "tags",
            "scalingConfig",
            "updateConfig",
            "nodeRepairConfig",
            "remoteAccess",
            "warmPoolConfig",
            "launchTemplate",
            "releaseVersion",
            "instanceTypes",
        ] {
            if let Some(supplied) = request.get(member) {
                assert_eq!(
                    &nodegroup(&created)[member],
                    supplied,
                    "case {case:?}: expected {member} {supplied:?}, got {:?}",
                    nodegroup(&created)[member]
                );
            }
        }

        // The server-generated parts.
        assert_eq!(
            nodegroup(&created)["clusterName"],
            serde_json::json!("roundtrip-cluster"),
            "case {case:?}: the node group names its cluster, got {:?}",
            nodegroup(&created)["clusterName"]
        );
        assert_eq!(
            nodegroup(&created)["version"],
            serde_json::json!(cluster_version),
            "case {case:?}: the node group takes the cluster's Kubernetes version, got {:?}",
            nodegroup(&created)["version"]
        );
        assert_eq!(
            nodegroup(&created)["status"],
            serde_json::json!("CREATING"),
            "case {case:?}: a new node group reports CREATING, got {:?}",
            nodegroup(&created)["status"]
        );
        assert!(
            nodegroup(&created)["createdAt"].is_number(),
            "case {case:?}: createdAt is an epoch timestamp, got {:?}",
            nodegroup(&created)["createdAt"]
        );
        assert_eq!(
            nodegroup(&created)["modifiedAt"],
            nodegroup(&created)["createdAt"],
            "case {case:?}: an unmodified node group was last modified when it was created, \
             got {:?}",
            nodegroup(&created)["modifiedAt"]
        );
        assert_eq!(
            nodegroup(&created)["health"],
            serde_json::json!({ "issues": [] }),
            "case {case:?}: a node group with no issues reports an empty health block, got {:?}",
            nodegroup(&created)["health"]
        );

        // And the read-back: the same document, status aside.
        let described = describe_nodegroup(&router, "roundtrip-cluster", name).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected 200 from DescribeNodegroup, got {described:?}"
        );
        assert_eq!(
            without_status(&described),
            without_status(&created),
            "case {case:?}: DescribeNodegroup must answer the record CreateNodegroup wrote"
        );

        // A read never changes what the next caller sees.
        let again = describe_nodegroup(&router, "roundtrip-cluster", name).await;
        assert_eq!(
            nodegroup(&again),
            nodegroup(&described),
            "case {case:?}: a second read must answer identically"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: every member the caller did not supply is filled in with the value the
// documentation names — the launch-template members with `null`, because the
// launch template carries them.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unspecified_members_take_their_documented_defaults() {
    let router = emulator();
    create_cluster(&router, "defaults-cluster").await;

    // Each row: the request, then the expected `amiType`, `diskSize`,
    // `instanceTypes` and `capacityType`. The AMI families are members of the
    // modelled `AMITypes` enum (`smithy_query query="shape" shape="AMITypes"`),
    // including ones no recorded request named.
    let cases: Vec<(&str, serde_json::Value, &str, serde_json::Value, serde_json::Value)> = vec![
        (
            "nothing optional supplied",
            minimal_request("all-defaults"),
            DEFAULT_AMI_TYPE,
            serde_json::json!(DEFAULT_DISK_SIZE_GIB),
            serde_json::json!([DEFAULT_INSTANCE_TYPE]),
        ),
        (
            "a Bottlerocket AMI family, which takes the Linux disk default",
            serde_json::json!({
                "nodegroupName": "bottlerocket",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": ["subnet-1"],
                "amiType": "BOTTLEROCKET_ARM_64",
            }),
            "BOTTLEROCKET_ARM_64",
            serde_json::json!(DEFAULT_DISK_SIZE_GIB),
            serde_json::json!([DEFAULT_INSTANCE_TYPE]),
        ),
        (
            "a Windows AMI family, which takes the Windows disk default",
            serde_json::json!({
                "nodegroupName": "windows-full",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": ["subnet-1"],
                "amiType": "WINDOWS_FULL_2019_x86_64",
            }),
            "WINDOWS_FULL_2019_x86_64",
            serde_json::json!(DEFAULT_WINDOWS_DISK_SIZE_GIB),
            serde_json::json!([DEFAULT_INSTANCE_TYPE]),
        ),
        (
            "an explicit disk size and instance type, which are not defaulted",
            serde_json::json!({
                "nodegroupName": "explicit",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": ["subnet-1"],
                "diskSize": 64,
                "instanceTypes": ["c7g.2xlarge"],
            }),
            DEFAULT_AMI_TYPE,
            serde_json::json!(64),
            serde_json::json!(["c7g.2xlarge"]),
        ),
        (
            "a launch template with no amiType, which is reported as CUSTOM",
            serde_json::json!({
                "nodegroupName": "templated",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": ["subnet-1"],
                "launchTemplate": { "name": "my-template" },
            }),
            "CUSTOM",
            serde_json::Value::Null,
            serde_json::Value::Null,
        ),
    ];

    for (case, request, ami_type, disk_size, instance_types) in &cases {
        let created = create_nodegroup(&router, "defaults-cluster", request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateNodegroup, got {created:?}"
        );
        assert_eq!(
            nodegroup(&created)["amiType"],
            serde_json::json!(ami_type),
            "case {case:?}: expected amiType {ami_type:?}, got {:?}",
            nodegroup(&created)["amiType"]
        );
        assert_eq!(
            &nodegroup(&created)["diskSize"],
            disk_size,
            "case {case:?}: expected diskSize {disk_size:?}, got {:?}",
            nodegroup(&created)["diskSize"]
        );
        assert_eq!(
            &nodegroup(&created)["instanceTypes"],
            instance_types,
            "case {case:?}: expected instanceTypes {instance_types:?}, got {:?}",
            nodegroup(&created)["instanceTypes"]
        );
        assert_eq!(
            nodegroup(&created)["capacityType"],
            serde_json::json!(DEFAULT_CAPACITY_TYPE),
            "case {case:?}: expected capacityType {DEFAULT_CAPACITY_TYPE:?}, got {:?}",
            nodegroup(&created)["capacityType"]
        );
        assert_eq!(
            nodegroup(&created)["scalingConfig"],
            serde_json::json!({
                "minSize": DEFAULT_SCALING_MIN_SIZE,
                "maxSize": DEFAULT_SCALING_MAX_SIZE,
                "desiredSize": DEFAULT_SCALING_DESIRED_SIZE,
            }),
            "case {case:?}: expected the default scaling triple, got {:?}",
            nodegroup(&created)["scalingConfig"]
        );
        assert_eq!(
            nodegroup(&created)["updateConfig"]["maxUnavailable"],
            serde_json::json!(DEFAULT_MAX_UNAVAILABLE),
            "case {case:?}: expected maxUnavailable {DEFAULT_MAX_UNAVAILABLE}, got {:?}",
            nodegroup(&created)["updateConfig"]
        );
        // No `releaseVersion` was named and this emulator publishes no AMI
        // catalogue, so it reports none rather than inventing one.
        assert_eq!(
            nodegroup(&created)["releaseVersion"],
            serde_json::Value::Null,
            "case {case:?}: expected no releaseVersion, got {:?}",
            nodegroup(&created)["releaseVersion"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: the node group's ARN names the region the request is signed for, the
// cluster and the node group, and ends in an id unique to that creation.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_nodegroup_arn_names_its_region_cluster_and_name() {
    let router = emulator();

    let cases = [
        (REGION, "arn-cluster-a", "nodes-one"),
        (REGION, "arn-cluster-b", "nodes-one"),
        ("eu-central-1", "arn-cluster-a", "nodes_two"),
        ("ap-southeast-2", "arn-cluster-a", "Nodes3"),
    ];

    let mut seen_ids = std::collections::BTreeSet::new();
    for (region, cluster, name) in cases {
        let cluster_answer = create_cluster_in(&router, region, cluster).await;
        assert!(
            cluster_answer.status == 200 || cluster_answer.status == 409,
            "case {region:?}/{cluster:?}: the cluster must exist, got {cluster_answer:?}"
        );

        let created = create_nodegroup_in(&router, region, cluster, &minimal_request(name)).await;
        assert_eq!(
            created.status, 200,
            "case {region:?}/{cluster:?}/{name:?}: expected 200, got {created:?}"
        );
        let arn = nodegroup(&created)["nodegroupArn"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let prefix = format!("arn:aws:eks:{region}:000000000000:nodegroup/{cluster}/{name}/");
        assert!(
            arn.starts_with(&prefix),
            "case {region:?}/{cluster:?}/{name:?}: expected an ARN starting {prefix:?}, got \
             {arn:?}"
        );

        // The trailing id is the 8-4-4-4-12 hex grouping the real ARNs use, and
        // is distinct per creation.
        let id = arn[prefix.len()..].to_owned();
        let groups: Vec<usize> = id.split('-').map(str::len).collect();
        assert_eq!(
            groups,
            vec![8, 4, 4, 4, 12],
            "case {region:?}/{cluster:?}/{name:?}: expected an 8-4-4-4-12 id, got {id:?}"
        );
        assert!(
            id.chars().all(|digit| digit.is_ascii_hexdigit() || digit == '-'),
            "case {region:?}/{cluster:?}/{name:?}: the id is hex, got {id:?}"
        );
        assert!(
            seen_ids.insert(id.clone()),
            "case {region:?}/{cluster:?}/{name:?}: the id {id:?} repeats another node group's"
        );

        // And the record is readable only through the region it was created in.
        let described = describe_nodegroup_in(&router, region, cluster, name).await;
        assert_eq!(
            nodegroup(&described)["nodegroupArn"],
            serde_json::json!(arn),
            "case {region:?}/{cluster:?}/{name:?}: the read-back carries the same ARN, got {:?}",
            nodegroup(&described)["nodegroupArn"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a node group is identified by (region, cluster, name); anything else is
// 404 `ResourceNotFoundException` on `DescribeNodegroup`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_node_group_outside_its_region_cluster_or_name_is_not_found() {
    let router = emulator();
    create_cluster(&router, "lookup-cluster").await;
    create_cluster(&router, "other-cluster").await;
    let created = create_nodegroup(&router, "lookup-cluster", &minimal_request("nodes")).await;
    assert_eq!(
        created.status, 200,
        "the node group every row below looks for must exist, got {created:?}"
    );

    let cases = [
        ("an unknown node group in a known cluster", REGION, "lookup-cluster", "nope"),
        ("a name differing only in case", REGION, "lookup-cluster", "NODES"),
        ("the right name in another cluster", REGION, "other-cluster", "nodes"),
        ("the right name in an unknown cluster", REGION, "no-cluster", "nodes"),
        ("the right name in another region", "eu-west-1", "lookup-cluster", "nodes"),
    ];

    for (case, region, cluster, name) in cases {
        let answer = describe_nodegroup_in(&router, region, cluster, name).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
    }

    // The node group that does exist is still found, so the rows above refused
    // for the right reason.
    let found = describe_nodegroup(&router, "lookup-cluster", "nodes").await;
    assert_eq!(
        found.status, 200,
        "the stored node group is still found, got {found:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: a node group name is unique within its cluster — a second create
// conflicts with 409 `ResourceInUseException`, unless it replays the same
// idempotency token, in which case it is absorbed and answers the stored
// record.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn names_conflict_within_the_cluster_and_a_replayed_token_is_absorbed() {
    let router = emulator();
    create_cluster(&router, "conflict-cluster").await;
    create_cluster(&router, "conflict-cluster-2").await;

    for case in ["alpha", "bravo9", "c_d-e"] {
        let first = create_nodegroup(&router, "conflict-cluster", &minimal_request(case)).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: the first CreateNodegroup succeeds, got {first:?}"
        );

        let again = create_nodegroup(&router, "conflict-cluster", &minimal_request(case)).await;
        assert_eq!(
            again.status, 409,
            "case {case:?}: re-creating the same name conflicts, got {again:?}"
        );
        assert_eq!(
            again.error_type.as_deref(),
            Some("ResourceInUseException"),
            "case {case:?}: expected ResourceInUseException, got {again:?}"
        );

        // The same name in ANOTHER cluster is a different node group.
        let elsewhere =
            create_nodegroup(&router, "conflict-cluster-2", &minimal_request(case)).await;
        assert_eq!(
            elsewhere.status, 200,
            "case {case:?}: the same name in another cluster is free, got {elsewhere:?}"
        );
        assert_ne!(
            nodegroup(&elsewhere)["nodegroupArn"],
            nodegroup(&first)["nodegroupArn"],
            "case {case:?}: two clusters' node groups are distinct resources"
        );
    }

    // A retry carries the SAME token and is absorbed; a different token on the
    // same name still conflicts; a blank token identifies nothing.
    for (case, token, expected_status) in [
        ("the same token, a retry", "token-retry", 200),
        ("a different token", "token-other", 409),
        ("a blank token", "", 409),
    ] {
        let mut request = minimal_request("tokened");
        request["clientRequestToken"] = serde_json::json!("token-retry");
        let first = create_nodegroup(&router, "conflict-cluster", &request).await;
        assert!(
            first.status == 200 || first.status == 409,
            "case {case:?}: the first create either lands or already exists, got {first:?}"
        );

        request["clientRequestToken"] = serde_json::json!(token);
        let retried = create_nodegroup(&router, "conflict-cluster", &request).await;
        assert_eq!(
            retried.status, expected_status,
            "case {case:?}: expected {expected_status}, got {retried:?}"
        );
        if expected_status == 200 {
            let described = describe_nodegroup(&router, "conflict-cluster", "tokened").await;
            assert_eq!(
                nodegroup(&retried)["nodegroupArn"],
                nodegroup(&described)["nodegroupArn"],
                "case {case:?}: an absorbed retry answers the stored node group"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: a status move the clock owes is materialised by whichever read first
// runs after it is due — and not before.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_new_nodegroup_reports_creating_until_the_clock_owes_it_active() {
    let router = emulator();
    create_cluster(&router, "settling-cluster").await;

    for case in ["settling-a", "settling-b"] {
        let created = create_nodegroup(&router, "settling-cluster", &minimal_request(case)).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateNodegroup, got {created:?}"
        );
        assert_eq!(
            nodegroup(&created)["status"],
            serde_json::json!("CREATING"),
            "case {case:?}: CreateNodegroup reports CREATING, got {:?}",
            nodegroup(&created)["status"]
        );

        // A caller acting straight after the create still observes the
        // transient state: the span is never zero.
        let immediately = describe_nodegroup(&router, "settling-cluster", case).await;
        assert_eq!(
            nodegroup(&immediately)["status"],
            serde_json::json!("CREATING"),
            "case {case:?}: a describe straight after the create still sees CREATING, got {:?}",
            nodegroup(&immediately)["status"]
        );
    }

    tokio::time::sleep(NODEGROUP_ACTIVATION_DELAY + std::time::Duration::from_millis(250)).await;

    for case in ["settling-a", "settling-b"] {
        let described = describe_nodegroup(&router, "settling-cluster", case).await;
        assert_eq!(
            nodegroup(&described)["status"],
            serde_json::json!("ACTIVE"),
            "case {case:?}: once the activation delay has passed the node group is ACTIVE, got \
             {:?}",
            nodegroup(&described)["status"]
        );
        let twice = describe_nodegroup(&router, "settling-cluster", case).await;
        assert_eq!(
            nodegroup(&twice),
            nodegroup(&described),
            "case {case:?}: a second read must answer identically"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: every cardinality and exclusivity rule `CreateNodegroup`'s own
// documentation states — and which the model does not express as a constraint —
// is refused with 400 `InvalidParameterException`, and nothing is stored.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_documented_request_rules_are_enforced() {
    let router = emulator();
    let cluster_version = create_cluster(&router, "rules-cluster").await;
    let twenty_types: Vec<String> = (0..MAX_INSTANCE_TYPES)
        .map(|index| format!("m5.type{index}"))
        .collect();
    let one_too_many: Vec<String> = (0..MAX_INSTANCE_TYPES + 1)
        .map(|index| format!("m5.type{index}"))
        .collect();

    // Each row: the case, the request, and whether it must be ACCEPTED. The
    // refusals span every documented rule; the acceptances pin the published
    // bound itself, so the refusals are not passing for the wrong reason.
    let cases: Vec<(&str, serde_json::Value, bool)> = vec![
        (
            "no subnet at all",
            serde_json::json!({
                "nodegroupName": "no-subnets",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": [],
            }),
            false,
        ),
        (
            "one subnet, the low end of the bound",
            serde_json::json!({
                "nodegroupName": "one-subnet",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": ["subnet-1"],
            }),
            true,
        ),
        (
            "a one GiB disk, the low end of the bound",
            serde_json::json!({
                "nodegroupName": "one-gib",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": ["subnet-1"],
                "diskSize": MIN_DISK_SIZE_GIB,
            }),
            true,
        ),
        (
            "a zero GiB disk, one past the low end of the bound",
            serde_json::json!({
                "nodegroupName": "zero-gib",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": ["subnet-1"],
                "diskSize": 0,
            }),
            false,
        ),
        (
            "a negative disk size",
            serde_json::json!({
                "nodegroupName": "negative-gib",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": ["subnet-1"],
                "diskSize": -1,
            }),
            false,
        ),
        (
            "20 instance types, the published maximum",
            serde_json::json!({
                "nodegroupName": "twenty-types",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": ["subnet-1"],
                "instanceTypes": twenty_types,
            }),
            true,
        ),
        (
            "21 instance types, one past the published maximum",
            serde_json::json!({
                "nodegroupName": "twentyone-types",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": ["subnet-1"],
                "instanceTypes": one_too_many,
            }),
            false,
        ),
        (
            "scalingConfig with only minSize",
            serde_json::json!({
                "nodegroupName": "partial-min",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": ["subnet-1"],
                "scalingConfig": { "minSize": 1 },
            }),
            false,
        ),
        (
            "scalingConfig with only maxSize",
            serde_json::json!({
                "nodegroupName": "partial-max",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": ["subnet-1"],
                "scalingConfig": { "maxSize": 3 },
            }),
            false,
        ),
        (
            "scalingConfig with two of the three properties",
            serde_json::json!({
                "nodegroupName": "partial-two",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": ["subnet-1"],
                "scalingConfig": { "minSize": 1, "maxSize": 3 },
            }),
            false,
        ),
        (
            "scalingConfig with all three properties",
            serde_json::json!({
                "nodegroupName": "scaling-all",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": ["subnet-1"],
                "scalingConfig": { "minSize": 1, "maxSize": 3, "desiredSize": 2 },
            }),
            true,
        ),
        (
            "desiredSize above maxSize",
            serde_json::json!({
                "nodegroupName": "desired-high",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": ["subnet-1"],
                "scalingConfig": { "minSize": 1, "maxSize": 3, "desiredSize": 4 },
            }),
            false,
        ),
        (
            "desiredSize below minSize",
            serde_json::json!({
                "nodegroupName": "desired-low",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": ["subnet-1"],
                "scalingConfig": { "minSize": 2, "maxSize": 3, "desiredSize": 1 },
            }),
            false,
        ),
        (
            "minSize above maxSize",
            serde_json::json!({
                "nodegroupName": "min-above-max",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": ["subnet-1"],
                "scalingConfig": { "minSize": 5, "maxSize": 2, "desiredSize": 3 },
            }),
            false,
        ),
        (
            "the three sizes equal, the tightest ordered triple",
            serde_json::json!({
                "nodegroupName": "sizes-equal",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": ["subnet-1"],
                "scalingConfig": { "minSize": 2, "maxSize": 2, "desiredSize": 2 },
            }),
            true,
        ),
        (
            "updateConfig naming both maxUnavailable forms",
            serde_json::json!({
                "nodegroupName": "update-both",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": ["subnet-1"],
                "updateConfig": { "maxUnavailable": 1, "maxUnavailablePercentage": 10 },
            }),
            false,
        ),
        (
            "updateConfig naming neither maxUnavailable form",
            serde_json::json!({
                "nodegroupName": "update-neither",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": ["subnet-1"],
                "updateConfig": { "updateStrategy": "DEFAULT" },
            }),
            false,
        ),
        (
            "updateConfig naming the percentage alone",
            serde_json::json!({
                "nodegroupName": "update-percentage",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": ["subnet-1"],
                "updateConfig": { "maxUnavailablePercentage": 100 },
            }),
            true,
        ),
        (
            "launchTemplate naming both id and name",
            serde_json::json!({
                "nodegroupName": "template-both",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": ["subnet-1"],
                "launchTemplate": { "id": "lt-1", "name": "lt-one" },
            }),
            false,
        ),
        (
            "launchTemplate naming neither id nor name",
            serde_json::json!({
                "nodegroupName": "template-neither",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": ["subnet-1"],
                "launchTemplate": { "version": "2" },
            }),
            false,
        ),
        (
            "launchTemplate beside diskSize",
            serde_json::json!({
                "nodegroupName": "template-disk",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": ["subnet-1"],
                "diskSize": 50,
                "launchTemplate": { "id": "lt-2" },
            }),
            false,
        ),
        (
            "launchTemplate beside remoteAccess",
            serde_json::json!({
                "nodegroupName": "template-remote",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": ["subnet-1"],
                "remoteAccess": { "ec2SshKey": "key" },
                "launchTemplate": { "id": "lt-3" },
            }),
            false,
        ),
        (
            "launchTemplate alone, by name",
            serde_json::json!({
                "nodegroupName": "template-alone",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": ["subnet-1"],
                "launchTemplate": { "name": "lt-four" },
            }),
            true,
        ),
        (
            "nodeRepairConfig naming both unhealthy-threshold forms",
            serde_json::json!({
                "nodegroupName": "repair-threshold",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": ["subnet-1"],
                "nodeRepairConfig": {
                    "enabled": true,
                    "maxUnhealthyNodeThresholdCount": 2,
                    "maxUnhealthyNodeThresholdPercentage": 20,
                },
            }),
            false,
        ),
        (
            "nodeRepairConfig naming both parallel-repair forms",
            serde_json::json!({
                "nodegroupName": "repair-parallel",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": ["subnet-1"],
                "nodeRepairConfig": {
                    "enabled": true,
                    "maxParallelNodesRepairedCount": 2,
                    "maxParallelNodesRepairedPercentage": 20,
                },
            }),
            false,
        ),
        (
            "nodeRepairConfig naming one of each pair",
            serde_json::json!({
                "nodegroupName": "repair-ok",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": ["subnet-1"],
                "nodeRepairConfig": {
                    "enabled": true,
                    "maxUnhealthyNodeThresholdCount": 2,
                    "maxParallelNodesRepairedPercentage": 20,
                },
            }),
            true,
        ),
        (
            "a version that is not the cluster's",
            serde_json::json!({
                "nodegroupName": "version-mismatch",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": ["subnet-1"],
                "version": "1.21",
            }),
            false,
        ),
        (
            "the cluster's own version",
            serde_json::json!({
                "nodegroupName": "version-match",
                "nodeRole": "arn:aws:iam::000000000000:role/eksNodeRole",
                "subnets": ["subnet-1"],
                "version": cluster_version,
            }),
            true,
        ),
    ];

    for (case, request, accepted) in &cases {
        let name = request["nodegroupName"]
            .as_str()
            .expect("the case names its node group");
        let answer = create_nodegroup(&router, "rules-cluster", request).await;
        let described = describe_nodegroup(&router, "rules-cluster", name).await;
        if *accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200 from CreateNodegroup, got {answer:?}"
            );
            assert_eq!(
                described.status, 200,
                "case {case:?}: an accepted node group is readable back, got {described:?}"
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected 400 from CreateNodegroup, got {answer:?}"
            );
            assert_eq!(
                answer.error_type.as_deref(),
                Some("InvalidParameterException"),
                "case {case:?}: expected InvalidParameterException, got {answer:?}"
            );
            // A refused create stores nothing.
            assert_eq!(
                described.status, 404,
                "case {case:?}: a refused create must store nothing, got {described:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: a node group needs a cluster — `CreateNodegroup` against a cluster name
// this region does not hold is refused with the service's 404
// `ResourceNotFoundException`, naming the cluster it could not resolve, and
// stores nothing. (The error CLASS of that refusal, and the ordering that makes
// it precede every report about the rest of the request, are the subject of
// `tests/all/error_class_selection.rs` and
// `tests/all/child_create_resolves_the_cluster_first.rs`.)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_node_group_cannot_be_created_without_its_cluster() {
    let router = emulator();
    create_cluster(&router, "present-cluster").await;

    let cases = [
        ("a cluster name nothing holds", REGION, "absent-cluster"),
        ("a name differing only in case", REGION, "PRESENT-CLUSTER"),
        (
            "a cluster that exists only in another region",
            "eu-west-1",
            "present-cluster",
        ),
    ];

    for (case, region, cluster) in cases {
        let answer = create_nodegroup_in(&router, region, cluster, &minimal_request("nodes")).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
        // The cluster is the resource reported missing, so the payload names it and
        // never the node group the request asked for.
        assert_eq!(
            answer.body["clusterName"].as_str(),
            Some(cluster),
            "case {case:?}: the payload names the cluster it could not resolve, got {:?}",
            answer.body["clusterName"]
        );
        assert_eq!(
            answer.body["nodegroupName"],
            serde_json::Value::Null,
            "case {case:?}: the payload must not echo the requested nodegroupName, got {:?}",
            answer.body["nodegroupName"]
        );
        let described = describe_nodegroup_in(&router, region, cluster, "nodes").await;
        assert_eq!(
            described.status, 404,
            "case {case:?}: a refused create must store nothing, got {described:?}"
        );
    }

    // The cluster that does exist accepts the same request.
    let accepted = create_nodegroup(&router, "present-cluster", &minimal_request("nodes")).await;
    assert_eq!(
        accepted.status, 200,
        "the present cluster accepts the same request, got {accepted:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: the per-cluster node group quota is reported as
// `ResourceLimitExceededException`, and counted per cluster.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_per_cluster_nodegroup_quota_is_reported_as_resource_limit_exceeded() {
    let router = emulator();
    create_cluster(&router, "quota-cluster").await;
    create_cluster(&router, "quota-cluster-2").await;

    for index in 0..MAX_NODEGROUPS_PER_CLUSTER {
        let name = format!("nodes-{index:03}");
        let answer = create_nodegroup(&router, "quota-cluster", &minimal_request(&name)).await;
        assert_eq!(
            answer.status, 200,
            "case {name:?}: the first {MAX_NODEGROUPS_PER_CLUSTER} node groups land, got \
             {answer:?}"
        );
    }

    let over = create_nodegroup(&router, "quota-cluster", &minimal_request("one-too-many")).await;
    assert_eq!(
        over.status, 400,
        "the node group past the quota is refused, got {over:?}"
    );
    assert_eq!(
        over.error_type.as_deref(),
        Some("ResourceLimitExceededException"),
        "expected ResourceLimitExceededException, got {over:?}"
    );

    // The quota is per cluster, so another cluster still has its whole
    // allowance.
    let elsewhere =
        create_nodegroup(&router, "quota-cluster-2", &minimal_request("one-too-many")).await;
    assert_eq!(
        elsewhere.status, 200,
        "the quota is counted per cluster, got {elsewhere:?}"
    );
}
