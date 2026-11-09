// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ExportIdleRecommendations` and the reader of what it creates,
//! `DescribeRecommendationExportJobs`.
//!
//! Every assertion drives the service the way a client does — an awsJson1_0 `POST /`
//! through the same `MultiServiceRouter` the emulator binary mounts — so a response that is
//! not computed from persisted state fails here.
//!
//! What is asserted is the RULE, over a table of inputs rather than one call: an export
//! call RECORDS a job for the Region its endpoint serves, answers that job's id and the
//! destination keys derived from the request's bucket and key prefix, the account, the
//! resource type and the job id; the job's `JobStatus` then walks `Queued` -> `InProgress`
//! -> `Complete` as the clock makes each step due; a second export of the same resource
//! type in the same Region while one is in progress is refused; and the reader selects over
//! the stored jobs by id and by the model's `JobFilter` vocabulary, pages at `maxResults`
//! and reports only the jobs of its own Region.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::{
    DEFAULT_PAGE_SIZE, EXPORT_JOB_STEP, JOB_STATUS_COMPLETE, JOB_STATUS_IN_PROGRESS,
    JOB_STATUS_QUEUED, RESOURCE_TYPE_IDLE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, call_in_region, create_bucket, emulator, error_code, opt_in, ACCOUNT,
};

/// The destination bucket these fixtures export to. Compute Optimizer does not create it;
/// the request just names it.
const BUCKET: &str = "recommendation-exports";

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

/// Wait until the clock has moved a job created `now` all the way to its terminal status.
async fn wait_for_completion() {
    tokio::time::sleep(2 * EXPORT_JOB_STEP + SLACK).await;
}

/// Export the account's idle resource recommendations, asserting the call was accepted, and
/// answer the whole response body.
async fn export_ok(router: &MultiServiceRouter, case: &str, request: Value) -> Value {
    let (status, body) = call(router, "ExportIdleRecommendations", request.clone()).await;
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

/// Describe the export jobs of `REGION`, asserting the read was accepted.
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

/// The one job `body` reports, or a panic naming what it reported instead.
fn only_job(case: &str, body: &Value) -> Value {
    let jobs = body["recommendationExportJobs"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the reader reports a job list, got {body}"));
    assert_eq!(
        jobs.len(),
        1,
        "case {case:?}: expected exactly one job, got {body}"
    );
    jobs[0].clone()
}

/// The destination keys an export answers are derived from the bucket and key prefix the
/// request named, the account the export is for, the resource type and the job id — one row
/// per form a key prefix can take, including none at all.
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
            Some("/nested/exports/"),
            "nested/exports/",
        ),
        ("a multi-segment prefix", Some("a/b/c"), "a/b/c/"),
    ];

    for (case, key_prefix, expected_prefix) in rows {
        // A fresh emulator per row: one export job may be in progress per Region at a time,
        // so each row gets its own store rather than waiting on the previous row's job.
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
            .unwrap_or_else(|| panic!("case {case:?}: expected a jobId, got {body}"));
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

        let stem = format!("{expected_prefix}{account}/ComputeOptimizer/{RESOURCE_TYPE_IDLE}/{job_id}");
        assert_eq!(
            body["s3Destination"],
            json!({
                "bucket": BUCKET,
                "key": format!("{stem}.csv"),
                "metadataKey": format!("{stem}-metadata.json"),
            }),
            "case {case:?}: unexpected destination, got {body}"
        );

        // The job the call recorded names exactly the objects the call answered.
        let reported = only_job(case, &describe_ok(&router, case, json!({})).await);
        assert_eq!(
            reported["destination"],
            json!({"s3": body["s3Destination"].clone()}),
            "case {case:?}: the reported destination must be the one the export answered, \
             got {reported}"
        );
    }
}

/// A created job is reported member for member, and its status walks the lifecycle the
/// clock owes: `Queued` while the caller can still see it queued, then `InProgress`, then
/// the terminal `Complete`, which never moves again.
#[tokio::test]
async fn a_created_job_is_reported_and_its_status_walks_the_lifecycle() {
    let router = enrolled_emulator().await;
    let job_id = export_job_id(&router, "a job to follow").await;

    let queued = only_job(
        "straight after the export",
        &describe_ok(&router, "straight after the export", json!({})).await,
    );
    assert_eq!(
        queued["jobId"].as_str(),
        Some(job_id.as_str()),
        "the reader reports the job the export minted, got {queued}"
    );
    assert_eq!(
        queued["resourceType"].as_str(),
        Some(RESOURCE_TYPE_IDLE),
        "an idle resource export carries the Idle resource type, got {queued}"
    );
    assert_eq!(
        queued["status"].as_str(),
        Some(JOB_STATUS_QUEUED),
        "a job the clock has not moved yet is Queued, got {queued}"
    );
    assert!(
        queued["failureReason"].is_null(),
        "a job that did not fail carries no failure reason, got {queued}"
    );
    let created_at = queued["creationTimestamp"]
        .as_f64()
        .unwrap_or_else(|| panic!("a reported job carries a creation timestamp, got {queued}"));
    assert_eq!(
        queued["lastUpdatedTimestamp"].as_f64(),
        Some(created_at),
        "a job that has not changed yet was last updated when it was created, got {queued}"
    );

    // (case, how long to wait before reading, the status expected then, whether the status
    // changed at this step — a terminal job stops being updated)
    let steps: Vec<(&str, Duration, &str, bool)> = vec![
        (
            "one step in",
            EXPORT_JOB_STEP + SLACK,
            JOB_STATUS_IN_PROGRESS,
            true,
        ),
        ("two steps in", EXPORT_JOB_STEP, JOB_STATUS_COMPLETE, true),
        (
            "well past the last step",
            2 * EXPORT_JOB_STEP,
            JOB_STATUS_COMPLETE,
            false,
        ),
    ];
    let mut previous_update = created_at;
    for (case, wait, expected, moves) in steps {
        tokio::time::sleep(wait).await;
        let job = only_job(case, &describe_ok(&router, case, json!({})).await);
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
        let updated_at = job["lastUpdatedTimestamp"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: expected a last-updated timestamp, got {job}"));
        if moves {
            assert!(
                updated_at > previous_update,
                "case {case:?}: a status change updates the job, expected later than \
                 {previous_update}, got {updated_at}"
            );
        } else {
            assert!(
                (updated_at - previous_update).abs() < f64::EPSILON,
                "case {case:?}: a terminal job stops being updated, expected \
                 {previous_update}, got {updated_at}"
            );
        }
        previous_update = updated_at;
    }
}

/// "You can have only one idle resource export job in progress per Amazon Web Services
/// Region": a second export is refused while the first is running, admitted once it has
/// completed, and never blocked by a job of another Region.
#[tokio::test]
async fn only_one_export_job_runs_per_region_at_a_time() {
    let router = enrolled_emulator().await;
    let first = export_job_id(&router, "the first export").await;

    let (status, body) = call(
        &router,
        "ExportIdleRecommendations",
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
        "ExportIdleRecommendations",
        json!({"s3DestinationConfig": {"bucket": BUCKET}}),
    )
    .await;
    assert_eq!(
        status, 200,
        "an export at another Region's endpoint is not blocked, got {status}: {body}"
    );
    let elsewhere = body["jobId"].as_str().expect("a jobId").to_string();
    assert_eq!(
        reported_ids(&describe_ok(&router, "the home Region", json!({})).await),
        vec![first.clone()],
        "a Region reports only its own export jobs"
    );
    let (status, body) = call_in_region(
        &router,
        "eu-west-1",
        "DescribeRecommendationExportJobs",
        json!({}),
    )
    .await;
    assert_eq!(status, 200, "reading another Region: got {status}: {body}");
    assert_eq!(
        reported_ids(&body),
        vec![elsewhere],
        "the other Region reports only the job created there"
    );

    // Once the first job reaches its terminal status it is no longer in progress, so the
    // next export is admitted.
    wait_for_completion().await;
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

/// An export request outside the service's documented input rules is refused with HTTP 400
/// and records no job — one row per rule, each on a value the rule rejects.
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
            // "This parameter can't be specified together with the include member accounts
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
            "a finding value outside the IdleFinding value set",
            json!({
                "s3DestinationConfig": {"bucket": BUCKET},
                "filters": [{"name": "Finding", "values": ["Oversized"]}],
            }),
            "InvalidParameterValueException",
        ),
        (
            "a resource type outside the IdleRecommendationResourceType value set",
            json!({
                "s3DestinationConfig": {"bucket": BUCKET},
                "filters": [{"name": "ResourceType", "values": ["LambdaFunction"]}],
            }),
            "InvalidParameterValueException",
        ),
        (
            "a filter carrying no name",
            json!({
                "s3DestinationConfig": {"bucket": BUCKET},
                "filters": [{"values": ["Idle"]}],
            }),
            "InvalidParameterValueException",
        ),
        (
            // `FileFormat` is a modelled enum whose only value is Csv, so the request
            // boundary refuses anything else before the handler runs.
            "a file format the model does not declare",
            json!({"s3DestinationConfig": {"bucket": BUCKET}, "fileFormat": "Parquet"}),
            "InvalidParameterValueException",
        ),
        (
            // Likewise `ExportableIdleField`.
            "a field to export the model does not declare",
            json!({
                "s3DestinationConfig": {"bucket": BUCKET},
                "fieldsToExport": ["ResourceArn", "NotAField"],
            }),
            "InvalidParameterValueException",
        ),
    ];

    for (case, request, wanted_code) in rows {
        let (status, body) = call(&router, "ExportIdleRecommendations", request.clone()).await;
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
    // VALUE, never the member. `accountIds` names the one account this emulator presents —
    // the only account an export of it can be asked for
    // (`recommendation_export_admission.rs`).
    let body = export_ok(
        &router,
        "every member set to a value the model declares",
        json!({
            "s3DestinationConfig": {"bucket": BUCKET, "keyPrefix": "quarterly"},
            "accountIds": [ACCOUNT],
            "includeMemberAccounts": false,
            "fileFormat": "Csv",
            "fieldsToExport": ["AccountId", "ResourceArn", "Finding", "SavingsOpportunity"],
            "filters": [{"name": "Finding", "values": ["Idle"]}],
        }),
    )
    .await;
    assert!(
        body["jobId"].is_string(),
        "an accepted export answers a job id, got {body}"
    );
}

/// The reader selects over the stored jobs by the model's `JobFilter` vocabulary: a filter
/// matches when the member its name selects on equals one of its values, several filters are
/// ANDed, and a filter carrying no values constrains nothing.
#[tokio::test]
async fn the_reader_selects_by_job_filter() {
    let router = enrolled_emulator().await;
    // Two jobs, both walked through to their terminal status, so the table below selects on
    // a status that cannot change under it. The transient statuses are pinned by
    // `a_created_job_is_reported_and_its_status_walks_the_lifecycle`.
    let first = export_job_id(&router, "the first job").await;
    wait_for_completion().await;
    let second = export_job_id(&router, "the second job").await;
    wait_for_completion().await;
    let both = vec![first.clone(), second.clone()];
    let none: Vec<String> = Vec::new();

    // (case, the `filters` the request carries, the jobs expected back)
    let rows: Vec<(&str, Value, Vec<String>)> = vec![
        ("no filter at all", json!([]), both.clone()),
        (
            "the status both jobs have reached",
            json!([{"name": "JobStatus", "values": [JOB_STATUS_COMPLETE]}]),
            both.clone(),
        ),
        (
            "a status no stored job is in",
            json!([{"name": "JobStatus", "values": [JOB_STATUS_QUEUED]}]),
            none.clone(),
        ),
        (
            "several statuses, none of them the jobs'",
            json!([{"name": "JobStatus", "values": [JOB_STATUS_IN_PROGRESS, "Failed"]}]),
            none.clone(),
        ),
        (
            "several statuses, one of them the jobs'",
            json!([{"name": "JobStatus", "values": ["Failed", JOB_STATUS_COMPLETE]}]),
            both.clone(),
        ),
        (
            "the resource type both jobs carry",
            json!([{"name": "ResourceType", "values": [RESOURCE_TYPE_IDLE]}]),
            both.clone(),
        ),
        (
            "a resource type no stored job carries",
            json!([{"name": "ResourceType", "values": ["Ec2Instance"]}]),
            none.clone(),
        ),
        (
            "two filters both jobs satisfy",
            json!([
                {"name": "ResourceType", "values": [RESOURCE_TYPE_IDLE]},
                {"name": "JobStatus", "values": [JOB_STATUS_COMPLETE]},
            ]),
            both.clone(),
        ),
        (
            "two filters of which only one is satisfied",
            json!([
                {"name": "ResourceType", "values": [RESOURCE_TYPE_IDLE]},
                {"name": "JobStatus", "values": ["Failed"]},
            ]),
            none.clone(),
        ),
        (
            "a filter carrying no values",
            json!([{"name": "JobStatus", "values": []}]),
            both.clone(),
        ),
    ];

    for (case, filters, expected) in rows {
        let body = describe_ok(&router, case, json!({"filters": filters.clone()})).await;
        assert_eq!(
            reported_ids(&body),
            expected,
            "case {case:?}: filters {filters} selected the wrong jobs, got {body}"
        );
    }

    // `jobIds` selects by identity, and names a job that must exist.
    let rows: Vec<(&str, Value, Vec<String>)> = vec![
        ("one id", json!([first.clone()]), vec![first.clone()]),
        ("the other id", json!([second.clone()]), vec![second.clone()]),
        ("both ids", json!([first.clone(), second.clone()]), both),
        (
            "both ids, the other way round",
            json!([second.clone(), first.clone()]),
            // Selection does not reorder: the reader reports creation order.
            vec![first.clone(), second.clone()],
        ),
    ];
    for (case, job_ids, expected) in rows {
        let body = describe_ok(&router, case, json!({"jobIds": job_ids.clone()})).await;
        assert_eq!(
            reported_ids(&body),
            expected,
            "case {case:?}: jobIds {job_ids} selected the wrong jobs, got {body}"
        );
    }

    // A job id this Region holds no job for does not exist.
    for (case, job_ids) in [
        ("an id no job carries", json!(["00000000-0000-4000-8000-000000000000"])),
        (
            "a stored id beside one no job carries",
            json!([first, "00000000-0000-4000-8000-000000000000"]),
        ),
    ] {
        let (status, body) = call(
            &router,
            "DescribeRecommendationExportJobs",
            json!({"jobIds": job_ids.clone()}),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected {job_ids} to be refused, got {status}: {body}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
    }
}

/// Paging walks the stored jobs exactly once, in creation order, and a token resumes only
/// the walk it was issued for.
#[tokio::test]
async fn paging_walks_the_stored_jobs_exactly_once() {
    let router = enrolled_emulator().await;
    let mut created: Vec<String> = Vec::new();
    for index in 0..3 {
        created.push(export_job_id(&router, &format!("job {index}")).await);
        // Only one job may be in progress at a time, so each export waits for the previous
        // job to finish.
        wait_for_completion().await;
    }

    // (case, the page size the request asks for, the page sizes expected)
    let rows: Vec<(&str, Option<i32>, Vec<usize>)> = vec![
        ("no page size at all", None, vec![3]),
        ("one job per page", Some(1), vec![1, 1, 1]),
        ("two jobs per page", Some(2), vec![2, 1]),
        ("more jobs per page than exist", Some(1_000), vec![3]),
        (
            // The range's lower bound, which the service documents no meaning for: the
            // reader falls back on its own default page size.
            "the lower bound of the modelled range",
            Some(0),
            vec![3],
        ),
    ];

    for (case, max_results, expected_pages) in rows {
        let mut walked: Vec<String> = Vec::new();
        let mut pages: Vec<usize> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut request = json!({});
            if let Some(max_results) = max_results {
                request["maxResults"] = json!(max_results);
            }
            if let Some(token) = next_token.as_deref() {
                request["nextToken"] = json!(token);
            }
            let body = describe_ok(&router, case, request).await;
            let page = reported_ids(&body);
            pages.push(page.len());
            walked.extend(page);
            next_token = body["nextToken"].as_str().map(ToString::to_string);
            if next_token.is_none() {
                break;
            }
            assert!(
                pages.len() <= 8,
                "case {case:?}: the walk does not terminate, pages so far {pages:?}"
            );
        }
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: unexpected page sizes for maxResults={max_results:?}"
        );
        assert_eq!(
            walked, created,
            "case {case:?}: the walk must report every job exactly once, in creation order"
        );
    }

    // The default page size is the ceiling a request naming no size gets; three jobs is
    // well inside it, which is why the walk above finished in one page.
    assert!(
        DEFAULT_PAGE_SIZE >= created.len(),
        "this fixture must fit in one default-sized page, got {DEFAULT_PAGE_SIZE}"
    );

    // A token is bound to the selection it was taken over.
    let first_page = describe_ok(&router, "a bounded first page", json!({"maxResults": 1})).await;
    let token = first_page["nextToken"]
        .as_str()
        .expect("a bounded page over a larger selection carries a token")
        .to_string();
    for (case, request) in [
        ("a forged token", json!({"nextToken": "not-a-token"})),
        (
            "a token issued over another selection",
            json!({
                "nextToken": token,
                "filters": [{"name": "JobStatus", "values": [JOB_STATUS_COMPLETE]}],
            }),
        ),
        ("an empty job id", json!({"jobIds": [""]})),
        (
            "a status outside the JobStatus value set",
            json!({"filters": [{"name": "JobStatus", "values": ["Done"]}]}),
        ),
        (
            "a resource type outside the ResourceType value set",
            json!({"filters": [{"name": "ResourceType", "values": ["EbsVolumes"]}]}),
        ),
        (
            "a filter carrying no name",
            json!({"filters": [{"values": [JOB_STATUS_COMPLETE]}]}),
        ),
    ] {
        let (status, body) = call(
            &router,
            "DescribeRecommendationExportJobs",
            request.clone(),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {request} to be refused, got {status}: {body}"
        );
        assert!(
            error_code(&body).contains("InvalidParameterValueException"),
            "case {case:?}: expected InvalidParameterValueException, got {body}"
        );
        assert!(
            body["recommendationExportJobs"].is_null(),
            "case {case:?}: a refused read reports nothing, got {body}"
        );
    }
}

/// The enrollment gate: "The account is not opted in to Compute Optimizer" is what both the
/// export and its reader answer while the account is not enrolled, and no job is recorded.
#[tokio::test]
async fn an_export_is_refused_while_the_account_is_not_opted_in() {
    let router = emulator();
    create_bucket(&router, BUCKET).await;

    for (case, operation, request) in [
        (
            "exporting",
            "ExportIdleRecommendations",
            json!({"s3DestinationConfig": {"bucket": BUCKET}}),
        ),
        (
            "reading the jobs",
            "DescribeRecommendationExportJobs",
            json!({}),
        ),
    ] {
        let (status, body) = call(&router, operation, request).await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected 403 while not opted in, got {status}: {body}"
        );
        assert!(
            error_code(&body).contains("OptInRequiredException"),
            "case {case:?}: expected OptInRequiredException, got {body}"
        );
    }

    // Enrolling is what admits the export, and the store the refused call left alone is
    // still empty.
    opt_in(&router).await;
    assert_eq!(
        reported_ids(&describe_ok(&router, "after enrolling", json!({})).await),
        Vec::<String>::new(),
        "a refused export records no job"
    );
    let job_id = export_job_id(&router, "after enrolling").await;
    assert_eq!(
        reported_ids(&describe_ok(&router, "after the export", json!({})).await),
        vec![job_id],
        "the export recorded its job once the account was enrolled"
    );
}
