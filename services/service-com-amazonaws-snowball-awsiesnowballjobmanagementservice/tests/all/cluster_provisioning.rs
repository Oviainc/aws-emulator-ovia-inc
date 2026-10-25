// SPDX-License-Identifier: CC0-1.0

//! How a cluster order's size is measured against the capacity its own request declares,
//! driven end to end over HTTP.
//!
//! One rule, asserted over a TABLE of inputs rather than over a single request: a
//! `CreateCluster` that provisions node jobs with the cluster is refused with
//! `InvalidInputCombinationException` (HTTP 400), storing neither the cluster nor any node
//! job, when its `InitialClusterSize` does not meet the node count its own
//! `OnDeviceServiceConfiguration` declares — `S3OnDeviceService.ServiceSize` is how many
//! nodes are needed, so the order meets it at exactly that many, and
//! `S3OnDeviceService.FaultTolerance` is how many nodes may be lost, so the order meets it
//! only with more nodes than that — and `ForceCreateJobs` is the caller's explicit
//! override, which creates the misprovisioned cluster exactly as asked.
//!
//! The rows differ along what the rule generalises over: which node count the
//! configuration declares (the exact count, the loss tolerance, both at once, including a
//! pair no size can satisfy), the value it declares it at (each published bound of the
//! modelled range and an interior value), the configuration forms that declare no node
//! count at all (NFS, the tape gateway and EKS Anywhere, Amazon S3 compatible storage
//! given by storage limit alone, and no configuration whatsoever), the size the order asks
//! for (sizes spanning the modelled range, including none at all), and whether the
//! override is set.
//!
//! Each row drives its own router on its own engine, so the jobs one row creates are never
//! counted against another's and every store stays inside a single `ListJobs` page.
//!
//! The harness (`router`, `router_with_engine`, `call_in_region`, `ok`, `refused`,
//! `create_address`, `deliverable_address`, `string`) and the cluster helpers
//! (`base_cluster`, `cluster_metadata`, `listed_job_ids`, `stored_cluster_ids`) are the
//! ones the other cluster suites already drive the service with, so every call here goes
//! through the same front door a client uses.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::state::CLUSTER_NODE_QUORUM;
use smithy_server_runtime::MultiServiceRouter;

use super::cluster_creation::listed_job_ids;
use super::cluster_device_capability::stored_cluster_ids;
use super::cluster_updates::{base_cluster, cluster_metadata};
use super::job_lifecycle::{
    call_in_region, create_address, deliverable_address, ok, refused, router, router_with_engine,
    string,
};

/// The region every request in this suite signs for.
const SIGNED_REGION: &str = "us-west-2";

/// `CreateClusterRequest$InitialClusterSize` is modelled `range 0..16`
/// (`smithy_query(query="operation", operation="CreateCluster")`), so these are the
/// smallest and largest sizes the member admits.
const INITIAL_CLUSTER_SIZE_MIN: i64 = 0;
const INITIAL_CLUSTER_SIZE_MAX: i64 = 16;

/// `S3OnDeviceServiceConfiguration$ServiceSize` is modelled `range 3..16` and
/// `$FaultTolerance` `range 1..4` (`smithy_query(query="shape",
/// shape="S3OnDeviceServiceConfiguration")`), so these are the published bounds of each
/// node count a configuration can declare.
const SERVICE_SIZE_MIN: i64 = 3;
const SERVICE_SIZE_MAX: i64 = 16;
const FAULT_TOLERANCE_MIN: i64 = 1;
const FAULT_TOLERANCE_MAX: i64 = 4;

/// The node counts an order is tried at: the smallest size that provisions anything, the
/// quorum a cluster ships with, the largest size the member admits, and sizes on either
/// side of every node count the configurations below declare.
const SIZES: &[i64] = &[1, 2, SERVICE_SIZE_MIN, 4, 5, 8, 15, SERVICE_SIZE_MAX];

/// What a configuration declares about the cluster's node count, as the model documents the
/// two members that state one: `ServiceSize` is "how many nodes are needed for Amazon S3
/// compatible storage on Snow family devices" (an exact node count), and `FaultTolerance`
/// is "the number of nodes that can go down without degrading the performance of the
/// cluster" (so the cluster needs one node more than that). A configuration that declares
/// neither leaves both empty and bounds nothing.
#[derive(Debug, Default, Clone, Copy)]
struct Declared {
    exact: Option<i64>,
    at_least: Option<i64>,
}

impl Declared {
    fn exact(nodes: i64) -> Self {
        Self {
            exact: Some(nodes),
            at_least: None,
        }
    }

    fn tolerating(losses: i64) -> Self {
        Self {
            exact: None,
            at_least: Some(losses + 1),
        }
    }

    fn and_tolerating(self, losses: i64) -> Self {
        Self {
            at_least: Some(losses + 1),
            ..self
        }
    }

    /// Whether a cluster of `size` nodes meets everything this configuration declared.
    fn is_met_by(self, size: i64) -> bool {
        self.exact.is_none_or(|nodes| size == nodes)
            && self.at_least.is_none_or(|nodes| size >= nodes)
    }
}

/// One configuration row: the `OnDeviceServiceConfiguration` an order carries (none at
/// all, for the row that carries none) and what it declares about the node count.
struct Row {
    case: String,
    configuration: Option<Value>,
    declared: Declared,
}

/// Every configuration row: the forms that declare a node count, at each published bound
/// and at an interior value, and the forms that declare none.
fn rows() -> Vec<Row> {
    let mut rows: Vec<Row> = vec![
        Row {
            case: "no on-device configuration at all".to_string(),
            configuration: None,
            declared: Declared::default(),
        },
        Row {
            case: "NFS storage, whose limit is per device rather than a node count".to_string(),
            configuration: Some(
                json!({ "NFSOnDeviceService": { "StorageLimit": 50, "StorageUnit": "TB" } }),
            ),
            declared: Declared::default(),
        },
        Row {
            case: "the tape gateway and EKS Anywhere services at once".to_string(),
            configuration: Some(json!({
                "TGWOnDeviceService": { "StorageLimit": 10, "StorageUnit": "TB" },
                "EKSOnDeviceService": {
                    "KubernetesVersion": "1.25",
                    "EKSAnywhereVersion": "0.14.3"
                }
            })),
            declared: Declared::default(),
        },
        Row {
            case: "Amazon S3 compatible storage given by storage limit alone".to_string(),
            configuration: Some(json!({
                "S3OnDeviceService": { "StorageLimit": 25.0, "StorageUnit": "TB" }
            })),
            declared: Declared::default(),
        },
        Row {
            case: "Amazon S3 compatible storage needing nodes it cannot also lose".to_string(),
            configuration: Some(json!({
                "S3OnDeviceService": {
                    "ServiceSize": SERVICE_SIZE_MIN,
                    "FaultTolerance": FAULT_TOLERANCE_MAX
                }
            })),
            declared: Declared::exact(SERVICE_SIZE_MIN).and_tolerating(FAULT_TOLERANCE_MAX),
        },
    ];

    // One row per value the exact node count is declared at: each published bound of the
    // modelled range and an interior value.
    for service_size in [SERVICE_SIZE_MIN, 8, SERVICE_SIZE_MAX] {
        rows.push(Row {
            case: format!("Amazon S3 compatible storage needing {service_size} nodes"),
            configuration: Some(json!({
                "S3OnDeviceService": {
                    "StorageLimit": 100.0,
                    "StorageUnit": "TB",
                    "ServiceSize": service_size
                }
            })),
            declared: Declared::exact(service_size),
        });
    }

    // One row per value the loss tolerance is declared at, with no exact count beside it:
    // each published bound of the modelled range and an interior value.
    for fault_tolerance in [FAULT_TOLERANCE_MIN, 2, FAULT_TOLERANCE_MAX] {
        rows.push(Row {
            case: format!("Amazon S3 compatible storage tolerating {fault_tolerance} lost nodes"),
            configuration: Some(json!({
                "S3OnDeviceService": {
                    "StorageLimit": 50.0,
                    "StorageUnit": "TB",
                    "FaultTolerance": fault_tolerance
                }
            })),
            declared: Declared::tolerating(fault_tolerance),
        });
    }

    // One row per pairing of both node counts, so an order has to meet them together.
    for (service_size, fault_tolerance) in [
        (SERVICE_SIZE_MIN, FAULT_TOLERANCE_MIN),
        (6, FAULT_TOLERANCE_MAX),
        (SERVICE_SIZE_MAX, 2),
    ] {
        rows.push(Row {
            case: format!(
                "Amazon S3 compatible storage needing {service_size} nodes and tolerating \
                 {fault_tolerance} losses"
            ),
            configuration: Some(json!({
                "S3OnDeviceService": {
                    "StorageLimit": 75.0,
                    "StorageUnit": "TB",
                    "ServiceSize": service_size,
                    "FaultTolerance": fault_tolerance
                }
            })),
            declared: Declared::exact(service_size).and_tolerating(fault_tolerance),
        });
    }

    rows
}

/// The order a row is driven with: the cluster order the other cluster suites use, carrying
/// the row's configuration, the size asked for, and the override when it is set.
fn order(address_id: &str, row: &Row, size: Option<i64>, force: Option<bool>) -> Value {
    let mut body = base_cluster(address_id);
    if let Some(configuration) = &row.configuration {
        body["OnDeviceServiceConfiguration"] = configuration.clone();
    }
    if let Some(size) = size {
        body["InitialClusterSize"] = json!(size);
    }
    if let Some(force) = force {
        body["ForceCreateJobs"] = json!(force);
    }
    body
}

/// Read the created cluster back and assert it holds the node jobs and the configuration
/// the order asked for.
async fn assert_created_as_asked(
    router: &MultiServiceRouter,
    created: &Value,
    body: &Value,
    nodes: usize,
    case: &str,
) {
    let cluster_id = string(created, "ClusterId", case);
    let reported = created
        .get("JobListEntries")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    assert_eq!(
        reported, nodes,
        "case {case:?}: expected the create to report {nodes} node jobs, got {created}"
    );
    let metadata = cluster_metadata(router, &cluster_id, case).await;
    assert_eq!(
        metadata
            .get("OnDeviceServiceConfiguration")
            .unwrap_or(&Value::Null),
        body.get("OnDeviceServiceConfiguration")
            .unwrap_or(&Value::Null),
        "case {case:?}: expected the configuration to read back as submitted, got {metadata}"
    );
    // A cluster leaves `AwaitingQuorum` only once it holds the node jobs it ships with.
    let expected_state = if nodes >= CLUSTER_NODE_QUORUM {
        "Pending"
    } else {
        "AwaitingQuorum"
    };
    assert_eq!(
        metadata.get("ClusterState").and_then(Value::as_str),
        Some(expected_state),
        "case {case:?}: expected a cluster holding {nodes} node jobs to be {expected_state}, \
         got {metadata}"
    );
}

/// A router on its own engine with one deliverable address, and a cluster already accepted
/// on it, so the refusals a row asserts are measured against a store that holds a cluster
/// and a node job rather than against an empty one.
async fn row_fixture(
    row: &Row,
    street: &str,
) -> (
    MultiServiceRouter,
    Arc<engine::InMemoryEngine>,
    String,
    BTreeSet<String>,
) {
    let (router, engine) = router_with_engine();
    let address_id = create_address(
        &router,
        deliverable_address("Provisioning Receiver", street, json!({})),
        "the address this row's orders ship to",
    )
    .await;
    // The baseline create carries the override, so it is accepted whatever the row's
    // configuration declares.
    let setup = format!("{}, the accepted order the refusals are measured against", row.case);
    let body = order(&address_id, row, Some(1), Some(true));
    let created = ok(&router, "CreateCluster", body.clone(), &setup).await;
    assert_created_as_asked(&router, &created, &body, 1, &setup).await;
    let jobs = listed_job_ids(&router, &setup).await;
    (router, engine, address_id, jobs)
}

#[tokio::test]
async fn create_cluster_refuses_a_size_the_orders_own_configuration_does_not_declare() {
    for (index, row) in rows().into_iter().enumerate() {
        let (router, engine, address_id, baseline_jobs) =
            row_fixture(&row, &format!("{} Provisioning Way", index + 1)).await;

        for size in SIZES {
            let size = *size;
            let case = format!("{}, asking for {size} node jobs", row.case);
            let body = order(&address_id, &row, Some(size), None);
            let clusters_before = stored_cluster_ids(&engine);
            let jobs_before = listed_job_ids(&router, &case).await;
            let nodes = usize::try_from(size).expect("a non-negative size");

            if row.declared.is_met_by(size) {
                let created = ok(&router, "CreateCluster", body.clone(), &case).await;
                assert_created_as_asked(&router, &created, &body, nodes, &case).await;
                assert_eq!(
                    listed_job_ids(&router, &case).await.len(),
                    jobs_before.len() + nodes,
                    "case {case:?}: expected the create to store one job per node asked for"
                );
            } else {
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
        }

        // The cluster the baseline create stored is still there: no refusal took anything
        // away either.
        assert!(
            listed_job_ids(&router, &row.case)
                .await
                .is_superset(&baseline_jobs),
            "case {:?}: expected the jobs the accepted order stored to survive the refusals",
            row.case
        );
    }
}

#[tokio::test]
async fn force_create_jobs_creates_the_misprovisioned_cluster_as_asked() {
    for (index, row) in rows().into_iter().enumerate() {
        let router = router();
        let address_id = create_address(
            &router,
            deliverable_address(
                "Forcing Receiver",
                &format!("{} Forcing Way", index + 1),
                json!({}),
            ),
            "the address this row's forced orders ship to",
        )
        .await;

        // The sizes the configuration does not declare — both ends of the modelled range,
        // so a forced order is exercised below and above what was declared — and one it
        // does declare, where the override changes nothing.
        let forced: Vec<i64> = [1, INITIAL_CLUSTER_SIZE_MAX]
            .into_iter()
            .filter(|size| !row.declared.is_met_by(*size))
            .chain(SIZES.iter().copied().find(|size| row.declared.is_met_by(*size)))
            .collect();
        for size in forced {
            let case = format!("{}, forcing {size} node jobs", row.case);
            let body = order(&address_id, &row, Some(size), Some(true));
            let created = ok(&router, "CreateCluster", body.clone(), &case).await;
            assert_created_as_asked(
                &router,
                &created,
                &body,
                usize::try_from(size).expect("a non-negative size"),
                &case,
            )
            .await;
        }
    }
}

#[tokio::test]
async fn an_order_that_provisions_no_node_job_is_created_empty_whatever_it_declares() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Empty Receiver", "1 Empty Way", json!({})),
        "the address every empty cluster order ships to",
    )
    .await;

    // "Creates an empty cluster … You use the CreateJob action separately to create the
    // jobs for each of these nodes": an order that asks for no node job attempts no
    // provisioning, so it is created whatever its configuration declares and whether the
    // override is set or not.
    for row in rows() {
        for size in [None, Some(INITIAL_CLUSTER_SIZE_MIN)] {
            for force in [None, Some(false), Some(true)] {
                let case = format!(
                    "{}, asking for {size:?} node jobs forced {force:?}",
                    row.case
                );
                let body = order(&address_id, &row, size, force);
                let created = ok(&router, "CreateCluster", body.clone(), &case).await;
                assert_created_as_asked(&router, &created, &body, 0, &case).await;
            }
        }
    }
}

#[tokio::test]
async fn a_node_count_outside_its_modelled_range_is_refused_before_the_handler() {
    let (router, engine) = router_with_engine();
    let address_id = create_address(
        &router,
        deliverable_address("Bound Receiver", "1 Bound Way", json!({})),
        "the address the out-of-range orders would ship to",
    )
    .await;
    let clusters_before = stored_cluster_ids(&engine);

    // One past each published bound of the two node counts a configuration declares, and of
    // the size an order asks for: each is outside the modelled range, so it is refused
    // before this operation's rule is reached at all.
    for (case, configuration, size) in [
        (
            "a service size one below its published minimum",
            Some(json!({ "S3OnDeviceService": { "ServiceSize": SERVICE_SIZE_MIN - 1 } })),
            1,
        ),
        (
            "a service size one past its published maximum",
            Some(json!({ "S3OnDeviceService": { "ServiceSize": SERVICE_SIZE_MAX + 1 } })),
            1,
        ),
        (
            "a fault tolerance one below its published minimum",
            Some(json!({ "S3OnDeviceService": { "FaultTolerance": FAULT_TOLERANCE_MIN - 1 } })),
            1,
        ),
        (
            "a fault tolerance one past its published maximum",
            Some(json!({ "S3OnDeviceService": { "FaultTolerance": FAULT_TOLERANCE_MAX + 1 } })),
            1,
        ),
        (
            "an initial cluster size one past its published maximum",
            None,
            INITIAL_CLUSTER_SIZE_MAX + 1,
        ),
        (
            "an initial cluster size one below its published minimum",
            None,
            INITIAL_CLUSTER_SIZE_MIN - 1,
        ),
    ] {
        let mut body = base_cluster(&address_id);
        if let Some(configuration) = configuration {
            body["OnDeviceServiceConfiguration"] = configuration;
        }
        body["InitialClusterSize"] = json!(size);
        let (status, parsed) = call_in_region(&router, SIGNED_REGION, "CreateCluster", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a value outside the modelled range to be refused, got \
             {status} with {parsed}"
        );
        assert_eq!(
            stored_cluster_ids(&engine),
            clusters_before,
            "case {case:?}: a refused CreateCluster stored a cluster anyway"
        );
    }
}
