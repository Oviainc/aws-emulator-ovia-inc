// SPDX-License-Identifier: CC0-1.0

//! What a `CreateJob` that names a `ClusterId` does with the job-level members the request
//! carries, driven end to end over HTTP.
//!
//! One rule, asserted over a TABLE of inputs rather than over a single request: a node
//! job's attributes are the cluster's only for the members the request leaves OUT — "you
//! only need to provide the clusterId value; the other job attributes are inherited from
//! the cluster" — while a member the request does carry is that job's own and is what
//! `DescribeJob` reports for it, and an id the request supplies in `AddressId` or
//! `ForwardingAddressId` has to name a stored address whether or not the job joins a
//! cluster.
//!
//! The rows differ along what the rule generalises over: which members the request carries
//! (none, one, several, every one of them), the enum value from each modelled catalog
//! (`SHIPPING_OPTION_VALUES`, `SNOWBALL_CAPACITY_VALUES`, `REMOTE_MANAGEMENT_VALUES`, read
//! from the crate rather than restated), the length of a value at each published bound of
//! `Description` and a value past it, the form of the structure members, the number of
//! buckets a `Resources` carries, and the number of nodes one cluster holds.
//!
//! The harness (`router`, `ok`, `refused`, `create_address`, `deliverable_address`,
//! `string`, `assert_prefixed_uuid`, `base_cluster`, `create_cluster`, `job_metadata`) is
//! the one `job_lifecycle` and `cluster_updates` already drive the service with, so every
//! call here goes through the same front door a client uses and nothing reaches into the
//! state struct.

use serde_json::{json, Value};
// The catalogs the modelled enums admit and the quorum this service holds a cluster to,
// read from the crate rather than restated, so a row cannot drift from the values the
// model allows or from the bound the handler enforces.
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::types::{
    REMOTE_MANAGEMENT_VALUES, SHIPPING_OPTION_VALUES, SNOWBALL_CAPACITY_VALUES,
};
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::state::CLUSTER_NODE_QUORUM;
use smithy_server_runtime::MultiServiceRouter;

use super::cluster_updates::{base_cluster, create_cluster, job_metadata};
use super::job_lifecycle::{
    assert_prefixed_uuid, complete_order, create_address, deliverable_address, ok, refused, router,
    string,
};

/// `CreateJobRequest$Description` is modelled `length 1..1024`
/// (`smithy_query(query="operation", operation="CreateJob")`), so these are the shortest
/// and longest values the member admits.
const DESCRIPTION_MIN_LEN: usize = 1;
const DESCRIPTION_MAX_LEN: usize = 1024;

/// An address id in the modelled `AddressId` form that names no stored address.
const UNKNOWN_ADDRESS_ID: &str = "ADID00000000-0000-0000-0000-000000000000";

/// The members a cluster carries that a node job of it inherits: every member
/// `CreateClusterRequest` and `CreateJobRequest` both model, as `JobMetadata` names them
/// (`smithy_query(query="operation", operation="CreateCluster")` beside
/// `smithy_query(query="operation", operation="CreateJob")`).
const INHERITED_MEMBERS: &[&str] = &[
    "JobType",
    "SnowballType",
    "ShippingOption",
    "SnowballCapacityPreference",
    "AddressId",
    "ForwardingAddressId",
    "Description",
    "RoleARN",
    "KmsKeyARN",
    "Resources",
    "Notification",
    "TaxDocuments",
    "RemoteManagement",
    "OnDeviceServiceConfiguration",
];

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
/// cluster's value for every member the request left out, and the request's own value for
/// every member it carried.
fn expected_node(cluster: &Value, supplied: &Value) -> Value {
    let mut expected = json!({});
    for member in INHERITED_MEMBERS {
        if let Some(value) = cluster.get(*member) {
            expected[*member] = value.clone();
        }
    }
    for (member, value) in supplied
        .as_object()
        .expect("the supplied members are a JSON object")
    {
        expected[member] = value.clone();
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

/// Assert the node `cluster_id` minted for `supplied` reports the merge of the cluster and
/// the request, as [`expected_node`] computes it.
async fn assert_node_reports_the_merge(
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

/// Every node row: one carrying nothing but the cluster id, one per job-level member the
/// request can carry, one per value each modelled catalog admits, one at each published
/// bound of `Description`, one per form of the structure members, and one carrying every
/// job-level member at once.
fn node_rows(forwarding_address_id: &str, alternate_address_id: &str) -> Vec<(String, Value)> {
    let mut rows: Vec<(String, Value)> = vec![
        (
            "the cluster id alone, carrying no member of its own".to_string(),
            json!({}),
        ),
        (
            "a description of this specific node".to_string(),
            json!({ "Description": "node 1" }),
        ),
        (
            "the shortest description the member admits".to_string(),
            json!({ "Description": "x".repeat(DESCRIPTION_MIN_LEN) }),
        ),
        (
            "the longest description the member admits".to_string(),
            json!({ "Description": "y".repeat(DESCRIPTION_MAX_LEN) }),
        ),
        (
            "a non-ascii description".to_string(),
            json!({ "Description": "ノード計画 — Núñez" }),
        ),
        (
            "the cluster's own description repeated".to_string(),
            json!({ "Description": "the plan the cluster was created with" }),
        ),
        (
            "a notification for every job state of this node".to_string(),
            json!({
                "Notification": {
                    "SnsTopicARN": "arn:aws:sns:us-west-2:123456789012:node-all",
                    "NotifyAll": true
                }
            }),
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
        ),
        (
            "a role and a key of this node's own".to_string(),
            json!({
                "RoleARN": "arn:aws:iam::123456789012:role/snowball-node",
                "KmsKeyARN": "arn:aws:kms:us-west-2:123456789012:key/node-key"
            }),
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
        ),
        (
            "tax documents of this node's own".to_string(),
            json!({ "TaxDocuments": { "IND": { "GSTIN": "29AAPFU0939F1ZV" } } }),
        ),
        (
            "on-device services of this node's own".to_string(),
            json!({
                "OnDeviceServiceConfiguration": {
                    "TGWOnDeviceService": { "StorageLimit": 2, "StorageUnit": "TB" }
                }
            }),
        ),
        (
            "a shipping address of this node's own".to_string(),
            json!({ "AddressId": alternate_address_id }),
        ),
        (
            "a forwarding address of this node's own".to_string(),
            json!({ "ForwardingAddressId": alternate_address_id }),
        ),
        (
            "back to the forwarding address the cluster was created with".to_string(),
            json!({ "ForwardingAddressId": forwarding_address_id }),
        ),
        (
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
        ),
    ];
    // One row per value each modelled catalog admits, so a row cannot be keyed to the one
    // value an example used.
    for option in SHIPPING_OPTION_VALUES {
        rows.push((
            format!("the shipping speed {option} for this node"),
            json!({ "ShippingOption": option }),
        ));
    }
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
    // Every job-level member at once, each differing from what the cluster carries.
    let everything = json!({
        "Description": "every member of this node's own",
        "ShippingOption": SHIPPING_OPTION_VALUES[0],
        "SnowballCapacityPreference": SNOWBALL_CAPACITY_VALUES[0],
        "RemoteManagement": REMOTE_MANAGEMENT_VALUES[0],
        "AddressId": alternate_address_id,
        "ForwardingAddressId": alternate_address_id,
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
        "ImpactLevel": "IL2",
    });
    rows.push(("every job-level member at once".to_string(), everything));
    rows
}

#[tokio::test]
async fn a_node_job_carries_the_members_its_request_supplied_and_inherits_the_rest() {
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
    let alternate_address_id = create_address(
        &router,
        deliverable_address("Node Alternate", "3 Node Way", json!({})),
        "the address a node asks for instead",
    )
    .await;

    let cluster_body = node_cluster(&address_id, &forwarding_address_id);
    for (case, supplied) in node_rows(&forwarding_address_id, &alternate_address_id) {
        // A cluster takes only its quorum of nodes, so each row gets a cluster of its own
        // and the whole table stays inside that bound.
        let (cluster_id, _) = create_cluster(&router, cluster_body.clone(), &case).await;
        assert_node_reports_the_merge(&router, &cluster_body, &cluster_id, &supplied, &case).await;
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
    let bounded_case = "the cluster after the out-of-bound descriptions";
    let listed = ok(
        &router,
        "ListClusterJobs",
        json!({ "ClusterId": bounded_cluster_id }),
        bounded_case,
    )
    .await;
    let entries = listed
        .get("JobListEntries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        entries.is_empty(),
        "case {bounded_case:?}: a refused create added a node job, got {entries:?}"
    );
}

#[tokio::test]
async fn each_node_of_one_cluster_keeps_the_members_its_own_request_supplied() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Quorum Receiver", "4 Node Way", json!({})),
        "the cluster's address",
    )
    .await;
    let forwarding_address_id = create_address(
        &router,
        deliverable_address("Quorum Forwarder", "5 Node Way", json!({})),
        "the cluster's forwarding address",
    )
    .await;
    let cluster_body = node_cluster(&address_id, &forwarding_address_id);
    let setup = "the cluster whose nodes are created one by one";
    let (cluster_id, _) = create_cluster(&router, cluster_body.clone(), setup).await;

    // Each node of the same cluster is created with a description and a notification of
    // its own, except the last, which carries nothing and so inherits the cluster's: a
    // caller has to be able to tell its nodes apart afterwards.
    let mut submitted: Vec<(String, Value)> = Vec::new();
    for node_number in 1..CLUSTER_NODE_QUORUM {
        submitted.push((
            format!("node {node_number} of the cluster"),
            json!({
                "Description": format!("node {node_number}"),
                "Notification": {
                    "SnsTopicARN": format!("arn:aws:sns:us-west-2:123456789012:node-{node_number}"),
                    "NotifyAll": true
                },
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

    // Every node still reports what its own create asked for, after every other node of
    // the cluster was created.
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
    }

    // The descriptions `ListClusterJobs` reports are the ones each node was created with,
    // in creation order.
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
    for (entry, (case, job_id, supplied)) in entries.iter().zip(nodes.iter()) {
        assert_eq!(
            entry.get("JobId").and_then(Value::as_str),
            Some(job_id.as_str()),
            "case {case:?}: expected the listing in creation order, got {entries:?}"
        );
        let expected_description = supplied
            .get("Description")
            .or_else(|| cluster_body.get("Description"))
            .expect("a description from the request or the cluster");
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
        deliverable_address("Refusal Receiver", "6 Node Way", json!({})),
        "the cluster's address",
    )
    .await;
    let forwarding_address_id = create_address(
        &router,
        deliverable_address("Refusal Forwarder", "7 Node Way", json!({})),
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
    assert!(
        entries.is_empty(),
        "case {listing_case:?}: a refused create added a node job, got {entries:?}"
    );

    // The same two members naming stored addresses are accepted, so the refusal is about
    // the record the id names and not about the member being present at all.
    let accepted_case = "both addresses naming stored records";
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
        "case {accepted_case:?}: expected the address the node asked for, got {node}"
    );
    assert_eq!(
        node.get("ForwardingAddressId").and_then(Value::as_str),
        Some(forwarding_address_id.as_str()),
        "case {accepted_case:?}: expected the forwarding address the node asked for, \
         got {node}"
    );
}
