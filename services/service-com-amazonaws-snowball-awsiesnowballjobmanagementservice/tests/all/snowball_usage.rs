// SPDX-License-Identifier: CC0-1.0

//! `GetSnowballUsage`, driven end to end over HTTP.
//!
//! One rule, asserted over a TABLE rather than over one store: a `GetSnowballUsage` call
//! reports the account's Snow device service limit — the quota no operation of this API
//! moves ("If you want to increase your service limit, contact Amazon Web Services
//! Support") — together with "the number of Snow devices that this account is currently
//! using", which is one device per job this service persisted that has a device of its own
//! and whose `JobState` has not released it. An export order's main job has no device of
//! its own ("Job parts are associated with Snowballs … the main job isn't",
//! `JobListEntry$IsMaster`), a `Cancelled` order never had an appliance prepared for it
//! ("You can only cancel a job before its `JobState` value changes to
//! `PreparingAppliance`", `CancelJob`), and a `Complete` order has finished with its device
//! back at Amazon Web Services; every other persisted job holds one.
//!
//! The rows differ along what the rule generalises over: the shape of the order (job type,
//! device type, a plain job, an export order's main job and its parts, a cluster's node
//! jobs), how many devices one call provisions, the job's state (the WHOLE modelled
//! `JobState` catalog), and how many jobs the store holds.
//!
//! Reporting only: the suite also asserts the call moves no job and changes no later
//! answer, which is what a usage report must not do.
//!
//! The shipment states this API moves a job through are not served by this emulator yet, so
//! a row that needs a job in a state other than the one `CreateJob` mints sets that state
//! in the service's own store before calling (`job_unlock_code::set_job_state`), the store
//! the handler reads; the `Cancelled` row needs no seeding, since `CancelJob` is served and
//! drives it over the wire. Everything the rule is about still travels over HTTP.

use serde_json::{json, Value};
// The quota this emulator reports, the export order's shape and the modelled `JobState`
// catalog, read from the crate rather than restated, so a case cannot drift from the rule
// it asserts.
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::create_job::{
    EXPORT_JOB_TYPE, JOB_PARTS_PER_EXPORT_JOB,
};
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::types::JOB_STATE_VALUES;
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::state::SNOWBALL_SERVICE_LIMIT;
use smithy_server_runtime::MultiServiceRouter;

use super::cluster_updates::{base_cluster, create_cluster};
use super::job_lifecycle::{create_address, deliverable_address, ok, string};
use super::job_unlock_code::{router_with_engine, set_job_state};
use super::return_shipping_labels::{create_job, job_parts};

/// The `JobState` values that leave an order holding no Snow device: `Cancelled`, which an
/// order reaches before any appliance is prepared for it ("You can only cancel a job before
/// its `JobState` value changes to `PreparingAppliance`", `CancelJob`), and `Complete`, the
/// state an order has finished in, with its device back at Amazon Web Services and free for
/// another order. Spelled here rather than read from the crate, so the expectation is the
/// rule and not the implementation's own answer.
const STATES_HOLDING_NO_DEVICE: &[&str] = &["Cancelled", "Complete"];

/// A required integer member of a response body.
fn integer(body: &Value, member: &str, case: &str) -> i64 {
    body[member]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?}: expected an integer {member}, got {body}"))
}

/// The usage this service reports: its `SnowballLimit` and its `SnowballsInUse`.
async fn reported_usage(router: &MultiServiceRouter, case: &str) -> (i64, i64) {
    let body = ok(router, "GetSnowballUsage", json!({}), case).await;
    (
        integer(&body, "SnowballLimit", case),
        integer(&body, "SnowballsInUse", case),
    )
}

/// Assert the reported usage is `expected_in_use` devices against the account's limit, which
/// every call reports whatever the store holds.
async fn assert_usage(router: &MultiServiceRouter, expected_in_use: i64, case: &str) {
    let (limit, in_use) = reported_usage(router, case).await;
    assert_eq!(
        limit,
        i64::from(SNOWBALL_SERVICE_LIMIT),
        "case {case:?}: expected the account's service limit {SNOWBALL_SERVICE_LIMIT}, got {limit}"
    );
    assert_eq!(
        in_use, expected_in_use,
        "case {case:?}: expected {expected_in_use} Snow devices in use, got {in_use}"
    );
}

/// Every order row: the devices each shape of `CreateJob` order leaves the account holding.
/// An export order mints a main job plus one job part per device, and it is the parts that
/// carry a device, so the order adds its part count and not the main job.
fn order_rows() -> Vec<(&'static str, Value, i64)> {
    vec![
        (
            "import job on an EDGE device",
            json!({ "JobType": "IMPORT", "SnowballType": "EDGE" }),
            1,
        ),
        (
            "local-use job on a Snowcone SSD",
            json!({
                "JobType": "LOCAL_USE",
                "SnowballType": "SNC1_SSD",
                "ShippingOption": "NEXT_DAY",
                "Resources": json!({}),
            }),
            1,
        ),
        (
            "an order naming only its job type",
            json!({ "JobType": "IMPORT" }),
            1,
        ),
        (
            "export order, whose job parts each carry a device",
            json!({
                "JobType": EXPORT_JOB_TYPE,
                "SnowballType": "EDGE_S",
                "ShippingOption": "EXPRESS",
            }),
            JOB_PARTS_PER_EXPORT_JOB as i64,
        ),
    ]
}

#[tokio::test]
async fn an_account_that_has_ordered_nothing_uses_no_device() {
    let (router, _engine) = router_with_engine();
    assert_usage(&router, 0, "a store holding no job at all").await;
}

#[tokio::test]
async fn every_order_adds_the_devices_it_provisions() {
    let (router, _engine) = router_with_engine();
    let mut expected = 0i64;
    assert_usage(&router, expected, "before any order").await;

    for (case, submitted, devices) in order_rows() {
        let job_id = create_job(&router, submitted, case).await;
        let parts = job_parts(&router, &job_id, case).await;
        // The devices the order really provisioned, as this service reports the order back:
        // an order with job parts carries one device per part and none on the main job.
        let provisioned = if parts.is_empty() {
            1
        } else {
            i64::try_from(parts.len()).expect("a job part count fits an i64")
        };
        assert_eq!(
            provisioned, devices,
            "case {case:?}: expected the order to provision {devices} devices, got \
             {provisioned} ({} job parts)",
            parts.len()
        );
        expected += devices;
        assert_usage(&router, expected, case).await;
    }
}

#[tokio::test]
async fn a_cluster_counts_one_device_per_node_job_it_provisions() {
    let (router, _engine) = router_with_engine();
    let mut expected = 0i64;

    // The rows are the node counts one create provisions: none (an empty cluster, whose
    // nodes are each a `CreateJob` of their own), one, two, and the five-node quorum a
    // cluster ships with.
    for nodes in [0i64, 1, 2, 5] {
        let case = format!("a cluster created with an initial size of {nodes}");
        let address_id = create_address(
            &router,
            deliverable_address(&case, "1 Cluster Way", json!({})),
            &case,
        )
        .await;
        let mut body = base_cluster(&address_id);
        body["InitialClusterSize"] = json!(nodes);
        let (_cluster_id, entries) = create_cluster(&router, body, &case).await;
        let provisioned = i64::try_from(entries.len()).expect("a node count fits an i64");
        assert_eq!(
            provisioned, nodes,
            "case {case:?}: expected the create to report {nodes} node jobs, got {provisioned}"
        );

        expected += nodes;
        assert_usage(&router, expected, &case).await;
    }
}

#[tokio::test]
async fn a_device_counts_while_its_order_holds_it_in_every_modelled_state() {
    let (router, engine) = router_with_engine();

    // One job stays in the state `CreateJob` mints, so the count under test is read beside a
    // device the walk never touches.
    let anchor = "an order left in the state it was created in";
    create_job(&router, json!({ "JobType": "IMPORT" }), anchor).await;
    let walked = "one order walked through every modelled JobState";
    let job_id = create_job(&router, json!({ "JobType": "IMPORT" }), walked).await;
    assert_usage(&router, 2, walked).await;

    for state in STATES_HOLDING_NO_DEVICE {
        assert!(
            JOB_STATE_VALUES.contains(state),
            "expected {state} among the modelled JobState values, got {JOB_STATE_VALUES:?}"
        );
    }

    // The rows are the WHOLE modelled `JobState` catalog, so the rule is asserted over every
    // state a job can be in rather than over the one a create mints.
    for wire in JOB_STATE_VALUES {
        let case = format!("{walked}: in state {wire}");
        set_job_state(&engine, &job_id, wire);
        let holds_a_device = !STATES_HOLDING_NO_DEVICE.contains(wire);
        assert_usage(&router, 1 + i64::from(holds_a_device), &case).await;
    }
}

#[tokio::test]
async fn cancelling_an_order_releases_the_devices_it_held() {
    let (router, _engine) = router_with_engine();

    // Each row is cancelled through the operation that serves it, so nothing about the
    // released device is seeded: a plain order releases its one device, and an export order
    // releases the devices of every job part the cancel takes with it.
    let rows: Vec<(&str, Value, i64)> = vec![
        (
            "a plain import order",
            json!({ "JobType": "IMPORT", "SnowballType": "EDGE" }),
            1,
        ),
        (
            "an export order and its job parts",
            json!({
                "JobType": EXPORT_JOB_TYPE,
                "SnowballType": "EDGE_S",
            }),
            JOB_PARTS_PER_EXPORT_JOB as i64,
        ),
    ];

    let mut expected = 0i64;
    let mut created: Vec<(&str, String, i64)> = Vec::new();
    for (case, submitted, devices) in rows {
        let job_id = create_job(&router, submitted, case).await;
        expected += devices;
        assert_usage(&router, expected, case).await;
        created.push((case, job_id, devices));
    }

    for (case, job_id, devices) in created {
        ok(&router, "CancelJob", json!({ "JobId": job_id }), case).await;
        expected -= devices;
        assert_usage(&router, expected, case).await;
    }
    assert_usage(&router, 0, "every order cancelled").await;
}

#[tokio::test]
async fn reading_the_usage_moves_nothing() {
    let (router, _engine) = router_with_engine();
    let case = "the usage read three times over one order";
    let job_id = create_job(
        &router,
        json!({ "JobType": "IMPORT", "SnowballType": "EDGE" }),
        case,
    )
    .await;

    let before = ok(&router, "DescribeJob", json!({ "JobId": job_id }), case).await;
    let listed_before = ok(&router, "ListJobs", json!({ "MaxResults": 100 }), case).await;
    assert_eq!(
        string(&before["JobMetadata"], "JobId", case),
        job_id,
        "case {case:?}: expected DescribeJob to report the order that was created"
    );

    let first = reported_usage(&router, case).await;
    for attempt in 1..=3 {
        let case = format!("{case}: read {attempt}");
        let again = reported_usage(&router, &case).await;
        assert_eq!(
            again, first,
            "case {case:?}: expected the usage report to be unchanged by reading it, got \
             {again:?} after {first:?}"
        );
    }

    // Reporting the usage is all the call does: the jobs themselves are untouched, so what a
    // later caller reads is unchanged.
    let after = ok(&router, "DescribeJob", json!({ "JobId": job_id }), case).await;
    let listed_after = ok(&router, "ListJobs", json!({ "MaxResults": 100 }), case).await;
    assert_eq!(
        after, before,
        "case {case:?}: GetSnowballUsage moved the job DescribeJob reports"
    );
    assert_eq!(
        listed_after, listed_before,
        "case {case:?}: GetSnowballUsage changed what ListJobs reports"
    );
}
