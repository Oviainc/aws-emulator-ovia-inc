// SPDX-License-Identifier: CC0-1.0

//! What cancelling a cluster's node job does to the cluster, driven end to end over HTTP.
//!
//! One rule, asserted over a TABLE of inputs rather than over a single request: a cluster's
//! five-node quorum is counted over the node jobs the cluster will SHIP with, so a node job
//! that has been cancelled does not count toward it — `CreateCluster` documents the quorum
//! as "Each cluster supports five nodes … The cluster does not ship until these five node
//! jobs have been created" and `ClusterLimitExceededException` documents the replacement as
//! "If you have fewer than five nodes for your cluster and you have more nodes to create
//! for this cluster, try again and create jobs until your cluster has exactly five nodes".
//! A cluster holding its quorum of live nodes reports `Pending` and takes no further node;
//! one short of it reports `AwaitingQuorum` and takes the nodes it is missing.
//!
//! The rows differ along what the rule generalises over: how many node jobs the cluster was
//! created with (one, one short of the quorum, the quorum, one past it, the largest initial
//! size the model admits) and how many of them are then cancelled (one, enough to drop the
//! cluster below the quorum, all of them) — so each row lands on a different live-node
//! count on either side of the quorum, and the state and the refusal expected of it are
//! computed from that count rather than restated per row.
//!
//! The harness (`router`, `ok`, `refused`, `create_address`, `deliverable_address`,
//! `string`, `complete_order`) and the cluster helpers (`base_cluster`, `create_cluster`,
//! `cluster_metadata`, `job_metadata`, `update_body`) are the ones `job_lifecycle`,
//! `cluster_updates` and `cluster_cancellation` already drive the service with, so every
//! call here goes through the same front door a client uses and nothing reaches into the
//! state struct.

use serde_json::{json, Value};
// The quorum this service holds a cluster to, read from the crate rather than restated, so
// a row cannot drift from the bound the handlers enforce.
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::state::CLUSTER_NODE_QUORUM;
use smithy_server_runtime::MultiServiceRouter;

use super::cluster_updates::{
    base_cluster, cluster_metadata, create_cluster, job_metadata, update_body,
};
use super::job_lifecycle::{
    complete_order, create_address, deliverable_address, ok, refused, router, string,
};

/// `CreateClusterRequest$InitialClusterSize` is modelled `range 0..16`
/// (`smithy_query(query="operation", operation="CreateCluster")`), so this is the largest
/// number of node jobs one create can ask for.
const INITIAL_CLUSTER_SIZE_MAX: usize = 16;

/// The `ClusterState` a cluster holding `live` live node jobs reports: a cluster whose node
/// jobs all exist is no longer awaiting them and waits to be prepared and shipped, and one
/// short of the quorum is awaiting the nodes it still needs.
fn expected_state(live: usize) -> &'static str {
    if live >= CLUSTER_NODE_QUORUM {
        "Pending"
    } else {
        "AwaitingQuorum"
    }
}

/// The `ClusterState` `DescribeCluster` reports for `cluster_id`.
async fn cluster_state(router: &MultiServiceRouter, cluster_id: &str, case: &str) -> String {
    let metadata = cluster_metadata(router, cluster_id, case).await;
    metadata
        .get("ClusterState")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a ClusterState, got {metadata}"))
        .to_string()
}

/// The `JobState` `DescribeJob` reports for `job_id`.
async fn job_state(router: &MultiServiceRouter, job_id: &str, case: &str) -> String {
    let metadata = job_metadata(router, job_id, case).await;
    metadata
        .get("JobState")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a JobState, got {metadata}"))
        .to_string()
}

/// The job ids `ListClusterJobs` reports for `cluster_id`, in the order it reports them.
async fn listed_node_ids(router: &MultiServiceRouter, cluster_id: &str, case: &str) -> Vec<String> {
    let listed = ok(
        router,
        "ListClusterJobs",
        json!({ "ClusterId": cluster_id }),
        case,
    )
    .await;
    listed
        .get("JobListEntries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| string(entry, "JobId", case))
        .collect()
}

/// Create one node job of `cluster_id` the way a replacement node is created — "you only
/// need to provide the clusterId value; the other job attributes are inherited from the
/// cluster" (`CreateJobRequest$ClusterId`) — and return the job id it was minted under.
async fn create_node(router: &MultiServiceRouter, cluster_id: &str, case: &str) -> String {
    let created = ok(
        router,
        "CreateJob",
        json!({ "ClusterId": cluster_id }),
        case,
    )
    .await;
    string(&created, "JobId", case)
}

/// Every row: the number of node jobs the cluster is created with, and how many of them are
/// then cancelled.
fn rows() -> Vec<(String, usize, usize)> {
    let plan: Vec<(usize, usize)> = vec![
        // A single node, cancelled: the cluster is back to holding none.
        (1, 1),
        // One short of the quorum, so the cancellation drops it further short — the case in
        // which a replacement must NOT be mistaken for the quorum being reached.
        (CLUSTER_NODE_QUORUM - 1, 1),
        // At the quorum, losing one node and losing every node.
        (CLUSTER_NODE_QUORUM, 1),
        (CLUSTER_NODE_QUORUM, 2),
        (CLUSTER_NODE_QUORUM, CLUSTER_NODE_QUORUM),
        // Past the quorum: a cancellation that leaves the quorum intact changes nothing
        // about the cluster's state, and one that cuts into it does.
        (CLUSTER_NODE_QUORUM + 1, 1),
        (INITIAL_CLUSTER_SIZE_MAX, 1),
        (
            INITIAL_CLUSTER_SIZE_MAX,
            INITIAL_CLUSTER_SIZE_MAX - CLUSTER_NODE_QUORUM + 1,
        ),
    ];
    plan.into_iter()
        .map(|(initial, cancelled)| {
            (
                format!("a cluster created with {initial} node jobs, {cancelled} cancelled"),
                initial,
                cancelled,
            )
        })
        .collect()
}

#[tokio::test]
async fn a_cancelled_node_job_stops_counting_toward_its_clusters_quorum() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Node Cancel Receiver", "13 Cluster Way", json!({})),
        "the address every cluster here was created with",
    )
    .await;

    for (case, initial, cancelled) in rows() {
        let mut body = base_cluster(&address_id);
        body["InitialClusterSize"] = json!(initial as i64);
        let (cluster_id, entries) = create_cluster(&router, body, &case).await;
        let node_ids: Vec<String> = entries
            .iter()
            .map(|entry| string(entry, "JobId", &case))
            .collect();
        assert_eq!(
            node_ids.len(),
            initial,
            "case {case:?}: expected one node job per node asked for, got {entries:?}"
        );
        let created_state = cluster_state(&router, &cluster_id, &case).await;
        assert_eq!(
            created_state,
            expected_state(initial),
            "case {case:?}: expected a cluster holding {initial} live node jobs to report \
             {}, got {created_state:?}",
            expected_state(initial)
        );

        // Each cancellation takes one node out of the quorum, so the cluster's state is
        // whatever the live-node count that is left calls for.
        for index in 0..cancelled {
            let node_case = format!("{case}, cancelling node {index}");
            ok(
                &router,
                "CancelJob",
                json!({ "JobId": node_ids[index] }),
                &node_case,
            )
            .await;

            let live = initial - index - 1;
            let state = cluster_state(&router, &cluster_id, &node_case).await;
            assert_eq!(
                state,
                expected_state(live),
                "case {node_case:?}: expected a cluster holding {live} live node jobs to \
                 report {}, got {state:?}",
                expected_state(live)
            );
            // Reading the cluster does not change what the next caller sees.
            let again = cluster_state(&router, &cluster_id, &node_case).await;
            assert_eq!(
                again, state,
                "case {node_case:?}: two DescribeCluster calls disagreed, {again:?} after \
                 {state:?}"
            );

            // The node that was cancelled is the only one that moved, and no node leaves
            // the cluster's listing: nothing in this API removes a job.
            for (other, node_id) in node_ids.iter().enumerate() {
                let node_state = job_state(&router, node_id, &node_case).await;
                let expected_node = if other <= index { "Cancelled" } else { "New" };
                assert_eq!(
                    node_state, expected_node,
                    "case {node_case:?}: expected node job {node_id} to read back as \
                     {expected_node}, got {node_state:?}"
                );
            }
            let listed = listed_node_ids(&router, &cluster_id, &node_case).await;
            assert_eq!(
                listed, node_ids,
                "case {node_case:?}: cancelling a node job changed the cluster's listing"
            );
        }

        // The nodes the cluster is now missing are the "more nodes to create for this
        // cluster" the limit error names, so each one is accepted until the quorum is whole
        // again — and the state follows the live count up exactly as it followed it down.
        let mut live = initial - cancelled;
        let mut expected_listing = node_ids.clone();
        while live < CLUSTER_NODE_QUORUM {
            let replacement_case = format!("{case}, replacement node {live}");
            let replacement = create_node(&router, &cluster_id, &replacement_case).await;
            live += 1;
            expected_listing.push(replacement.clone());
            let replacement_state = job_state(&router, &replacement, &replacement_case).await;
            assert_eq!(
                replacement_state, "New",
                "case {replacement_case:?}: expected a replacement node to start in New, got \
                 {replacement_state:?}"
            );
            let state = cluster_state(&router, &cluster_id, &replacement_case).await;
            assert_eq!(
                state,
                expected_state(live),
                "case {replacement_case:?}: expected a cluster holding {live} live node jobs \
                 to report {}, got {state:?}",
                expected_state(live)
            );
            let listed = listed_node_ids(&router, &cluster_id, &replacement_case).await;
            assert_eq!(
                listed, expected_listing,
                "case {replacement_case:?}: expected the replacement node to join the \
                 cluster's listing"
            );
        }

        // With the quorum whole again the cluster takes no further node, whichever mix of
        // cancelled and live nodes it holds.
        let full_case = format!("{case}, one node past the restored quorum");
        let before = cluster_metadata(&router, &cluster_id, &full_case).await;
        refused(
            &router,
            "CreateJob",
            json!({ "ClusterId": cluster_id }),
            "ClusterLimitExceededException",
            &full_case,
        )
        .await;
        let after = cluster_metadata(&router, &cluster_id, &full_case).await;
        assert_eq!(
            after, before,
            "case {full_case:?}: a refused CreateJob changed the cluster"
        );
        let listed = listed_node_ids(&router, &cluster_id, &full_case).await;
        assert_eq!(
            listed, expected_listing,
            "case {full_case:?}: a refused CreateJob changed the cluster's listing"
        );
    }
}

#[tokio::test]
async fn a_cluster_short_of_its_quorum_again_can_be_updated_and_cancelled() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Wedged Cluster Receiver", "14 Cluster Way", json!({})),
        "the cluster's address",
    )
    .await;

    // A cluster that reached its quorum is past the window both `UpdateCluster` ("While a
    // cluster's ClusterState value is in the AwaitingQuorum state, you can update some of
    // the information associated with a cluster") and `CancelCluster` ("You can only cancel
    // a cluster job while it's in the AwaitingQuorum status") name.
    let case = "a cluster at its quorum";
    let mut body = base_cluster(&address_id);
    body["InitialClusterSize"] = json!(CLUSTER_NODE_QUORUM as i64);
    let (cluster_id, entries) = create_cluster(&router, body, case).await;
    let node_ids: Vec<String> = entries
        .iter()
        .map(|entry| string(entry, "JobId", case))
        .collect();
    for operation in ["UpdateCluster", "CancelCluster"] {
        let body = if operation == "UpdateCluster" {
            update_body(&cluster_id, &json!({ "Description": "too early" }))
        } else {
            json!({ "ClusterId": cluster_id })
        };
        refused(
            &router,
            operation,
            body,
            "InvalidJobStateException",
            &format!("{case}, {operation}"),
        )
        .await;
    }

    // Cancelling one of its nodes puts the cluster back to awaiting the node it lost, which
    // is the state both windows are open in.
    let short_case = "the same cluster, one node cancelled";
    ok(
        &router,
        "CancelJob",
        json!({ "JobId": node_ids[0] }),
        short_case,
    )
    .await;
    let state = cluster_state(&router, &cluster_id, short_case).await;
    assert_eq!(
        state, "AwaitingQuorum",
        "case {short_case:?}: expected a cluster short of its quorum to await it, got {state:?}"
    );

    let description = "the plan after losing a node";
    ok(
        &router,
        "UpdateCluster",
        update_body(&cluster_id, &json!({ "Description": description })),
        short_case,
    )
    .await;
    let updated = cluster_metadata(&router, &cluster_id, short_case).await;
    assert_eq!(
        updated.get("Description").and_then(Value::as_str),
        Some(description),
        "case {short_case:?}: expected the update to land on the cluster, got {updated}"
    );

    ok(
        &router,
        "CancelCluster",
        json!({ "ClusterId": cluster_id }),
        short_case,
    )
    .await;
    let cancelled = cluster_state(&router, &cluster_id, short_case).await;
    assert_eq!(
        cancelled, "Cancelled",
        "case {short_case:?}: expected the cluster to cancel, got {cancelled:?}"
    );
    for node_id in &node_ids {
        let node_state = job_state(&router, node_id, short_case).await;
        assert_eq!(
            node_state, "Cancelled",
            "case {short_case:?}: expected the node job {node_id} to be cancelled with its \
             cluster, got {node_state:?}"
        );
    }

    // A cancelled cluster is in a state the quorum does not govern, so nothing about its
    // nodes moves it: a second cancellation of a node is refused and the cluster stays
    // cancelled.
    refused(
        &router,
        "CancelJob",
        json!({ "JobId": node_ids[1] }),
        "InvalidJobStateException",
        "a node of a cancelled cluster, cancelled again",
    )
    .await;
    let still = cluster_state(&router, &cluster_id, short_case).await;
    assert_eq!(
        still, "Cancelled",
        "case {short_case:?}: expected a cancelled cluster to stay cancelled, got {still:?}"
    );
}

#[tokio::test]
async fn cancelling_a_job_that_is_no_clusters_node_leaves_every_cluster_alone() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Standalone Cancel Receiver", "15 Cluster Way", json!({})),
        "the address every order here was created with",
    )
    .await;

    // Two clusters: one holding its quorum, one short of it, so a cancellation that reached
    // either would be visible in its state.
    let full_case = "the cluster at its quorum";
    let mut full_body = base_cluster(&address_id);
    full_body["InitialClusterSize"] = json!(CLUSTER_NODE_QUORUM as i64);
    let (full_id, full_entries) = create_cluster(&router, full_body, full_case).await;

    let short_case = "the cluster short of its quorum";
    let mut short_body = base_cluster(&address_id);
    short_body["InitialClusterSize"] = json!(CLUSTER_NODE_QUORUM as i64 - 1);
    let (short_id, _) = create_cluster(&router, short_body, short_case).await;

    // Each row is an order belonging to no cluster: a plain job, and an export order, whose
    // job parts are cancelled with their main job.
    let orders: Vec<(&str, Value)> = vec![
        (
            "a job belonging to no cluster",
            complete_order(&json!({ "AddressId": address_id })),
        ),
        (
            "an export order belonging to no cluster",
            complete_order(&json!({ "JobType": "EXPORT", "AddressId": address_id })),
        ),
    ];
    for (case, body) in orders {
        let created = ok(&router, "CreateJob", body, case).await;
        let job_id = string(&created, "JobId", case);
        ok(&router, "CancelJob", json!({ "JobId": job_id }), case).await;
        let job = job_state(&router, &job_id, case).await;
        assert_eq!(
            job, "Cancelled",
            "case {case:?}: expected the cancelled job to read back as Cancelled, got {job:?}"
        );

        for (cluster_case, cluster_id, live) in [
            (full_case, &full_id, CLUSTER_NODE_QUORUM),
            (short_case, &short_id, CLUSTER_NODE_QUORUM - 1),
        ] {
            let state = cluster_state(&router, cluster_id, cluster_case).await;
            assert_eq!(
                state,
                expected_state(live),
                "case {case:?}: cancelling it moved {cluster_case}, which reads back as \
                 {state:?}"
            );
        }
        for entry in &full_entries {
            let node_id = string(entry, "JobId", case);
            let node_state = job_state(&router, &node_id, case).await;
            assert_eq!(
                node_state, "New",
                "case {case:?}: cancelling it moved the node job {node_id}, which reads back \
                 as {node_state:?}"
            );
        }
    }
}
