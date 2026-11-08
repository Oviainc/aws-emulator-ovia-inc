// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the OUTCOME of a MicroVM image version's asynchronous build,
//! driven over the same router the emulator boots with Amazon S3 mounted beside this
//! service on one engine — exactly as the assembled router mounts them.
//!
//! RULE: the asynchronous build of a MicroVM image version resolves the version's
//! `codeArtifact` against the emulator's own object store and settles on the SUCCESS
//! branch only when that store serves the artifact; when the store ANSWERS that it does
//! not, the build settles the FAILURE branch — the version `FAILED`/`INACTIVE` carrying
//! the reason for that answer, every per-target build record `FAILED` with the same
//! reason and no snapshot, and the image on the failed value of the lifecycle that
//! started the build (`CREATE_FAILED` for a create, `UPDATE_FAILED` for an update) with
//! `latestFailedImageVersion` naming that version and the version it was serving left
//! active. An image with no active version then has nothing to launch, so `RunMicrovm`
//! is refused.
//!
//! Each test states that rule and exercises it over a SET of inputs: the rows differ in
//! WHICH half of the object address the store does not serve (the bucket, the key) and in
//! the form of the key, so the recorded case — a bucket that does not exist — is one row
//! of the class. The wire harness (`call`, `PAST_THE_SPAN`, …) is the one
//! `microvm_lifecycle` owns; the router here is its own, because this suite needs the
//! owner of objects on the same engine.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::code_artifact::{
    BUCKET_ABSENT_REASON, OBJECT_ABSENT_REASON,
};
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::{
    no_active_version_message, state_time_bucket, BUILD_STATE_FAILED, BUILD_STATE_SUCCESSFUL,
    FIRST_IMAGE_VERSION, IMAGE_STATE_CREATED, IMAGE_STATE_CREATE_FAILED, IMAGE_STATE_UPDATED,
    IMAGE_STATE_UPDATE_FAILED, VERSION_STATE_FAILED, VERSION_STATE_SUCCESSFUL,
    VERSION_STATUS_ACTIVE, VERSION_STATUS_INACTIVE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::microvm_lifecycle::{as_uri_value, call, image_body, PAST_THE_SPAN, REGION};

/// A router carrying this service and the one its builds resolve their code artifact
/// against — Amazon S3 — over ONE shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Verifying signatures is the emulator's concern, not this service's; these tests
    // sign nothing, so the check is off and the scope header is read for routing only.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambdamicrovms_lambdamicrovms::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router
}

/// An Amazon S3 client over this emulator's own in-process callback — the seam a build
/// resolves its artifact through, so an object staged here is the object the build sees.
fn s3(router: &MultiServiceRouter) -> aws_sdk_s3::Client {
    let callback = smithy_server_runtime::EmbeddedEmulator::from_router(router.clone());
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
        .region(aws_sdk_s3::config::Region::new(REGION))
        .endpoint_url(format!("https://s3.{REGION}.amazonaws.com"))
        .force_path_style(true)
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    aws_sdk_s3::Client::from_conf(config)
}

/// Stage one object through Amazon S3's OWN operations, creating its bucket first: what
/// a build can fetch is what that service really holds.
async fn stage_object(router: &MultiServiceRouter, bucket: &str, key: &str) {
    let client = s3(router);
    // A regional endpoint requires the create to state its own location constraint.
    let created = client
        .create_bucket()
        .bucket(bucket)
        .create_bucket_configuration(
            aws_sdk_s3::types::CreateBucketConfiguration::builder()
                .location_constraint(aws_sdk_s3::types::BucketLocationConstraint::from(REGION))
                .build(),
        )
        .send()
        .await;
    assert!(
        created.is_ok(),
        "staging {bucket}: the bucket must be created, got {created:?}"
    );
    let put = client
        .put_object()
        .bucket(bucket)
        .key(key)
        .body(aws_sdk_s3::primitives::ByteStream::from_static(
            b"a MicroVM code artifact",
        ))
        .send()
        .await;
    assert!(
        put.is_ok(),
        "staging s3://{bucket}/{key}: the object must be stored, got {put:?}"
    );
}

/// Create one image whose `codeArtifact` is `uri` and return the ARN the create minted.
async fn create_image(router: &MultiServiceRouter, name: &str, uri: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        "/2025-09-09/microvm-images",
        Some(image_body(name, json!({"codeArtifact": {"uri": uri}}))),
    )
    .await;
    assert_eq!(status, 201, "create {name}: got {status} {body}");
    body["imageArn"].as_str().unwrap_or_default().to_string()
}

/// `GET /2025-09-09/microvm-images/{identifier}`.
async fn get_image(router: &MultiServiceRouter, identifier: &str) -> (u16, Value) {
    let uri = format!(
        "/2025-09-09/microvm-images/{}",
        as_uri_value(identifier)
    );
    call(router, "GET", &uri, None).await
}

/// The versions of one image, newest first.
async fn list_versions(router: &MultiServiceRouter, identifier: &str) -> Vec<Value> {
    let uri = format!(
        "/2025-09-09/microvm-images/{}/versions",
        as_uri_value(identifier)
    );
    let (status, body) = call(router, "GET", &uri, None).await;
    assert_eq!(status, 200, "list versions of {identifier}: got {body}");
    body["items"].as_array().cloned().unwrap_or_default()
}

/// `GET /2025-09-09/microvm-images/{identifier}/versions/{image_version}`.
async fn get_version(
    router: &MultiServiceRouter,
    identifier: &str,
    image_version: &str,
) -> (u16, Value) {
    let uri = format!(
        "/2025-09-09/microvm-images/{}/versions/{}",
        as_uri_value(identifier),
        as_uri_value(image_version)
    );
    call(router, "GET", &uri, None).await
}

/// The build records of one version of one image.
async fn list_builds(
    router: &MultiServiceRouter,
    identifier: &str,
    image_version: &str,
) -> Vec<Value> {
    let uri = format!(
        "/2025-09-09/microvm-images/{}/versions/{}/builds",
        as_uri_value(identifier),
        as_uri_value(image_version)
    );
    let (status, body) = call(router, "GET", &uri, None).await;
    assert_eq!(status, 200, "list builds of {identifier}: got {body}");
    body["items"].as_array().cloned().unwrap_or_default()
}

/// `GET .../versions/{image_version}/builds/{build_id}`.
async fn get_build(
    router: &MultiServiceRouter,
    identifier: &str,
    image_version: &str,
    build_id: &str,
) -> (u16, Value) {
    let uri = format!(
        "/2025-09-09/microvm-images/{}/versions/{}/builds/{}",
        as_uri_value(identifier),
        as_uri_value(image_version),
        as_uri_value(build_id)
    );
    call(router, "GET", &uri, None).await
}

/// One listing row of `ListMicrovmImages` for the image named `name`.
async fn listed_image(router: &MultiServiceRouter, name: &str) -> Value {
    let (status, body) = call(router, "GET", "/2025-09-09/microvm-images", None).await;
    assert_eq!(status, 200, "list images: got {body}");
    body["items"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|item| item["name"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("the listing carries the image {name}: got {body}"))
}

/// A member's text, or the empty string when the member is absent.
fn text(value: &Value, member: &str) -> String {
    value[member].as_str().unwrap_or_default().to_string()
}

/// Assert that a settled version's `versionStateTimeBucket` is the bucket of the instant
/// the version's state settled in — the `updatedAt` the same answer reports — rather than
/// merely present: the member is a projection of the record, so it must agree with it.
fn assert_state_time_bucket(version: &Value, case: &str) {
    let updated_at = version["updatedAt"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: a settled version reports updatedAt: {version}"));
    let expected = state_time_bucket(aws_smithy_types::DateTime::from_secs_f64(updated_at));
    let actual = version["versionStateTimeBucket"].as_str().map(str::to_string);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected versionStateTimeBucket {expected:?}, got {actual:?}"
    );
}

/// One row: what a failing assertion calls it, the image name, the `codeArtifact` URI,
/// the object to stage first (if any), and the `stateReason` the build must settle with
/// (`None` for a build that must succeed).
type ArtifactRow = (
    &'static str,
    &'static str,
    String,
    Option<(&'static str, &'static str)>,
    Option<&'static str>,
);

/// The rows every build-outcome assertion below spans: which half of the object address
/// the store serves, and the form of the key.
fn artifact_rows() -> Vec<ArtifactRow> {
    vec![
        (
            "the artifact is served",
            "served-artifact",
            "s3://mvm-build-artifacts/app.zip".to_string(),
            Some(("mvm-build-artifacts", "app.zip")),
            None,
        ),
        (
            "the artifact is served under a nested prefix",
            "served-nested-artifact",
            "s3://mvm-nested-artifacts/team/build/app.zip".to_string(),
            Some(("mvm-nested-artifacts", "team/build/app.zip")),
            None,
        ),
        (
            "the bucket does not exist",
            "absent-bucket",
            "s3://mvm-no-such-bucket/app.zip".to_string(),
            None,
            Some(BUCKET_ABSENT_REASON),
        ),
        (
            "the bucket exists and the object does not",
            "absent-object",
            "s3://mvm-empty-artifacts/app.zip".to_string(),
            Some(("mvm-empty-artifacts", "another.zip")),
            Some(OBJECT_ABSENT_REASON),
        ),
        (
            "the object is absent under a nested prefix",
            "absent-nested-object",
            "s3://mvm-partial-artifacts/team/build/app.zip".to_string(),
            Some(("mvm-partial-artifacts", "team/build/other.zip")),
            Some(OBJECT_ABSENT_REASON),
        ),
    ]
}

/// RULE: a create's asynchronous build settles the SUCCESS branch only when the object
/// store serves its code artifact, and otherwise settles the FAILURE branch — the image
/// `CREATE_FAILED` with `latestFailedImageVersion` naming the version and no
/// `latestActiveImageVersion`, reported identically by the image's own read and by the
/// listing that carries it.
#[tokio::test]
async fn an_unfetchable_code_artifact_settles_the_image_on_the_failed_branch() {
    let router = emulator();

    for (case, name, uri, staged, reason) in artifact_rows() {
        if let Some((bucket, key)) = staged {
            stage_object(&router, bucket, key).await;
        }
        let image_arn = create_image(&router, name, &uri).await;
        tokio::time::sleep(PAST_THE_SPAN).await;

        let (status, image) = get_image(&router, &image_arn).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {image}");

        let expected_state = match reason {
            None => IMAGE_STATE_CREATED,
            Some(_) => IMAGE_STATE_CREATE_FAILED,
        };
        let actual_state = text(&image, "state");
        assert_eq!(
            actual_state, expected_state,
            "case {case:?}: expected state {expected_state}, got {actual_state:?}"
        );

        let expected_active = reason.is_none().then(|| FIRST_IMAGE_VERSION.to_string());
        let actual_active = image["latestActiveImageVersion"]
            .as_str()
            .map(str::to_string);
        assert_eq!(
            actual_active, expected_active,
            "case {case:?}: expected latestActiveImageVersion {expected_active:?}, \
             got {actual_active:?}"
        );

        let expected_failed = reason.is_some().then(|| FIRST_IMAGE_VERSION.to_string());
        let actual_failed = image["latestFailedImageVersion"]
            .as_str()
            .map(str::to_string);
        assert_eq!(
            actual_failed, expected_failed,
            "case {case:?}: expected latestFailedImageVersion {expected_failed:?}, \
             got {actual_failed:?}"
        );

        // The listing projects the same stored record, so its row cannot disagree.
        let row = listed_image(&router, name).await;
        for member in ["state", "latestActiveImageVersion", "latestFailedImageVersion"] {
            assert_eq!(
                row[member], image[member],
                "case {case:?}: the listing row's {member} must match the image read, \
                 got {} and {}",
                row[member], image[member]
            );
        }
    }
}

/// RULE: the version the build was of settles `FAILED`/`INACTIVE` with the reason the
/// object store answered with — and `SUCCESSFUL`/`ACTIVE` with no reason when the
/// artifact was served — reported identically by the single-version read and by the
/// version listing, each carrying the time bucket the version's state settled in.
#[tokio::test]
async fn a_settled_version_reports_its_state_reason_and_state_time_bucket() {
    let router = emulator();

    for (case, name, uri, staged, reason) in artifact_rows() {
        if let Some((bucket, key)) = staged {
            stage_object(&router, bucket, key).await;
        }
        let image_arn = create_image(&router, name, &uri).await;
        tokio::time::sleep(PAST_THE_SPAN).await;

        let (status, version) = get_version(&router, &image_arn, FIRST_IMAGE_VERSION).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {version}"
        );

        let expected_state = match reason {
            None => VERSION_STATE_SUCCESSFUL,
            Some(_) => VERSION_STATE_FAILED,
        };
        let actual_state = text(&version, "state");
        assert_eq!(
            actual_state, expected_state,
            "case {case:?}: expected version state {expected_state}, got {actual_state:?}"
        );

        let expected_status = match reason {
            None => VERSION_STATUS_ACTIVE,
            Some(_) => VERSION_STATUS_INACTIVE,
        };
        let actual_status = text(&version, "status");
        assert_eq!(
            actual_status, expected_status,
            "case {case:?}: expected version status {expected_status}, got {actual_status:?}"
        );

        let expected_reason = reason.map(::std::string::ToString::to_string);
        let actual_reason = version["stateReason"].as_str().map(str::to_string);
        assert_eq!(
            actual_reason, expected_reason,
            "case {case:?}: expected stateReason {expected_reason:?}, got {actual_reason:?}"
        );

        assert_state_time_bucket(&version, case);

        // The listing row is the same projection of the same record.
        let versions = list_versions(&router, &image_arn).await;
        let listed = versions
            .iter()
            .find(|row| row["imageVersion"].as_str() == Some(FIRST_IMAGE_VERSION))
            .unwrap_or_else(|| panic!("case {case:?}: the listing carries the version"));
        for member in ["state", "status", "stateReason", "versionStateTimeBucket"] {
            assert_eq!(
                listed[member], version[member],
                "case {case:?}: the listing row's {member} must match the version read, \
                 got {} and {}",
                listed[member], version[member]
            );
        }
    }
}

/// RULE: a version's build is the fan-out of its per-target builds, so EVERY build record
/// of a failed version reports `FAILED` with the same reason and no snapshot, and every
/// build record of a successful one reports `SUCCESSFUL` with a snapshot and no reason —
/// on the build listing and on the single-build read alike.
#[tokio::test]
async fn every_build_record_of_a_version_carries_the_builds_verdict() {
    let router = emulator();

    for (case, name, uri, staged, reason) in artifact_rows() {
        if let Some((bucket, key)) = staged {
            stage_object(&router, bucket, key).await;
        }
        let image_arn = create_image(&router, name, &uri).await;
        tokio::time::sleep(PAST_THE_SPAN).await;

        let builds = list_builds(&router, &image_arn, FIRST_IMAGE_VERSION).await;
        assert!(
            !builds.is_empty(),
            "case {case:?}: a scheduled build fans out into at least one build record"
        );

        let expected_state = match reason {
            None => BUILD_STATE_SUCCESSFUL,
            Some(_) => BUILD_STATE_FAILED,
        };
        let expected_reason = reason.map(::std::string::ToString::to_string);
        for build in &builds {
            let actual_state = text(build, "buildState");
            assert_eq!(
                actual_state, expected_state,
                "case {case:?}: expected buildState {expected_state} on every build, \
                 got {actual_state:?} on {build}"
            );
            let actual_reason = build["stateReason"].as_str().map(str::to_string);
            assert_eq!(
                actual_reason, expected_reason,
                "case {case:?}: expected stateReason {expected_reason:?} on every build, \
                 got {actual_reason:?}"
            );

            let build_id = text(build, "buildId");
            let (status, read) = get_build(&router, &image_arn, FIRST_IMAGE_VERSION, &build_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 for build {build_id}, got {status} {read}"
            );
            let read_state = text(&read, "buildState");
            assert_eq!(
                read_state, expected_state,
                "case {case:?}: expected buildState {expected_state} from the build read, \
                 got {read_state:?}"
            );
            let read_reason = read["stateReason"].as_str().map(str::to_string);
            assert_eq!(
                read_reason, expected_reason,
                "case {case:?}: expected stateReason {expected_reason:?} from the build read, \
                 got {read_reason:?}"
            );
            // A build that produced nothing has no memory snapshot to describe.
            let has_snapshot = !read["snapshotBuild"].is_null();
            assert_eq!(
                has_snapshot,
                reason.is_none(),
                "case {case:?}: expected snapshotBuild present={}, got {read}",
                reason.is_none()
            );
        }
    }
}

/// RULE: the same resolution decides the UPDATE path's build, so an update whose artifact
/// the store does not serve leaves the image `UPDATE_FAILED` with the new version as its
/// latest failed one and the version it was serving still active — while an update whose
/// artifact IS served promotes the new version.
#[tokio::test]
async fn an_update_whose_artifact_is_not_served_fails_its_own_version_only() {
    let router = emulator();
    stage_object(&router, "mvm-update-artifacts", "app.zip").await;

    // The rows are the two outcomes of the update's own build, each on an image whose
    // FIRST version was built from a served artifact.
    let rows: Vec<(&str, &str, String, Option<&str>)> = vec![
        (
            "the update's artifact is served",
            "update-served",
            "s3://mvm-update-artifacts/app.zip".to_string(),
            None,
        ),
        (
            "the update's bucket does not exist",
            "update-absent-bucket",
            "s3://mvm-no-such-update-bucket/app.zip".to_string(),
            Some(BUCKET_ABSENT_REASON),
        ),
        (
            "the update's object does not exist",
            "update-absent-object",
            "s3://mvm-update-artifacts/missing.zip".to_string(),
            Some(OBJECT_ABSENT_REASON),
        ),
    ];

    for (case, name, uri, reason) in rows {
        let image_arn = create_image(&router, name, "s3://mvm-update-artifacts/app.zip").await;
        tokio::time::sleep(PAST_THE_SPAN).await;

        let update_uri = format!("/2025-09-09/microvm-images/{}", as_uri_value(&image_arn));
        let (status, updated) = call(
            &router,
            "PUT",
            &update_uri,
            Some(image_body(name, json!({"codeArtifact": {"uri": uri}}))),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from the update, got {status} {updated}"
        );
        let next_version = text(&updated, "imageVersion");
        tokio::time::sleep(PAST_THE_SPAN).await;

        let (status, image) = get_image(&router, &image_arn).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {image}");

        let expected_state = match reason {
            None => IMAGE_STATE_UPDATED,
            Some(_) => IMAGE_STATE_UPDATE_FAILED,
        };
        let actual_state = text(&image, "state");
        assert_eq!(
            actual_state, expected_state,
            "case {case:?}: expected state {expected_state}, got {actual_state:?}"
        );

        // A build that produced nothing cannot replace the version the image serves.
        let expected_active = match reason {
            None => next_version.clone(),
            Some(_) => FIRST_IMAGE_VERSION.to_string(),
        };
        let actual_active = text(&image, "latestActiveImageVersion");
        assert_eq!(
            actual_active, expected_active,
            "case {case:?}: expected latestActiveImageVersion {expected_active}, \
             got {actual_active:?}"
        );

        let expected_failed = reason.is_some().then(|| next_version.clone());
        let actual_failed = image["latestFailedImageVersion"]
            .as_str()
            .map(str::to_string);
        assert_eq!(
            actual_failed, expected_failed,
            "case {case:?}: expected latestFailedImageVersion {expected_failed:?}, \
             got {actual_failed:?}"
        );

        let (status, version) = get_version(&router, &image_arn, &next_version).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 for version {next_version}, got {status} {version}"
        );
        let expected_reason = reason.map(::std::string::ToString::to_string);
        let actual_reason = version["stateReason"].as_str().map(str::to_string);
        assert_eq!(
            actual_reason, expected_reason,
            "case {case:?}: expected stateReason {expected_reason:?}, got {actual_reason:?}"
        );
        assert_state_time_bucket(&version, case);
    }
}

/// RULE: `RunMicrovm` launches from an image's ACTIVE version, so an image whose build
/// failed has no version to launch and the call is refused with the modelled 404 naming
/// the image — while an image whose build was served launches.
#[tokio::test]
async fn running_an_image_with_no_active_version_is_refused() {
    let router = emulator();
    stage_object(&router, "mvm-run-artifacts", "app.zip").await;

    let rows: Vec<(&str, &str, String, bool)> = vec![
        (
            "the artifact was served",
            "run-served",
            "s3://mvm-run-artifacts/app.zip".to_string(),
            true,
        ),
        (
            "the bucket does not exist",
            "run-absent-bucket",
            "s3://mvm-no-such-run-bucket/app.zip".to_string(),
            false,
        ),
        (
            "the object does not exist",
            "run-absent-object",
            "s3://mvm-run-artifacts/missing.zip".to_string(),
            false,
        ),
    ];

    for (case, name, uri, launches) in rows {
        let image_arn = create_image(&router, name, &uri).await;
        tokio::time::sleep(PAST_THE_SPAN).await;

        let (status, body) = call(
            &router,
            "POST",
            "/2025-09-09/microvms",
            Some(json!({"imageIdentifier": image_arn})),
        )
        .await;

        match launches {
            true => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 from the launch, got {status} {body}"
                );
                let launched = text(&body, "imageVersion");
                assert_eq!(
                    launched, FIRST_IMAGE_VERSION,
                    "case {case:?}: expected the active version {FIRST_IMAGE_VERSION}, \
                     got {launched:?}"
                );
            }
            false => {
                assert_eq!(
                    status, 404,
                    "case {case:?}: expected 404, got {status} {body}"
                );
                let expected = no_active_version_message(&image_arn);
                let actual = text(&body, "message");
                assert_eq!(
                    actual, expected,
                    "case {case:?}: expected message {expected:?}, got {actual:?}"
                );
            }
        }
    }
}
