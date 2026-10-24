// SPDX-License-Identifier: CC0-1.0

//! `GetJobManifest`, driven end to end over HTTP.
//!
//! One rule, asserted over a TABLE rather than over a single job: a `GetJobManifest` call
//! for a job this service persisted that holds a device of its own, is in the one state
//! that device is in the caller's hands (`GetJobManifest`: "The manifest is an encrypted
//! file that you can download after your job enters the WithCustomer status. This is the
//! only valid status for calling this API"), and is still inside the window the job's
//! credentials live for ("The credentials of a given job, including its manifest file and
//! unlock code, expire 360 days after the job is created") answers an Amazon S3 URL
//! addressing THAT job's manifest in the Region the request was signed for, carrying a
//! token distinct from every other URL the service issued and an expiry exactly
//! [`JOB_MANIFEST_AVAILABILITY`] after the call ("You can access the manifest file for up
//! to 60 minutes after this request has been made. To access the manifest file after 60
//! minutes have passed, you'll have to make another call to the GetJobManifest action").
//! Every other call is refused: a job no record answers to, the main job of an export
//! order, which "aren't associated with any Snowballs" (`JobListEntry$IsMaster`), and a
//! job past its credential window as `InvalidResourceException`; every other `JobState` as
//! `InvalidJobStateException`.
//!
//! The rows differ along what the rule generalises over: the job type and device the order
//! was placed for, whether the job is a plain job or one job part of an export order, the
//! job's state (the WHOLE modelled `JobState` catalog), the Region the call is signed for
//! (including a scope that names no Region at all), how old the job is relative to the
//! published credential window (both sides of the bound and one well past it), and how
//! many times the same job is asked.
//!
//! The shipment states this API moves a job through are not served by this emulator yet,
//! so a row that needs a job in a state other than the one `CreateJob` mints sets that
//! state in the service's own store before calling (`job_unlock_code::set_job_state`), the
//! store the handler reads. Everything the rule is about still travels over the wire.
//!
//! The call harness (`router_with_engine`, `call_in_region`, `ok`, `refused`, `string`,
//! `create_address`, `deliverable_address`, `complete_order`) is the one `job_lifecycle`
//! already drives the service with, and the URL reader
//! (`software_updates::parse_published_url`) is the one the other published-URL suite
//! uses, so the two assert the same URL shape.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
// The modelled `JobState` catalog, the published windows and the manifest's bucket and
// object, read from the crate rather than restated, so a case cannot drift from the rule
// it asserts.
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::types::JOB_STATE_VALUES;
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::state::{
    DEVICE_CREDENTIAL_VALIDITY, JOB_MANIFEST_AVAILABILITY, JOB_MANIFEST_BUCKET,
    JOB_MANIFEST_OBJECT, SOFTWARE_UPDATE_BUCKET, SOFTWARE_UPDATE_OBJECT,
};
use smithy_server_runtime::MultiServiceRouter;

use super::job_lifecycle::{
    call_in_region, complete_order, create_address, deliverable_address, ok, refused,
    router_with_engine, string,
};
use super::job_unlock_code::{age_job, set_job_state};
use super::software_updates::{now_epoch_seconds, parameter, parse_published_url, DEFAULT_REGION};

/// The one state a job's manifest is released from.
const RELEASING_STATE: &str = "WithCustomer";

/// The query parameter a published manifest URL carries its grant's token in.
const MANIFEST_TOKEN: &str = "manifestToken";

/// Call `GetJobManifest` for `job_id` signed for `region` and assert the URL it answered
/// addresses that job's manifest under `expected_host`, with an expiry
/// [`JOB_MANIFEST_AVAILABILITY`] after the call. Returns the URL's token.
async fn assert_manifest_url(
    router: &MultiServiceRouter,
    region: &str,
    job_id: &str,
    expected_host: &str,
    case: &str,
) -> String {
    let issued_no_earlier_than = now_epoch_seconds();
    let (status, body) =
        call_in_region(router, region, "GetJobManifest", json!({ "JobId": job_id })).await;
    let issued_no_later_than = now_epoch_seconds();
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetJobManifest to answer 200, got {status} with {body}"
    );

    let uri = string(&body, "ManifestURI", case);
    let url = parse_published_url(&uri, case);
    assert_eq!(
        url.host, expected_host,
        "case {case:?}: expected the manifest under {expected_host}, got {uri:?}"
    );
    assert_eq!(
        url.path,
        format!("{job_id}/{JOB_MANIFEST_OBJECT}"),
        "case {case:?}: expected the URL to address {job_id}'s manifest, got {uri:?}"
    );

    let token = parameter(&url, MANIFEST_TOKEN, case);
    assert!(
        token.len() == 64
            && token
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "case {case:?}: expected a 64-character lowercase-hex {MANIFEST_TOKEN}, got {token:?}"
    );

    let expires: u64 = parameter(&url, "Expires", case)
        .parse()
        .unwrap_or_else(|error| {
            panic!("case {case:?}: expected epoch seconds in Expires, got {error} for {uri:?}")
        });
    let window = JOB_MANIFEST_AVAILABILITY.as_secs();
    assert!(
        (issued_no_earlier_than + window..=issued_no_later_than + window).contains(&expires),
        "case {case:?}: expected the URL to expire {window} seconds after the call \
         (between {} and {}), got {expires}",
        issued_no_earlier_than + window,
        issued_no_later_than + window
    );

    token
}

/// The host a manifest URL signed for `region` addresses.
fn manifest_host(region: &str) -> String {
    format!("{JOB_MANIFEST_BUCKET}.s3.{region}.amazonaws.com")
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

/// The ids of the jobs an order of `submitted`'s shape leaves holding a device: the plain
/// job it minted, or the job parts of an export order, each put into the state the
/// manifest is released from.
async fn device_bearing_jobs(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    submitted: Value,
    case: &str,
) -> Vec<String> {
    let job_id = create_job(router, submitted, case).await;
    // An export create mints a MAIN job plus one job part per device; it is the parts that
    // carry a device, so they are what the manifest is asked for. Every other create mints
    // one plain job, which carries its own device.
    let parts = job_parts(router, &job_id, case).await;
    let with_a_device = if parts.is_empty() {
        vec![job_id]
    } else {
        parts
    };
    for job in &with_a_device {
        set_job_state(engine, job, RELEASING_STATE);
    }
    with_a_device
}

/// Every order row: one per job type and device the manifest is asked for, so the rule is
/// asserted over the class of persisted jobs rather than over one of them.
fn order_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "import job on an EDGE device",
            json!({
                "JobType": "IMPORT",
                "SnowballType": "EDGE",
                "ShippingOption": "SECOND_DAY",
                "Resources": { "S3Resources": [{ "BucketArn": "arn:aws:s3:::manifest-bucket" }] },
            }),
        ),
        (
            "local-use job on a Snowcone SSD",
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

#[tokio::test]
async fn get_job_manifest_publishes_a_manifest_url_for_every_job_with_a_device() {
    let (router, engine) = router_with_engine();
    let mut tokens: BTreeSet<String> = BTreeSet::new();
    let mut jobs_asked = 0usize;

    for (case, submitted) in order_rows() {
        for job in device_bearing_jobs(&router, &engine, submitted, case).await {
            let token = assert_manifest_url(
                &router,
                DEFAULT_REGION,
                &job,
                &manifest_host(DEFAULT_REGION),
                case,
            )
            .await;
            assert!(
                tokens.insert(token.clone()),
                "case {case:?}: GetJobManifest issued {token}, which it had issued before"
            );
            jobs_asked += 1;
        }
    }

    assert!(
        jobs_asked >= order_rows().len(),
        "expected at least one device-bearing job per order row, got {jobs_asked} for {} rows",
        order_rows().len()
    );
}

#[tokio::test]
async fn the_manifest_is_released_only_while_the_customer_holds_the_device() {
    let (router, engine) = router_with_engine();
    let case = "one job walked through every modelled JobState";
    let job_id = create_job(&router, json!({ "JobType": "IMPORT" }), case).await;

    // The rows are the WHOLE modelled `JobState` catalog, so the rule is asserted over
    // every state a job can be in rather than over the one `CreateJob` mints.
    assert!(
        JOB_STATE_VALUES.contains(&RELEASING_STATE),
        "expected {RELEASING_STATE} among the modelled JobState values, got {JOB_STATE_VALUES:?}"
    );
    for wire in JOB_STATE_VALUES {
        let case = format!("{case}: in state {wire}");
        set_job_state(&engine, &job_id, wire);
        if *wire == RELEASING_STATE {
            assert_manifest_url(
                &router,
                DEFAULT_REGION,
                &job_id,
                &manifest_host(DEFAULT_REGION),
                &case,
            )
            .await;
        } else {
            refused(
                &router,
                "GetJobManifest",
                json!({ "JobId": job_id }),
                "InvalidJobStateException",
                &case,
            )
            .await;
        }
    }
}

#[tokio::test]
async fn a_job_with_no_manifest_of_its_own_is_refused_as_an_invalid_resource() {
    let (router, engine) = router_with_engine();

    // An export order: the main job is associated with no device, its job parts are. It is
    // put into the releasing state so the refusal can only be about the device it lacks.
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
    set_job_state(&engine, &main_job, RELEASING_STATE);

    let rows: Vec<(&str, String)> = vec![
        (
            "a JID that was never created",
            "JID00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "an MID that was never created",
            "MID00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (export_case, main_job),
    ];

    for (case, job_id) in rows {
        assert_eq!(
            job_id.len(),
            39,
            "case {case:?}: {job_id} is not a 39-character JobId"
        );
        refused(
            &router,
            "GetJobManifest",
            json!({ "JobId": job_id }),
            "InvalidResourceException",
            case,
        )
        .await;
    }
}

#[tokio::test]
async fn the_manifest_url_addresses_the_region_the_request_was_signed_for() {
    let (router, engine) = router_with_engine();
    let case = "a job every Region asks about";
    let job_id = create_job(&router, json!({ "JobType": "IMPORT" }), case).await;
    set_job_state(&engine, &job_id, RELEASING_STATE);

    // The signed Region is interpolated into the bucket's S3 host when the credential
    // scope carries one with an AWS Region's SHAPE; a scope that names no Region leaves the
    // URL on S3's global endpoint, which carries no Region at all.
    let rows: Vec<(&str, &str, String)> = vec![
        ("signed for us-west-2", "us-west-2", manifest_host("us-west-2")),
        (
            "signed for eu-central-1",
            "eu-central-1",
            manifest_host("eu-central-1"),
        ),
        (
            "signed for a three-segment Region",
            "ap-southeast-3",
            manifest_host("ap-southeast-3"),
        ),
        (
            "a scope whose region names no Region",
            "notaregion",
            format!("{JOB_MANIFEST_BUCKET}.s3.amazonaws.com"),
        ),
    ];

    for (case, region, expected_host) in rows {
        assert_manifest_url(&router, region, &job_id, &expected_host, case).await;
    }
}

#[tokio::test]
async fn the_manifest_is_published_only_inside_the_job_credential_window() {
    let (router, engine) = router_with_engine();

    // Both sides of the published bound, and one age well past it: the window runs from
    // the job's creation instant, so a job older than it has no credentials left to serve.
    let hour = Duration::from_secs(60 * 60);
    let rows: Vec<(&str, Duration, bool)> = vec![
        ("a job created just now", Duration::ZERO, true),
        (
            "a job an hour short of the window's end",
            DEVICE_CREDENTIAL_VALIDITY - hour,
            true,
        ),
        (
            "a job exactly at the window's end",
            DEVICE_CREDENTIAL_VALIDITY,
            false,
        ),
        (
            "a job an hour past the window's end",
            DEVICE_CREDENTIAL_VALIDITY + hour,
            false,
        ),
        (
            "a job twice the window old",
            DEVICE_CREDENTIAL_VALIDITY * 2,
            false,
        ),
    ];

    for (case, age, readable) in rows {
        let job_id = create_job(&router, json!({ "JobType": "IMPORT" }), case).await;
        set_job_state(&engine, &job_id, RELEASING_STATE);
        age_job(&engine, &job_id, age);
        if readable {
            assert_manifest_url(
                &router,
                DEFAULT_REGION,
                &job_id,
                &manifest_host(DEFAULT_REGION),
                case,
            )
            .await;
        } else {
            refused(
                &router,
                "GetJobManifest",
                json!({ "JobId": job_id }),
                "InvalidResourceException",
                case,
            )
            .await;
        }
    }
}

#[tokio::test]
async fn every_call_publishes_its_own_window_and_moves_no_job() {
    let (router, engine) = router_with_engine();
    let case = "the same job asked three times";
    let job_id = create_job(
        &router,
        json!({ "JobType": "IMPORT", "SnowballType": "EDGE" }),
        case,
    )
    .await;
    set_job_state(&engine, &job_id, RELEASING_STATE);

    let before = ok(&router, "DescribeJob", json!({ "JobId": job_id }), case).await;
    let listed_before = ok(&router, "ListJobs", json!({ "MaxResults": 100 }), case).await;

    let host = manifest_host(DEFAULT_REGION);
    let mut tokens: BTreeSet<String> = BTreeSet::new();
    for attempt in 1..=3 {
        let case = format!("{case}: call {attempt}");
        let token = assert_manifest_url(&router, DEFAULT_REGION, &job_id, &host, &case).await;
        assert!(
            tokens.insert(token.clone()),
            "case {case:?}: GetJobManifest re-issued {token}, but each call publishes its own \
             60-minute window"
        );
    }

    // Publishing a URL is all the call does: the job itself is untouched, so what a later
    // caller reads is unchanged.
    let after = ok(&router, "DescribeJob", json!({ "JobId": job_id }), case).await;
    let listed_after = ok(&router, "ListJobs", json!({ "MaxResults": 100 }), case).await;
    assert_eq!(
        after, before,
        "case {case:?}: GetJobManifest moved the job DescribeJob reports"
    );
    assert_eq!(
        listed_after, listed_before,
        "case {case:?}: GetJobManifest changed what ListJobs reports"
    );
}

#[tokio::test]
async fn the_manifest_url_and_the_update_url_address_different_files_of_the_same_job() {
    let (router, engine) = router_with_engine();
    let case = "one job asked for both of its published files";
    let job_id = create_job(&router, json!({ "JobType": "IMPORT" }), case).await;
    set_job_state(&engine, &job_id, RELEASING_STATE);

    let manifest_token = assert_manifest_url(
        &router,
        DEFAULT_REGION,
        &job_id,
        &manifest_host(DEFAULT_REGION),
        case,
    )
    .await;

    let updates = ok(&router, "GetSoftwareUpdates", json!({ "JobId": job_id }), case).await;
    let update_url = parse_published_url(&string(&updates, "UpdatesURI", case), case);
    assert_eq!(
        update_url.host,
        format!("{SOFTWARE_UPDATE_BUCKET}.s3.{DEFAULT_REGION}.amazonaws.com"),
        "case {case:?}: expected the update file under its own bucket, got {update_url:?}"
    );
    assert_eq!(
        update_url.path,
        format!("{job_id}/{SOFTWARE_UPDATE_OBJECT}"),
        "case {case:?}: expected the URL to address {job_id}'s update file, got {update_url:?}"
    );
    assert!(
        !update_url.query.contains_key(MANIFEST_TOKEN),
        "case {case:?}: expected the update URL to carry its own token parameter, got \
         {update_url:?}"
    );
    let update_token = parameter(&update_url, "updateToken", case);
    assert_ne!(
        update_token, manifest_token,
        "case {case:?}: expected the two files' grants to carry distinct tokens, got \
         {update_token:?} for both"
    );
}
