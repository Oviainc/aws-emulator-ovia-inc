// SPDX-License-Identifier: CC0-1.0

//! `UpdateJobShipmentState`, driven end to end over HTTP.
//!
//! One rule, asserted over a TABLE rather than over a single job: a report of a shipment
//! state names the leg of the device's journey that has just finished — "Set to `RECEIVED`
//! when the device arrives at your location. Set to `RETURNED` when you have returned the
//! device to Amazon Web Services" (`UpdateJobShipmentStateRequest$ShipmentState`) — so the
//! call moves the job that holds that device to the `JobState` the finished leg leaves it
//! in, which a later `DescribeJob` and `ListJobs` report and which other operations of this
//! API then answer from. A report the device is not actually on the leg for is refused with
//! `InvalidJobStateException` and moves nothing ("Updates the state when a shipment state
//! changes to a different state"), and a report for a job no record answers to, or for one
//! that holds no device of its own, with `InvalidResourceException`.
//!
//! The rows differ along what the rule generalises over: the `ShipmentState` reported (the
//! WHOLE modelled catalog), the `JobState` the job is in when it is reported (the WHOLE
//! modelled catalog), the job type and device the order was placed for, whether the job is
//! a plain job, one job part of an export order or a cluster's node job, how many times the
//! same leg is reported, and what the job's own id names when no record answers to it.
//!
//! A job this emulator mints is `New` and no operation of this API ships a device, so a row
//! that needs a job already on a leg sets that state in the service's own store before
//! calling (`job_unlock_code::set_job_state`), the store the handler reads. Everything the
//! rule is about — the report, the refusals and the state a later caller reads — still
//! travels over the wire.
//!
//! The call harness (`router_with_engine`, `ok`, `refused`, `string`, `create_address`,
//! `deliverable_address`, `complete_order`, `job_metadata`, `base_cluster`,
//! `create_cluster`) is the one `job_lifecycle`, `cluster_updates` and `job_manifest`
//! already drive the service with.

use serde_json::{json, Value};
// The modelled catalogs, read from the crate rather than restated, so a row cannot drift
// from the values the model admits.
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::types::{
    JOB_STATE_VALUES, SHIPMENT_STATE_VALUES,
};
use smithy_server_runtime::MultiServiceRouter;

use super::cluster_updates::{base_cluster, create_cluster, job_metadata};
use super::job_lifecycle::{
    complete_order, create_address, deliverable_address, ok, refused, router_with_engine, string,
};
use super::job_unlock_code::set_job_state;

/// Each leg of a device's journey a `ShipmentState` reports: the value, the `JobState` the
/// device is on that leg in, and the `JobState` the finished leg leaves the job in.
///
/// Read off what the model documents for each value. `RECEIVED` — "the device arrives at
/// your location" — ends the journey `InTransitToCustomer` names and leaves the device in
/// the caller's hands (`WithCustomer`); `RETURNED` — "you have returned the device to
/// Amazon Web Services" — ends the stay `WithCustomer` names and leaves the device on its
/// way back (`InTransitToAWS`).
const LEGS: &[(&str, &str, &str)] = &[
    ("RECEIVED", "InTransitToCustomer", "WithCustomer"),
    ("RETURNED", "WithCustomer", "InTransitToAWS"),
];

/// A `JobId` in the modelled form that names no stored job.
const UNKNOWN_JOB_ID: &str = "JID00000000-0000-0000-0000-000000000000";
/// A main `JobId` in the modelled form that names no stored job.
const UNKNOWN_MAIN_JOB_ID: &str = "MID00000000-0000-0000-0000-000000000000";

/// Report `shipment_state` for `job_id`. The response models no member, so what the call
/// did is read back from the job afterwards rather than out of the answer.
async fn report(
    router: &MultiServiceRouter,
    job_id: &str,
    shipment_state: &str,
    case: &str,
) -> Value {
    ok(
        router,
        "UpdateJobShipmentState",
        json!({ "JobId": job_id, "ShipmentState": shipment_state }),
        case,
    )
    .await
}

/// The `JobState` `DescribeJob` reports for `job_id`.
async fn described_state(router: &MultiServiceRouter, job_id: &str, case: &str) -> String {
    let metadata = job_metadata(router, job_id, case).await;
    string(&metadata, "JobState", case)
}

/// The `JobState` `ListJobs` reports for `job_id`, so the move is read back through both
/// readers the model documents it on.
async fn listed_state(router: &MultiServiceRouter, job_id: &str, case: &str) -> String {
    let listed = ok(router, "ListJobs", json!({ "MaxResults": 100 }), case).await;
    let entries = listed
        .get("JobListEntries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let entry = entries
        .iter()
        .find(|entry| entry.get("JobId").and_then(Value::as_str) == Some(job_id))
        .unwrap_or_else(|| panic!("case {case:?}: expected ListJobs to report {job_id}"));
    string(entry, "JobState", case)
}

/// Assert both readers report `expected` for `job_id`.
async fn assert_state(
    router: &MultiServiceRouter,
    job_id: &str,
    expected: &str,
    case: &str,
) {
    let described = described_state(router, job_id, case).await;
    assert_eq!(
        described, expected,
        "case {case:?}: expected DescribeJob to report {job_id} in state {expected}, got \
         {described:?}"
    );
    let listed = listed_state(router, job_id, case).await;
    assert_eq!(
        listed, expected,
        "case {case:?}: expected ListJobs to report {job_id} in state {expected}, got {listed:?}"
    );
}

/// Create one job of `submitted`'s shape against a deliverable address and return its
/// `JobId`. The row's members are merged over the harness's order defaults
/// (`job_lifecycle::complete_order`), so a row states only the axis it varies.
async fn create_job(router: &MultiServiceRouter, submitted: Value, case: &str) -> String {
    let address_id = create_address(
        router,
        deliverable_address(case, "123 Main Street", json!({})),
        case,
    )
    .await;
    let mut body = complete_order(&submitted);
    body["AddressId"] = json!(address_id);
    let created = ok(router, "CreateJob", body, case).await;
    string(&created, "JobId", case)
}

/// The job ids `DescribeJob` reports as `job_id`'s job parts — empty for a job with none.
async fn job_parts(router: &MultiServiceRouter, job_id: &str, case: &str) -> Vec<String> {
    let described = ok(router, "DescribeJob", json!({ "JobId": job_id }), case).await;
    described
        .get("SubJobMetadata")
        .and_then(Value::as_array)
        .map(|parts| parts.iter().map(|part| string(part, "JobId", case)).collect())
        .unwrap_or_default()
}

/// Create one node job of a new cluster and return its `JobId`: a job that holds a device
/// of its own while carrying the attributes of an order with four more.
async fn create_cluster_node(router: &MultiServiceRouter, case: &str) -> String {
    let address_id = create_address(
        router,
        deliverable_address(case, "500 Cluster Way", json!({})),
        case,
    )
    .await;
    let (cluster_id, _) = create_cluster(router, base_cluster(&address_id), case).await;
    let created = ok(
        router,
        "CreateJob",
        json!({ "ClusterId": cluster_id }),
        case,
    )
    .await;
    string(&created, "JobId", case)
}

/// Every order row: one per job type and device a device-bearing job is ordered for, so the
/// rule is asserted over the class of jobs that carry a device rather than over one of them.
fn order_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "import job on an EDGE device",
            json!({
                "JobType": "IMPORT",
                "SnowballType": "EDGE",
                "ShippingOption": "SECOND_DAY",
                "Resources": { "S3Resources": [{ "BucketArn": "arn:aws:s3:::shipment-bucket" }] },
            }),
        ),
        (
            "local-use job on a Snowcone SSD shipped next day",
            json!({
                "JobType": "LOCAL_USE",
                "SnowballType": "SNC1_SSD",
                "ShippingOption": "NEXT_DAY",
                "RemoteManagement": "INSTALLED_AUTOSTART",
            }),
        ),
        (
            "an order naming only its job type",
            json!({ "JobType": "IMPORT" }),
        ),
        (
            "export order, whose job parts each carry a device",
            json!({
                "JobType": "EXPORT",
                "SnowballType": "EDGE_S",
                "ShippingOption": "EXPRESS",
                "Resources": { "S3Resources": [{ "BucketArn": "arn:aws:s3:::export-bucket" }] },
            }),
        ),
    ]
}

/// The jobs an order of `submitted`'s shape leaves holding a device: the plain job it
/// minted, or the job parts of an export order.
async fn device_bearing_jobs(
    router: &MultiServiceRouter,
    submitted: Value,
    case: &str,
) -> Vec<String> {
    let job_id = create_job(router, submitted, case).await;
    // An export create mints a MAIN job plus one job part per device; it is the parts that
    // carry a device, so they are what a shipment state is reported for. Every other create
    // mints one plain job, which carries its own device.
    let parts = job_parts(router, &job_id, case).await;
    if parts.is_empty() {
        vec![job_id]
    } else {
        parts
    }
}

#[test]
fn the_legs_below_are_the_whole_modelled_shipment_state_catalog() {
    let reported: Vec<&str> = LEGS.iter().map(|(value, _, _)| *value).collect();
    assert_eq!(
        reported, SHIPMENT_STATE_VALUES,
        "expected one leg per modelled ShipmentState value, got {reported:?} for \
         {SHIPMENT_STATE_VALUES:?}"
    );
    for (value, on_the_leg, after_the_leg) in LEGS {
        for state in [on_the_leg, after_the_leg] {
            assert!(
                JOB_STATE_VALUES.contains(state),
                "case {value:?}: {state} is not a modelled JobState, got {JOB_STATE_VALUES:?}"
            );
        }
    }
}

#[tokio::test]
async fn every_reported_leg_moves_the_job_to_where_it_left_the_device() {
    let (router, engine) = router_with_engine();

    for (case, submitted) in order_rows() {
        for job_id in device_bearing_jobs(&router, submitted, case).await {
            for (reported, on_the_leg, after_the_leg) in LEGS {
                let case = format!("{case}: {job_id} reporting {reported}");
                set_job_state(&engine, &job_id, on_the_leg);
                report(&router, &job_id, reported, &case).await;
                assert_state(&router, &job_id, after_the_leg, &case).await;
            }
        }
    }
}

#[tokio::test]
async fn a_cluster_node_reports_its_own_device_without_moving_the_rest_of_the_order() {
    let (router, engine) = router_with_engine();
    let case = "a node job of a cluster";
    let node = create_cluster_node(&router, case).await;
    let other = create_job(&router, json!({ "JobType": "IMPORT" }), case).await;

    for (reported, on_the_leg, after_the_leg) in LEGS {
        let case = format!("{case} reporting {reported}");
        set_job_state(&engine, &node, on_the_leg);
        report(&router, &node, reported, &case).await;
        assert_state(&router, &node, after_the_leg, &case).await;
        // A device travels on its own: no other job of this store moved with it.
        assert_state(&router, &other, "New", &case).await;
    }
}

#[tokio::test]
async fn the_whole_journey_is_walked_one_leg_at_a_time() {
    let (router, engine) = router_with_engine();
    let case = "one job walked from in transit to returned";
    let job_id = create_job(&router, json!({ "JobType": "IMPORT" }), case).await;

    // The first leg this API reports is the delivery, which starts from the state the device
    // is in transit to the caller in; from there every later state is the API's own doing.
    let (first, on_the_first_leg, _) = LEGS[0];
    set_job_state(&engine, &job_id, on_the_first_leg);
    for (reported, on_the_leg, after_the_leg) in LEGS {
        let case = format!("{case}: reporting {reported}");
        assert_state(&router, &job_id, on_the_leg, &case).await;
        report(&router, &job_id, reported, &case).await;
        assert_state(&router, &job_id, after_the_leg, &case).await;
    }

    // "Updates the state when a shipment state changes to a different state": the journey
    // runs one way, so neither leg can be reported a second time and the job stays where the
    // last report left it.
    let (last, _, after_the_last_leg) = *LEGS.last().expect("the catalog spells at least one leg");
    for reported in [first, last] {
        let case = format!("{case}: reporting {reported} again");
        refused(
            &router,
            "UpdateJobShipmentState",
            json!({ "JobId": job_id, "ShipmentState": reported }),
            "InvalidJobStateException",
            &case,
        )
        .await;
        assert_state(&router, &job_id, after_the_last_leg, &case).await;
    }
}

#[tokio::test]
async fn a_leg_is_reported_only_while_the_device_is_on_it() {
    let (router, engine) = router_with_engine();

    for (reported, on_the_leg, after_the_leg) in LEGS {
        let case = format!("reporting {reported} from every modelled JobState");
        let job_id = create_job(&router, json!({ "JobType": "IMPORT" }), &case).await;

        // The rows are the WHOLE modelled `JobState` catalog, so the rule is asserted over
        // every state a job can be in rather than over the one the report is served from.
        for wire in JOB_STATE_VALUES {
            let case = format!("{case}: in state {wire}");
            set_job_state(&engine, &job_id, wire);
            if wire == on_the_leg {
                report(&router, &job_id, reported, &case).await;
                assert_state(&router, &job_id, after_the_leg, &case).await;
            } else {
                refused(
                    &router,
                    "UpdateJobShipmentState",
                    json!({ "JobId": job_id, "ShipmentState": reported }),
                    "InvalidJobStateException",
                    &case,
                )
                .await;
                // A refused report is not a report: the job is where it was.
                assert_state(&router, &job_id, wire, &case).await;
            }
        }
    }
}

#[tokio::test]
async fn a_job_with_no_device_of_its_own_is_refused_as_an_invalid_resource() {
    let (router, engine) = router_with_engine();

    // An export order: the main job is associated with no device, its job parts are. It is
    // put on each reported leg in turn below, so the refusal can only be about the device it
    // lacks.
    let export_case = "export order whose main job has no device";
    let main_job = create_job(
        &router,
        json!({
            "JobType": "EXPORT",
            "SnowballType": "EDGE",
            "Resources": { "S3Resources": [{ "BucketArn": "arn:aws:s3:::export-bucket" }] },
        }),
        export_case,
    )
    .await;
    assert!(
        !job_parts(&router, &main_job, export_case).await.is_empty(),
        "case {export_case:?}: expected the export create to mint job parts"
    );

    for (reported, on_the_leg, _) in LEGS {
        let rows: Vec<(&str, String)> = vec![
            ("a JID that was never created", UNKNOWN_JOB_ID.to_string()),
            (
                "an MID that was never created",
                UNKNOWN_MAIN_JOB_ID.to_string(),
            ),
            (export_case, main_job.clone()),
        ];
        for (case, job_id) in rows {
            let case = format!("{case}: reporting {reported}");
            assert_eq!(
                job_id.len(),
                39,
                "case {case:?}: {job_id} is not a 39-character JobId"
            );
            if job_id == main_job {
                set_job_state(&engine, &job_id, on_the_leg);
            }
            refused(
                &router,
                "UpdateJobShipmentState",
                json!({ "JobId": job_id, "ShipmentState": reported }),
                "InvalidResourceException",
                &case,
            )
            .await;
        }
    }
}

#[tokio::test]
async fn a_shipment_state_outside_the_modelled_catalog_names_no_leg() {
    let (router, engine) = router_with_engine();
    let case = "a job on the first reported leg";
    let job_id = create_job(&router, json!({ "JobType": "IMPORT" }), case).await;
    let (_, on_the_first_leg, _) = LEGS[0];
    set_job_state(&engine, &job_id, on_the_first_leg);

    // Values just outside the catalog along each axis a caller gets it wrong on: the wrong
    // case, a value of a NEIGHBOURING modelled enum of this service, a plausible carrier
    // word, and an empty value.
    let rows: Vec<(&str, &str)> = vec![
        ("the right value in the wrong case", "received"),
        ("a JobState value, which is not a ShipmentState", "WithCustomer"),
        ("a carrier word the model does not spell", "DELIVERED"),
        ("no value at all", ""),
    ];

    for (case, value) in rows {
        assert!(
            !SHIPMENT_STATE_VALUES.contains(&value),
            "case {case:?}: {value:?} is a modelled ShipmentState, so it is not outside the \
             catalog"
        );
        refused(
            &router,
            "UpdateJobShipmentState",
            json!({ "JobId": job_id, "ShipmentState": value }),
            "ValidationException",
            case,
        )
        .await;
        // A refused report moves nothing: the job is still on the leg it was put on.
        assert_state(&router, &job_id, on_the_first_leg, case).await;
    }
}

#[tokio::test]
async fn the_device_credentials_follow_the_reported_shipment_state() {
    let (router, engine) = router_with_engine();
    let case = "the credentials of a job whose device arrives and is returned";
    let job_id = create_job(&router, json!({ "JobType": "IMPORT" }), case).await;
    let (received, on_the_delivery_leg, _) = LEGS[0];
    set_job_state(&engine, &job_id, on_the_delivery_leg);

    // "The only valid status for calling this API is WithCustomer as the manifest and Unlock
    // code values are used for securing your device and should only be used when you have
    // the device" (`GetJobUnlockCode`): before the device arrives there are no credentials to
    // read, which is what makes the reported arrival observable beyond `DescribeJob`.
    for operation in ["GetJobManifest", "GetJobUnlockCode"] {
        refused(
            &router,
            operation,
            json!({ "JobId": job_id }),
            "InvalidJobStateException",
            &format!("{case}: {operation} before the device arrives"),
        )
        .await;
    }

    report(&router, &job_id, received, case).await;
    for operation in ["GetJobManifest", "GetJobUnlockCode"] {
        ok(
            &router,
            operation,
            json!({ "JobId": job_id }),
            &format!("{case}: {operation} while the caller holds the device"),
        )
        .await;
    }

    let (returned, _, _) = LEGS[1];
    report(&router, &job_id, returned, case).await;
    for operation in ["GetJobManifest", "GetJobUnlockCode"] {
        refused(
            &router,
            operation,
            json!({ "JobId": job_id }),
            "InvalidJobStateException",
            &format!("{case}: {operation} once the device is returned"),
        )
        .await;
    }
}
