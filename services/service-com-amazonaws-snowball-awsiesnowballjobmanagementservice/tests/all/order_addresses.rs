// SPDX-License-Identifier: CC0-1.0

//! The addresses a Snow order names are resolved against the addresses this service holds,
//! driven end to end over HTTP.
//!
//! One rule, asserted over a TABLE of inputs and against EVERY operation that names an
//! address: an id supplied in `AddressId` or `ForwardingAddressId` has to name an address
//! this service stored, and an order naming one it holds no record of is refused with
//! `InvalidResourceException` ("The specified resource can't be found. Check the
//! information you provided in your last request, and try again") reporting
//! `ResourceType` `address`, rather than stored against an address nobody created. A
//! forwarding address is an address too, so the same rule holds for it, and a request
//! naming more than one unstored id is answered about the first of them.
//!
//! The four operations that name an address — `CreateJob`, `CreateCluster`, `UpdateJob`
//! and `UpdateCluster` — answer the one question, so every row below is driven against
//! all four and the refusal each answers is compared to the same expected body. The rows
//! differ along what the rule generalises over: which of the two members carries the
//! unstored id, how many of them do, whether the other member names a stored address, and
//! the form of the unstored id itself (three distinct values of the modelled `AddressId`
//! pattern). The accepted half of the rule is a suite of its own: an order whose every
//! named id is stored is accepted by all four operations and reads back pointing at those
//! addresses.
//!
//! The harness (`router`, `ok`, `call_in_region`, `create_address`, `deliverable_address`,
//! `complete_order`, `string`, `base_cluster`, `create_cluster`, `job_metadata`,
//! `cluster_metadata`, `merge`) is the one this crate's other suites drive the service
//! with, so every call here goes through the same front door a client uses and nothing
//! reaches into the state struct.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::cluster_updates::{base_cluster, cluster_metadata, create_cluster, job_metadata, merge};
use super::job_lifecycle::{
    call_in_region, complete_order, create_address, deliverable_address, ok, string,
};
use super::order_named_resources::REGION;

/// Three ids in the modelled `AddressId` form — `length 40..40` and
/// `^ADID[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`
/// (`smithy_query(query="shape", shape="AddressId")`) — that name no address this service
/// stored: the ids below are well-formed, so what refuses them is the store holding no
/// such address rather than the member's own constraints.
const UNSTORED_ADDRESS_IDS: [&str; 3] = [
    "ADID00000000-0000-0000-0000-000000000000",
    "ADIDffffffff-ffff-ffff-ffff-ffffffffffff",
    "ADID0123abcd-4567-89ef-0123-456789abcdef",
];

/// The `ResourceType` a refusal about an address reports: the kind of record the id named.
const ADDRESS_RESOURCE_TYPE: &str = "address";

/// What `InvalidResourceException` must say about `address_id`: the wording the model
/// documents for a resource that can't be found, over the id the request supplied.
fn expected_message(address_id: &str) -> String {
    format!(
        "The address ID {address_id} can't be found. Check the information you provided in \
         your last request, and try again."
    )
}

/// One refusal row: the case, the order members that name an address, and the id the
/// refusal must be about.
struct Row {
    case: String,
    fragment: Value,
    refused_id: &'static str,
}

/// The stored addresses and the order records every row is driven against.
struct Held {
    shipping_address_id: String,
    forwarding_address_id: String,
    alternate_address_id: String,
    job_id: String,
    cluster_id: String,
}

impl Held {
    /// Create the addresses an accepted order names, plus the job and the cluster the
    /// update rows are driven against.
    async fn create(router: &MultiServiceRouter) -> Self {
        let shipping_address_id = create_address(
            router,
            deliverable_address("Order Receiver", "1 Resolution Way", json!({})),
            "the address every order in this suite ships to",
        )
        .await;
        let forwarding_address_id = create_address(
            router,
            deliverable_address("Order Forwarder", "2 Resolution Way", json!({})),
            "the forwarding address an accepted order names",
        )
        .await;
        let alternate_address_id = create_address(
            router,
            deliverable_address(
                "Alternate Order Receiver",
                "3 Resolution Way",
                json!({ "City": "Portland", "StateOrProvince": "OR", "PostalCode": "97201" }),
            ),
            "the address an accepted update moves an order to",
        )
        .await;

        let mut held = Self {
            shipping_address_id,
            forwarding_address_id,
            alternate_address_id,
            job_id: String::new(),
            cluster_id: String::new(),
        };
        let case = "the job every update row is driven against";
        let created = ok(router, "CreateJob", held.job_order(&json!({})), case).await;
        held.job_id = string(&created, "JobId", case);
        let case = "the cluster every update row is driven against";
        let (cluster_id, _) = create_cluster(router, held.cluster_order(&json!({})), case).await;
        held.cluster_id = cluster_id;
        held
    }

    /// A complete `CreateJob` body shipping to the held address, with `fragment` merged
    /// over it so a row states only the members it varies.
    fn job_order(&self, fragment: &Value) -> Value {
        let mut body = complete_order(&json!({ "AddressId": self.shipping_address_id }));
        merge(&mut body, fragment);
        body
    }

    /// The same for `CreateCluster`, in the one job type and device type a cluster
    /// supports.
    fn cluster_order(&self, fragment: &Value) -> Value {
        let mut body = base_cluster(&self.shipping_address_id);
        merge(&mut body, fragment);
        body
    }

    /// An `UpdateJob` body naming the held job and carrying `fragment`.
    fn job_update(&self, fragment: &Value) -> Value {
        let mut body = json!({ "JobId": self.job_id });
        merge(&mut body, fragment);
        body
    }

    /// An `UpdateCluster` body naming the held cluster and carrying `fragment`.
    fn cluster_update(&self, fragment: &Value) -> Value {
        let mut body = json!({ "ClusterId": self.cluster_id });
        merge(&mut body, fragment);
        body
    }

    /// Every operation that names an address, each with the body that carries `fragment`.
    fn calls(&self, fragment: &Value) -> Vec<(&'static str, Value)> {
        vec![
            ("CreateJob", self.job_order(fragment)),
            ("CreateCluster", self.cluster_order(fragment)),
            ("UpdateJob", self.job_update(fragment)),
            ("UpdateCluster", self.cluster_update(fragment)),
        ]
    }

    /// The refusal rows: one per way a request can name an address the store does not
    /// hold.
    fn refusal_rows(&self) -> Vec<Row> {
        let mut rows = vec![
            Row {
                case: "a shipping address no record answers to".to_string(),
                fragment: json!({ "AddressId": UNSTORED_ADDRESS_IDS[0] }),
                refused_id: UNSTORED_ADDRESS_IDS[0],
            },
            Row {
                case: "a forwarding address no record answers to".to_string(),
                fragment: json!({ "ForwardingAddressId": UNSTORED_ADDRESS_IDS[1] }),
                refused_id: UNSTORED_ADDRESS_IDS[1],
            },
            Row {
                case: "both members naming no record, answered about the shipping address"
                    .to_string(),
                fragment: json!({
                    "AddressId": UNSTORED_ADDRESS_IDS[0],
                    "ForwardingAddressId": UNSTORED_ADDRESS_IDS[1],
                }),
                refused_id: UNSTORED_ADDRESS_IDS[0],
            },
            Row {
                case: "a stored shipping address beside a forwarding address naming none"
                    .to_string(),
                fragment: json!({
                    "AddressId": self.shipping_address_id,
                    "ForwardingAddressId": UNSTORED_ADDRESS_IDS[2],
                }),
                refused_id: UNSTORED_ADDRESS_IDS[2],
            },
            Row {
                case: "a shipping address naming none beside a stored forwarding address"
                    .to_string(),
                fragment: json!({
                    "AddressId": UNSTORED_ADDRESS_IDS[2],
                    "ForwardingAddressId": self.forwarding_address_id,
                }),
                refused_id: UNSTORED_ADDRESS_IDS[2],
            },
        ];
        // Every well-formed id the store holds no address for is refused the same way,
        // whichever of the two members carried it, so each value gets a row of its own in
        // each member rather than the suite resting on one id.
        for unstored in UNSTORED_ADDRESS_IDS {
            rows.push(Row {
                case: format!("the unstored id {unstored} as the shipping address"),
                fragment: json!({ "AddressId": unstored }),
                refused_id: unstored,
            });
            rows.push(Row {
                case: format!("the unstored id {unstored} as the forwarding address"),
                fragment: json!({ "ForwardingAddressId": unstored }),
                refused_id: unstored,
            });
        }
        rows
    }
}

/// A Snowball call that must be refused with HTTP 400, answering the parsed body.
async fn refused_body(
    router: &MultiServiceRouter,
    operation: &str,
    body: Value,
    case: &str,
) -> Value {
    let (status, parsed) = call_in_region(router, REGION, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected {operation} to answer 400, got {status} with {parsed}"
    );
    parsed
}

/// The `Message` an error body carries, whichever casing the protocol put it under.
fn message(body: &Value) -> String {
    body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// How many jobs `ListJobs` reports, so a refused create can be shown to have stored
/// nothing.
async fn job_count(router: &MultiServiceRouter, case: &str) -> usize {
    let listed = ok(router, "ListJobs", json!({}), case).await;
    listed["JobListEntries"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

/// How many clusters `ListClusters` reports, for the same reason.
async fn cluster_count(router: &MultiServiceRouter, case: &str) -> usize {
    let listed = ok(router, "ListClusters", json!({}), case).await;
    listed["ClusterListEntries"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

#[tokio::test]
async fn an_order_naming_an_address_no_record_answers_to_is_refused_by_every_operation() {
    let router = super::job_lifecycle::router();
    let held = Held::create(&router).await;

    // What the held records look like before any refused call: a refusal must leave both
    // exactly as they are.
    let job_before = job_metadata(&router, &held.job_id, "the held job before any refusal").await;
    let cluster_before = cluster_metadata(
        &router,
        &held.cluster_id,
        "the held cluster before any refusal",
    )
    .await;
    let jobs_before = job_count(&router, "the jobs held before any refusal").await;
    let clusters_before = cluster_count(&router, "the clusters held before any refusal").await;

    for row in held.refusal_rows() {
        for (operation, body) in held.calls(&row.fragment) {
            let case = format!("{operation}: {}", row.case);
            let refusal = refused_body(&router, operation, body, &case).await;
            let wire_type = refusal["__type"].as_str().unwrap_or_default();
            assert!(
                wire_type.contains("InvalidResourceException"),
                "case {case:?}: expected an InvalidResourceException, got {refusal}"
            );
            let reported = message(&refusal);
            let expected = expected_message(row.refused_id);
            assert_eq!(
                reported, expected,
                "case {case:?}: expected the refusal {expected:?}, got {reported:?}"
            );
            let reported_type = refusal["ResourceType"].as_str();
            assert_eq!(
                reported_type,
                Some(ADDRESS_RESOURCE_TYPE),
                "case {case:?}: expected ResourceType {ADDRESS_RESOURCE_TYPE:?}, \
                 got {reported_type:?}"
            );
        }
    }

    // Nothing a refused call named reached the store: no order was minted and neither held
    // record changed.
    let jobs_after = job_count(&router, "the jobs held after every refusal").await;
    assert_eq!(
        jobs_after, jobs_before,
        "expected the refused creates to have stored no job, got {jobs_after} jobs where \
         {jobs_before} were held"
    );
    let clusters_after = cluster_count(&router, "the clusters held after every refusal").await;
    assert_eq!(
        clusters_after, clusters_before,
        "expected the refused creates to have stored no cluster, got {clusters_after} \
         clusters where {clusters_before} were held"
    );
    let job_after = job_metadata(&router, &held.job_id, "the held job after every refusal").await;
    assert_eq!(
        job_after, job_before,
        "expected the refused updates to have left the job as it was, got {job_after}"
    );
    let cluster_after = cluster_metadata(
        &router,
        &held.cluster_id,
        "the held cluster after every refusal",
    )
    .await;
    assert_eq!(
        cluster_after, cluster_before,
        "expected the refused updates to have left the cluster as it was, got {cluster_after}"
    );
}

#[tokio::test]
async fn an_order_naming_stored_addresses_is_accepted_by_every_operation_and_reads_back() {
    let router = super::job_lifecycle::router();
    let held = Held::create(&router).await;

    // A create naming a stored address in each member is accepted, and the order reads
    // back pointing at exactly those addresses.
    let both_stored = json!({
        "AddressId": held.shipping_address_id,
        "ForwardingAddressId": held.forwarding_address_id,
    });
    let case = "a job naming a stored address in each member";
    let created = ok(&router, "CreateJob", held.job_order(&both_stored), case).await;
    let job_id = string(&created, "JobId", case);
    let metadata = job_metadata(&router, &job_id, case).await;
    for (member, expected) in [
        ("AddressId", &held.shipping_address_id),
        ("ForwardingAddressId", &held.forwarding_address_id),
    ] {
        assert_eq!(
            metadata.get(member).and_then(Value::as_str),
            Some(expected.as_str()),
            "case {case:?}: expected {member} to read back as {expected}, got {metadata}"
        );
    }

    let case = "a cluster naming a stored address in each member";
    let (cluster_id, _) = create_cluster(&router, held.cluster_order(&both_stored), case).await;
    let metadata = cluster_metadata(&router, &cluster_id, case).await;
    for (member, expected) in [
        ("AddressId", &held.shipping_address_id),
        ("ForwardingAddressId", &held.forwarding_address_id),
    ] {
        assert_eq!(
            metadata.get(member).and_then(Value::as_str),
            Some(expected.as_str()),
            "case {case:?}: expected {member} to read back as {expected}, got {metadata}"
        );
    }

    // An update naming stored addresses is accepted too, and moves the order to them.
    let moved = json!({
        "AddressId": held.alternate_address_id,
        "ForwardingAddressId": held.shipping_address_id,
    });
    let case = "an update moving the job to stored addresses";
    ok(&router, "UpdateJob", held.job_update(&moved), case).await;
    let metadata = job_metadata(&router, &held.job_id, case).await;
    for (member, expected) in [
        ("AddressId", &held.alternate_address_id),
        ("ForwardingAddressId", &held.shipping_address_id),
    ] {
        assert_eq!(
            metadata.get(member).and_then(Value::as_str),
            Some(expected.as_str()),
            "case {case:?}: expected {member} to read back as {expected}, got {metadata}"
        );
    }

    let case = "an update moving the cluster to stored addresses";
    ok(&router, "UpdateCluster", held.cluster_update(&moved), case).await;
    let metadata = cluster_metadata(&router, &held.cluster_id, case).await;
    for (member, expected) in [
        ("AddressId", &held.alternate_address_id),
        ("ForwardingAddressId", &held.shipping_address_id),
    ] {
        assert_eq!(
            metadata.get(member).and_then(Value::as_str),
            Some(expected.as_str()),
            "case {case:?}: expected {member} to read back as {expected}, got {metadata}"
        );
    }
}

#[tokio::test]
async fn update_cluster_answers_for_the_cluster_it_names_before_the_address() {
    let router = super::job_lifecycle::router();
    let held = Held::create(&router).await;

    // `UpdateCluster` resolves the cluster it was asked to update first: a request that
    // names both a cluster this service holds no record of and an address it holds no
    // record of is answered about the cluster, so sharing the address rule with the other
    // three operations does not reorder this one's refusals.
    let case = "an update naming neither a stored cluster nor a stored address";
    let unknown_cluster_id = "CID00000000-0000-0000-0000-000000000000";
    let refusal = refused_body(
        &router,
        "UpdateCluster",
        json!({
            "ClusterId": unknown_cluster_id,
            "AddressId": UNSTORED_ADDRESS_IDS[0],
        }),
        case,
    )
    .await;
    let reported_type = refusal["ResourceType"].as_str();
    assert_eq!(
        reported_type,
        Some("cluster"),
        "case {case:?}: expected ResourceType \"cluster\", got {reported_type:?} in {refusal}"
    );
    let reported = message(&refusal);
    assert!(
        reported.contains(unknown_cluster_id),
        "case {case:?}: expected the refusal to name {unknown_cluster_id:?}, got {reported:?}"
    );

    // And the address rule is still what answers a request whose cluster is held.
    let case = "an update naming a stored cluster and an unstored address";
    let refusal = refused_body(
        &router,
        "UpdateCluster",
        held.cluster_update(&json!({ "AddressId": UNSTORED_ADDRESS_IDS[0] })),
        case,
    )
    .await;
    let reported = message(&refusal);
    let expected = expected_message(UNSTORED_ADDRESS_IDS[0]);
    assert_eq!(
        reported, expected,
        "case {case:?}: expected the refusal {expected:?}, got {reported:?}"
    );
}
