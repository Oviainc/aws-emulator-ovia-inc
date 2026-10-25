// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for `ListTransformJobs`.
//!
//! Every case goes over the wire (`POST /` with
//! `X-Amz-Target: SageMaker.ListTransformJobs`, `awsJson1_1`) through the same
//! `MultiServiceRouter` the production router uses, so what these tests exercise is what
//! a real SDK client gets.
//!
//! Most fixture jobs are put into the shared store directly, through the same
//! `crate::state` record `CreateTransformJob` writes, so each row's instants, status,
//! failure reason and end time are exactly what the table declares rather than whatever a
//! create would mint from the wall clock — which is what the listing's order, filters,
//! paging and projection are asserted over. Seeding is therefore the fixture, and only
//! the listing is under test, which is what the shared harness's `pub(crate) engine` is
//! for, so this suite reuses [`super::inference_deployment_lifecycle::Emulator`] rather
//! than growing another copy of the dispatch path. One case goes the whole way through
//! `CreateTransformJob` and reads the job back through both `ListTransformJobs` and
//! `DescribeTransformJob`, so the listing is shown to report the records the writer
//! actually stored.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: every
//! documented sort field and order, every documented filter on each published bound, the
//! projection of each stored member into a `TransformJobSummary`, the paging walk at every
//! page size up to past the end of the result set, the documented `MaxResults` default and
//! the server-side ceiling, and the refusals the modelled constraints require.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_sagemaker_sagemaker::state::{
    ComAmazonawsSagemakerSagemakerState, TransformJob, MAX_PAGE_SIZE, TRANSFORM_JOB_SETTLES_AFTER,
    TRANSFORM_NOT_RUN_FAILURE,
};

use super::inference_deployment_lifecycle::{error_code, str_field, Emulator, ACCOUNT};

/// The region these tests sign for, so a seeded ARN and a minted one agree — the one
/// [`Emulator::call`] signs for.
const REGION: &str = "us-east-1";

/// The instant the fixture's timestamps hang off, so every bound in a table is exact
/// rather than relative to the test's wall clock.
const EPOCH_BASE_SECS: u64 = 1_700_000_000;

/// `MaxResults`' documented default for this operation ("The maximum number of transform
/// jobs to return in the response. The default value is 10." — smithy_query
/// operation=ListTransformJobs).
const DOCUMENTED_DEFAULT_MAX_RESULTS: usize = 10;

/// `TransformJobStatus`'s members (smithy_query shape=TransformJobStatus). A job in one of
/// the terminal ones is where the clock would already have left it.
const TRANSFORM_JOB_STATUSES: [&str; 5] =
    ["Completed", "Failed", "InProgress", "Stopped", "Stopping"];

/// The `TransformJobStatus` members past which nothing more is owed, so
/// `TransformEndTime` is set exactly for them ("Indicates when the transform job ends on
/// compute instances" — `TransformJobSummary$TransformEndTime`).
const TERMINAL_STATUSES: [&str; 3] = ["Completed", "Failed", "Stopped"];

/// Long enough to pass `state::TRANSFORM_JOB_SETTLES_AFTER`, so the terminal status the
/// clock owes is due by the time the next call runs.
const PAST_SETTLE_DUE: Duration = TRANSFORM_JOB_SETTLES_AFTER.saturating_mul(3);

/// Put a transform job in the shared store, under the `TransformJobName` it is keyed by.
fn seed(emulator: &Emulator, job: TransformJob) {
    emulator
        .engine
        .state::<ComAmazonawsSagemakerSagemakerState>()
        .lock()
        .transform_jobs
        .insert(job.name.clone(), job);
}

/// The names the store holds, so a read can be shown not to change it.
fn stored_jobs(emulator: &Emulator) -> Vec<String> {
    emulator
        .engine
        .state::<ComAmazonawsSagemakerSagemakerState>()
        .lock()
        .transform_jobs
        .keys()
        .cloned()
        .collect()
}

/// `ListTransformJobs` with `body`, requiring a 200.
async fn list(emulator: &Emulator, body: &Value) -> Value {
    emulator.ok("ListTransformJobs", body).await
}

/// `EPOCH_BASE_SECS + offset` as the epoch-seconds number `awsJson1_1` carries a
/// `Timestamp` as.
fn epoch_secs(offset: u64) -> i64 {
    i64::try_from(EPOCH_BASE_SECS + offset).expect("the fixture's instants fit an i64")
}

/// `EPOCH_BASE_SECS + offset` as a stored instant.
fn at(offset: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(EPOCH_BASE_SECS + offset)
}

/// A `transform-job` ARN of the form this service mints
/// (`arn:aws:sagemaker:<region>:<account>:transform-job/<name>`).
fn transform_job_arn(name: &str) -> String {
    format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:transform-job/{name}")
}

/// A stored transform job: `created` and `modified` are offsets from [`EPOCH_BASE_SECS`],
/// `TransformEndTime` is set exactly for the terminal statuses, and `FailureReason` only
/// where the job failed.
///
/// A row still `InProgress` is due to end an hour from the test's own clock, so the
/// listing's clock-owed transition is not what these cases observe; the create round trip
/// below is where that transition is asserted.
fn job(name: &str, status: &str, created: u64, modified: u64) -> TransformJob {
    let terminal = TERMINAL_STATUSES.contains(&status);
    TransformJob {
        name: name.to_string(),
        arn: transform_job_arn(name),
        model_name: "fixture-model".to_string(),
        status: status.to_string(),
        failure_reason: (status == "Failed").then(|| TRANSFORM_NOT_RUN_FAILURE.to_string()),
        max_concurrent_transforms: 1,
        model_client_config: None,
        max_payload_in_mb: 6,
        batch_strategy: None,
        environment: None,
        transform_input: Default::default(),
        transform_output: Default::default(),
        data_capture_config: None,
        transform_resources: Default::default(),
        data_processing: None,
        experiment_config: None,
        tags: Vec::new(),
        creation_time: at(created),
        last_modified_time: at(modified),
        transform_end_time: terminal.then(|| at(modified)),
        ends_at: if terminal {
            at(modified)
        } else {
            SystemTime::now() + Duration::from_secs(3600)
        },
    }
}

/// Four transform jobs whose rows differ along name, status, creation time, last-modified
/// time (ordered the OPPOSITE way to creation time, so the two bounds cannot be confused),
/// failure reason and end time.
fn seed_fixture(emulator: &Emulator) {
    for record in [
        job("alpha-one", "InProgress", 10, 40),
        job("beta-two", "Completed", 20, 30),
        job("demo-three", "Failed", 30, 20),
        job("demo-four", "Stopped", 40, 10),
    ] {
        seed(emulator, record);
    }
}

fn summaries(listed: &Value) -> Vec<Value> {
    listed
        .get("TransformJobSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("TransformJobSummaries is a required output member: {listed}"))
}

/// The `TransformJobName`s a response lists, in the order it listed them.
fn listed_names(listed: &Value) -> Vec<String> {
    summaries(listed)
        .iter()
        .map(|summary| {
            summary
                .get("TransformJobName")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

fn next_token(listed: &Value) -> Option<String> {
    listed
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// RULE: with nothing stored, the listing answers 200 with an empty
/// `TransformJobSummaries` and no `NextToken` — never a 501, and never a token to follow.
#[tokio::test]
async fn an_empty_store_lists_no_transform_jobs() {
    let emulator = Emulator::new();

    // Rows: the bare call, and one carrying each filter — an absent resource is an empty
    // page for a listing whatever the request selected on.
    let cases = [
        ("no-members", json!({})),
        ("name-contains", json!({ "NameContains": "anything" })),
        ("status-equals", json!({ "StatusEquals": "InProgress" })),
        (
            "creation-bounds",
            json!({ "CreationTimeAfter": epoch_secs(0), "CreationTimeBefore": epoch_secs(100) }),
        ),
        (
            "max-results",
            json!({ "MaxResults": 100, "SortBy": "Name", "SortOrder": "Ascending" }),
        ),
    ];

    for (case, body) in cases {
        let listed = list(&emulator, &body).await;
        assert_eq!(
            listed_names(&listed),
            Vec::<String>::new(),
            "case {case}: an empty store must list nothing, got {listed:?}"
        );
        assert_eq!(
            next_token(&listed),
            None,
            "case {case}: an empty page must carry no NextToken, got {listed:?}"
        );
    }
}

/// RULE: the listing orders the stored jobs by `SortBy` in `SortOrder`, defaulting to
/// `CreationTime` / `Descending` as those members' model docs state, and breaks a tie on
/// `TransformJobName` ascending in both directions.
#[tokio::test]
async fn the_listing_honours_every_documented_sort_field_and_order() {
    let emulator = Emulator::new();
    seed_fixture(&emulator);

    // `SortBy` is `Name | CreationTime | Status` and `SortOrder` is
    // `Ascending | Descending` (smithy_query operation=ListTransformJobs): the rows below
    // are every combination, plus the two defaults.
    let by_creation = ["alpha-one", "beta-two", "demo-three", "demo-four"];
    let by_name = ["alpha-one", "beta-two", "demo-four", "demo-three"];
    // `Completed` < `Failed` < `InProgress` < `Stopped`.
    let by_status = ["beta-two", "demo-three", "alpha-one", "demo-four"];

    let reversed = |order: [&'static str; 4]| {
        let mut rows = order.to_vec();
        rows.reverse();
        rows
    };

    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        // `SortOrder`'s documented default for THIS operation is `Descending`.
        ("defaults", json!({}), reversed(by_creation)),
        (
            "creation-ascending",
            json!({ "SortBy": "CreationTime", "SortOrder": "Ascending" }),
            by_creation.to_vec(),
        ),
        (
            "creation-descending",
            json!({ "SortBy": "CreationTime", "SortOrder": "Descending" }),
            reversed(by_creation),
        ),
        (
            "name-ascending",
            json!({ "SortBy": "Name", "SortOrder": "Ascending" }),
            by_name.to_vec(),
        ),
        (
            "name-descending",
            json!({ "SortBy": "Name", "SortOrder": "Descending" }),
            reversed(by_name),
        ),
        (
            "status-ascending",
            json!({ "SortBy": "Status", "SortOrder": "Ascending" }),
            by_status.to_vec(),
        ),
        (
            "status-descending",
            json!({ "SortBy": "Status", "SortOrder": "Descending" }),
            reversed(by_status),
        ),
        (
            "sort-order-only",
            json!({ "SortOrder": "Ascending" }),
            by_creation.to_vec(),
        ),
        (
            "sort-by-only",
            json!({ "SortBy": "Name" }),
            reversed(by_name),
        ),
    ];

    for (case, body, expected) in cases {
        let listed = list(&emulator, &body).await;
        assert_eq!(
            listed_names(&listed),
            expected,
            "case {case}: ListTransformJobs order, got {listed:?}"
        );
    }
}

/// RULE: jobs sharing a sort key are ordered by `TransformJobName` ascending whichever way
/// the sort key runs, so a page boundary is stable.
#[tokio::test]
async fn jobs_sharing_a_sort_key_are_tie_broken_by_name_in_both_orders() {
    let emulator = Emulator::new();
    // Three jobs with one creation instant and one status: only the tie-break orders them.
    for name in ["c-job", "a-job", "b-job"] {
        seed(&emulator, job(name, "Completed", 10, 10));
    }

    for (case, sort_by) in [("creation-tie", "CreationTime"), ("status-tie", "Status")] {
        for order in ["Ascending", "Descending"] {
            let listed = list(
                &emulator,
                &json!({ "SortBy": sort_by, "SortOrder": order }),
            )
            .await;
            assert_eq!(
                listed_names(&listed),
                vec!["a-job", "b-job", "c-job"],
                "case {case}/{order}: a shared sort key is tie-broken by name ascending, \
                 got {listed:?}"
            );
        }
    }
}

/// RULE: each documented filter selects on the job's stored value, every time bound is
/// inclusive, an omitted filter selects every job, and filters combine conjunctively.
#[tokio::test]
async fn every_documented_filter_selects_the_jobs_it_names() {
    let emulator = Emulator::new();
    seed_fixture(&emulator);

    // Rows differ along which filter is applied, the bound's position relative to the
    // stored instants (before, exactly on — the inclusive case — and after), the needle's
    // shape, and the status named. Ordered by name ascending so the expectation is exact.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        ("no-filter", json!({}), vec!["alpha-one", "beta-two", "demo-four", "demo-three"]),
        (
            "name-contains-prefix",
            json!({ "NameContains": "demo" }),
            vec!["demo-four", "demo-three"],
        ),
        (
            "name-contains-infix",
            json!({ "NameContains": "-" }),
            vec!["alpha-one", "beta-two", "demo-four", "demo-three"],
        ),
        (
            "name-contains-whole-name",
            json!({ "NameContains": "beta-two" }),
            vec!["beta-two"],
        ),
        (
            "name-contains-no-match",
            json!({ "NameContains": "gamma" }),
            vec![],
        ),
        (
            "status-equals-completed",
            json!({ "StatusEquals": "Completed" }),
            vec!["beta-two"],
        ),
        (
            "status-equals-failed",
            json!({ "StatusEquals": "Failed" }),
            vec!["demo-three"],
        ),
        (
            "status-equals-in-progress",
            json!({ "StatusEquals": "InProgress" }),
            vec!["alpha-one"],
        ),
        (
            "status-equals-stopped",
            json!({ "StatusEquals": "Stopped" }),
            vec!["demo-four"],
        ),
        (
            "status-equals-stopping-none-stored",
            json!({ "StatusEquals": "Stopping" }),
            vec![],
        ),
        (
            // The bound lands exactly on beta-two's creation instant, and an inclusive
            // bound still selects it.
            "creation-after-inclusive",
            json!({ "CreationTimeAfter": epoch_secs(20) }),
            vec!["beta-two", "demo-four", "demo-three"],
        ),
        (
            "creation-before-inclusive",
            json!({ "CreationTimeBefore": epoch_secs(20) }),
            vec!["alpha-one", "beta-two"],
        ),
        (
            "creation-window-of-one-instant",
            json!({ "CreationTimeAfter": epoch_secs(20), "CreationTimeBefore": epoch_secs(20) }),
            vec!["beta-two"],
        ),
        (
            "creation-window-past-every-job",
            json!({ "CreationTimeAfter": epoch_secs(41) }),
            vec![],
        ),
        (
            "modified-after-inclusive",
            json!({ "LastModifiedTimeAfter": epoch_secs(30) }),
            vec!["alpha-one", "beta-two"],
        ),
        (
            "modified-before-inclusive",
            json!({ "LastModifiedTimeBefore": epoch_secs(20) }),
            vec!["demo-four", "demo-three"],
        ),
        (
            // The two time ranges are independent: a creation window that keeps three jobs
            // and a last-modified window that keeps two leave only their intersection.
            "creation-and-modified-windows",
            json!({
                "CreationTimeAfter": epoch_secs(20),
                "LastModifiedTimeAfter": epoch_secs(20),
            }),
            vec!["beta-two", "demo-three"],
        ),
        (
            "name-and-status",
            json!({ "NameContains": "demo", "StatusEquals": "Stopped" }),
            vec!["demo-four"],
        ),
        (
            "name-and-status-disjoint",
            json!({ "NameContains": "alpha", "StatusEquals": "Completed" }),
            vec![],
        ),
    ];

    for (case, body, expected) in cases {
        let mut request = json!({ "SortBy": "Name", "SortOrder": "Ascending" });
        for (key, value) in body.as_object().expect("a JSON object of filter members") {
            request[key] = value.clone();
        }
        let listed = list(&emulator, &request).await;
        assert_eq!(
            listed_names(&listed),
            expected,
            "case {case}: ListTransformJobs selection, got {listed:?}"
        );
    }
}

/// One row of the projection table: a stored fixture job, and what its summary must
/// carry. `created` and `modified` are offsets from [`EPOCH_BASE_SECS`].
struct ExpectedSummary {
    name: &'static str,
    status: &'static str,
    created: u64,
    modified: u64,
}

/// RULE: each listed job is projected into a `TransformJobSummary` carrying the members
/// the store holds for it — name, ARN, creation and last-modified instants, status, and
/// the end time and failure reason exactly where the job has them.
#[tokio::test]
async fn summaries_project_every_stored_member_of_each_listed_job() {
    let emulator = Emulator::new();
    seed_fixture(&emulator);

    let expected = [
        ExpectedSummary {
            name: "alpha-one",
            status: "InProgress",
            created: 10,
            modified: 40,
        },
        ExpectedSummary {
            name: "beta-two",
            status: "Completed",
            created: 20,
            modified: 30,
        },
        ExpectedSummary {
            name: "demo-three",
            status: "Failed",
            created: 30,
            modified: 20,
        },
        ExpectedSummary {
            name: "demo-four",
            status: "Stopped",
            created: 40,
            modified: 10,
        },
    ];

    let listed = list(
        &emulator,
        &json!({ "SortBy": "CreationTime", "SortOrder": "Ascending" }),
    )
    .await;
    let summaries = summaries(&listed);
    assert_eq!(
        summaries.len(),
        expected.len(),
        "the fixture's jobs must all be listed, got {listed:?}"
    );

    for (summary, row) in summaries.iter().zip(expected) {
        let ExpectedSummary {
            name,
            status,
            created,
            modified,
        } = row;
        let case = name;
        assert_eq!(
            summary.get("TransformJobName").and_then(Value::as_str),
            Some(name),
            "case {case}: TransformJobName, got {summary:?}"
        );
        assert_eq!(
            summary.get("TransformJobArn").and_then(Value::as_str),
            Some(transform_job_arn(name).as_str()),
            "case {case}: TransformJobArn, got {summary:?}"
        );
        assert_eq!(
            summary.get("CreationTime").and_then(Value::as_i64),
            Some(epoch_secs(created)),
            "case {case}: CreationTime, got {summary:?}"
        );
        assert_eq!(
            summary.get("LastModifiedTime").and_then(Value::as_i64),
            Some(epoch_secs(modified)),
            "case {case}: LastModifiedTime, got {summary:?}"
        );
        assert_eq!(
            summary.get("TransformJobStatus").and_then(Value::as_str),
            Some(status),
            "case {case}: TransformJobStatus, got {summary:?}"
        );
        let terminal = TERMINAL_STATUSES.contains(&status);
        assert_eq!(
            summary.get("TransformEndTime").and_then(Value::as_i64),
            terminal.then(|| epoch_secs(modified)),
            "case {case}: TransformEndTime must be present exactly where the job ended, \
             got {summary:?}"
        );
        assert_eq!(
            summary.get("FailureReason").and_then(Value::as_str),
            (status == "Failed").then_some(TRANSFORM_NOT_RUN_FAILURE),
            "case {case}: FailureReason must be present exactly where the job failed, \
             got {summary:?}"
        );
    }
}

/// RULE: paging with `MaxResults` walks the whole ordered result exactly once — each page
/// holds at most `MaxResults` entries, a `NextToken` is present only while more remain,
/// and feeding it back resumes immediately after the previous page.
#[tokio::test]
async fn paging_walks_the_whole_ordered_result_exactly_once() {
    let emulator = Emulator::new();
    seed_fixture(&emulator);
    let ordered = vec!["alpha-one", "beta-two", "demo-four", "demo-three"];

    // Rows: every page size from one entry up to past the end of the result set.
    // `MaxResults` is `range 1..100` (smithy_query operation=ListTransformJobs), so 1 and
    // 100 are the published bounds.
    for page_size in [1_i64, 2, 3, 4, 5, 100] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= 10,
                "case {case}: paging must terminate, saw {seen:?}"
            );
            let mut body = json!({
                "SortBy": "Name",
                "SortOrder": "Ascending",
                "MaxResults": page_size,
            });
            if let Some(ref value) = token {
                body["NextToken"] = Value::String(value.clone());
            }
            let listed = list(&emulator, &body).await;
            let names = listed_names(&listed);
            assert!(
                names.len() as i64 <= page_size,
                "case {case}: a page must hold at most {page_size} entries, got {names:?}"
            );
            seen.extend(names);
            token = next_token(&listed);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, ordered,
            "case {case}: the walk must cover the ordered result exactly once"
        );
    }

    // A token that names no boundary of this result set is refused rather than silently
    // restarting the listing.
    for (case, token) in [
        ("not-hex", "not-a-token"),
        // Well-formed, but names a job no longer in the result set.
        ("unknown-boundary", "6e6f2d73756368"),
    ] {
        let (status, parsed) = emulator
            .call("ListTransformJobs", &json!({ "NextToken": token }))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case}: error code, got {parsed:?}"
        );
    }
}

/// RULE: an absent `MaxResults` takes this operation's documented default of 10, an
/// explicit one is capped by the service's own ceiling so no single call serialises the
/// whole store, and the resume token still walks the full set.
#[tokio::test]
async fn the_page_takes_the_documented_default_and_is_capped_server_side() {
    let ceiling = usize::try_from(MAX_PAGE_SIZE).expect("the page ceiling is positive");
    let stored = ceiling + 7;

    let emulator = Emulator::new();
    for index in 0..stored {
        let offset = index as u64;
        seed(
            &emulator,
            job(&format!("job-{index:03}"), "Completed", offset, offset),
        );
    }

    for (case, body, expected_page) in [
        ("no-max-results", json!({}), DOCUMENTED_DEFAULT_MAX_RESULTS),
        (
            "max-results-at-the-bound",
            json!({ "MaxResults": 100 }),
            ceiling,
        ),
        ("max-results-below-the-ceiling", json!({ "MaxResults": 7 }), 7),
        ("max-results-at-the-low-bound", json!({ "MaxResults": 1 }), 1),
    ] {
        let mut request = json!({ "SortBy": "Name", "SortOrder": "Ascending" });
        for (key, value) in body.as_object().expect("a JSON object of extra members") {
            request[key] = value.clone();
        }
        let listed = list(&emulator, &request).await;
        let names = listed_names(&listed);
        assert_eq!(
            names.len(),
            expected_page,
            "case {case}: page size, got {} entries",
            names.len()
        );
        assert!(
            next_token(&listed).is_some(),
            "case {case}: entries remain, so a NextToken must be present, got {listed:?}"
        );
    }

    // The capped walk still reaches every stored job exactly once.
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({ "SortBy": "Name", "SortOrder": "Ascending" });
        if let Some(ref value) = token {
            body["NextToken"] = Value::String(value.clone());
        }
        let listed = list(&emulator, &body).await;
        let names = listed_names(&listed);
        assert!(
            names.len() <= ceiling,
            "case capped-walk: a page must never exceed the ceiling, got {} entries",
            names.len()
        );
        seen.extend(names);
        token = next_token(&listed);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        seen.len(),
        stored,
        "case capped-walk: the walk must cover every stored job exactly once, got {} entries",
        seen.len()
    );
}

/// RULE: the listing reports what `CreateTransformJob` stored, and the terminal status the
/// clock owes shows up in the summary exactly as `DescribeTransformJob` reports it.
#[tokio::test]
async fn created_jobs_are_listed_and_their_clock_owed_status_matches_describe() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateModel",
            &json!({ "ModelName": "m", "PrimaryContainer": { "Image": "demo:1" } }),
        )
        .await;

    // Rows differ along the name's shape, which the listing projects verbatim.
    let names = ["t", "job1", "demo-transform-job-with-hyphens"];
    for name in names {
        let created = emulator
            .ok(
                "CreateTransformJob",
                &json!({
                    "TransformJobName": name,
                    "ModelName": "m",
                    "TransformInput": {
                        "DataSource": {
                            "S3DataSource": {
                                "S3DataType": "S3Prefix",
                                "S3Uri": "s3://demo-bucket/input/",
                            },
                        },
                    },
                    "TransformOutput": { "S3OutputPath": "s3://demo-bucket/output/" },
                    "TransformResources": { "InstanceType": "ml.m5.large", "InstanceCount": 1 },
                }),
            )
            .await;
        assert_eq!(
            str_field(&created, "TransformJobArn"),
            transform_job_arn(name),
            "case {name}: CreateTransformJob ARN, got {created:?}"
        );
    }

    // Before any end is due, every created job lists as running, with no end time and no
    // failure reason.
    let running = list(&emulator, &json!({ "SortBy": "Name", "SortOrder": "Ascending" })).await;
    assert_eq!(
        listed_names(&running),
        vec!["demo-transform-job-with-hyphens", "job1", "t"],
        "case fresh: every created job is listed, got {running:?}"
    );
    for summary in summaries(&running) {
        let case = str_field(&summary, "TransformJobName");
        assert_eq!(
            summary.get("TransformJobStatus").and_then(Value::as_str),
            Some("InProgress"),
            "case {case}: a job lists as running before its end is due, got {summary:?}"
        );
        assert_eq!(
            summary.get("TransformEndTime"),
            None,
            "case {case}: a running job has not ended, got {summary:?}"
        );
        assert_eq!(
            summary.get("FailureReason"),
            None,
            "case {case}: a running job has not failed, got {summary:?}"
        );
        assert_eq!(
            summary.get("LastModifiedTime").and_then(Value::as_f64),
            summary.get("CreationTime").and_then(Value::as_f64),
            "case {case}: an untouched job was last modified when it was created, \
             got {summary:?}"
        );
    }

    tokio::time::sleep(PAST_SETTLE_DUE).await;

    let settled = list(&emulator, &json!({ "SortBy": "Name", "SortOrder": "Ascending" })).await;
    for summary in summaries(&settled) {
        let name = str_field(&summary, "TransformJobName");
        let described = emulator
            .ok("DescribeTransformJob", &json!({ "TransformJobName": name }))
            .await;
        assert_eq!(
            summary.get("TransformJobStatus"),
            described.get("TransformJobStatus"),
            "case {name}: the summary's status must be the one the reader reports, \
             got {summary:?} and {described:?}"
        );
        assert_eq!(
            summary.get("FailureReason"),
            described.get("FailureReason"),
            "case {name}: the summary's failure reason must be the reader's, \
             got {summary:?} and {described:?}"
        );
        assert_eq!(
            summary.get("TransformEndTime"),
            described.get("TransformEndTime"),
            "case {name}: the summary's end time must be the reader's, \
             got {summary:?} and {described:?}"
        );
        assert_eq!(
            summary.get("LastModifiedTime").and_then(Value::as_f64),
            summary.get("TransformEndTime").and_then(Value::as_f64),
            "case {name}: a job the clock ended was last modified when it ended, \
             got {summary:?}"
        );
    }

    // The transition happened once: a later listing answers exactly the same.
    let again = list(&emulator, &json!({ "SortBy": "Name", "SortOrder": "Ascending" })).await;
    assert_eq!(
        again, settled,
        "case stable: the settled listing must be stable across reads, got {again:?} then \
         {settled:?}"
    );
}

/// RULE: listing is a report — it changes nothing, so a second identical call answers
/// identically and the store still holds exactly what it held.
#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    seed_fixture(&emulator);
    let before = stored_jobs(&emulator);

    let cases = [
        ("no-members", json!({})),
        ("max-results", json!({ "MaxResults": 2 })),
        (
            "filtered",
            json!({ "StatusEquals": "Completed", "SortBy": "Name" }),
        ),
        (
            "time-bounded",
            json!({ "CreationTimeAfter": epoch_secs(10), "SortBy": "Status" }),
        ),
    ];

    for (case, body) in cases {
        let first = list(&emulator, &body).await;
        let second = list(&emulator, &body).await;
        assert_eq!(
            first, second,
            "case {case}: a repeated read must answer identically, got {first:?} then {second:?}"
        );
        assert_eq!(
            stored_jobs(&emulator),
            before,
            "case {case}: a read must not change the store"
        );
    }
}

/// RULE: a request outside a member's modelled constraint, or naming a value its enum
/// does not hold, is refused with HTTP 400 — never a 200 listing something, and never a
/// 501.
#[tokio::test]
async fn requests_outside_the_modelled_constraints_are_refused() {
    let emulator = Emulator::new();
    seed_fixture(&emulator);

    // Rows: each published bound of this operation's members and one past it —
    // `MaxResults` range 1..100 and `NameContains` len 0..63 with pattern
    // `^[a-zA-Z0-9\-]+$` (smithy_query operation=ListTransformJobs) — plus a value outside
    // each enum.
    let within: Vec<(&str, Value)> = vec![
        ("max-results-at-1", json!({ "MaxResults": 1 })),
        ("max-results-at-100", json!({ "MaxResults": 100 })),
        (
            "name-contains-at-63",
            json!({ "NameContains": "a".repeat(63) }),
        ),
    ];
    for (case, body) in within {
        let (status, parsed) = emulator.call("ListTransformJobs", &body).await;
        assert_eq!(
            status, 200,
            "case {case}: a value on the published bound is accepted, got {status} with {parsed}"
        );
    }

    let mut refused: Vec<(String, Value)> = vec![
        ("max-results-below-1".to_string(), json!({ "MaxResults": 0 })),
        (
            "max-results-above-100".to_string(),
            json!({ "MaxResults": 101 }),
        ),
        (
            "name-contains-past-63".to_string(),
            json!({ "NameContains": "a".repeat(64) }),
        ),
        (
            "name-contains-outside-pattern".to_string(),
            json!({ "NameContains": "bad_name" }),
        ),
        ("sort-by-unknown".to_string(), json!({ "SortBy": "EndTime" })),
        (
            "sort-by-wrong-case".to_string(),
            json!({ "SortBy": "creationtime" }),
        ),
        (
            "sort-order-wrong-case".to_string(),
            json!({ "SortOrder": "ascending" }),
        ),
        (
            "status-equals-unknown".to_string(),
            json!({ "StatusEquals": "Pending" }),
        ),
    ];
    // Each `TransformJobStatus` member lower-cased is NOT that member, so none of them is
    // accepted in the wrong case either.
    for status in TRANSFORM_JOB_STATUSES {
        refused.push((
            format!("status-equals-wrong-case-{status}"),
            json!({ "StatusEquals": status.to_lowercase() }),
        ));
    }

    for (case, body) in refused {
        let (status, parsed) = emulator.call("ListTransformJobs", &body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {parsed}"
        );
    }

    // A refused request changed nothing: the fixture still lists in full.
    let listed = list(&emulator, &json!({})).await;
    assert_eq!(
        listed_names(&listed).len(),
        4,
        "a refused request must leave the store untouched, got {listed:?}"
    );
}
