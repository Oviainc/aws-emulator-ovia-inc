// SPDX-License-Identifier: CC0-1.0

//! `DescribeCluster`, driven end to end over HTTP.
//!
//! One rule, asserted over a TABLE of inputs rather than over a single request:
//! `DescribeCluster` reports the cluster the request names as the `ClusterMetadata` the
//! model declares — every member that was persisted for it, the lifecycle state its node
//! jobs have brought it to, and nothing else — computes that report from the stored cluster
//! alone, so a member no create or update supplied is reported by no value at all and the
//! clusters this emulator holds beside it read back unchanged; and it is a REPORT, so no
//! number of calls moves the cluster's state or alters what a later caller sees. A request
//! naming a cluster no record answers to is refused with `InvalidResourceException`
//! (HTTP 400), naming the id it was asked about and storing nothing.
//!
//! The rows differ along what the rule generalises over: which members the stored cluster
//! carries (only the four `CreateCluster` requires, some of the optional ones, every member
//! `ClusterMetadata` models), the enum value from the modelled catalog
//! (`SHIPPING_OPTION_VALUES`, read from the crate rather than restated), the length of a
//! value at each published bound of `Description` and a non-ascii one, the shape of the
//! structure members (one bucket, several, a key range, each on-device service form), and
//! the number of node jobs the cluster holds (none, one, one short of the quorum, the
//! quorum, the largest initial size the model admits) — which is what the reported
//! `ClusterState` follows from, whether the nodes came with the create or were added one
//! `CreateJob` at a time afterwards.
//!
//! The harness (`router`, `router_with_engine`, `call_in_region`, `ok`, `create_address`,
//! `deliverable_address`, `string`) and the cluster helpers (`base_cluster`,
//! `cluster_metadata`, `create_cluster`, `stored_cluster_ids`) are the ones the other
//! cluster suites already drive the service with, so every call here goes through the same
//! front door a client uses.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
// The catalog the modelled `ShippingOption` enum admits and the quorum this service holds a
// cluster to, read from the crate rather than restated, so a row cannot drift from the
// values the model allows or from the bound the handler enforces.
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::types::SHIPPING_OPTION_VALUES;
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::state::CLUSTER_NODE_QUORUM;
use smithy_server_runtime::MultiServiceRouter;

use super::cluster_device_capability::stored_cluster_ids;
use super::cluster_updates::{base_cluster, cluster_metadata, create_cluster};
use super::job_lifecycle::{
    call_in_region, create_address, deliverable_address, ok, router, router_with_engine, string,
};

/// The region every request in this suite signs for.
const SIGNED_REGION: &str = "us-west-2";

/// Every member `ClusterMetadata` models, so the report is checked against the model's
/// structure rather than against the members one request happened to carry
/// (`smithy_query(query="shape", shape="ClusterMetadata")`).
const CLUSTER_METADATA_MEMBERS: &[&str] = &[
    "ClusterId",
    "Description",
    "KmsKeyARN",
    "RoleARN",
    "ClusterState",
    "JobType",
    "SnowballType",
    "CreationDate",
    "Resources",
    "AddressId",
    "ShippingOption",
    "Notification",
    "ForwardingAddressId",
    "TaxDocuments",
    "OnDeviceServiceConfiguration",
];

/// The `ClusterMetadata` members the service computes for itself rather than taking from the
/// request: the id it minted, the lifecycle state it holds the cluster in, and the instant
/// it created it.
const SERVICE_COMPUTED_MEMBERS: &[&str] = &["ClusterId", "ClusterState", "CreationDate"];

/// The members a `CreateCluster` carries that `ClusterMetadata` does NOT model
/// (`smithy_query(query="operation", operation="CreateCluster")` beside
/// `smithy_query(query="shape", shape="ClusterMetadata")`), so a report that carried one
/// would be reporting a member the shape has no room for.
const MEMBERS_CLUSTER_METADATA_DOES_NOT_MODEL: &[&str] = &[
    "SnowballCapacityPreference",
    "RemoteManagement",
    "InitialClusterSize",
    "LongTermPricingIds",
    "ForceCreateJobs",
    "JobListEntries",
];

/// `CreateClusterRequest$Description` is modelled `length 1..1024`
/// (`smithy_query(query="operation", operation="CreateCluster")`), so these are the shortest
/// and longest values the member admits.
const DESCRIPTION_MIN_LEN: usize = 1;
const DESCRIPTION_MAX_LEN: usize = 1024;

/// `CreateClusterRequest$InitialClusterSize` is modelled `range 0..16`
/// (`smithy_query(query="operation", operation="CreateCluster")`), so this is the largest
/// number of node jobs a create can provision.
const INITIAL_CLUSTER_SIZE_MAX: usize = 16;

/// The state a cluster that does not yet hold the node jobs it ships with is in, and the one
/// it is in once it does (`shared_types::CLUSTER_STATE_VALUES`).
const AWAITING_QUORUM: &str = "AwaitingQuorum";
const PENDING: &str = "Pending";

/// Cluster ids in the modelled `ClusterId` form —
/// `^CID[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`
/// (`smithy_query(query="operation", operation="DescribeCluster")`) — that name no stored
/// cluster, spanning the hex range the pattern admits.
const UNKNOWN_CLUSTER_IDS: &[&str] = &[
    "CID00000000-0000-0000-0000-000000000000",
    "CIDffffffff-ffff-ffff-ffff-ffffffffffff",
    "CID0123abcd-4567-89ef-0123-456789abcdef",
    "CIDdeadbeef-feed-face-cafe-0123456789ab",
];

/// The report `DescribeCluster` answers for `cluster_id`, with the state it reports checked
/// against `expected_state` and the whole report checked against the shape the model
/// declares.
async fn described(
    router: &MultiServiceRouter,
    cluster_id: &str,
    expected_state: &str,
    case: &str,
) -> Value {
    let metadata = cluster_metadata(router, cluster_id, case).await;
    let members: Vec<String> = metadata
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ClusterMetadata, got {metadata}"))
        .keys()
        .cloned()
        .collect();
    for member in &members {
        assert!(
            CLUSTER_METADATA_MEMBERS.contains(&member.as_str()),
            "case {case:?}: expected only members ClusterMetadata models, got {member:?} in \
             {metadata}"
        );
    }
    assert_eq!(
        metadata.get("ClusterId").and_then(Value::as_str),
        Some(cluster_id),
        "case {case:?}: expected the report to name the cluster it was asked about, got \
         {metadata}"
    );
    assert_eq!(
        metadata.get("ClusterState").and_then(Value::as_str),
        Some(expected_state),
        "case {case:?}: expected ClusterState {expected_state}, got {metadata}"
    );
    metadata
}

/// Assert `metadata` reports exactly the members `submitted` carried, by the value it
/// carried, and reports NO value for a member `ClusterMetadata` models that `submitted` left
/// out.
fn assert_reports_what_was_stored(metadata: &Value, submitted: &Value, case: &str) {
    for member in CLUSTER_METADATA_MEMBERS {
        if SERVICE_COMPUTED_MEMBERS.contains(member) {
            continue;
        }
        assert_eq!(
            metadata.get(*member).unwrap_or(&Value::Null),
            submitted.get(*member).unwrap_or(&Value::Null),
            "case {case:?}: expected {member} to be reported exactly as stored, got {metadata}"
        );
    }
    for member in MEMBERS_CLUSTER_METADATA_DOES_NOT_MODEL {
        assert_eq!(
            metadata.get(*member),
            None,
            "case {case:?}: expected no {member} on a ClusterMetadata, got {metadata}"
        );
    }
}

/// The epoch seconds a reported timestamp stands for: `awsJson1_1` puts a timestamp on the
/// wire as epoch seconds, which may carry a fraction.
fn epoch_seconds(metadata: &Value, member: &str, case: &str) -> f64 {
    metadata
        .get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: expected a numeric {member}, got {metadata}"))
}

/// Now, in epoch seconds.
fn now_epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_secs_f64()
}

/// One row: the create body the cluster is stored from, and how many node jobs it holds by
/// the time it is described.
struct Row {
    case: String,
    body: Value,
    nodes: usize,
}

/// The create body carrying only the members `CreateCluster` requires, so a row built on it
/// proves an optional member nothing supplied is reported by no value at all.
fn required_members_only(address_id: &str, shipping_option: &str) -> Value {
    json!({
        "JobType": "LOCAL_USE",
        "SnowballType": "EDGE",
        "ShippingOption": shipping_option,
        "AddressId": address_id,
    })
}

/// Every row: one per shape of stored cluster, crossed with the node-job counts the reported
/// state follows from.
fn rows(address_id: &str, forwarding_address_id: &str) -> Vec<Row> {
    // The node-job counts a cluster is described at: none, one, one short of the quorum it
    // ships with, the quorum itself, and the largest size a create can provision.
    let node_counts: Vec<usize> = vec![
        0,
        1,
        CLUSTER_NODE_QUORUM - 1,
        CLUSTER_NODE_QUORUM,
        INITIAL_CLUSTER_SIZE_MAX,
    ];

    let mut rows: Vec<Row> = Vec::new();
    for (index, nodes) in node_counts.iter().copied().enumerate() {
        // The least a cluster can be stored from, at every node count.
        let shipping_option = SHIPPING_OPTION_VALUES[index % SHIPPING_OPTION_VALUES.len()];
        let mut minimal = required_members_only(address_id, shipping_option);
        minimal["InitialClusterSize"] = json!(nodes);
        rows.push(Row {
            case: format!(
                "a cluster stored from the required members alone, shipped {shipping_option}, \
                 holding {nodes} node jobs"
            ),
            body: minimal,
            nodes,
        });

        // The same cluster carrying the optional members a cluster keeps.
        let mut some = base_cluster(address_id);
        some["InitialClusterSize"] = json!(nodes);
        rows.push(Row {
            case: format!("a cluster carrying the optional members, holding {nodes} node jobs"),
            body: some,
            nodes,
        });
    }

    // One row per value the modelled shipping catalog admits, so the report is exercised on
    // every value the member can hold rather than on the one a recording carried.
    for (index, option) in SHIPPING_OPTION_VALUES.iter().enumerate() {
        let nodes = index % (CLUSTER_NODE_QUORUM + 1);
        let mut body = base_cluster(address_id);
        body["ShippingOption"] = json!(option);
        body["InitialClusterSize"] = json!(nodes);
        rows.push(Row {
            case: format!("a cluster shipped {option} holding {nodes} node jobs"),
            body,
            nodes,
        });
    }

    // Each published bound of `Description`, and a value that is not ascii, so the member is
    // reported back byte for byte whatever its length and alphabet.
    for (what, description) in [
        (
            "the shortest description the member admits",
            "x".repeat(DESCRIPTION_MIN_LEN),
        ),
        (
            "the longest description the member admits",
            "y".repeat(DESCRIPTION_MAX_LEN),
        ),
        (
            "a non-ascii description",
            "クラスター計画 — Núñez".to_string(),
        ),
    ] {
        let mut body = base_cluster(address_id);
        body["Description"] = json!(description);
        body["InitialClusterSize"] = json!(1);
        rows.push(Row {
            case: format!("a cluster described by {what}"),
            body,
            nodes: 1,
        });
    }

    // The structure members, in each form they take: several buckets and a key range rather
    // than one bucket, a notification naming topics rather than all of them, and each
    // on-device service configuration. A configuration that declares a node count is asked
    // for at exactly that many nodes, since an order that does not meet it is refused as
    // misprovisioned (`cluster_provisioning` is the suite that asserts that rule).
    let structures: Vec<(&str, Value, usize)> = vec![
        (
            "several buckets and a key range",
            json!({
                "Resources": {
                    "S3Resources": [
                        { "BucketArn": "arn:aws:s3:::cluster-described-one" },
                        {
                            "BucketArn": "arn:aws:s3:::cluster-described-two",
                            "KeyRange": { "BeginMarker": "a", "EndMarker": "m" }
                        }
                    ]
                }
            }),
            1,
        ),
        (
            "a notification naming the states it is sent for",
            json!({
                "Notification": {
                    "SnsTopicARN": "arn:aws:sns:us-west-2:123456789012:cluster-described",
                    "JobStatesToNotify": ["New", "InTransitToCustomer"],
                    "NotifyAll": false
                }
            }),
            2,
        ),
        (
            "NFS storage on the device",
            json!({
                "OnDeviceServiceConfiguration": {
                    "NFSOnDeviceService": { "StorageLimit": 50, "StorageUnit": "TB" }
                }
            }),
            1,
        ),
        (
            "Amazon S3 compatible storage on the device",
            json!({
                "OnDeviceServiceConfiguration": {
                    "S3OnDeviceService": {
                        "StorageLimit": 25.0,
                        "StorageUnit": "TB",
                        "ServiceSize": 3,
                        "FaultTolerance": 1
                    }
                }
            }),
            3,
        ),
        (
            "the tape gateway and EKS Anywhere services at once",
            json!({
                "OnDeviceServiceConfiguration": {
                    "TGWOnDeviceService": { "StorageLimit": 10, "StorageUnit": "TB" },
                    "EKSOnDeviceService": {
                        "KubernetesVersion": "1.25",
                        "EKSAnywhereVersion": "0.14.3"
                    }
                }
            }),
            1,
        ),
    ];
    for (what, overrides, nodes) in structures {
        let mut body = base_cluster(address_id);
        for (member, value) in overrides.as_object().expect("the overrides are an object") {
            body[member] = value.clone();
        }
        body["InitialClusterSize"] = json!(nodes);
        rows.push(Row {
            case: format!("a cluster carrying {what}"),
            body,
            nodes,
        });
    }

    // Every member `ClusterMetadata` models at once, forwarding address included, so no
    // member of the shape is left unexercised.
    let mut everything = base_cluster(address_id);
    everything["ForwardingAddressId"] = json!(forwarding_address_id);
    everything["OnDeviceServiceConfiguration"] = json!({
        "NFSOnDeviceService": { "StorageLimit": 1, "StorageUnit": "TB" }
    });
    everything["InitialClusterSize"] = json!(CLUSTER_NODE_QUORUM);
    rows.push(Row {
        case: "a cluster carrying every member ClusterMetadata models".to_string(),
        body: everything,
        nodes: CLUSTER_NODE_QUORUM,
    });

    rows
}

#[tokio::test]
async fn describe_cluster_reports_the_stored_cluster_and_the_state_its_node_jobs_put_it_in() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Described Receiver", "13 Cluster Way", json!({})),
        "the address every described cluster ships to",
    )
    .await;
    let forwarding_address_id = create_address(
        &router,
        deliverable_address("Described Forwarder", "14 Cluster Way", json!({})),
        "the address a described cluster forwards to",
    )
    .await;

    // Every report the table produced, so each one can be read back at the end: a report
    // computed from the cluster the request names cannot be disturbed by a later create.
    let mut reports: BTreeMap<String, (Value, String)> = BTreeMap::new();

    for row in rows(&address_id, &forwarding_address_id) {
        let Row { case, body, nodes } = row;
        let before = now_epoch_seconds();
        let (cluster_id, _) = create_cluster(&router, body.clone(), &case).await;

        // A cluster leaves `AwaitingQuorum` only once it holds the node jobs it ships with.
        let expected_state = if nodes >= CLUSTER_NODE_QUORUM {
            PENDING
        } else {
            AWAITING_QUORUM
        };
        let metadata = described(&router, &cluster_id, expected_state, &case).await;
        assert_reports_what_was_stored(&metadata, &body, &case);

        // The creation instant is the one the cluster was created at, not a value minted per
        // read: it sits in the window the create happened in and does not move between
        // reads.
        let creation_date = epoch_seconds(&metadata, "CreationDate", &case);
        let after = now_epoch_seconds();
        assert!(
            creation_date >= before && creation_date <= after,
            "case {case:?}: expected a CreationDate within [{before}, {after}], got \
             {creation_date}"
        );

        // Reading is not writing: three more reports are the same report.
        for repeat in 0..3 {
            let repeat_case = format!("{case}, read back {repeat}");
            let again = described(&router, &cluster_id, expected_state, &repeat_case).await;
            assert_eq!(
                again, metadata,
                "case {repeat_case:?}: expected a repeated DescribeCluster to report the same \
                 cluster, got {again}"
            );
        }

        reports.insert(cluster_id, (metadata, format!("{case} (read back later)")));
    }

    // Every cluster the table stored still reports exactly what it reported when it was
    // created: the report is computed from the cluster the request names.
    for (cluster_id, (expected, case)) in &reports {
        let state = expected
            .get("ClusterState")
            .and_then(Value::as_str)
            .expect("the stored report named a state");
        let metadata = described(&router, cluster_id, state, case).await;
        assert_eq!(
            &metadata, expected,
            "case {case:?}: expected the cluster to read back unchanged, got {metadata}"
        );
    }
}

#[tokio::test]
async fn describe_cluster_follows_the_cluster_through_the_node_jobs_added_to_it() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Quorum Receiver", "15 Cluster Way", json!({})),
        "the address the growing cluster ships to",
    )
    .await;

    // A cluster provisioned with no node job at all, so every node it reaches its quorum
    // with is added by a `CreateJob` of its own afterwards.
    let case = "a cluster created with no node job";
    let body = base_cluster(&address_id);
    let (cluster_id, entries) = create_cluster(&router, body.clone(), case).await;
    assert!(
        entries.is_empty(),
        "case {case:?}: expected no node job from a create that asked for none, got {entries:?}"
    );
    let initial = described(&router, &cluster_id, AWAITING_QUORUM, case).await;
    assert_reports_what_was_stored(&initial, &body, case);

    for node in 1..=CLUSTER_NODE_QUORUM {
        let node_case = format!("{case}, after node job {node} of {CLUSTER_NODE_QUORUM}");
        ok(
            &router,
            "CreateJob",
            json!({ "ClusterId": cluster_id }),
            &node_case,
        )
        .await;

        // The quorum is what moves the cluster on, so every node before the last leaves it
        // where it was.
        let expected_state = if node >= CLUSTER_NODE_QUORUM {
            PENDING
        } else {
            AWAITING_QUORUM
        };
        // Polling is not what moves it: the state is the same on every read, before and
        // after the node that completes the quorum.
        let mut previous: Option<Value> = None;
        for repeat in 0..3 {
            let repeat_case = format!("{node_case}, read {repeat}");
            let metadata = described(&router, &cluster_id, expected_state, &repeat_case).await;
            // Everything but the state is still the cluster that was created.
            assert_reports_what_was_stored(&metadata, &body, &repeat_case);
            assert_eq!(
                metadata.get("CreationDate"),
                initial.get("CreationDate"),
                "case {repeat_case:?}: expected the creation instant to stand still, got \
                 {metadata}"
            );
            if let Some(previous) = &previous {
                assert_eq!(
                    &metadata, previous,
                    "case {repeat_case:?}: expected a repeated DescribeCluster to report the \
                     same cluster, got {metadata}"
                );
            }
            previous = Some(metadata);
        }
    }
}

#[tokio::test]
async fn a_describe_naming_a_cluster_no_record_answers_to_is_refused_and_stores_nothing() {
    let (router, engine) = router_with_engine();
    let address_id = create_address(
        &router,
        deliverable_address("Known Described Receiver", "16 Cluster Way", json!({})),
        "the address the known cluster ships to",
    )
    .await;

    // A cluster that was stored, so each refused row is measured against a store that holds
    // one rather than against an empty one.
    let setup = "the cluster the refused rows are measured against";
    let mut body = base_cluster(&address_id);
    body["InitialClusterSize"] = json!(1);
    let (cluster_id, _) = create_cluster(&router, body.clone(), setup).await;
    let before = described(&router, &cluster_id, AWAITING_QUORUM, setup).await;
    let clusters_before = stored_cluster_ids(&engine);

    // Every row names a cluster id in the modelled form that no record answers to: ids
    // spanning the hex range, and the neighbour of the stored id that differs from it in a
    // single character.
    let mut unknown: Vec<String> = UNKNOWN_CLUSTER_IDS.iter().map(|id| id.to_string()).collect();
    unknown.push(neighbour_id(&cluster_id));
    for unknown_id in &unknown {
        let case = format!("the cluster id {unknown_id}");
        let (status, parsed) = call_in_region(
            &router,
            SIGNED_REGION,
            "DescribeCluster",
            json!({ "ClusterId": unknown_id }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected DescribeCluster to answer 400, got {status} with {parsed}"
        );
        let wire_type = parsed["__type"].as_str().unwrap_or_default();
        assert!(
            wire_type.contains("InvalidResourceException"),
            "case {case:?}: expected an InvalidResourceException, got {parsed}"
        );
        let message = parsed["message"]
            .as_str()
            .or_else(|| parsed["Message"].as_str())
            .unwrap_or_default();
        assert!(
            message.contains(unknown_id.as_str()),
            "case {case:?}: expected the refusal to name the cluster id it was asked about, \
             got {parsed}"
        );
        assert!(
            !message.contains(cluster_id.as_str()),
            "case {case:?}: expected the refusal to name no other cluster, got {parsed}"
        );
        assert_eq!(
            stored_cluster_ids(&engine),
            clusters_before,
            "case {case:?}: a refused DescribeCluster changed which clusters are stored"
        );
    }

    // And the cluster that was stored still reports exactly what it reported before.
    let after = described(&router, &cluster_id, AWAITING_QUORUM, setup).await;
    assert_eq!(
        after, before,
        "case {setup:?}: expected the stored cluster to be untouched by the refusals, got \
         {after}"
    );
    let described_id = string(&after, "ClusterId", setup);
    assert_eq!(
        described_id, cluster_id,
        "case {setup:?}: expected the report to name the stored cluster, got {after}"
    );
}

/// `id` with its last hex character replaced by another the modelled pattern admits, so the
/// result is a well-formed `ClusterId` that names no stored cluster.
fn neighbour_id(id: &str) -> String {
    let mut characters: Vec<char> = id.chars().collect();
    let last = characters
        .last_mut()
        .expect("a cluster id is not the empty string");
    *last = if *last == '0' { '1' } else { '0' };
    characters.into_iter().collect()
}
