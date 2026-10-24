// SPDX-License-Identifier: CC0-1.0

//! `CancelCluster`, driven end to end over HTTP.
//!
//! One rule, asserted over a TABLE of inputs rather than over a single request: while a
//! cluster's `ClusterState` is `AwaitingQuorum`, `CancelCluster` moves that cluster — and
//! every node job it holds that is itself still cancellable — to `Cancelled`, which a
//! later `DescribeCluster` and `DescribeJob` report; a request naming a cluster in any
//! other state is refused with `InvalidJobStateException` and one naming a cluster no
//! record answers to with `InvalidResourceException`, in both cases changing nothing.
//!
//! The rows differ along what the rule generalises over: how many node jobs the cluster
//! holds (none, the smallest initial size the model admits, one, one short of the quorum),
//! which optional members the cluster carries (none beyond the required four, one, all of
//! them), the enum values from the modelled catalogs (`JOB_TYPE_VALUES`,
//! `SNOWBALL_TYPE_VALUES`, `SHIPPING_OPTION_VALUES`, read from the crate rather than
//! restated), and the length of `Description` at each published bound.
//!
//! The harness (`router`, `ok`, `refused`, `create_address`, `deliverable_address`,
//! `string`) and the cluster helpers (`base_cluster`, `create_cluster`,
//! `cluster_metadata`, `job_metadata`) are the ones `job_lifecycle` and `cluster_updates`
//! already drive the service with, so every call here goes through the same front door a
//! client uses and nothing reaches into the state struct.

use serde_json::{json, Value};
// The catalogs the modelled enums admit and the quorum this service holds a cluster to,
// read from the crate rather than restated, so a row cannot drift from the values the
// model allows or from the bound the handler enforces.
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::types::{
    JOB_TYPE_VALUES, SHIPPING_OPTION_VALUES, SNOWBALL_TYPE_VALUES,
};
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::state::CLUSTER_NODE_QUORUM;
use smithy_server_runtime::MultiServiceRouter;

use super::cluster_updates::{base_cluster, cluster_metadata, create_cluster, job_metadata};
use super::job_lifecycle::{create_address, deliverable_address, ok, refused, router, string};

/// `CreateClusterRequest$Description` is modelled `length 1..1024`
/// (`smithy_query(query="operation", operation="CreateCluster")`), so these are the
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

/// The four members `CreateCluster` makes required, and nothing else: the smallest cluster
/// a row can be built from (`smithy_query(query="operation", operation="CreateCluster")`).
fn required_members_only(
    address_id: &str,
    job_type: &str,
    snowball_type: &str,
    shipping: &str,
) -> Value {
    json!({
        "JobType": job_type,
        "SnowballType": snowball_type,
        "ShippingOption": shipping,
        "AddressId": address_id,
    })
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

/// The `ClusterState` `DescribeCluster` reports for `cluster_id`.
async fn cluster_state(router: &MultiServiceRouter, cluster_id: &str, case: &str) -> String {
    let metadata = cluster_metadata(router, cluster_id, case).await;
    metadata
        .get("ClusterState")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a ClusterState, got {metadata}"))
        .to_string()
}

/// Every cluster a cancellation row creates: each one still awaiting its quorum, differing
/// along the number of node jobs it holds, which optional members it carries, the enum
/// value it takes from the modelled catalog and the length of its description.
fn cancellable_rows(address_id: &str) -> Vec<(String, Value)> {
    // One row per shipping speed, device type and job type the model admits, so the rows
    // span the catalogs rather than one recorded value.
    let shipping = SHIPPING_OPTION_VALUES;
    let devices = SNOWBALL_TYPE_VALUES;
    let job_types = JOB_TYPE_VALUES;

    let mut rows: Vec<(String, Value)> = vec![
        (
            "every member a cluster carries, no node jobs".to_string(),
            base_cluster(address_id),
        ),
        (
            "the required members alone".to_string(),
            required_members_only(address_id, job_types[0], devices[0], shipping[0]),
        ),
    ];

    // The node-job count the rule generalises over: none at all, the smallest size the
    // modelled range admits, one, and one short of the quorum — every count from which a
    // cluster is still awaiting its quorum.
    let sizes: Vec<i64> = vec![
        INITIAL_CLUSTER_SIZE_MIN,
        1,
        CLUSTER_NODE_QUORUM as i64 - 2,
        CLUSTER_NODE_QUORUM as i64 - 1,
    ];
    for size in sizes {
        let mut body = base_cluster(address_id);
        body["InitialClusterSize"] = json!(size);
        rows.push((format!("a cluster holding {size} node jobs"), body));
    }

    // One row per entry of each modelled catalog, on a cluster that also holds a node job,
    // so the rule is exercised on values the issue did not cite.
    for (index, device) in devices.iter().enumerate() {
        let mut body = required_members_only(
            address_id,
            job_types[index % job_types.len()],
            device,
            shipping[index % shipping.len()],
        );
        body["InitialClusterSize"] = json!(1);
        rows.push((format!("a {device} cluster with one node job"), body));
    }

    // Each published bound of `Description`.
    for len in [DESCRIPTION_MIN_LEN, DESCRIPTION_MAX_LEN] {
        let mut body = base_cluster(address_id);
        body["Description"] = json!("d".repeat(len));
        rows.push((format!("a description of {len} characters"), body));
    }

    rows
}

#[tokio::test]
async fn cancel_cluster_moves_a_cluster_awaiting_quorum_and_its_nodes_to_cancelled() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Cancelling Receiver", "9 Cluster Way", json!({})),
        "the address every cancelled cluster was created with",
    )
    .await;

    for (case, body) in cancellable_rows(&address_id) {
        let (cluster_id, entries) = create_cluster(&router, body, &case).await;
        let before = cluster_state(&router, &cluster_id, &case).await;
        assert_eq!(
            before, "AwaitingQuorum",
            "case {case:?}: expected a freshly created cluster to await its quorum, got \
             {before:?}"
        );
        let node_ids: Vec<String> = entries
            .iter()
            .map(|entry| string(entry, "JobId", &case))
            .collect();
        for node_id in &node_ids {
            let state = job_state(&router, node_id, &case).await;
            assert_eq!(
                state, "New",
                "case {case:?}: expected the node job {node_id} to start in New, got {state:?}"
            );
        }

        ok(
            &router,
            "CancelCluster",
            json!({ "ClusterId": cluster_id }),
            &case,
        )
        .await;

        let after = cluster_state(&router, &cluster_id, &case).await;
        assert_eq!(
            after, "Cancelled",
            "case {case:?}: expected the cancelled cluster to read back as Cancelled, got \
             {after:?}"
        );
        // Everything else about the cluster is left as it was: cancelling moves the state
        // and nothing else.
        let metadata = cluster_metadata(&router, &cluster_id, &case).await;
        assert_eq!(
            metadata.get("ClusterId").and_then(Value::as_str),
            Some(cluster_id.as_str()),
            "case {case:?}: expected the cancelled cluster to keep its id, got {metadata}"
        );
        // Reading does not change what the next caller sees.
        let again = cluster_metadata(&router, &cluster_id, &case).await;
        assert_eq!(
            again, metadata,
            "case {case:?}: two DescribeCluster calls disagreed after the cancellation"
        );

        // Every node job the cluster holds moved with it.
        for node_id in &node_ids {
            let state = job_state(&router, node_id, &case).await;
            assert_eq!(
                state, "Cancelled",
                "case {case:?}: expected the node job {node_id} to be cancelled with its \
                 cluster, got {state:?}"
            );
        }

        // A cluster that was already cancelled is no longer in `AwaitingQuorum`, so a
        // second cancellation is refused and changes nothing.
        refused(
            &router,
            "CancelCluster",
            json!({ "ClusterId": cluster_id }),
            "InvalidJobStateException",
            &format!("{case}, cancelled a second time"),
        )
        .await;
        let twice = cluster_metadata(&router, &cluster_id, &case).await;
        assert_eq!(
            twice, metadata,
            "case {case:?}: a refused second CancelCluster changed the cluster"
        );
    }
}

#[tokio::test]
async fn cancel_cluster_is_refused_once_the_cluster_has_left_awaiting_quorum() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Quorum Cancel Receiver", "10 Cluster Way", json!({})),
        "the clusters' address",
    )
    .await;

    // A cluster that holds its whole quorum of node jobs has nothing left to await, so it
    // is past the window the cancellation rule names — whether the quorum was reached by
    // the create itself or by a later `CreateJob` naming the cluster.
    let rows: Vec<(String, i64, usize)> = vec![
        (
            "a cluster created with its whole quorum of nodes".to_string(),
            CLUSTER_NODE_QUORUM as i64,
            CLUSTER_NODE_QUORUM,
        ),
        (
            "a cluster created with the largest initial size the model admits".to_string(),
            INITIAL_CLUSTER_SIZE_MAX,
            INITIAL_CLUSTER_SIZE_MAX as usize,
        ),
    ];
    for (case, initial_size, expected_nodes) in rows {
        let mut body = base_cluster(&address_id);
        body["InitialClusterSize"] = json!(initial_size);
        let (cluster_id, entries) = create_cluster(&router, body, &case).await;
        assert_eq!(
            entries.len(),
            expected_nodes,
            "case {case:?}: expected one job per node asked for, got {entries:?}"
        );
        let before = cluster_metadata(&router, &cluster_id, &case).await;
        assert_eq!(
            before.get("ClusterState").and_then(Value::as_str),
            Some("Pending"),
            "case {case:?}: expected the cluster to have left AwaitingQuorum, got {before}"
        );

        refused(
            &router,
            "CancelCluster",
            json!({ "ClusterId": cluster_id }),
            "InvalidJobStateException",
            &case,
        )
        .await;

        let after = cluster_metadata(&router, &cluster_id, &case).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused CancelCluster changed the cluster"
        );
        for entry in &entries {
            let node_id = string(entry, "JobId", &case);
            let state = job_state(&router, &node_id, &case).await;
            assert_eq!(
                state, "New",
                "case {case:?}: a refused CancelCluster cancelled the node job {node_id}, \
                 which reads back as {state:?}"
            );
        }
    }

    // The same cluster, reached the quorum one node at a time through `CreateJob`.
    let case = "a cluster whose nodes were created one by one up to the quorum";
    let (cluster_id, entries) = create_cluster(&router, base_cluster(&address_id), case).await;
    assert!(
        entries.is_empty(),
        "case {case:?}: expected an empty cluster, got {entries:?}"
    );
    for node in 0..CLUSTER_NODE_QUORUM {
        let created = ok(
            &router,
            "CreateJob",
            json!({ "ClusterId": cluster_id }),
            &format!("{case}: node {node}"),
        )
        .await;
        assert!(
            created.get("JobId").and_then(Value::as_str).is_some(),
            "case {case:?}: expected node {node} to be created, got {created}"
        );
    }
    let at_quorum = cluster_metadata(&router, &cluster_id, case).await;
    assert_eq!(
        at_quorum.get("ClusterState").and_then(Value::as_str),
        Some("Pending"),
        "case {case:?}: expected the cluster to have left AwaitingQuorum, got {at_quorum}"
    );
    refused(
        &router,
        "CancelCluster",
        json!({ "ClusterId": cluster_id }),
        "InvalidJobStateException",
        case,
    )
    .await;
    let after = cluster_metadata(&router, &cluster_id, case).await;
    assert_eq!(
        after, at_quorum,
        "case {case:?}: a refused CancelCluster changed the cluster"
    );
}

#[tokio::test]
async fn cancel_cluster_refuses_a_cluster_id_no_record_answers_to() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Unknown Cancel Receiver", "11 Cluster Way", json!({})),
        "a known address",
    )
    .await;
    let setup = "the cluster the refusals must not touch";
    let (cluster_id, _) = create_cluster(&router, base_cluster(&address_id), setup).await;
    let before = cluster_metadata(&router, &cluster_id, setup).await;

    // Each row is a well-formed `ClusterId` that names no stored cluster: the id the store
    // never minted, and the ids of the two other resource kinds this API mints, which are
    // not cluster ids at all.
    let mut flipped = cluster_id.clone();
    let last = flipped.pop().expect("a non-empty cluster id");
    flipped.push(if last == '0' { '1' } else { '0' });
    let unknown_ids: Vec<(&str, String)> = vec![
        (
            "a cluster id that was never created",
            UNKNOWN_CLUSTER_ID.to_string(),
        ),
        (
            "a cluster id differing from a stored one in its last digit",
            flipped,
        ),
    ];
    for (case, unknown_id) in unknown_ids {
        refused(
            &router,
            "CancelCluster",
            json!({ "ClusterId": unknown_id }),
            "InvalidResourceException",
            case,
        )
        .await;
        let after = cluster_metadata(&router, &cluster_id, case).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused CancelCluster changed the stored cluster"
        );
    }
}

#[tokio::test]
async fn cancelling_one_cluster_leaves_every_other_cluster_and_job_alone() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Isolation Receiver", "12 Cluster Way", json!({})),
        "the address every cluster here was created with",
    )
    .await;

    // Two clusters, each holding node jobs, and a job belonging to no cluster at all.
    let mut first_body = base_cluster(&address_id);
    first_body["InitialClusterSize"] = json!(2);
    let first_case = "the cluster that gets cancelled";
    let (first_id, first_nodes) = create_cluster(&router, first_body, first_case).await;

    let mut second_body = base_cluster(&address_id);
    second_body["InitialClusterSize"] = json!(3);
    let second_case = "the cluster that must be left alone";
    let (second_id, second_nodes) = create_cluster(&router, second_body, second_case).await;

    let standalone_case = "a job belonging to no cluster";
    let created = ok(
        &router,
        "CreateJob",
        json!({
            "JobType": "LOCAL_USE",
            "SnowballType": "EDGE",
            "ShippingOption": "SECOND_DAY",
            "AddressId": address_id,
        }),
        standalone_case,
    )
    .await;
    let standalone_id = string(&created, "JobId", standalone_case);

    ok(
        &router,
        "CancelCluster",
        json!({ "ClusterId": first_id }),
        first_case,
    )
    .await;

    let first_state = cluster_state(&router, &first_id, first_case).await;
    assert_eq!(
        first_state, "Cancelled",
        "case {first_case:?}: expected the cancelled cluster to read back as Cancelled, got \
         {first_state:?}"
    );
    for entry in &first_nodes {
        let node_id = string(entry, "JobId", first_case);
        let state = job_state(&router, &node_id, first_case).await;
        assert_eq!(
            state, "Cancelled",
            "case {first_case:?}: expected the node job {node_id} to be cancelled with its \
             cluster, got {state:?}"
        );
    }

    let second_state = cluster_state(&router, &second_id, second_case).await;
    assert_eq!(
        second_state, "AwaitingQuorum",
        "case {second_case:?}: cancelling another cluster moved this one, which reads back \
         as {second_state:?}"
    );
    for entry in &second_nodes {
        let node_id = string(entry, "JobId", second_case);
        let state = job_state(&router, &node_id, second_case).await;
        assert_eq!(
            state, "New",
            "case {second_case:?}: cancelling another cluster moved the node job {node_id}, \
             which reads back as {state:?}"
        );
    }
    let standalone_state = job_state(&router, &standalone_id, standalone_case).await;
    assert_eq!(
        standalone_state, "New",
        "case {standalone_case:?}: cancelling a cluster moved a job that is not its node, \
         which reads back as {standalone_state:?}"
    );

    // The cluster that was left alone can still be cancelled: the refusal is about state,
    // not about one cancellation per store.
    ok(
        &router,
        "CancelCluster",
        json!({ "ClusterId": second_id }),
        second_case,
    )
    .await;
    let second_after = cluster_state(&router, &second_id, second_case).await;
    assert_eq!(
        second_after, "Cancelled",
        "case {second_case:?}: expected the second cluster to cancel too, got {second_after:?}"
    );
}
