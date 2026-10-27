// SPDX-License-Identifier: CC0-1.0

//! The members a cluster holds for its whole order, applied from either end of that order,
//! driven end to end over HTTP.
//!
//! One rule, asserted over a TABLE of inputs rather than over a single request: a cluster
//! holds one value of each of these members for its whole order — "you need to provide only
//! this clusterId value. The other job attributes are inherited from the cluster"
//! (`CreateJobRequest$ClusterId`), and `ClusterMetadata` models ONE address and ONE shipping
//! speed for the whole cluster — so updating the order through the cluster
//! (`UpdateCluster`) and updating it through one of the cluster's node jobs (`UpdateJob`)
//! apply the same set the same way: every member the request supplies lands on the cluster
//! and on each of its node jobs, and every member it omits is left as it was. Two orders
//! given the same members through the two different operations therefore report the same
//! values for every member of the set.
//!
//! The rows differ along what the rule generalises over: which members the request carries
//! (none, one, several, all of them), the enum value from the modelled catalog
//! (`SHIPPING_OPTION_VALUES`, reached through `cluster_updates::update_rows` rather than
//! restated), the length of a value at each published bound of `Description`, and the shape
//! of the structure members. The order each row is applied to holds node jobs from both
//! paths that mint them (`CreateCluster`'s `InitialClusterSize` and a `CreateJob` naming the
//! cluster), so each operation is driven over several records of one order.
//!
//! The harness is the one `job_lifecycle`, `cluster_updates` and `cluster_node_updates`
//! already drive the service with, so every call here goes through the same front door a
//! client uses and nothing reaches into the state struct.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::cluster_node_updates::{at, cluster_with_nodes, INHERITED_MEMBERS};
use super::cluster_updates::{cluster_metadata, job_metadata, update_body, update_rows};
use super::job_lifecycle::{create_address, deliverable_address, ok, router};

/// How many node jobs each order below holds, and by which path they were minted: the
/// create's `InitialClusterSize` and a `CreateJob` naming the cluster, so both ends of the
/// order are exercised over more than one record and over both ways a node arrives.
const INITIAL_NODES: usize = 2;
const ATTACHED_NODES: usize = 1;

/// What one order reports: its cluster's `ClusterMetadata` and each node job's
/// `JobMetadata`, in the order the nodes were created.
struct OrderReport {
    cluster: Value,
    nodes: Vec<Value>,
}

impl OrderReport {
    /// Every member the order holds one value of, as the two records report it: each member
    /// of `INHERITED_MEMBERS` plus the shipping speed, which a cluster reports directly and
    /// a job reports under `ShippingDetails`.
    ///
    /// Named `(record, member, value)` so a mismatch says which record of the order
    /// disagreed.
    fn order_members(&self) -> Vec<(String, String, Value)> {
        let mut members = Vec::new();
        for name in INHERITED_MEMBERS {
            members.push((
                "the cluster".to_string(),
                (*name).to_string(),
                at(&self.cluster, name),
            ));
        }
        members.push((
            "the cluster".to_string(),
            "ShippingOption".to_string(),
            at(&self.cluster, "ShippingOption"),
        ));
        for (index, node) in self.nodes.iter().enumerate() {
            for name in INHERITED_MEMBERS {
                members.push((
                    format!("node job {index}"),
                    (*name).to_string(),
                    at(node, name),
                ));
            }
            members.push((
                format!("node job {index}"),
                "ShippingOption".to_string(),
                at(&at(node, "ShippingDetails"), "ShippingOption"),
            ));
        }
        members
    }
}

/// Read back what the order whose cluster is `cluster_id` and whose node jobs are
/// `node_ids` reports.
async fn read_order(
    router: &MultiServiceRouter,
    cluster_id: &str,
    node_ids: &[String],
    case: &str,
) -> OrderReport {
    let cluster = cluster_metadata(router, cluster_id, case).await;
    let mut nodes = Vec::new();
    for job_id in node_ids {
        nodes.push(job_metadata(router, job_id, case).await);
    }
    OrderReport { cluster, nodes }
}

/// The body of an `UpdateJob` call: the update's members plus the job it names.
fn job_update_body(job_id: &str, update: &Value) -> Value {
    let mut body = update.clone();
    body["JobId"] = json!(job_id);
    body
}

#[tokio::test]
async fn an_order_takes_the_same_members_through_update_cluster_and_through_update_job() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Order Member Receiver", "20 Cluster Way", json!({})),
        "the address the orders are created with",
    )
    .await;
    let alternate_address_id = create_address(
        &router,
        deliverable_address(
            "Alternate Order Member Receiver",
            "21 Cluster Way",
            json!({ "City": "Portland", "StateOrProvince": "OR", "PostalCode": "97201" }),
        ),
        "the address an update moves the orders to",
    )
    .await;

    for (row, update) in update_rows(&address_id, &alternate_address_id) {
        let case = format!("the order updated with {row}");

        // Two orders of the same shape: one updated through its cluster, one through the
        // first of its node jobs, with the same members in both requests.
        let through_cluster = format!("{case}, through the cluster");
        let (cluster_a, nodes_a) = cluster_with_nodes(
            &router,
            &address_id,
            INITIAL_NODES,
            ATTACHED_NODES,
            &through_cluster,
        )
        .await;
        let through_node = format!("{case}, through a node job");
        let (cluster_b, nodes_b) = cluster_with_nodes(
            &router,
            &address_id,
            INITIAL_NODES,
            ATTACHED_NODES,
            &through_node,
        )
        .await;
        let created = read_order(&router, &cluster_a, &nodes_a, &through_cluster).await;

        ok(
            &router,
            "UpdateCluster",
            update_body(&cluster_a, &update),
            &through_cluster,
        )
        .await;
        ok(
            &router,
            "UpdateJob",
            job_update_body(&nodes_b[0], &update),
            &through_node,
        )
        .await;

        let from_cluster = read_order(&router, &cluster_a, &nodes_a, &through_cluster).await;
        let from_node = read_order(&router, &cluster_b, &nodes_b, &through_node).await;

        // The two operations applied the same set to the same records of their order.
        let applied = from_cluster.order_members();
        let applied_from_node = from_node.order_members();
        assert_eq!(
            applied.len(),
            applied_from_node.len(),
            "case {case:?}: expected both orders to hold the same records, got \
             {applied:?} against {applied_from_node:?}"
        );
        for ((record, member, by_cluster), (_, _, by_node)) in
            applied.iter().zip(applied_from_node.iter())
        {
            assert_eq!(
                by_cluster, by_node,
                "case {case:?}: expected {record} to report the same {member} however the \
                 order was updated, got {by_cluster} through UpdateCluster against \
                 {by_node} through UpdateJob"
            );
        }

        // And what they applied is what the request supplied: a member the row names reads
        // back as the row's value on every record of the order, and a member it omits is
        // what the create left there.
        let named = update
            .as_object()
            .expect("an update row is a JSON object")
            .clone();
        for (record, member, value) in &applied {
            match named.get(member) {
                Some(expected) => assert_eq!(
                    value, expected,
                    "case {case:?}: expected {record} to report the supplied {member} \
                     {expected}, got {value}"
                ),
                None => {
                    let (_, _, before) = created
                        .order_members()
                        .into_iter()
                        .find(|(r, m, _)| r == record && m == member)
                        .expect("every member of the order is reported before the update");
                    assert_eq!(
                        value, &before,
                        "case {case:?}: expected {record} to leave the omitted {member} as \
                         it was ({before}), got {value}"
                    );
                }
            }
        }
    }
}
