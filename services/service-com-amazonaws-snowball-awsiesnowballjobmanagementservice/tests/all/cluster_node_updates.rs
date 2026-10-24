// SPDX-License-Identifier: CC0-1.0

//! What an `UpdateCluster` reaches: the cluster AND the node jobs that cluster already
//! holds, driven end to end over HTTP.
//!
//! One rule, asserted over a TABLE of inputs rather than over a single request: a cluster's
//! node job holds the cluster's attributes ("you need to provide only this clusterId value.
//! The other job attributes are inherited from the cluster", `CreateJobRequest$ClusterId`),
//! so every member an `UpdateCluster` names is reported by a later `DescribeJob` on each of
//! that cluster's node jobs exactly as `DescribeCluster` reports it — for each node that is
//! itself still within its own update window — while a member the update omits is left as
//! it was, and the node keeps its own id, state, cluster and creation instant.
//!
//! The rows differ along what the rule generalises over: how many node jobs the cluster
//! holds (none, one, two, one short of the quorum), which path minted them
//! (`CreateCluster`'s `InitialClusterSize`, `CreateJob` naming the cluster, both), and which
//! members the update carries — the whole row set `cluster_updates` drives the cluster with
//! (one per modelled member, one per value of the modelled `ShippingOption` catalog, one at
//! each published bound of `Description`, one carrying every member at once, one carrying
//! none). A node past its own update window is its own row.
//!
//! The harness is the one `job_lifecycle` and `cluster_updates` already drive the service
//! with, so every call here goes through the same front door a client uses and nothing
//! reaches into the state struct.

use serde_json::{json, Value};
// The quorum this service holds a cluster to, read from the crate rather than restated, so
// a row cannot drift from the bound the handler enforces: a cluster is updatable only while
// it is short of its quorum.
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::state::CLUSTER_NODE_QUORUM;
use smithy_server_runtime::MultiServiceRouter;

use super::cluster_updates::{
    base_cluster, cluster_metadata, create_cluster, job_metadata, update_body, update_rows,
};
use super::job_lifecycle::{create_address, deliverable_address, ok, router, string};

/// The members a node job holds the cluster's value for: every member
/// `UpdateClusterRequest` models (`smithy_query(query="operation", operation="UpdateCluster")`)
/// that `JobMetadata` reports under the same name as `ClusterMetadata`. The remaining
/// modelled member, `ShippingOption`, is reported by a job under `ShippingDetails` and is
/// checked beside these.
const INHERITED_MEMBERS: &[&str] = &[
    "AddressId",
    "ForwardingAddressId",
    "Description",
    "RoleARN",
    "Resources",
    "Notification",
    "OnDeviceServiceConfiguration",
];

/// `body`'s member `name`, reading an absent member as JSON null, so a member neither record
/// reports compares equal.
fn at(body: &Value, name: &str) -> Value {
    body.get(name).cloned().unwrap_or(Value::Null)
}

/// A cluster still short of its quorum, holding `initial` node jobs minted by the create and
/// `attached` node jobs attached one by one by `CreateJob`: its id and every node job id, in
/// creation order.
async fn cluster_with_nodes(
    router: &MultiServiceRouter,
    address_id: &str,
    initial: usize,
    attached: usize,
    case: &str,
) -> (String, Vec<String>) {
    assert!(
        initial + attached < CLUSTER_NODE_QUORUM,
        "case {case:?}: a cluster holding its quorum has left AwaitingQuorum, so it is not \
         updatable: asked for {initial} + {attached} nodes"
    );
    let mut body = base_cluster(address_id);
    if initial > 0 {
        body["InitialClusterSize"] = json!(initial);
    }
    let (cluster_id, entries) = create_cluster(router, body, case).await;
    assert_eq!(
        entries.len(),
        initial,
        "case {case:?}: expected one job per node asked for, got {entries:?}"
    );
    let mut node_ids: Vec<String> = entries
        .iter()
        .map(|entry| string(entry, "JobId", case))
        .collect();
    for node in 0..attached {
        let created = ok(
            router,
            "CreateJob",
            json!({ "ClusterId": cluster_id }),
            &format!("{case}: attached node {node}"),
        )
        .await;
        node_ids.push(string(&created, "JobId", case));
    }
    (cluster_id, node_ids)
}

/// Assert the node job `job_id` reports the attributes `cluster` reports, and nothing of its
/// own changed: it still names its cluster, is still `New`, and was still created when it
/// was.
async fn assert_node_agrees_with_cluster(
    router: &MultiServiceRouter,
    cluster: &Value,
    cluster_id: &str,
    job_id: &str,
    created: &Value,
    case: &str,
) {
    let node = job_metadata(router, job_id, case).await;
    for name in INHERITED_MEMBERS {
        assert_eq!(
            at(&node, name),
            at(cluster, name),
            "case {case:?}: expected node {job_id} to report its cluster's {name} {}, got {} \
             (node {node})",
            at(cluster, name),
            at(&node, name)
        );
    }
    let node_option = at(&at(&node, "ShippingDetails"), "ShippingOption");
    assert_eq!(
        node_option,
        at(cluster, "ShippingOption"),
        "case {case:?}: expected node {job_id} to report its cluster's shipping speed {}, \
         got {node_option} (node {node})",
        at(cluster, "ShippingOption")
    );
    assert_eq!(
        at(&node, "ClusterId"),
        json!(cluster_id),
        "case {case:?}: an update must not change which cluster node {job_id} belongs to, \
         got {node}"
    );
    assert_eq!(
        at(&node, "JobState"),
        json!("New"),
        "case {case:?}: an update must not move node {job_id} out of New, got {node}"
    );
    assert_eq!(
        at(&node, "CreationDate"),
        at(created, "CreationDate"),
        "case {case:?}: an update must not change when node {job_id} was created, got {node} \
         after {created}"
    );
    assert_eq!(
        at(&node, "JobId"),
        json!(job_id),
        "case {case:?}: expected node {job_id} to keep its own id, got {node}"
    );
}

#[tokio::test]
async fn update_cluster_reaches_the_node_jobs_the_cluster_already_holds() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Node Update Receiver", "9 Cluster Way", json!({})),
        "the address the clusters are created with",
    )
    .await;
    let alternate_address_id = create_address(
        &router,
        deliverable_address(
            "Alternate Node Update Receiver",
            "10 Cluster Way",
            json!({ "City": "Portland", "StateOrProvince": "OR", "PostalCode": "97201" }),
        ),
        "the address an update moves the clusters to",
    )
    .await;

    // One row per shape of cluster membership: no node at all, one node, two nodes, one
    // short of the quorum, each minted by the create, attached by `CreateJob`, or both.
    let shapes: Vec<(&str, usize, usize)> = vec![
        ("a cluster holding no node job yet", 0, 0),
        ("one node job minted by the create", 1, 0),
        ("two node jobs minted by the create", 2, 0),
        (
            "one short of the quorum, all minted by the create",
            CLUSTER_NODE_QUORUM - 1,
            0,
        ),
        ("one node job attached by CreateJob", 0, 1),
        (
            "one short of the quorum, all attached by CreateJob",
            0,
            CLUSTER_NODE_QUORUM - 1,
        ),
        ("node jobs from both paths", 2, CLUSTER_NODE_QUORUM - 3),
    ];

    for (shape, initial, attached) in shapes {
        let (cluster_id, node_ids) =
            cluster_with_nodes(&router, &address_id, initial, attached, shape).await;
        assert_eq!(
            node_ids.len(),
            initial + attached,
            "case {shape:?}: expected every node asked for, got {node_ids:?}"
        );
        // What each node reported when it was created, so a later read proves only the
        // updated members moved.
        let mut created = Vec::new();
        for job_id in &node_ids {
            created.push(job_metadata(&router, job_id, shape).await);
        }

        for (row, update) in update_rows(&address_id, &alternate_address_id) {
            let case = format!("{shape}, updated with {row}");
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

            let cluster = cluster_metadata(&router, &cluster_id, &case).await;
            assert_eq!(
                at(&cluster, "ClusterState"),
                json!("AwaitingQuorum"),
                "case {case:?}: a cluster short of its quorum is still awaiting it, \
                 got {cluster}"
            );
            // The cluster took the update, which is what the nodes must then agree with.
            for (member, expected) in update
                .as_object()
                .expect("an update row is a JSON object")
                .iter()
            {
                assert_eq!(
                    &at(&cluster, member),
                    expected,
                    "case {case:?}: expected the cluster's {member} to read back as \
                     {expected}, got {cluster}"
                );
            }

            for (job_id, created) in node_ids.iter().zip(created.iter()) {
                assert_node_agrees_with_cluster(
                    &router,
                    &cluster,
                    &cluster_id,
                    job_id,
                    created,
                    &case,
                )
                .await;
            }
        }
    }
}

#[tokio::test]
async fn an_update_leaves_a_node_job_past_its_own_update_window_alone() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Window Receiver", "11 Cluster Way", json!({})),
        "the cluster's address",
    )
    .await;
    let alternate_address_id = create_address(
        &router,
        deliverable_address("Window Alternate", "12 Cluster Way", json!({})),
        "the address the update moves the cluster to",
    )
    .await;

    let setup = "a cluster one of whose nodes was cancelled";
    let (cluster_id, node_ids) = cluster_with_nodes(&router, &address_id, 2, 0, setup).await;
    let (cancelled_id, live_id) = (&node_ids[0], &node_ids[1]);
    ok(
        &router,
        "CancelJob",
        json!({ "JobId": cancelled_id }),
        setup,
    )
    .await;
    let cancelled_before = job_metadata(&router, cancelled_id, setup).await;
    assert_eq!(
        at(&cancelled_before, "JobState"),
        json!("Cancelled"),
        "case {setup:?}: expected the cancelled node to be Cancelled, got {cancelled_before}"
    );
    let live_before = job_metadata(&router, live_id, setup).await;

    for (row, update) in update_rows(&address_id, &alternate_address_id) {
        let case = format!("{setup}, updated with {row}");
        ok(
            &router,
            "UpdateCluster",
            update_body(&cluster_id, &update),
            &case,
        )
        .await;

        let cluster = cluster_metadata(&router, &cluster_id, &case).await;
        assert_node_agrees_with_cluster(
            &router,
            &cluster,
            &cluster_id,
            live_id,
            &live_before,
            &case,
        )
        .await;
        let cancelled_after = job_metadata(&router, cancelled_id, &case).await;
        assert_eq!(
            cancelled_after, cancelled_before,
            "case {case:?}: an update must leave a node past its own update window alone, \
             got {cancelled_after} after {cancelled_before}"
        );
    }
}
