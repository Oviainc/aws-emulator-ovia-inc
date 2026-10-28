// SPDX-License-Identifier: CC0-1.0

//! What a `CreateJob` that names a `ClusterId` does with the members the request carries,
//! driven end to end over HTTP.
//!
//! One rule, asserted over a TABLE of inputs rather than over a single request: every
//! attribute a cluster's own record holds for the order it ships — the members
//! `ClusterMetadata` reports, ONE value of each for the whole cluster — is carried by every
//! node job of that cluster rather than copied per node ("you only need to provide the
//! clusterId value; the other job attributes are inherited from the cluster"), so a node
//! create either names such an attribute with the value the cluster already holds, which
//! changes nothing, or names a different one and is refused with
//! `InvalidInputCombinationException`, storing nothing. The members a cluster holds no
//! order-wide value for — the capacity a device is configured with and whether it is
//! managed remotely, neither of which `ClusterMetadata` reports — are inherited only where
//! the node's own request named none, and the members only a job models are the node's own
//! throughout. An id the request supplies in `AddressId` or `ForwardingAddressId` has to
//! name a stored address whether or not the job joins a cluster.
//!
//! The rows differ along what the rule generalises over: which members the request carries
//! (none, one, several, every one of them), whether each named attribute agrees with the
//! cluster or differs from it, the enum value from each modelled catalog
//! (`SHIPPING_OPTION_VALUES`, `SNOWBALL_CAPACITY_VALUES`, `REMOTE_MANAGEMENT_VALUES`, read
//! from the crate rather than restated), the length of a value at each published bound of
//! `Description` and a value past it, the form of the structure members, the number of
//! buckets a `Resources` carries, and the number of nodes one cluster holds.
//!
//! The harness (`router`, `ok`, `refused`, `call_in_region`, `create_address`,
//! `deliverable_address`, `string`, `assert_prefixed_uuid`, `base_cluster`,
//! `create_cluster`, `cluster_metadata`, `job_metadata`) is the one `job_lifecycle` and
//! `cluster_updates` already drive the service with, so every call here goes through the
//! same front door a client uses and nothing reaches into the state struct.

use serde_json::{json, Value};
// The catalogs the modelled enums admit and the quorum this service holds a cluster to,
// read from the crate rather than restated, so a row cannot drift from the values the
// model allows or from the bound the handler enforces.
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::types::{
    REMOTE_MANAGEMENT_VALUES, SHIPPING_OPTION_VALUES, SNOWBALL_CAPACITY_VALUES,
};
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::state::CLUSTER_NODE_QUORUM;
use smithy_server_runtime::MultiServiceRouter;

use super::cluster_node_creation_window::listed_node_ids;
use super::cluster_updates::{base_cluster, cluster_metadata, create_cluster, job_metadata};
use super::job_lifecycle::{
    assert_prefixed_uuid, call_in_region, complete_order, create_address, deliverable_address, ok,
    refused, router, string,
};

/// The region every call in this suite is signed for, as the rest of the suites sign them.
const REGION: &str = "us-west-2";

/// `CreateJobRequest$Description` is modelled `length 1..1024`
/// (`smithy_query(query="operation", operation="CreateJob")`), so these are the shortest
/// and longest values the member admits.
const DESCRIPTION_MIN_LEN: usize = 1;
const DESCRIPTION_MAX_LEN: usize = 1024;

/// An address id in the modelled `AddressId` form that names no stored address.
const UNKNOWN_ADDRESS_ID: &str = "ADID00000000-0000-0000-0000-000000000000";

/// The attributes a cluster holds for the whole order it ships: every member
/// `ClusterMetadata` reports (`smithy_query(query="shape", shape="ClusterMetadata")`) that
/// `CreateJobRequest` also models. A node job of the cluster carries the cluster's value
/// for each of them, so a create can only repeat it.
const ORDER_ATTRIBUTES: &[&str] = &[
    "JobType",
    "SnowballType",
    "ShippingOption",
    "AddressId",
    "ForwardingAddressId",
    "Description",
    "RoleARN",
    "KmsKeyARN",
    "Resources",
    "Notification",
    "TaxDocuments",
    "OnDeviceServiceConfiguration",
];

/// The members a cluster keeps for the node jobs it mints but holds no order-wide value
/// for: `ClusterMetadata` reports neither (`smithy_query(query="shape",
/// shape="ClusterMetadata")`), so a node inherits one only where its own request named
/// none.
const NODE_INHERITED_MEMBERS: &[&str] = &["SnowballCapacityPreference", "RemoteManagement"];

/// The members only a job models, which a cluster holds nothing to inherit them from, so a
/// node reports one exactly when the request supplied it.
const JOB_ONLY_MEMBERS: &[&str] = &[
    "LongTermPricingId",
    "DeviceConfiguration",
    "ImpactLevel",
    "PickupDetails",
];

/// A value of the modelled `SnowballCapacity` catalog (`SNOWBALL_CAPACITY_VALUES`) for the
/// cluster to carry, so a row that leaves the member out proves the cluster's value was
/// inherited.
const CLUSTER_CAPACITY: &str = "T100";
/// A value of the modelled `RemoteManagement` catalog (`REMOTE_MANAGEMENT_VALUES`), there
/// for the same reason.
const CLUSTER_REMOTE_MANAGEMENT: &str = "NOT_INSTALLED";

/// The cluster every row below creates its node from: [`base_cluster`] plus the members a
/// cluster keeps only for its node jobs, so a row that omits a member proves the cluster's
/// value was inherited rather than that the member was absent on both sides.
fn node_cluster(address_id: &str, forwarding_address_id: &str) -> Value {
    let mut cluster = base_cluster(address_id);
    cluster["ForwardingAddressId"] = json!(forwarding_address_id);
    cluster["SnowballCapacityPreference"] = json!(CLUSTER_CAPACITY);
    cluster["RemoteManagement"] = json!(CLUSTER_REMOTE_MANAGEMENT);
    cluster["OnDeviceServiceConfiguration"] = json!({
        "NFSOnDeviceService": { "StorageLimit": 1, "StorageUnit": "TB" }
    });
    cluster
}

/// What `DescribeJob` must report for a node created from `cluster` with `supplied`: the
/// cluster's value for every attribute of the order, and the request's own value for a
/// member the cluster holds no order-wide value for.
fn expected_node(cluster: &Value, supplied: &Value) -> Value {
    let mut expected = json!({});
    for member in ORDER_ATTRIBUTES {
        if let Some(value) = cluster.get(*member) {
            expected[*member] = value.clone();
        }
    }
    for member in NODE_INHERITED_MEMBERS {
        if let Some(value) = supplied.get(*member).or_else(|| cluster.get(*member)) {
            expected[*member] = value.clone();
        }
    }
    expected
}

/// The value `DescribeJob` reports for `member`, reading an absent one as null.
///
/// `CreateJob` takes a shipping speed; `JobMetadata` reports it inside `ShippingDetails`,
/// beside the shipments a moving device would have.
fn reported<'a>(node: &'a Value, member: &str) -> &'a Value {
    if member == "ShippingOption" {
        node["ShippingDetails"]
            .get("ShippingOption")
            .unwrap_or(&Value::Null)
    } else {
        node.get(member).unwrap_or(&Value::Null)
    }
}

/// Create a node of `cluster_id` from `supplied` and return the `JobId` minted for it.
async fn create_node(
    router: &MultiServiceRouter,
    cluster_id: &str,
    supplied: &Value,
    case: &str,
) -> String {
    let mut body = supplied.clone();
    body["ClusterId"] = json!(cluster_id);
    let created = ok(router, "CreateJob", body, case).await;
    let job_id = string(&created, "JobId", case);
    assert_prefixed_uuid(&job_id, "JID", case);
    job_id
}

/// Drive a node create that must be refused as a combination that cannot hold, answering
/// the `Message` the refusal carried.
pub(crate) async fn refused_combination(
    router: &MultiServiceRouter,
    cluster_id: &str,
    supplied: &Value,
    case: &str,
) -> String {
    let mut body = supplied.clone();
    body["ClusterId"] = json!(cluster_id);
    let (status, parsed) = call_in_region(router, REGION, "CreateJob", body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a node differing from its cluster to be refused, got \
         {status} with {parsed}"
    );
    let wire_type = parsed["__type"].as_str().unwrap_or_default();
    assert!(
        wire_type.contains("InvalidInputCombinationException"),
        "case {case:?}: expected an InvalidInputCombinationException, got {parsed}"
    );
    parsed["Message"]
        .as_str()
        .or_else(|| parsed["message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// Assert the node `cluster_id` minted for `supplied` reports the cluster's whole order, as
/// [`expected_node`] computes it, plus the members only a job models that the request
/// carried.
async fn assert_node_reports_the_clusters_order(
    router: &MultiServiceRouter,
    cluster_body: &Value,
    cluster_id: &str,
    supplied: &Value,
    case: &str,
) {
    let job_id = create_node(router, cluster_id, supplied, case).await;
    let node = job_metadata(router, &job_id, case).await;
    let expected = expected_node(cluster_body, supplied);
    for (member, value) in expected
        .as_object()
        .expect("the expected node is a JSON object")
    {
        assert_eq!(
            reported(&node, member),
            value,
            "case {case:?}: expected {member} to read back as {value}, got {node}"
        );
    }
    assert_eq!(
        node.get("ClusterId").and_then(Value::as_str),
        Some(cluster_id),
        "case {case:?}: expected the node to name its cluster, got {node}"
    );
    assert_eq!(
        node.get("JobState").and_then(Value::as_str),
        Some("New"),
        "case {case:?}: expected a created node job to be New, got {node}"
    );
    for member in JOB_ONLY_MEMBERS {
        let got = reported(&node, member);
        match supplied.get(*member) {
            Some(value) => assert_eq!(
                got, value,
                "case {case:?}: expected {member} to read back as {value}, got {node}"
            ),
            None => assert!(
                got.is_null(),
                "case {case:?}: a cluster carries no {member}, so a node that asked for \
                 none must report none, got {got}"
            ),
        }
    }
}

/// Every node row a cluster created from `cluster_body` accepts: one carrying nothing but
/// the cluster id, one per attribute of the order stated back as the cluster holds it, one
/// carrying the whole order at once, one per value each catalog of a node's own members
/// admits, one per member only a job models, and one carrying everything a node may ask for.
fn accepted_rows(cluster_body: &Value) -> Vec<(String, Value)> {
    let mut rows: Vec<(String, Value)> = vec![
        (
            "the cluster id alone, carrying no member of its own".to_string(),
            json!({}),
        ),
        (
            "the whole order stated back as the cluster holds it".to_string(),
            cluster_body.clone(),
        ),
    ];
    // One row per attribute of the order, each repeating the cluster's own value: a node
    // create that names one is asking for what it would have inherited anyway.
    for member in ORDER_ATTRIBUTES {
        if let Some(value) = cluster_body.get(*member) {
            let mut row = json!({});
            row[*member] = value.clone();
            rows.push((format!("the cluster's own {member} stated back"), row));
        }
    }
    // One row per value each catalog of a member a node holds for itself admits, so a row
    // cannot be keyed to the one value an example used.
    for capacity in SNOWBALL_CAPACITY_VALUES {
        rows.push((
            format!("the capacity preference {capacity} for this node"),
            json!({ "SnowballCapacityPreference": capacity }),
        ));
    }
    for remote in REMOTE_MANAGEMENT_VALUES {
        rows.push((
            format!("the remote management setting {remote} for this node"),
            json!({ "RemoteManagement": remote }),
        ));
    }
    rows.push((
        "the members only a job models".to_string(),
        json!({
            "ImpactLevel": "IL4",
            "PickupDetails": {
                "Name": "Pickup Person",
                "PhoneNumber": "+1 206 555 0144",
                "Email": "pickup@example.com"
            },
            "DeviceConfiguration": {
                "SnowconeDeviceConfiguration": {
                    "WirelessConnection": { "IsWifiEnabled": true }
                }
            },
        }),
    ));
    // Everything at once: the whole order stated back beside every member that is the
    // node's own.
    let mut everything = cluster_body.clone();
    everything["SnowballCapacityPreference"] = json!(SNOWBALL_CAPACITY_VALUES[0]);
    everything["RemoteManagement"] = json!(REMOTE_MANAGEMENT_VALUES[0]);
    everything["ImpactLevel"] = json!("IL2");
    rows.push((
        "the whole order stated back beside every member of the node's own".to_string(),
        everything,
    ));
    rows
}

/// Every node row that asks for an attribute of the order other than the one its cluster
/// holds, with the member the refusal must be about: the one the request disagrees on, or
/// the first of them where more than one does.
fn refused_rows(
    cluster_body: &Value,
    alternate_address_id: &str,
) -> Vec<(String, Value, Option<&'static str>)> {
    let mut rows: Vec<(String, Value, Option<&'static str>)> = vec![
        (
            "another job type".to_string(),
            json!({ "JobType": "IMPORT" }),
            Some("JobType"),
        ),
        (
            "another device type".to_string(),
            json!({ "SnowballType": "EDGE_C" }),
            Some("SnowballType"),
        ),
        (
            "a shipping address of its own".to_string(),
            json!({ "AddressId": alternate_address_id }),
            Some("AddressId"),
        ),
        (
            "a forwarding address of its own".to_string(),
            json!({ "ForwardingAddressId": alternate_address_id }),
            Some("ForwardingAddressId"),
        ),
        (
            "a description of this specific node".to_string(),
            json!({ "Description": "node 1" }),
            Some("Description"),
        ),
        (
            "the shortest description the member admits".to_string(),
            json!({ "Description": "x".repeat(DESCRIPTION_MIN_LEN) }),
            Some("Description"),
        ),
        (
            "the longest description the member admits".to_string(),
            json!({ "Description": "y".repeat(DESCRIPTION_MAX_LEN) }),
            Some("Description"),
        ),
        (
            "a non-ascii description".to_string(),
            json!({ "Description": "ノード計画 — Núñez" }),
            Some("Description"),
        ),
        (
            "a role of its own".to_string(),
            json!({ "RoleARN": "arn:aws:iam::123456789012:role/snowball-node" }),
            Some("RoleARN"),
        ),
        (
            "a key of its own".to_string(),
            json!({ "KmsKeyARN": "arn:aws:kms:us-west-2:123456789012:key/node-key" }),
            Some("KmsKeyARN"),
        ),
        (
            "resources over one bucket with a key range".to_string(),
            json!({
                "Resources": {
                    "S3Resources": [{
                        "BucketArn": "arn:aws:s3:::node-one",
                        "KeyRange": { "BeginMarker": "a/", "EndMarker": "m/" }
                    }]
                }
            }),
            Some("Resources"),
        ),
        (
            "resources over two buckets and a Lambda association".to_string(),
            json!({
                "Resources": {
                    "S3Resources": [
                        { "BucketArn": "arn:aws:s3:::node-two-first" },
                        { "BucketArn": "arn:aws:s3:::node-two-second" }
                    ],
                    "LambdaResources": [{
                        "LambdaArn": "arn:aws:lambda:us-west-2:123456789012:function:node",
                        "EventTriggers": [{
                            "EventResourceARN": "arn:aws:s3:::node-two-first"
                        }]
                    }]
                }
            }),
            Some("Resources"),
        ),
        (
            "a notification for every job state of this node".to_string(),
            json!({
                "Notification": {
                    "SnsTopicARN": "arn:aws:sns:us-west-2:123456789012:node-all",
                    "NotifyAll": true
                }
            }),
            Some("Notification"),
        ),
        (
            "a notification for two named job states".to_string(),
            json!({
                "Notification": {
                    "SnsTopicARN": "arn:aws:sns:us-west-2:123456789012:node-some",
                    "JobStatesToNotify": ["InTransitToCustomer", "Complete"],
                    "NotifyAll": false
                }
            }),
            Some("Notification"),
        ),
        (
            "tax documents of its own".to_string(),
            json!({ "TaxDocuments": { "IND": { "GSTIN": "29AAPFU0939F1ZV" } } }),
            Some("TaxDocuments"),
        ),
        (
            "on-device services of its own".to_string(),
            json!({
                "OnDeviceServiceConfiguration": {
                    "TGWOnDeviceService": { "StorageLimit": 2, "StorageUnit": "TB" }
                }
            }),
            Some("OnDeviceServiceConfiguration"),
        ),
    ];

    // One row per shipping speed the catalog admits other than the cluster's own, so the
    // rule is exercised across that catalog rather than on one value.
    let held_speed = cluster_body
        .get("ShippingOption")
        .and_then(Value::as_str)
        .expect("the cluster carries a shipping speed");
    for speed in SHIPPING_OPTION_VALUES
        .iter()
        .filter(|speed| **speed != held_speed)
    {
        rows.push((
            format!("the shipping speed {speed} for this node alone"),
            json!({ "ShippingOption": speed }),
            Some("ShippingOption"),
        ));
    }

    // A member the node does hold for itself does not make a differing order attribute
    // acceptable, and neither does repeating the rest of the order correctly.
    rows.push((
        "a description of its own beside the members only a job models".to_string(),
        json!({
            "Description": "node beside its own capacity",
            "SnowballCapacityPreference": SNOWBALL_CAPACITY_VALUES[0],
            "ImpactLevel": "IL2",
        }),
        Some("Description"),
    ));
    let mut whole_order_but_one = cluster_body.clone();
    whole_order_but_one["AddressId"] = json!(alternate_address_id);
    rows.push((
        "the whole order stated back but for the address".to_string(),
        whole_order_but_one,
        Some("AddressId"),
    ));
    rows.push((
        "every attribute of the order differing at once".to_string(),
        json!({
            "JobType": "IMPORT",
            "SnowballType": "EDGE_C",
            "ShippingOption": SHIPPING_OPTION_VALUES
                .iter()
                .find(|speed| **speed != held_speed)
                .expect("a shipping speed other than the cluster's"),
            "AddressId": alternate_address_id,
            "ForwardingAddressId": alternate_address_id,
            "Description": "every attribute of this node's own",
            "RoleARN": "arn:aws:iam::123456789012:role/snowball-every-member",
            "KmsKeyARN": "arn:aws:kms:us-west-2:123456789012:key/every-member",
            "Resources": {
                "S3Resources": [{ "BucketArn": "arn:aws:s3:::node-every-member" }]
            },
            "Notification": {
                "SnsTopicARN": "arn:aws:sns:us-west-2:123456789012:node-every-member",
                "NotifyAll": true
            },
            "TaxDocuments": { "IND": { "GSTIN": "07AAPFU0939F1ZV" } },
            "OnDeviceServiceConfiguration": {
                "NFSOnDeviceService": { "StorageLimit": 4, "StorageUnit": "TB" }
            },
        }),
        // The refusal names the first attribute it finds disagreeing, and the job type is
        // the first one it considers.
        Some("JobType"),
    ));
    rows
}

#[tokio::test]
async fn a_node_job_carries_the_clusters_whole_order_and_its_own_job_level_members() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Node Receiver", "1 Node Way", json!({})),
        "the cluster's address",
    )
    .await;
    let forwarding_address_id = create_address(
        &router,
        deliverable_address("Node Forwarder", "2 Node Way", json!({})),
        "the cluster's forwarding address",
    )
    .await;

    let cluster_body = node_cluster(&address_id, &forwarding_address_id);
    for (case, supplied) in accepted_rows(&cluster_body) {
        // A cluster takes only its quorum of nodes, so each row gets a cluster of its own
        // and the whole table stays inside that bound.
        let (cluster_id, _) = create_cluster(&router, cluster_body.clone(), &case).await;
        assert_node_reports_the_clusters_order(
            &router,
            &cluster_body,
            &cluster_id,
            &supplied,
            &case,
        )
        .await;
    }

    // A description outside the published bounds is not a node description the request can
    // carry at all: it is refused before the handler runs, and no node is created.
    let (bounded_cluster_id, _) = create_cluster(
        &router,
        cluster_body.clone(),
        "the cluster the out-of-bound descriptions name",
    )
    .await;
    for (case, description) in [
        (
            "one below the published minimum description length",
            String::new(),
        ),
        (
            "one past the published maximum description length",
            "z".repeat(DESCRIPTION_MAX_LEN + 1),
        ),
    ] {
        refused(
            &router,
            "CreateJob",
            json!({ "ClusterId": bounded_cluster_id, "Description": description }),
            "ValidationException",
            case,
        )
        .await;
    }
    assert!(
        listed_node_ids(
            &router,
            &bounded_cluster_id,
            "the cluster after the out-of-bound descriptions"
        )
        .await
        .is_empty(),
        "a refused create added a node job to the cluster"
    );

    // The rule holds at each published bound of the one order attribute that has them: a
    // node repeating a description of that length is accepted and inherits it, and one
    // differing from it by a single character is refused.
    for len in [DESCRIPTION_MIN_LEN, DESCRIPTION_MAX_LEN] {
        let case = format!("a cluster described in {len} characters");
        let mut body = cluster_body.clone();
        body["Description"] = json!("d".repeat(len));
        let (cluster_id, _) = create_cluster(&router, body.clone(), &case).await;
        assert_node_reports_the_clusters_order(
            &router,
            &body,
            &cluster_id,
            &json!({ "Description": "d".repeat(len) }),
            &format!("{case}, the node repeating it"),
        )
        .await;
        let differing = format!("{}e", "d".repeat(len - 1));
        refused_combination(
            &router,
            &cluster_id,
            &json!({ "Description": differing }),
            &format!("{case}, a node differing in one character"),
        )
        .await;
    }
}

#[tokio::test]
async fn a_node_asking_for_an_order_attribute_of_its_own_is_refused_and_stores_nothing() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Divergence Receiver", "3 Node Way", json!({})),
        "the cluster's address",
    )
    .await;
    let forwarding_address_id = create_address(
        &router,
        deliverable_address("Divergence Forwarder", "4 Node Way", json!({})),
        "the cluster's forwarding address",
    )
    .await;
    let alternate_address_id = create_address(
        &router,
        deliverable_address("Divergence Alternate", "5 Node Way", json!({})),
        "the address a node asks for instead",
    )
    .await;

    let cluster_body = node_cluster(&address_id, &forwarding_address_id);
    let setup = "the cluster every refused row names";
    let (cluster_id, _) = create_cluster(&router, cluster_body.clone(), setup).await;
    // One node of the cluster, created the way a node is ("you only need to provide the
    // clusterId value"), so each refusal below is shown to leave an existing node alone too.
    let node_id = create_node(&router, &cluster_id, &json!({}), setup).await;
    let cluster_before = cluster_metadata(&router, &cluster_id, setup).await;
    let node_before = job_metadata(&router, &node_id, setup).await;

    for (case, supplied, member) in refused_rows(&cluster_body, &alternate_address_id) {
        let message = refused_combination(&router, &cluster_id, &supplied, &case).await;
        if let Some(member) = member {
            assert!(
                message.contains(member),
                "case {case:?}: expected the refusal to name the {member} it is about, got \
                 {message:?}"
            );
            // The model documents this error's sentence for the job-type/device-type pair,
            // and that sentence names the two request members rather than a cluster; every
            // other attribute is refused naming the cluster whose value it disagrees with.
            if !matches!(member, "JobType" | "SnowballType") {
                assert!(
                    message.contains(&cluster_id),
                    "case {case:?}: expected the refusal to name the cluster {cluster_id}, \
                     got {message:?}"
                );
            }
        }

        // Nothing was stored: the cluster still reports its own order, its node set still
        // holds the one node it held, and that node is as it was created.
        let cluster_after = cluster_metadata(&router, &cluster_id, &case).await;
        assert_eq!(
            cluster_after, cluster_before,
            "case {case:?}: a refused node create changed the cluster, got {cluster_after} \
             after {cluster_before}"
        );
        let listed = listed_node_ids(&router, &cluster_id, &case).await;
        assert_eq!(
            listed,
            vec![node_id.clone()],
            "case {case:?}: a refused node create changed the cluster's node listing"
        );
        let node_after = job_metadata(&router, &node_id, &case).await;
        assert_eq!(
            node_after, node_before,
            "case {case:?}: a refused node create changed the node the cluster held, got \
             {node_after} after {node_before}"
        );
    }
}

#[tokio::test]
async fn each_node_of_one_cluster_reports_the_clusters_order_and_keeps_its_own_members() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Quorum Receiver", "6 Node Way", json!({})),
        "the cluster's address",
    )
    .await;
    let forwarding_address_id = create_address(
        &router,
        deliverable_address("Quorum Forwarder", "7 Node Way", json!({})),
        "the cluster's forwarding address",
    )
    .await;
    let cluster_body = node_cluster(&address_id, &forwarding_address_id);
    let setup = "the cluster whose nodes are created one by one";
    let (cluster_id, _) = create_cluster(&router, cluster_body.clone(), setup).await;

    // Each node of the same cluster is created with a capacity and pickup details of its
    // own — members the cluster holds no order-wide value for — except the last, which
    // carries nothing and so inherits the cluster's: a caller has to be able to tell its
    // nodes apart afterwards.
    let mut submitted: Vec<(String, Value)> = Vec::new();
    for node_number in 1..CLUSTER_NODE_QUORUM {
        submitted.push((
            format!("node {node_number} of the cluster"),
            json!({
                "SnowballCapacityPreference":
                    SNOWBALL_CAPACITY_VALUES[node_number % SNOWBALL_CAPACITY_VALUES.len()],
                "PickupDetails": { "Name": format!("Pickup Person {node_number}") },
            }),
        ));
    }
    submitted.push((
        format!("node {CLUSTER_NODE_QUORUM} of the cluster, carrying nothing of its own"),
        json!({}),
    ));

    let mut nodes: Vec<(String, String, Value)> = Vec::new();
    for (case, supplied) in &submitted {
        let job_id = create_node(&router, &cluster_id, supplied, case).await;
        nodes.push((case.clone(), job_id, supplied.clone()));
    }

    // Every node reports the cluster's whole order and still reports what its own create
    // asked for, after every other node of the cluster was created.
    for (case, job_id, supplied) in &nodes {
        let node = job_metadata(&router, job_id, case).await;
        let expected = expected_node(&cluster_body, supplied);
        for (member, value) in expected
            .as_object()
            .expect("the expected node is a JSON object")
        {
            assert_eq!(
                reported(&node, member),
                value,
                "case {case:?}: expected {member} to read back as {value}, got {node}"
            );
        }
        if let Some(details) = supplied.get("PickupDetails") {
            assert_eq!(
                reported(&node, "PickupDetails"),
                details,
                "case {case:?}: expected the node to keep the pickup details only it has, \
                 got {node}"
            );
        }
    }

    // The descriptions `ListClusterJobs` reports are the cluster's, since the order holds
    // one description for every device it ships, and the entries come in creation order.
    let listing_case = "the cluster's whole node listing";
    let listed = ok(
        &router,
        "ListClusterJobs",
        json!({ "ClusterId": cluster_id }),
        listing_case,
    )
    .await;
    let entries = listed
        .get("JobListEntries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        entries.len(),
        nodes.len(),
        "case {listing_case:?}: expected one entry per node, got {entries:?}"
    );
    let expected_description = cluster_body
        .get("Description")
        .expect("the cluster carries a description");
    for (entry, (case, job_id, _)) in entries.iter().zip(nodes.iter()) {
        assert_eq!(
            entry.get("JobId").and_then(Value::as_str),
            Some(job_id.as_str()),
            "case {case:?}: expected the listing in creation order, got {entries:?}"
        );
        assert_eq!(
            entry.get("Description").unwrap_or(&Value::Null),
            expected_description,
            "case {case:?}: expected the listed description {expected_description}, \
             got {entry}"
        );
    }
}

#[tokio::test]
async fn an_address_a_job_names_has_to_be_stored_whether_or_not_it_joins_a_cluster() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Refusal Receiver", "8 Node Way", json!({})),
        "the cluster's address",
    )
    .await;
    let forwarding_address_id = create_address(
        &router,
        deliverable_address("Refusal Forwarder", "9 Node Way", json!({})),
        "the cluster's forwarding address",
    )
    .await;
    let cluster_body = node_cluster(&address_id, &forwarding_address_id);
    let setup = "the cluster the refused rows name";
    let (cluster_id, _) = create_cluster(&router, cluster_body, setup).await;

    // The member that names the address is what the check belongs to, so each row is
    // refused the same way whether it joins a cluster or orders a device on its own.
    let rows: Vec<(String, Value)> = vec![
        (
            "a shipping address no record answers to".to_string(),
            json!({ "AddressId": UNKNOWN_ADDRESS_ID }),
        ),
        (
            "a forwarding address no record answers to".to_string(),
            json!({ "ForwardingAddressId": UNKNOWN_ADDRESS_ID }),
        ),
        (
            "both addresses naming no record".to_string(),
            json!({
                "AddressId": UNKNOWN_ADDRESS_ID,
                "ForwardingAddressId": UNKNOWN_ADDRESS_ID
            }),
        ),
        (
            "a stored shipping address beside a forwarding address naming none".to_string(),
            json!({
                "AddressId": address_id,
                "ForwardingAddressId": UNKNOWN_ADDRESS_ID
            }),
        ),
    ];
    for (case, row) in &rows {
        let mut node_body = row.clone();
        node_body["ClusterId"] = json!(cluster_id);
        refused(
            &router,
            "CreateJob",
            node_body,
            "InvalidResourceException",
            &format!("{case}, as a node of a cluster"),
        )
        .await;

        let mut own_body = complete_order(row);
        if own_body.get("AddressId").is_none() {
            own_body["AddressId"] = json!(address_id);
        }
        refused(
            &router,
            "CreateJob",
            own_body,
            "InvalidResourceException",
            &format!("{case}, as a job of its own"),
        )
        .await;
    }

    // A refused create stored nothing, so the cluster still holds no node.
    let listing_case = "the cluster after every refused create";
    assert!(
        listed_node_ids(&router, &cluster_id, listing_case)
            .await
            .is_empty(),
        "case {listing_case:?}: a refused create added a node job"
    );

    // The same two members naming the addresses the cluster itself ships to are accepted,
    // so the refusal is about the record the id names and not about the member being
    // present at all.
    let accepted_case = "both addresses naming the cluster's own stored records";
    let job_id = create_node(
        &router,
        &cluster_id,
        &json!({
            "AddressId": address_id,
            "ForwardingAddressId": forwarding_address_id
        }),
        accepted_case,
    )
    .await;
    let node = job_metadata(&router, &job_id, accepted_case).await;
    assert_eq!(
        node.get("AddressId").and_then(Value::as_str),
        Some(address_id.as_str()),
        "case {accepted_case:?}: expected the address the order ships to, got {node}"
    );
    assert_eq!(
        node.get("ForwardingAddressId").and_then(Value::as_str),
        Some(forwarding_address_id.as_str()),
        "case {accepted_case:?}: expected the order's forwarding address, got {node}"
    );
}
