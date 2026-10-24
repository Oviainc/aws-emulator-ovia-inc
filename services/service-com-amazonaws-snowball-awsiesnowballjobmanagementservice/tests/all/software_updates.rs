// SPDX-License-Identifier: CC0-1.0

//! `GetSoftwareUpdates`, driven end to end over HTTP.
//!
//! One rule, asserted over a TABLE rather than over a single job: a `GetSoftwareUpdates`
//! call for a job this service persisted that still has a device of its own answers an
//! Amazon S3 URL addressing THAT job's update file in the Region the request was signed
//! for, carrying a token distinct from every other URL the service issued and an expiry
//! exactly [`SOFTWARE_UPDATE_AVAILABILITY`] after the call
//! (`GetSoftwareUpdatesResult$UpdatesURI`: "The software update will be available for 2
//! days after this request is made. To access an update after the 2 days have passed,
//! you'll have to make another call to GetSoftwareUpdates"); a job no record answers to,
//! and the main job of an export order, which "aren't associated with any Snowballs"
//! (`JobListEntry$IsMaster`), are refused as `InvalidResourceException`, and a job whose
//! state means no device will ever be prepared for it as `InvalidJobStateException`.
//!
//! The rows differ along what the rule generalises over: the job type and device the order
//! was placed for, whether the job is a plain job or one job part of an export order, the
//! Region the call is signed for (including a scope that names no Region at all), and how
//! many times the same job is asked.
//!
//! The harness (`router`, `ok`, `refused`, `call_in_region`, `create_address`,
//! `deliverable_address`, `string`) is the one `job_lifecycle` already drives the service
//! with, so every call here goes through the same front door a client uses.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
// The published window, the bucket and the object name, read from the crate rather than
// restated, so a case cannot drift from the rule it asserts.
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::state::{
    SOFTWARE_UPDATE_AVAILABILITY, SOFTWARE_UPDATE_BUCKET, SOFTWARE_UPDATE_OBJECT,
};
use smithy_server_runtime::MultiServiceRouter;

use super::job_lifecycle::{
    call_in_region, complete_order, create_address, deliverable_address, ok, refused, string,
};

/// `UpdatesURI` is modelled `len 1..1024`
/// (`smithy_query(query="operation", operation="GetSoftwareUpdates")`).
const UPDATES_URI_MAX_LEN: usize = 1024;

/// The Region `job_lifecycle`'s `ok`/`refused` helpers sign for.
const DEFAULT_REGION: &str = "us-west-2";

/// One update URL, split into the parts a client reads it as.
#[derive(Debug)]
struct UpdateUrl {
    host: String,
    path: String,
    query: BTreeMap<String, String>,
}

/// Split `uri` into host, path and query parameters, asserting it is an `https` URL.
fn parse_update_url(uri: &str, case: &str) -> UpdateUrl {
    assert!(
        uri.len() <= UPDATES_URI_MAX_LEN && !uri.is_empty(),
        "case {case:?}: expected an UpdatesURI within the modelled 1..{UPDATES_URI_MAX_LEN} \
         characters, got {} characters: {uri:?}",
        uri.len()
    );
    let rest = uri
        .strip_prefix("https://")
        .unwrap_or_else(|| panic!("case {case:?}: expected an https URL, got {uri:?}"));
    let (host, after_host) = rest
        .split_once('/')
        .unwrap_or_else(|| panic!("case {case:?}: expected a path in {uri:?}"));
    let (path, query) = after_host.split_once('?').unwrap_or((after_host, ""));
    let parameters = query
        .split('&')
        .filter(|parameter| !parameter.is_empty())
        .map(|parameter| {
            let (name, value) = parameter.split_once('=').unwrap_or_else(|| {
                panic!("case {case:?}: expected name=value query parameters in {uri:?}")
            });
            (name.to_string(), value.to_string())
        })
        .collect();
    UpdateUrl {
        host: host.to_string(),
        path: path.to_string(),
        query: parameters,
    }
}

/// A query parameter the URL must carry.
fn parameter(url: &UpdateUrl, name: &str, case: &str) -> String {
    url.query
        .get(name)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {name} parameter, got {url:?}"))
        .clone()
}

/// Whole seconds since the UNIX epoch, as the URL's `Expires` parameter spells an instant.
fn now_epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the UNIX epoch")
        .as_secs()
}

/// Call `GetSoftwareUpdates` for `job_id` signed for `region` and assert the URL it
/// answered addresses that job's update file under `expected_host`, with an expiry
/// [`SOFTWARE_UPDATE_AVAILABILITY`] after the call. Returns the URL's token.
async fn assert_update_url(
    router: &MultiServiceRouter,
    region: &str,
    job_id: &str,
    expected_host: &str,
    case: &str,
) -> String {
    let issued_no_earlier_than = now_epoch_seconds();
    let (status, body) = call_in_region(
        router,
        region,
        "GetSoftwareUpdates",
        json!({ "JobId": job_id }),
    )
    .await;
    let issued_no_later_than = now_epoch_seconds();
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetSoftwareUpdates to answer 200, got {status} with {body}"
    );

    let uri = string(&body, "UpdatesURI", case);
    let url = parse_update_url(&uri, case);
    assert_eq!(
        url.host, expected_host,
        "case {case:?}: expected the update file under {expected_host}, got {uri:?}"
    );
    assert_eq!(
        url.path,
        format!("{job_id}/{SOFTWARE_UPDATE_OBJECT}"),
        "case {case:?}: expected the URL to address {job_id}'s update file, got {uri:?}"
    );

    let token = parameter(&url, "updateToken", case);
    assert!(
        token.len() == 64
            && token
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "case {case:?}: expected a 64-character lowercase-hex updateToken, got {token:?}"
    );

    let expires: u64 = parameter(&url, "Expires", case)
        .parse()
        .unwrap_or_else(|error| {
            panic!("case {case:?}: expected epoch seconds in Expires, got {error} for {uri:?}")
        });
    let window = SOFTWARE_UPDATE_AVAILABILITY.as_secs();
    assert!(
        (issued_no_earlier_than + window..=issued_no_later_than + window).contains(&expires),
        "case {case:?}: expected the URL to expire {window} seconds after the call \
         (between {} and {}), got {expires}",
        issued_no_earlier_than + window,
        issued_no_later_than + window
    );

    token
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
        .map(|parts| {
            parts
                .iter()
                .map(|part| string(part, "JobId", case))
                .collect()
        })
        .unwrap_or_default()
}

/// Every order row: one per job type and device the update is asked for, so the rule is
/// asserted over the class of persisted jobs rather than over one of them.
fn order_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "import job on an EDGE device",
            json!({
                "JobType": "IMPORT",
                "SnowballType": "EDGE",
                "ShippingOption": "SECOND_DAY",
                "Resources": {
                    "S3Resources": [{ "BucketArn": "arn:aws:s3:::update-bucket" }]
                },
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
                "Resources": {
                    "S3Resources": [{ "BucketArn": "arn:aws:s3:::export-bucket" }]
                },
            }),
        ),
    ]
}

#[tokio::test]
async fn get_software_updates_publishes_an_update_url_for_every_job_with_a_device() {
    let router = super::job_lifecycle::router();
    let mut tokens: BTreeSet<String> = BTreeSet::new();

    for (case, submitted) in order_rows() {
        let job_id = create_job(&router, submitted, case).await;
        // An export create mints a MAIN job plus one job part per device; it is the parts
        // that carry a device, so they are what the update is asked for. Every other
        // create mints one plain job, which carries its own device.
        let parts = job_parts(&router, &job_id, case).await;
        let with_a_device: Vec<String> = if parts.is_empty() {
            vec![job_id.clone()]
        } else {
            parts
        };

        for job in &with_a_device {
            let token = assert_update_url(
                &router,
                DEFAULT_REGION,
                job,
                &format!("{SOFTWARE_UPDATE_BUCKET}.s3.{DEFAULT_REGION}.amazonaws.com"),
                case,
            )
            .await;
            assert!(
                tokens.insert(token.clone()),
                "case {case:?}: GetSoftwareUpdates issued {token}, which it had issued before"
            );
        }
    }
}

#[tokio::test]
async fn every_call_issues_its_own_url_for_the_same_job() {
    let router = super::job_lifecycle::router();
    let case = "the same job asked three times";
    let job_id = create_job(&router, json!({ "JobType": "IMPORT" }), case).await;

    let host = format!("{SOFTWARE_UPDATE_BUCKET}.s3.{DEFAULT_REGION}.amazonaws.com");
    let mut tokens: BTreeSet<String> = BTreeSet::new();
    for attempt in 1..=3 {
        let case = format!("{case}: call {attempt}");
        let token = assert_update_url(&router, DEFAULT_REGION, &job_id, &host, &case).await;
        assert!(
            tokens.insert(token.clone()),
            "case {case:?}: GetSoftwareUpdates re-issued {token}, but each call publishes its \
             own two-day window"
        );
    }
}

#[tokio::test]
async fn the_url_addresses_the_region_the_request_was_signed_for() {
    let router = super::job_lifecycle::router();
    let job_id = create_job(
        &router,
        json!({ "JobType": "IMPORT" }),
        "a job every Region asks about",
    )
    .await;

    // The signed Region is interpolated into the bucket's S3 host when the credential
    // scope carries one with an AWS Region's SHAPE; a scope that names no Region leaves
    // the URL on S3's global endpoint, which carries no Region at all.
    let rows: Vec<(&str, &str, String)> = vec![
        (
            "signed for us-west-2",
            "us-west-2",
            format!("{SOFTWARE_UPDATE_BUCKET}.s3.us-west-2.amazonaws.com"),
        ),
        (
            "signed for eu-central-1",
            "eu-central-1",
            format!("{SOFTWARE_UPDATE_BUCKET}.s3.eu-central-1.amazonaws.com"),
        ),
        (
            "signed for a three-segment Region",
            "ap-southeast-3",
            format!("{SOFTWARE_UPDATE_BUCKET}.s3.ap-southeast-3.amazonaws.com"),
        ),
        (
            "a scope whose region names no Region",
            "notaregion",
            format!("{SOFTWARE_UPDATE_BUCKET}.s3.amazonaws.com"),
        ),
    ];

    for (case, region, expected_host) in rows {
        assert_update_url(&router, region, &job_id, &expected_host, case).await;
    }
}

#[tokio::test]
async fn get_software_updates_refuses_a_job_with_no_update_file() {
    let router = super::job_lifecycle::router();

    // An export order: the main job is associated with no device, its job parts are.
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

    // A cancelled order: `CancelJob` moves the main job and every part still inside its
    // own cancellation window, so no device is ever prepared for any of them.
    let cancelled_case = "a cancelled job";
    let cancelled = create_job(&router, json!({ "JobType": "IMPORT" }), cancelled_case).await;
    ok(
        &router,
        "CancelJob",
        json!({ "JobId": cancelled }),
        cancelled_case,
    )
    .await;

    let cancelled_order_case = "a job part of a cancelled export order";
    let cancelled_main = create_job(
        &router,
        json!({
            "JobType": "EXPORT",
            "SnowballType": "EDGE",
            "Resources": { "S3Resources": [{ "BucketArn": "arn:aws:s3:::export-bucket" }] },
        }),
        cancelled_order_case,
    )
    .await;
    let cancelled_part = job_parts(&router, &cancelled_main, cancelled_order_case)
        .await
        .into_iter()
        .next()
        .expect("the export create minted a job part");
    ok(
        &router,
        "CancelJob",
        json!({ "JobId": cancelled_main }),
        cancelled_order_case,
    )
    .await;

    let rows: Vec<(&str, String, &str)> = vec![
        (
            "a JID that was never created",
            "JID00000000-0000-0000-0000-000000000000".to_string(),
            "InvalidResourceException",
        ),
        (
            "an MID that was never created",
            "MID00000000-0000-0000-0000-000000000000".to_string(),
            "InvalidResourceException",
        ),
        (export_case, main_job, "InvalidResourceException"),
        (cancelled_case, cancelled, "InvalidJobStateException"),
        (
            cancelled_order_case,
            cancelled_part,
            "InvalidJobStateException",
        ),
    ];

    for (case, job_id, error_code) in rows {
        assert_eq!(
            job_id.len(),
            39,
            "case {case:?}: {job_id} is not a 39-character JobId"
        );
        refused(
            &router,
            "GetSoftwareUpdates",
            json!({ "JobId": job_id }),
            error_code,
            case,
        )
        .await;
    }
}

#[tokio::test]
async fn asking_for_an_update_changes_nothing_a_later_caller_reads() {
    let router = super::job_lifecycle::router();
    let case = "a job read before and after its update is asked for";
    let job_id = create_job(
        &router,
        json!({ "JobType": "IMPORT", "SnowballType": "EDGE" }),
        case,
    )
    .await;

    let before = ok(&router, "DescribeJob", json!({ "JobId": job_id }), case).await;
    let listed_before = ok(&router, "ListJobs", json!({ "MaxResults": 100 }), case).await;

    for attempt in 1..=2 {
        let case = format!("{case}: call {attempt}");
        assert_update_url(
            &router,
            DEFAULT_REGION,
            &job_id,
            &format!("{SOFTWARE_UPDATE_BUCKET}.s3.{DEFAULT_REGION}.amazonaws.com"),
            &case,
        )
        .await;
    }

    let after = ok(&router, "DescribeJob", json!({ "JobId": job_id }), case).await;
    let listed_after = ok(&router, "ListJobs", json!({ "MaxResults": 100 }), case).await;
    assert_eq!(
        after, before,
        "case {case:?}: GetSoftwareUpdates moved the job DescribeJob reports"
    );
    assert_eq!(
        listed_after, listed_before,
        "case {case:?}: GetSoftwareUpdates changed what ListJobs reports"
    );
}
