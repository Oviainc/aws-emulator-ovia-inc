// SPDX-License-Identifier: CC0-1.0

//! The window in which a cluster still takes node jobs, driven end to end over HTTP.
//!
//! One rule, asserted over a TABLE of inputs rather than over a single request: `CreateJob`
//! naming a `ClusterId` adds a node to that cluster only while node creation is still what
//! the cluster is waiting on — "The cluster does not ship until these five node jobs have
//! been created" (`CreateCluster`) — so a cluster that has left that window, which is where
//! `CancelCluster` leaves the cluster it terminates ("You can only cancel a cluster job
//! while it's in the `AwaitingQuorum` status"), takes no further node: the create is refused
//! with `InvalidInputCombinationException` and the cluster's node set does not grow. A
//! cluster still inside the window that already holds its five nodes is refused on the
//! five-node ceiling instead (`ClusterLimitExceededException`), which is the error that
//! names a count.
//!
//! The rows differ along what the rule generalises over: how many node jobs the cluster held
//! when it left the window (none, one, two short of the quorum, one short of it), which
//! optional members the cluster carries, every value the modelled `ShippingOption` catalog
//! admits (`SHIPPING_OPTION_VALUES`, read from the crate rather than restated), the length of
//! `Description` at each published bound, and — for each such cluster — which members the
//! refused `CreateJob` itself carries, from the bare `ClusterId` to a node request naming
//! every member a node can state for itself.
//!
//! The harness (`router`, `ok`, `refused`, `create_address`, `deliverable_address`,
//! `complete_order`, `string`) and the cluster helpers (`base_cluster`, `create_cluster`,
//! `cluster_metadata`, `job_metadata`) are the ones the other cluster suites already drive
//! the service with, so every call here goes through the same front door a client uses and
//! nothing reaches into the state struct.

use serde_json::{json, Value};
// The one job and device type a cluster is ordered in, the modelled shipping catalog, the
// modelled cluster-state catalog, the lifecycle enum itself and the quorum this service
// holds a cluster to, all read from the crate rather than restated, so a row cannot drift
// from the values the service accepts or from the bound the handler enforces.
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::create_cluster::{
    CLUSTER_JOB_TYPE, CLUSTER_SNOWBALL_TYPE,
};
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::types::{
    CLUSTER_STATE_VALUES, SHIPPING_OPTION_VALUES,
};
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::state::{
    ClusterState, CLUSTER_NODE_QUORUM,
};
use smithy_server_runtime::MultiServiceRouter;

use super::cluster_updates::{base_cluster, cluster_metadata, create_cluster, job_metadata};
use super::job_lifecycle::{
    complete_order, create_address, deliverable_address, ok, refused, router, string,
};

/// `CreateClusterRequest$Description` and `CreateJobRequest$Description` are both modelled
/// `length 1..1024` (`smithy_query(query="operation", operation="CreateCluster")` and
/// `smithy_query(query="operation", operation="CreateJob")`), so these are the shortest and
/// longest values either member admits.
const DESCRIPTION_MIN_LEN: usize = 1;
const DESCRIPTION_MAX_LEN: usize = 1024;

/// Every state the cluster lifecycle enum spells, so the set below is checked against the
/// model's catalog rather than against itself.
const ALL_CLUSTER_STATES: &[ClusterState] = &[
    ClusterState::AwaitingQuorum,
    ClusterState::Pending,
    ClusterState::InUse,
    ClusterState::Complete,
    ClusterState::Cancelled,
];

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

/// Every job this store holds, as `ListJobs` reports them: the whole job store, so a job
/// minted by a request that should have been refused shows up here even when it joined no
/// cluster.
async fn listed_job_ids(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let listed = ok(router, "ListJobs", json!({}), case).await;
    listed
        .get("JobListEntries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| string(entry, "JobId", case))
        .collect()
}

/// Everything a refused `CreateJob` must leave exactly as it was: the cluster it named, the
/// node jobs that cluster holds and every job this store holds.
async fn observable_state(
    router: &MultiServiceRouter,
    cluster_id: &str,
    case: &str,
) -> (Value, Vec<String>, Vec<String>) {
    (
        cluster_metadata(router, cluster_id, case).await,
        listed_node_ids(router, cluster_id, case).await,
        listed_job_ids(router, case).await,
    )
}

/// The clusters a row is built from: each one still short of its quorum when it is created,
/// differing along the number of node jobs it holds, which optional members it carries, the
/// shipping speed it takes from the modelled catalog and the length of its description.
fn cluster_rows(address_id: &str) -> Vec<(String, Value)> {
    let mut rows: Vec<(String, Value)> = vec![
        (
            "every member a cluster carries, no node jobs".to_string(),
            base_cluster(address_id),
        ),
        (
            "the required members alone".to_string(),
            json!({
                "JobType": CLUSTER_JOB_TYPE,
                "SnowballType": CLUSTER_SNOWBALL_TYPE,
                "ShippingOption": SHIPPING_OPTION_VALUES[0],
                "AddressId": address_id,
            }),
        ),
    ];

    // The node-job count the rule generalises over: none at all, one, two short of the
    // quorum and one short of it — every count from which a cluster is still awaiting its
    // quorum when it is cancelled.
    for size in [1, CLUSTER_NODE_QUORUM as i64 - 2, CLUSTER_NODE_QUORUM as i64 - 1] {
        let mut body = base_cluster(address_id);
        body["InitialClusterSize"] = json!(size);
        rows.push((format!("a cluster holding {size} node jobs"), body));
    }

    // One row per entry of the modelled shipping catalog, so the rule is exercised on
    // values the issue did not cite.
    for speed in SHIPPING_OPTION_VALUES {
        let mut body = base_cluster(address_id);
        body["ShippingOption"] = json!(speed);
        body["InitialClusterSize"] = json!(1);
        rows.push((format!("a cluster shipped {speed} with one node job"), body));
    }

    // Each published bound of `Description`.
    for len in [DESCRIPTION_MIN_LEN, DESCRIPTION_MAX_LEN] {
        let mut body = base_cluster(address_id);
        body["Description"] = json!("d".repeat(len));
        rows.push((format!("a description of {len} characters"), body));
    }

    rows
}

/// Every `CreateJob` body a node of `cluster_id` can be asked for with: the bare cluster id
/// a node needs ("you only need to provide the clusterId value"), the cluster's own job and
/// device type stated back, each member a node states for itself, and a request naming all
/// of them at once. None of them disagrees with the cluster, so what the refusal turns on is
/// the cluster's state and nothing else.
fn node_request_rows(cluster_id: &str, address_id: &str) -> Vec<(String, Value)> {
    let mut rows: Vec<(String, Value)> = vec![
        (
            "the cluster id alone".to_string(),
            json!({ "ClusterId": cluster_id }),
        ),
        (
            "the cluster id with the cluster's own job and device type".to_string(),
            json!({
                "ClusterId": cluster_id,
                "JobType": CLUSTER_JOB_TYPE,
                "SnowballType": CLUSTER_SNOWBALL_TYPE,
            }),
        ),
        (
            "a node naming its own description, address and shipping speed".to_string(),
            json!({
                "ClusterId": cluster_id,
                "Description": "d".repeat(DESCRIPTION_MIN_LEN),
                "AddressId": address_id,
                "ShippingOption": SHIPPING_OPTION_VALUES[0],
            }),
        ),
        (
            "a node naming every member it can state for itself".to_string(),
            json!({
                "ClusterId": cluster_id,
                "JobType": CLUSTER_JOB_TYPE,
                "SnowballType": CLUSTER_SNOWBALL_TYPE,
                "ShippingOption": SHIPPING_OPTION_VALUES[0],
                "AddressId": address_id,
                "ForwardingAddressId": address_id,
                "Description": "d".repeat(DESCRIPTION_MAX_LEN),
                "RoleARN": "arn:aws:iam::123456789012:role/snowball-late-node",
                "KmsKeyARN": "arn:aws:kms:us-west-2:123456789012:key/late-node-key",
                "Notification": {
                    "SnsTopicARN": "arn:aws:sns:us-west-2:123456789012:late-node",
                    "NotifyAll": true
                },
                "RemoteManagement": "INSTALLED_ONLY",
            }),
        ),
    ];

    // One row per shipping speed the model admits, so the refusal is asserted across that
    // catalog on the request side too.
    for speed in SHIPPING_OPTION_VALUES {
        rows.push((
            format!("a node shipped {speed}"),
            json!({ "ClusterId": cluster_id, "ShippingOption": speed }),
        ));
    }

    rows
}

#[tokio::test]
async fn create_job_is_refused_for_a_cluster_that_has_left_its_node_creation_window() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Terminated Cluster Receiver", "16 Cluster Way", json!({})),
        "the address every cluster here was created with",
    )
    .await;

    for (case, body) in cluster_rows(&address_id) {
        let (cluster_id, entries) = create_cluster(&router, body, &case).await;
        let mut expected_nodes: Vec<String> = entries
            .iter()
            .map(|entry| string(entry, "JobId", &case))
            .collect();

        // While the cluster is still awaiting its quorum the very same create is ACCEPTED:
        // the refusal below is about the window, not about the request. A cluster one node
        // short of its quorum would leave that window by reaching it, so the control is
        // taken only where it keeps the cluster short.
        if expected_nodes.len() + 1 < CLUSTER_NODE_QUORUM {
            let control_case = format!("{case}, a node created while awaiting quorum");
            let created = ok(
                &router,
                "CreateJob",
                json!({ "ClusterId": cluster_id }),
                &control_case,
            )
            .await;
            let node_id = string(&created, "JobId", &control_case);
            let state = job_state(&router, &node_id, &control_case).await;
            assert_eq!(
                state, "New",
                "case {control_case:?}: expected the accepted node {node_id} to start in New, \
                 got {state:?}"
            );
            expected_nodes.push(node_id);
            let listed = listed_node_ids(&router, &cluster_id, &control_case).await;
            assert_eq!(
                listed, expected_nodes,
                "case {control_case:?}: expected the accepted node to join the cluster's \
                 listing"
            );
        }

        // Cancelling the cluster terminates the order, which takes it out of the window.
        ok(
            &router,
            "CancelCluster",
            json!({ "ClusterId": cluster_id }),
            &case,
        )
        .await;
        let cancelled = cluster_state(&router, &cluster_id, &case).await;
        assert_eq!(
            cancelled, "Cancelled",
            "case {case:?}: expected the cancelled cluster to read back as Cancelled, got \
             {cancelled:?}"
        );

        let before = observable_state(&router, &cluster_id, &case).await;
        for (request_case, request) in node_request_rows(&cluster_id, &address_id) {
            let refusal_case = format!("{case}, {request_case}");
            refused(
                &router,
                "CreateJob",
                request,
                "InvalidInputCombinationException",
                &refusal_case,
            )
            .await;

            let after = observable_state(&router, &cluster_id, &refusal_case).await;
            assert_eq!(
                after.0, before.0,
                "case {refusal_case:?}: a refused CreateJob changed the cluster"
            );
            assert_eq!(
                after.1, before.1,
                "case {refusal_case:?}: a refused CreateJob changed the cluster's node \
                 listing, which reads back as {:?}",
                after.1
            );
            assert_eq!(
                after.2, before.2,
                "case {refusal_case:?}: a refused CreateJob minted a job, so the job store \
                 reads back as {:?}",
                after.2
            );
        }

        // Every node the cluster did hold is still a node of it, cancelled with it: the
        // refusals neither added nor removed one.
        for node_id in &expected_nodes {
            let state = job_state(&router, node_id, &case).await;
            assert_eq!(
                state, "Cancelled",
                "case {case:?}: expected the node job {node_id} to be cancelled with its \
                 cluster, got {state:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_cluster_holding_its_quorum_is_refused_on_the_five_node_ceiling() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Quorum Node Receiver", "17 Cluster Way", json!({})),
        "the clusters' address",
    )
    .await;

    // A cluster that holds its five nodes has not left the window — node creation is still
    // what its lifecycle is about — so the refusal is the one that names the count, whether
    // the quorum was reached by the create itself or by a later `CreateJob`.
    let rows: Vec<(String, Option<i64>)> = vec![
        (
            "a cluster created with its whole quorum of nodes".to_string(),
            Some(CLUSTER_NODE_QUORUM as i64),
        ),
        (
            "a cluster whose nodes were created one by one up to the quorum".to_string(),
            None,
        ),
    ];
    for (case, initial_size) in rows {
        let mut body = base_cluster(&address_id);
        if let Some(size) = initial_size {
            body["InitialClusterSize"] = json!(size);
        }
        let (cluster_id, entries) = create_cluster(&router, body, &case).await;
        let mut expected_nodes: Vec<String> = entries
            .iter()
            .map(|entry| string(entry, "JobId", &case))
            .collect();
        while expected_nodes.len() < CLUSTER_NODE_QUORUM {
            let node_case = format!("{case}, node {}", expected_nodes.len());
            let created = ok(
                &router,
                "CreateJob",
                json!({ "ClusterId": cluster_id }),
                &node_case,
            )
            .await;
            expected_nodes.push(string(&created, "JobId", &node_case));
        }
        let state = cluster_state(&router, &cluster_id, &case).await;
        assert_eq!(
            state, "Pending",
            "case {case:?}: expected a cluster holding its quorum to report Pending, got \
             {state:?}"
        );

        let before = observable_state(&router, &cluster_id, &case).await;
        refused(
            &router,
            "CreateJob",
            json!({ "ClusterId": cluster_id }),
            "ClusterLimitExceededException",
            &case,
        )
        .await;
        let after = observable_state(&router, &cluster_id, &case).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused CreateJob changed what the service reports"
        );
    }
}

#[tokio::test]
async fn a_terminated_cluster_refuses_only_its_own_nodes() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Window Isolation Receiver", "18 Cluster Way", json!({})),
        "the address every order here was created with",
    )
    .await;

    let cancelled_case = "the cluster that gets cancelled";
    let mut cancelled_body = base_cluster(&address_id);
    cancelled_body["InitialClusterSize"] = json!(2);
    let (cancelled_id, _) = create_cluster(&router, cancelled_body, cancelled_case).await;
    ok(
        &router,
        "CancelCluster",
        json!({ "ClusterId": cancelled_id }),
        cancelled_case,
    )
    .await;
    refused(
        &router,
        "CreateJob",
        json!({ "ClusterId": cancelled_id }),
        "InvalidInputCombinationException",
        cancelled_case,
    )
    .await;

    // A cluster still inside the window takes the node the cancelled one refused, and a job
    // that joins no cluster is not governed by any cluster's state at all: the refusal is
    // about the cluster the request named.
    let live_case = "a cluster still awaiting its quorum";
    let (live_id, live_entries) = create_cluster(&router, base_cluster(&address_id), live_case).await;
    let created = ok(
        &router,
        "CreateJob",
        json!({ "ClusterId": live_id }),
        live_case,
    )
    .await;
    let node_id = string(&created, "JobId", live_case);
    let node_state = job_state(&router, &node_id, live_case).await;
    assert_eq!(
        node_state, "New",
        "case {live_case:?}: expected the accepted node to start in New, got {node_state:?}"
    );
    let listed = listed_node_ids(&router, &live_id, live_case).await;
    let mut expected_listing: Vec<String> = live_entries
        .iter()
        .map(|entry| string(entry, "JobId", live_case))
        .collect();
    expected_listing.push(node_id);
    assert_eq!(
        listed, expected_listing,
        "case {live_case:?}: expected the accepted node to join the cluster's listing"
    );

    let standalone_case = "a job belonging to no cluster";
    let created = ok(
        &router,
        "CreateJob",
        complete_order(&json!({ "AddressId": address_id })),
        standalone_case,
    )
    .await;
    let standalone_id = string(&created, "JobId", standalone_case);
    let standalone_state = job_state(&router, &standalone_id, standalone_case).await;
    assert_eq!(
        standalone_state, "New",
        "case {standalone_case:?}: expected a job joining no cluster to start in New, got \
         {standalone_state:?}"
    );
}

#[test]
fn only_a_cluster_the_quorum_still_governs_takes_a_node_job() {
    let accepting: Vec<&str> = ALL_CLUSTER_STATES
        .iter()
        .filter(|state| state.accepts_node_jobs())
        .map(|state| state.as_wire())
        .collect();
    assert_eq!(
        accepting,
        vec!["AwaitingQuorum", "Pending"],
        "a cluster takes node jobs only while node creation is what it is waiting on — the \
         states the five-node quorum governs — got {accepting:?}"
    );
    // The states this emulator spells are the states the model spells, so the set above is
    // a statement about the whole modelled catalog.
    let spelled: Vec<&str> = ALL_CLUSTER_STATES.iter().map(|s| s.as_wire()).collect();
    for state in CLUSTER_STATE_VALUES {
        assert!(
            spelled.contains(state),
            "the modelled ClusterState value {state} is not spelled by this emulator: \
             {spelled:?}"
        );
    }
}
