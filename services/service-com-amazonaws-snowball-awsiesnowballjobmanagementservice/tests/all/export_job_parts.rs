// SPDX-License-Identifier: CC0-1.0

//! The structure a `CreateJob` request produces, driven end to end over HTTP.
//!
//! One rule, asserted over a TABLE of creates rather than over the one the issue
//! recorded: a successful request to create an EXPORT job produces a MAIN job, which is
//! associated with no device and is reported by `ListJobs` with `IsMaster=true`, plus at
//! least one job part per device, each of which is reported with `IsMaster=false` and
//! appears in the main job's `SubJobMetadata`; a request for any other job type produces
//! exactly one plain job, with `IsMaster=false` and no `SubJobMetadata` member at all
//! (`JobListEntry$IsMaster`, `DescribeJobResult$SubJobMetadata`).
//!
//! The rows differ along what the rule generalises over: the `JobType` from the modelled
//! catalog including a create that names none, the device type, the resources the order
//! covers, and the number of orders the store holds.
//!
//! The harness (`router`, `ok`, `create_address`, `deliverable_address`, `string`,
//! `assert_prefixed_uuid`) is the one `job_lifecycle` already drives the service with, so
//! every call here goes through the same front door a client uses.

use std::collections::BTreeSet;

use serde_json::{json, Value};
// The job type that mints an order and how many parts an order is split into, read from
// the handler rather than restated, so a test cannot drift from the rule it asserts.
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::create_job::{
    EXPORT_JOB_TYPE, JOB_PARTS_PER_EXPORT_JOB,
};
use smithy_server_runtime::MultiServiceRouter;

use super::job_lifecycle::{
    assert_prefixed_uuid, create_address, deliverable_address, ok, router, string,
};

/// `IsMaster` is modelled with a `false` default, which the protocol may leave off the
/// wire, so an absent member reads as `false`.
fn is_master(entry: &Value) -> bool {
    entry.get("IsMaster").and_then(Value::as_bool).unwrap_or(false)
}

/// The job parts `DescribeJob` reported, which is an empty list when it reported no
/// `SubJobMetadata` member.
fn reported_parts(described: &Value) -> Vec<Value> {
    described
        .get("SubJobMetadata")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Assert `described` carries no `SubJobMetadata` member at all.
fn assert_no_parts(described: &Value, case: &str) {
    let reported = described.get("SubJobMetadata").unwrap_or(&Value::Null);
    assert!(
        reported.is_null(),
        "case {case:?}: expected no SubJobMetadata on a job that is not an export \
         order's main job, got {reported}"
    );
}

/// Every `JobListEntry` the store holds. `MaxResults` is modelled `range 0..100` and
/// every case here stores far fewer jobs than that, so one page is the whole store.
async fn list_all(router: &MultiServiceRouter, case: &str) -> Vec<Value> {
    let listed = ok(router, "ListJobs", json!({ "MaxResults": 100 }), case).await;
    assert!(
        listed.get("NextToken").unwrap_or(&Value::Null).is_null(),
        "case {case:?}: expected every job on one page, got {listed}"
    );
    listed["JobListEntries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected JobListEntries, got {listed}"))
        .clone()
}

/// The entry `ListJobs` reported for `job_id`.
fn entry_for<'a>(entries: &'a [Value], job_id: &str, case: &str) -> &'a Value {
    entries
        .iter()
        .find(|entry| entry.get("JobId").and_then(Value::as_str) == Some(job_id))
        .unwrap_or_else(|| panic!("case {case:?}: ListJobs did not report {job_id}: {entries:?}"))
}

/// Create an order on `address_id` and return the `JobId` the service answered with.
async fn create_order(
    router: &MultiServiceRouter,
    address_id: &str,
    submitted: &Value,
    case: &str,
) -> String {
    let mut body = submitted.clone();
    body["AddressId"] = json!(address_id);
    let created = ok(router, "CreateJob", body, case).await;
    string(&created, "JobId", case)
}

/// An address every order in this suite ships to.
async fn export_address(router: &MultiServiceRouter) -> String {
    create_address(
        router,
        deliverable_address("Export Receiver", "9 Export Way", json!({})),
        "the orders' shipping address",
    )
    .await
}

/// Every create row: one per `JobType` the modelled catalog admits plus a create that
/// names none, differing in device type and in the resources the order covers. The
/// `EXPORT` + `EDGE_S` row with one bucket is the one the issue recorded.
fn order_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "an export order over one bucket",
            json!({
                "JobType": "EXPORT",
                "SnowballType": "EDGE_S",
                "ShippingOption": "STANDARD",
                "RoleARN": "arn:aws:iam::123456789012:role/snowball",
                "Resources": {
                    "S3Resources": [{ "BucketArn": "arn:aws:s3:::export-src" }]
                },
            }),
        ),
        (
            "an export order over several buckets with key ranges",
            json!({
                "JobType": "EXPORT",
                "SnowballType": "EDGE",
                "ShippingOption": "EXPRESS",
                "Description": "export two archives",
                "Resources": {
                    "S3Resources": [
                        {
                            "BucketArn": "arn:aws:s3:::export-first",
                            "KeyRange": { "BeginMarker": "a/", "EndMarker": "m/" }
                        },
                        { "BucketArn": "arn:aws:s3:::export-second" }
                    ]
                },
            }),
        ),
        (
            "an export order naming no resources at all",
            json!({ "JobType": "EXPORT" }),
        ),
        (
            "an import order over one bucket",
            json!({
                "JobType": "IMPORT",
                "SnowballType": "EDGE",
                "ShippingOption": "SECOND_DAY",
                "Resources": {
                    "S3Resources": [{ "BucketArn": "arn:aws:s3:::import-dst" }]
                },
            }),
        ),
        (
            "a local-use order",
            json!({
                "JobType": "LOCAL_USE",
                "SnowballType": "SNC1_SSD",
                "ShippingOption": "NEXT_DAY",
            }),
        ),
        (
            "an order that names no job type",
            json!({ "SnowballType": "EDGE_C" }),
        ),
    ]
}

#[tokio::test]
async fn an_export_create_mints_a_main_job_and_its_job_parts_and_nothing_else_does() {
    let router = router();
    let address_id = export_address(&router).await;
    // Every id this suite sees, so no record is ever reported twice under one id.
    let mut minted: BTreeSet<String> = BTreeSet::new();

    for (case, submitted) in order_rows() {
        let job_type = submitted
            .get("JobType")
            .and_then(Value::as_str)
            .map(str::to_string);
        let is_export = job_type.as_deref() == Some(EXPORT_JOB_TYPE);

        let job_id = create_order(&router, &address_id, &submitted, case).await;
        // A main job's id carries the `MID` form of the modelled `JobId` pattern
        // `^(M|J)ID[0-9a-f]{8}-…$`; a job associated with a device carries the `JID` one.
        let expected_prefix = if is_export { "MID" } else { "JID" };
        assert_prefixed_uuid(&job_id, expected_prefix, case);
        assert!(
            minted.insert(job_id.clone()),
            "case {case:?}: CreateJob answered with {job_id}, an id already in use"
        );

        let described = ok(&router, "DescribeJob", json!({ "JobId": job_id }), case).await;
        let parts = reported_parts(&described);
        if is_export {
            assert_eq!(
                parts.len(),
                JOB_PARTS_PER_EXPORT_JOB,
                "case {case:?}: expected an export order's main job to report its \
                 {JOB_PARTS_PER_EXPORT_JOB} job part(s), got {described}"
            );
        } else {
            assert_no_parts(&described, case);
        }
        // "Main jobs aren't associated with any Snowballs": a main job never names a
        // device, and no appliance has been prepared for any job in `New` either.
        let device = described["JobMetadata"]
            .get("SnowballId")
            .unwrap_or(&Value::Null);
        assert!(
            device.is_null(),
            "case {case:?}: expected a job in New to name no device, got {device}"
        );

        let entries = list_all(&router, case).await;
        let entry = entry_for(&entries, &job_id, case);
        assert_eq!(
            is_master(entry),
            is_export,
            "case {case:?}: expected IsMaster {is_export} for a {} job, got {entry}",
            job_type.as_deref().unwrap_or("untyped")
        );

        for (index, part) in parts.iter().enumerate() {
            let part_case = format!("{case}, job part {index}");
            let part_id = string(part, "JobId", &part_case);
            assert_prefixed_uuid(&part_id, "JID", &part_case);
            assert!(
                minted.insert(part_id.clone()),
                "case {part_case:?}: the job part {part_id} reuses an id already in use"
            );

            // A job part is the device-bearing half of the same order: the same kind of
            // job, in the same state, shipping to the same address, covering the same
            // resources as the order it belongs to.
            assert_eq!(
                part.get("JobType").and_then(Value::as_str),
                job_type.as_deref(),
                "case {part_case:?}: expected the part to be a {job_type:?} job, got {part}"
            );
            assert_eq!(
                part.get("JobState").and_then(Value::as_str),
                Some("New"),
                "case {part_case:?}: expected the part to start in New, got {part}"
            );
            assert_eq!(
                part.get("AddressId").and_then(Value::as_str),
                Some(address_id.as_str()),
                "case {part_case:?}: expected the part to ship to {address_id}, got {part}"
            );
            for member in [
                "Resources",
                "Description",
                "RoleARN",
                "SnowballType",
                "ShippingDetails",
            ] {
                assert_eq!(
                    part.get(member),
                    described["JobMetadata"].get(member),
                    "case {part_case:?}: expected the part to inherit {member} from its \
                     main job, got {part} beside {}",
                    described["JobMetadata"]
                );
            }

            // The part is listed in its own right, and not as a main job.
            let part_entry = entry_for(&entries, &part_id, &part_case);
            assert!(
                !is_master(part_entry),
                "case {part_case:?}: a job part is not a main job, got {part_entry}"
            );

            // And `DescribeJob` on the part reports that part, with no parts of its own.
            let described_part =
                ok(&router, "DescribeJob", json!({ "JobId": part_id }), &part_case).await;
            assert_eq!(
                described_part["JobMetadata"], *part,
                "case {part_case:?}: DescribeJob on the part and the main job's \
                 SubJobMetadata disagree"
            );
            assert_no_parts(&described_part, &part_case);
        }

        // Reading does not change the structure a later caller sees.
        let described_again = ok(&router, "DescribeJob", json!({ "JobId": job_id }), case).await;
        assert_eq!(
            described_again, described,
            "case {case:?}: two DescribeJob calls reported different structures"
        );
        assert_eq!(
            list_all(&router, case).await,
            entries,
            "case {case:?}: reading the jobs changed what ListJobs reports"
        );
    }
}

#[tokio::test]
async fn each_export_order_has_its_own_job_parts_and_lists_them_after_its_main_job() {
    let router = router();
    let address_id = export_address(&router).await;

    // Each row is one more order in the store, so the rule holds for an export order
    // beside other orders rather than only for the first job created.
    let orders: Vec<(&str, Value)> = vec![
        ("the first export order", json!({ "JobType": "EXPORT" })),
        ("an import order between them", json!({ "JobType": "IMPORT" })),
        (
            "the second export order",
            json!({ "JobType": "EXPORT", "SnowballType": "EDGE_S" }),
        ),
    ];

    // The ids `ListJobs` must report, in order: every order's own job followed by that
    // order's job parts.
    let mut expected: Vec<(String, bool)> = Vec::new();
    let mut parts_seen: BTreeSet<String> = BTreeSet::new();
    for (case, submitted) in &orders {
        let job_id = create_order(&router, &address_id, submitted, case).await;
        let described = ok(&router, "DescribeJob", json!({ "JobId": job_id }), case).await;
        let parts = reported_parts(&described);
        expected.push((job_id.clone(), !parts.is_empty()));
        for part in &parts {
            let part_id = string(part, "JobId", case);
            assert!(
                parts_seen.insert(part_id.clone()),
                "case {case:?}: the job part {part_id} is already a part of another order"
            );
            expected.push((part_id, false));
        }
    }

    let listed: Vec<(String, bool)> = list_all(&router, "every order")
        .await
        .iter()
        .map(|entry| (string(entry, "JobId", "an entry"), is_master(entry)))
        .collect();
    assert_eq!(
        listed, expected,
        "expected ListJobs to report every main job, followed by its own job parts, \
         in creation order"
    );
}

#[tokio::test]
async fn cancelling_an_export_order_cancels_its_job_parts() {
    let router = router();
    let address_id = export_address(&router).await;
    let case = "an export order and a plain job beside it";

    let export_id = create_order(
        &router,
        &address_id,
        &json!({ "JobType": "EXPORT", "SnowballType": "EDGE_S" }),
        case,
    )
    .await;
    let import_id = create_order(&router, &address_id, &json!({ "JobType": "IMPORT" }), case).await;

    let described = ok(&router, "DescribeJob", json!({ "JobId": export_id }), case).await;
    let part_ids: Vec<String> = reported_parts(&described)
        .iter()
        .map(|part| string(part, "JobId", case))
        .collect();
    assert!(
        !part_ids.is_empty(),
        "case {case:?}: expected the export order to have job parts, got {described}"
    );

    ok(&router, "CancelJob", json!({ "JobId": export_id }), case).await;

    // The order is what was cancelled: its main job and every part of it. The job beside
    // it belongs to another order and is untouched.
    for cancelled_id in std::iter::once(&export_id).chain(part_ids.iter()) {
        let reported = ok(
            &router,
            "DescribeJob",
            json!({ "JobId": cancelled_id }),
            case,
        )
        .await;
        assert_eq!(
            reported["JobMetadata"]
                .get("JobState")
                .and_then(Value::as_str),
            Some("Cancelled"),
            "case {case:?}: expected {cancelled_id} to be Cancelled, got {reported}"
        );
    }
    let untouched = ok(&router, "DescribeJob", json!({ "JobId": import_id }), case).await;
    assert_eq!(
        untouched["JobMetadata"]
            .get("JobState")
            .and_then(Value::as_str),
        Some("New"),
        "case {case:?}: expected the order beside the cancelled one to stay New, \
         got {untouched}"
    );
}
