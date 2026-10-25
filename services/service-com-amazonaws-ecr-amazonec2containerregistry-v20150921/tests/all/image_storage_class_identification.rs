// SPDX-License-Identifier: CC0-1.0

//! How `UpdateImageStorageClass` names the image it transitions, end to end over the wire.
//!
//! The rule under test: a storage class update addresses its image with the same
//! `imageId` resolution every operation taking one uses — a digest, a tag, or both when
//! they agree — transitions exactly that stored image, and answers with the registry, the
//! repository and the image's own stored digest; an identifier that resolves to no stored
//! image, a malformed one, and a request naming another account's registry are refused and
//! persist nothing.
//!
//! The suite beside this one (`image_storage_class_transitions`) covers which transitions
//! the state machine allows; this one covers WHICH image a call lands on and what it
//! reports.
//!
//! The wire harness is `crate::wire`'s: every case goes through the router, so it exercises
//! routing, deserialization, the handler, the persisted state and serialization — the same
//! path a client takes.

use serde_json::{json, Value};
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921 as ecr;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{at, call, emulator, given_layer, ok, refused, REGISTRY_ID};

use ecr::state;

/// The two `TargetStorageClass` values a request may ask for
/// (`smithy_query(query="shape", shape="TargetStorageClass")`: `ARCHIVE` | `STANDARD`).
const ARCHIVE: &str = "ARCHIVE";
/// See [`ARCHIVE`].
const STANDARD: &str = "STANDARD";

/// Push one image carrying `tag` into `repository`, answering the digest the registry
/// addresses it by. `salt` gives each image of a suite its own blob, so no two images of a
/// case collide on a digest.
async fn given_tagged_image(
    router: &MultiServiceRouter,
    case: &str,
    repository: &str,
    salt: &str,
    tag: &str,
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
        &json!({
            "repositoryName": repository,
            "imageManifest": manifest,
            "imageTag": tag,
        }),
    )
    .await;
    state::manifest_digest(&manifest)
}

/// The `ImageDetail` of one digest, asking for images of any status so an archived image is
/// read back too.
async fn detail(router: &MultiServiceRouter, case: &str, repository: &str, digest: &str) -> Value {
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

/// One `imageId` of a request, built from the parts the case names.
fn image_id(digest: Option<&str>, tag: Option<&str>) -> Value {
    let mut id = json!({});
    if let Some(digest) = digest {
        id["imageDigest"] = json!(digest);
    }
    if let Some(tag) = tag {
        id["imageTag"] = json!(tag);
    }
    id
}

/// A well-formed content digest nothing in the repository was pushed under: the
/// `sha256:<64 hex>` form a registry addresses a blob by, so it is the IDENTIFIER that
/// misses and not the digest's form.
const UNSTORED_DIGEST: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";

/// The wording ECR refuses an unresolvable `imageId` with, spelled out here rather than
/// imported so a wrong message in the implementation fails this test. An absent member of
/// the requested identifier renders as `null`.
fn image_not_found_message(repository: &str, digest: Option<&str>, tag: Option<&str>) -> String {
    format!(
        "The image with imageId {{imageDigest:'{}', imageTag:'{}'}} does not exist within \
         the repository with name '{repository}' in the registry with id '{REGISTRY_ID}'",
        digest.unwrap_or("null"),
        tag.unwrap_or("null")
    )
}

/// Every form an `imageId` may take lands on the same stored image, and the answer reports
/// the registry, the repository and the image's own stored digest.
///
/// The rows differ along what the rule generalises over: which members of the identifier
/// the request carries (digest alone, tag alone, both together) and whether it names the
/// registry explicitly or leaves it to the default.
#[tokio::test]
async fn a_transition_addresses_its_image_by_digest_by_tag_or_by_both() {
    // (case, carries the digest, carries the tag, names the registry explicitly)
    let forms: Vec<(&str, bool, bool, bool)> = vec![
        ("by digest", true, false, false),
        ("by tag", false, true, false),
        ("by digest and tag", true, true, false),
        ("by digest, registry named", true, false, true),
        ("by tag, registry named", false, true, true),
    ];
    let tag = "release";

    for (case, by_digest, by_tag, with_registry) in forms {
        let router = emulator();
        let repository = "identification";
        ok(
            &router,
            case,
            "CreateRepository",
            &json!({"repositoryName": repository}),
        )
        .await;
        let digest = given_tagged_image(&router, case, repository, case, tag).await;

        let mut request = json!({
            "repositoryName": repository,
            "imageId": image_id(by_digest.then_some(digest.as_str()), by_tag.then_some(tag)),
            "targetStorageClass": ARCHIVE,
        });
        if with_registry {
            request["registryId"] = json!(REGISTRY_ID);
        }
        let answer = ok(&router, case, "UpdateImageStorageClass", &request).await;

        assert_eq!(
            at(&answer, "/imageStatus"),
            json!(state::ARCHIVED_IMAGE_STATUS),
            "case {case:?}: expected the image to report {}, got {answer:?}",
            state::ARCHIVED_IMAGE_STATUS
        );
        assert_eq!(
            at(&answer, "/registryId"),
            json!(REGISTRY_ID),
            "case {case:?}: expected the registry {REGISTRY_ID}, got {answer:?}"
        );
        assert_eq!(
            at(&answer, "/repositoryName"),
            json!(repository),
            "case {case:?}: expected the repository {repository:?}, got {answer:?}"
        );
        // The digest answered is the STORED one, whether or not the request carried it.
        assert_eq!(
            at(&answer, "/imageId/imageDigest"),
            json!(digest),
            "case {case:?}: expected the stored digest {digest:?}, got {answer:?}"
        );
        let expected_tag = if by_tag { json!(tag) } else { Value::Null };
        assert_eq!(
            at(&answer, "/imageId/imageTag"),
            expected_tag,
            "case {case:?}: expected the tag {expected_tag:?}, got {answer:?}"
        );

        // Round trip: the transition is persisted on that image, so a read reports the
        // archived status and the instant the archive happened.
        let stored = detail(&router, case, repository, &digest).await;
        assert_eq!(
            at(&stored, "/imageStatus"),
            json!(state::ARCHIVED_IMAGE_STATUS),
            "case {case:?}: expected the stored image archived, got {stored:?}"
        );
        assert!(
            at(&stored, "/lastArchivedAt") != Value::Null,
            "case {case:?}: expected a lastArchivedAt on the archived image, got {stored:?}"
        );
        assert_eq!(
            at(&stored, "/lastActivatedAt"),
            Value::Null,
            "case {case:?}: an image that was never restored carries no lastActivatedAt, \
             got {stored:?}"
        );

        // The same identifier restores it, so the resolution is the operation's and not
        // the archive direction's.
        let mut restore = request.clone();
        restore["targetStorageClass"] = json!(STANDARD);
        let restored = ok(&router, case, "UpdateImageStorageClass", &restore).await;
        assert_eq!(
            at(&restored, "/imageStatus"),
            json!(state::ACTIVATING_IMAGE_STATUS),
            "case {case:?}: expected the restore to report {}, got {restored:?}",
            state::ACTIVATING_IMAGE_STATUS
        );
        let stored = detail(&router, case, repository, &digest).await;
        assert!(
            at(&stored, "/lastActivatedAt") != Value::Null,
            "case {case:?}: expected a lastActivatedAt on the restored image, got {stored:?}"
        );
    }
}

/// Transitioning one image leaves its neighbours in the repository where they are: the
/// storage class is per-image state.
#[tokio::test]
async fn a_transition_moves_only_the_image_it_names() {
    let router = emulator();
    let repository = "neighbours";
    let case = "one of three images archived";
    ok(
        &router,
        case,
        "CreateRepository",
        &json!({"repositoryName": repository}),
    )
    .await;
    let mut digests = Vec::new();
    for salt in ["first", "second", "third"] {
        digests.push(given_tagged_image(&router, case, repository, salt, salt).await);
    }

    ok(
        &router,
        case,
        "UpdateImageStorageClass",
        &json!({
            "repositoryName": repository,
            "imageId": {"imageDigest": digests[1]},
            "targetStorageClass": ARCHIVE,
        }),
    )
    .await;

    for (index, digest) in digests.iter().enumerate() {
        let expected = if index == 1 {
            state::ARCHIVED_IMAGE_STATUS
        } else {
            state::ACTIVE_IMAGE_STATUS
        };
        let stored = detail(&router, case, repository, digest).await;
        assert_eq!(
            at(&stored, "/imageStatus"),
            json!(expected),
            "case {case:?}: image {index} must be {expected}, got {stored:?}"
        );
    }
}

/// Every request that addresses no transitionable image is refused with the error its
/// failure is, carries ECR's wording for it, and persists nothing.
///
/// The rows differ along WHAT about the request cannot be resolved: the repository, the
/// digest, the tag, the two together, the digest's form, an identifier with no members at
/// all, and the registry the request names.
#[tokio::test]
async fn an_unresolvable_request_is_refused_and_changes_nothing() {
    let router = emulator();
    let repository = "refusals";
    let case = "arranging the refusal suite";
    ok(
        &router,
        case,
        "CreateRepository",
        &json!({"repositoryName": repository}),
    )
    .await;
    let tag = "live";
    let digest = given_tagged_image(&router, case, repository, "subject", tag).await;
    let other_tag = "spare";
    let other = given_tagged_image(&router, case, repository, "spare", other_tag).await;
    let before = detail(&router, case, repository, &digest).await;
    let other_before = detail(&router, case, repository, &other).await;

    // An account id that is not this registry's, in the modelled `^[0-9]{12}$` form so it
    // is the registry that is wrong and not the parameter's shape.
    let another_registry = "123456789012";
    let absent_repository = "no-such-repository";
    let absent_tag = "no-such-tag";
    let malformed_digest = "not-a-digest";

    // (case, the request, the expected error code, the expected message)
    let rows: Vec<(&str, Value, &str, String)> = vec![
        (
            "unknown repository",
            json!({
                "repositoryName": absent_repository,
                "imageId": {"imageDigest": digest},
                "targetStorageClass": ARCHIVE,
            }),
            "RepositoryNotFoundException",
            format!(
                "The repository with name '{absent_repository}' does not exist in the \
                 registry with id '{REGISTRY_ID}'"
            ),
        ),
        (
            "unstored digest",
            json!({
                "repositoryName": repository,
                "imageId": {"imageDigest": UNSTORED_DIGEST},
                "targetStorageClass": ARCHIVE,
            }),
            "ImageNotFoundException",
            image_not_found_message(repository, Some(UNSTORED_DIGEST), None),
        ),
        (
            "unknown tag",
            json!({
                "repositoryName": repository,
                "imageId": {"imageTag": absent_tag},
                "targetStorageClass": ARCHIVE,
            }),
            "ImageNotFoundException",
            image_not_found_message(repository, None, Some(absent_tag)),
        ),
        (
            "digest and tag naming different images",
            json!({
                "repositoryName": repository,
                "imageId": {"imageDigest": digest, "imageTag": other_tag},
                "targetStorageClass": ARCHIVE,
            }),
            "ImageNotFoundException",
            image_not_found_message(repository, Some(&digest), Some(other_tag)),
        ),
        (
            "malformed digest",
            json!({
                "repositoryName": repository,
                "imageId": {"imageDigest": malformed_digest},
                "targetStorageClass": ARCHIVE,
            }),
            "InvalidParameterException",
            format!(
                "Invalid parameter at 'imageIds' failed to satisfy constraint: imageDigest \
                 '{malformed_digest}' must be of the form <algorithm>:<hex digest>"
            ),
        ),
        (
            "identifier with neither member",
            json!({
                "repositoryName": repository,
                "imageId": {},
                "targetStorageClass": ARCHIVE,
            }),
            "InvalidParameterException",
            "an image identifier must carry an imageDigest, an imageTag, or both".to_string(),
        ),
        (
            "another account's registry",
            json!({
                "repositoryName": repository,
                "registryId": another_registry,
                "imageId": {"imageDigest": digest},
                "targetStorageClass": ARCHIVE,
            }),
            "InvalidParameterException",
            format!(
                "registryId '{another_registry}' is not this registry; the only registry \
                 served here is '{REGISTRY_ID}'"
            ),
        ),
    ];

    for (row, request, code, message) in rows {
        let body = refused(&router, row, "UpdateImageStorageClass", &request, code).await;
        assert_eq!(
            body["message"].as_str().unwrap_or_default(),
            message,
            "case {row:?}: expected the message {message:?}, got {:?}",
            body["message"]
        );
    }

    // Nothing moved: both stored images are exactly what they were before the refusals.
    let after = detail(&router, case, repository, &digest).await;
    assert_eq!(
        after, before,
        "case {case:?}: a refused update must change nothing, got {after:?} after {before:?}"
    );
    let other_after = detail(&router, case, repository, &other).await;
    assert_eq!(
        other_after, other_before,
        "case {case:?}: a refused update must leave the other image alone, got \
         {other_after:?} after {other_before:?}"
    );
}

/// A target storage class outside the modelled `TargetStorageClass` enum, and a missing
/// one, are refused at the request boundary — before any image is resolved — so no
/// unknown target can reach the state machine.
#[tokio::test]
async fn a_target_outside_the_modelled_enum_is_refused() {
    let router = emulator();
    let repository = "targets";
    let case = "arranging the target suite";
    ok(
        &router,
        case,
        "CreateRepository",
        &json!({"repositoryName": repository}),
    )
    .await;
    let digest = given_tagged_image(&router, case, repository, "target", "live").await;
    let before = detail(&router, case, repository, &digest).await;

    // Rows differ along HOW the target is wrong: an unmodelled value, a lower-case
    // spelling of a modelled one, and no target at all.
    let rows: Vec<(&str, Value)> = vec![
        (
            "unmodelled target",
            json!({
                "repositoryName": repository,
                "imageId": {"imageDigest": digest},
                "targetStorageClass": "GLACIER",
            }),
        ),
        (
            "lower-case target",
            json!({
                "repositoryName": repository,
                "imageId": {"imageDigest": digest},
                "targetStorageClass": "archive",
            }),
        ),
        (
            "no target",
            json!({
                "repositoryName": repository,
                "imageId": {"imageDigest": digest},
            }),
        ),
    ];

    for (row, request) in rows {
        let answer = call(&router, "UpdateImageStorageClass", &request).await;
        assert_eq!(
            answer.status, 400,
            "case {row:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            !answer.error_code().is_empty() && answer.error_code() != "NotImplemented",
            "case {row:?}: expected a modelled refusal, got {:?}",
            answer.body
        );
        let after = detail(&router, row, repository, &digest).await;
        assert_eq!(
            after, before,
            "case {row:?}: a refused update must change nothing, got {after:?} after \
             {before:?}"
        );
    }
}
