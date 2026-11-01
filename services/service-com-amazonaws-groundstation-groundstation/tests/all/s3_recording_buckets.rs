// SPDX-License-Identifier: CC0-1.0

//! An `s3RecordingConfig`'s bucket, resolved against the emulator's OWN S3.
//!
//! The RULE under test: Ground Station resolves the bucket an `s3RecordingConfig` names
//! before it will store the config, and refuses ONLY when that bucket cannot be resolved — a
//! bucket the account actually holds is accepted (HTTP 200, `configType` `s3-recording`)
//! and reads back, while a `bucketArn` naming no bucket is refused with
//! `InvalidParameterException` (HTTP 431). The same holds of an update.
//!
//! So this suite mounts the S3 service beside Ground Station on ONE engine — what the
//! assembled emulator does — and creates its buckets through S3's own front door, so what
//! the refusal is grounded in is S3's answer rather than anything this test seeded.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_groundstation_groundstation::state::UNRESOLVABLE_RECORDING_BUCKET;
use service_com_amazonaws_groundstation_groundstation::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::scheduling_loop::{send, Reply, REGION};

/// A SigV4 `Authorization` header in the `s3` scope — the routing signal that picks the
/// S3 service for a request to the bucket front door.
const S3_AUTH: &str = "AWS4-HMAC-SHA256 Credential=AKIDTESTS3/20260101/us-east-1/s3/\
                       aws4_request, SignedHeaders=host, Signature=abcdef";

/// A router holding Ground Station AND S3 over one shared engine, with no bucket and no
/// config in it yet.
///
/// `pub(crate)` here and on the three helpers below: the sibling suite
/// `tests/all/recording_bucket_refusal_message.rs` resolves buckets through the same two
/// front doors, so this plumbing is written once rather than stood up twice.
pub(crate) fn harness_with_object_storage() -> MultiServiceRouter {
    harness_with_object_storage_and_engine().0
}

/// As [`harness_with_object_storage`], handing back the shared engine as well, for a suite
/// that has to read this service's store to observe something no modelled read reports
/// (`tests/all/agent_task_response_url.rs`).
pub(crate) fn harness_with_object_storage_and_engine(
) -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// `PUT /<bucket>` against the emulator's S3 front door — `CreateBucket`, path-style
/// because the emulator serves one endpoint per region. Answers the HTTP status.
///
/// No `LocationConstraint`, so the bucket lands in `us-east-1`, the region the Ground
/// Station requests in this suite are signed for.
pub(crate) async fn create_bucket(router: &MultiServiceRouter, bucket: &str) -> u16 {
    let request = http::Request::builder()
        .method("PUT")
        .uri(format!("https://s3.{REGION}.amazonaws.com/{bucket}"))
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("content-type", "application/xml")
        .header("authorization", S3_AUTH)
        .body(bytes::Bytes::new())
        .expect("build the CreateBucket request");
    router.dispatch(request).await.status().as_u16()
}

/// An `s3RecordingConfig` recording to `bucket_arn`.
pub(crate) fn recording_config(bucket_arn: &str) -> Value {
    json!({"s3RecordingConfig": {
        "bucketArn": bucket_arn,
        "roleArn": "arn:aws:iam::000000000000:role/GroundStationS3Role",
        "prefix": "data/{satellite_id}/{year}/{month}/{day}",
    }})
}

/// `POST /config`.
pub(crate) async fn create(router: &MultiServiceRouter, name: &str, config_data: &Value) -> Reply {
    send(
        router,
        "POST",
        "/config",
        Some(json!({"name": name, "configData": config_data})),
    )
    .await
}

#[tokio::test]
async fn a_recording_config_is_created_for_a_bucket_the_account_holds() {
    let router = harness_with_object_storage();

    // Bucket names differing along what the rule generalises over: length (one at the
    // published three-character minimum), and the characters S3 allows in a name (hyphens,
    // digits, dots). Each is created through S3, so Ground Station's acceptance is grounded
    // in S3 holding it.
    for bucket in ["gs1", "aws-groundstation-downlink", "my.recording-bucket-01"] {
        let created = create_bucket(&router, bucket).await;
        assert_eq!(
            created, 200,
            "bucket {bucket}: expected CreateBucket 200, got {created}"
        );
    }

    // Rows: the bucket ARN as written, and the bucket it names. The last row carries a
    // key after the bucket, which still names its bucket, so the ARN's form is varied too.
    let rows = [
        ("short name", "arn:aws:s3:::gs1"),
        ("hyphenated name", "arn:aws:s3:::aws-groundstation-downlink"),
        ("dotted name", "arn:aws:s3:::my.recording-bucket-01"),
        ("a key under an existing bucket", "arn:aws:s3:::gs1/downlink/key"),
    ];
    for (case, bucket_arn) in rows {
        let config_data = recording_config(bucket_arn);
        let reply = create(&router, &format!("recording {case}"), &config_data).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected CreateConfig 200, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.field("configType"),
            &json!("s3-recording"),
            "case {case}: expected configType s3-recording, got {}",
            reply.field("configType")
        );

        let config_id = reply.string("configId");
        let read_back = send(
            &router,
            "GET",
            &format!("/config/s3-recording/{config_id}"),
            None,
        )
        .await;
        assert_eq!(
            read_back.status, 200,
            "case {case}: expected GetConfig 200, got {} {}",
            read_back.status, read_back.body
        );
        assert_eq!(
            read_back.field("configData"),
            &config_data,
            "case {case}: GetConfig must return the configData that was created, got {}",
            read_back.field("configData")
        );
    }
}

#[tokio::test]
async fn a_recording_config_is_refused_only_for_a_bucket_that_does_not_exist() {
    let router = harness_with_object_storage();
    assert_eq!(
        create_bucket(&router, "aws-groundstation-downlink").await,
        200,
        "the one bucket this account holds must be created"
    );

    // Rows: buckets S3 does not hold. They differ along the rule — a name nothing
    // resembles, a NEAR MISS of the held bucket's name (so acceptance cannot be a prefix
    // match), a name that differs only in case, and a key under a bucket that does not
    // exist.
    for (case, bucket_arn) in [
        ("a name nothing holds", "arn:aws:s3:::absent-bucket"),
        (
            "a near miss of the held bucket",
            "arn:aws:s3:::aws-groundstation-downlinks",
        ),
        (
            "the held bucket's name in another case",
            "arn:aws:s3:::AWS-Groundstation-Downlink",
        ),
        (
            "a key under a bucket nothing holds",
            "arn:aws:s3:::absent-bucket/downlink/key",
        ),
    ] {
        let reply = create(&router, "recording refused", &recording_config(bucket_arn)).await;
        assert_eq!(
            reply.status, 431,
            "case {case}: expected 431, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "InvalidParameterException",
            "case {case}: expected InvalidParameterException, got {}",
            reply.body
        );
        assert_eq!(
            reply.string("message"),
            UNRESOLVABLE_RECORDING_BUCKET,
            "case {case}: wrong refusal text, got {}",
            reply.body
        );
    }

    // The listing is the store: only the config whose bucket exists was kept.
    let held = create(
        &router,
        "recording held",
        &recording_config("arn:aws:s3:::aws-groundstation-downlink"),
    )
    .await;
    assert_eq!(
        held.status, 200,
        "the config whose bucket exists is created, got {} {}",
        held.status, held.body
    );

    // An UPDATE is held to the same rule, and a refused one changes nothing.
    let config_id = held.string("configId");
    let path = format!("/config/s3-recording/{config_id}");
    let refused = send(
        &router,
        "PUT",
        &path,
        Some(json!({
            "name": "recording held",
            "configData": recording_config("arn:aws:s3:::absent-bucket"),
        })),
    )
    .await;
    assert_eq!(
        refused.status, 431,
        "case update-to-an-absent-bucket: expected 431, got {} {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.string("message"),
        UNRESOLVABLE_RECORDING_BUCKET,
        "case update-to-an-absent-bucket: wrong refusal text, got {}",
        refused.body
    );
    let unchanged = send(&router, "GET", &path, None).await;
    assert_eq!(
        unchanged.field("configData"),
        &recording_config("arn:aws:s3:::aws-groundstation-downlink"),
        "case update-to-an-absent-bucket: a refused update must change nothing, got {}",
        unchanged.field("configData")
    );

    // And an update to another bucket the account DOES hold goes through.
    assert_eq!(
        create_bucket(&router, "second-downlink-bucket").await,
        200,
        "the second bucket must be created"
    );
    let moved_data = recording_config("arn:aws:s3:::second-downlink-bucket");
    let moved = send(
        &router,
        "PUT",
        &path,
        Some(json!({"name": "recording moved", "configData": moved_data})),
    )
    .await;
    assert_eq!(
        moved.status, 200,
        "case update-to-an-existing-bucket: expected 200, got {} {}",
        moved.status, moved.body
    );
    let read_back = send(&router, "GET", &path, None).await;
    assert_eq!(
        read_back.field("configData"),
        &recording_config("arn:aws:s3:::second-downlink-bucket"),
        "case update-to-an-existing-bucket: the stored config records to the new bucket, \
         got {}",
        read_back.field("configData")
    );

    // And an update addressed to a config that is not there is answered as a missing
    // resource, whatever its body says about a bucket: the config the URI addresses is
    // resolved before the body is judged.
    let dead = send(
        &router,
        "PUT",
        "/config/s3-recording/00000000-0000-0000-0000-000000000000",
        Some(json!({
            "name": "nothing to rename",
            "configData": recording_config("arn:aws:s3:::absent-bucket"),
        })),
    )
    .await;
    assert_eq!(
        dead.status, 434,
        "case update-a-dead-config-to-an-absent-bucket: expected 434, got {} {}",
        dead.status, dead.body
    );
    assert_eq!(
        dead.error_code(),
        "ResourceNotFoundException",
        "case update-a-dead-config-to-an-absent-bucket: wrong error shape, got {}",
        dead.body
    );
}
