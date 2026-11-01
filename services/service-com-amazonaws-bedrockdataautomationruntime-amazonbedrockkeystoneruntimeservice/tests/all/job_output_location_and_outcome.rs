// SPDX-License-Identifier: CC0-1.0

//! What a finished job REPORTS: where its output lives, and whether it finished at all.
//!
//! Amazon S3 is mounted beside this service on one engine — the way the assembled
//! emulator runs them — because both rules under test are about the object store: the
//! asset a submission points at is an object of it, and the location a reader reports is
//! an object of it too.
//!
//! THE FIRST RULE: the output location `GetDataAutomationStatus` reports is the JOB's own
//! metadata document under the `outputConfiguration.s3Uri` the submission named —
//! `<prefix>/<invocationId>/job_metadata.json` — which is what a client reads a finished
//! job's results through. The prefix itself names no job (every job submitted under it
//! shares it), so it is never what a reader answers. Asserted over a table whose rows
//! differ along what the rule generalises over: whether the prefix carries a trailing key
//! separator, how many segments it has (the bucket root, one segment, a deep path), and
//! the shape of the bucket name — plus two jobs under ONE prefix, which must report two
//! locations.
//!
//! THE SECOND RULE: a job's work begins by READING the asset it was pointed at, so a job
//! whose asset the object store holds none of cannot carry out the work it was accepted
//! for: it ends in the modelled failure status attributable to the request (`ClientError`)
//! carrying `errorType` and `errorMessage`, never in `Success`. Asserted over a table
//! whose rows differ along WHICH part of the address does not resolve (the bucket, the
//! key, a key that only prefixes or only extends a staged one, a URI that addresses no
//! object at all) beside the staged assets that do resolve.
//!
//! The last two tests pin the other half of that rule. A failure has to be GROUNDED in
//! what the object store answered: with no Amazon S3 mounted to ask, nothing is known
//! about the asset and no job is failed — which is why every other suite in this crate
//! may submit jobs whose buckets no store holds. And the resolution happens when the job
//! RUNS, not when the submission is accepted, so an asset staged after the submission is
//! there for the job.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockdataautomationruntime_amazonbedrockkeystoneruntimeservice as bda;
use service_com_amazonaws_s3_amazons3 as s3;
use smithy_server_runtime::MultiServiceRouter;

/// This service's sigv4 signing name (`OPERATIONS.md`): the credential scope the router
/// routes a data-automation call on.
const SIGNING_NAME: &str = "bedrock";

/// The region every request below signs for, and the region the staged bucket is created
/// in: the handler reads the region out of the SigV4 credential scope and addresses the
/// object store in it, so the two have to agree.
const REGION: &str = "us-west-2";

/// A legal `dataAutomationProfileArn` (pattern from `smithy_query` shape
/// `DataAutomationProfileArn`, whose account segment accepts the literal `aws`).
const PROFILE_ARN: &str =
    "arn:aws:bedrock:us-west-2:aws:data-automation-profile/us.data-automation-v1";

/// The data-automation project every submission below names — the member that says WHAT
/// to extract. Arranged in the owning (build-time) service's store by
/// [`crate::support::seed_project`], because a submission naming a project nobody created
/// is refused before any rule here could be reached.
const PROJECT_ARN: &str = "arn:aws:bedrock:us-west-2:000000000000:data-automation-project/invoices";

/// The bucket the staged assets live in.
const ASSET_BUCKET: &str = "in-bucket";

/// A bucket no staging below ever creates.
const ABSENT_BUCKET: &str = "absent-in-bucket";

/// The keys staged in [`ASSET_BUCKET`]: one at the bucket root and two under nested
/// prefixes, so a row can address an asset that is there and a row can address a key that
/// only prefixes or only extends one.
const STAGED_KEYS: &[&str] = &["doc.pdf", "assets/nested/scan.tiff", "assets/clip.mp4"];

/// The bytes every staged asset holds. NOTHING reads an asset's content — no extraction
/// runs here — so one short payload serves every row.
const ASSET_BYTES: &[u8] = b"%PDF-1.7 emulator data automation asset";

/// The modelled `AutomationJobStatus` values, as `AUTOMATION_JOB_STATUS_VALUES` lists
/// them (`smithy_query` shape `AutomationJobStatus`).
const JOB_STATUS_VALUES: &[&str] = &[
    "ClientError",
    "Created",
    "InProgress",
    "ServiceError",
    "Success",
];

/// The modelled `@length` ceiling of `S3Uri` (`smithy_query` shape `S3Uri`), which the
/// reported `outputConfiguration.s3Uri` is a value of.
const MAX_S3_URI_CHARS: usize = 1024;

/// Long enough for a job submitted during the test to have been owed its terminal
/// transition: both emulated spans, with room for a slow machine.
fn past_the_lifecycle() -> Duration {
    bda::state::CREATED_FOR + bda::state::IN_PROGRESS_FOR + Duration::from_millis(500)
}

/// One emulator: this service and Amazon S3 on a single shared engine behind the
/// production router, with the referenced project arranged and the assets staged.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    crate::support::seed_project(&engine, PROJECT_ARN, "data-automation-invoices");
    stage_assets(&engine);
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(bda::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(s3::EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// An emulator with this service alone: no object store to ask about any asset.
fn emulator_without_the_object_store() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    crate::support::seed_project(&engine, PROJECT_ARN, "data-automation-invoices");
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(bda::EmulatedService::new(engine));
    router
}

/// Put [`STAGED_KEYS`] in [`ASSET_BUCKET`] in the object store on `engine`, the way a
/// caller would have uploaded them before submitting.
fn stage_assets(engine: &Arc<engine::InMemoryEngine>) {
    for key in STAGED_KEYS {
        stage_asset(engine, key);
    }
}

/// Put one object at `key` in [`ASSET_BUCKET`], creating that bucket the first time.
fn stage_asset(engine: &Arc<engine::InMemoryEngine>, key: &str) {
    let state = s3::state::service_state(&**engine).expect("the object store on this engine");
    let mut store = state.lock();
    if store.bucket(ASSET_BUCKET).is_none() {
        assert_eq!(
            store.create_bucket(ASSET_BUCKET, REGION),
            s3::state::BucketCreation::Created,
            "the staging bucket {ASSET_BUCKET:?} must be created"
        );
    }
    let object = s3::state::Object::stored(ASSET_BYTES.to_vec(), None);
    assert_eq!(
        store.put_object(ASSET_BUCKET, key, object),
        Some(s3::state::ObjectWrite::Created),
        "the asset {key:?} must be staged in {ASSET_BUCKET:?}"
    );
}

/// One wire answer: the HTTP status and the parsed JSON body.
struct Answer {
    status: u16,
    body: Value,
}

/// Call one operation of this service on `router` the way an SDK would (awsJson1_1:
/// `POST /` with `X-Amz-Target`).
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header(
            "x-amz-target",
            format!("AmazonBedrockKeystoneRuntimeService.{operation}"),
        )
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(
            serde_json::to_vec(&body).expect("serialize the request body"),
        ))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("a JSON response body")
    };
    Answer { status, body }
}

/// Submit one invocation pointing at `input_s3_uri`, writing its output under
/// `output_s3_uri`, and return its `invocationArn`.
async fn submit(
    router: &MultiServiceRouter,
    case: &str,
    input_s3_uri: &str,
    output_s3_uri: &str,
) -> String {
    let body = json!({
        "inputConfiguration": {"s3Uri": input_s3_uri},
        "outputConfiguration": {"s3Uri": output_s3_uri},
        "dataAutomationProfileArn": PROFILE_ARN,
        "dataAutomationConfiguration": {"dataAutomationProjectArn": PROJECT_ARN},
    });
    let answer = call(router, "InvokeDataAutomationAsync", body).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from InvokeDataAutomationAsync, got {} / {}",
        answer.status, answer.body
    );
    answer.body["invocationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no invocationArn in {}", answer.body))
        .to_string()
}

/// Read one job's status, asserting the call itself succeeded.
async fn read(router: &MultiServiceRouter, case: &str, invocation_arn: &str) -> Value {
    let answer = call(
        router,
        "GetDataAutomationStatus",
        json!({"invocationArn": invocation_arn}),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200 from GetDataAutomationStatus, got {} / {}",
        answer.status, answer.body
    );
    answer.body
}

/// The `outputConfiguration.s3Uri` a status answer reports.
fn reported_output_uri(case: &str, body: &Value) -> String {
    body["outputConfiguration"]["s3Uri"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no outputConfiguration.s3Uri in {body}"))
        .to_string()
}

#[tokio::test]
async fn the_reported_output_location_is_this_jobs_own_metadata_object_under_the_prefix() {
    let (router, _engine) = emulator();

    // Each row carries the prefix the submission names and, SPELLED OUT, the location the
    // job's metadata document sits at under it — so a prefix written with a trailing
    // separator and the same prefix written without one state the same expectation.
    let rows: &[(&str, &str, &str)] = &[
        (
            "a bucket root with no trailing separator",
            "s3://out-bucket",
            "s3://out-bucket",
        ),
        (
            "a bucket root with a trailing separator",
            "s3://out-bucket/",
            "s3://out-bucket",
        ),
        (
            "one prefix segment with a trailing separator",
            "s3://out-bucket/results/",
            "s3://out-bucket/results",
        ),
        (
            "one prefix segment without a trailing separator",
            "s3://out-bucket/results",
            "s3://out-bucket/results",
        ),
        (
            "a deep prefix",
            "s3://out-bucket/team/invoices/2026/",
            "s3://out-bucket/team/invoices/2026",
        ),
        (
            "a bucket name carrying a dot and a dash",
            "s3://out.bucket-1/results/",
            "s3://out.bucket-1/results",
        ),
    ];

    for (case, submitted_prefix, want_base) in rows {
        let arn = submit(
            &router,
            case,
            &format!("s3://{ASSET_BUCKET}/{}", STAGED_KEYS[0]),
            submitted_prefix,
        )
        .await;
        let id = crate::support::invocation_id(&arn);
        let want = format!("{want_base}/{id}/job_metadata.json");

        let body = read(&router, case, &arn).await;
        let reported = reported_output_uri(case, &body);
        assert_eq!(
            reported, want,
            "case {case:?}: the reader must report this job's own metadata location, got {reported:?}"
        );
        assert!(
            reported.starts_with("s3://") && reported.len() <= MAX_S3_URI_CHARS,
            "case {case:?}: the reported location must stay inside the modelled S3Uri bounds, \
             got {} chars in {reported:?}",
            reported.len()
        );

        // The location is a function of the record, so polling again never moves it.
        let again = read(&router, case, &arn).await;
        assert_eq!(
            reported_output_uri(case, &again),
            reported,
            "case {case:?}: a later read must report the same location"
        );
    }

    // The location is the JOB's, not the prefix's: two submissions under one prefix are
    // two jobs, each addressed under its own id.
    let prefix = "s3://out-bucket/shared/";
    let input = format!("s3://{ASSET_BUCKET}/{}", STAGED_KEYS[0]);
    let first = submit(&router, "two jobs, one prefix (a)", &input, prefix).await;
    let second = submit(&router, "two jobs, one prefix (b)", &input, prefix).await;
    let first_body = read(&router, "two jobs, one prefix (a)", &first).await;
    let second_body = read(&router, "two jobs, one prefix (b)", &second).await;
    let first_uri = reported_output_uri("two jobs, one prefix (a)", &first_body);
    let second_uri = reported_output_uri("two jobs, one prefix (b)", &second_body);
    assert_ne!(
        first_uri, second_uri,
        "case two jobs under one prefix: each job must report its own location, got {first_uri:?} twice"
    );
    for (case, arn, uri) in [
        ("two jobs, one prefix (a)", &first, &first_uri),
        ("two jobs, one prefix (b)", &second, &second_uri),
    ] {
        assert_eq!(
            *uri,
            format!(
                "s3://out-bucket/shared/{}/job_metadata.json",
                crate::support::invocation_id(arn)
            ),
            "case {case:?}: the location must carry this job's own id, got {uri:?}"
        );
    }
}

#[tokio::test]
async fn a_job_whose_asset_the_object_store_holds_none_of_fails_instead_of_succeeding() {
    let (router, engine) = emulator();

    // `true` means the asset the row points at is one the store holds, so the job
    // completes; `false` means the store itself answers that there is no such object, so
    // the job cannot do the work it was accepted for. Rows differ in WHICH part of the
    // address does not resolve.
    let rows: &[(&str, String, bool)] = &[
        (
            "an asset at the bucket root",
            format!("s3://{ASSET_BUCKET}/{}", STAGED_KEYS[0]),
            true,
        ),
        (
            "an asset under a nested prefix",
            format!("s3://{ASSET_BUCKET}/{}", STAGED_KEYS[1]),
            true,
        ),
        (
            "another asset under that prefix",
            format!("s3://{ASSET_BUCKET}/{}", STAGED_KEYS[2]),
            true,
        ),
        (
            "a key the bucket does not hold",
            format!("s3://{ASSET_BUCKET}/assets/missing.pdf"),
            false,
        ),
        (
            "a key that only prefixes a staged one",
            format!("s3://{ASSET_BUCKET}/assets/nested/scan"),
            false,
        ),
        (
            "a key that only extends a staged one",
            format!("s3://{ASSET_BUCKET}/{}.bak", STAGED_KEYS[0]),
            false,
        ),
        (
            "a bucket the store does not hold",
            format!("s3://{ABSENT_BUCKET}/{}", STAGED_KEYS[0]),
            false,
        ),
        (
            "a URI naming the bucket alone, which addresses no object",
            format!("s3://{ASSET_BUCKET}"),
            false,
        ),
    ];

    let mut arns: Vec<(&str, String, bool)> = Vec::new();
    for (case, input_s3_uri, resolves) in rows {
        let arn = submit(&router, case, input_s3_uri, "s3://out-bucket/outcome/").await;
        arns.push((case, arn, *resolves));
    }

    // One wait for every job: each was submitted before it, so the clock owes each of
    // them its terminal transition by now.
    tokio::time::sleep(past_the_lifecycle()).await;

    for (case, arn, resolves) in &arns {
        let body = read(&router, case, arn).await;
        let status = body["status"].as_str().unwrap_or_default();
        assert!(
            JOB_STATUS_VALUES.contains(&status),
            "case {case:?}: {status:?} is not a modelled AutomationJobStatus"
        );
        let want_status = if *resolves { "Success" } else { "ClientError" };
        assert_eq!(
            status, want_status,
            "case {case:?}: expected {want_status:?}, got {body}"
        );

        // A job that finished reports when it finished, whichever way it went.
        assert!(
            !body["jobCompletionTime"].is_null(),
            "case {case:?}: a finished job must report a completion time, got {body}"
        );
        assert!(
            !body["jobDurationInSeconds"].is_null(),
            "case {case:?}: a finished job must report a duration, got {body}"
        );

        if *resolves {
            assert!(
                body["errorType"].is_null() && body["errorMessage"].is_null(),
                "case {case:?}: a job that completed carries no error members, got {body}"
            );
        } else {
            assert_eq!(
                body["errorType"].as_str(),
                Some("ClientError"),
                "case {case:?}: a failed job must report the failure class it was attributed to, \
                 got {body}"
            );
            assert_eq!(
                body["errorMessage"].as_str(),
                Some(bda::state::INPUT_ASSET_ABSENT_MESSAGE),
                "case {case:?}: a failed job must report why it failed, got {body}"
            );
        }

        // A terminal status is one the job can never leave: a later read answers the same
        // way, and so does one taken after the asset has been staged.
        let again = read(&router, case, arn).await;
        assert_eq!(
            again["status"], body["status"],
            "case {case:?}: a read must not change what a later caller sees"
        );
        assert_eq!(
            again["errorMessage"], body["errorMessage"],
            "case {case:?}: the recorded failure must stay put"
        );
    }

    // Staging the missing asset now cannot resurrect a job that already failed: its
    // outcome was settled when the clock made it due.
    stage_asset(&engine, "assets/missing.pdf");
    let (case, arn, _) = arns
        .iter()
        .find(|(case, _, _)| *case == "a key the bucket does not hold")
        .expect("the row whose key the bucket did not hold");
    let body = read(&router, case, arn).await;
    assert_eq!(
        body["status"].as_str(),
        Some("ClientError"),
        "case {case:?}: an asset staged after the job failed must not change its outcome, got {body}"
    );
}

#[tokio::test]
async fn an_asset_staged_before_the_job_finishes_is_there_for_the_job() {
    // The resolution happens when the job RUNS, not when the submission is accepted: a
    // caller that uploads the asset right after submitting has a job that completes.
    let (router, engine) = emulator();
    let case = "an asset staged after the submission";
    let key = "assets/late/arrival.pdf";
    let arn = submit(
        &router,
        case,
        &format!("s3://{ASSET_BUCKET}/{key}"),
        "s3://out-bucket/late/",
    )
    .await;

    stage_asset(&engine, key);
    tokio::time::sleep(past_the_lifecycle()).await;

    let body = read(&router, case, &arn).await;
    assert_eq!(
        body["status"].as_str(),
        Some("Success"),
        "case {case:?}: an asset in the store when the job ran must let it complete, got {body}"
    );
    assert!(
        body["errorMessage"].is_null(),
        "case {case:?}: a job that completed carries no error message, got {body}"
    );
}

#[tokio::test]
async fn with_no_object_store_to_ask_no_job_is_failed() {
    // The other half of the rule: a failure has to be GROUNDED in what the object store
    // answered. With no Amazon S3 mounted, nothing is established about the asset, so the
    // job completes rather than being failed on an assumption.
    let router = emulator_without_the_object_store();
    let case = "no object store mounted";
    let arn = submit(
        &router,
        case,
        &format!("s3://{ASSET_BUCKET}/assets/never-staged.pdf"),
        "s3://out-bucket/ungrounded/",
    )
    .await;

    tokio::time::sleep(past_the_lifecycle()).await;

    let body = read(&router, case, &arn).await;
    assert_eq!(
        body["status"].as_str(),
        Some("Success"),
        "case {case:?}: with nothing to ask about the asset no job may be failed, got {body}"
    );
    assert!(
        body["errorType"].is_null() && body["errorMessage"].is_null(),
        "case {case:?}: a job nothing failed carries no error members, got {body}"
    );
}
