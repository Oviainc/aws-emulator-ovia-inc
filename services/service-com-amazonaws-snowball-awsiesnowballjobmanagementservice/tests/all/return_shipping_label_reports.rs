// SPDX-License-Identifier: CC0-1.0

//! `DescribeReturnShippingLabel` as a REPORTING operation, driven end to end over HTTP.
//!
//! The rule these suites pin: a reporting operation of this service answers a response its
//! own model declares — a 200 computed from the record the creating call persisted, or one
//! of the operation's modelled client errors — for every class of input it admits, and
//! never the framework's "scaffolded but not implemented" 501, which claims no handler
//! exists. For `DescribeReturnShippingLabel` that means the reported `Status` is always a
//! value of the modelled `ShippingLabelStatus` catalog, and the label it reports is the one
//! stored against the `JobId` in the request — so a store holding several labels at once
//! answers each job with its own.
//!
//! `return_shipping_labels` asserts what the label's members CONTAIN for one job at a time;
//! these suites assert the two things that file leaves open: that no input class reaches the
//! 501 channel, and that several stored labels stay separable. The rows differ along what
//! the rule generalises over: the input class (a job no record answers to under each
//! modelled `JobId` prefix, a job with no device to return, a job with no label, a label
//! whose task is still running, a finished label, a job part of an export order), the job
//! and device shape, the shipping speed, and how many labelled jobs the store holds.
//!
//! Nothing here reaches into the state struct: every call goes through the same front door
//! a client uses, so a response that is not computed from persisted state cannot pass.

use std::collections::BTreeSet;

use serde_json::{json, Value};
// The modelled status catalog, so the reported status is checked against the model rather
// than against a list restated here.
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::types::SHIPPING_LABEL_STATUS_VALUES;
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::state::RETURN_SHIPPING_LABEL_OBJECT;
use smithy_server_runtime::MultiServiceRouter;

use super::job_lifecycle::{call_in_region, ok, string};
use super::return_shipping_labels::{
    create_job, job_parts, label_subject, parameter, parse_label_url, wait_for_the_label,
};

/// The Region every call in these suites signs for.
const SIGNED_REGION: &str = "us-west-2";

/// The wire error code of the framework's scaffold channel
/// (`crate::scaffolded::Scaffolded::NotImplemented`), which answers HTTP 501.
const SCAFFOLD_ERROR_CODE: &str = "NotImplemented";

/// What an input class is answered with.
#[derive(Debug)]
enum Expected {
    /// HTTP 200 reporting this `ShippingLabelStatus`, with the label's file and expiry
    /// present only once the creating task has produced them.
    Reported {
        status: &'static str,
        has_label_file: bool,
    },
    /// HTTP 400 with this modelled error code.
    Refused(&'static str),
}

/// One `DescribeReturnShippingLabel` call, as the status and body a client reads.
async fn describe(router: &MultiServiceRouter, job_id: &str) -> (u16, Value) {
    call_in_region(
        router,
        SIGNED_REGION,
        "DescribeReturnShippingLabel",
        json!({ "JobId": job_id }),
    )
    .await
}

/// Assert `job_id` is answered as `expected` — and, whatever the answer, that it is a
/// modelled one rather than the scaffold channel's 501.
async fn assert_answered(
    router: &MultiServiceRouter,
    job_id: &str,
    expected: &Expected,
    case: &str,
) {
    let (status, body) = describe(router, job_id).await;
    let wire_type = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        status != 501 && !wire_type.contains(SCAFFOLD_ERROR_CODE),
        "case {case:?}: DescribeReturnShippingLabel answered the unimplemented-scaffold \
         channel, got {status} with {body}"
    );

    match expected {
        Expected::Reported {
            status: expected_status,
            has_label_file,
        } => {
            assert_eq!(
                status, 200,
                "case {case:?}: expected a reported label, got {status} with {body}"
            );
            let reported = string(&body, "Status", case);
            assert!(
                SHIPPING_LABEL_STATUS_VALUES.contains(&reported.as_str()),
                "case {case:?}: expected a Status from the modelled catalog \
                 {SHIPPING_LABEL_STATUS_VALUES:?}, got {reported:?}"
            );
            assert_eq!(
                reported, *expected_status,
                "case {case:?}: expected Status {expected_status:?}, got {body}"
            );
            assert_eq!(
                body.get("ReturnShippingLabelURI").is_some(),
                *has_label_file,
                "case {case:?}: expected a label URI present={has_label_file}, got {body}"
            );
            assert_eq!(
                body.get("ExpirationDate").is_some(),
                *has_label_file,
                "case {case:?}: expected an expiry present={has_label_file}, got {body}"
            );
        }
        Expected::Refused(error_code) => {
            assert_eq!(
                status, 400,
                "case {case:?}: expected a refusal, got {status} with {body}"
            );
            assert!(
                wire_type.contains(error_code),
                "case {case:?}: expected a {error_code}, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn describe_return_shipping_label_answers_a_modelled_response_for_every_input_class() {
    let router = super::job_lifecycle::router();
    let mut rows: Vec<(&str, String, Expected)> = Vec::new();

    // A job no record answers to, under each prefix the modelled `JobId` pattern admits
    // (`^(M|J)ID[0-9a-f]{8}-...$`).
    rows.push((
        "a JID no job answers to",
        "JID11111111-1111-1111-1111-111111111111".to_string(),
        Expected::Refused("InvalidResourceException"),
    ));
    rows.push((
        "an MID no job answers to",
        "MID11111111-1111-1111-1111-111111111111".to_string(),
        Expected::Refused("InvalidResourceException"),
    ));

    // The main job of an export order "aren't associated with any Snowballs"
    // (`JobListEntry$IsMaster`), so it has no device to return and no label of its own.
    let main_export_case = "the main job of an export order";
    let main_export = create_job(
        &router,
        json!({
            "JobType": "EXPORT",
            "SnowballType": "EDGE_S",
            "Resources": { "S3Resources": [{ "BucketArn": "arn:aws:s3:::reports-bucket" }] },
        }),
        main_export_case,
    )
    .await;
    rows.push((
        main_export_case,
        main_export.clone(),
        Expected::Refused("InvalidResourceException"),
    ));

    // A cancelled job never ships a device, so it has none to return.
    let cancelled_case = "a cancelled local-use job";
    let cancelled = create_job(
        &router,
        json!({ "JobType": "LOCAL_USE", "SnowballType": "SNC1_HDD" }),
        cancelled_case,
    )
    .await;
    ok(
        &router,
        "CancelJob",
        json!({ "JobId": cancelled }),
        cancelled_case,
    )
    .await;
    rows.push((
        cancelled_case,
        cancelled,
        Expected::Refused("InvalidJobStateException"),
    ));

    // A job with a returnable device but no label created for it yet.
    let no_label_case = "a job no label was created for";
    let no_label = create_job(
        &router,
        json!({ "JobType": "IMPORT", "SnowballType": "EDGE_CG" }),
        no_label_case,
    )
    .await;
    rows.push((
        no_label_case,
        no_label,
        Expected::Refused("InvalidResourceException"),
    ));

    // A label whose creating task is still running: a status, but no file and no window.
    let in_progress_case = "a label whose task is still running";
    let in_progress = create_job(
        &router,
        json!({ "JobType": "IMPORT", "SnowballType": "EDGE", "ShippingOption": "NEXT_DAY" }),
        in_progress_case,
    )
    .await;
    ok(
        &router,
        "CreateReturnShippingLabel",
        json!({ "JobId": in_progress }),
        in_progress_case,
    )
    .await;
    rows.push((
        in_progress_case,
        in_progress,
        Expected::Reported {
            status: "InProgress",
            has_label_file: false,
        },
    ));

    for (case, job_id, expected) in &rows {
        assert_answered(&router, job_id, expected, case).await;
    }

    // The two classes that need the task to have finished: a plain job and one job part of
    // an export order, which is the job that carries a device.
    let finished_case = "a finished label on a plain job";
    let finished = create_job(
        &router,
        json!({ "JobType": "IMPORT", "SnowballType": "EDGE_C", "ShippingOption": "STANDARD" }),
        finished_case,
    )
    .await;
    let part_case = "a finished label on a job part of an export order";
    let part = label_subject(
        &router,
        json!({
            "JobType": "EXPORT",
            "SnowballType": "EDGE",
            "Resources": { "S3Resources": [{ "BucketArn": "arn:aws:s3:::parts-bucket" }] },
        }),
        true,
        part_case,
    )
    .await;
    for (case, job_id) in [(finished_case, &finished), (part_case, &part)] {
        ok(
            &router,
            "CreateReturnShippingLabel",
            json!({ "JobId": job_id, "ShippingOption": "EXPRESS" }),
            case,
        )
        .await;
    }
    wait_for_the_label();
    for (case, job_id) in [(finished_case, &finished), (part_case, &part)] {
        assert_answered(
            &router,
            job_id,
            &Expected::Reported {
                status: "Succeeded",
                has_label_file: true,
            },
            case,
        )
        .await;
    }

    // The main job of the export order the job part belongs to still has no label of its
    // own: the job part's label is not reported for it.
    assert!(
        !job_parts(&router, &main_export, main_export_case)
            .await
            .is_empty(),
        "case {main_export_case:?}: expected the export create to mint job parts"
    );
    assert_answered(
        &router,
        &main_export,
        &Expected::Refused("InvalidResourceException"),
        main_export_case,
    )
    .await;
}

/// One labelled job: the shape it was ordered in and the speed its return travels at.
struct Labelled {
    case: &'static str,
    submitted: Value,
    shipping_option: &'static str,
}

/// The labelled jobs the store holds at once, differing along the job type, the device, the
/// job's own shipping speed and the speed its return was asked for.
fn labelled_jobs() -> Vec<Labelled> {
    vec![
        Labelled {
            case: "an import job on an Edge, returned NEXT_DAY",
            submitted: json!({
                "JobType": "IMPORT",
                "SnowballType": "EDGE",
                "ShippingOption": "SECOND_DAY",
            }),
            shipping_option: "NEXT_DAY",
        },
        Labelled {
            case: "a local-use job on a Snowcone SSD, returned EXPRESS",
            submitted: json!({
                "JobType": "LOCAL_USE",
                "SnowballType": "SNC1_SSD",
                "RemoteManagement": "INSTALLED_AUTOSTART",
            }),
            shipping_option: "EXPRESS",
        },
        Labelled {
            case: "an import job on a storage-optimized device, returned STANDARD",
            submitted: json!({
                "JobType": "IMPORT",
                "SnowballType": "EDGE_S",
                "ShippingOption": "EXPRESS",
            }),
            shipping_option: "STANDARD",
        },
        Labelled {
            case: "an import job on a compute-optimized device, returned SECOND_DAY",
            submitted: json!({ "JobType": "IMPORT", "SnowballType": "EDGE_C" }),
            shipping_option: "SECOND_DAY",
        },
    ]
}

#[tokio::test]
async fn every_job_is_reported_the_label_stored_against_its_own_id() {
    let router = super::job_lifecycle::router();
    let rows = labelled_jobs();

    // Every label exists at once, so a handler reporting anything but the record stored
    // against the requested JobId answers another job's label.
    let mut jobs: Vec<(&'static str, String, &'static str)> = Vec::new();
    for row in &rows {
        let job_id = create_job(&router, row.submitted.clone(), row.case).await;
        ok(
            &router,
            "CreateReturnShippingLabel",
            json!({ "JobId": job_id, "ShippingOption": row.shipping_option }),
            row.case,
        )
        .await;
        jobs.push((row.case, job_id, row.shipping_option));
    }
    wait_for_the_label();

    // Read them back in the reverse of the order they were created in, so a handler that
    // reports the most recent label — or the first one it holds — cannot pass.
    let mut tokens: BTreeSet<String> = BTreeSet::new();
    let mut first_read: Vec<Value> = Vec::new();
    for (case, job_id, shipping_option) in jobs.iter().rev() {
        let reported = ok(
            &router,
            "DescribeReturnShippingLabel",
            json!({ "JobId": job_id }),
            case,
        )
        .await;
        let uri = string(&reported, "ReturnShippingLabelURI", case);
        let url = parse_label_url(&uri, case);
        assert_eq!(
            url.path,
            format!("{job_id}/{RETURN_SHIPPING_LABEL_OBJECT}"),
            "case {case:?}: expected the URI to address {job_id}'s own label file, got {uri:?}"
        );
        assert_eq!(
            url.query.get("shippingOption").map(String::as_str),
            Some(*shipping_option),
            "case {case:?}: expected the label to travel at {shipping_option:?}, got {uri:?}"
        );
        let token = parameter(&url, "labelToken", case);
        assert!(
            tokens.insert(token.clone()),
            "case {case:?}: the label of {job_id} carries token {token}, which another job's \
             label already carries"
        );
        first_read.push(reported);
    }

    // And reading them all again answers exactly the same labels: reporting one job's label
    // changes nothing another caller — or another job — later reads.
    for ((case, job_id, _), expected) in jobs.iter().rev().zip(first_read.iter()) {
        let reported = ok(
            &router,
            "DescribeReturnShippingLabel",
            json!({ "JobId": job_id }),
            case,
        )
        .await;
        assert_eq!(
            &reported, expected,
            "case {case:?}: the second read of {job_id}'s label answered something else"
        );
    }
}
