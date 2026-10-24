// SPDX-License-Identifier: CC0-1.0

//! The Snowball cluster lifecycle — `CreateCluster`, `DescribeCluster`, `UpdateCluster`
//! and the node jobs a cluster is made of — driven end to end over HTTP.
//!
//! One rule, asserted over a TABLE of inputs rather than over a single request: while a
//! cluster's `ClusterState` is `AwaitingQuorum`, `UpdateCluster` writes every member the
//! request names onto the stored cluster and leaves every member it omits as it was, so a
//! later `DescribeCluster` reports the merged cluster; a request naming a cluster in any
//! other state is refused with `InvalidJobStateException`, and one naming a cluster or an
//! address no record answers to with `InvalidResourceException`, in both cases changing
//! nothing.
//!
//! The rows differ along what the rule generalises over: which members the request carries
//! (none, one, several, all of them), the enum value from the modelled catalog
//! (`SHIPPING_OPTION_VALUES`, read from the crate rather than restated), the length of a
//! value at each published bound of `Description`, the shape of the structure members, and
//! the number of node jobs the cluster holds (none, one, one short of the quorum, the
//! quorum, the largest initial size the model admits).
//!
//! The harness (`router`, `ok`, `refused`, `create_address`, `deliverable_address`,
//! `string`, `assert_prefixed_uuid`) is the one `job_lifecycle` already drives the service
//! with, so every call here goes through the same front door a client uses and nothing
//! reaches into the state struct.

use serde_json::{json, Value};
// The catalog the modelled `ShippingOption` enum admits and the quorum this service holds
// a cluster to, read from the crate rather than restated, so a row cannot drift from the
// values the model allows or from the bound the handler enforces.
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::types::{
    CLUSTER_STATE_VALUES, SHIPPING_OPTION_VALUES,
};
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::state::{
    ClusterState, CLUSTER_NODE_QUORUM,
};
use smithy_server_runtime::MultiServiceRouter;

use super::job_lifecycle::{
    assert_prefixed_uuid, create_address, deliverable_address, ok, refused, router, string,
};

/// `UpdateClusterRequest$Description` is modelled `length 1..1024`
/// (`smithy_query(query="operation", operation="UpdateCluster")`), so these are the
/// shortest and longest values the member admits.
const DESCRIPTION_MIN_LEN: usize = 1;
const DESCRIPTION_MAX_LEN: usize = 1024;

/// `CreateClusterRequest$InitialClusterSize` is modelled `range 0..16`
/// (`smithy_query(query="operation", operation="CreateCluster")`), so these are the
/// smallest and largest sizes the member admits.
const INITIAL_CLUSTER_SIZE_MIN: i64 = 0;
const INITIAL_CLUSTER_SIZE_MAX: i64 = 16;

/// A cluster id in the modelled `ClusterId` form that names no stored cluster.
const UNKNOWN_CLUSTER_ID: &str = "CID00000000-0000-0000-0000-000000000000";
/// An address id in the modelled `AddressId` form that names no stored address.
const UNKNOWN_ADDRESS_ID: &str = "ADID00000000-0000-0000-0000-000000000000";

/// Every state the cluster lifecycle enum spells, so the set below is checked against the
/// model's catalog rather than against itself.
const ALL_CLUSTER_STATES: &[ClusterState] = &[
    ClusterState::AwaitingQuorum,
    ClusterState::Pending,
    ClusterState::InUse,
    ClusterState::Complete,
    ClusterState::Cancelled,
];

/// The `ClusterMetadata` `DescribeCluster` reports for `cluster_id`.
pub(crate) async fn cluster_metadata(
    router: &MultiServiceRouter,
    cluster_id: &str,
    case: &str,
) -> Value {
    let described = ok(
        router,
        "DescribeCluster",
        json!({ "ClusterId": cluster_id }),
        case,
    )
    .await;
    described["ClusterMetadata"].clone()
}

/// The `JobMetadata` `DescribeJob` reports for `job_id`.
pub(crate) async fn job_metadata(router: &MultiServiceRouter, job_id: &str, case: &str) -> Value {
    let described = ok(router, "DescribeJob", json!({ "JobId": job_id }), case).await;
    described["JobMetadata"].clone()
}

/// The create every suite below starts from: a cluster carrying a value for each member an
/// update can replace, so a row that names one member proves the others were left alone.
/// Only members `ClusterMetadata` reports are here; the ones a cluster keeps for its node
/// jobs alone are exercised by the inheritance suite below.
pub(crate) fn base_cluster(address_id: &str) -> Value {
    json!({
        "JobType": "LOCAL_USE",
        "SnowballType": "EDGE",
        "ShippingOption": "SECOND_DAY",
        "AddressId": address_id,
        "Description": "the plan the cluster was created with",
        "RoleARN": "arn:aws:iam::123456789012:role/snowball-cluster-created",
        "KmsKeyARN": "arn:aws:kms:us-west-2:123456789012:key/created-key",
        "Resources": { "S3Resources": [{ "BucketArn": "arn:aws:s3:::cluster-created" }] },
        "Notification": {
            "SnsTopicARN": "arn:aws:sns:us-west-2:123456789012:cluster-created",
            "NotifyAll": true
        },
        "TaxDocuments": { "IND": { "GSTIN": "27AAPFU0939F1ZV" } },
    })
}

/// Merge `update` over `expected`, which is what applying the update to the stored cluster
/// must do: a named member replaces what was stored, an omitted one leaves it alone.
fn merge(expected: &mut Value, update: &Value) {
    for (member, value) in update.as_object().expect("the update is a JSON object") {
        expected[member] = value.clone();
    }
}

/// The body of an `UpdateCluster` call: the update's members plus the cluster it names.
fn update_body(cluster_id: &str, update: &Value) -> Value {
    let mut body = update.clone();
    body["ClusterId"] = json!(cluster_id);
    body
}

/// Every update row: one per member `UpdateClusterRequest` models, one per value the
/// modelled `ShippingOption` catalog admits, one at each published bound of `Description`,
/// one carrying every updatable member at once, and one carrying none.
fn update_rows(address_id: &str, alternate_address_id: &str) -> Vec<(String, Value)> {
    let mut rows: Vec<(String, Value)> = vec![
        (
            "the cluster id alone, naming no member to change".to_string(),
            json!({}),
        ),
        (
            "the description alone".to_string(),
            json!({ "Description": "re-planned cluster" }),
        ),
        (
            "the shortest description the member admits".to_string(),
            json!({ "Description": "x".repeat(DESCRIPTION_MIN_LEN) }),
        ),
        (
            "the longest description the member admits".to_string(),
            json!({ "Description": "y".repeat(DESCRIPTION_MAX_LEN) }),
        ),
        (
            "a non-ascii description".to_string(),
            json!({ "Description": "クラスター計画の更新 — Núñez" }),
        ),
        (
            "a different shipping address".to_string(),
            json!({ "AddressId": alternate_address_id }),
        ),
        (
            "back to the address the cluster was created with".to_string(),
            json!({ "AddressId": address_id }),
        ),
        (
            "a forwarding address".to_string(),
            json!({ "ForwardingAddressId": alternate_address_id }),
        ),
        (
            "a new role".to_string(),
            json!({ "RoleARN": "arn:aws:iam::123456789012:role/snowball-cluster-updated" }),
        ),
        (
            "resources over one bucket with a key range".to_string(),
            json!({
                "Resources": {
                    "S3Resources": [{
                        "BucketArn": "arn:aws:s3:::cluster-updated",
                        "KeyRange": { "BeginMarker": "b/", "EndMarker": "n/" }
                    }]
                }
            }),
        ),
        (
            "resources over several buckets".to_string(),
            json!({
                "Resources": {
                    "S3Resources": [
                        { "BucketArn": "arn:aws:s3:::cluster-updated-first" },
                        { "BucketArn": "arn:aws:s3:::cluster-updated-second" }
                    ]
                }
            }),
        ),
        (
            "a notification over a new topic".to_string(),
            json!({
                "Notification": {
                    "SnsTopicARN": "arn:aws:sns:us-west-2:123456789012:cluster-updates",
                    "JobStatesToNotify": ["InProgress", "Complete"],
                    "NotifyAll": false
                }
            }),
        ),
        (
            "on-device services".to_string(),
            json!({
                "OnDeviceServiceConfiguration": {
                    "NFSOnDeviceService": { "StorageLimit": 25, "StorageUnit": "TB" }
                }
            }),
        ),
        (
            "every updatable member at once".to_string(),
            json!({
                "AddressId": alternate_address_id,
                "ForwardingAddressId": address_id,
                "Description": "everything at once",
                "RoleARN": "arn:aws:iam::123456789012:role/snowball-cluster-everything",
                "ShippingOption": "EXPRESS",
                "Resources": {
                    "S3Resources": [{ "BucketArn": "arn:aws:s3:::cluster-everything" }]
                },
                "Notification": {
                    "SnsTopicARN": "arn:aws:sns:us-west-2:123456789012:cluster-everything",
                    "NotifyAll": true
                },
                "OnDeviceServiceConfiguration": {
                    "S3OnDeviceService": { "StorageLimit": 1.0, "StorageUnit": "TB" }
                },
            }),
        ),
    ];
    // One row per value the modelled catalog admits, so the rule is asserted over the
    // whole enum rather than over the value one request happened to carry.
    for option in SHIPPING_OPTION_VALUES {
        rows.push((
            format!("the shipping speed {option}"),
            json!({ "ShippingOption": option }),
        ));
    }
    rows
}

/// Assert `metadata` reports exactly what `submitted` asked for, for a cluster in `state`.
fn assert_cluster_metadata_matches(metadata: &Value, submitted: &Value, state: &str, case: &str) {
    assert_eq!(
        metadata.get("ClusterState").and_then(Value::as_str),
        Some(state),
        "case {case:?}: expected ClusterState {state}, got {metadata}"
    );
    for (member, expected) in submitted
        .as_object()
        .expect("the submitted cluster is a JSON object")
    {
        assert_eq!(
            metadata.get(member).unwrap_or(&Value::Null),
            expected,
            "case {case:?}: expected {member} to read back as {expected}, got {metadata}"
        );
    }
}

/// Create a cluster from `body` and return the `ClusterId` the service minted, together
/// with the `JobListEntries` the create reported.
pub(crate) async fn create_cluster(
    router: &MultiServiceRouter,
    body: Value,
    case: &str,
) -> (String, Vec<Value>) {
    let created = ok(router, "CreateCluster", body, case).await;
    let cluster_id = string(&created, "ClusterId", case);
    assert_prefixed_uuid(&cluster_id, "CID", case);
    let entries = created
        .get("JobListEntries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    (cluster_id, entries)
}

#[test]
fn only_the_state_a_cluster_is_created_in_is_updatable() {
    let updatable: Vec<&str> = ALL_CLUSTER_STATES
        .iter()
        .filter(|state| state.is_updatable())
        .map(|state| state.as_wire())
        .collect();
    assert_eq!(
        updatable,
        vec!["AwaitingQuorum"],
        "a cluster's information can only be updated while its ClusterState value is \
         AwaitingQuorum, per UpdateCluster's documentation"
    );
    // The states this emulator spells are the states the model spells, so a reported one
    // is always a value the modelled enum admits.
    let spelled: Vec<&str> = ALL_CLUSTER_STATES.iter().map(|s| s.as_wire()).collect();
    for state in CLUSTER_STATE_VALUES {
        assert!(
            spelled.contains(state),
            "the modelled ClusterState value {state} is not spelled by this emulator: \
             {spelled:?}"
        );
    }
    assert_eq!(
        spelled.len(),
        CLUSTER_STATE_VALUES.len(),
        "expected exactly the modelled ClusterState values, got {spelled:?} against \
         {CLUSTER_STATE_VALUES:?}"
    );
}

#[tokio::test]
async fn update_cluster_writes_the_members_it_names_and_leaves_the_rest_alone() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Cluster Receiver", "1 Cluster Way", json!({})),
        "the address the cluster is created with",
    )
    .await;
    let alternate_address_id = create_address(
        &router,
        deliverable_address(
            "Alternate Cluster Receiver",
            "2 Cluster Way",
            json!({ "City": "Portland", "StateOrProvince": "OR", "PostalCode": "97201" }),
        ),
        "the address an update moves the cluster to",
    )
    .await;

    // What the cluster must read back as: the create, with each applied update merged over
    // it.
    let mut expected = base_cluster(&address_id);
    let case = "the cluster every row updates";
    let (cluster_id, entries) = create_cluster(&router, expected.clone(), case).await;
    assert!(
        entries.is_empty(),
        "case {case:?}: a create that asked for no initial cluster size must report no \
         job, got {entries:?}"
    );
    let created_metadata = cluster_metadata(&router, &cluster_id, case).await;
    let creation_date = created_metadata
        .get("CreationDate")
        .cloned()
        .unwrap_or(Value::Null);
    assert!(
        !creation_date.is_null(),
        "case {case:?}: expected the created cluster to report a CreationDate, \
         got {created_metadata}"
    );
    assert_cluster_metadata_matches(&created_metadata, &expected, "AwaitingQuorum", case);

    for (case, update) in update_rows(&address_id, &alternate_address_id) {
        let answered = ok(
            &router,
            "UpdateCluster",
            update_body(&cluster_id, &update),
            &case,
        )
        .await;
        assert_eq!(
            answered,
            json!({}),
            "case {case:?}: UpdateClusterResult models no member, got {answered}"
        );

        merge(&mut expected, &update);
        let metadata = cluster_metadata(&router, &cluster_id, &case).await;
        assert_cluster_metadata_matches(&metadata, &expected, "AwaitingQuorum", &case);
        assert_eq!(
            metadata.get("ClusterId").and_then(Value::as_str),
            Some(cluster_id.as_str()),
            "case {case:?}: an update must not change the cluster's id, got {metadata}"
        );
        assert_eq!(
            metadata.get("CreationDate"),
            Some(&creation_date),
            "case {case:?}: an update must not change the instant the cluster was \
             created, got {metadata}"
        );
        // Reading does not change what the next caller sees.
        let again = cluster_metadata(&router, &cluster_id, &case).await;
        assert_eq!(
            again, metadata,
            "case {case:?}: two DescribeCluster calls disagreed after the update"
        );
    }
}

#[tokio::test]
async fn update_cluster_is_refused_once_the_cluster_has_left_awaiting_quorum() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Quorum Receiver", "3 Cluster Way", json!({})),
        "the cluster's address",
    )
    .await;

    // A cluster created with its full quorum of node jobs has nothing left to await, so it
    // is past the window the update rule names.
    let setup = "a cluster created with its whole quorum of nodes";
    let mut body = base_cluster(&address_id);
    body["InitialClusterSize"] = json!(CLUSTER_NODE_QUORUM);
    let (cluster_id, entries) = create_cluster(&router, body, setup).await;
    assert_eq!(
        entries.len(),
        CLUSTER_NODE_QUORUM,
        "case {setup:?}: expected one job per node asked for, got {entries:?}"
    );
    let before = cluster_metadata(&router, &cluster_id, setup).await;
    assert_eq!(
        before.get("ClusterState").and_then(Value::as_str),
        Some("Pending"),
        "case {setup:?}: expected the cluster to have left AwaitingQuorum, got {before}"
    );

    // Every row the updatable case accepts is refused here, and changes nothing.
    for (case, update) in update_rows(&address_id, &address_id) {
        refused(
            &router,
            "UpdateCluster",
            update_body(&cluster_id, &update),
            "InvalidJobStateException",
            &case,
        )
        .await;
        let after = cluster_metadata(&router, &cluster_id, &case).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused UpdateCluster changed the cluster"
        );
    }
}

#[tokio::test]
async fn update_cluster_refuses_a_resource_no_record_answers_to() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Known Cluster Receiver", "4 Cluster Way", json!({})),
        "a known address",
    )
    .await;
    let setup = "the cluster the rows name";
    let (cluster_id, _) = create_cluster(&router, base_cluster(&address_id), setup).await;
    let before = cluster_metadata(&router, &cluster_id, setup).await;

    // Each row names one resource that does not exist: the cluster itself, the shipping
    // address, or the forwarding address.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a cluster id that was never created",
            json!({ "ClusterId": UNKNOWN_CLUSTER_ID, "Description": "no such cluster" }),
        ),
        (
            "an AddressId that was never created",
            json!({ "ClusterId": cluster_id, "AddressId": UNKNOWN_ADDRESS_ID }),
        ),
        (
            "a ForwardingAddressId that was never created",
            json!({ "ClusterId": cluster_id, "ForwardingAddressId": UNKNOWN_ADDRESS_ID }),
        ),
        (
            "a known address beside one that was never created",
            json!({
                "ClusterId": cluster_id,
                "AddressId": address_id,
                "ForwardingAddressId": UNKNOWN_ADDRESS_ID
            }),
        ),
    ];
    for (case, body) in rows {
        refused(&router, "UpdateCluster", body, "InvalidResourceException", case).await;
        let after = cluster_metadata(&router, &cluster_id, case).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused UpdateCluster changed the cluster"
        );
    }

    // The reader refuses the same way, and a create naming an address no record answers to
    // stores nothing to read.
    refused(
        &router,
        "DescribeCluster",
        json!({ "ClusterId": UNKNOWN_CLUSTER_ID }),
        "InvalidResourceException",
        "describing a cluster that was never created",
    )
    .await;
    let mut unknown_address = base_cluster(&address_id);
    unknown_address["AddressId"] = json!(UNKNOWN_ADDRESS_ID);
    refused(
        &router,
        "CreateCluster",
        unknown_address,
        "InvalidResourceException",
        "creating a cluster for an address that was never created",
    )
    .await;
}

#[tokio::test]
async fn a_cluster_awaits_its_quorum_until_its_node_jobs_are_created() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Node Receiver", "5 Cluster Way", json!({})),
        "the clusters' address",
    )
    .await;

    // One row per number of node jobs a create can ask for: none at all, the smallest and
    // largest the modelled range admits, one short of the quorum, and the quorum itself.
    // The state is whether the cluster is still waiting for node jobs, so it turns exactly
    // at the quorum.
    let sizes: Vec<Option<i64>> = vec![
        None,
        Some(INITIAL_CLUSTER_SIZE_MIN),
        Some(1),
        Some(CLUSTER_NODE_QUORUM as i64 - 1),
        Some(CLUSTER_NODE_QUORUM as i64),
        Some(INITIAL_CLUSTER_SIZE_MAX),
    ];
    for size in sizes {
        let case = format!("a cluster created with an initial size of {size:?}");
        let mut body = base_cluster(&address_id);
        if let Some(size) = size {
            body["InitialClusterSize"] = json!(size);
        }
        let expected_nodes = usize::try_from(size.unwrap_or(0)).expect("a non-negative size");
        let (cluster_id, entries) = create_cluster(&router, body, &case).await;
        assert_eq!(
            entries.len(),
            expected_nodes,
            "case {case:?}: expected one job per node asked for, got {entries:?}"
        );

        let expected_state = if expected_nodes >= CLUSTER_NODE_QUORUM {
            "Pending"
        } else {
            "AwaitingQuorum"
        };
        let metadata = cluster_metadata(&router, &cluster_id, &case).await;
        assert_cluster_metadata_matches(
            &metadata,
            &base_cluster(&address_id),
            expected_state,
            &case,
        );

        // Each node the create made is a job of its own, associated with the cluster and
        // carrying the attributes it inherited from it.
        for entry in &entries {
            let job_id = string(entry, "JobId", &case);
            assert_prefixed_uuid(&job_id, "JID", &case);
            let node = job_metadata(&router, &job_id, &case).await;
            assert_eq!(
                node.get("ClusterId").and_then(Value::as_str),
                Some(cluster_id.as_str()),
                "case {case:?}: expected the node job to name its cluster, got {node}"
            );
            assert_eq!(
                node.get("JobState").and_then(Value::as_str),
                Some("New"),
                "case {case:?}: expected a created node job to be New, got {node}"
            );
            for member in ["AddressId", "Description", "RoleARN", "Resources", "JobType"] {
                assert_eq!(
                    node.get(member),
                    metadata.get(member),
                    "case {case:?}: expected the node job to inherit the cluster's \
                     {member}, got {node} beside {metadata}"
                );
            }
            assert_eq!(
                node["ShippingDetails"].get("ShippingOption"),
                metadata.get("ShippingOption"),
                "case {case:?}: expected the node job to inherit the cluster's shipping \
                 speed, got {node} beside {metadata}"
            );
        }
    }
}

#[tokio::test]
async fn create_job_joins_a_cluster_until_it_holds_its_quorum() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Joining Receiver", "6 Cluster Way", json!({})),
        "the cluster's address",
    )
    .await;
    let alternate_address_id = create_address(
        &router,
        deliverable_address("Joining Alternate", "7 Cluster Way", json!({})),
        "the address the update moves the cluster to",
    )
    .await;

    let setup = "an empty cluster its nodes are created for one by one";
    let (cluster_id, entries) = create_cluster(&router, base_cluster(&address_id), setup).await;
    assert!(
        entries.is_empty(),
        "case {setup:?}: expected an empty cluster, got {entries:?}"
    );

    // The first node inherits what the cluster was created with.
    let first_case = "the cluster's first node";
    let created = ok(
        &router,
        "CreateJob",
        json!({ "ClusterId": cluster_id }),
        first_case,
    )
    .await;
    let first_node_id = string(&created, "JobId", first_case);
    let first_node = job_metadata(&router, &first_node_id, first_case).await;
    assert_eq!(
        first_node.get("Description").and_then(Value::as_str),
        base_cluster(&address_id)
            .get("Description")
            .and_then(Value::as_str),
        "case {first_case:?}: expected the node to inherit the cluster's description, \
         got {first_node}"
    );

    // A node job inherits the cluster's attributes at the instant it is created, so an
    // update to the cluster reaches the nodes created after it and leaves the ones created
    // before it as they were.
    let update_case = "the cluster updated between two of its nodes";
    let update = json!({
        "Description": "the re-planned cluster",
        "AddressId": alternate_address_id,
    });
    ok(
        &router,
        "UpdateCluster",
        update_body(&cluster_id, &update),
        update_case,
    )
    .await;
    let second = ok(
        &router,
        "CreateJob",
        json!({ "ClusterId": cluster_id }),
        update_case,
    )
    .await;
    let second_node_id = string(&second, "JobId", update_case);
    let second_node = job_metadata(&router, &second_node_id, update_case).await;
    for member in ["Description", "AddressId"] {
        assert_eq!(
            second_node.get(member),
            update.get(member),
            "case {update_case:?}: expected the node created after the update to inherit \
             the updated {member}, got {second_node}"
        );
    }
    let first_again = job_metadata(&router, &first_node_id, update_case).await;
    assert_eq!(
        first_again, first_node,
        "case {update_case:?}: updating the cluster changed a node job created before it"
    );

    // The cluster is still awaiting its quorum until every node job exists, and takes no
    // node past it.
    for node_number in 3..=CLUSTER_NODE_QUORUM {
        let case = format!("node {node_number} of the cluster");
        let before = cluster_metadata(&router, &cluster_id, &case).await;
        assert_eq!(
            before.get("ClusterState").and_then(Value::as_str),
            Some("AwaitingQuorum"),
            "case {case:?}: a cluster short of its quorum is still awaiting it, got {before}"
        );
        ok(
            &router,
            "CreateJob",
            json!({ "ClusterId": cluster_id }),
            &case,
        )
        .await;
    }
    let quorum_case = "the cluster holding its whole quorum";
    let at_quorum = cluster_metadata(&router, &cluster_id, quorum_case).await;
    assert_eq!(
        at_quorum.get("ClusterState").and_then(Value::as_str),
        Some("Pending"),
        "case {quorum_case:?}: a cluster whose node jobs all exist no longer awaits them, \
         got {at_quorum}"
    );
    refused(
        &router,
        "CreateJob",
        json!({ "ClusterId": cluster_id }),
        "ClusterLimitExceededException",
        "one node past the quorum a cluster supports",
    )
    .await;
    refused(
        &router,
        "UpdateCluster",
        update_body(&cluster_id, &json!({ "Description": "too late" })),
        "InvalidJobStateException",
        "an update once the cluster has its quorum",
    )
    .await;
    let after = cluster_metadata(&router, &cluster_id, quorum_case).await;
    assert_eq!(
        after, at_quorum,
        "case {quorum_case:?}: a refused call changed the cluster"
    );
}

#[tokio::test]
async fn a_node_job_takes_the_clusters_job_and_device_type_or_is_refused() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Combination Receiver", "8 Cluster Way", json!({})),
        "the cluster's address",
    )
    .await;
    let setup = "the cluster the node rows join";
    let created_body = base_cluster(&address_id);
    let (cluster_id, _) = create_cluster(&router, created_body.clone(), setup).await;
    let cluster_job_type = created_body["JobType"].as_str().expect("a job type");
    let cluster_snowball_type = created_body["SnowballType"].as_str().expect("a device type");

    // A node request may repeat the cluster's own job and device type, or leave either
    // off: "you only need to provide the clusterId value; the other job attributes are
    // inherited from the cluster". Each accepted row still takes the cluster's values.
    let accepted: Vec<(&str, Value)> = vec![
        ("the cluster id alone", json!({})),
        (
            "the cluster's own job type repeated",
            json!({ "JobType": cluster_job_type }),
        ),
        (
            "the cluster's own device type repeated",
            json!({ "SnowballType": cluster_snowball_type }),
        ),
        (
            "both of the cluster's own types repeated",
            json!({
                "JobType": cluster_job_type,
                "SnowballType": cluster_snowball_type
            }),
        ),
    ];
    for (case, row) in accepted {
        let mut body = row;
        body["ClusterId"] = json!(cluster_id);
        let answered = ok(&router, "CreateJob", body, case).await;
        let node_id = string(&answered, "JobId", case);
        let node = job_metadata(&router, &node_id, case).await;
        assert_eq!(
            node.get("JobType").and_then(Value::as_str),
            Some(cluster_job_type),
            "case {case:?}: expected the node to carry the cluster's job type, got {node}"
        );
        assert_eq!(
            node.get("SnowballType").and_then(Value::as_str),
            Some(cluster_snowball_type),
            "case {case:?}: expected the node to carry the cluster's device type, got {node}"
        );
    }

    // A row that names a type the cluster does not have is a combination that cannot hold:
    // "Confirm that the CreateClusterRequest$SnowballType value supports your
    // CreateJobRequest$JobType, and try again" (`InvalidInputCombinationException`). Each
    // row differs along which type disagrees, and over values from the modelled catalogs.
    let refusals: Vec<(&str, Value)> = vec![
        ("another job type", json!({ "JobType": "IMPORT" })),
        ("a third job type", json!({ "JobType": "EXPORT" })),
        ("another device type", json!({ "SnowballType": "EDGE_C" })),
        ("a third device type", json!({ "SnowballType": "SNC1_SSD" })),
        (
            "both types disagreeing at once",
            json!({ "JobType": "IMPORT", "SnowballType": "EDGE_CG" }),
        ),
        (
            "the cluster's device type beside another job type",
            json!({ "JobType": "IMPORT", "SnowballType": cluster_snowball_type }),
        ),
    ];
    let jobs_before = cluster_node_count(&router, &cluster_id, "before the refused rows").await;
    for (case, row) in refusals {
        let mut body = row;
        body["ClusterId"] = json!(cluster_id);
        refused(
            &router,
            "CreateJob",
            body,
            "InvalidInputCombinationException",
            case,
        )
        .await;
        assert_eq!(
            cluster_node_count(&router, &cluster_id, case).await,
            jobs_before,
            "case {case:?}: a refused CreateJob created a node anyway"
        );
    }
}

/// How many jobs `ListJobs` reports for `cluster_id`: the nodes the cluster holds, read
/// back through the list reader rather than out of the state struct.
async fn cluster_node_count(router: &MultiServiceRouter, cluster_id: &str, case: &str) -> usize {
    let listed = ok(router, "ListJobs", json!({ "MaxResults": 100 }), case).await;
    let entries = listed["JobListEntries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected JobListEntries, got {listed}"))
        .clone();
    let mut nodes = 0usize;
    for entry in entries {
        let job_id = string(&entry, "JobId", case);
        let node = job_metadata(router, &job_id, case).await;
        if node.get("ClusterId").and_then(Value::as_str) == Some(cluster_id) {
            nodes += 1;
        }
    }
    nodes
}
