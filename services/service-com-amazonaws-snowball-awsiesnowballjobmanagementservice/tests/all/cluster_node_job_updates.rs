// SPDX-License-Identifier: CC0-1.0

//! What an `UpdateJob` that names a cluster's node job reaches: the cluster AND every other
//! node job that cluster holds, driven end to end over HTTP.
//!
//! One rule, asserted over a TABLE of inputs rather than over a single request: a record
//! that carries another record's attributes rather than copies of its own — a cluster's node
//! job, whose attributes are the cluster's ("you need to provide only this clusterId value.
//! The other job attributes are inherited from the cluster", `CreateJobRequest$ClusterId`,
//! and `ClusterMetadata` models ONE address and ONE shipping speed for the whole cluster — is
//! updated as the whole order it belongs to, so a member an update names lands on every
//! record of that order still within its own update window and on none of another order,
//! leaving no two records of one order disagreeing; a member only the named record models is
//! that record's own; and a member of an order whose own update window has closed cannot be
//! updated from either side, so naming one is refused with `InvalidJobStateException`.
//!
//! The rows differ along what the rule generalises over: which members the request carries
//! (none, one, several, all of them, the members only a job models), the enum value from each
//! modelled catalog (`SHIPPING_OPTION_VALUES` through `cluster_updates::update_rows`,
//! `SNOWBALL_CAPACITY_VALUES`, read from the crate rather than restated), the length of a
//! value at each published bound of `Description`, the shape of the structure members, how
//! many node jobs the cluster holds, which path minted them (`CreateCluster`'s
//! `InitialClusterSize`, `CreateJob` naming the cluster, both), which of those nodes the
//! update names, and whether the cluster is still in the state it is updatable in.
//!
//! The harness is the one `job_lifecycle`, `cluster_updates` and `cluster_node_updates`
//! already drive the service with, so every call here goes through the same front door a
//! client uses and nothing reaches into the state struct.

use serde_json::{json, Value};
// The catalog the modelled `SnowballCapacity` enum admits and the quorum this service holds
// a cluster to, read from the crate rather than restated, so a row cannot drift from the
// values the model allows or from the bound the handlers enforce.
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::types::SNOWBALL_CAPACITY_VALUES;
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::state::CLUSTER_NODE_QUORUM;
use smithy_server_runtime::MultiServiceRouter;

use super::cluster_node_updates::{assert_node_agrees_with_cluster, at, cluster_with_nodes};
use super::cluster_updates::{
    base_cluster, cluster_metadata, create_cluster, job_metadata, update_rows,
};
use super::job_lifecycle::{create_address, deliverable_address, ok, refused, router, string};

/// The body of an `UpdateJob` call: the update's members plus the job it names.
fn update_body(job_id: &str, update: &Value) -> Value {
    let mut body = update.clone();
    body["JobId"] = json!(job_id);
    body
}

/// Every update row carrying only members `UpdateJobRequest` models and
/// `UpdateClusterRequest` does not (`smithy_query(query="operation", operation="UpdateJob")`
/// beside `smithy_query(query="operation", operation="UpdateCluster")`): the device capacity
/// a job is configured with — one row per value the modelled catalog admits — and the person
/// picking the device up. These are the named job's own, not the cluster's.
fn job_member_rows() -> Vec<(String, Value)> {
    let mut rows: Vec<(String, Value)> = vec![
        (
            "pickup details".to_string(),
            json!({
                "PickupDetails": {
                    "Name": "Node Pickup Person",
                    "PhoneNumber": "+1 206 555 0133",
                    "Email": "node-pickup@example.com"
                }
            }),
        ),
        (
            "pickup details naming the person alone".to_string(),
            json!({ "PickupDetails": { "Name": "Node Pickup Only" } }),
        ),
    ];
    for capacity in SNOWBALL_CAPACITY_VALUES {
        rows.push((
            format!("the capacity preference {capacity}"),
            json!({ "SnowballCapacityPreference": capacity }),
        ));
    }
    rows
}

/// Create one node job of `cluster_id` the way a replacement node is created — "you only
/// need to provide the clusterId value; the other job attributes are inherited from the
/// cluster" (`CreateJobRequest$ClusterId`) — and return the job id it was minted under.
async fn attach_node(router: &MultiServiceRouter, cluster_id: &str, case: &str) -> String {
    let created = ok(router, "CreateJob", json!({ "ClusterId": cluster_id }), case).await;
    string(&created, "JobId", case)
}

#[tokio::test]
async fn update_job_on_a_cluster_node_reaches_the_cluster_and_its_other_nodes() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Node Job Update Receiver", "13 Cluster Way", json!({})),
        "the address the clusters are created with",
    )
    .await;
    let alternate_address_id = create_address(
        &router,
        deliverable_address(
            "Alternate Node Job Receiver",
            "14 Cluster Way",
            json!({ "City": "Portland", "StateOrProvince": "OR", "PostalCode": "97201" }),
        ),
        "the address an update moves the clusters to",
    )
    .await;

    // One row per shape of cluster membership and per choice of which node the update
    // names: the only node, the first of several, the last of several, one minted by the
    // create, one attached by `CreateJob`.
    let shapes: Vec<(&str, usize, usize, usize)> = vec![
        ("one node job minted by the create", 1, 0, 0),
        ("two node jobs minted by the create, the first named", 2, 0, 0),
        ("two node jobs minted by the create, the last named", 2, 0, 1),
        ("one node job attached by CreateJob", 0, 1, 0),
        (
            "node jobs from both paths, an attached one named",
            2,
            CLUSTER_NODE_QUORUM - 4,
            2,
        ),
        (
            "one short of the quorum, the last named",
            CLUSTER_NODE_QUORUM - 1,
            0,
            CLUSTER_NODE_QUORUM - 2,
        ),
    ];

    for (shape, initial, attached, named) in shapes {
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
        let named_id = node_ids[named].clone();

        for (row, update) in update_rows(&address_id, &alternate_address_id) {
            let case = format!("{shape}, the node updated with {row}");
            let answered = ok(&router, "UpdateJob", update_body(&named_id, &update), &case).await;
            assert_eq!(
                answered,
                json!({}),
                "case {case:?}: UpdateJobResult models no member, got {answered}"
            );

            // The cluster whose attributes the node carries took the update.
            let cluster = cluster_metadata(&router, &cluster_id, &case).await;
            assert_eq!(
                at(&cluster, "ClusterState"),
                json!("AwaitingQuorum"),
                "case {case:?}: an update to a node must not move the cluster's state, \
                 got {cluster}"
            );
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

            // And so did every node of it, the one the request named included: no two
            // records of one cluster order devices to different addresses at different
            // speeds.
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
async fn a_node_created_after_the_update_inherits_the_updated_cluster() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Later Node Receiver", "15 Cluster Way", json!({})),
        "the cluster's address",
    )
    .await;
    let alternate_address_id = create_address(
        &router,
        deliverable_address("Later Node Alternate", "16 Cluster Way", json!({})),
        "the address the update moves the order to",
    )
    .await;

    // Each row updates one node of its own cluster and then attaches a node, which must
    // inherit what the update left behind rather than what the create asked for.
    for (row, update) in update_rows(&address_id, &alternate_address_id) {
        let case = format!("a node attached after an update with {row}");
        let (cluster_id, node_ids) = cluster_with_nodes(&router, &address_id, 2, 0, &case).await;
        ok(
            &router,
            "UpdateJob",
            update_body(&node_ids[0], &update),
            &case,
        )
        .await;

        let later_id = attach_node(&router, &cluster_id, &case).await;
        let later_created = job_metadata(&router, &later_id, &case).await;
        let cluster = cluster_metadata(&router, &cluster_id, &case).await;
        assert_node_agrees_with_cluster(
            &router,
            &cluster,
            &cluster_id,
            &later_id,
            &later_created,
            &case,
        )
        .await;
    }
}

#[tokio::test]
async fn a_member_only_a_job_models_lands_on_the_node_the_request_named_alone() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Node Own Member Receiver", "17 Cluster Way", json!({})),
        "the cluster's address",
    )
    .await;

    let setup = "a cluster holding two node jobs";
    let (cluster_id, node_ids) = cluster_with_nodes(&router, &address_id, 2, 0, setup).await;
    let (named_id, sibling_id) = (node_ids[0].clone(), node_ids[1].clone());
    let cluster_before = cluster_metadata(&router, &cluster_id, setup).await;
    let sibling_before = job_metadata(&router, &sibling_id, setup).await;

    for (row, update) in job_member_rows() {
        let case = format!("{setup}, one node updated with {row}");
        ok(&router, "UpdateJob", update_body(&named_id, &update), &case).await;

        let named = job_metadata(&router, &named_id, &case).await;
        for (member, expected) in update
            .as_object()
            .expect("an update row is a JSON object")
            .iter()
        {
            assert_eq!(
                &at(&named, member),
                expected,
                "case {case:?}: expected the named node's {member} to read back as \
                 {expected}, got {named}"
            );
        }
        let sibling_after = job_metadata(&router, &sibling_id, &case).await;
        assert_eq!(
            sibling_after, sibling_before,
            "case {case:?}: a member only a job models is not the cluster's, so it must \
             leave the other node alone, got {sibling_after} after {sibling_before}"
        );
        let cluster_after = cluster_metadata(&router, &cluster_id, &case).await;
        assert_eq!(
            cluster_after, cluster_before,
            "case {case:?}: a member only a job models is not the cluster's, so it must \
             leave the cluster alone, got {cluster_after} after {cluster_before}"
        );
    }
}

#[tokio::test]
async fn a_node_of_a_cluster_past_its_update_window_keeps_the_clusters_attributes() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Closed Window Receiver", "18 Cluster Way", json!({})),
        "the cluster's address",
    )
    .await;
    let alternate_address_id = create_address(
        &router,
        deliverable_address("Closed Window Alternate", "19 Cluster Way", json!({})),
        "the address the refused updates name",
    )
    .await;

    // A cluster holding its quorum of node jobs has left `AwaitingQuorum`, the one state a
    // cluster's information can be updated from, while its nodes are still `New`: so what
    // closes the window on the cluster's attributes here is the cluster's own state.
    let setup = "a cluster holding its quorum of node jobs";
    let mut body = base_cluster(&address_id);
    body["InitialClusterSize"] = json!(CLUSTER_NODE_QUORUM);
    let (cluster_id, entries) = create_cluster(&router, body, setup).await;
    let node_ids: Vec<String> = entries
        .iter()
        .map(|entry| string(entry, "JobId", setup))
        .collect();
    assert_eq!(
        node_ids.len(),
        CLUSTER_NODE_QUORUM,
        "case {setup:?}: expected one job per node of the quorum, got {entries:?}"
    );
    let cluster_before = cluster_metadata(&router, &cluster_id, setup).await;
    assert_eq!(
        at(&cluster_before, "ClusterState"),
        json!("Pending"),
        "case {setup:?}: a cluster holding its quorum has left AwaitingQuorum, \
         got {cluster_before}"
    );
    let mut nodes_before = Vec::new();
    for job_id in &node_ids {
        let node = job_metadata(&router, job_id, setup).await;
        assert_eq!(
            at(&node, "JobState"),
            json!("New"),
            "case {setup:?}: expected node {job_id} to still be New, got {node}"
        );
        nodes_before.push(node);
    }

    for (row, update) in update_rows(&address_id, &alternate_address_id) {
        let case = format!("{setup}, a node updated with {row}");
        let body = update_body(&node_ids[0], &update);
        let names_a_cluster_member = !update
            .as_object()
            .expect("an update row is a JSON object")
            .is_empty();
        if names_a_cluster_member {
            refused(
                &router,
                "UpdateJob",
                body,
                "InvalidJobStateException",
                &case,
            )
            .await;
        } else {
            // A request naming no member of the order asks for nothing the closed window
            // withholds, so it is served and changes nothing.
            let answered = ok(&router, "UpdateJob", body, &case).await;
            assert_eq!(
                answered,
                json!({}),
                "case {case:?}: UpdateJobResult models no member, got {answered}"
            );
        }

        let cluster_after = cluster_metadata(&router, &cluster_id, &case).await;
        assert_eq!(
            cluster_after, cluster_before,
            "case {case:?}: expected the cluster past its update window to be unchanged, \
             got {cluster_after} after {cluster_before}"
        );
        for (job_id, before) in node_ids.iter().zip(nodes_before.iter()) {
            let after = job_metadata(&router, job_id, &case).await;
            assert_eq!(
                &after, before,
                "case {case:?}: expected node {job_id} to be unchanged, got {after} \
                 after {before}"
            );
        }
    }

    // The members only a job models are not the cluster's, so the closed window withholds
    // none of them: each still lands on the node the request names.
    for (row, update) in job_member_rows() {
        let case = format!("{setup}, a node updated with {row}");
        ok(
            &router,
            "UpdateJob",
            update_body(&node_ids[0], &update),
            &case,
        )
        .await;
        let named = job_metadata(&router, &node_ids[0], &case).await;
        for (member, expected) in update
            .as_object()
            .expect("an update row is a JSON object")
            .iter()
        {
            assert_eq!(
                &at(&named, member),
                expected,
                "case {case:?}: expected the named node's {member} to read back as \
                 {expected}, got {named}"
            );
        }
        let cluster_after = cluster_metadata(&router, &cluster_id, &case).await;
        assert_eq!(
            cluster_after, cluster_before,
            "case {case:?}: a member only a job models must leave the cluster alone, \
             got {cluster_after} after {cluster_before}"
        );
    }
}
