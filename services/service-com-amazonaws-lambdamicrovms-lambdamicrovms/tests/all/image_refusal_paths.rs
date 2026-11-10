// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what the MicroVM-image plane answers when a call CANNOT be
//! served, driven over the same router the emulator boots.
//!
//! Three rules, each one level more general than the case that exposed it:
//!
//! * a not-found refusal of a build-addressed read carries AWS's own sentence for that
//!   error class, built from the ADDRESS the request carried — its `imageIdentifier`,
//!   `imageVersion` and `buildId` path labels;
//! * a paginated child enumeration of an image (`ListMicrovmImageVersions`,
//!   `ListMicrovmImageBuilds`) REPORTS what this region's store holds at the address the
//!   request named and refuses nothing past the authorization verdict: an address this
//!   account's own image namespace holds no children at gets a TERMINAL EMPTY PAGE, while
//!   an identifier of ANOTHER namespace is still denied (403) and a value that is no
//!   identifier of this API at all is still a bad request (400);
//! * `UpdateMicrovmImage` starts from the image's CURRENT state, so only a record whose
//!   own lifecycle work has settled on a version it built admits an update — every other
//!   `MicrovmImageState` value is refused with the modelled `ValidationException`
//!   (HTTP 400) and leaves the record exactly as it was.
//!
//! Each test states its rule and exercises it over a SET of inputs; the values any one
//! row happens to carry are examples, not the specification. The wire harness
//! (`emulator`, `call`, `image_body`, …) is the one `microvm_lifecycle` owns, the build
//! and version read paths are `image_build_read_path`'s and `image_version_listing`'s,
//! and the router with an object store beside this service — the one a build resolves its
//! code artifact against — is `image_build_outcome`'s.

use serde_json::{json, Value};
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::ops::types::MICROVM_IMAGE_STATE_VALUES;
use service_com_amazonaws_lambdamicrovms_lambdamicrovms::state::{
    access_denied_message, image_not_updatable_message, invalid_arn_format_message,
    managed_base_image_arn, service_state, FIRST_IMAGE_VERSION, IMAGE_STATE_CREATE_FAILED,
    MANAGED_BASE_IMAGE_IDS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::image_build_read_path::{get_build, list_builds, page_build_ids};
use crate::image_version_listing::list_versions;
use crate::microvm_image_and_instance_paths::{delete_image, get_image};
use crate::microvm_lifecycle::{
    as_uri_value, call, emulator, emulator_with_engine, expected_image_arn, image_body, ACCOUNT,
    PAST_THE_SPAN, REGION,
};

/// A build role ARN: `RoleArn`, pattern
/// `^arn:aws[a-z\-]*:iam::[0-9]{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`.
const BUILD_ROLE_ARN: &str = "arn:aws:iam::123456789012:role/microvm-refusals";

/// `PUT /2025-09-09/microvm-images/{imageIdentifier}` carrying the three members
/// `UpdateMicrovmImage` requires.
async fn update_image(router: &MultiServiceRouter, identifier: &str) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/2025-09-09/microvm-images/{}", as_uri_value(identifier)),
        Some(json!({
            "baseImageArn": managed_base_image_arn(REGION, MANAGED_BASE_IMAGE_IDS[0]),
            "buildRoleArn": BUILD_ROLE_ARN,
            "codeArtifact": {"uri": "s3://artifacts/app-next.zip"},
        })),
    )
    .await
}

/// Create the image `name`, asserting the create, and hand back the ARN it minted.
async fn create_image(router: &MultiServiceRouter, name: &str, extra: Value) -> String {
    let (status, body) = call(
        router,
        "POST",
        "/2025-09-09/microvm-images",
        Some(image_body(name, extra)),
    )
    .await;
    assert_eq!(
        status, 201,
        "create {name:?}: expected 201, got {status} {body}"
    );
    body["imageArn"].as_str().unwrap_or_default().to_string()
}

/// AWS's sentence for a build-addressed not-found, spelled out here rather than read from
/// the crate so a change to the wording fails this suite: the fixed text `Build not found
/// for MicroVMImageID: `, `, VersionID: ` and `, BuildID: `, with the three variable parts
/// taken from the path labels the request carried.
fn expected_build_not_found(identifier: &str, image_version: &str, build_id: &str) -> String {
    format!(
        "Build not found for MicroVMImageID: {identifier}, VersionID: {image_version}, \
         BuildID: {build_id}"
    )
}

/// The state the store holds for the image `name`, as the image's own read reports it.
async fn image_state(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = get_image(router, name).await;
    assert_eq!(status, 200, "read {name:?}: expected 200, got {status} {body}");
    body["state"].as_str().unwrap_or_default().to_string()
}

/// Write `state` onto the stored image `name`, so one row can address an image in any
/// `MicrovmImageState` value — including the ones a deletion settles in, which this
/// service's own clock reaches by removing the record.
fn seed_image_state(engine: &engine::InMemoryEngine, name: &str, state: &str) {
    let store = service_state(engine).expect("the service's own store");
    let mut store = store.lock();
    let image = store
        .image_named_mut(REGION, name)
        .unwrap_or_else(|| panic!("the created image {name:?} is held"));
    image.state = state.to_string();
}

/// RULE: a build-addressed read that resolves its image and version and finds no build
/// under the `buildId` the request named answers the modelled `ResourceNotFoundException`
/// (HTTP 404) whose message is AWS's own sentence for the class, built from the address
/// the request carried.
#[tokio::test]
async fn a_build_the_version_does_not_hold_is_refused_in_awss_words() {
    let router = emulator();
    let image_arn = create_image(&router, "worded", json!({})).await;
    let (status, listed) = list_builds(&router, &image_arn, FIRST_IMAGE_VERSION, "").await;
    assert_eq!(status, 200, "listing the builds got {status} {listed}");
    let held_build = page_build_ids(&listed, "worded")
        .first()
        .cloned()
        .unwrap_or_default();

    // Rows differ along what the sentence's variable parts generalise over: WHICH form of
    // the identifier the request addressed the image by, and the FORM of the build id
    // that is missing — the id form this service mints, a caller's own token, one at the
    // published maximum length of `NonBlankString` (`@length(1..2048)`), and one carrying
    // a separator.
    let identifiers = [("the image name", "worded".to_string()), ("the image ARN", image_arn.clone())];
    let build_ids = [
        "build-ffffffffffffffff".to_string(),
        "no-such-build".to_string(),
        "b".repeat(2048),
        "build.2026-01_candidate".to_string(),
    ];

    for (form, identifier) in &identifiers {
        for build_id in &build_ids {
            let case = format!("{form} {identifier:?} build {build_id:?}");
            let (status, body) =
                get_build(&router, identifier, FIRST_IMAGE_VERSION, build_id).await;
            assert_eq!(
                status, 404,
                "case {case}: expected 404, got {status} {body}"
            );
            assert_eq!(
                body["__type"].as_str(),
                Some("ResourceNotFoundException"),
                "case {case}: expected ResourceNotFoundException, got {body}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some(
                    expected_build_not_found(identifier, FIRST_IMAGE_VERSION, build_id).as_str()
                ),
                "case {case}: expected AWS's sentence built from the request's own address, \
                 got {:?}",
                body["message"]
            );
        }

        // The positive control: the build the version DOES hold is served through the
        // same path, so the rows above miss on the build id and not on a dead read.
        let (status, body) = get_build(&router, identifier, FIRST_IMAGE_VERSION, &held_build).await;
        assert_eq!(
            status, 200,
            "case {form}: the held build is readable, got {status} {body}"
        );
    }
}

/// RULE: past the authorization verdict a paginated child enumeration of an image reports
/// what this region's store holds at the address the request named — so an address of
/// this account's own image namespace holding no children, however it came to hold none,
/// is a terminal empty page rather than a refusal.
#[tokio::test]
async fn an_address_holding_no_children_gets_a_terminal_empty_page() {
    let (router, engine) = emulator_with_engine();
    let held_arn = create_image(&router, "held", json!({})).await;

    // An image this account holds whose deletion the clock has already completed: the
    // record is gone, so the identifier names nothing here.
    create_image(&router, "removed", json!({})).await;
    let (status, deleted) = delete_image(&router, "removed").await;
    assert_eq!(status, 200, "the delete expected 200, got {status} {deleted}");
    tokio::time::sleep(PAST_THE_SPAN).await;

    // Rows differ in HOW the address of this account's own namespace comes to hold no
    // children: a name nothing was created under, another region's ARN for a held name, a
    // record the clock has removed, and a held image addressed at a version it does not
    // hold (which only the build listing can name).
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "an ARN for a name never created",
            expected_image_arn("never-created"),
            FIRST_IMAGE_VERSION,
        ),
        (
            "another region's ARN for a held name",
            format!("arn:aws:lambda:eu-central-1:{ACCOUNT}:microvm-image:held"),
            FIRST_IMAGE_VERSION,
        ),
        (
            "a record the clock has removed",
            expected_image_arn("removed"),
            FIRST_IMAGE_VERSION,
        ),
        (
            "a held image's name, at a version it does not hold",
            "held".to_string(),
            "9.9",
        ),
        (
            "a held image's ARN, at a version in no form this service mints",
            held_arn.clone(),
            "latest",
        ),
    ];

    for (case, identifier, image_version) in &rows {
        let (status, body) = list_builds(&router, identifier, image_version, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: listing the builds of {identifier:?} at {image_version:?} expected \
             200, got {status} {body}"
        );
        assert_eq!(
            body["items"],
            json!([]),
            "case {case:?}: items is projected as an empty list, got {body}"
        );
        assert!(
            body["nextToken"].is_null(),
            "case {case:?}: an exhausted listing issues no nextToken, got {:?}",
            body["nextToken"]
        );

        // The version listing has no version label, so it is exercised with the rows
        // whose IMAGE holds nothing — the held image itself does hold versions.
        if identifier == "held" || identifier == &held_arn {
            continue;
        }
        let (status, body) = list_versions(&router, identifier, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: listing the versions of {identifier:?} expected 200, got {status} \
             {body}"
        );
        assert_eq!(
            body["items"],
            json!([]),
            "case {case:?}: items is projected as an empty list, got {body}"
        );
    }

    // An image this account holds whose version records are all gone: the identifier
    // resolves and the collection is empty, which is the same empty page.
    {
        let store = service_state(&*engine).expect("the service's own store");
        let mut store = store.lock();
        store
            .image_named_mut(REGION, "held")
            .expect("the created image is held")
            .versions
            .clear();
    }
    let (status, body) = list_versions(&router, &held_arn, "").await;
    assert_eq!(
        status, 200,
        "an emptied image's versions expected 200, got {status} {body}"
    );
    assert_eq!(
        body["items"],
        json!([]),
        "an emptied image holds no versions to enumerate, got {body}"
    );

    // The refusals the enumerations KEEP: an identifier of another namespace is denied
    // before anything is looked up, and a value that is no identifier of this API at all
    // is a request that can never succeed.
    let managed = managed_base_image_arn(REGION, MANAGED_BASE_IMAGE_IDS[0]);
    let other_account = format!("arn:aws:lambda:{REGION}:210987654321:microvm-image:someone-elses");
    let refusals: Vec<(&str, String, u16, &str, String)> = vec![
        (
            "the AWS-owned namespace",
            managed.clone(),
            403,
            "AccessDeniedException",
            access_denied_message(),
        ),
        (
            "another account's namespace",
            other_account.clone(),
            403,
            "AccessDeniedException",
            access_denied_message(),
        ),
        (
            "a bare name no image of this account carries",
            "never-created".to_string(),
            400,
            "ValidationException",
            invalid_arn_format_message("never-created"),
        ),
        (
            "no identifier of this API at all",
            "not-an-identifier".to_string(),
            400,
            "ValidationException",
            invalid_arn_format_message("not-an-identifier"),
        ),
    ];

    for (case, identifier, expected_status, expected_type, expected_message) in refusals {
        for (operation, (status, body)) in [
            (
                "ListMicrovmImageVersions",
                list_versions(&router, &identifier, "").await,
            ),
            (
                "ListMicrovmImageBuilds",
                list_builds(&router, &identifier, FIRST_IMAGE_VERSION, "").await,
            ),
        ] {
            assert_eq!(
                status, expected_status,
                "case {case:?} on {operation}: expected {expected_status}, got {status} {body}"
            );
            assert_eq!(
                body["__type"].as_str(),
                Some(expected_type),
                "case {case:?} on {operation}: expected {expected_type}, got {body}"
            );
            assert_eq!(
                body["message"].as_str(),
                Some(expected_message.as_str()),
                "case {case:?} on {operation}: expected the refusal of that namespace, got {:?}",
                body["message"]
            );
        }
    }
}

/// RULE: `UpdateMicrovmImage` builds the next version from the image's CURRENT record, so
/// only a state whose lifecycle settled on a version it built admits one — `CREATED`,
/// `UPDATED` and `UPDATE_FAILED` of the nine `MicrovmImageState` values
/// (`smithy_query(query="shape",shape="MicrovmImageState")`). Every other value is
/// refused with HTTP 400 `ValidationException` naming the image's own ARN, and the
/// refused call leaves the record in the state it found it in.
#[tokio::test]
async fn an_update_is_refused_for_every_state_that_admits_none() {
    let (router, engine) = emulator_with_engine();

    // Rows span the WHOLE published enum, so the table is the state dimension itself
    // rather than a sample of it; the states an update may start from are the settled
    // values of the lifecycles that built a version.
    let admits_update = ["CREATED", "UPDATED", "UPDATE_FAILED"];
    assert_eq!(
        MICROVM_IMAGE_STATE_VALUES.len(),
        9,
        "the table must span every published MicrovmImageState value, got {:?}",
        MICROVM_IMAGE_STATE_VALUES
    );

    for (row, state) in MICROVM_IMAGE_STATE_VALUES.iter().enumerate() {
        // Each row gets its own image, so an accepted update cannot change what the next
        // row is answered.
        let name = format!("state-{row}");
        let image_arn = create_image(&router, &name, json!({})).await;
        tokio::time::sleep(PAST_THE_SPAN).await;
        // The create's build settles on the next call that reads the store, so the image
        // is read back FIRST: what this row then writes is the state of a settled record
        // rather than one the next call's clock is about to overwrite.
        assert_eq!(
            image_state(&router, &name).await,
            "CREATED",
            "case {state:?}: the create's build must have settled before the row writes a state"
        );
        seed_image_state(&engine, &name, state);

        let (status, body) = update_image(&router, &name).await;
        if admits_update.contains(state) {
            assert_eq!(
                status, 200,
                "case {state:?}: a settled, built image admits an update, got {status} {body}"
            );
            assert_eq!(
                image_state(&router, &name).await,
                "UPDATING",
                "case {state:?}: the accepted update puts the image's build in flight"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {state:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            body["__type"].as_str(),
            Some("ValidationException"),
            "case {state:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(image_not_updatable_message(&image_arn).as_str()),
            "case {state:?}: the refusal names the image's own ARN, got {:?}",
            body["message"]
        );
        assert_eq!(
            image_state(&router, &name).await,
            *state,
            "case {state:?}: a refused update leaves the record in the state it found"
        );
    }
}

/// RULE: the same guard holds for a state this service's own clock settles an image in —
/// an image whose creation build FAILED has no version to carry forward, so the update is
/// refused and the record stays in the terminal state its creation settled in, which is
/// what lets a client's poll for a terminal creation state converge.
#[tokio::test]
async fn an_update_of_an_image_whose_creation_failed_is_refused() {
    let router = crate::image_build_outcome::emulator();

    // Rows differ in WHICH half of the object address the store does not serve, so the
    // failed creation is reached through the build's own resolution rather than seeded.
    let rows: Vec<(&str, &str, String)> = vec![
        (
            "a bucket the object store does not serve",
            "absent-bucket",
            "s3://no-such-bucket/app.zip".to_string(),
        ),
        (
            "a key the served bucket does not hold",
            "absent-key",
            "s3://staged-artifacts/no-such-key.zip".to_string(),
        ),
    ];
    crate::image_build_outcome::stage_object(&router, "staged-artifacts", "app.zip").await;

    for (case, name, uri) in rows {
        let image_arn = crate::image_build_outcome::create_image(&router, name, &uri).await;
        tokio::time::sleep(PAST_THE_SPAN).await;
        let (status, image) = crate::image_build_outcome::get_image(&router, name).await;
        assert_eq!(status, 200, "case {case:?}: reading got {status} {image}");
        assert_eq!(
            image["state"].as_str(),
            Some(IMAGE_STATE_CREATE_FAILED),
            "case {case:?}: the creation's build must have failed, got {image}"
        );

        let (status, body) = update_image(&router, &image_arn).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            body["__type"].as_str(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(image_not_updatable_message(&image_arn).as_str()),
            "case {case:?}: the refusal names the image's own ARN, got {:?}",
            body["message"]
        );

        // The refused update wrote nothing: the record is still the one the failed
        // creation settled, so a poll for a terminal creation state converges.
        let (status, after) = crate::image_build_outcome::get_image(&router, name).await;
        assert_eq!(status, 200, "case {case:?}: re-reading got {status} {after}");
        assert_eq!(
            after["state"].as_str(),
            Some(IMAGE_STATE_CREATE_FAILED),
            "case {case:?}: a refused update leaves the terminal state as it was, got {after}"
        );
        assert_eq!(
            after["latestActiveImageVersion"],
            Value::Null,
            "case {case:?}: a creation that built nothing has no active version, got {after}"
        );
    }
}
