// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the Earth Observation job slice of
//! `sagemaker-geospatial`: every assertion drives the service over its restJson1
//! wire through the same router the production entrypoint serves, so what is
//! checked is the HTTP behaviour and not an internal helper.
//!
//! The rule under test throughout: each operation computes its response from the
//! persisted record, so a create is observable from a later read, a tag change
//! and a stop are observable from a later get, and a delete is observable as a
//! 404. Every table row is a different input along the axis the rule generalises
//! over — job config variant, tag set, ordering key, page size, status filter —
//! and the recorded happy path is one row among them.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names this service's
/// signing name and the region an ARN minted for the request must carry. The
/// router routes on this scope; signature verification is off for these tests.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
Credential=AKIAEMULATOREXAMPLE/20260101/us-east-1/sagemaker-geospatial/aws4_request, \
SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The region of [`AUTHORIZATION`]'s credential scope.
const REGION: &str = "us-east-1";

/// The account id every ARN a generated emulator emits carries.
const ACCOUNT: &str = "000000000000";

/// An `ExecutionRoleArn` satisfying the model's pattern
/// `^arn:(aws[a-z-]*):iam::([0-9]{12}):role/[a-zA-Z0-9+=,.@_/-]+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/sagemaker-geospatial-demo";

/// A `RasterDataCollectionArn` satisfying the model's pattern
/// `^arn:aws[a-z-]{0,12}:sagemaker-geospatial:[a-z0-9-]{1,25}:[0-9]{12}:raster-data-collection/(public|premium|user)/[a-z0-9]{12,}$`.
const COLLECTION_ARN: &str = "arn:aws:sagemaker-geospatial:us-east-1:000000000000:\
raster-data-collection/public/nmqj48dcu3g7ayw8";

/// The whole status timeline is at most this long: two phases of
/// `crate::state::JOB_PHASE` plus slack for a loaded test machine.
const TIMELINE_BUDGET: Duration = Duration::from_secs(10);

/// One response: the status, the error code when the service refused, and the
/// decoded JSON body.
struct Answer {
    status: u16,
    code: String,
    body: Value,
}

impl Answer {
    fn field(&self, name: &str) -> &Value {
        &self.body[name]
    }

    fn string(&self, name: &str) -> String {
        self.body[name].as_str().unwrap_or_default().to_string()
    }
}

/// The router the production entrypoint serves, carrying this one service over a
/// single shared engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_sagemakergeospatial_sagemakergeospatial::EmulatedService::new(engine),
    );
    router
}

/// Drive one request through the router and decode what came back.
async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Option<Value>) -> Answer {
    let payload = match body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(&value).expect("serializable body")),
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://sagemaker-geospatial.{REGION}.amazonaws.com{path}"))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    let code = body
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Answer { status, code, body }
}

/// Percent-encode a value for one `@httpLabel` path segment, so an ARN's `:` and
/// `/` travel as one segment the way an AWS SDK sends them.
fn label(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(byte));
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Whether `arn` satisfies the model's `EarthObservationJobArn` pattern
/// `^arn:aws[a-z-]{0,12}:sagemaker-geospatial:[a-z0-9-]{1,25}:[0-9]{12}:earth-observation-job/[a-z0-9]{12,}$`,
/// checked field by field so the failure says which field is wrong.
fn is_job_arn(arn: &str) -> bool {
    let fields: Vec<&str> = arn.split(':').collect();
    if fields.len() != 6 || fields[0] != "arn" || fields[2] != "sagemaker-geospatial" {
        return false;
    }
    let Some(partition_suffix) = fields[1].strip_prefix("aws") else {
        return false;
    };
    if partition_suffix.len() > 12
        || !partition_suffix
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b == b'-')
    {
        return false;
    }
    let region = fields[3];
    if !(1..=25).contains(&region.len())
        || !region
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return false;
    }
    if fields[4].len() != 12 || !fields[4].bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let Some(id) = fields[5].strip_prefix("earth-observation-job/") else {
        return false;
    };
    id.len() >= 12 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// A `StartEarthObservationJob` request body: a raster data collection query plus
/// the given job config and tags.
fn start_body(name: &str, job_config: Value, tags: Option<Value>) -> Value {
    let mut body = json!({
        "Name": name,
        "ExecutionRoleArn": ROLE_ARN,
        "InputConfig": {
            "RasterDataCollectionQuery": {
                "RasterDataCollectionArn": COLLECTION_ARN,
                // `TimeRangeFilterInput` carries no `@timestampFormat`, so
                // restJson1's default applies: epoch seconds.
                "TimeRangeFilter": { "StartTime": 1_704_067_200, "EndTime": 1_706_745_600 }
            }
        },
        "JobConfig": job_config,
    });
    if let Some(tags) = tags {
        body["Tags"] = tags;
    }
    body
}

async fn start(router: &MultiServiceRouter, body: Value) -> Answer {
    call(router, "POST", "/earth-observation-jobs", Some(body)).await
}

async fn get(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/earth-observation-jobs/{}", label(arn)),
        None,
    )
    .await
}

async fn list(router: &MultiServiceRouter, body: Value) -> Answer {
    call(router, "POST", "/list-earth-observation-jobs", Some(body)).await
}

async fn stop(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        "POST",
        "/earth-observation-jobs/stop",
        Some(json!({ "Arn": arn })),
    )
    .await
}

async fn delete(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/earth-observation-jobs/{}", label(arn)),
        None,
    )
    .await
}

async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(router, "GET", &format!("/tags/{}", label(arn)), None).await
}

async fn tag(router: &MultiServiceRouter, arn: &str, tags: Value) -> Answer {
    call(
        router,
        "PUT",
        &format!("/tags/{}", label(arn)),
        Some(json!({ "Tags": tags })),
    )
    .await
}

/// Poll `GetEarthObservationJob` until the job's status is one of `wanted`.
///
/// The clock owes the transitions; the poll only observes them, so this never
/// drives the timeline forward by itself.
async fn poll_until(router: &MultiServiceRouter, arn: &str, wanted: &[&str], case: &str) -> Answer {
    let deadline = Instant::now() + TIMELINE_BUDGET;
    let mut last = get(router, arn).await;
    while Instant::now() < deadline {
        let status = last.string("Status");
        if wanted.contains(&status.as_str()) {
            return last;
        }
        std::thread::sleep(Duration::from_millis(50));
        last = get(router, arn).await;
    }
    panic!(
        "case {case:?}: expected the job to reach one of {wanted:?} within \
         {TIMELINE_BUDGET:?}, last status was {:?} (body {})",
        last.string("Status"),
        last.body
    );
}

/// A create is read back out of the store, member for member, for every job
/// config the union offers and with and without optional members.
#[tokio::test]
async fn a_started_job_is_read_back_from_the_store() {
    // One row per `JobConfigInput` union member this slice can be driven with,
    // crossed with the optional members a caller may or may not send.
    let cases: Vec<(&str, Value, Option<Value>, Option<&str>)> = vec![
        (
            "band-math",
            json!({ "BandMathConfig": { "PredefinedIndices": ["NDVI"] } }),
            Some(json!({ "env": "test", "team": "geo" })),
            Some("alias/geospatial"),
        ),
        (
            "cloud-removal-no-tags",
            json!({ "CloudRemovalConfig": { "AlgorithmName": "INTERPOLATION" } }),
            None,
            None,
        ),
        (
            "temporal-statistics",
            json!({ "TemporalStatisticsConfig": { "Statistics": ["MEAN", "MEDIAN"] } }),
            Some(json!({ "env": "prod" })),
            None,
        ),
        (
            "land-cover-segmentation",
            json!({ "LandCoverSegmentationConfig": {} }),
            Some(json!({})),
            Some("alias/aws-sagemaker-geospatial"),
        ),
    ];

    for (case, job_config, tags, kms_key_id) in cases {
        let router = router();
        let mut body = start_body(case, job_config.clone(), tags.clone());
        if let Some(kms) = kms_key_id {
            body["KmsKeyId"] = json!(kms);
        }

        let started = start(&router, body).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected StartEarthObservationJob to answer 200, got {} ({})",
            started.status, started.body
        );
        let arn = started.string("Arn");
        assert!(
            is_job_arn(&arn),
            "case {case:?}: expected a minted ARN matching the modelled \
             EarthObservationJobArn pattern, got {arn:?}"
        );
        assert!(
            arn.contains(&format!(":{REGION}:{ACCOUNT}:")),
            "case {case:?}: expected the ARN to carry the request's region and the \
             emulator account, got {arn:?}"
        );
        assert_eq!(
            started.string("Status"),
            "INITIALIZING",
            "case {case:?}: expected a started job to open in INITIALIZING, got {:?}",
            started.string("Status")
        );
        assert_eq!(
            started.string("Name"),
            case,
            "case {case:?}: expected the submitted Name back, got {:?}",
            started.string("Name")
        );
        assert_eq!(
            started.field("JobConfig"),
            &job_config,
            "case {case:?}: expected the submitted JobConfig back, got {}",
            started.field("JobConfig")
        );
        assert!(
            started
                .field("CreationTime")
                .as_f64()
                .is_some_and(|secs| secs > 0.0),
            "case {case:?}: expected CreationTime as a timestamp in restJson1's default \
             epoch-seconds form (the framework's codegen carries no @timestampFormat), got {}",
            started.field("CreationTime")
        );
        assert_eq!(
            started.field("InputConfig")["RasterDataCollectionQuery"]
                ["RasterDataCollectionArn"]
                .as_str()
                .unwrap_or_default(),
            COLLECTION_ARN,
            "case {case:?}: expected the submitted collection ARN in InputConfigOutput, got {}",
            started.field("InputConfig")
        );

        // Reading it back through Get reports the same record.
        let fetched = get(&router, &arn).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected GetEarthObservationJob to answer 200, got {} ({})",
            fetched.status, fetched.body
        );
        for member in [
            "Arn",
            "Name",
            "CreationTime",
            "ExecutionRoleArn",
            "JobConfig",
            "KmsKeyId",
            "Tags",
        ] {
            assert_eq!(
                fetched.field(member),
                started.field(member),
                "case {case:?}: expected Get to report the same {member} as Start, got {} vs {}",
                fetched.field(member),
                started.field(member)
            );
        }
        assert_eq!(
            fetched.string("ExecutionRoleArn"),
            ROLE_ARN,
            "case {case:?}: expected the submitted ExecutionRoleArn back, got {:?}",
            fetched.string("ExecutionRoleArn")
        );
        match kms_key_id {
            Some(kms) => assert_eq!(
                fetched.string("KmsKeyId"),
                kms,
                "case {case:?}: expected the submitted KmsKeyId back, got {:?}",
                fetched.string("KmsKeyId")
            ),
            None => assert!(
                fetched.field("KmsKeyId").is_null(),
                "case {case:?}: expected no KmsKeyId for a request that sent none, got {}",
                fetched.field("KmsKeyId")
            ),
        }

        // The tag map the create persisted is exactly what the tag read reports.
        let tags_read = list_tags(&router, &arn).await;
        assert_eq!(
            tags_read.status, 200,
            "case {case:?}: expected ListTagsForResource to answer 200, got {} ({})",
            tags_read.status, tags_read.body
        );
        assert_eq!(
            tags_read.field("Tags"),
            &tags.clone().unwrap_or_else(|| json!({})),
            "case {case:?}: expected the tag map the create persisted, got {}",
            tags_read.field("Tags")
        );

        // And the job appears in the listing, with the operation type its stored
        // job config names.
        let listed = list(&router, json!({})).await;
        let summaries = listed.field("EarthObservationJobSummaries");
        assert_eq!(
            summaries.as_array().map(Vec::len),
            Some(1),
            "case {case:?}: expected exactly the one started job in the listing, got {summaries}"
        );
        let expected_operation = job_config
            .as_object()
            .and_then(|o| o.keys().next().cloned())
            .unwrap_or_default();
        assert_eq!(
            summaries[0]["OperationType"].as_str().unwrap_or_default(),
            expected_operation,
            "case {case:?}: expected OperationType {expected_operation:?}, got {}",
            summaries[0]["OperationType"]
        );
        assert_eq!(
            summaries[0]["Arn"].as_str().unwrap_or_default(),
            arn,
            "case {case:?}: expected the summary to carry the minted ARN, got {}",
            summaries[0]["Arn"]
        );
    }
}

/// The status timeline the clock owes: a started job is non-terminal at first —
/// and refuses deletion while it is — then reaches a terminal status on its own,
/// after which it can be deleted and is gone from every read.
#[tokio::test]
async fn the_clock_drives_the_status_timeline_and_delete_is_observable() {
    let router = router();
    let started = start(
        &router,
        start_body(
            "timeline",
            json!({ "BandMathConfig": { "PredefinedIndices": ["NDVI"] } }),
            None,
        ),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case \"timeline\": expected 200 from Start, got {} ({})",
        started.status, started.body
    );
    let arn = started.string("Arn");

    // A caller acting straight after the start still sees the transient status,
    // and the state error that goes with a job that is still running.
    let immediately = get(&router, &arn).await;
    assert_eq!(
        immediately.string("Status"),
        "INITIALIZING",
        "case \"timeline\": expected INITIALIZING straight after the start, got {:?}",
        immediately.string("Status")
    );
    let refused = delete(&router, &arn).await;
    assert_eq!(
        (refused.status, refused.code.as_str()),
        (409, "ConflictException"),
        "case \"timeline\": expected 409 ConflictException deleting a running job, got {} {} ({})",
        refused.status,
        refused.code,
        refused.body
    );

    // Left alone, the job reaches a terminal status. The run itself cannot be
    // performed in-process, so the terminal status is the modelled failure one
    // and it carries the reason.
    let terminal = poll_until(
        &router,
        &arn,
        &["FAILED", "COMPLETED", "STOPPED"],
        "timeline",
    )
    .await;
    assert_eq!(
        terminal.string("Status"),
        "FAILED",
        "case \"timeline\": expected the modelled failure status for a run this \
         emulator cannot perform, got {:?}",
        terminal.string("Status")
    );
    assert!(
        terminal.field("ErrorDetails")["Message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "case \"timeline\": expected ErrorDetails to say why the job failed, got {}",
        terminal.field("ErrorDetails")
    );
    assert_eq!(
        terminal.field("ErrorDetails")["Type"]
            .as_str()
            .unwrap_or_default(),
        "SERVER_ERROR",
        "case \"timeline\": expected a SERVER_ERROR error type, got {}",
        terminal.field("ErrorDetails")
    );
    assert!(
        terminal.field("DurationInSeconds").is_number(),
        "case \"timeline\": expected DurationInSeconds as a number, got {}",
        terminal.field("DurationInSeconds")
    );

    // Deleting a terminal job removes it: the next read is a 404 and the listing
    // no longer carries it.
    let deleted = delete(&router, &arn).await;
    assert_eq!(
        deleted.status, 200,
        "case \"timeline\": expected 200 deleting a terminal job, got {} ({})",
        deleted.status, deleted.body
    );
    let after_delete = get(&router, &arn).await;
    assert_eq!(
        (after_delete.status, after_delete.code.as_str()),
        (404, "ResourceNotFoundException"),
        "case \"timeline\": expected 404 ResourceNotFoundException after the delete, got {} {}",
        after_delete.status,
        after_delete.code
    );
    let listed = list(&router, json!({})).await;
    assert_eq!(
        listed.field("EarthObservationJobSummaries")
            .as_array()
            .map(Vec::len),
        Some(0),
        "case \"timeline\": expected the deleted job to be gone from the listing, got {}",
        listed.field("EarthObservationJobSummaries")
    );
    let tags_after_delete = list_tags(&router, &arn).await;
    assert_eq!(
        tags_after_delete.status, 404,
        "case \"timeline\": expected 404 reading the tags of a deleted job, got {} ({})",
        tags_after_delete.status, tags_after_delete.body
    );
}

/// Stopping a job is observable from a later get, and a job that already reached
/// a terminal status cannot be stopped.
#[tokio::test]
async fn a_stop_is_observable_from_a_later_get() {
    let router = router();
    let arn = start(
        &router,
        start_body("stoppable", json!({ "CloudMaskingConfig": {} }), None),
    )
    .await
    .string("Arn");

    let stopped = stop(&router, &arn).await;
    assert_eq!(
        stopped.status, 200,
        "case \"stoppable\": expected 200 from Stop, got {} ({})",
        stopped.status, stopped.body
    );
    let stopping = get(&router, &arn).await;
    assert_eq!(
        stopping.string("Status"),
        "STOPPING",
        "case \"stoppable\": expected STOPPING straight after the stop, got {:?}",
        stopping.string("Status")
    );

    let settled = poll_until(&router, &arn, &["STOPPED"], "stoppable").await;
    assert_eq!(
        settled.string("Status"),
        "STOPPED",
        "case \"stoppable\": expected the stop to settle at STOPPED, got {:?}",
        settled.string("Status")
    );

    let again = stop(&router, &arn).await;
    assert_eq!(
        (again.status, again.code.as_str()),
        (409, "ConflictException"),
        "case \"stoppable\": expected 409 ConflictException stopping a terminal job, got {} {}",
        again.status,
        again.code
    );

    // A stopped job is terminal, so it deletes.
    assert_eq!(
        delete(&router, &arn).await.status,
        200,
        "case \"stoppable\": expected a stopped job to delete"
    );
}

/// `ListEarthObservationJobs` filters on `StatusEquals`, orders by
/// `SortBy`/`SortOrder`, and pages at `MaxResults` with a token that resumes
/// exactly where the page stopped.
#[tokio::test]
async fn listing_filters_sorts_and_pages() {
    let router = router();
    // Names chosen so the alphabetical order differs from the creation order.
    let names = ["delta", "alpha", "echo", "bravo", "charlie"];
    let mut arns = Vec::new();
    for name in names {
        let answer = start(
            &router,
            start_body(name, json!({ "GeoMosaicConfig": {} }), None),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {name:?}: expected 200 from Start, got {} ({})",
            answer.status, answer.body
        );
        arns.push(answer.string("Arn"));
    }

    // Ordering: one row per (SortBy, SortOrder) pair, each with the order the
    // stored records put the names in.
    let mut by_name: Vec<&str> = names.to_vec();
    by_name.sort_unstable();
    let mut by_name_desc = by_name.clone();
    by_name_desc.reverse();
    let creation_order: Vec<&str> = names.to_vec();
    let mut creation_order_desc = creation_order.clone();
    creation_order_desc.reverse();

    let ordering_cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "name-ascending",
            json!({ "SortBy": "Name", "SortOrder": "ASCENDING" }),
            by_name.clone(),
        ),
        (
            "name-descending",
            json!({ "SortBy": "Name", "SortOrder": "DESCENDING" }),
            by_name_desc,
        ),
        (
            "creation-ascending",
            json!({ "SortBy": "CreationTime", "SortOrder": "ASCENDING" }),
            creation_order,
        ),
        (
            "creation-descending-is-the-default",
            json!({}),
            creation_order_desc,
        ),
    ];
    for (case, request, expected) in ordering_cases {
        let answer = list(&router, request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 from List, got {} ({})",
            answer.status, answer.body
        );
        let got: Vec<String> = answer
            .field("EarthObservationJobSummaries")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|summary| summary["Name"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            got, expected,
            "case {case:?}: expected the names in {expected:?}, got {got:?}"
        );
        assert!(
            answer.field("NextToken").is_null(),
            "case {case:?}: expected no NextToken on a single full page, got {}",
            answer.field("NextToken")
        );
    }

    // Paging: one row per page size, each walking the whole set exactly once.
    for page_size in [1_u64, 2, 3, 5, 20] {
        let case = format!("max-results-{page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "MaxResults": page_size, "SortBy": "Name" });
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let answer = list(&router, request).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected 200 from List, got {} ({})",
                answer.status, answer.body
            );
            let page = answer
                .field("EarthObservationJobSummaries")
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                page.len() as u64 <= page_size,
                "case {case:?}: expected at most {page_size} summaries on a page, got {}",
                page.len()
            );
            for summary in &page {
                seen.push(summary["Arn"].as_str().unwrap_or_default().to_string());
            }
            pages += 1;
            assert!(
                pages <= names.len() + 1,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
            match answer.field("NextToken").as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        let mut unique = seen.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            names.len(),
            "case {case:?}: expected every stored job exactly once across the pages, got {seen:?}"
        );
        let mut expected = arns.clone();
        expected.sort_unstable();
        assert_eq!(
            unique, expected,
            "case {case:?}: expected the pages to cover exactly the stored ARNs, got {unique:?}"
        );
    }

    // Filtering: one row per status, against what the records actually hold.
    let filter_cases: Vec<(&str, usize)> = vec![
        ("INITIALIZING", names.len()),
        ("COMPLETED", 0),
        ("STOPPED", 0),
        ("DELETED", 0),
    ];
    for (status, expected) in filter_cases {
        let answer = list(&router, json!({ "StatusEquals": status })).await;
        let got = answer
            .field("EarthObservationJobSummaries")
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            got, expected,
            "case {status:?}: expected {expected} summaries for this status filter, got {got} ({})",
            answer.body
        );
    }

    // Each published `MaxResults` bound is accepted and one past it is refused:
    // the model declares `@range(min: 1, max: 20)`.
    for (case, max_results, expected_status) in [
        ("min-bound", 1_i64, 200_u16),
        ("max-bound", 20, 200),
        ("below-min", 0, 400),
        ("above-max", 21, 400),
    ] {
        let answer = list(&router, json!({ "MaxResults": max_results })).await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected MaxResults={max_results} to answer {expected_status}, \
             got {} ({})",
            answer.status, answer.body
        );
    }

    // A sort key this service cannot order by, and a token it did not issue, are
    // refused as ValidationException rather than silently ignored. The oversized
    // row is still inside the model's `@length(max: 8192)` on `NextToken`, so it
    // reaches the handler and is refused by the handler's own tighter bound.
    for (case, request) in [
        ("unknown-sort-by", json!({ "SortBy": "NotAMember" })),
        ("forged-token", json!({ "NextToken": "not-a-token-we-issued" })),
        ("oversized-token", json!({ "NextToken": "A".repeat(4096) })),
    ] {
        let answer = list(&router, request).await;
        assert_eq!(
            (answer.status, answer.code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} {} ({})",
            answer.status,
            answer.code,
            answer.body
        );
    }
}

/// A repeated `StartEarthObservationJob` carrying the same `ClientToken` reports
/// the job the first call minted; a different token mints a different job.
#[tokio::test]
async fn a_repeated_client_token_returns_the_first_job() {
    let router = router();
    let token_a = "11111111-1111-1111-1111-111111111111";
    let token_b = "22222222-2222-2222-2222-222222222222";

    let mut first_body = start_body("idempotent", json!({ "StackConfig": {} }), None);
    first_body["ClientToken"] = json!(token_a);
    let first = start(&router, first_body.clone()).await;
    assert_eq!(
        first.status, 200,
        "case \"first\": expected 200 from Start, got {} ({})",
        first.status, first.body
    );

    let repeat = start(&router, first_body).await;
    assert_eq!(
        repeat.string("Arn"),
        first.string("Arn"),
        "case \"repeat\": expected the first job's ARN back for a repeated ClientToken, got {:?}",
        repeat.string("Arn")
    );

    let mut other_body = start_body("idempotent", json!({ "StackConfig": {} }), None);
    other_body["ClientToken"] = json!(token_b);
    let other = start(&router, other_body).await;
    assert_ne!(
        other.string("Arn"),
        first.string("Arn"),
        "case \"other-token\": expected a different ClientToken to mint a new job, got {:?}",
        other.string("Arn")
    );

    let listed = list(&router, json!({})).await;
    assert_eq!(
        listed.field("EarthObservationJobSummaries")
            .as_array()
            .map(Vec::len),
        Some(2),
        "case \"repeat\": expected two stored jobs for two distinct tokens, got {}",
        listed.field("EarthObservationJobSummaries")
    );
}

/// `TagResource` writes into the same store the create wrote, so the resource's
/// own read and the tag read both report the change.
#[tokio::test]
async fn tagging_is_observable_from_every_read() {
    let router = router();
    let arn = start(
        &router,
        start_body(
            "taggable",
            json!({ "ZonalStatisticsConfig": {
                "ZoneS3Path": "s3://example-bucket/zones.geojson",
                "Statistics": ["MEAN"]
            } }),
            Some(json!({ "env": "test", "owner": "geo" })),
        ),
    )
    .await
    .string("Arn");

    // One row per kind of change: a new key, an overwrite, and both at once.
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "add-a-key",
            json!({ "stage": "one" }),
            json!({ "env": "test", "owner": "geo", "stage": "one" }),
        ),
        (
            "overwrite-a-key",
            json!({ "env": "prod" }),
            json!({ "env": "prod", "owner": "geo", "stage": "one" }),
        ),
        (
            "add-and-overwrite",
            json!({ "owner": "platform", "region": REGION }),
            json!({
                "env": "prod",
                "owner": "platform",
                "stage": "one",
                "region": REGION
            }),
        ),
    ];
    for (case, change, expected) in cases {
        let tagged = tag(&router, &arn, change.clone()).await;
        assert_eq!(
            tagged.status, 200,
            "case {case:?}: expected 200 from TagResource, got {} ({})",
            tagged.status, tagged.body
        );
        let read = list_tags(&router, &arn).await;
        assert_eq!(
            read.field("Tags"),
            &expected,
            "case {case:?}: expected the merged tag map, got {}",
            read.field("Tags")
        );
        let fetched = get(&router, &arn).await;
        assert_eq!(
            fetched.field("Tags"),
            &expected,
            "case {case:?}: expected the job read to report the same tags, got {}",
            fetched.field("Tags")
        );
    }

    // The tag map is bounded, so repeated calls cannot grow one record without
    // limit: AWS's tagging restrictions cap a resource at 50 tags, and a request
    // that would take it past that is refused while the stored map is untouched.
    // On its own, untagged job, so the rows below count only their own keys.
    use service_com_amazonaws_sagemakergeospatial_sagemakergeospatial::state::MAX_TAGS_PER_RESOURCE;
    let bounded_arn = start(
        &router,
        start_body("tag-bound", json!({ "CloudMaskingConfig": {} }), None),
    )
    .await
    .string("Arn");
    let before = list_tags(&router, &bounded_arn).await;
    let mut too_many = serde_json::Map::new();
    for index in 0..=MAX_TAGS_PER_RESOURCE {
        too_many.insert(format!("key-{index}"), json!("value"));
    }
    let refused = tag(&router, &bounded_arn, Value::Object(too_many)).await;
    assert_eq!(
        (refused.status, refused.code.as_str()),
        (400, "ValidationException"),
        "case \"over-the-tag-bound\": expected 400 ValidationException past \
         {MAX_TAGS_PER_RESOURCE} tags, got {} {} ({})",
        refused.status,
        refused.code,
        refused.body
    );
    let after = list_tags(&router, &bounded_arn).await;
    assert_eq!(
        after.field("Tags"),
        before.field("Tags"),
        "case \"over-the-tag-bound\": expected the refused call to leave the stored \
         tags untouched, got {} vs {}",
        after.field("Tags"),
        before.field("Tags")
    );

    // Exactly at the bound is accepted.
    let mut at_bound = serde_json::Map::new();
    for index in 0..MAX_TAGS_PER_RESOURCE {
        at_bound.insert(format!("key-{index}"), json!("value"));
    }
    let accepted = tag(&router, &bounded_arn, Value::Object(at_bound)).await;
    assert_eq!(
        accepted.status, 200,
        "case \"at-the-tag-bound\": expected {MAX_TAGS_PER_RESOURCE} tags to be \
         accepted, got {} ({})",
        accepted.status, accepted.body
    );
    assert_eq!(
        list_tags(&router, &bounded_arn)
            .await
            .field("Tags")
            .as_object()
            .map(serde_json::Map::len),
        Some(MAX_TAGS_PER_RESOURCE),
        "case \"at-the-tag-bound\": expected the resource to carry exactly \
         {MAX_TAGS_PER_RESOURCE} tags"
    );
}

/// Every operation that addresses a job by ARN answers 404 for an ARN this
/// service holds no record of, and the modelled 400 for an ARN outside the
/// pattern.
#[tokio::test]
async fn unknown_and_malformed_arns_are_refused() {
    let router = router();
    // Well formed under the model's pattern, and stored nowhere.
    let unknown = format!(
        "arn:aws:sagemaker-geospatial:{REGION}:{ACCOUNT}:earth-observation-job/zzzzzzzzzzzz"
    );
    assert!(
        is_job_arn(&unknown),
        "the unknown ARN must itself satisfy the modelled pattern, got {unknown:?}"
    );

    assert_eq!(
        (get(&router, &unknown).await.status, get(&router, &unknown).await.code),
        (404, "ResourceNotFoundException".to_string()),
        "case \"get-unknown\": expected 404 ResourceNotFoundException"
    );
    for (case, answer) in [
        ("stop-unknown", stop(&router, &unknown).await),
        ("delete-unknown", delete(&router, &unknown).await),
        ("list-tags-unknown", list_tags(&router, &unknown).await),
        (
            "tag-unknown",
            tag(&router, &unknown, json!({ "env": "test" })).await,
        ),
    ] {
        assert_eq!(
            (answer.status, answer.code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected 404 ResourceNotFoundException, got {} {} ({})",
            answer.status,
            answer.code,
            answer.body
        );
    }

    // Outside the modelled `EarthObservationJobArn` pattern: a different service,
    // a non-numeric account, too short a resource id, and not an ARN at all.
    let malformed = [
        format!("arn:aws:sagemaker:{REGION}:{ACCOUNT}:earth-observation-job/zzzzzzzzzzzz"),
        format!("arn:aws:sagemaker-geospatial:{REGION}:not-an-account:earth-observation-job/zzzzzzzzzzzz"),
        format!("arn:aws:sagemaker-geospatial:{REGION}:{ACCOUNT}:earth-observation-job/short"),
        "not-an-arn".to_string(),
    ];
    for arn in malformed {
        assert!(
            !is_job_arn(&arn),
            "case {arn:?}: this row must be outside the modelled pattern"
        );
        let answer = get(&router, &arn).await;
        assert_eq!(
            answer.status, 400,
            "case {arn:?}: expected 400 for an ARN outside the modelled pattern, got {} ({})",
            answer.status, answer.body
        );
    }

    // An input config naming neither source, and one naming both, are refused.
    let mut neither = start_body("bad-input", json!({ "CloudMaskingConfig": {} }), None);
    neither["InputConfig"] = json!({});
    let mut both = start_body("bad-input", json!({ "CloudMaskingConfig": {} }), None);
    both["InputConfig"]["PreviousEarthObservationJobArn"] = json!(unknown);
    for (case, body) in [("neither-source", neither), ("both-sources", both)] {
        let answer = start(&router, body).await;
        assert_eq!(
            (answer.status, answer.code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} {} ({})",
            answer.status,
            answer.code,
            answer.body
        );
    }

    // A job chained on a previous job this service holds no record of is a 404.
    let mut chained = start_body("chained", json!({ "CloudMaskingConfig": {} }), None);
    chained["InputConfig"] = json!({ "PreviousEarthObservationJobArn": unknown });
    let answer = start(&router, chained).await;
    assert_eq!(
        (answer.status, answer.code.as_str()),
        (404, "ResourceNotFoundException"),
        "case \"unknown-previous-job\": expected 404 ResourceNotFoundException, got {} {} ({})",
        answer.status,
        answer.code,
        answer.body
    );
}

/// The store is bounded: a caller cannot grow this emulator's memory without
/// limit by looping `StartEarthObservationJob`, and the bound is a bound rather
/// than a wall — freeing a record makes room for another.
#[tokio::test]
async fn the_store_refuses_to_grow_without_bound() {
    use service_com_amazonaws_sagemakergeospatial_sagemakergeospatial::state::MAX_EARTH_OBSERVATION_JOBS;

    let router = router();
    let mut first_arn = String::new();
    for index in 0..MAX_EARTH_OBSERVATION_JOBS {
        let answer = start(
            &router,
            start_body("bounded", json!({ "CloudMaskingConfig": {} }), None),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case \"fill-{index}\": expected 200 while the store has room, got {} ({})",
            answer.status, answer.body
        );
        if index == 0 {
            first_arn = answer.string("Arn");
        }
    }

    let refused = start(
        &router,
        start_body("over-the-bound", json!({ "CloudMaskingConfig": {} }), None),
    )
    .await;
    assert_eq!(
        (refused.status, refused.code.as_str()),
        (402, "ServiceQuotaExceededException"),
        "case \"over-the-bound\": expected the modelled 402 quota error at \
         {MAX_EARTH_OBSERVATION_JOBS} stored jobs, got {} {} ({})",
        refused.status,
        refused.code,
        refused.body
    );

    // Freeing one record makes room for one more, so the cap bounds the store
    // instead of ending the service.
    poll_until(&router, &first_arn, &["FAILED", "STOPPED"], "bounded").await;
    assert_eq!(
        delete(&router, &first_arn).await.status,
        200,
        "case \"bounded\": expected the first job to delete"
    );
    let after_delete = start(
        &router,
        start_body("after-delete", json!({ "CloudMaskingConfig": {} }), None),
    )
    .await;
    assert_eq!(
        after_delete.status, 200,
        "case \"after-delete\": expected a start to succeed once a record was freed, \
         got {} ({})",
        after_delete.status, after_delete.body
    );
}

/// The operations outside this slice still answer 501, so a client can tell
/// "not written yet" from a real answer.
#[tokio::test]
async fn operations_outside_this_slice_still_answer_501() {
    let router = router();
    let answer = call(
        &router,
        "POST",
        "/export-earth-observation-job",
        Some(json!({
            "Arn": format!(
                "arn:aws:sagemaker-geospatial:{REGION}:{ACCOUNT}:earth-observation-job/zzzzzzzzzzzz"
            ),
            "ExecutionRoleArn": ROLE_ARN,
            "OutputConfig": { "S3Data": { "S3Uri": "s3://example-bucket/out/" } },
        })),
    )
    .await;
    assert_eq!(
        answer.status, 501,
        "case \"export\": expected 501 from an operation nobody has written yet, got {} ({})",
        answer.status, answer.body
    );
}
