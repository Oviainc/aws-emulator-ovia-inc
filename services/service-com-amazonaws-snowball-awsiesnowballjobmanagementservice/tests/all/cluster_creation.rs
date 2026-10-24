// SPDX-License-Identifier: CC0-1.0

//! `CreateCluster`, driven end to end over HTTP.
//!
//! One rule, asserted over a TABLE of inputs rather than over a single request:
//! `CreateCluster` mints a fresh `ClusterId`, persists every member the request carries
//! against it — so a later `DescribeCluster` reports what was stored — and creates exactly
//! `InitialClusterSize` node jobs (none when the member is absent), each a job of its own
//! that inherits the cluster's attributes, including the ones `ClusterMetadata` does not
//! report but `JobMetadata` does, with the nth node job carrying the nth
//! `LongTermPricingIds` entry and none past that list's end; the created node jobs, and
//! only those, are what `JobListEntries` reports, which a create that made no job omits
//! altogether. A request naming a resource no record answers to — an address it ships or
//! forwards to, or a long-term pricing its node jobs would be associated with — is refused
//! with `InvalidResourceException` and stores neither the cluster nor any node job.
//!
//! The rows differ along what the rule generalises over: the number of node jobs asked for
//! (absent, each published bound of the modelled range, one, the quorum), the length of the
//! `LongTermPricingIds` list relative to that number (none, shorter, equal, longer), the
//! enum value taken from each modelled catalog (`SNOWBALL_CAPACITY_VALUES`,
//! `REMOTE_MANAGEMENT_VALUES`, `SHIPPING_OPTION_VALUES`, read from the crate rather than
//! restated), the form of the on-device service configuration, and which resource a refused
//! row names.
//!
//! `CreateLongTermPricing` is not served by this emulator yet, so a row that needs a stored
//! long-term pricing records it in the service's own store before calling — the store the
//! handler reads, reached the way the service reaches it, exactly as `job_unlock_code` does
//! for a job state. Everything the rule is about still travels over the wire.
//!
//! The harness (`router_with_engine`, `ok`, `refused`, `create_address`,
//! `deliverable_address`, `string`, `assert_prefixed_uuid`) and the cluster helpers
//! (`base_cluster`, `cluster_metadata`, `job_metadata`) are the ones `job_lifecycle`,
//! `job_unlock_code` and `cluster_updates` already drive the service with, so every call
//! here goes through the same front door a client uses.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
// The catalogs the modelled enums admit and the quorum this service holds a cluster to,
// read from the crate rather than restated, so a row cannot drift from the values the
// model allows or from the bound the handler enforces.
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::types::{
    REMOTE_MANAGEMENT_VALUES, SHIPPING_OPTION_VALUES, SNOWBALL_CAPACITY_VALUES,
};
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::state::{
    service_state, CLUSTER_NODE_QUORUM,
};
use smithy_server_runtime::MultiServiceRouter;

use super::cluster_updates::{base_cluster, cluster_metadata, job_metadata};
use super::job_lifecycle::{
    assert_prefixed_uuid, create_address, deliverable_address, ok, refused, string,
};
use super::job_unlock_code::router_with_engine;

/// `CreateClusterRequest$InitialClusterSize` is modelled `range 0..16`
/// (`smithy_query(query="operation", operation="CreateCluster")`), so these are the
/// smallest and largest sizes the member admits.
const INITIAL_CLUSTER_SIZE_MIN: i64 = 0;
const INITIAL_CLUSTER_SIZE_MAX: i64 = 16;

/// An address id in the modelled `AddressId` form that names no stored address.
const UNKNOWN_ADDRESS_ID: &str = "ADID00000000-0000-0000-0000-000000000000";

/// A long-term pricing id in the modelled `LongTermPricingId` form that names no stored
/// long-term pricing record.
const UNKNOWN_LONG_TERM_PRICING_ID: &str = "LTPIDffffffff-ffff-ffff-ffff-ffffffffffff";

/// `ListJobsRequest$MaxResults` is modelled `range 0..100`
/// (`smithy_query(query="operation", operation="ListJobs")`), so this is the largest page
/// the reader below can ask for.
const LIST_JOBS_MAX_RESULTS: i64 = 100;

/// The `ClusterMetadata` members `DescribeCluster` models, so a submitted value for one of
/// them must read back (`smithy_query(query="shape", shape="ClusterMetadata")`). The other
/// members a create carries — `SnowballCapacityPreference`, `RemoteManagement`,
/// `LongTermPricingIds`, `InitialClusterSize` — are not modelled on `ClusterMetadata`, and
/// are checked below on the node jobs, whose `JobMetadata` does model them.
const CLUSTER_METADATA_MEMBERS: &[&str] = &[
    "Description",
    "KmsKeyARN",
    "RoleARN",
    "JobType",
    "SnowballType",
    "Resources",
    "AddressId",
    "ShippingOption",
    "Notification",
    "ForwardingAddressId",
    "TaxDocuments",
    "OnDeviceServiceConfiguration",
];

/// A long-term pricing id in the modelled `LongTermPricingId` form —
/// `^LTPID[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`
/// (`smithy_query(query="shape", shape="LongTermPricingId")`) — distinguished by `seed`.
fn long_term_pricing_id(seed: usize) -> String {
    format!("LTPID{seed:08x}-0000-0000-0000-{seed:012x}")
}

/// Record `ids` as long-term pricing this store holds, so a row can associate node jobs
/// with one.
///
/// `CreateLongTermPricing` is still a scaffold in this crate, so the only way to stand a
/// long-term pricing record up is to write it into the service's own store — the store the
/// handler reads, reached the way the service reaches it — exactly as `job_unlock_code`
/// sets a `JobState` this emulator's operations do not reach yet. Everything the rule is
/// about still travels over the wire.
fn hold_long_term_pricing<'a>(
    engine: &Arc<engine::InMemoryEngine>,
    ids: impl Iterator<Item = &'a String>,
) {
    let state = service_state(&**engine).expect("the service's store is on this engine");
    let mut store = state.lock();
    for id in ids {
        store.long_term_pricing_ids.insert(id.clone());
    }
}

/// One create row: the request body, how many node jobs it must produce, and the
/// long-term pricing ids it supplied for them.
struct Row {
    case: String,
    body: Value,
    nodes: usize,
    pricing_ids: Vec<String>,
}

/// Every create row: one per combination of the node-job count asked for and the length of
/// the long-term pricing list relative to it, plus one per entry of each modelled enum
/// catalog, so the rule is exercised on values no recording cited.
fn create_rows(address_id: &str) -> Vec<Row> {
    // (the size the request carries, how many pricing ids it supplies). The pricing list is
    // shorter than, equal to and longer than the node count, and absent altogether.
    let sizes: Vec<(Option<i64>, usize)> = vec![
        (None, 0),
        (None, 1),
        (Some(INITIAL_CLUSTER_SIZE_MIN), 1),
        (Some(1), 0),
        (Some(2), 1),
        (Some(3), 3),
        (Some(CLUSTER_NODE_QUORUM as i64), CLUSTER_NODE_QUORUM + 2),
        (Some(INITIAL_CLUSTER_SIZE_MAX), 2),
    ];

    let mut rows: Vec<Row> = Vec::new();
    let mut seed = 0usize;
    for (index, (size, pricing_count)) in sizes.into_iter().enumerate() {
        let mut body = base_cluster(address_id);
        if let Some(size) = size {
            body["InitialClusterSize"] = json!(size);
        }
        // Each row also takes its enum values from the modelled catalogs, so the rows span
        // them rather than repeating one value.
        body["ShippingOption"] = json!(SHIPPING_OPTION_VALUES[index % SHIPPING_OPTION_VALUES.len()]);
        body["SnowballCapacityPreference"] =
            json!(SNOWBALL_CAPACITY_VALUES[index % SNOWBALL_CAPACITY_VALUES.len()]);
        body["RemoteManagement"] =
            json!(REMOTE_MANAGEMENT_VALUES[index % REMOTE_MANAGEMENT_VALUES.len()]);
        let pricing_ids: Vec<String> = (0..pricing_count)
            .map(|_| {
                seed += 1;
                long_term_pricing_id(seed)
            })
            .collect();
        if !pricing_ids.is_empty() {
            body["LongTermPricingIds"] = json!(pricing_ids);
        }
        let nodes = usize::try_from(size.unwrap_or(0)).expect("a non-negative initial size");
        rows.push(Row {
            case: format!(
                "an initial cluster size of {size:?} with {pricing_count} long-term pricing ids"
            ),
            body,
            nodes,
            pricing_ids,
        });
    }

    // One row per entry of the device-capacity and remote-management catalogs, each on a
    // cluster holding a single node job, so every value the model admits is inherited at
    // least once.
    for (index, capacity) in SNOWBALL_CAPACITY_VALUES.iter().enumerate() {
        let remote = REMOTE_MANAGEMENT_VALUES[index % REMOTE_MANAGEMENT_VALUES.len()];
        let mut body = base_cluster(address_id);
        body["InitialClusterSize"] = json!(1);
        body["SnowballCapacityPreference"] = json!(capacity);
        body["RemoteManagement"] = json!(remote);
        seed += 1;
        let pricing_ids = vec![long_term_pricing_id(seed)];
        body["LongTermPricingIds"] = json!(pricing_ids);
        rows.push(Row {
            case: format!("a {capacity} cluster managed {remote}"),
            body,
            nodes: 1,
            pricing_ids,
        });
    }
    for remote in REMOTE_MANAGEMENT_VALUES {
        let mut body = base_cluster(address_id);
        body["InitialClusterSize"] = json!(2);
        body["RemoteManagement"] = json!(remote);
        rows.push(Row {
            case: format!("a cluster managed {remote} with no long-term pricing"),
            body,
            nodes: 2,
            pricing_ids: Vec::new(),
        });
    }

    // One row per on-device service a cluster can be configured with, so the structure
    // member a create persists is round-tripped on every form it takes
    // (`smithy_query(query="shape", shape="OnDeviceServiceConfiguration")` names
    // `NFSOnDeviceService`, `S3OnDeviceService`, `TGWOnDeviceService` and
    // `EKSOnDeviceService`; `StorageUnit` is modelled with the single value `TB`, and
    // `S3OnDeviceService`'s `ServiceSize` is `range 3..16` and its `FaultTolerance`
    // `range 1..4`, so the values below sit inside each published bound).
    let on_device_configurations: Vec<(&str, Value)> = vec![
        (
            "NFS storage",
            json!({ "NFSOnDeviceService": { "StorageLimit": 50, "StorageUnit": "TB" } }),
        ),
        (
            "S3 compatible storage at the smallest service size",
            json!({
                "S3OnDeviceService": {
                    "StorageLimit": 25.0,
                    "StorageUnit": "TB",
                    "ServiceSize": 3,
                    "FaultTolerance": 1
                }
            }),
        ),
        (
            "S3 compatible storage at the largest service size",
            json!({
                "S3OnDeviceService": {
                    "StorageLimit": 100.0,
                    "StorageUnit": "TB",
                    "ServiceSize": 16,
                    "FaultTolerance": 4
                }
            }),
        ),
        (
            "the tape gateway and EKS Anywhere services at once",
            json!({
                "TGWOnDeviceService": { "StorageLimit": 10, "StorageUnit": "TB" },
                "EKSOnDeviceService": {
                    "KubernetesVersion": "1.25",
                    "EKSAnywhereVersion": "0.14.3"
                }
            }),
        ),
    ];
    for (what, configuration) in on_device_configurations {
        let mut body = base_cluster(address_id);
        body["InitialClusterSize"] = json!(1);
        body["OnDeviceServiceConfiguration"] = configuration;
        seed += 1;
        let pricing_ids = vec![long_term_pricing_id(seed)];
        body["LongTermPricingIds"] = json!(pricing_ids);
        rows.push(Row {
            case: format!("a cluster configured for {what}"),
            body,
            nodes: 1,
            pricing_ids,
        });
    }

    rows
}

/// The node jobs `JobListEntries` reports for a create, in the order the create made them.
fn reported_nodes(created: &Value, expected: usize, case: &str) -> Vec<Value> {
    match created.get("JobListEntries") {
        // "List of jobs created for this cluster": a create that made no job reports no
        // list rather than an empty one.
        None => {
            assert_eq!(
                expected, 0,
                "case {case:?}: expected {expected} node jobs, got no JobListEntries in \
                 {created}"
            );
            Vec::new()
        }
        Some(entries) => {
            let entries = entries
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: expected a list, got {created}"))
                .clone();
            assert_eq!(
                entries.len(),
                expected,
                "case {case:?}: expected one entry per node job asked for, got {entries:?}"
            );
            assert!(
                expected > 0,
                "case {case:?}: expected no JobListEntries at all for a create that made no \
                 job, got {entries:?}"
            );
            entries
        }
    }
}

#[tokio::test]
async fn create_cluster_persists_its_inputs_and_mints_the_node_jobs_it_was_asked_for() {
    let (router, engine) = router_with_engine();
    let address_id = create_address(
        &router,
        deliverable_address("Creation Receiver", "9 Cluster Way", json!({})),
        "the address every created cluster ships to",
    )
    .await;
    let forwarding_address_id = create_address(
        &router,
        deliverable_address("Creation Forwarder", "10 Cluster Way", json!({})),
        "the address a created cluster forwards to",
    )
    .await;

    // Every minted id, so no two creates and no two node jobs can share one.
    let mut cluster_ids: BTreeSet<String> = BTreeSet::new();
    let mut job_ids: BTreeSet<String> = BTreeSet::new();

    let rows = create_rows(&address_id);
    // The long-term pricing every row associates its node jobs with must be a record this
    // store holds, since a create naming one it does not is refused.
    hold_long_term_pricing(&engine, rows.iter().flat_map(|row| row.pricing_ids.iter()));
    // What the whole table must have created by the end: one cluster per row and one job
    // per node any row asked for.
    let expected_clusters = rows.len();
    let expected_jobs: usize = rows.iter().map(|row| row.nodes).sum();

    for row in rows {
        let Row {
            case,
            mut body,
            nodes,
            pricing_ids,
        } = row;
        // A forwarding address is a member a cluster keeps too, so every row carries one.
        body["ForwardingAddressId"] = json!(forwarding_address_id);

        let created = ok(&router, "CreateCluster", body.clone(), &case).await;
        let cluster_id = string(&created, "ClusterId", &case);
        assert_prefixed_uuid(&cluster_id, "CID", &case);
        assert!(
            cluster_ids.insert(cluster_id.clone()),
            "case {case:?}: expected a freshly minted cluster id, got the already-minted \
             {cluster_id:?}"
        );

        // What was asked for is what the stored cluster reports.
        let metadata = cluster_metadata(&router, &cluster_id, &case).await;
        assert_eq!(
            metadata.get("ClusterId").and_then(Value::as_str),
            Some(cluster_id.as_str()),
            "case {case:?}: expected the cluster to report its own id, got {metadata}"
        );
        for member in CLUSTER_METADATA_MEMBERS {
            assert_eq!(
                metadata.get(*member).unwrap_or(&Value::Null),
                body.get(*member).unwrap_or(&Value::Null),
                "case {case:?}: expected {member} to read back as submitted, got {metadata}"
            );
        }
        let creation_date = metadata
            .get("CreationDate")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected a CreationDate, got {metadata}"));
        // A cluster leaves `AwaitingQuorum` only once it holds the node jobs it ships with,
        // which an initial size at or past the quorum gives it straight away.
        let expected_state = if nodes >= CLUSTER_NODE_QUORUM {
            "Pending"
        } else {
            "AwaitingQuorum"
        };
        assert_eq!(
            metadata.get("ClusterState").and_then(Value::as_str),
            Some(expected_state),
            "case {case:?}: expected a cluster holding {nodes} node jobs to be \
             {expected_state}, got {metadata}"
        );

        // Exactly the node jobs the create made, in the order it made them.
        let entries = reported_nodes(&created, nodes, &case);
        for (index, entry) in entries.iter().enumerate() {
            let node_case = format!("{case}, node job {index}");
            let job_id = string(entry, "JobId", &node_case);
            assert_prefixed_uuid(&job_id, "JID", &node_case);
            assert!(
                job_ids.insert(job_id.clone()),
                "case {node_case:?}: expected a freshly minted job id, got the \
                 already-minted {job_id:?}"
            );
            assert_eq!(
                entry.get("JobState").and_then(Value::as_str),
                Some("New"),
                "case {node_case:?}: expected a created node job to be New, got {entry}"
            );
            assert!(
                !entry
                    .get("IsMaster")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                "case {node_case:?}: expected a cluster node to be no main job, got {entry}"
            );

            // The node job is readable on its own and carries the cluster's attributes,
            // the members `ClusterMetadata` does not report included.
            let node = job_metadata(&router, &job_id, &node_case).await;
            assert_eq!(
                node.get("ClusterId").and_then(Value::as_str),
                Some(cluster_id.as_str()),
                "case {node_case:?}: expected the node job to name its cluster, got {node}"
            );
            for member in [
                "JobType",
                "SnowballType",
                "Description",
                "AddressId",
                "ForwardingAddressId",
                "RoleARN",
                "KmsKeyARN",
                "Resources",
                "Notification",
                "TaxDocuments",
                "SnowballCapacityPreference",
                "RemoteManagement",
            ] {
                assert_eq!(
                    node.get(member).unwrap_or(&Value::Null),
                    body.get(member).unwrap_or(&Value::Null),
                    "case {node_case:?}: expected the node job to inherit the cluster's \
                     {member}, got {node}"
                );
            }
            assert_eq!(
                node["ShippingDetails"].get("ShippingOption"),
                body.get("ShippingOption"),
                "case {node_case:?}: expected the node job to inherit the cluster's \
                 shipping speed, got {node}"
            );
            // The cluster and the jobs the same call created share one creation instant.
            assert_eq!(
                node.get("CreationDate"),
                Some(&creation_date),
                "case {node_case:?}: expected the node job to be created with its cluster \
                 at {creation_date}, got {node}"
            );
            // "Lists long-term pricing id that will be used to associate with jobs
            // automatically created for the new cluster": the ids are a list over those
            // jobs, so the nth job takes the nth id and a job past the list's end none.
            assert_eq!(
                node.get("LongTermPricingId").and_then(Value::as_str),
                pricing_ids.get(index).map(String::as_str),
                "case {node_case:?}: expected the long-term pricing id at position \
                 {index} of {pricing_ids:?}, got {node}"
            );
        }
    }

    // Every row created its own cluster and exactly the node jobs it asked for, each under
    // an id of its own.
    assert_eq!(
        cluster_ids.len(),
        expected_clusters,
        "expected one distinct cluster per row, got {}",
        cluster_ids.len()
    );
    assert_eq!(
        job_ids.len(),
        expected_jobs,
        "expected one distinct node job per node asked for, got {}",
        job_ids.len()
    );
    // And the store holds exactly those jobs: no create made a job it did not report.
    assert_eq!(
        job_ids,
        listed_job_ids(&router, "every job the table created").await,
        "expected the reported node jobs to be exactly the stored jobs"
    );
}

#[tokio::test]
async fn a_create_naming_a_resource_no_record_answers_to_stores_nothing() {
    let (router, engine) = router_with_engine();
    let address_id = create_address(
        &router,
        deliverable_address("Known Creation Receiver", "11 Cluster Way", json!({})),
        "a known address",
    )
    .await;

    // A cluster that was accepted, so the refused rows below are checked against a store
    // that already holds node jobs rather than against an empty one.
    let setup = "the cluster the refused rows are measured against";
    let mut accepted = base_cluster(&address_id);
    accepted["InitialClusterSize"] = json!(2);
    let created = ok(&router, "CreateCluster", accepted, setup).await;
    let cluster_id = string(&created, "ClusterId", setup);
    let before = listed_job_ids(&router, setup).await;
    assert_eq!(
        before.len(),
        2,
        "case {setup:?}: expected the accepted create to have made its two node jobs, got \
         {before:?}"
    );

    // A long-term pricing record a row can name, so a row that names a known one beside an
    // unknown one is possible at all.
    let known_pricing_id = long_term_pricing_id(1);
    hold_long_term_pricing(&engine, [known_pricing_id.clone()].iter());

    // Each row names one resource no record answers to, in the modelled form for its kind:
    // the shipping address, the forwarding address, or one of the long-term pricing ids the
    // node jobs would be associated with. Each asks for node jobs, so a refusal that stored
    // anything shows up as a job.
    let rows: Vec<(&str, Value)> = vec![
        (
            "an AddressId that was never created",
            json!({ "AddressId": UNKNOWN_ADDRESS_ID }),
        ),
        (
            "a ForwardingAddressId that was never created",
            json!({ "ForwardingAddressId": UNKNOWN_ADDRESS_ID }),
        ),
        (
            "a known address beside a forwarding one that was never created",
            json!({
                "AddressId": address_id,
                "ForwardingAddressId": UNKNOWN_ADDRESS_ID
            }),
        ),
        (
            "a LongTermPricingId that was never created",
            json!({ "LongTermPricingIds": [UNKNOWN_LONG_TERM_PRICING_ID] }),
        ),
        (
            "a known long-term pricing beside one that was never created",
            json!({
                "LongTermPricingIds": [known_pricing_id, UNKNOWN_LONG_TERM_PRICING_ID]
            }),
        ),
    ];
    for (case, overrides) in rows {
        for size in [INITIAL_CLUSTER_SIZE_MIN, 1, INITIAL_CLUSTER_SIZE_MAX] {
            let row_case = format!("{case}, asking for {size} node jobs");
            let mut body = base_cluster(&address_id);
            body["InitialClusterSize"] = json!(size);
            for (member, value) in overrides.as_object().expect("the overrides are an object") {
                body[member] = value.clone();
            }
            refused(
                &router,
                "CreateCluster",
                body,
                "InvalidResourceException",
                &row_case,
            )
            .await;
            assert_eq!(
                listed_job_ids(&router, &row_case).await,
                before,
                "case {row_case:?}: a refused CreateCluster stored a job anyway"
            );
        }
    }

    // The cluster the accepted create stored is still the one it stored.
    let metadata = cluster_metadata(&router, &cluster_id, setup).await;
    assert_eq!(
        metadata.get("AddressId").and_then(Value::as_str),
        Some(address_id.as_str()),
        "case {setup:?}: expected the stored cluster to be untouched by the refusals, got \
         {metadata}"
    );
}

/// Every job id `ListJobs` reports, read back through the list reader rather than out of
/// the state struct.
///
/// One page at the largest `MaxResults` the member admits, and the absence of a
/// `NextToken` is asserted, so a store that outgrew a page fails loudly here rather than
/// silently answering with a truncated set.
pub(crate) async fn listed_job_ids(router: &MultiServiceRouter, case: &str) -> BTreeSet<String> {
    let listed = ok(
        router,
        "ListJobs",
        json!({ "MaxResults": LIST_JOBS_MAX_RESULTS }),
        case,
    )
    .await;
    assert_eq!(
        listed.get("NextToken"),
        None,
        "case {case:?}: expected every stored job on one page, got {listed}"
    );
    listed["JobListEntries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected JobListEntries, got {listed}"))
        .iter()
        .map(|entry| string(entry, "JobId", case))
        .collect()
}
