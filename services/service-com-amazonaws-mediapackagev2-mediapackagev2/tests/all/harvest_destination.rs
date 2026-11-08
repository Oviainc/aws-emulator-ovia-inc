// SPDX-License-Identifier: CC0-1.0

//! `CreateHarvestJob` pre-flights the DESTINATION of the harvest it is asked to carry out.
//!
//! RULE under test: a harvest job EXPORTS content to an S3 bucket — `CreateHarvestJob`
//! "Creates a new harvest job to export content from a MediaPackage v2 channel to an S3
//! bucket" and `S3DestinationConfig$BucketName` is "The name of an S3 bucket within which
//! harvested content will be exported"
//! (`smithy_query(query="shape",shape="com.amazonaws.mediapackagev2#S3DestinationConfig",model_path="services/service-com-amazonaws-mediapackagev2-mediapackagev2/smithy-model/com-amazonaws-mediapackagev2-mediapackagev2.json")`)
//! — so a submission whose destination the harvest could not be written to is refused
//! outright, with the modelled `ValidationException` (HTTP 400,
//! `x-amzn-errortype: ValidationException`) carrying
//! `ValidationExceptionType: HARVEST_JOB_UNABLE_TO_WRITE_TO_S3_DESTINATION`, and persists
//! NOTHING: the following `GetHarvestJob` is a 404 and `ListHarvestJobs` reports no such
//! job. A destination the harvest could be written to is accepted and read back.
//!
//! Whether the harvest could be written there is read off the object store that would have
//! to take it, so every case drives real HTTP through the `MultiServiceRouter` over ONE
//! engine carrying this service's store and Amazon S3's — the way the assembled emulator
//! runs them — and every bucket a submission is accepted against is created with a real
//! `CreateBucket` through Amazon S3's own front door.
//!
//! The pre-flight is a READ: it leaves the destination exactly as the caller left it, which
//! the listing of the accepted destinations' buckets pins.
//!
//! The rows differ along what the rule generalises over: the bucket NAME's form
//! (hyphenated, dotted, digits) and both published bounds of the modelled `@length` 3..63,
//! the shape of the destination path, and — on the refusal side — well-formed names of
//! several forms that no `CreateBucket` ever created, including one of each length bound,
//! plus a bucket the object store holds in ANOTHER Region, which the harvest also could not
//! be written to.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_mediapackagev2_mediapackagev2::state;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and the one its buckets are created in.
const REGION: &str = "us-west-2";

/// A second Region, for the destination a submission may not harvest to because the object
/// store holds it somewhere else.
const OTHER_REGION: &str = "eu-west-1";

/// The channel group, channel and origin endpoint every submission harvests from.
const GROUP: &str = "destination-group";
/// See [`GROUP`].
const CHANNEL: &str = "destination-channel";
/// See [`GROUP`].
const ENDPOINT: &str = "destination-endpoint";
/// The manifest the endpoint publishes, which every submission harvests.
const MANIFEST: &str = "index";

/// The published bounds of `S3DestinationConfig$BucketName` (`@length` min 3, max 63), so
/// the rows state them once.
const MIN_BUCKET_NAME_CHARS: usize = 3;
/// See [`MIN_BUCKET_NAME_CHARS`].
const MAX_BUCKET_NAME_CHARS: usize = 63;

/// One emulator: this service and Amazon S3 on a single shared engine behind the production
/// router — what `core/router` assembles, narrowed to the two services the rule spans.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // This suite signs nothing: it exercises handler behaviour, not authentication.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_mediapackagev2_mediapackagev2::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router
}

/// One request to a service's front door, signed for `region`, answering the status, the
/// `x-amzn-errortype` header and the body bytes.
async fn send(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    signing_name: &str,
    region: &str,
    path: &str,
    body: &[u8],
) -> (u16, String, Vec<u8>) {
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", host)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .header("content-type", "application/json")
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    (status, error_type, response.body().to_vec())
}

/// One MediaPackageV2 control-plane request, with the body parsed as JSON.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, String, Value) {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let (status, error_type, bytes) = send(
        router,
        method,
        &format!("mediapackagev2.{REGION}.amazonaws.com"),
        "mediapackagev2",
        REGION,
        path,
        payload.as_bytes(),
    )
    .await;
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, error_type, parsed)
}

/// `PUT /<bucket>` — `CreateBucket` through Amazon S3's own front door, in `region`.
async fn create_bucket_in(router: &MultiServiceRouter, bucket: &str, region: &str) {
    let body = format!(
        "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
         <LocationConstraint>{region}</LocationConstraint>\
         </CreateBucketConfiguration>"
    );
    let (status, _, served) = send(
        router,
        "PUT",
        &format!("s3.{region}.amazonaws.com"),
        "s3",
        region,
        &format!("/{bucket}"),
        body.as_bytes(),
    )
    .await;
    assert_eq!(
        status,
        200,
        "the destination bucket {bucket:?} must be created first, got {status} {:?}",
        String::from_utf8_lossy(&served)
    );
}

/// `CreateBucket` in the Region every submission below is served in.
async fn create_bucket(router: &MultiServiceRouter, bucket: &str) {
    create_bucket_in(router, bucket, REGION).await;
}

/// `GET /<bucket>?list-type=2` — `ListObjectsV2`, answering the body as text.
async fn list_objects(router: &MultiServiceRouter, bucket: &str) -> String {
    let (status, _, served) = send(
        router,
        "GET",
        &format!("s3.{REGION}.amazonaws.com"),
        "s3",
        REGION,
        &format!("/{bucket}?list-type=2"),
        &[],
    )
    .await;
    assert_eq!(
        status, 200,
        "listing the destination bucket {bucket:?} expected 200, got {status}"
    );
    String::from_utf8_lossy(&served).to_string()
}

/// Create the group, channel and origin endpoint every submission harvests from.
async fn provision(router: &MultiServiceRouter) {
    let (status, _, body) = call(
        router,
        "POST",
        "/channelGroup",
        Some(json!({"ChannelGroupName": GROUP})),
    )
    .await;
    assert_eq!(status, 200, "setup: CreateChannelGroup got {status} {body}");
    let (status, _, body) = call(
        router,
        "POST",
        &format!("/channelGroup/{GROUP}/channel"),
        Some(json!({"ChannelName": CHANNEL})),
    )
    .await;
    assert_eq!(status, 200, "setup: CreateChannel got {status} {body}");
    let (status, _, body) = call(
        router,
        "POST",
        &format!("/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint"),
        Some(json!({
            "OriginEndpointName": ENDPOINT,
            "ContainerType": "TS",
            "HlsManifests": [{"ManifestName": MANIFEST}],
        })),
    )
    .await;
    assert_eq!(status, 200, "setup: CreateOriginEndpoint got {status} {body}");
}

/// Epoch seconds, far enough ahead that a scheduled window does not open mid-test.
fn epoch_seconds_from_now(offset: f64) -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock after the epoch")
        .as_secs_f64()
        + offset
}

/// Submit one harvest job named `name`, exporting to `bucket` under `path`.
async fn submit(
    router: &MultiServiceRouter,
    endpoint: &str,
    name: &str,
    bucket: &str,
    path: &str,
) -> (u16, String, Value) {
    let start = epoch_seconds_from_now(600.0);
    call(
        router,
        "POST",
        &format!("/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint/{endpoint}/harvestJob"),
        Some(json!({
            "HarvestJobName": name,
            "HarvestedManifests": {"HlsManifests": [{"ManifestName": MANIFEST}]},
            "ScheduleConfiguration": {"StartTime": start, "EndTime": start + 120.0},
            "Destination": {"S3Destination": {"BucketName": bucket, "DestinationPath": path}},
        })),
    )
    .await
}

/// Read one harvest job back.
async fn get_job(router: &MultiServiceRouter, name: &str) -> (u16, String, Value) {
    call(
        router,
        "GET",
        &format!(
            "/channelGroup/{GROUP}/channel/{CHANNEL}/originEndpoint/{ENDPOINT}/harvestJob/{name}"
        ),
        None,
    )
    .await
}

/// Every harvest job name `ListHarvestJobs` currently reports for the group.
async fn listed_jobs(router: &MultiServiceRouter) -> Vec<String> {
    let (status, _, body) = call(router, "GET", &format!("/channelGroup/{GROUP}/harvestJob"), None).await;
    assert_eq!(status, 200, "ListHarvestJobs expected 200, got {status} {body}");
    body.get("Items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.get("HarvestJobName")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One destination the harvest could be written to: the bucket to create and name, and the
/// path within it.
struct Accepted {
    case: &'static str,
    name: &'static str,
    bucket: String,
    path: &'static str,
}

/// Destinations a submission is accepted against, spanning the bucket-name forms Amazon S3
/// holds a bucket under — including both published bounds of the member's `@length` — and
/// the shape of the destination path.
fn accepted() -> Vec<Accepted> {
    vec![
        Accepted {
            case: "a hyphenated bucket, a nested path",
            name: "accepted-hyphenated",
            bucket: "destination-harvests".to_string(),
            path: "harvests/2026/01/01/",
        },
        Accepted {
            case: "a dotted bucket, a bare path",
            name: "accepted_dotted",
            bucket: "destination.harvests.two".to_string(),
            path: "out",
        },
        Accepted {
            case: "a bucket of digits, a dotted path",
            name: "Accepted3",
            bucket: "20260101123456".to_string(),
            path: "replay/live.ism/",
        },
        Accepted {
            case: "a bucket name at the published floor",
            name: "accepted-floor",
            bucket: "d".repeat(MIN_BUCKET_NAME_CHARS),
            path: "floor/",
        },
        Accepted {
            case: "a bucket name at the published ceiling",
            name: "accepted-ceiling",
            bucket: format!("d{}", "x".repeat(MAX_BUCKET_NAME_CHARS - 1)),
            path: "ceiling/",
        },
    ]
}

#[tokio::test]
async fn a_destination_the_harvest_could_be_written_to_is_accepted_and_read_back() {
    let router = emulator();
    provision(&router).await;

    for row in accepted() {
        let case = row.case;
        create_bucket(&router, &row.bucket).await;
        let (status, _, created) = submit(&router, ENDPOINT, row.name, &row.bucket, row.path).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateHarvestJob expected 200, got {status} {created}"
        );
        assert_eq!(
            created.pointer("/Destination/S3Destination/BucketName"),
            Some(&json!(row.bucket)),
            "case {case:?}: the destination must be reported as sent, got {created}"
        );

        // The job is readable back: the response is the stored record.
        let (status, _, read) = get_job(&router, row.name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetHarvestJob expected 200, got {status} {read}"
        );
        assert_eq!(
            read.pointer("/Destination/S3Destination/DestinationPath"),
            Some(&json!(row.path)),
            "case {case:?}: the read must render the stored destination, got {read}"
        );

        // The pre-flight READ the destination: nothing was written to it.
        let listed = list_objects(&router, &row.bucket).await;
        assert!(
            !listed.contains("<Contents>"),
            "case {case:?}: the pre-flight must leave the destination empty, got {listed}"
        );
    }

    let mut expected: Vec<String> = accepted().iter().map(|row| row.name.to_string()).collect();
    expected.sort();
    let mut listed = listed_jobs(&router).await;
    listed.sort();
    assert_eq!(
        listed, expected,
        "the listing must report exactly the accepted jobs, got {listed:?}"
    );
}

#[tokio::test]
async fn a_destination_the_harvest_could_not_be_written_to_is_refused_and_persists_nothing() {
    let router = emulator();
    provision(&router).await;
    // One bucket IS created, so the rows below are refused for the bucket they name rather
    // than for an object store that holds nothing at all.
    create_bucket(&router, "destination-held").await;

    // Well-formed names Amazon S3 would hold a bucket under, that no `CreateBucket` ever
    // created. They differ in form and in length, so the refusal is not a property of one
    // spelling.
    let absent: Vec<String> = vec![
        "destination-never-created".to_string(),
        "destination.never.created".to_string(),
        "neverthere3".to_string(),
        "n".repeat(MIN_BUCKET_NAME_CHARS),
        "n".repeat(MAX_BUCKET_NAME_CHARS),
        // And one the object store DOES hold, in another Region: the harvest and its
        // destination have to be in the same Region, so this is the same refusal.
        {
            let elsewhere = "destination-another-region";
            create_bucket_in(&router, elsewhere, OTHER_REGION).await;
            elsewhere.to_string()
        },
    ];
    let expected_message = state::validation_message(
        Some(state::harvest_destination::HARVEST_DESTINATION_MEMBER),
        &state::harvest_destination::writable_destination_requirement(),
    );

    for (index, bucket) in absent.iter().enumerate() {
        let case = format!("a submission naming the bucket {bucket:?}, which the object store does not hold in this Region");
        let name = format!("absent-bucket-{index}");
        let (status, error_type, refusal) = submit(&router, ENDPOINT, &name, bucket, "out/").await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {refusal}"
        );
        assert_eq!(
            error_type, "ValidationException",
            "case {case:?}: expected the ValidationException error type, got {error_type:?}"
        );
        assert_eq!(
            refusal.get("ValidationExceptionType"),
            Some(&json!(
                state::harvest_destination::VALIDATION_HARVEST_JOB_UNABLE_TO_WRITE_TO_S3_DESTINATION
            )),
            "case {case:?}: expected the modelled discriminator, got {refusal}"
        );
        assert_eq!(
            refusal.get("Message"),
            Some(&json!(expected_message)),
            "case {case:?}: expected the service's sentence, got {refusal}"
        );

        // Nothing was persisted: the job is neither readable nor listed.
        let (status, _, read) = get_job(&router, &name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused submission must persist no job, got {status} {read}"
        );
        let listed = listed_jobs(&router).await;
        assert!(
            !listed.contains(&name),
            "case {case:?}: a refused submission must not be listed, got {listed:?}"
        );
    }

    // The control: the same submission against the bucket the object store DOES hold is
    // accepted, so what the rows above were refused for is the destination and nothing
    // else.
    let (status, _, created) = submit(&router, ENDPOINT, "held-bucket", "destination-held", "out/").await;
    assert_eq!(
        status, 200,
        "case \"the bucket the object store holds\": expected 200, got {status} {created}"
    );
}

#[tokio::test]
async fn a_resource_the_request_addresses_is_reported_before_the_destination() {
    let router = emulator();
    provision(&router).await;

    // The endpoint the submission names resolves to nothing AND the destination could not
    // be written to: what such a caller has to fix is the resource it addressed, so that is
    // what is reported.
    let (status, error_type, refusal) = submit(
        &router,
        "destination-endpoint-never-created",
        "order-endpoint",
        "destination-never-created",
        "out/",
    )
    .await;
    assert_eq!(
        status, 404,
        "a submission against an endpoint that does not exist must answer 404, got {status} \
         {refusal}"
    );
    assert_eq!(
        error_type, "ResourceNotFoundException",
        "expected the ResourceNotFoundException error type, got {error_type:?}"
    );
}
