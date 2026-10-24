// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the AMI store task slice: `CreateStoreImageTask` and
//! `DescribeStoreImageTasks`, driven over the wire the service actually speaks —
//! `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. `CreateStoreImageTask` STORES A TASK FOR THE DESTINATION THE REQUEST NAMES and
//!    `DescribeStoreImageTasks` REPORTS THE STORE: the bucket round-trips, and the members
//!    the service owns — the object key (the AMI's id plus the stored object's suffix), the
//!    start time, the state and the progress — are derived rather than taken. Every create
//!    records a task, so storing one AMI twice is two tasks, newest first;
//! 2. the CLOCK, not a caller, moves a task `InProgress` -> `Completed` and takes it out of
//!    the account's view at the end of its published window. A read reports the move; it
//!    never performs one;
//! 3. a bucket name that is not a bucket name, an AMI id the account cannot resolve, a page
//!    size outside the published range, a filter name the operation does not publish and a
//!    `MaxResults` beside `ImageIds` are refused with EC2's own error code, and nothing is
//!    stored;
//! 4. `ImageIds`, `Filters` and `MaxResults` narrow and page the report, newest task first
//!    — and `ImageIds` makes the filters it is given beside IGNORED.
//!
//! Each table row is a distinct input along what the rule generalises over — the form and
//! the length of the bucket name at each published bound and one past it, which AMI, how
//! many AMIs, which filter, which values, how many values, and which page size at each
//! published bound and one past it — so no row is the specification.

use std::time::{Duration, Instant};

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, ImageSpec, MAX_S3_BUCKET_NAME_CHARS,
    MIN_S3_BUCKET_NAME_CHARS, S3_BUCKET_NAME_RESERVED_PREFIXES, S3_BUCKET_NAME_RESERVED_SUFFIXES,
    STORE_IMAGE_OBJECT_KEY_SUFFIX, STORE_IMAGE_TASK_COMPLETE_PERCENTAGE,
    STORE_IMAGE_TASK_MAX_IN_PROGRESS_PERCENTAGE, STORE_TASK_STATE_COMPLETED,
    STORE_TASK_STATE_FAILED, STORE_TASK_STATE_IN_PROGRESS, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, call, element, elements, error_code, filters, indexed};

/// The Region every call here is signed for. A store task carries no Region member, so the
/// Region only selects the ec2Query envelope.
const REGION: &str = "us-west-2";

/// A well-formed AMI id no image of the account is stored under.
const UNKNOWN_IMAGE_ID: &str = "ami-0ffffffffffffffff";

/// Something that is not an AMI id at all.
const MALFORMED_IMAGE_ID: &str = "ami-not-an-id";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One ec2Query call through the shared harness.
async fn api(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// One `DescribeStoreImageTasks` call.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    api(router, "DescribeStoreImageTasks", params).await
}

/// One `CreateStoreImageTask` call for `image_id` into `bucket`.
async fn store(router: &MultiServiceRouter, image_id: &str, bucket: &str) -> (u16, String) {
    api(
        router,
        "CreateStoreImageTask",
        &[p("ImageId", image_id), p("Bucket", bucket)],
    )
    .await
}

/// Wait until every clock-owed transition started before now has certainly fallen due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(250)).await;
}

/// How many store tasks the store holds.
fn stored_count(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().store_image_tasks.len()
}

/// Write `count` AMIs of the calling account into the engine's store, the way a register
/// operation would, and report the ids they were given.
fn seed_images(engine: &engine::InMemoryEngine, count: usize) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    (0..count)
        .map(|index| {
            guard.insert_image(ImageSpec {
                name: format!("store-image-{index}"),
                ..ImageSpec::default()
            })
        })
        .collect()
}

/// The object key the service derives for `image_id`: AWS stores the AMI as a single object
/// named `<ami-id>.bin` ([`STORE_IMAGE_OBJECT_KEY_SUFFIX`]).
fn object_key(image_id: &str) -> String {
    format!("{image_id}{STORE_IMAGE_OBJECT_KEY_SUFFIX}")
}

/// A bucket name of exactly `length` characters that satisfies the Amazon S3 bucket naming
/// rules: lowercase letters only, so it begins and ends with a letter whatever the length.
fn bucket_of_length(length: usize) -> String {
    "a".repeat(length)
}

/// RULE 1: a create stores a task for the destination the request names, and the describe
/// reports it — the bucket as given, every other member derived.
#[tokio::test]
async fn a_create_stores_a_task_and_the_describe_reports_it() {
    let (router, engine) = super::ec2_query::emulator();

    // The rows differ along what the rule generalises over: the FORM of the bucket name
    // (plain, hyphenated, dotted, digits) and its LENGTH at each published bound of the
    // Amazon S3 bucket naming rules.
    let rows: Vec<(&str, String)> = vec![
        ("a plain bucket name", "ami-archive".to_string()),
        ("a hyphenated bucket name", "ami-archive-eu-west-1".to_string()),
        ("a dotted bucket name", "ami.archive.example".to_string()),
        ("a bucket name of digits", "20261022".to_string()),
        (
            "the shortest bucket name the rules allow",
            bucket_of_length(MIN_S3_BUCKET_NAME_CHARS),
        ),
        (
            "the longest bucket name the rules allow",
            bucket_of_length(MAX_S3_BUCKET_NAME_CHARS),
        ),
    ];
    let images = seed_images(&engine, rows.len());

    for ((case, bucket), image_id) in rows.iter().zip(&images) {
        let (status, body) = store(&router, image_id, bucket).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStoreImageTask, got {status} with \
             body {body}"
        );
        let reported = element(&body, "objectKey").unwrap_or_default();
        assert_eq!(
            reported,
            object_key(image_id),
            "case {case:?}: expected the object key {:?}, got {reported:?} in body {body}",
            object_key(image_id)
        );

        // Read the task back through the describe, narrowed to the AMI just stored.
        let (status, body) = describe(&router, &indexed("ImageId", &[image_id.as_str()])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStoreImageTasks, got {status} \
             with body {body}"
        );
        for (member, expected) in [
            ("amiId", image_id.clone()),
            ("bucket", bucket.clone()),
            ("s3objectKey", object_key(image_id)),
        ] {
            let actual = element(&body, member).unwrap_or_default();
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {member} {expected:?}, got {actual:?} in body {body}"
            );
        }
        let start_time = element(&body, "taskStartTime").unwrap_or_default();
        assert!(
            as_timestamp(&start_time).is_some(),
            "case {case:?}: expected an ISO 8601 taskStartTime, got {start_time:?} in body \
             {body}"
        );
        let progress: i32 = element(&body, "progressPercentage")
            .unwrap_or_default()
            .parse()
            .unwrap_or(-1);
        assert!(
            (0..=STORE_IMAGE_TASK_COMPLETE_PERCENTAGE).contains(&progress),
            "case {case:?}: expected a progress percentage in 0..=\
             {STORE_IMAGE_TASK_COMPLETE_PERCENTAGE}, got {progress} in body {body}"
        );
    }

    assert_eq!(
        stored_count(&engine),
        rows.len(),
        "one task is stored per create"
    );

    // "Tasks are listed in reverse chronological order": the newest create is reported
    // first.
    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "the whole report: expected HTTP 200, got {status} with body {body}"
    );
    let newest_first: Vec<String> = images.iter().rev().cloned().collect();
    let reported = elements(&body, "amiId");
    assert_eq!(
        reported, newest_first,
        "expected the tasks newest first {newest_first:?}, got {reported:?} in body {body}"
    );
}

/// RULE 1, continued: every create RECORDS A TASK, so storing one AMI twice — into one
/// bucket or into two — is two tasks, both reported, newest first. Nothing de-duplicates a
/// destination: "you get a paginated list of store tasks from the last 31 days"
/// (`DescribeStoreImageTasks`) is every task started, not one per destination.
#[tokio::test]
async fn every_create_records_a_task_and_the_newest_is_reported_first() {
    let (router, engine) = super::ec2_query::emulator();
    let image_id = seed_images(&engine, 1).remove(0);

    // The rows differ along what the rule generalises over: the same destination again, and
    // a different one — each adds a task either way.
    for (case, bucket, expected) in [
        ("the first store", "first-bucket", 1),
        ("the same destination again", "first-bucket", 2),
        ("the same destination a third time", "first-bucket", 3),
        ("another bucket", "second-bucket", 4),
        ("the second destination again", "second-bucket", 5),
    ] {
        let (status, body) = store(&router, &image_id, bucket).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let held = stored_count(&engine);
        assert_eq!(
            held, expected,
            "case {case:?}: expected {expected} task(s) stored, got {held}"
        );
        let (_, body) = describe(&router, &[]).await;
        let reported = elements(&body, "amiId");
        assert_eq!(
            reported.len(),
            expected,
            "case {case:?}: expected {expected} reported task(s), got {reported:?} in body \
             {body}"
        );
        // The task just started is the newest, so it is reported first.
        let first_bucket = element(&body, "bucket").unwrap_or_default();
        assert_eq!(
            first_bucket, bucket,
            "case {case:?}: expected the newest task to target {bucket:?}, got \
             {first_bucket:?} in body {body}"
        );
        // Every task of one AMI reports the one object key its id derives.
        for (index, key) in elements(&body, "s3objectKey").iter().enumerate() {
            assert_eq!(
                key,
                &object_key(&image_id),
                "case {case:?}: task {index} reports the object key the AMI's id derives, \
                 got {key:?} in body {body}"
            );
        }
    }
}

/// RULE 2: the clock moves a task `InProgress` -> `Completed`, and a read reports that move
/// rather than performing one.
#[tokio::test]
async fn the_clock_completes_a_task_and_a_read_only_reports_it() {
    let (router, engine) = super::ec2_query::emulator();
    let image_id = seed_images(&engine, 1).remove(0);

    let (status, body) = store(&router, &image_id, "ami-archive").await;
    assert_eq!(status, 200, "the create: got {status} with body {body}");

    // The span is never zero, so a caller reading straight after the create still sees the
    // transient state.
    let (_, body) = describe(&router, &[]).await;
    assert_eq!(
        element(&body, "storeTaskState").unwrap_or_default(),
        STORE_TASK_STATE_IN_PROGRESS,
        "straight after the create the task is still in progress, got body {body}"
    );
    let progress: i32 = element(&body, "progressPercentage")
        .unwrap_or_default()
        .parse()
        .unwrap_or(-1);
    assert!(
        (0..=STORE_IMAGE_TASK_MAX_IN_PROGRESS_PERCENTAGE).contains(&progress),
        "a task that has not finished reports at most \
         {STORE_IMAGE_TASK_MAX_IN_PROGRESS_PERCENTAGE}, got {progress} in body {body}"
    );
    let start_time = element(&body, "taskStartTime").unwrap_or_default();

    past_the_transition().await;
    let (_, body) = describe(&router, &[]).await;
    assert_eq!(
        element(&body, "storeTaskState").unwrap_or_default(),
        STORE_TASK_STATE_COMPLETED,
        "the clock moves the task to completed, got body {body}"
    );
    assert_eq!(
        element(&body, "progressPercentage").unwrap_or_default(),
        STORE_IMAGE_TASK_COMPLETE_PERCENTAGE.to_string(),
        "a finished upload reports all of it, got body {body}"
    );
    assert!(
        element(&body, "storeTaskFailureReason").is_none(),
        "a task that succeeded carries no failure reason, got body {body}"
    );

    // A read reports the task; it does not restart it, and two reads report the same task.
    let (_, repeat) = describe(&router, &[]).await;
    assert_eq!(
        element(&repeat, "taskStartTime").unwrap_or_default(),
        start_time,
        "a later read reports the same start time, got body {repeat}"
    );
    assert_eq!(
        element(&repeat, "storeTaskState").unwrap_or_default(),
        STORE_TASK_STATE_COMPLETED,
        "a completed task stays completed, got body {repeat}"
    );
    assert_eq!(
        stored_count(&engine),
        1,
        "reading the report neither stores nor drops a task"
    );
}

/// RULE 2, continued: "Currently, only tasks from the past 31 days can be viewed" — a task
/// whose window has run out is reported no more, and the clock is what takes it out of the
/// view.
#[tokio::test]
async fn a_task_leaves_the_view_at_the_end_of_its_window() {
    let (router, engine) = super::ec2_query::emulator();
    let images = seed_images(&engine, 2);

    for image_id in &images {
        let (status, body) = store(&router, image_id, "ami-archive").await;
        assert_eq!(status, 200, "the creates: got {status} with body {body}");
    }
    let (_, body) = describe(&router, &[]).await;
    assert_eq!(
        elements(&body, "amiId").len(),
        2,
        "both tasks are in view to begin with, got body {body}"
    );

    // Bring the window of the OLDER task to its end, the way 31 days of clock would.
    {
        let state = engine.state::<Ec2State>();
        let mut guard = state.lock();
        let oldest = guard
            .store_image_tasks
            .keys()
            .next()
            .cloned()
            .expect("two tasks are stored");
        let record = guard
            .store_image_tasks
            .get_mut(&oldest)
            .expect("the oldest task is stored");
        record.leaves_view_at = Instant::now();
    }

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "after the window: expected HTTP 200, got {status} with body {body}"
    );
    let reported = elements(&body, "amiId");
    assert_eq!(
        reported,
        vec![images[1].clone()],
        "a task past its window is reported no more, got {reported:?} in body {body}"
    );
    assert_eq!(
        stored_count(&engine),
        1,
        "the task past its window is dropped from the store"
    );
}

/// One row of the selection table: what the case is, the parameters the describe carries,
/// and the AMI ids it must report, in order.
type SelectionRow = (&'static str, Vec<(String, String)>, Vec<String>);

/// RULE 4: `ImageIds`, `Filters` and `MaxResults` narrow and page the report, newest first,
/// and `ImageIds` makes any filter beside it ignored.
#[tokio::test]
async fn ids_filters_and_page_sizes_narrow_the_report() {
    let (router, engine) = super::ec2_query::emulator();
    let images = seed_images(&engine, 3);
    // Two destinations, so the `bucket` filter has something to choose between.
    let buckets = ["ami-archive-one", "ami-archive-two", "ami-archive-one"];
    for (image_id, bucket) in images.iter().zip(buckets) {
        let (status, body) = store(&router, image_id, bucket).await;
        assert_eq!(status, 200, "the creates: got {status} with body {body}");
    }
    // Every assertion below is about selection, not about progress, so the tasks are taken
    // past their upload first: their state is then settled whatever the test's own timing.
    past_the_transition().await;

    let [first, second, third] = [images[0].clone(), images[1].clone(), images[2].clone()];
    // "Tasks are listed in reverse chronological order".
    let every = vec![third.clone(), second.clone(), first.clone()];

    let rows: Vec<SelectionRow> = vec![
        ("no narrowing at all", Vec::new(), every.clone()),
        (
            "one id named",
            indexed("ImageId", &[second.as_str()]),
            vec![second.clone()],
        ),
        (
            "two ids named, reported newest first",
            indexed("ImageId", &[first.as_str(), third.as_str()]),
            vec![third.clone(), first.clone()],
        ),
        (
            // A well-formed id with no image and no task behind it selects nothing: a store
            // task outlives the AMI it stored, so an unresolvable id is not a refusal.
            "an id no task was started for",
            indexed("ImageId", &[UNKNOWN_IMAGE_ID]),
            Vec::new(),
        ),
        (
            "the bucket filter on the shared destination",
            filters(&[("bucket", &["ami-archive-one"])]),
            vec![third.clone(), first.clone()],
        ),
        (
            "the bucket filter on the other destination",
            filters(&[("bucket", &["ami-archive-two"])]),
            vec![second.clone()],
        ),
        (
            "the bucket filter with both values, joined with an OR",
            filters(&[("bucket", &["ami-archive-two", "ami-archive-one"])]),
            every.clone(),
        ),
        (
            "the bucket filter as an EC2 wildcard",
            filters(&[("bucket", &["ami-archive-*"])]),
            every.clone(),
        ),
        (
            "the bucket filter on a bucket nothing targeted",
            filters(&[("bucket", &["ami-archive-three"])]),
            Vec::new(),
        ),
        (
            "the task-state filter on the state the clock has reached",
            filters(&[("task-state", &[STORE_TASK_STATE_COMPLETED])]),
            every.clone(),
        ),
        (
            "the task-state filter on the state the clock has left",
            filters(&[("task-state", &[STORE_TASK_STATE_IN_PROGRESS])]),
            Vec::new(),
        ),
        (
            // The third word of the catalog: nothing here fails an upload, so no task is in
            // it.
            "the task-state filter on the failed state",
            filters(&[("task-state", &[STORE_TASK_STATE_FAILED])]),
            Vec::new(),
        ),
        (
            "two filters, joined with an AND",
            filters(&[
                ("bucket", &["ami-archive-one"]),
                ("task-state", &[STORE_TASK_STATE_COMPLETED]),
            ]),
            vec![third.clone(), first.clone()],
        ),
        (
            "two filters that disagree",
            filters(&[
                ("bucket", &["ami-archive-one"]),
                ("task-state", &[STORE_TASK_STATE_IN_PROGRESS]),
            ]),
            Vec::new(),
        ),
        (
            // "When you specify the ImageIds parameter, any filters that you specify are
            // ignored" (`Filters`).
            "an id and a filter that would have excluded it",
            {
                let mut params = indexed("ImageId", &[second.as_str()]);
                params.extend(filters(&[("bucket", &["ami-archive-one"])]));
                params
            },
            vec![second.clone()],
        ),
        (
            "the page size at the bottom of its published range",
            vec![p("MaxResults", "1")],
            vec![third.clone()],
        ),
        (
            "the page size at the top of its published range",
            vec![p("MaxResults", "200")],
            every.clone(),
        ),
    ];

    for (case, params, expected) in rows {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let reported = elements(&body, "amiId");
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?} in body {body}"
        );
    }

    // One page size BELOW and one ABOVE the modelled `1..200` range are refused before the
    // handler runs, by the boundary validator the model's `@range` drives — so these assert
    // the status, the code being the query protocols' own for a boundary refusal.
    for (case, max_results) in [
        ("one below the published page size", "0"),
        ("one above the published page size", "201"),
    ] {
        let (status, body) = describe(&router, &[p("MaxResults", max_results)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
    }

    // A page of one walks every task newest first, and the last page carries no token:
    // "this value is null when there are no more items to return".
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for step in 0..every.len() {
        let mut params = vec![p("MaxResults", "1")];
        if let Some(carried) = token.clone() {
            params.push(p("NextToken", carried));
        }
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 200,
            "page {step}: expected HTTP 200, got {status} with body {body}"
        );
        let page = elements(&body, "amiId");
        assert_eq!(
            page.len(),
            1,
            "page {step}: a page size of one reports one task, got {page:?} in body {body}"
        );
        walked.extend(page);
        token = element(&body, "nextToken");
        let expected_more = step + 1 < every.len();
        assert_eq!(
            token.is_some(),
            expected_more,
            "page {step}: a token is carried only while a task follows, got body {body}"
        );
    }
    assert_eq!(
        walked, every,
        "paging one at a time reports every task newest first, got {walked:?}"
    );

    // A page size below the number of tasks cuts the first page and names where the next
    // one starts.
    let (status, body) = describe(&router, &[p("MaxResults", "2")]).await;
    assert_eq!(status, 200, "a page of two: got {status} with body {body}");
    assert_eq!(
        elements(&body, "amiId"),
        vec![third.clone(), second.clone()],
        "a page of two reports the two newest, got body {body}"
    );
    let token = element(&body, "nextToken").expect("a page of two leaves one task");
    let (status, body) = describe(&router, &[p("MaxResults", "2"), p("NextToken", token)]).await;
    assert_eq!(status, 200, "the second page: got {status} with body {body}");
    assert_eq!(
        elements(&body, "amiId"),
        vec![first.clone()],
        "the second page reports the rest, got body {body}"
    );
    assert!(
        element(&body, "nextToken").is_none(),
        "the last page carries no token, got body {body}"
    );
}

/// One row of a refusal table: what the case is, the parameters the call carries, and the
/// HTTP status and EC2 error code it must be refused with.
type RefusalRow = (&'static str, Vec<(String, String)>, u16, &'static str);

/// RULE 3: what the describe refuses, and the code it refuses it with.
#[tokio::test]
async fn the_describe_refuses_a_bad_request() {
    let (router, engine) = super::ec2_query::emulator();
    let image_id = seed_images(&engine, 1).remove(0);
    let (status, body) = store(&router, &image_id, "ami-archive").await;
    assert_eq!(status, 200, "the create: got {status} with body {body}");

    let twenty_one: Vec<&str> = (0..21).map(|_| UNKNOWN_IMAGE_ID).collect();
    let rows: Vec<RefusalRow> = vec![
        (
            "an id that is not an AMI id at all",
            indexed("ImageId", &[MALFORMED_IMAGE_ID]),
            400,
            "InvalidAMIID.Malformed",
        ),
        (
            "one id above the twenty a request may name",
            indexed("ImageId", &twenty_one),
            400,
            "InvalidParameterValue",
        ),
        (
            // "You cannot specify this parameter and the ImageIds parameter in the same
            // call" (`MaxResults`).
            "a page size beside the ids",
            {
                let mut params = indexed("ImageId", &[image_id.as_str()]);
                params.push(p("MaxResults", "1"));
                params
            },
            400,
            "InvalidParameterCombination",
        ),
        (
            "a filter name the operation does not publish",
            filters(&[("ami-id", &[UNKNOWN_IMAGE_ID])]),
            400,
            "InvalidParameterValue",
        ),
        (
            "a token that came from nowhere",
            vec![p("NextToken", "not-a-token")],
            400,
            "InvalidPaginationToken",
        ),
        (
            // "If you have the required permissions, the error response is
            // DryRunOperation" (`DryRun`), which EC2 answers with HTTP 412.
            "a dry run of a request that would have succeeded",
            vec![p("DryRun", "true")],
            412,
            "DryRunOperation",
        ),
        (
            // A dry run of a request that would have been REFUSED is refused with that
            // reason instead.
            "a dry run of a request that would have been refused",
            {
                let mut params = indexed("ImageId", &[MALFORMED_IMAGE_ID]);
                params.push(p("DryRun", "true"));
                params
            },
            400,
            "InvalidAMIID.Malformed",
        ),
    ];

    for (case, params, expected_status, expected) in rows {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected,
            "case {case:?}: expected the code {expected:?}, got {code:?} in body {body}"
        );
    }

    assert_eq!(
        stored_count(&engine),
        1,
        "a refused describe neither stores nor drops a task"
    );
}

/// RULE 3: what the create refuses, and the code it refuses it with. Nothing is stored by
/// any of them.
#[tokio::test]
async fn the_create_refuses_a_bad_request() {
    let (router, engine) = super::ec2_query::emulator();
    let image_id = seed_images(&engine, 1).remove(0);

    // The bad-bucket rows differ along every way the Amazon S3 general purpose bucket
    // naming rules can be broken (`crate::state::s3_bucket_name_is_well_formed`, which
    // cites each sentence): one character below the published floor, one above the ceiling,
    // an uppercase letter, a character the rules do not admit, a dot or a hyphen at either
    // end, two adjacent dots, a name formatted as an IP address, and a reserved prefix and
    // suffix.
    let too_short = bucket_of_length(MIN_S3_BUCKET_NAME_CHARS - 1);
    let too_long = bucket_of_length(MAX_S3_BUCKET_NAME_CHARS + 1);
    let bad_buckets: Vec<(&str, String)> = vec![
        ("an empty bucket name", String::new()),
        ("one character below the shortest name", too_short),
        ("one character above the longest name", too_long),
        ("an uppercase letter", "AMI-Archive".to_string()),
        ("a character the rules do not admit", "ami_archive".to_string()),
        ("a leading dot", ".ami-archive".to_string()),
        ("a trailing hyphen", "ami-archive-".to_string()),
        ("two adjacent dots", "ami..archive".to_string()),
        ("a name formatted as an IP address", "192.168.5.4".to_string()),
        (
            "a reserved prefix",
            format!("{}ami-archive", S3_BUCKET_NAME_RESERVED_PREFIXES[0]),
        ),
        (
            "a reserved suffix",
            format!("ami-archive{}", S3_BUCKET_NAME_RESERVED_SUFFIXES[0]),
        ),
    ];
    for (case, bucket) in &bad_buckets {
        let (status, body) = store(&router, &image_id, bucket).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "InaccessibleStorageLocation",
            "case {case:?}: expected the code \"InaccessibleStorageLocation\", got {code:?} \
             in body {body}"
        );
    }

    let rows: Vec<RefusalRow> = vec![
        (
            "an AMI id the account can see no image for",
            vec![p("ImageId", UNKNOWN_IMAGE_ID), p("Bucket", "ami-archive")],
            400,
            "InvalidAMIID.NotFound",
        ),
        (
            "an id that is not an AMI id at all",
            vec![p("ImageId", MALFORMED_IMAGE_ID), p("Bucket", "ami-archive")],
            400,
            "InvalidAMIID.Malformed",
        ),
        (
            // Both members are `required`, so the boundary validator refuses a request that
            // omits one before the handler runs; the code is the query protocols' own for a
            // boundary refusal.
            "no bucket named",
            vec![p("ImageId", image_id.as_str())],
            400,
            "MalformedInput",
        ),
        (
            "no AMI named",
            vec![p("Bucket", "ami-archive")],
            400,
            "MalformedInput",
        ),
        (
            // A bucket member that is PRESENT but empty reaches the handler, where it is not
            // a bucket name.
            "an empty bucket name",
            vec![p("ImageId", image_id.as_str()), p("Bucket", "")],
            400,
            "InaccessibleStorageLocation",
        ),
        (
            "a dry run of a request that would have succeeded",
            vec![
                p("ImageId", image_id.as_str()),
                p("Bucket", "ami-archive"),
                p("DryRun", "true"),
            ],
            412,
            "DryRunOperation",
        ),
        (
            // A dry run of a request that would have been REFUSED is refused with that
            // reason instead.
            "a dry run of a request that would have been refused",
            vec![
                p("ImageId", UNKNOWN_IMAGE_ID),
                p("Bucket", "ami-archive"),
                p("DryRun", "true"),
            ],
            400,
            "InvalidAMIID.NotFound",
        ),
    ];
    for (case, params, expected_status, expected) in rows {
        let (status, body) = api(&router, "CreateStoreImageTask", &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected,
            "case {case:?}: expected the code {expected:?}, got {code:?} in body {body}"
        );
    }

    assert_eq!(
        stored_count(&engine),
        0,
        "a refused create stores nothing"
    );
    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "the report after the refusals: got {status} with body {body}"
    );
    assert!(
        elements(&body, "amiId").is_empty(),
        "a refused create leaves no task to report, got body {body}"
    );
}
