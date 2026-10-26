// SPDX-License-Identifier: CC0-1.0

//! `GetSoftwareUpdates`, driven end to end over HTTP.
//!
//! One rule, asserted over a TABLE rather than over a single job: the update file is the
//! software of the Snow device a job ordered ("Returns an Amazon S3 presigned URL for an
//! update file associated with a specified JobId"), so a `GetSoftwareUpdates` call answers
//! a URL only for a job this service persisted that a device has already been prepared and
//! shipped for — and that URL addresses THAT job's update file in the Region the request
//! was signed for, carrying a token distinct from every other URL the service issued and
//! an expiry exactly [`SOFTWARE_UPDATE_AVAILABILITY`] after the call
//! (`GetSoftwareUpdatesResult$UpdatesURI`: "The software update will be available for 2
//! days after this request is made. To access an update after the 2 days have passed,
//! you'll have to make another call to GetSoftwareUpdates"). Every other call is refused:
//! a job no record answers to, and the main job of an export order, which "aren't
//! associated with any Snowballs" (`JobListEntry$IsMaster`), as `InvalidResourceException`;
//! every `JobState` in which no device has been prepared and shipped for the job as
//! `InvalidJobStateException`.
//!
//! The rows differ along what the rule generalises over: the job's state (the WHOLE
//! modelled `JobState` catalog, each side of the rule), the job type and device the order
//! was placed for, whether the job is a plain job or one job part of an export order, the
//! Region the call is signed for (including a scope that names no Region at all), and how
//! many times the same job is asked.
//!
//! No operation of this API ships a device, so a row that needs a job in a state other
//! than the one `CreateJob` mints sets that state in the service's own store before calling
//! (`job_unlock_code::set_job_state`), the store the handler reads. Everything the rule is
//! about still travels over the wire.
//!
//! The harness (`router_with_engine`, `ok`, `refused`, `call_in_region`, `create_address`,
//! `deliverable_address`, `complete_order`, `string`) is the one `job_lifecycle` already
//! drives the service with, so every call here goes through the same front door a client
//! uses.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
// The modelled `JobState` catalog, the published window, the bucket and the object name,
// read from the crate rather than restated, so a case cannot drift from the rule it
// asserts.
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::types::JOB_STATE_VALUES;
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::state::{
    SOFTWARE_UPDATE_AVAILABILITY, SOFTWARE_UPDATE_BUCKET, SOFTWARE_UPDATE_OBJECT,
};
use smithy_server_runtime::MultiServiceRouter;

use super::job_lifecycle::{
    call_in_region, complete_order, create_address, deliverable_address, ok, refused,
    router_with_engine, string,
};
use super::job_unlock_code::set_job_state;

/// `UpdatesURI` and `ManifestURI` are both modelled `len 1..1024`
/// (`smithy_query(query="operation", operation="GetSoftwareUpdates")`,
/// `operation="GetJobManifest"`).
pub(crate) const PUBLISHED_URI_MAX_LEN: usize = 1024;

/// The Region `job_lifecycle`'s `ok`/`refused` helpers sign for.
pub(crate) const DEFAULT_REGION: &str = "us-west-2";

/// The `JobState` values in which a device has been built and sent out for the job, so the
/// job has a device of its own whose software an update file is.
///
/// `JobMetadata$SnowballId` is the "unique ID associated with a device"
/// (`smithy_query(query="doc", shape="JobMetadata", member="SnowballId")`); of the modelled
/// `JobState` catalog (`shared_types::JOB_STATE_VALUES`) these are the values that place a
/// built device somewhere — on its way to the caller, in the caller's hands, on its way
/// back, at the sorting facility, or back at Amazon Web Services with the transfer running
/// or finished. The values left out are the ones in which the order has no device: `New`,
/// `PreparingAppliance` and `PreparingShipment`, in which the order is still being made
/// ready, `Cancelled`, which stops it before any appliance is prepared ("You can only
/// cancel a job before its `JobState` value changes to `PreparingAppliance`", `CancelJob`),
/// and `Listing` and `Pending`, the states an export order's main job passes through, which
/// is "associated with no Snowballs" (`JobListEntry$IsMaster`).
const DEVICE_BEARING_STATES: &[&str] = &[
    "InTransitToCustomer",
    "WithCustomer",
    "InTransitToAWS",
    "WithAWSSortingFacility",
    "WithAWS",
    "InProgress",
    "Complete",
];

/// The state a case puts a job into when all it needs is a job with a device.
const A_DEVICE_BEARING_STATE: &str = "WithCustomer";

/// One URL this service published for a job's file, split into the parts a client reads
/// it as.
#[derive(Debug)]
pub(crate) struct PublishedUrl {
    pub(crate) host: String,
    pub(crate) path: String,
    pub(crate) query: BTreeMap<String, String>,
}

/// Split `uri` into host, path and query parameters, asserting it is an `https` URL.
pub(crate) fn parse_published_url(uri: &str, case: &str) -> PublishedUrl {
    assert!(
        uri.len() <= PUBLISHED_URI_MAX_LEN && !uri.is_empty(),
        "case {case:?}: expected a URI within the modelled 1..{PUBLISHED_URI_MAX_LEN} \
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
    PublishedUrl {
        host: host.to_string(),
        path: path.to_string(),
        query: parameters,
    }
}

/// A query parameter the URL must carry.
pub(crate) fn parameter(url: &PublishedUrl, name: &str, case: &str) -> String {
    url.query
        .get(name)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {name} parameter, got {url:?}"))
        .clone()
}

/// Whole seconds since the UNIX epoch, as the URL's `Expires` parameter spells an instant.
pub(crate) fn now_epoch_seconds() -> u64 {
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
    let url = parse_published_url(&uri, case);
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

/// The host an update URL signed for `region` addresses.
fn update_host(region: &str) -> String {
    format!("{SOFTWARE_UPDATE_BUCKET}.s3.{region}.amazonaws.com")
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

/// The ids of the jobs an order of `submitted`'s shape leaves holding a device of their
/// own: the plain job it minted, or the job parts of an export order, which are what carry
/// a device.
async fn device_bearing_jobs(
    router: &MultiServiceRouter,
    submitted: Value,
    case: &str,
) -> Vec<String> {
    let job_id = create_job(router, submitted, case).await;
    let parts = job_parts(router, &job_id, case).await;
    if parts.is_empty() {
        vec![job_id]
    } else {
        parts
    }
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

/// An export order's shape, for the rows that need the half of an order that carries a
/// device to be a job part rather than a plain job.
fn export_order() -> Value {
    json!({
        "JobType": "EXPORT",
        "SnowballType": "EDGE",
        "Resources": { "S3Resources": [{ "BucketArn": "arn:aws:s3:::export-bucket" }] },
    })
}

#[tokio::test]
async fn get_software_updates_publishes_an_update_url_for_every_job_with_a_device() {
    let (router, engine) = router_with_engine();
    let mut tokens: BTreeSet<String> = BTreeSet::new();

    for (case, submitted) in order_rows() {
        for job in device_bearing_jobs(&router, submitted, case).await {
            set_job_state(&engine, &job, A_DEVICE_BEARING_STATE);
            let token =
                assert_update_url(&router, DEFAULT_REGION, &job, &update_host(DEFAULT_REGION), case)
                    .await;
            assert!(
                tokens.insert(token.clone()),
                "case {case:?}: GetSoftwareUpdates issued {token}, which it had issued before"
            );
        }
    }
}

#[tokio::test]
async fn the_update_file_exists_exactly_while_the_job_has_a_device() {
    let (router, engine) = router_with_engine();

    // The device-bearing states are a subset of the modelled catalog, so a value that
    // leaves the model cannot quietly pass as one.
    for state in DEVICE_BEARING_STATES {
        assert!(
            JOB_STATE_VALUES.contains(state),
            "case {state:?}: expected a modelled JobState, which {JOB_STATE_VALUES:?} does not \
             list"
        );
    }

    // Both halves of the class that carries a device: a plain job, and one job part of an
    // export order.
    let plain = create_job(&router, json!({ "JobType": "IMPORT" }), "a plain job").await;
    let part = device_bearing_jobs(&router, export_order(), "a job part of an export order")
        .await
        .into_iter()
        .next()
        .expect("the export create minted a job part");

    for (shape, job_id) in [("a plain job", &plain), ("an export job part", &part)] {
        // The WHOLE modelled catalog, each state on its own side of the rule.
        for state in JOB_STATE_VALUES {
            let case = format!("{shape} in state {state}");
            set_job_state(&engine, job_id, state);
            if DEVICE_BEARING_STATES.contains(state) {
                assert_update_url(
                    &router,
                    DEFAULT_REGION,
                    job_id,
                    &update_host(DEFAULT_REGION),
                    &case,
                )
                .await;
            } else {
                refused(
                    &router,
                    "GetSoftwareUpdates",
                    json!({ "JobId": job_id }),
                    "InvalidJobStateException",
                    &case,
                )
                .await;
            }
        }
    }
}

#[tokio::test]
async fn every_call_issues_its_own_url_for_the_same_job() {
    let (router, engine) = router_with_engine();
    let case = "the same job asked three times";
    let job_id = create_job(&router, json!({ "JobType": "IMPORT" }), case).await;
    set_job_state(&engine, &job_id, A_DEVICE_BEARING_STATE);

    let host = update_host(DEFAULT_REGION);
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
    let (router, engine) = router_with_engine();
    let job_id = create_job(
        &router,
        json!({ "JobType": "IMPORT" }),
        "a job every Region asks about",
    )
    .await;
    set_job_state(&engine, &job_id, A_DEVICE_BEARING_STATE);

    // The signed Region is interpolated into the bucket's S3 host when the credential
    // scope carries one with an AWS Region's SHAPE; a scope that names no Region leaves
    // the URL on S3's global endpoint, which carries no Region at all.
    let rows: Vec<(&str, &str, String)> = vec![
        ("signed for us-west-2", "us-west-2", update_host("us-west-2")),
        (
            "signed for eu-central-1",
            "eu-central-1",
            update_host("eu-central-1"),
        ),
        (
            "signed for a three-segment Region",
            "ap-southeast-3",
            update_host("ap-southeast-3"),
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
    let (router, engine) = router_with_engine();

    // An export order: the main job is associated with no device, its job parts are — and
    // that is a property of the record rather than of where the order got to, so it holds
    // even with the main job moved into a state a device-bearing job would be served in.
    let export_case = "export order whose main job has no device";
    let main_job = create_job(&router, export_order(), export_case).await;
    assert!(
        !job_parts(&router, &main_job, export_case).await.is_empty(),
        "case {export_case:?}: expected the export create to mint job parts"
    );
    let shipped_main_case = "main job of an export order moved to a shipped state";
    let shipped_main = create_job(&router, export_order(), shipped_main_case).await;
    set_job_state(&engine, &shipped_main, A_DEVICE_BEARING_STATE);

    // A job still being prepared: no device has been built and sent for it yet, which is
    // the state `CreateJob` leaves an order in.
    let new_case = "a newly created job, for which no device is prepared yet";
    let newly_created = create_job(&router, json!({ "JobType": "IMPORT" }), new_case).await;

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
    let cancelled_main = create_job(&router, export_order(), cancelled_order_case).await;
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
        (
            shipped_main_case,
            shipped_main,
            "InvalidResourceException",
        ),
        (new_case, newly_created, "InvalidJobStateException"),
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
    let (router, engine) = router_with_engine();
    let case = "a job read before and after its update is asked for";
    let job_id = create_job(
        &router,
        json!({ "JobType": "IMPORT", "SnowballType": "EDGE" }),
        case,
    )
    .await;
    set_job_state(&engine, &job_id, A_DEVICE_BEARING_STATE);

    let before = ok(&router, "DescribeJob", json!({ "JobId": job_id }), case).await;
    let listed_before = ok(&router, "ListJobs", json!({ "MaxResults": 100 }), case).await;

    for attempt in 1..=2 {
        let case = format!("{case}: call {attempt}");
        assert_update_url(
            &router,
            DEFAULT_REGION,
            &job_id,
            &update_host(DEFAULT_REGION),
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
