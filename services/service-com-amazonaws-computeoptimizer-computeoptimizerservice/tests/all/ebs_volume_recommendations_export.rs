// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ExportEBSVolumeRecommendations`.
//!
//! Every call goes through the same `MultiServiceRouter` dispatch a client uses, so what is
//! asserted is what a client sees. The RULES asserted here are the ones this export shares
//! with every other `Export*Recommendations` operation of the service — it RECORDS one job
//! for the Region its endpoint serves, answers that job's id and the destination keys derived
//! from the request's bucket and key prefix, the account and the resource type, the job's
//! `JobStatus` then walks the lifecycle the clock drives, and only one volume export job runs
//! per Region at a time — plus the ones specific to it: the job carries the `EbsVolume`
//! resource type, and the request is refused for every input rule the operation documents,
//! over the same `EBSFilters` vocabulary `GetEBSVolumeRecommendations` selects with. Each is
//! checked over a TABLE of inputs rather than one call.
//!
//! The reader of what these calls record, `DescribeRecommendationExportJobs`, has its own
//! suite (`recommendation_export_jobs.rs`); it is used here only to read a created job back.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    EXPORT_JOB_STEP, JOB_STATUS_COMPLETE, JOB_STATUS_IN_PROGRESS, JOB_STATUS_QUEUED,
    RESOURCE_TYPE_EBS_VOLUME,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, call_in_region, create_bucket, emulator, error_code, opt_in, ACCOUNT};

/// The operation under test.
const EXPORT: &str = "ExportEBSVolumeRecommendations";

/// The destination bucket these fixtures export to. Compute Optimizer does not create it;
/// the request just names it.
const BUCKET: &str = "ebs-recommendation-exports";

/// Enough slack over one lifecycle step for the clock to have made it due whatever the test
/// scheduler is doing; the step itself is the service's own ([`EXPORT_JOB_STEP`]).
const SLACK: Duration = Duration::from_millis(120);

/// An emulator with the account opted in, which every export call requires.
async fn enrolled_emulator() -> MultiServiceRouter {
    let router = emulator();
    create_bucket(&router, BUCKET).await;
    opt_in(&router).await;
    router
}

/// Export the account's Amazon EBS volume recommendations, asserting the call was accepted,
/// and answer the whole response body.
async fn export_ok(router: &MultiServiceRouter, case: &str, request: Value) -> Value {
    let (status, body) = call(router, EXPORT, request.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {request} to be accepted, got {status}: {body}"
    );
    body
}

/// Export with the default destination and answer the job id.
async fn export_job_id(router: &MultiServiceRouter, case: &str) -> String {
    let body = export_ok(
        router,
        case,
        json!({"s3DestinationConfig": {"bucket": BUCKET}}),
    )
    .await;
    body["jobId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: an accepted export answers a jobId, got {body}"))
        .to_string()
}

/// Read the export jobs of the request's Region back, asserting the read was accepted.
async fn describe_ok(router: &MultiServiceRouter, case: &str, request: Value) -> Value {
    let (status, body) = call(router, "DescribeRecommendationExportJobs", request.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {request} to be accepted, got {status}: {body}"
    );
    body
}

/// The job ids a `DescribeRecommendationExportJobs` body reports, in the order it reported
/// them.
fn reported_ids(body: &Value) -> Vec<String> {
    body["recommendationExportJobs"]
        .as_array()
        .unwrap_or_else(|| panic!("the reader reports a job list, got {body}"))
        .iter()
        .map(|job| job["jobId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The one job the reader reports for `job_id`.
fn reported_job(case: &str, body: &Value, job_id: &str) -> Value {
    body["recommendationExportJobs"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the reader reports a job list, got {body}"))
        .iter()
        .find(|job| job["jobId"] == json!(job_id))
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no job reported for {job_id}, got {body}"))
}

/// The destination keys an export answers are derived from the bucket and key prefix the
/// request named, the account the export is for, the resource type and the job id — one row
/// per form a key prefix can take, including none at all — and the job the call recorded
/// names exactly the objects the call answered.
#[tokio::test]
async fn the_destination_is_derived_from_the_request_and_the_minted_job_id() {
    let account = engine::emulator_account_id();

    // (case, the `keyPrefix` the request carries, the prefix the keys are expected under)
    let rows: Vec<(&str, Option<&str>, &str)> = vec![
        ("no key prefix at all", None, ""),
        ("an empty key prefix", Some(""), ""),
        ("a single-segment prefix", Some("exports"), "exports/"),
        (
            "a prefix with a trailing slash",
            Some("exports/"),
            "exports/",
        ),
        (
            "a prefix with leading and trailing slashes",
            Some("/nested/volume-exports/"),
            "nested/volume-exports/",
        ),
        ("a multi-segment prefix", Some("a/b/c"), "a/b/c/"),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (case, key_prefix, expected_prefix) in rows {
        // A fresh emulator per row: one volume export job may be in progress per Region at a
        // time, so each row gets its own store rather than waiting on the previous row's job.
        let router = enrolled_emulator().await;
        let mut destination_config = json!({"bucket": BUCKET});
        if let Some(key_prefix) = key_prefix {
            destination_config["keyPrefix"] = json!(key_prefix);
        }
        let body = export_ok(
            &router,
            case,
            json!({"s3DestinationConfig": destination_config}),
        )
        .await;

        let job_id = body["jobId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a jobId, got {body}"))
            .to_string();
        // A `JobId` is an opaque string the service mints; this emulator hands out a
        // UUID-shaped one, which is what AWS's own job ids look like.
        assert_eq!(
            job_id.len(),
            36,
            "case {case:?}: expected a 36-character job id, got {job_id:?}"
        );
        assert!(
            job_id
                .char_indices()
                .all(|(at, character)| if matches!(at, 8 | 13 | 18 | 23) {
                    character == '-'
                } else {
                    character.is_ascii_hexdigit()
                }),
            "case {case:?}: expected a UUID-shaped job id, got {job_id:?}"
        );
        assert!(
            !minted.contains(&job_id),
            "case {case:?}: every export job gets its own id, got {job_id} twice"
        );
        minted.push(job_id.clone());

        let stem = format!(
            "{expected_prefix}{account}/ComputeOptimizer/{RESOURCE_TYPE_EBS_VOLUME}/{job_id}"
        );
        assert_eq!(
            body["s3Destination"],
            json!({
                "bucket": BUCKET,
                "key": format!("{stem}.csv"),
                "metadataKey": format!("{stem}-metadata.json"),
            }),
            "case {case:?}: unexpected destination, got {body}"
        );

        // The job the call recorded names exactly the objects the call answered, and carries
        // the resource type whose recommendations were exported.
        let listed = describe_ok(&router, case, json!({})).await;
        assert_eq!(
            reported_ids(&listed),
            vec![job_id.clone()],
            "case {case:?}: the export records exactly one job, got {listed}"
        );
        let job = reported_job(case, &listed, &job_id);
        assert_eq!(
            job["destination"],
            json!({"s3": body["s3Destination"].clone()}),
            "case {case:?}: the reported destination must be the one the export answered, \
             got {job}"
        );
        assert_eq!(
            job["resourceType"].as_str(),
            Some(RESOURCE_TYPE_EBS_VOLUME),
            "case {case:?}: a volume recommendations export is of {RESOURCE_TYPE_EBS_VOLUME}, \
             got {job}"
        );
        assert!(
            job["failureReason"].is_null(),
            "case {case:?}: a job that did not fail carries no failure reason, got {job}"
        );
    }
}

/// The job a volume export records walks the lifecycle the clock owes: `Queued` while the
/// caller can still see it queued, then `InProgress`, then the terminal `Complete`, which
/// never moves again. Reading it never moves it.
#[tokio::test]
async fn the_recorded_job_status_walks_the_lifecycle() {
    let router = enrolled_emulator().await;
    let job_id = export_job_id(&router, "a job to follow").await;

    let created = reported_job(
        "straight after the export",
        &describe_ok(&router, "straight after the export", json!({})).await,
        &job_id,
    );
    assert_eq!(
        created["status"].as_str(),
        Some(JOB_STATUS_QUEUED),
        "a job the clock has not moved yet is Queued, got {created}"
    );
    let created_at = created["creationTimestamp"]
        .as_f64()
        .unwrap_or_else(|| panic!("a reported job carries a creation timestamp, got {created}"));
    assert_eq!(
        created["lastUpdatedTimestamp"].as_f64(),
        Some(created_at),
        "a job that has not changed yet was last updated when it was created, got {created}"
    );

    // (case, how long to wait before reading, the status expected then)
    let steps: Vec<(&str, Duration, &str)> = vec![
        (
            "one step in",
            EXPORT_JOB_STEP + SLACK,
            JOB_STATUS_IN_PROGRESS,
        ),
        ("two steps in", EXPORT_JOB_STEP, JOB_STATUS_COMPLETE),
        (
            "well past the last step",
            2 * EXPORT_JOB_STEP,
            JOB_STATUS_COMPLETE,
        ),
    ];
    for (case, wait, expected) in steps {
        tokio::time::sleep(wait).await;
        let job = reported_job(case, &describe_ok(&router, case, json!({})).await, &job_id);
        assert_eq!(
            job["status"].as_str(),
            Some(expected),
            "case {case:?}: expected status {expected}, got {job}"
        );
        assert_eq!(
            job["creationTimestamp"].as_f64(),
            Some(created_at),
            "case {case:?}: the creation timestamp never changes, got {job}"
        );
        // A read reports the status the clock has made due and writes nothing, so the same
        // read twice answers the same job.
        let again = reported_job(case, &describe_ok(&router, case, json!({})).await, &job_id);
        assert_eq!(
            job, again,
            "case {case:?}: a read must not change what a later caller sees"
        );
    }
}

/// "You can have only one Amazon EBS volume export job in progress per Amazon Web Services
/// Region": a second export is refused while the first is running, admitted once it has
/// completed, and never blocked by a job of another Region.
#[tokio::test]
async fn only_one_volume_export_job_runs_per_region_at_a_time() {
    let router = enrolled_emulator().await;
    let first = export_job_id(&router, "the first export").await;

    let (status, body) = call(
        &router,
        EXPORT,
        json!({"s3DestinationConfig": {"bucket": BUCKET}}),
    )
    .await;
    assert_eq!(
        status, 400,
        "a second export while one is in progress is refused, got {status}: {body}"
    );
    assert!(
        error_code(&body).contains("LimitExceededException"),
        "expected LimitExceededException, got {body}"
    );
    assert_eq!(
        reported_ids(&describe_ok(&router, "after the refusal", json!({})).await),
        vec![first.clone()],
        "a refused export records no job"
    );

    // Another Region's endpoint has its own in-progress job: the limit is per Region.
    let (status, body) = call_in_region(
        &router,
        "eu-west-1",
        EXPORT,
        json!({"s3DestinationConfig": {"bucket": BUCKET}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "an export at another Region's endpoint is not blocked, got {status}: {body}"
    );
    assert_eq!(
        reported_ids(&describe_ok(&router, "the home Region", json!({})).await),
        vec![first.clone()],
        "a Region reports only its own export jobs"
    );

    // Once the first job reaches its terminal status it is no longer in progress, so the
    // next export is admitted.
    tokio::time::sleep(2 * EXPORT_JOB_STEP + SLACK).await;
    let second = export_job_id(&router, "after the first completed").await;
    assert_ne!(
        second, first,
        "every export job gets its own id, got {second} twice"
    );
    assert_eq!(
        reported_ids(&describe_ok(&router, "both jobs", json!({})).await),
        vec![first, second],
        "the reader reports the jobs in creation order"
    );
}

/// An export job of ANOTHER resource type never blocks a volume export, and the two are
/// reported side by side: the one-job-in-progress limit counts the jobs of the resource type
/// being exported.
#[tokio::test]
async fn a_job_of_another_resource_type_does_not_block_a_volume_export() {
    let router = enrolled_emulator().await;
    let (status, body) = call(
        &router,
        "ExportRDSDatabaseRecommendations",
        json!({"s3DestinationConfig": {"bucket": BUCKET}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "a database export is accepted, got {status}: {body}"
    );
    let database_job = body["jobId"]
        .as_str()
        .unwrap_or_else(|| panic!("an accepted export answers a jobId, got {body}"))
        .to_string();

    let volume_job = export_job_id(&router, "a volume export beside a database one").await;
    assert_eq!(
        reported_ids(&describe_ok(&router, "both resource types", json!({})).await),
        vec![database_job, volume_job.clone()],
        "a job of another resource type does not block a volume export"
    );

    // And the volume job is the one a `ResourceType` filter for volumes selects.
    let listed = describe_ok(
        &router,
        "filtered to volumes",
        json!({"filters": [{"name": "ResourceType", "values": [RESOURCE_TYPE_EBS_VOLUME]}]}),
    )
    .await;
    assert_eq!(
        reported_ids(&listed),
        vec![volume_job],
        "only the volume export job carries {RESOURCE_TYPE_EBS_VOLUME}, got {listed}"
    );
}

/// An export request outside the service's documented input rules is refused with HTTP 400
/// and records no job — one row per rule, each on a value the rule rejects — and a request
/// that carries every member at a value the model declares is accepted.
#[tokio::test]
async fn an_invalid_export_request_is_refused() {
    let router = enrolled_emulator().await;

    // (case, request, the wire error code expected)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            // "You must create the destination Amazon S3 bucket ... Compute Optimizer does
            // not create the S3 bucket for you."
            "a destination naming no bucket",
            json!({"s3DestinationConfig": {}}),
            "InvalidParameterValueException",
        ),
        (
            "a destination whose bucket is blank",
            json!({"s3DestinationConfig": {"bucket": "   "}}),
            "InvalidParameterValueException",
        ),
        (
            // "This parameter cannot be specified together with the include member accounts
            // parameter. The parameters are mutually exclusive."
            "accountIds together with includeMemberAccounts",
            json!({
                "s3DestinationConfig": {"bucket": BUCKET},
                "accountIds": [ACCOUNT],
                "includeMemberAccounts": true,
            }),
            "InvalidParameterValueException",
        ),
        (
            "an account id that is not 12 digits",
            json!({"s3DestinationConfig": {"bucket": BUCKET}, "accountIds": ["12345"]}),
            "InvalidParameterValueException",
        ),
        (
            "an account id that is not digits at all",
            json!({"s3DestinationConfig": {"bucket": BUCKET}, "accountIds": ["abcdefghijkl"]}),
            "InvalidParameterValueException",
        ),
        (
            // "The valid values are `Optimized`, or `NotOptimized`" (`EBSFilter$values`).
            "a finding outside the EBSFinding value set",
            json!({
                "s3DestinationConfig": {"bucket": BUCKET},
                "filters": [{"name": "Finding", "values": ["Underprovisioned"]}],
            }),
            "InvalidParameterValueException",
        ),
        (
            "one valid finding beside one the model does not declare",
            json!({
                "s3DestinationConfig": {"bucket": BUCKET},
                "filters": [{"name": "Finding", "values": ["Optimized", "Enormous"]}],
            }),
            "InvalidParameterValueException",
        ),
        (
            "a filter carrying no name",
            json!({
                "s3DestinationConfig": {"bucket": BUCKET},
                "filters": [{"values": ["Optimized"]}],
            }),
            "InvalidParameterValueException",
        ),
        (
            // `EBSFilterName` is a modelled enum whose only value is Finding, so the request
            // boundary refuses every other name — including the `tag:key` and `tag-key`
            // names `EBSFilter$name`'s prose mentions — before the handler runs.
            "a filter name the model does not declare",
            json!({
                "s3DestinationConfig": {"bucket": BUCKET},
                "filters": [{"name": "tag:Owner", "values": ["TeamA"]}],
            }),
            "InvalidParameterValueException",
        ),
        (
            // `FileFormat` is a modelled enum whose only value is Csv.
            "a file format the model does not declare",
            json!({"s3DestinationConfig": {"bucket": BUCKET}, "fileFormat": "Parquet"}),
            "InvalidParameterValueException",
        ),
        (
            // Likewise `ExportableVolumeField`.
            "a field to export the model does not declare",
            json!({
                "s3DestinationConfig": {"bucket": BUCKET},
                "fieldsToExport": ["VolumeArn", "NotAField"],
            }),
            "InvalidParameterValueException",
        ),
    ];

    for (case, request, wanted_code) in rows {
        let (status, body) = call(&router, EXPORT, request.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {request} to be refused, got {status}: {body}"
        );
        assert!(
            error_code(&body).contains(wanted_code),
            "case {case:?}: expected {wanted_code}, got {body}"
        );
        assert!(
            body["jobId"].is_null(),
            "case {case:?}: a refused export answers no job id, got {body}"
        );
        assert_eq!(
            reported_ids(&describe_ok(&router, case, json!({})).await),
            Vec::<String>::new(),
            "case {case:?}: a refused export records no job"
        );
    }

    // Every field a valid request may carry together, accepted: the rows above refuse a
    // VALUE, never the member. The exportable fields are `ExportableVolumeField` values the
    // model declares (`smithy_query query="shape" shape="ExportableVolumeField"`), including
    // ones no row above cites.
    let body = export_ok(
        &router,
        "every member set to a value the model declares",
        json!({
            "s3DestinationConfig": {"bucket": BUCKET, "keyPrefix": "quarterly"},
            "accountIds": [ACCOUNT],
            "includeMemberAccounts": false,
            "fileFormat": "Csv",
            "fieldsToExport": ["AccountId", "VolumeArn", "Finding",
                               "CurrentConfigurationVolumeType", "LookbackPeriodInDays",
                               "RecommendationOptionsMonthlyPrice", "RootVolume", "Tags"],
            "filters": [{"name": "Finding", "values": ["Optimized", "NotOptimized"]}],
        }),
    )
    .await;
    assert!(
        body["jobId"].is_string(),
        "an accepted export answers a job id, got {body}"
    );
}

/// The enrollment gate: "The account is not opted in to Compute Optimizer" is what the export
/// answers while the account is not enrolled, before and after opting back out, and no job is
/// recorded either time.
#[tokio::test]
async fn an_export_is_refused_while_the_account_is_not_opted_in() {
    let router = emulator();
    create_bucket(&router, BUCKET).await;
    let request = json!({"s3DestinationConfig": {"bucket": BUCKET}});

    for case in ["before enrolling", "after opting back out"] {
        let (status, body) = call(&router, EXPORT, request.clone()).await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected 403 while not opted in, got {status}: {body}"
        );
        assert!(
            error_code(&body).contains("OptInRequiredException"),
            "case {case:?}: expected OptInRequiredException, got {body}"
        );

        if case == "before enrolling" {
            // Enrolling is what admits the export; the store the refused call left alone is
            // still empty, and the export then records its one job.
            opt_in(&router).await;
            assert_eq!(
                reported_ids(&describe_ok(&router, case, json!({})).await),
                Vec::<String>::new(),
                "case {case:?}: a refused export records no job"
            );
            let job_id = export_job_id(&router, case).await;
            assert_eq!(
                reported_ids(&describe_ok(&router, case, json!({})).await),
                vec![job_id],
                "case {case:?}: the export recorded its job once the account was enrolled"
            );

            let (status, body) = call(
                &router,
                "UpdateEnrollmentStatus",
                json!({"status": "Inactive", "includeMemberAccounts": false}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: opting back out: got {status}: {body}"
            );
        }
    }
}

/// The same `EBSFilters` vocabulary gates the READER and the EXPORT: a filter either
/// operation admits, the other admits too, and one either refuses, the other refuses too.
#[tokio::test]
async fn the_export_and_the_reader_admit_the_same_filters() {
    // (case, the filters, whether both operations accept them)
    let rows: Vec<(&str, Value, bool)> = vec![
        ("no filters at all", json!([]), true),
        (
            "the one filter name the model declares",
            json!([{"name": "Finding", "values": ["NotOptimized"]}]),
            true,
        ),
        (
            "both findings at once",
            json!([{"name": "Finding", "values": ["Optimized", "NotOptimized"]}]),
            true,
        ),
        (
            "a filter carrying no value, which selects nothing",
            json!([{"name": "Finding", "values": []}]),
            true,
        ),
        (
            "two filters of the same name",
            json!([{"name": "Finding", "values": ["Optimized"]},
                   {"name": "Finding", "values": ["NotOptimized"]}]),
            true,
        ),
        (
            "a finding the model does not declare",
            json!([{"name": "Finding", "values": ["Overprovisioned"]}]),
            false,
        ),
        (
            "a filter carrying no name",
            json!([{"values": ["Optimized"]}]),
            false,
        ),
    ];

    for (case, filters, accepted) in rows {
        // A fresh emulator per row: only one volume export job runs per Region at a time.
        let router = enrolled_emulator().await;
        let (read_status, read_body) = call(
            &router,
            "GetEBSVolumeRecommendations",
            json!({"filters": filters.clone()}),
        )
        .await;
        let (export_status, export_body) = call(
            &router,
            EXPORT,
            json!({"s3DestinationConfig": {"bucket": BUCKET}, "filters": filters.clone()}),
        )
        .await;
        let wanted = if accepted { 200 } else { 400 };
        assert_eq!(
            read_status, wanted,
            "case {case:?}: the reader answered {read_status} for {filters}: {read_body}"
        );
        assert_eq!(
            export_status, wanted,
            "case {case:?}: the export answered {export_status} for {filters}: {export_body}"
        );
        if !accepted {
            assert!(
                error_code(&read_body).contains("InvalidParameterValueException"),
                "case {case:?}: the reader refuses with InvalidParameterValueException, \
                 got {read_body}"
            );
            assert!(
                error_code(&export_body).contains("InvalidParameterValueException"),
                "case {case:?}: the export refuses with InvalidParameterValueException, \
                 got {export_body}"
            );
        }
    }
}
