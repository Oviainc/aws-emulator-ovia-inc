// SPDX-License-Identifier: CC0-1.0

//! `CreateReturnShippingLabel` and `DescribeReturnShippingLabel`, driven end to end over
//! HTTP.
//!
//! One rule, asserted over a TABLE rather than over a single job: a
//! `CreateReturnShippingLabel` call for a job this service persisted that still has a
//! device to return STARTS the task that creates that job's label and answers that task's
//! status, and `DescribeReturnShippingLabel` reports the status the clock makes true at the
//! instant it is asked — `InProgress` while the task runs, with no label file and no expiry
//! to report, and `Succeeded` from then on, with an Amazon S3 URI addressing THAT job's
//! label file in the Region the request was signed for and the instant the label expires. A
//! second create while the task is running is the "more than once when other requests are
//! not completed" `ConflictException`, and a second create while the label is valid is
//! `ReturnShippingLabelAlreadyExistsException`; a job no record answers to, the main job of
//! an export order, which "aren't associated with any Snowballs"
//! (`JobListEntry$IsMaster`), and a job that has no label yet are refused as
//! `InvalidResourceException`, and a job whose state leaves it with no device to return as
//! `InvalidJobStateException`.
//!
//! The rows differ along what the rule generalises over: the job type and device the order
//! was placed for, whether the subject is a plain job or one job part of an export order,
//! whether the return's `ShippingOption` is supplied or inherited from the job, which value
//! of the modelled `ShippingOption` catalog it carries, the Region the call is signed for
//! (including a scope that names no Region at all), and how many times the same job is
//! asked.
//!
//! Nothing here reaches into the state struct, so a response that is not computed from
//! persisted state cannot pass. The one behaviour no case can reach is a label's EXPIRY:
//! `RETURN_SHIPPING_LABEL_VALIDITY` is weeks long and this emulator has no clock a test can
//! move, so the expiry is asserted through the instant `ExpirationDate` reports rather than
//! by waiting for it.
//!
//! The harness (`router`, `ok`, `refused`, `call_in_region`, `create_address`,
//! `deliverable_address`, `string`) is the one `job_lifecycle` already drives the service
//! with, so every call here goes through the same front door a client uses.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
// The values the modelled `ShippingOption` enum admits, so the table is written against
// the catalog rather than against a list of its own.
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::types::SHIPPING_OPTION_VALUES;
// The windows, the bucket and the object name, read from the crate rather than restated, so
// a case cannot drift from the rule it asserts.
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::state::{
    RETURN_SHIPPING_LABEL_BUCKET, RETURN_SHIPPING_LABEL_OBJECT, RETURN_SHIPPING_LABEL_PREPARATION,
    RETURN_SHIPPING_LABEL_VALIDITY,
};
use smithy_server_runtime::MultiServiceRouter;

use super::job_lifecycle::{
    call_in_region, complete_order, create_address, deliverable_address, ok, refused, string,
};

/// `ReturnShippingLabelURI` is modelled `len 1..1024`
/// (`smithy_query(query="operation", operation="DescribeReturnShippingLabel")`).
const LABEL_URI_MAX_LEN: usize = 1024;

/// The Region `job_lifecycle`'s `ok`/`refused` helpers sign for.
const DEFAULT_REGION: &str = "us-west-2";

/// How long a case waits past [`RETURN_SHIPPING_LABEL_PREPARATION`] before it expects the
/// label-creating task to have finished.
const PREPARATION_MARGIN: Duration = Duration::from_millis(150);

/// One label URI, split into the parts a client reads it as.
#[derive(Debug)]
pub(crate) struct LabelUrl {
    pub(crate) host: String,
    pub(crate) path: String,
    pub(crate) query: BTreeMap<String, String>,
}

/// Split `uri` into host, path and query parameters, asserting it is an `https` URL within
/// the modelled length.
pub(crate) fn parse_label_url(uri: &str, case: &str) -> LabelUrl {
    assert!(
        !uri.is_empty() && uri.len() <= LABEL_URI_MAX_LEN,
        "case {case:?}: expected a ReturnShippingLabelURI within the modelled \
         1..{LABEL_URI_MAX_LEN} characters, got {} characters: {uri:?}",
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
    LabelUrl {
        host: host.to_string(),
        path: path.to_string(),
        query: parameters,
    }
}

/// A query parameter the URL must carry.
pub(crate) fn parameter(url: &LabelUrl, name: &str, case: &str) -> String {
    url.query
        .get(name)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {name} parameter, got {url:?}"))
        .clone()
}

/// Seconds since the UNIX epoch, as a `Timestamp` member spells an instant on the wire.
fn now_epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the UNIX epoch")
        .as_secs_f64()
}

/// Wait out the label-creating task.
///
/// A blocking sleep: the workspace's `tokio` carries no timer, and a test waiting for a
/// span this emulator fixes has nothing else to run meanwhile.
pub(crate) fn wait_for_the_label() {
    std::thread::sleep(RETURN_SHIPPING_LABEL_PREPARATION + PREPARATION_MARGIN);
}

/// Create one job of `submitted`'s shape against a deliverable address and return its
/// `JobId`. The row's members are merged over the harness's order defaults
/// (`job_lifecycle::complete_order`), so a row states only the axis it varies.
pub(crate) async fn create_job(
    router: &MultiServiceRouter,
    submitted: Value,
    case: &str,
) -> String {
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
pub(crate) async fn job_parts(
    router: &MultiServiceRouter,
    job_id: &str,
    case: &str,
) -> Vec<String> {
    let described = ok(router, "DescribeJob", json!({ "JobId": job_id }), case).await;
    described
        .get("SubJobMetadata")
        .and_then(Value::as_array)
        .map(|parts| parts.iter().map(|part| string(part, "JobId", case)).collect())
        .unwrap_or_default()
}

/// The job the label is created for: the job `submitted` mints, or — when `use_job_part` —
/// the first job part of the export order it mints, which is the job that carries a device.
pub(crate) async fn label_subject(
    router: &MultiServiceRouter,
    submitted: Value,
    use_job_part: bool,
    case: &str,
) -> String {
    let job_id = create_job(router, submitted, case).await;
    if !use_job_part {
        return job_id;
    }
    job_parts(router, &job_id, case)
        .await
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected the export create to mint a job part"))
}

/// One row of the table: a job to return, the `ShippingOption` the return is asked for and
/// the speed the label is then expected to travel at.
struct Row {
    case: &'static str,
    submitted: Value,
    use_job_part: bool,
    /// The `ShippingOption` the `CreateReturnShippingLabel` call carries, or `None` when it
    /// names none.
    asked: Option<&'static str>,
    /// The speed the label records: the asked-for one, or the job's own when none was
    /// asked, or nothing at all when neither names one.
    expected: Option<&'static str>,
}

/// Every row: one per job shape, per presence of the return's own `ShippingOption`, and per
/// value of the modelled catalog (`shared_types::SHIPPING_OPTION_VALUES`) — which
/// [`the_rows_cover_the_modelled_shipping_option_catalog`] checks the rows exhaust.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "import job shipped SECOND_DAY, returned NEXT_DAY",
            submitted: json!({
                "JobType": "IMPORT",
                "SnowballType": "EDGE",
                "ShippingOption": "SECOND_DAY",
                "Resources": { "S3Resources": [{ "BucketArn": "arn:aws:s3:::return-bucket" }] },
            }),
            use_job_part: false,
            asked: Some("NEXT_DAY"),
            expected: Some("NEXT_DAY"),
        },
        Row {
            case: "local-use job on a Snowcone SSD, returned at the job's own speed",
            submitted: json!({
                "JobType": "LOCAL_USE",
                "SnowballType": "SNC1_SSD",
                "ShippingOption": "SECOND_DAY",
                "RemoteManagement": "INSTALLED_AUTOSTART",
            }),
            use_job_part: false,
            asked: None,
            expected: Some("SECOND_DAY"),
        },
        Row {
            case: "an order naming only its job type, returned EXPRESS",
            submitted: json!({ "JobType": "IMPORT" }),
            use_job_part: false,
            asked: Some("EXPRESS"),
            expected: Some("EXPRESS"),
        },
        Row {
            case: "an import job on a compute-optimized device, returned at the job's own speed",
            submitted: json!({
                "JobType": "IMPORT",
                "SnowballType": "EDGE_C",
                "ShippingOption": "STANDARD",
            }),
            use_job_part: false,
            asked: None,
            expected: Some("STANDARD"),
        },
        Row {
            case: "a job part of an export order, returned STANDARD",
            submitted: json!({
                "JobType": "EXPORT",
                "SnowballType": "EDGE_S",
                "ShippingOption": "EXPRESS",
                "Resources": { "S3Resources": [{ "BucketArn": "arn:aws:s3:::export-bucket" }] },
            }),
            use_job_part: true,
            asked: Some("STANDARD"),
            expected: Some("STANDARD"),
        },
    ]
}

/// The body of a `CreateReturnShippingLabel` call for `job_id`, carrying `asked` when the
/// row names one.
fn create_body(job_id: &str, asked: Option<&str>) -> Value {
    match asked {
        Some(option) => json!({ "JobId": job_id, "ShippingOption": option }),
        None => json!({ "JobId": job_id }),
    }
}

#[test]
fn the_rows_cover_the_modelled_shipping_option_catalog() {
    let asked: BTreeSet<&str> = rows().iter().filter_map(|row| row.asked).collect();
    let inherited: BTreeSet<&str> = rows()
        .iter()
        .filter(|row| row.asked.is_none())
        .filter_map(|row| row.expected)
        .collect();
    let covered: BTreeSet<&str> = asked.union(&inherited).copied().collect();
    let catalog: BTreeSet<&str> = SHIPPING_OPTION_VALUES.iter().copied().collect();
    assert_eq!(
        covered, catalog,
        "the rows must exercise every modelled ShippingOption, got {covered:?} against {catalog:?}"
    );
}

#[tokio::test]
async fn a_return_shipping_label_is_created_for_every_job_with_a_device_to_return() {
    let router = super::job_lifecycle::router();
    let mut tokens: BTreeSet<String> = BTreeSet::new();

    for row in rows() {
        let case = row.case;
        let job_id = label_subject(&router, row.submitted.clone(), row.use_job_part, case).await;

        // The create STARTS the task, so the status it answers is the task's, not a
        // finished label's.
        let requested_no_earlier_than = now_epoch_seconds();
        let created = ok(
            &router,
            "CreateReturnShippingLabel",
            create_body(&job_id, row.asked),
            case,
        )
        .await;
        let requested_no_later_than = now_epoch_seconds();
        assert_eq!(
            string(&created, "Status", case),
            "InProgress",
            "case {case:?}: expected the create to answer the task's status, got {created}"
        );

        // While the task runs there is no label file and no window to report, and a second
        // create is the request that "is not completed".
        let in_progress = ok(
            &router,
            "DescribeReturnShippingLabel",
            json!({ "JobId": job_id }),
            case,
        )
        .await;
        assert_eq!(
            string(&in_progress, "Status", case),
            "InProgress",
            "case {case:?}: expected the label to still be in progress, got {in_progress}"
        );
        assert_eq!(
            in_progress.get("ReturnShippingLabelURI"),
            None,
            "case {case:?}: expected no label URI while the task runs, got {in_progress}"
        );
        assert_eq!(
            in_progress.get("ExpirationDate"),
            None,
            "case {case:?}: expected no expiry while the task runs, got {in_progress}"
        );
        refused(
            &router,
            "CreateReturnShippingLabel",
            create_body(&job_id, row.asked),
            "ConflictException",
            case,
        )
        .await;

        wait_for_the_label();

        let described = ok(
            &router,
            "DescribeReturnShippingLabel",
            json!({ "JobId": job_id }),
            case,
        )
        .await;
        assert_eq!(
            string(&described, "Status", case),
            "Succeeded",
            "case {case:?}: expected the finished task to report Succeeded, got {described}"
        );

        let expiration = described
            .get("ExpirationDate")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected an ExpirationDate on a created label, got {described}")
            });
        let window = (RETURN_SHIPPING_LABEL_PREPARATION + RETURN_SHIPPING_LABEL_VALIDITY).as_secs_f64();
        assert!(
            (requested_no_earlier_than + window..=requested_no_later_than + window)
                .contains(&expiration),
            "case {case:?}: expected the label to expire {window} seconds after the create \
             (between {} and {}), got {expiration}",
            requested_no_earlier_than + window,
            requested_no_later_than + window
        );

        let uri = string(&described, "ReturnShippingLabelURI", case);
        let url = parse_label_url(&uri, case);
        assert_eq!(
            url.host,
            format!("{RETURN_SHIPPING_LABEL_BUCKET}.s3.{DEFAULT_REGION}.amazonaws.com"),
            "case {case:?}: expected the label under the signed Region's bucket host, got {uri:?}"
        );
        assert_eq!(
            url.path,
            format!("{job_id}/{RETURN_SHIPPING_LABEL_OBJECT}"),
            "case {case:?}: expected the URI to address {job_id}'s label file, got {uri:?}"
        );
        assert_eq!(
            parameter(&url, "Expires", case),
            format!("{}", expiration as u64),
            "case {case:?}: expected the URI to expire when ExpirationDate says ({expiration}), \
             got {uri:?}"
        );
        assert_eq!(
            url.query.get("shippingOption").map(String::as_str),
            row.expected,
            "case {case:?}: expected the label to travel at {:?}, got {uri:?}",
            row.expected
        );

        let token = parameter(&url, "labelToken", case);
        assert!(
            token.len() == 64
                && token
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "case {case:?}: expected a 64-character lowercase-hex labelToken, got {token:?}"
        );
        assert!(
            tokens.insert(token.clone()),
            "case {case:?}: the service minted {token}, which it had minted before"
        );

        // The label is now a valid one, so a further create is refused and leaves it alone.
        refused(
            &router,
            "CreateReturnShippingLabel",
            create_body(&job_id, row.asked),
            "ReturnShippingLabelAlreadyExistsException",
            case,
        )
        .await;
        let described_again = ok(
            &router,
            "DescribeReturnShippingLabel",
            json!({ "JobId": job_id }),
            case,
        )
        .await;
        assert_eq!(
            described_again, described,
            "case {case:?}: the refused create changed the label the service reports"
        );
    }
}

#[tokio::test]
async fn describing_a_label_reports_the_region_the_request_was_signed_for() {
    let router = super::job_lifecycle::router();
    let case = "a label every Region asks about";
    let job_id = create_job(&router, json!({ "JobType": "IMPORT" }), case).await;
    ok(
        &router,
        "CreateReturnShippingLabel",
        json!({ "JobId": job_id, "ShippingOption": "NEXT_DAY" }),
        case,
    )
    .await;
    wait_for_the_label();

    // The signed Region is interpolated into the bucket's S3 host when the credential
    // scope carries one with an AWS Region's SHAPE; a scope that names no Region leaves the
    // URI on S3's global endpoint, which carries no Region at all.
    let rows: Vec<(&str, &str, String)> = vec![
        (
            "signed for us-west-2",
            "us-west-2",
            format!("{RETURN_SHIPPING_LABEL_BUCKET}.s3.us-west-2.amazonaws.com"),
        ),
        (
            "signed for eu-central-1",
            "eu-central-1",
            format!("{RETURN_SHIPPING_LABEL_BUCKET}.s3.eu-central-1.amazonaws.com"),
        ),
        (
            "signed for a three-segment Region",
            "ap-southeast-3",
            format!("{RETURN_SHIPPING_LABEL_BUCKET}.s3.ap-southeast-3.amazonaws.com"),
        ),
        (
            "a scope whose region names no Region",
            "notaregion",
            format!("{RETURN_SHIPPING_LABEL_BUCKET}.s3.amazonaws.com"),
        ),
    ];

    for (case, region, expected_host) in rows {
        let (status, body) = call_in_region(
            &router,
            region,
            "DescribeReturnShippingLabel",
            json!({ "JobId": job_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeReturnShippingLabel to answer 200, got {status} \
             with {body}"
        );
        let uri = string(&body, "ReturnShippingLabelURI", case);
        let url = parse_label_url(&uri, case);
        assert_eq!(
            url.host, expected_host,
            "case {case:?}: expected the label under {expected_host}, got {uri:?}"
        );
    }
}

#[tokio::test]
async fn a_job_with_no_device_to_return_has_no_return_shipping_label() {
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

    // A cancelled job: `CancelJob` stops it before any appliance is prepared, so no device
    // of its own will ever ship and none can be returned.
    let cancelled_case = "a cancelled job";
    let cancelled = create_job(&router, json!({ "JobType": "IMPORT" }), cancelled_case).await;
    ok(
        &router,
        "CancelJob",
        json!({ "JobId": cancelled }),
        cancelled_case,
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
    ];

    for (case, job_id, error_code) in rows {
        assert_eq!(
            job_id.len(),
            39,
            "case {case:?}: {job_id} is not a 39-character JobId"
        );
        for operation in ["CreateReturnShippingLabel", "DescribeReturnShippingLabel"] {
            refused(
                &router,
                operation,
                json!({ "JobId": job_id }),
                error_code,
                case,
            )
            .await;
        }
    }
}

#[tokio::test]
async fn a_job_whose_label_was_never_created_has_none_to_describe() {
    let router = super::job_lifecycle::router();
    let case = "a job no return shipping label was created for";
    let job_id = create_job(
        &router,
        json!({ "JobType": "LOCAL_USE", "SnowballType": "EDGE_CG" }),
        case,
    )
    .await;

    refused(
        &router,
        "DescribeReturnShippingLabel",
        json!({ "JobId": job_id }),
        "InvalidResourceException",
        case,
    )
    .await;
}

#[tokio::test]
async fn describing_a_label_changes_nothing_a_later_caller_reads() {
    let router = super::job_lifecycle::router();
    let case = "a job read before and after its label is created and described";
    let job_id = create_job(
        &router,
        json!({ "JobType": "IMPORT", "SnowballType": "EDGE", "ShippingOption": "SECOND_DAY" }),
        case,
    )
    .await;

    let job_before = ok(&router, "DescribeJob", json!({ "JobId": job_id }), case).await;
    let listed_before = ok(&router, "ListJobs", json!({ "MaxResults": 100 }), case).await;

    ok(
        &router,
        "CreateReturnShippingLabel",
        json!({ "JobId": job_id }),
        case,
    )
    .await;
    wait_for_the_label();

    // Two reads of the same label answer the same thing: the status, the URI and the expiry
    // are derived from what the create recorded, never minted per call.
    let first = ok(
        &router,
        "DescribeReturnShippingLabel",
        json!({ "JobId": job_id }),
        case,
    )
    .await;
    let second = ok(
        &router,
        "DescribeReturnShippingLabel",
        json!({ "JobId": job_id }),
        case,
    )
    .await;
    assert_eq!(
        second, first,
        "case {case:?}: DescribeReturnShippingLabel answered two different labels for one job"
    );

    // And the job itself is untouched: creating a return label is not a shipment.
    let job_after = ok(&router, "DescribeJob", json!({ "JobId": job_id }), case).await;
    let listed_after = ok(&router, "ListJobs", json!({ "MaxResults": 100 }), case).await;
    assert_eq!(
        job_after, job_before,
        "case {case:?}: the return shipping label moved the job DescribeJob reports"
    );
    assert_eq!(
        listed_after, listed_before,
        "case {case:?}: the return shipping label changed what ListJobs reports"
    );
}
