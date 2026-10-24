// SPDX-License-Identifier: CC0-1.0

//! Which Snow Family devices a cluster can be ordered on, driven end to end over HTTP.
//!
//! One rule, asserted over a TABLE of inputs rather than over the single request an issue
//! recorded: a cluster is ordered only in the one job type and the one device type a
//! cluster's own documentation says a cluster supports, so `CreateCluster` refuses every
//! other pairing of the two modelled enums with `InvalidInputCombinationException`
//! (HTTP 400) and stores nothing for it — neither a cluster a later `DescribeCluster`
//! could report nor any of the node jobs `InitialClusterSize` asked for — and a node job
//! is only ever a device of the type its cluster was ordered on, whatever the node request
//! names.
//!
//! The rows differ along what the rule generalises over: the device type (every value the
//! modelled `SnowballType` enum admits, which spans the Snowcone, the original Snowball,
//! the Edge, the Snowblade and the rack families), the job type (every value the modelled
//! `JobType` enum admits), how many node jobs the refused order asked for (absent, each
//! published bound of the modelled `InitialClusterSize` range, one, the quorum — crossed
//! against one unsupported pairing per device family), and which of the two members a node
//! request names.
//!
//! `job_order_validation` asserts over the same cross product which pairing `CreateCluster`
//! accepts; what is added here is what the store holds after a refusal — a refused order
//! that asked for node jobs must leave none behind — and the device type a cluster's node
//! jobs can be.
//!
//! The harness (`router`, `router_with_engine`, `ok`, `refused`, `create_address`,
//! `deliverable_address`, `string`) and the cluster helpers (`base_cluster`,
//! `cluster_metadata`, `job_metadata`, `listed_job_ids`) are the ones `job_lifecycle`,
//! `cluster_updates` and `cluster_creation` already drive the service with, so every call
//! here goes through the same front door a client uses. `ListClusters` is still a scaffold
//! in this crate, so the clusters the emulator holds are counted in the service's own
//! store — the store the handler writes, reached the way the service reaches it, exactly
//! as `cluster_creation` records a long-term pricing there. Everything the rule is about
//! still travels over the wire.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
// The one job and device type a cluster is ordered in, the catalogs the two modelled enums
// admit and the quorum this service holds a cluster to, read from the crate rather than
// restated, so a row cannot drift from the values the model allows.
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::create_cluster::{
    CLUSTER_JOB_TYPE, CLUSTER_SNOWBALL_TYPE,
};
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::types::{
    JOB_TYPE_VALUES, SNOWBALL_TYPE_VALUES,
};
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::state::{
    service_state, CLUSTER_NODE_QUORUM,
};

use super::cluster_creation::listed_job_ids;
use super::cluster_updates::{base_cluster, cluster_metadata, job_metadata};
use super::job_lifecycle::{
    create_address, deliverable_address, ok, refused, router, router_with_engine, string,
};

/// `CreateClusterRequest$InitialClusterSize` is modelled `range 0..16`
/// (`smithy_query(query="operation", operation="CreateCluster")`), so these are the
/// smallest and largest sizes the member admits.
const INITIAL_CLUSTER_SIZE_MIN: i64 = 0;
const INITIAL_CLUSTER_SIZE_MAX: i64 = 16;

/// Every cluster id this emulator holds.
///
/// `ListClusters` is still a scaffold in this crate, so the only way to see a cluster the
/// caller was never given an id for is the service's own store — the store `CreateCluster`
/// writes, reached the way the service reaches it.
fn stored_cluster_ids(engine: &Arc<engine::InMemoryEngine>) -> BTreeSet<String> {
    let state = service_state(&**engine).expect("the service's store is on this engine");
    let store = state.lock();
    store.clusters.keys().cloned().collect()
}

/// A cluster order on `snowball_type` devices for `job_type`, every other member taken
/// from the order the cluster suites already drive the service with, so a row differs only
/// along the two members the rule is about.
fn cluster_order(address_id: &str, job_type: &str, snowball_type: &str) -> Value {
    let mut body = base_cluster(address_id);
    body["JobType"] = json!(job_type);
    body["SnowballType"] = json!(snowball_type);
    body
}

#[tokio::test]
async fn create_cluster_refuses_every_pairing_but_the_one_a_cluster_supports_and_stores_nothing() {
    let (router, engine) = router_with_engine();
    let address_id = create_address(
        &router,
        deliverable_address("Cluster Device Receiver", "12 Cluster Way", json!({})),
        "the address every cluster order ships to",
    )
    .await;

    // An order that was accepted, so each refused row below is measured against a store
    // that already holds a cluster and its node jobs rather than against an empty one.
    let setup = "the cluster the refused rows are measured against";
    let mut accepted = cluster_order(&address_id, CLUSTER_JOB_TYPE, CLUSTER_SNOWBALL_TYPE);
    accepted["InitialClusterSize"] = json!(2);
    let created = ok(&router, "CreateCluster", accepted, setup).await;
    let cluster_id = string(&created, "ClusterId", setup);
    let clusters_before = stored_cluster_ids(&engine);
    let jobs_before = listed_job_ids(&router, setup).await;
    assert_eq!(
        jobs_before.len(),
        2,
        "case {setup:?}: expected the accepted order to have made its two node jobs, got \
         {jobs_before:?}"
    );

    // Every pairing of the two modelled enums but the supported one, each asking for a node
    // job so a refusal that stored anything shows up as one.
    let mut rows: Vec<(String, Value)> = Vec::new();
    for snowball_type in SNOWBALL_TYPE_VALUES {
        for job_type in JOB_TYPE_VALUES {
            if *snowball_type == CLUSTER_SNOWBALL_TYPE && *job_type == CLUSTER_JOB_TYPE {
                continue;
            }
            let mut body = cluster_order(&address_id, job_type, snowball_type);
            body["InitialClusterSize"] = json!(1);
            rows.push((
                format!("a {job_type} cluster on {snowball_type} devices"),
                body,
            ));
        }
    }

    // The node-job count a refused order asks for is an axis of its own, crossed against
    // one unsupported pairing per device family rather than against all of them: none
    // stated at all, each published bound of the modelled range, and the quorum — so a
    // refusal is shown to store no node job however many it was asked for.
    let sizes: Vec<Option<i64>> = vec![
        None,
        Some(INITIAL_CLUSTER_SIZE_MIN),
        Some(CLUSTER_NODE_QUORUM as i64),
        Some(INITIAL_CLUSTER_SIZE_MAX),
    ];
    for (job_type, snowball_type) in [
        ("IMPORT", "SNC1_SSD"),
        ("EXPORT", "SNC1_HDD"),
        ("IMPORT", "STANDARD"),
        ("LOCAL_USE", "V3_5C"),
        ("IMPORT", CLUSTER_SNOWBALL_TYPE),
    ] {
        for size in &sizes {
            let mut body = cluster_order(&address_id, job_type, snowball_type);
            if let Some(size) = size {
                body["InitialClusterSize"] = json!(size);
            }
            rows.push((
                format!(
                    "a {job_type} cluster on {snowball_type} devices asking for {size:?} node jobs"
                ),
                body,
            ));
        }
    }

    for (case, body) in rows {
        refused(
            &router,
            "CreateCluster",
            body,
            "InvalidInputCombinationException",
            &case,
        )
        .await;
        assert_eq!(
            stored_cluster_ids(&engine),
            clusters_before,
            "case {case:?}: a refused CreateCluster stored a cluster anyway"
        );
        assert_eq!(
            listed_job_ids(&router, &case).await,
            jobs_before,
            "case {case:?}: a refused CreateCluster stored a node job anyway"
        );
    }

    // The cluster the accepted order stored is still the cluster it stored, on the device
    // type a cluster is formed from, so it is the pairing the refusals turn on and not the
    // shape of the request.
    let metadata = cluster_metadata(&router, &cluster_id, setup).await;
    assert_eq!(
        metadata.get("SnowballType").and_then(Value::as_str),
        Some(CLUSTER_SNOWBALL_TYPE),
        "case {setup:?}: expected the stored cluster to be untouched by the refusals, got \
         {metadata}"
    );
    assert_eq!(
        metadata.get("JobType").and_then(Value::as_str),
        Some(CLUSTER_JOB_TYPE),
        "case {setup:?}: expected the stored cluster to be untouched by the refusals, got \
         {metadata}"
    );
}

#[tokio::test]
async fn a_cluster_node_is_only_ever_a_device_of_the_type_its_cluster_was_ordered_on() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Cluster Node Receiver", "13 Cluster Way", json!({})),
        "the address the cluster and its nodes ship to",
    )
    .await;

    let setup = "the cluster the node rows join";
    let created = ok(
        &router,
        "CreateCluster",
        cluster_order(&address_id, CLUSTER_JOB_TYPE, CLUSTER_SNOWBALL_TYPE),
        setup,
    )
    .await;
    let cluster_id = string(&created, "ClusterId", setup);

    // A node request that names a device type or a job type other than its cluster's is
    // refused and stores nothing, over every other value the two modelled enums admit —
    // so no device family a cluster cannot be formed from reaches a cluster node either.
    let mut rows: Vec<(String, Value)> = Vec::new();
    for snowball_type in SNOWBALL_TYPE_VALUES
        .iter()
        .filter(|value| **value != CLUSTER_SNOWBALL_TYPE)
    {
        rows.push((
            format!("a node of the cluster asked for as a {snowball_type} device"),
            json!({ "ClusterId": cluster_id, "SnowballType": snowball_type }),
        ));
    }
    for job_type in JOB_TYPE_VALUES
        .iter()
        .filter(|value| **value != CLUSTER_JOB_TYPE)
    {
        rows.push((
            format!("a node of the cluster asked for as a {job_type} job"),
            json!({ "ClusterId": cluster_id, "JobType": job_type }),
        ));
    }

    let before = listed_job_ids(&router, "before the refused node rows").await;
    assert!(
        before.is_empty(),
        "case {setup:?}: expected a cluster created with no initial size to hold no node \
         job, got {before:?}"
    );
    for (case, body) in rows {
        refused(
            &router,
            "CreateJob",
            body,
            "InvalidInputCombinationException",
            &case,
        )
        .await;
        assert_eq!(
            listed_job_ids(&router, &case).await,
            before,
            "case {case:?}: a refused CreateJob stored a node job anyway"
        );
    }

    // A node request that names nothing but its cluster, and one that repeats the cluster's
    // own pairing, are both accepted — and each reads back as a device of the cluster's
    // type.
    for (case, body) in [
        (
            "a node naming nothing but its cluster",
            json!({ "ClusterId": cluster_id }),
        ),
        (
            "a node repeating its cluster's own job and device type",
            json!({
                "ClusterId": cluster_id,
                "JobType": CLUSTER_JOB_TYPE,
                "SnowballType": CLUSTER_SNOWBALL_TYPE
            }),
        ),
    ] {
        let node = ok(&router, "CreateJob", body, case).await;
        let job_id = string(&node, "JobId", case);
        let metadata = job_metadata(&router, &job_id, case).await;
        assert_eq!(
            metadata.get("ClusterId").and_then(Value::as_str),
            Some(cluster_id.as_str()),
            "case {case:?}: expected the node job to name its cluster, got {metadata}"
        );
        assert_eq!(
            metadata.get("SnowballType").and_then(Value::as_str),
            Some(CLUSTER_SNOWBALL_TYPE),
            "case {case:?}: expected the node job to be a device of its cluster's type, \
             got {metadata}"
        );
        assert_eq!(
            metadata.get("JobType").and_then(Value::as_str),
            Some(CLUSTER_JOB_TYPE),
            "case {case:?}: expected the node job to be a job of its cluster's type, got \
             {metadata}"
        );
    }

    // Exactly the two node jobs the accepted requests made are what the store holds, so no
    // refused row left one behind.
    let stored = listed_job_ids(&router, "every node job the accepted rows made").await;
    assert_eq!(
        stored.len(),
        2,
        "expected the store to hold exactly the two accepted node jobs, got {stored:?}"
    );
}
