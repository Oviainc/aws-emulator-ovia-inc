// SPDX-License-Identifier: CC0-1.0

//! Whether any create or update can leave one record of a cluster's order disagreeing with
//! another, driven end to end over HTTP.
//!
//! One rule, asserted over a TABLE of inputs rather than over a single request: the
//! attributes a cluster holds for the whole order it ships — the set `UpdateCluster` and
//! `UpdateJob` both write to the cluster AND to every node job of it, because
//! `ClusterMetadata` reports ONE value of each for the whole cluster — have one value per
//! order, so a `CreateJob` naming a `ClusterId` cannot mint a node carrying a value of its
//! own for one of them: a request repeating the cluster's value is served and the node
//! reports it, and a request naming a different value is refused with
//! `InvalidInputCombinationException`, storing nothing. A later update to any record of the
//! order therefore has no per-node value to overwrite, while the members a node does hold
//! for itself survive every such update.
//!
//! The rows differ along what the rule generalises over: which members the request carries
//! (none, one, several, every one of them) — the whole `update_rows` table the cluster
//! suites drive the two updates with, read from there rather than restated — whether each
//! named value agrees with the cluster's or differs from it, the enum value from the
//! modelled `ShippingOption` catalog and the `SnowballCapacity` catalog those rows span, the
//! length of a value at each published bound of `Description`, the shape of the structure
//! members, how many node jobs the order holds and which path minted them, and which record
//! of the order the update names.
//!
//! The harness is the one `job_lifecycle`, `cluster_updates`, `cluster_node_updates` and
//! `cluster_node_job_members` already drive the service with, so every call here goes
//! through the same front door a client uses and nothing reaches into the state struct.

use serde_json::{json, Value};
// The catalog the modelled `SnowballCapacity` enum admits, read from the crate rather than
// restated, so a node's own capacity cannot drift from the values the model allows.
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::types::SNOWBALL_CAPACITY_VALUES;
use smithy_server_runtime::MultiServiceRouter;

use super::cluster_node_creation_window::listed_node_ids;
use super::cluster_node_job_members::refused_combination;
use super::cluster_node_updates::{assert_node_agrees_with_cluster, at, cluster_with_nodes};
use super::cluster_updates::{cluster_metadata, job_metadata, update_rows};
use super::job_lifecycle::{create_address, deliverable_address, ok, router, string};

/// How many node jobs each order below starts with, and by which path they were minted: the
/// create's `InitialClusterSize` and a `CreateJob` naming the cluster, so the rule is
/// exercised over more than one record and over both ways a node arrives. One node short of
/// the quorum is left free for the create each row attempts.
const INITIAL_NODES: usize = 1;
const ATTACHED_NODES: usize = 1;

/// Create a node of `cluster_id` carrying `supplied`, answering the `JobId` minted for it.
async fn attach_node(
    router: &MultiServiceRouter,
    cluster_id: &str,
    supplied: &Value,
    case: &str,
) -> String {
    let mut body = supplied.clone();
    body["ClusterId"] = json!(cluster_id);
    let created = ok(router, "CreateJob", body, case).await;
    string(&created, "JobId", case)
}

/// Whether every member `named` carries is already the value `cluster` reports for it, so a
/// node create naming them asks for nothing the order does not already hold.
fn agrees_with_cluster(cluster: &Value, named: &Value) -> bool {
    named
        .as_object()
        .expect("the row is a JSON object")
        .iter()
        .all(|(member, value)| &at(cluster, member) == value)
}

#[tokio::test]
async fn a_node_create_may_only_repeat_the_members_an_update_writes_across_the_order() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Order Invariant Receiver", "22 Cluster Way", json!({})),
        "the address the orders are created with",
    )
    .await;
    let alternate_address_id = create_address(
        &router,
        deliverable_address(
            "Order Invariant Alternate",
            "23 Cluster Way",
            json!({ "City": "Portland", "StateOrProvince": "OR", "PostalCode": "97201" }),
        ),
        "the address a row names instead of the order's",
    )
    .await;

    // The member set is the one the two updates write across the whole order: every row of
    // it is attempted as a node create, so the two rules are asserted against each other
    // rather than each against itself.
    for (row, named) in update_rows(&address_id, &alternate_address_id) {
        let case = format!("a node created naming {row}");
        let (cluster_id, node_ids) = cluster_with_nodes(
            &router,
            &address_id,
            INITIAL_NODES,
            ATTACHED_NODES,
            &case,
        )
        .await;
        let cluster_before = cluster_metadata(&router, &cluster_id, &case).await;
        let mut created_before = Vec::new();
        for job_id in &node_ids {
            created_before.push(job_metadata(&router, job_id, &case).await);
        }

        if agrees_with_cluster(&cluster_before, &named) {
            // Nothing in the request differs from the order, so the create is served and
            // the node reports the order it joined.
            let node_id = attach_node(&router, &cluster_id, &named, &case).await;
            let created = job_metadata(&router, &node_id, &case).await;
            assert_node_agrees_with_cluster(
                &router,
                &cluster_before,
                &cluster_id,
                &node_id,
                &created,
                &case,
            )
            .await;
            let mut expected_listing = node_ids.clone();
            expected_listing.push(node_id);
            assert_eq!(
                listed_node_ids(&router, &cluster_id, &case).await,
                expected_listing,
                "case {case:?}: expected the accepted node to join the cluster's listing"
            );
        } else {
            let message = refused_combination(&router, &cluster_id, &named, &case).await;
            assert!(
                message.contains(&cluster_id),
                "case {case:?}: expected the refusal to name the cluster {cluster_id}, got \
                 {message:?}"
            );
            assert_eq!(
                listed_node_ids(&router, &cluster_id, &case).await,
                node_ids,
                "case {case:?}: a refused node create changed the cluster's node listing"
            );
        }

        // Either way the order itself is untouched: the create neither moved the cluster
        // nor rewrote the nodes it already held.
        let cluster_after = cluster_metadata(&router, &cluster_id, &case).await;
        assert_eq!(
            cluster_after, cluster_before,
            "case {case:?}: a node create changed the cluster it joined, got \
             {cluster_after} after {cluster_before}"
        );
        for (job_id, before) in node_ids.iter().zip(created_before.iter()) {
            let after = job_metadata(&router, job_id, &case).await;
            assert_eq!(
                &after, before,
                "case {case:?}: a node create changed the node {job_id} the cluster already \
                 held, got {after} after {before}"
            );
        }
    }
}

#[tokio::test]
async fn an_update_to_one_record_of_an_order_finds_no_per_node_value_to_discard() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Survival Receiver", "24 Cluster Way", json!({})),
        "the address the orders are created with",
    )
    .await;
    let alternate_address_id = create_address(
        &router,
        deliverable_address("Survival Alternate", "25 Cluster Way", json!({})),
        "the address the updates move the orders to",
    )
    .await;

    for (row, update) in update_rows(&address_id, &alternate_address_id) {
        let case = format!("an order updated with {row}");
        let (cluster_id, mut node_ids) = cluster_with_nodes(
            &router,
            &address_id,
            INITIAL_NODES,
            ATTACHED_NODES,
            &case,
        )
        .await;
        // One more node, carrying the only members a node holds for itself: the capacity
        // its device is configured with and the person picking it up. These are what an
        // update to another record must leave alone.
        let own_members = json!({
            "SnowballCapacityPreference": SNOWBALL_CAPACITY_VALUES[0],
            "PickupDetails": { "Name": "Survival Pickup Person" },
        });
        let own_node_id = attach_node(&router, &cluster_id, &own_members, &case).await;
        node_ids.push(own_node_id.clone());

        let mut created = Vec::new();
        for job_id in &node_ids {
            created.push(job_metadata(&router, job_id, &case).await);
        }

        // The update names the node that carries members of its own, so a rule that wrote
        // the cluster's values over everything a node holds would be caught here.
        let mut body = update.clone();
        body["JobId"] = json!(own_node_id);
        ok(&router, "UpdateJob", body, &case).await;

        let cluster = cluster_metadata(&router, &cluster_id, &case).await;
        for (job_id, created) in node_ids.iter().zip(created.iter()) {
            assert_node_agrees_with_cluster(
                &router, &cluster, &cluster_id, job_id, created, &case,
            )
            .await;
        }

        // And the members only a job has are still the ones that node was created with:
        // the update reached the order, not the node's own configuration.
        let own_node = job_metadata(&router, &own_node_id, &case).await;
        for (member, expected) in own_members
            .as_object()
            .expect("the node's own members are a JSON object")
        {
            assert_eq!(
                &at(&own_node, member),
                expected,
                "case {case:?}: expected the node to keep the {member} only it has \
                 ({expected}), got {own_node}"
            );
        }

        // A node created after the update inherits what the update left behind, and may
        // still state its own capacity: the order has one value of each attribute, whenever
        // a node joins it.
        let later_case = format!("{case}, a node created afterwards");
        let later_id = attach_node(&router, &cluster_id, &own_members, &later_case).await;
        let later = job_metadata(&router, &later_id, &later_case).await;
        assert_node_agrees_with_cluster(
            &router,
            &cluster,
            &cluster_id,
            &later_id,
            &later,
            &later_case,
        )
        .await;
        assert_eq!(
            at(&later, "SnowballCapacityPreference"),
            own_members["SnowballCapacityPreference"],
            "case {later_case:?}: expected the later node to keep the capacity it asked \
             for, got {later}"
        );
    }
}
