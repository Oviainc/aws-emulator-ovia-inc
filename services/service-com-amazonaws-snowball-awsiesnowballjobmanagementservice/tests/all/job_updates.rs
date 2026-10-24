// SPDX-License-Identifier: CC0-1.0

//! `UpdateJob`, driven end to end over HTTP.
//!
//! One rule, asserted over a TABLE of updates rather than over a single request: while a
//! job's `JobState` is `New`, `UpdateJob` writes every member the request names onto the
//! stored job and leaves every member it omits as it was, so a later `DescribeJob` or
//! `ListJobs` reports the merged job; a request naming a job in any other state is refused
//! with `InvalidJobStateException`, and one naming a job or an address no record answers to
//! with `InvalidResourceException`, in both cases changing nothing.
//!
//! The rows differ along what the rule generalises over: which members the request carries
//! (none, one, several, all of them), the enum value from the modelled catalogs
//! (`SHIPPING_OPTION_VALUES`, `SNOWBALL_CAPACITY_VALUES`, read from the crate rather than
//! restated), the length of a value at each published bound of `Description`, the shape of
//! the structure members, and whether the job named is a plain job, an export order's main
//! job or one of that order's job parts.
//!
//! The harness (`router`, `ok`, `refused`, `create_address`, `deliverable_address`,
//! `string`, `assert_job_metadata_matches`, `ALL_JOB_STATES`) is the one `job_lifecycle`
//! already drives the service with, so every call here goes through the same front door a
//! client uses and nothing reaches into the state struct.

use serde_json::{json, Value};
// The catalogs the modelled enums admit, read from the crate's generated types rather
// than restated, so a row cannot drift from the values the model allows.
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::types::{
    SHIPPING_OPTION_VALUES, SNOWBALL_CAPACITY_VALUES,
};
use smithy_server_runtime::MultiServiceRouter;

use super::job_lifecycle::{
    assert_job_metadata_matches, create_address, deliverable_address, ok, refused, router, string,
    ALL_JOB_STATES,
};

/// `UpdateJobRequest$Description` is modelled `length 1..1024`
/// (`smithy_query(query="operation", operation="UpdateJob")`), so these are the shortest
/// and longest values the member admits.
const DESCRIPTION_MIN_LEN: usize = 1;
const DESCRIPTION_MAX_LEN: usize = 1024;

/// A job id in the modelled `JobId` form that names no stored job.
const UNKNOWN_JOB_ID: &str = "JID00000000-0000-0000-0000-000000000000";
/// A main job's id in the modelled `JobId` form that names no stored job.
const UNKNOWN_MAIN_JOB_ID: &str = "MID00000000-0000-0000-0000-000000000000";
/// An address id in the modelled `AddressId` form that names no stored address.
const UNKNOWN_ADDRESS_ID: &str = "ADID00000000-0000-0000-0000-000000000000";

/// The `JobMetadata` `DescribeJob` reports for `job_id`.
async fn job_metadata(router: &MultiServiceRouter, job_id: &str, case: &str) -> Value {
    let described = ok(router, "DescribeJob", json!({ "JobId": job_id }), case).await;
    described["JobMetadata"].clone()
}

/// The ids of the job parts `DescribeJob` reports for `job_id`, empty for a job with none.
async fn job_part_ids(router: &MultiServiceRouter, job_id: &str, case: &str) -> Vec<String> {
    let described = ok(router, "DescribeJob", json!({ "JobId": job_id }), case).await;
    described
        .get("SubJobMetadata")
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .map(|part| string(part, "JobId", case))
                .collect()
        })
        .unwrap_or_default()
}

/// Merge `update` over `expected`, which is what applying the update to the stored job
/// must do: a named member replaces what was stored, an omitted one leaves it alone.
fn merge(expected: &mut Value, update: &Value) {
    for (member, value) in update.as_object().expect("the update is a JSON object") {
        expected[member] = value.clone();
    }
}

/// The body of an `UpdateJob` call: the update's members plus the job it names.
fn update_body(job_id: &str, update: &Value) -> Value {
    let mut body = update.clone();
    body["JobId"] = json!(job_id);
    body
}

/// The create every suite below starts from: a job carrying a value for each member an
/// update can replace, so a row that names one member proves the others were left alone.
fn base_job(address_id: &str) -> Value {
    json!({
        "JobType": "IMPORT",
        "SnowballType": "EDGE",
        "ShippingOption": "SECOND_DAY",
        "SnowballCapacityPreference": "T80",
        "Description": "the plan the job was created with",
        "RoleARN": "arn:aws:iam::123456789012:role/snowball-created",
        "Resources": { "S3Resources": [{ "BucketArn": "arn:aws:s3:::created-bucket" }] },
        "AddressId": address_id,
    })
}

/// Every update row: one per member `UpdateJobRequest` models, one per value the modelled
/// `ShippingOption` and `SnowballCapacity` catalogs admit, one at each published bound of
/// `Description`, one carrying every updatable member at once, and one carrying none.
fn update_rows(address_id: &str, alternate_address_id: &str) -> Vec<(String, Value)> {
    let mut rows: Vec<(String, Value)> = vec![
        ("the job id alone, naming no member to change".to_string(), json!({})),
        (
            "the description alone".to_string(),
            json!({ "Description": "re-planned transfer" }),
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
            json!({ "Description": "輸送計画の更新 — Núñez" }),
        ),
        (
            "a different shipping address".to_string(),
            json!({ "AddressId": alternate_address_id }),
        ),
        (
            "back to the address the job was created with".to_string(),
            json!({ "AddressId": address_id }),
        ),
        (
            "a forwarding address".to_string(),
            json!({ "ForwardingAddressId": alternate_address_id }),
        ),
        (
            "a new role".to_string(),
            json!({ "RoleARN": "arn:aws:iam::123456789012:role/snowball-updated" }),
        ),
        (
            "resources over one bucket with a key range".to_string(),
            json!({
                "Resources": {
                    "S3Resources": [{
                        "BucketArn": "arn:aws:s3:::updated-bucket",
                        "KeyRange": { "BeginMarker": "b/", "EndMarker": "n/" }
                    }]
                }
            }),
        ),
        (
            "resources over several buckets".to_string(),
            json!({
                "Resources": {
                    "S3Resources": [
                        { "BucketArn": "arn:aws:s3:::updated-first" },
                        { "BucketArn": "arn:aws:s3:::updated-second" }
                    ]
                }
            }),
        ),
        (
            "a notification over a new topic".to_string(),
            json!({
                "Notification": {
                    "SnsTopicARN": "arn:aws:sns:us-west-2:123456789012:snowball-updates",
                    "JobStatesToNotify": ["InProgress", "Complete"],
                    "NotifyAll": false
                }
            }),
        ),
        (
            "a notification for every state".to_string(),
            json!({
                "Notification": {
                    "SnsTopicARN": "arn:aws:sns:us-west-2:123456789012:snowball-all",
                    "NotifyAll": true
                }
            }),
        ),
        (
            "on-device services".to_string(),
            json!({
                "OnDeviceServiceConfiguration": {
                    "NFSOnDeviceService": { "StorageLimit": 25, "StorageUnit": "TB" }
                }
            }),
        ),
        (
            "pickup details".to_string(),
            json!({
                "PickupDetails": {
                    "Name": "New Pickup Person",
                    "PhoneNumber": "+1 206 555 0199",
                    "Email": "new-pickup@example.com"
                }
            }),
        ),
        (
            "every updatable member at once".to_string(),
            json!({
                "AddressId": alternate_address_id,
                "ForwardingAddressId": address_id,
                "Description": "everything at once",
                "RoleARN": "arn:aws:iam::123456789012:role/snowball-everything",
                "ShippingOption": "EXPRESS",
                "SnowballCapacityPreference": "T100",
                "Resources": {
                    "S3Resources": [{ "BucketArn": "arn:aws:s3:::everything-bucket" }]
                },
                "Notification": {
                    "SnsTopicARN": "arn:aws:sns:us-west-2:123456789012:snowball-everything",
                    "NotifyAll": true
                },
                "OnDeviceServiceConfiguration": {
                    "S3OnDeviceService": { "StorageLimit": 1.0, "StorageUnit": "TB" }
                },
                "PickupDetails": {
                    "Name": "Everything Pickup",
                    "PhoneNumber": "+1 206 555 0111"
                },
            }),
        ),
    ];
    // One row per value each modelled catalog admits, so the rule is asserted over the
    // whole enum rather than over the value one request happened to carry.
    for option in SHIPPING_OPTION_VALUES {
        rows.push((
            format!("the shipping speed {option}"),
            json!({ "ShippingOption": option }),
        ));
    }
    for capacity in SNOWBALL_CAPACITY_VALUES {
        rows.push((
            format!("the capacity preference {capacity}"),
            json!({ "SnowballCapacityPreference": capacity }),
        ));
    }
    rows
}

#[test]
fn only_the_state_a_job_is_created_in_is_updatable() {
    let updatable: Vec<&str> = ALL_JOB_STATES
        .iter()
        .filter(|state| state.is_updatable())
        .map(|state| state.as_wire())
        .collect();
    assert_eq!(
        updatable,
        vec!["New"],
        "a job's information can only be updated while its JobState value is New, per \
         UpdateJob's documentation"
    );
}

#[tokio::test]
async fn update_job_writes_the_members_it_names_and_leaves_the_rest_alone() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Update Receiver", "1 Update Way", json!({})),
        "the address the job is created with",
    )
    .await;
    let alternate_address_id = create_address(
        &router,
        deliverable_address(
            "Alternate Receiver",
            "2 Alternate Way",
            json!({ "City": "Portland", "StateOrProvince": "OR", "PostalCode": "97201" }),
        ),
        "the address an update moves the job to",
    )
    .await;

    // What the job must read back as: the create, with each applied update merged over it.
    let mut expected = base_job(&address_id);
    let created = ok(
        &router,
        "CreateJob",
        expected.clone(),
        "the job every row updates",
    )
    .await;
    let job_id = string(&created, "JobId", "the job every row updates");
    let created_metadata = job_metadata(&router, &job_id, "the created job").await;
    let creation_date = created_metadata
        .get("CreationDate")
        .cloned()
        .unwrap_or(Value::Null);
    assert!(
        !creation_date.is_null(),
        "expected the created job to report a CreationDate, got {created_metadata}"
    );

    for (case, update) in update_rows(&address_id, &alternate_address_id) {
        let answered = ok(&router, "UpdateJob", update_body(&job_id, &update), &case).await;
        assert_eq!(
            answered,
            json!({}),
            "case {case:?}: UpdateJobResult models no member, got {answered}"
        );

        merge(&mut expected, &update);
        let metadata = job_metadata(&router, &job_id, &case).await;
        assert_job_metadata_matches(&metadata, &expected, "New", &case);
        assert_eq!(
            metadata.get("JobId").and_then(Value::as_str),
            Some(job_id.as_str()),
            "case {case:?}: an update must not change the job's id, got {metadata}"
        );
        assert_eq!(
            metadata.get("CreationDate"),
            Some(&creation_date),
            "case {case:?}: an update must not change the instant the job was created, \
             got {metadata}"
        );
        // Reading does not change what the next caller sees.
        let again = job_metadata(&router, &job_id, &case).await;
        assert_eq!(
            again, metadata,
            "case {case:?}: two DescribeJob calls disagreed after the update"
        );
    }

    // The other reader reports the persisted job too, so what was written is state and not
    // one reader's view of a request.
    let case = "the job the last row left behind";
    let listed = ok(&router, "ListJobs", json!({ "MaxResults": 100 }), case).await;
    let entries = listed["JobListEntries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected JobListEntries, got {listed}"));
    let entry = entries
        .iter()
        .find(|entry| entry.get("JobId").and_then(Value::as_str) == Some(job_id.as_str()))
        .unwrap_or_else(|| panic!("case {case:?}: ListJobs did not report {job_id}: {listed}"));
    assert_eq!(
        entry.get("Description"),
        expected.get("Description"),
        "case {case:?}: expected ListJobs to report the updated description, got {entry}"
    );
    assert_eq!(
        entry.get("JobState").and_then(Value::as_str),
        Some("New"),
        "case {case:?}: an update must not move the job's state, got {entry}"
    );
}

#[tokio::test]
async fn update_job_is_refused_once_the_job_has_left_new() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Cancelled Receiver", "3 Cancelled Way", json!({})),
        "the job's address",
    )
    .await;
    let setup = "a job that has left New";
    let created = ok(&router, "CreateJob", base_job(&address_id), setup).await;
    let job_id = string(&created, "JobId", setup);
    // `CancelJob` is the only operation this slice has that moves a job out of `New`.
    ok(&router, "CancelJob", json!({ "JobId": job_id }), setup).await;
    let before = job_metadata(&router, &job_id, setup).await;
    assert_eq!(
        before.get("JobState").and_then(Value::as_str),
        Some("Cancelled"),
        "case {setup:?}: expected the job to have left New, got {before}"
    );

    // Every row the updatable case accepts is refused here, and changes nothing.
    for (case, update) in update_rows(&address_id, &address_id) {
        refused(
            &router,
            "UpdateJob",
            update_body(&job_id, &update),
            "InvalidJobStateException",
            &case,
        )
        .await;
        let after = job_metadata(&router, &job_id, &case).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused UpdateJob changed the job"
        );
    }
}

#[tokio::test]
async fn update_job_refuses_a_resource_no_record_answers_to() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Known Receiver", "4 Known Way", json!({})),
        "a known address",
    )
    .await;
    let setup = "the job the rows name";
    let created = ok(&router, "CreateJob", base_job(&address_id), setup).await;
    let job_id = string(&created, "JobId", setup);
    let before = job_metadata(&router, &job_id, setup).await;

    // Each row names one resource that does not exist: the job itself, in either id form
    // the modelled pattern admits, the shipping address, or the forwarding address.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a job id that was never created",
            json!({ "JobId": UNKNOWN_JOB_ID, "Description": "no such job" }),
        ),
        (
            "a main job id that was never created",
            json!({ "JobId": UNKNOWN_MAIN_JOB_ID, "Description": "no such order" }),
        ),
        (
            "an AddressId that was never created",
            json!({ "JobId": job_id, "AddressId": UNKNOWN_ADDRESS_ID }),
        ),
        (
            "a ForwardingAddressId that was never created",
            json!({ "JobId": job_id, "ForwardingAddressId": UNKNOWN_ADDRESS_ID }),
        ),
        (
            "a known address beside one that was never created",
            json!({
                "JobId": job_id,
                "AddressId": address_id,
                "ForwardingAddressId": UNKNOWN_ADDRESS_ID
            }),
        ),
    ];
    for (case, body) in rows {
        refused(
            &router,
            "UpdateJob",
            body,
            "InvalidResourceException",
            case,
        )
        .await;
        let after = job_metadata(&router, &job_id, case).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused UpdateJob changed the job"
        );
    }
}

#[tokio::test]
async fn updating_an_export_orders_main_job_updates_its_job_parts() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Order Receiver", "5 Order Way", json!({})),
        "the orders' address",
    )
    .await;
    let alternate_address_id = create_address(
        &router,
        deliverable_address("Order Alternate", "6 Order Way", json!({})),
        "the address the update moves the order to",
    )
    .await;

    let case = "an export order and a plain job beside it";
    let mut export_body = base_job(&address_id);
    export_body["JobType"] = json!("EXPORT");
    export_body["SnowballType"] = json!("EDGE_S");
    let export_created = ok(&router, "CreateJob", export_body, case).await;
    let main_job_id = string(&export_created, "JobId", case);
    let part_ids = job_part_ids(&router, &main_job_id, case).await;
    assert!(
        !part_ids.is_empty(),
        "case {case:?}: expected the export order to have job parts, got none"
    );
    let plain_created = ok(&router, "CreateJob", base_job(&address_id), case).await;
    let plain_job_id = string(&plain_created, "JobId", case);
    let plain_before = job_metadata(&router, &plain_job_id, case).await;

    // A job part carries the order's attributes, so an update to the order reaches them:
    // the members below are exactly those a part inherits from its main job.
    let update = json!({
        "AddressId": alternate_address_id,
        "Description": "the re-planned export",
        "RoleARN": "arn:aws:iam::123456789012:role/snowball-export-updated",
        "ShippingOption": "EXPRESS",
        "Resources": {
            "S3Resources": [{ "BucketArn": "arn:aws:s3:::export-updated" }]
        },
    });
    ok(
        &router,
        "UpdateJob",
        update_body(&main_job_id, &update),
        case,
    )
    .await;

    let main_metadata = job_metadata(&router, &main_job_id, case).await;
    let mut expected = base_job(&address_id);
    expected["JobType"] = json!("EXPORT");
    expected["SnowballType"] = json!("EDGE_S");
    merge(&mut expected, &update);
    assert_job_metadata_matches(&main_metadata, &expected, "New", case);
    for part_id in &part_ids {
        let part_case = format!("{case}, job part {part_id}");
        let part = job_metadata(&router, part_id, &part_case).await;
        for member in [
            "AddressId",
            "Description",
            "RoleARN",
            "Resources",
            "ShippingDetails",
        ] {
            assert_eq!(
                part.get(member),
                main_metadata.get(member),
                "case {part_case:?}: expected the part to carry the order's updated \
                 {member}, got {part} beside {main_metadata}"
            );
        }
    }

    // The job beside the order belongs to another order and is untouched.
    let plain_after = job_metadata(&router, &plain_job_id, case).await;
    assert_eq!(
        plain_after, plain_before,
        "case {case:?}: updating one order changed a job of another"
    );

    // And an update that names a job part is the only record it touches: a part holds no
    // parts of its own.
    let part_case = "a job part updated on its own";
    let part_id = &part_ids[0];
    let part_update = json!({ "Description": "this part only" });
    ok(
        &router,
        "UpdateJob",
        update_body(part_id, &part_update),
        part_case,
    )
    .await;
    let part_after = job_metadata(&router, part_id, part_case).await;
    assert_eq!(
        part_after.get("Description"),
        part_update.get("Description"),
        "case {part_case:?}: expected the part's own description to be updated, \
         got {part_after}"
    );
    let main_after = job_metadata(&router, &main_job_id, part_case).await;
    assert_eq!(
        main_after, main_metadata,
        "case {part_case:?}: updating a job part changed its main job"
    );
}
