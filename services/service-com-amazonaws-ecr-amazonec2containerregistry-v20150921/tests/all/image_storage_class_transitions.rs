// SPDX-License-Identifier: CC0-1.0

//! `UpdateImageStorageClass`'s storage-class state machine, end to end over the wire.
//!
//! The rule under test: a storage class update is supported only where it has a transition
//! to make — an image in Amazon ECR standard storage can be archived, and an image in
//! Amazon ECR archive can be restored. A target storage class the image's own status has no
//! transition to, because it is already that storage class or is still leaving it, is
//! refused with `ImageStorageClassUpdateNotSupportedException` and leaves every stored
//! instant exactly as the last real transition set it — so a client using the error to
//! detect "already archived" sees it.
//!
//! The wire harness is `crate::wire`'s: every case goes through the router, so it exercises
//! routing, deserialization, the handlers, the persisted state and serialization — the same
//! path a client takes.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921 as ecr;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{at, call, emulator, given_layer, ok, refused};

use ecr::state;

/// Long enough past [`state::IMAGE_RESTORE_DELAY`] that the restore the clock owes is
/// certainly due. Derived from the span itself, never a bare number.
const PAST_THE_RESTORE: Duration =
    state::IMAGE_RESTORE_DELAY.saturating_add(Duration::from_millis(150));

/// The `TargetStorageClass` values a request may ask for
/// (`smithy_query(query="shape", shape="TargetStorageClass")`: `ARCHIVE` | `STANDARD`), so
/// the table below covers every target rather than the one the report cited.
const ARCHIVE: &str = "ARCHIVE";
/// See [`ARCHIVE`].
const STANDARD: &str = "STANDARD";

/// Push one image with a single filesystem layer into `repository`, answering the digest
/// the registry addresses it by. `salt` gives each image of a suite its own blob, so no two
/// images collide on a digest.
async fn given_image(
    router: &MultiServiceRouter,
    case: &str,
    repository: &str,
    salt: &str,
) -> String {
    let content = format!("layer-of-{salt}").into_bytes();
    let layer = given_layer(router, case, repository, &content).await;
    let manifest = json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.docker.distribution.manifest.v2+json",
        "layers": [{
            "mediaType": "application/vnd.docker.image.rootfs.diff.tar.gzip",
            "digest": layer,
            "size": content.len(),
        }],
    })
    .to_string();
    ok(
        router,
        case,
        "PutImage",
        &json!({"repositoryName": repository, "imageManifest": manifest}),
    )
    .await;
    state::manifest_digest(&manifest)
}

/// Ask for one transition, answering the whole response body.
async fn transition(
    router: &MultiServiceRouter,
    case: &str,
    repository: &str,
    digest: &str,
    target: &str,
) -> Value {
    ok(
        router,
        case,
        "UpdateImageStorageClass",
        &json!({
            "repositoryName": repository,
            "imageId": {"imageDigest": digest},
            "targetStorageClass": target,
        }),
    )
    .await
}

/// The `ImageDetail` of one digest, asking for images of any status so an archived image is
/// read too.
async fn detail(
    router: &MultiServiceRouter,
    case: &str,
    repository: &str,
    digest: &str,
) -> Value {
    let body = ok(
        router,
        case,
        "DescribeImages",
        &json!({
            "repositoryName": repository,
            "imageIds": [{"imageDigest": digest}],
            "filter": {"imageStatus": "ANY"},
        }),
    )
    .await;
    at(&body, "/imageDetails/0")
}

/// Put one image into the status `status` names and answer its digest: a push leaves it
/// `ACTIVE`, an archive moves it to `ARCHIVED`, a restore starts it `ACTIVATING`, and a
/// restore the clock has made due leaves it `ACTIVE` again.
async fn given_image_in(
    router: &MultiServiceRouter,
    case: &str,
    repository: &str,
    salt: &str,
    status: &str,
    restored: bool,
) -> String {
    let digest = given_image(router, case, repository, salt).await;
    if status != state::ACTIVE_IMAGE_STATUS || restored {
        transition(router, case, repository, &digest, ARCHIVE).await;
    }
    if status == state::ACTIVATING_IMAGE_STATUS || restored {
        transition(router, case, repository, &digest, STANDARD).await;
    }
    if restored {
        tokio::time::sleep(PAST_THE_RESTORE).await;
    }
    let reported = detail(router, case, repository, &digest).await;
    assert_eq!(
        at(&reported, "/imageStatus"),
        json!(status),
        "case {case:?}: the arranged image must be in {status}, got {reported:?}"
    );
    digest
}

/// Every (status, target) pair: the two transitions that have something to do are honoured,
/// and every other pair is refused with the modelled error and changes nothing.
///
/// The rows differ along what the rule generalises over: which status the image is in
/// (`ACTIVE` as pushed, `ARCHIVED`, `ACTIVATING` while a restore is in flight, and `ACTIVE`
/// again after a restore the clock completed — so a status reached twice over is covered
/// too) and which of the two target storage classes is asked for. The report's own case is
/// the `ARCHIVED` row asking for `ARCHIVE`.
#[tokio::test]
async fn a_storage_class_update_is_supported_only_where_it_has_a_transition_to_make() {
    // (case, the status the image is arranged in, whether it got there through a restore)
    let arrangements: Vec<(&str, &str, bool)> = vec![
        ("as pushed", state::ACTIVE_IMAGE_STATUS, false),
        ("archived", state::ARCHIVED_IMAGE_STATUS, false),
        ("restoring", state::ACTIVATING_IMAGE_STATUS, false),
        ("restored", state::ACTIVE_IMAGE_STATUS, true),
    ];

    for (arrangement, status, restored) in arrangements {
        for target in [ARCHIVE, STANDARD] {
            let case = format!("{arrangement}, asked for {target}");
            let router = emulator();
            let repository = "storage-class";
            ok(
                &router,
                &case,
                "CreateRepository",
                &json!({"repositoryName": repository}),
            )
            .await;
            let digest =
                given_image_in(&router, &case, repository, arrangement, status, restored).await;
            let before = detail(&router, &case, repository, &digest).await;

            // The two transitions that move an image between the storage classes: archive
            // what is in standard storage, restore what is in archive. Everything else
            // names the class the image already is, or one it is still leaving.
            let supported = match target {
                ARCHIVE => status == state::ACTIVE_IMAGE_STATUS,
                _ => status == state::ARCHIVED_IMAGE_STATUS,
            };

            if supported {
                let answer = transition(&router, &case, repository, &digest, target).await;
                let expected = if target == ARCHIVE {
                    state::ARCHIVED_IMAGE_STATUS
                } else {
                    state::ACTIVATING_IMAGE_STATUS
                };
                assert_eq!(
                    at(&answer, "/imageStatus"),
                    json!(expected),
                    "case {case:?}: expected the image to report {expected}, got {:?}",
                    at(&answer, "/imageStatus")
                );
                continue;
            }

            let body = refused(
                &router,
                &case,
                "UpdateImageStorageClass",
                &json!({
                    "repositoryName": repository,
                    "imageId": {"imageDigest": digest},
                    "targetStorageClass": target,
                }),
                "ImageStorageClassUpdateNotSupportedException",
            )
            .await;
            // The refusal states what was asked for and the state that has no transition
            // to it, both read off the request and the stored image.
            let expected =
                format!("Image cannot be updated to {target} storage class when it is in {status} state");
            assert_eq!(
                body["message"].as_str().unwrap_or_default(),
                expected,
                "case {case:?}: expected the message {expected:?}, got {:?}",
                body["message"]
            );

            // Nothing moved: the status and both transition instants are what they were
            // before the refused call.
            let after = detail(&router, &case, repository, &digest).await;
            assert_eq!(
                after, before,
                "case {case:?}: a refused update must change nothing, got {after:?} after \
                 {before:?}"
            );
        }
    }
}

/// The supported transitions can be taken again once the image is back in the other
/// storage class: archive, restore, wait for the restore the clock owes, archive again.
///
/// So the refusal is about the image's STATE and not about the image having been
/// transitioned before.
#[tokio::test]
async fn an_image_may_be_archived_again_once_it_is_back_in_standard_storage() {
    let router = emulator();
    let repository = "re-archive";
    let case = "archive, restore, archive again";
    ok(
        &router,
        case,
        "CreateRepository",
        &json!({"repositoryName": repository}),
    )
    .await;
    let digest = given_image(&router, case, repository, "cycle").await;

    for round in 1..=2 {
        let archived = transition(&router, case, repository, &digest, ARCHIVE).await;
        assert_eq!(
            at(&archived, "/imageStatus"),
            json!(state::ARCHIVED_IMAGE_STATUS),
            "case {case:?}: round {round} must archive the image, got {:?}",
            at(&archived, "/imageStatus")
        );
        let restoring = transition(&router, case, repository, &digest, STANDARD).await;
        assert_eq!(
            at(&restoring, "/imageStatus"),
            json!(state::ACTIVATING_IMAGE_STATUS),
            "case {case:?}: round {round} must start the restore, got {:?}",
            at(&restoring, "/imageStatus")
        );
        // Asking for the restore again while it is in flight has nothing to transition.
        let answer = call(
            &router,
            "UpdateImageStorageClass",
            &json!({
                "repositoryName": repository,
                "imageId": {"imageDigest": digest},
                "targetStorageClass": STANDARD,
            }),
        )
        .await;
        assert_eq!(
            answer.error_code(),
            "ImageStorageClassUpdateNotSupportedException",
            "case {case:?}: round {round} must refuse a restore already in flight, got {:?}",
            answer.body
        );
        tokio::time::sleep(PAST_THE_RESTORE).await;
    }

    let detail = detail(&router, case, repository, &digest).await;
    assert_eq!(
        at(&detail, "/imageStatus"),
        json!(state::ACTIVE_IMAGE_STATUS),
        "case {case:?}: the image ends in standard storage, got {detail:?}"
    );
}
