// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateMicrovmImageVersion` — publishing a version's
//! availability — and for what `RunMicrovm` then does with the version that was
//! published, driven over the same router the emulator boots with Amazon S3 mounted
//! beside this service on one engine (a build's outcome is the object store's answer, so
//! a FAILED version can only be reached through the owner of objects).
//!
//! RULE: a version's STATUS is the availability a caller publishes ("ACTIVE (can be used
//! by RunMicrovm) or INACTIVE (blocked from launching new MicroVMs)") and its STATE is
//! what its build settled on; publishing a status therefore never rewrites the state, and
//! it moves the image's own `latestActiveImageVersion` — exactly one version of an image
//! is ACTIVE, so the one just published takes that pointer and the one that held it steps
//! down, and taking the status off the version the image was serving leaves it with none.
//! A MicroVM is launched from what a build PRODUCED, so a version published ACTIVE whose
//! builds produced no image is refused by `RunMicrovm` with the modelled 404 naming the
//! missing build.
//!
//! Each test states that rule and exercises it over a SET of inputs: the rows differ in
//! the state the version's build settled on, in which status is published, and in whether
//! the launch pins a version. The wire harness (`call`, `PAST_THE_SPAN`, …) is the one
//! `microvm_lifecycle` owns, and the Amazon S3 harness is `image_build_outcome`'s.

use serde_json::{json, Value};
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::code_artifact::BUCKET_ABSENT_REASON;
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::{
    access_denied_message, image_not_found_message, image_version_not_found_message,
    invalid_arn_format_message, managed_base_image_arn, next_image_version,
    no_image_build_message, FIRST_IMAGE_VERSION, IMAGE_STATE_UPDATED, MANAGED_BASE_IMAGE_IDS,
    VERSION_STATE_FAILED, VERSION_STATE_SUCCESSFUL, VERSION_STATUS_ACTIVE, VERSION_STATUS_INACTIVE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::image_build_outcome::{
    create_image, emulator, get_image, get_version, stage_object,
};
use crate::microvm_lifecycle::{as_uri_value, call, image_body, PAST_THE_SPAN, REGION};

/// The bucket the suite stages a fetchable artifact in, so a build of it succeeds.
const SERVED_BUCKET: &str = "mvm-status-artifacts";

/// The object key of that artifact.
const SERVED_KEY: &str = "app.zip";

/// A `codeArtifact$uri` whose bucket the object store does not serve, so a build of it
/// settles on the failure branch.
const UNFETCHABLE_URI: &str = "s3://mvm-status-no-such-bucket/app.zip";

/// `PATCH /2025-09-09/microvm-images/{imageIdentifier}/versions/{imageVersion}`.
async fn publish_status(
    router: &MultiServiceRouter,
    identifier: &str,
    image_version: &str,
    status: &str,
) -> (u16, Value) {
    let uri = format!(
        "/2025-09-09/microvm-images/{}/versions/{}",
        as_uri_value(identifier),
        as_uri_value(image_version)
    );
    call(router, "PATCH", &uri, Some(json!({"status": status}))).await
}

/// `POST /2025-09-09/microvms`.
async fn run_microvm(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(router, "POST", "/2025-09-09/microvms", Some(body)).await
}

/// The MicroVMs this account runs from the image `identifier`.
async fn microvms_of(router: &MultiServiceRouter, identifier: &str) -> Vec<Value> {
    let uri = format!(
        "/2025-09-09/microvms?imageIdentifier={}",
        as_uri_value(identifier)
    );
    let (status, body) = call(router, "GET", &uri, None).await;
    assert_eq!(status, 200, "list MicroVMs of {identifier}: got {body}");
    body["items"].as_array().cloned().unwrap_or_default()
}

/// One image whose first build SUCCEEDED, with the ARN the create minted.
async fn image_with_successful_build(router: &MultiServiceRouter, name: &str) -> String {
    stage_object(router, SERVED_BUCKET, SERVED_KEY).await;
    let arn = create_image(
        router,
        name,
        &format!("s3://{SERVED_BUCKET}/{SERVED_KEY}"),
    )
    .await;
    tokio::time::sleep(PAST_THE_SPAN).await;
    arn
}

/// One image whose first build FAILED, with the ARN the create minted.
async fn image_with_failed_build(router: &MultiServiceRouter, name: &str) -> String {
    let arn = create_image(router, name, UNFETCHABLE_URI).await;
    tokio::time::sleep(PAST_THE_SPAN).await;
    arn
}

/// RULE: publishing a status writes the version's `status` and nothing else about it —
/// its `state` and `stateReason` are what the build settled on — and the answer is the
/// stored record, so the version's own read reports exactly what the publication
/// returned.
#[tokio::test]
async fn publishing_a_status_leaves_the_state_the_build_settled_on() {
    let router = emulator();

    // Rows differ in the state the build settled on and in the status published, which is
    // the whole space of this call: the state must survive each one.
    let successful = image_with_successful_build(&router, "status-successful").await;
    let failed = image_with_failed_build(&router, "status-failed").await;
    let rows: Vec<(&str, &str, &str, Option<&str>)> = vec![
        (
            "a successful version blocked from launching",
            successful.as_str(),
            VERSION_STATUS_INACTIVE,
            None,
        ),
        (
            "a successful version published again",
            successful.as_str(),
            VERSION_STATUS_ACTIVE,
            None,
        ),
        (
            "a failed version published ACTIVE",
            failed.as_str(),
            VERSION_STATUS_ACTIVE,
            Some(BUCKET_ABSENT_REASON),
        ),
        (
            "a failed version blocked again",
            failed.as_str(),
            VERSION_STATUS_INACTIVE,
            Some(BUCKET_ABSENT_REASON),
        ),
    ];

    for (case, image_arn, status_asked, reason) in rows {
        let expected_state = match reason {
            None => VERSION_STATE_SUCCESSFUL,
            Some(_) => VERSION_STATE_FAILED,
        };
        let (status, published) =
            publish_status(&router, image_arn, FIRST_IMAGE_VERSION, status_asked).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {published}"
        );
        assert_eq!(
            published["status"].as_str(),
            Some(status_asked),
            "case {case:?}: expected status {status_asked}, got {published}"
        );
        assert_eq!(
            published["state"].as_str(),
            Some(expected_state),
            "case {case:?}: the state is the build's, expected {expected_state}, got {published}"
        );
        assert_eq!(
            published["stateReason"].as_str(),
            reason,
            "case {case:?}: expected stateReason {reason:?}, got {:?}",
            published["stateReason"]
        );
        assert_eq!(
            published["imageArn"].as_str(),
            Some(image_arn),
            "case {case:?}: the answer names the image it published for, got {published}"
        );

        // The answer is the PERSISTED record: the version's own read reports the same.
        let (status, read) = get_version(&router, image_arn, FIRST_IMAGE_VERSION).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the version reads back, got {status} {read}"
        );
        for member in ["status", "state", "stateReason", "imageVersion", "imageArn"] {
            assert_eq!(
                read[member], published[member],
                "case {case:?}: the read's {member} must match the publication, got {} and {}",
                read[member], published[member]
            );
        }
    }
}

/// RULE: exactly one version of an image is ACTIVE, and publishing a status is what moves
/// the image's `latestActiveImageVersion`: the version just published takes it, the one
/// that held it steps down to INACTIVE, and publishing INACTIVE on the version the image
/// was serving leaves the image with no active version. The image itself settles
/// `UPDATED`, because publishing a status is an update of it.
#[tokio::test]
async fn publishing_a_status_moves_the_images_active_version() {
    let router = emulator();

    let image_arn = image_with_successful_build(&router, "status-pointer").await;
    // A SECOND version of the same image, so the rows can span which of two versions is
    // published — the dimension the "exactly one ACTIVE" rule generalises over.
    let (status, updated) = call(
        &router,
        "PUT",
        &format!("/2025-09-09/microvm-images/{}", as_uri_value(&image_arn)),
        Some(image_body(
            "ignored-by-the-update",
            json!({"codeArtifact": {"uri": format!("s3://{SERVED_BUCKET}/{SERVED_KEY}")}}),
        )),
    )
    .await;
    assert_eq!(
        status, 200,
        "the second version must be minted, got {status} {updated}"
    );
    tokio::time::sleep(PAST_THE_SPAN).await;
    let second = next_image_version([FIRST_IMAGE_VERSION]);

    // Rows: the version published, the status asked for, and the image's expected
    // `latestActiveImageVersion` afterwards.
    let rows: Vec<(&str, &str, &str, Option<&str>)> = vec![
        (
            "the newest version is the active one after its build",
            second.as_str(),
            VERSION_STATUS_ACTIVE,
            Some(second.as_str()),
        ),
        (
            "publishing the older version steps the newer one down",
            FIRST_IMAGE_VERSION,
            VERSION_STATUS_ACTIVE,
            Some(FIRST_IMAGE_VERSION),
        ),
        (
            "blocking the active version leaves the image with none",
            FIRST_IMAGE_VERSION,
            VERSION_STATUS_INACTIVE,
            None,
        ),
        (
            "publishing the newer version again",
            second.as_str(),
            VERSION_STATUS_ACTIVE,
            Some(second.as_str()),
        ),
    ];

    for (case, image_version, status_asked, expected_active) in rows {
        let (status, published) =
            publish_status(&router, &image_arn, image_version, status_asked).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {published}"
        );

        let (status, image) = get_image(&router, &image_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the image reads back, got {status} {image}"
        );
        let actual_active = image["latestActiveImageVersion"].as_str();
        assert_eq!(
            actual_active, expected_active,
            "case {case:?}: expected latestActiveImageVersion {expected_active:?}, got \
             {actual_active:?}"
        );
        assert_eq!(
            image["state"].as_str(),
            Some(IMAGE_STATE_UPDATED),
            "case {case:?}: publishing a status updates the image, got {image}"
        );

        // Every OTHER version of the image is INACTIVE, whichever one was published.
        for other in [FIRST_IMAGE_VERSION, second.as_str()] {
            if Some(other) == actual_active {
                continue;
            }
            let (status, read) = get_version(&router, &image_arn, other).await;
            assert_eq!(
                status, 200,
                "case {case:?}: version {other} reads back, got {status} {read}"
            );
            assert_eq!(
                read["status"].as_str(),
                Some(VERSION_STATUS_INACTIVE),
                "case {case:?}: only one version is ACTIVE, got {read}"
            );
        }
    }
}

/// RULE: a MicroVM is launched from what a build PRODUCED, so a version published ACTIVE
/// whose builds produced no image is refused with the modelled 404 naming the missing
/// build for the image and version the launch resolved — and nothing is launched.
#[tokio::test]
async fn a_launch_of_a_version_with_no_successful_build_is_refused() {
    let router = emulator();

    let image_arn = image_with_failed_build(&router, "status-unbuilt").await;
    let (status, published) =
        publish_status(&router, &image_arn, FIRST_IMAGE_VERSION, VERSION_STATUS_ACTIVE).await;
    assert_eq!(
        status, 200,
        "the failed version is published ACTIVE, got {status} {published}"
    );

    // Rows differ in how the launch addresses the version — the image's own active
    // version, or the version pinned explicitly — and in which form it names the image,
    // because none of those changes what the build produced.
    let rows: Vec<(&str, Value)> = vec![
        (
            "the image's active version, by ARN",
            json!({"imageIdentifier": image_arn.clone()}),
        ),
        (
            "the image's active version, by name",
            json!({"imageIdentifier": "status-unbuilt"}),
        ),
        (
            "the version pinned explicitly",
            json!({"imageIdentifier": image_arn.clone(), "imageVersion": FIRST_IMAGE_VERSION}),
        ),
    ];

    for (case, body) in rows {
        let (status, refused) = run_microvm(&router, body).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {refused}"
        );
        assert_eq!(
            refused["__type"].as_str(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {refused}"
        );
        assert_eq!(
            refused["message"].as_str(),
            Some(no_image_build_message(&image_arn, FIRST_IMAGE_VERSION).as_str()),
            "case {case:?}: expected the missing-build message, got {:?}",
            refused["message"]
        );
    }

    // Nothing was launched: the refusals left the MicroVM collection of that image empty.
    let running = microvms_of(&router, &image_arn).await;
    assert!(
        running.is_empty(),
        "a refused launch runs no MicroVM, got {running:?}"
    );

    // The control: an image whose build DID produce an image launches through the very
    // same path, so the rows above are refused by the missing build and not by a dead
    // path.
    let built = image_with_successful_build(&router, "status-built").await;
    let (status, run) = run_microvm(&router, json!({"imageIdentifier": built.clone()})).await;
    assert_eq!(status, 200, "a built image launches, got {status} {run}");
}

/// RULE: the publication is addressed at a `MicrovmImageIdentifier`, so it answers the
/// same verdicts every other call addressed at one does — the account's authorization
/// first (403 for an identifier outside its own image namespace), then existence in the
/// request's region (404), with a value that is no identifier of this API at all refused
/// as the bad request it is (400) — and a version the resolved image does not hold is the
/// modelled 404 naming it.
#[tokio::test]
async fn a_status_publication_outside_this_accounts_own_versions_is_refused() {
    let router = emulator();

    let image_arn = image_with_successful_build(&router, "status-refusals").await;
    let managed = managed_base_image_arn(REGION, MANAGED_BASE_IMAGE_IDS[0]);
    let absent = format!("arn:aws:lambda:{REGION}:000000000000:microvm-image:never-created");
    let other_account = format!("arn:aws:lambda:{REGION}:210987654321:microvm-image:someone-elses");

    // Rows span the refusal classes: AWS's own catalogue, another account's namespace,
    // this account's namespace with no record of the identifier, a value that is no
    // identifier, and a version the resolved image does not hold.
    let rows: Vec<(&str, String, &str, u16, String, &str)> = vec![
        (
            "an AWS-owned managed base image",
            managed.clone(),
            FIRST_IMAGE_VERSION,
            403,
            access_denied_message(),
            "AccessDeniedException",
        ),
        (
            "another account's image",
            other_account.clone(),
            FIRST_IMAGE_VERSION,
            403,
            access_denied_message(),
            "AccessDeniedException",
        ),
        (
            "this account's namespace, a name nothing was created under",
            absent.clone(),
            FIRST_IMAGE_VERSION,
            404,
            image_not_found_message(&absent),
            "ResourceNotFoundException",
        ),
        (
            "a value that is no identifier of this API",
            "not-an-identifier".to_string(),
            FIRST_IMAGE_VERSION,
            400,
            invalid_arn_format_message("not-an-identifier"),
            "ValidationException",
        ),
        (
            "a version this account's own image does not hold",
            image_arn.clone(),
            "9.9",
            404,
            image_version_not_found_message(&image_arn, "9.9"),
            "ResourceNotFoundException",
        ),
    ];

    for (case, identifier, image_version, expected_status, expected_message, expected_type) in rows
    {
        let (status, body) = publish_status(
            &router,
            &identifier,
            image_version,
            VERSION_STATUS_INACTIVE,
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {body}"
        );
        assert_eq!(
            body["__type"].as_str(),
            Some(expected_type),
            "case {case:?}: expected {expected_type}, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(expected_message.as_str()),
            "case {case:?}: expected {expected_message:?}, got {:?}",
            body["message"]
        );
    }

    // No refusal wrote anything: the account's own version is still the active one.
    let (status, image) = get_image(&router, &image_arn).await;
    assert_eq!(status, 200, "the image reads back, got {status} {image}");
    assert_eq!(
        image["latestActiveImageVersion"].as_str(),
        Some(FIRST_IMAGE_VERSION),
        "a refused publication changes nothing a later caller sees, got {image}"
    );
}
