// SPDX-License-Identifier: CC0-1.0

//! What a HarvestJob LEAVES BEHIND, and which terminal status that decides.
//!
//! THE RULE UNDER TEST. A harvest exports a window of an origin endpoint's packaged
//! content to the caller's bucket: `S3Destination$BucketName` is "The name of an S3 bucket
//! within which harvested content will be exported" and `S3Destination$ManifestKey` "The
//! key in the specified S3 bucket where the harvested top-level manifest will be placed".
//! So once the clock has made a submitted job's harvest due, the top-level manifest of the
//! endpoint's own packaging format IS an object at that key of that bucket, and the job's
//! terminal status is the OUTCOME of that export: `SUCCEEDED` where the object store took
//! the manifest, `FAILED` where it could not be placed.
//!
//! Every case drives real HTTP through the `MultiServiceRouter` over ONE engine carrying
//! this service's store and Amazon S3's — the way the assembled emulator runs them — so
//! each destination bucket is created with a real `CreateBucket` and each harvested
//! manifest is read back with a real `GetObject`.
//!
//! The rows differ along what the rule generalises over: the endpoint's PACKAGING FORMAT
//! (and so which manifest format is written), the length and form of the harvested window,
//! the shape of the destination key (bare, nested, dotted) and which bucket it is in. The
//! failure side varies how the destination stops being writable AFTER the submission was
//! accepted: a bucket the account no longer holds (deleted between the submit and the
//! instant the harvest fell due), and an assembly with no object store at all. A destination
//! that could not be written to WHEN THE JOB WAS SUBMITTED is refused outright rather than
//! submitted — `tests/all/harvest_destination.rs` is the suite for that pre-flight.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_mediapackage_mediapackage::state;
use service_com_amazonaws_mediapackage_mediapackage::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::{call, create_channel, CROSS_ACCOUNT_HARVEST_ROLE, REGION};
use crate::harvest_jobs::{hold_transitions_open, make_transitions_due};

/// A SigV4 credential scope naming Amazon S3, which is what claims a request for the
/// object store. No authenticator is registered, so the signature is never checked; the
/// scope is what routes it.
fn s3_authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}/s3/aws4_request, \
         SignedHeaders=host, Signature=unverified"
    )
}

/// One emulator: this service and Amazon S3 on a single shared engine behind the
/// production router — what `core/router` assembles, narrowed to the two services the rule
/// under test spans.
fn emulator_with_object_storage() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// One path-style request to the emulator's Amazon S3 front door, answering the status,
/// the `Content-Type` it was served with and the body bytes.
///
/// Shared with `tests/all/harvest_destination.rs`, which provisions and reads the same
/// destinations through the same front door.
pub(crate) async fn s3(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: &[u8],
) -> (u16, String, Vec<u8>) {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://s3.{REGION}.amazonaws.com{path}"))
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("authorization", s3_authorization())
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("the request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let media_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    (status, media_type, response.body().to_vec())
}

/// `PUT /<bucket>` — `CreateBucket`, naming this Region so the endpoint the request was
/// sent to can serve it.
pub(crate) async fn create_bucket(router: &MultiServiceRouter, bucket: &str) {
    let body = format!(
        "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
         <LocationConstraint>{REGION}</LocationConstraint>\
         </CreateBucketConfiguration>"
    );
    let (status, _, served) = s3(router, "PUT", &format!("/{bucket}"), body.as_bytes()).await;
    assert_eq!(
        status,
        200,
        "the destination bucket {bucket:?} must be created first, got {status} {:?}",
        String::from_utf8_lossy(&served)
    );
}

/// `DELETE /<bucket>` — `DeleteBucket`, which a bucket holding no object accepts (204).
async fn delete_bucket(router: &MultiServiceRouter, bucket: &str) {
    let (status, _, served) = s3(router, "DELETE", &format!("/{bucket}"), &[]).await;
    assert_eq!(
        status,
        204,
        "the destination bucket {bucket:?} must be removable, got {status} {:?}",
        String::from_utf8_lossy(&served)
    );
}

/// `GET /<bucket>/<key>` — `GetObject`, answering the status, the media type and the bytes.
pub(crate) async fn get_object(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
) -> (u16, String, Vec<u8>) {
    s3(router, "GET", &format!("/{bucket}/{key}"), &[]).await
}

/// One harvest under test: the endpoint it exports, the destination it writes to, and what
/// the manifest at that destination must be.
struct Row {
    /// What a failing assertion calls this row.
    case: &'static str,
    id: &'static str,
    channel: &'static str,
    endpoint: &'static str,
    /// The single packaging configuration the endpoint is created with, and its member
    /// name: the manifest format a harvest writes is the one this endpoint serves.
    package_member: &'static str,
    package: Value,
    bucket: &'static str,
    manifest_key: &'static str,
    start: &'static str,
    end: &'static str,
    /// The media type the harvested manifest is stored as.
    media_type: &'static str,
    /// Text the harvested document must carry — the format's own opening, and for the
    /// formats that state a duration, the harvested window.
    manifest_marker: &'static str,
}

/// The harvests this suite drives, one row per point of the class the rule generalises
/// over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "HLS, a nested key, one hour",
            id: "export-hls",
            channel: "export-hls-channel",
            endpoint: "export-hls",
            package_member: "hlsPackage",
            package: json!({}),
            bucket: "harvest-out",
            manifest_key: "out/index.m3u8",
            start: "2026-01-01T00:00:00Z",
            end: "2026-01-01T01:00:00Z",
            media_type: "application/vnd.apple.mpegurl",
            manifest_marker: "#EXTM3U",
        },
        Row {
            case: "CMAF, a bare key, a minute and a half",
            id: "export_cmaf",
            channel: "export-cmaf-channel",
            endpoint: "export-cmaf",
            package_member: "cmafPackage",
            package: json!({ "hlsManifests": [{ "id": "a" }] }),
            bucket: "harvest.out-second",
            manifest_key: "index.m3u8",
            start: "2026-02-03T10:15:00+00:00",
            end: "2026-02-03T10:16:30+00:00",
            media_type: "application/vnd.apple.mpegurl",
            manifest_marker: "#EXTM3U",
        },
        Row {
            case: "DASH, a deep key, half an hour",
            id: "ExportDash3",
            channel: "export-dash-channel",
            endpoint: "export-dash",
            package_member: "dashPackage",
            package: json!({ "segmentDurationSeconds": 4 }),
            bucket: "harvest-out",
            manifest_key: "harvests/2026/02/03/index.mpd",
            start: "2026-02-03T10:15:00Z",
            end: "2026-02-03T10:45:00Z",
            media_type: "application/dash+xml",
            manifest_marker: "mediaPresentationDuration=\"PT1800S\"",
        },
        Row {
            case: "Microsoft Smooth, a dotted key, ten and a quarter seconds",
            id: "export-mss-4",
            channel: "export-mss-channel",
            endpoint: "export-mss",
            package_member: "mssPackage",
            package: json!({}),
            bucket: "harvest-out-third",
            manifest_key: "replay/live.ism/Manifest",
            start: "2026-03-04T23:59:59.250Z",
            end: "2026-03-05T00:00:09.500Z",
            media_type: "application/vnd.ms-sstr+xml",
            manifest_marker: "Duration=\"102500000\"",
        },
    ]
}

/// Create the channel and the startover-enabled origin endpoint `row` harvests.
async fn create_endpoint(router: &MultiServiceRouter, row: &Row) {
    create_channel(router, row.channel).await;
    let mut request = json!({
        "channelId": row.channel,
        "id": row.endpoint,
        // A harvest exports content the endpoint retains for startover playback, so every
        // endpoint here retains a window; `tests/all/harvest_startover.rs` is the suite for
        // the submission that names one which does not.
        "startoverWindowSeconds": 3_600
    });
    request[row.package_member] = row.package.clone();
    let (status, body) = call(router, "POST", "/origin_endpoints", Some(&request)).await;
    assert_eq!(
        status, 200,
        "case {:?}: creating the endpoint expected 200, got {status} {body:?}",
        row.case
    );
}

/// Submit `row`'s harvest job, holding the owed harvest open so the transient window is
/// observable, and answer the create response.
async fn submit(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    row: &Row,
    bucket: &str,
) -> Value {
    let (status, created) = call(
        router,
        "POST",
        "/harvest_jobs",
        Some(&json!({
            "id": row.id,
            "originEndpointId": row.endpoint,
            "startTime": row.start,
            "endTime": row.end,
            "s3Destination": {
                "bucketName": bucket,
                "manifestKey": row.manifest_key,
                "roleArn": CROSS_ACCOUNT_HARVEST_ROLE
            }
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: CreateHarvestJob expected 200, got {status} {created:?}",
        row.case
    );
    assert_eq!(
        created.get("status").and_then(Value::as_str),
        Some(state::HARVEST_IN_PROGRESS),
        "case {:?}: a submitted job must be {}, got {created:?}",
        row.case,
        state::HARVEST_IN_PROGRESS
    );
    hold_transitions_open(engine);
    created
}

/// The `status` a `DescribeHarvestJob` reports for `id`.
async fn described_status(router: &MultiServiceRouter, case: &str, id: &str) -> String {
    let (status, body) = call(router, "GET", &format!("/harvest_jobs/{id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeHarvestJob expected 200, got {status} {body:?}"
    );
    body.get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn a_harvest_writes_its_top_level_manifest_and_then_reports_success() {
    let (router, engine) = emulator_with_object_storage();
    for bucket in ["harvest-out", "harvest.out-second", "harvest-out-third"] {
        create_bucket(&router, bucket).await;
    }

    for row in rows() {
        let case = row.case;
        create_endpoint(&router, &row).await;
        submit(&router, &engine, &row, row.bucket).await;

        // Before the clock has made the harvest due the destination holds nothing: the
        // export is the work the clock owes, not something the submit did.
        let (absent, _, _) = get_object(&router, row.bucket, row.manifest_key).await;
        assert_eq!(
            absent, 404,
            "case {case:?}: the manifest must not be in the destination while the job is \
             still {}, got {absent}",
            state::HARVEST_IN_PROGRESS
        );
        assert_eq!(
            described_status(&router, case, row.id).await,
            state::HARVEST_IN_PROGRESS,
            "case {case:?}: the job must still be {} before its harvest is due",
            state::HARVEST_IN_PROGRESS
        );

        // Once it IS due, the harvest runs: the manifest is at the key the submission
        // named, and the job reports the success that write earned.
        make_transitions_due(&engine);
        assert_eq!(
            described_status(&router, case, row.id).await,
            state::HARVEST_SUCCEEDED,
            "case {case:?}: a harvest whose manifest the destination took must report {}",
            state::HARVEST_SUCCEEDED
        );
        let (status, media_type, body) = get_object(&router, row.bucket, row.manifest_key).await;
        let document = String::from_utf8_lossy(&body).to_string();
        assert_eq!(
            status, 200,
            "case {case:?}: the harvested manifest must be readable at \
             {}/{}, got {status} {document:?}",
            row.bucket, row.manifest_key
        );
        assert_eq!(
            media_type, row.media_type,
            "case {case:?}: expected the manifest served as {:?}, got {media_type:?}",
            row.media_type
        );
        assert!(
            document.contains(row.manifest_marker),
            "case {case:?}: the harvested manifest must carry {:?}, got {document:?}",
            row.manifest_marker
        );

        // And it stays there: a terminal job owes nothing further, so a later read neither
        // moves the status nor writes again.
        assert_eq!(
            described_status(&router, case, row.id).await,
            state::HARVEST_SUCCEEDED,
            "case {case:?}: a job in a terminal status must not move again"
        );
    }
}

#[tokio::test]
async fn a_harvest_whose_manifest_cannot_be_placed_reports_failure() {
    let (router, engine) = emulator_with_object_storage();
    // The destination of each row is created — a submission naming a bucket the account does
    // not hold is refused outright — and then DELETED while the job is still harvesting, so
    // by the time the clock makes the export due the manifest has nowhere to go. The rows
    // differ along the shape of the name, so the outcome is not a property of one spelling.
    let doomed_buckets = ["harvest-doomed", "harvest.doomed-2", "doomed3"];

    for (row, bucket) in rows().into_iter().zip(doomed_buckets) {
        let case = row.case;
        create_endpoint(&router, &row).await;
        create_bucket(&router, bucket).await;
        submit(&router, &engine, &row, bucket).await;
        delete_bucket(&router, bucket).await;

        make_transitions_due(&engine);
        assert_eq!(
            described_status(&router, case, row.id).await,
            state::HARVEST_FAILED,
            "case {case:?}: a harvest whose manifest the destination {bucket:?} could not \
             take must report {}",
            state::HARVEST_FAILED
        );
        let (absent, _, _) = get_object(&router, bucket, row.manifest_key).await;
        assert_eq!(
            absent, 404,
            "case {case:?}: a failed harvest must have left nothing in {bucket:?}, got \
             {absent}"
        );
    }
}

#[tokio::test]
async fn a_harvest_with_no_object_store_to_write_to_reports_failure() {
    // This emulator holds MediaPackage alone: nothing answers the write at all, so the
    // manifest is not at the key the submission named and the harvest produced no output.
    let (router, engine) = crate::channel_lifecycle::emulator_with_engine();
    for row in rows() {
        let case = row.case;
        create_endpoint(&router, &row).await;
        submit(&router, &engine, &row, row.bucket).await;

        make_transitions_due(&engine);
        assert_eq!(
            described_status(&router, case, row.id).await,
            state::HARVEST_FAILED,
            "case {case:?}: with no object store to write to the job must report {}",
            state::HARVEST_FAILED
        );
    }
}
