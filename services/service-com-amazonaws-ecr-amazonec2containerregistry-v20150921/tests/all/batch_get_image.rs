// SPDX-License-Identifier: CC0-1.0

//! `BatchGetImage`, end to end over the wire.
//!
//! The rule under test: for every `imageIds` entry, the call reports the manifest of
//! the stored image that answers to every part of that entry, and reports the entry
//! itself in `failures` — with the `ImageFailureCode` that says why — when no stored
//! image does; the call as a whole fails only when the repository it names is not in
//! this registry, or when it names another registry. So a batch is per-entry: a miss
//! never hides a hit, and an empty repository answers 200 with every entry failed.
//!
//! The wire plumbing is the crate's shared harness (`tests/all/wire.rs`); the seeding
//! helpers are the lifecycle suite's.

use serde_json::{json, Value};

use crate::repository_image_lifecycle::{given_repository, manifest_digest};
use crate::wire::{at, call, emulator, ok, REGISTRY_ID};

/// A Docker image manifest V2 schema 2, which declares its own media type.
///
/// The manifests this suite pushes reference no blob: a manifest cannot be pushed before
/// every blob it references is in the repository (`PutImage`'s
/// `LayersNotFoundException`), and what is under test here is how an `imageIds` entry
/// resolves and what the stored manifest reports — not the repository's blobs, which the
/// `layer_availability` suite covers.
const MANIFEST_DOCKER_V2: &str = concat!(
    r#"{"schemaVersion":2,"#,
    r#""mediaType":"application/vnd.docker.distribution.manifest.v2+json"}"#
);

/// An OCI image manifest, which declares a different media type. See
/// [`MANIFEST_DOCKER_V2`] on why it references no blob.
const MANIFEST_OCI: &str = concat!(
    r#"{"schemaVersion":2,"#,
    r#""mediaType":"application/vnd.oci.image.manifest.v1+json"}"#
);

/// A manifest that declares no media type at all, so the stored image reports none.
const MANIFEST_UNTYPED: &str = r#"{"schemaVersion":1,"name":"untyped"}"#;

/// A manifest nothing is ever pushed under, so its digest names no stored image.
const MANIFEST_NEVER_PUSHED: &str = r#"{"schemaVersion":2,"never":"pushed"}"#;

/// One row of the batching table: the case name, the entries the call asks for, the
/// manifests it must report in `images`, and the codes it must report in `failures`.
type BatchCase<'a> = (&'a str, Vec<Value>, Vec<&'a str>, Vec<&'a str>);

/// What one `imageIds` entry must resolve to.
#[derive(Debug)]
enum Expected {
    /// The entry names this stored image: the manifest it was pushed with, the media
    /// type that manifest declares, and the tag the request addressed it by.
    Image {
        manifest: &'static str,
        media_type: Option<&'static str>,
        tag: Option<&'static str>,
    },
    /// The entry names no stored image, and is reported with this `ImageFailureCode`.
    Failure(&'static str),
}

/// The repository every resolution case reads, seeded through `PutImage` so that what
/// `BatchGetImage` answers can only come from the persisted push.
///
/// It holds a Docker V2 manifest under two tags, an OCI manifest under one, and an
/// untagged manifest that declares no media type.
async fn seeded(name: &str) -> smithy_server_runtime::MultiServiceRouter {
    let router = emulator();
    given_repository(&router, name, json!({})).await;
    for (case, manifest, tag) in [
        ("docker v2 as v1", MANIFEST_DOCKER_V2, Some("v1")),
        ("docker v2 as latest", MANIFEST_DOCKER_V2, Some("latest")),
        ("oci as oci", MANIFEST_OCI, Some("oci")),
        ("untyped, untagged", MANIFEST_UNTYPED, None),
    ] {
        let mut request = json!({"repositoryName": name, "imageManifest": manifest});
        if let Some(tag) = tag {
            request["imageTag"] = json!(tag);
        }
        ok(&router, case, "PutImage", &request).await;
    }
    router
}

/// Assert one `BatchGetImage` answer carrying exactly one entry's result.
fn assert_single(
    case: &str,
    repository: &str,
    requested: &Value,
    answer: &Value,
    expected: &Expected,
) {
    let images = answer["images"].as_array().cloned().unwrap_or_default();
    let failures = answer["failures"].as_array().cloned().unwrap_or_default();
    match expected {
        Expected::Image {
            manifest,
            media_type,
            tag,
        } => {
            assert!(
                failures.is_empty(),
                "case {case:?}: expected no failures, got {failures:?}"
            );
            assert_eq!(
                images.len(),
                1,
                "case {case:?}: expected exactly one image, got {images:?}"
            );
            let image = &images[0];
            assert_eq!(
                at(image, "/imageManifest"),
                json!(manifest),
                "case {case:?}: expected the pushed manifest, got {:?}",
                image["imageManifest"]
            );
            assert_eq!(
                at(image, "/imageManifestMediaType"),
                media_type.map_or(Value::Null, |value| json!(value)),
                "case {case:?}: expected media type {media_type:?}, got {:?}",
                image["imageManifestMediaType"]
            );
            assert_eq!(
                at(image, "/imageId/imageDigest"),
                json!(manifest_digest(manifest)),
                "case {case:?}: expected the manifest's own digest, got {:?}",
                image["imageId"]
            );
            assert_eq!(
                at(image, "/imageId/imageTag"),
                tag.map_or(Value::Null, |value| json!(value)),
                "case {case:?}: expected the requested tag {tag:?}, got {:?}",
                image["imageId"]
            );
            assert_eq!(
                at(image, "/registryId"),
                json!(REGISTRY_ID),
                "case {case:?}: expected this registry, got {:?}",
                image["registryId"]
            );
            assert_eq!(
                at(image, "/repositoryName"),
                json!(repository),
                "case {case:?}: expected the repository {repository:?} named, got {:?}",
                image["repositoryName"]
            );
        }
        Expected::Failure(code) => {
            assert!(
                images.is_empty(),
                "case {case:?}: expected no images, got {images:?}"
            );
            assert_eq!(
                failures.len(),
                1,
                "case {case:?}: expected exactly one failure, got {failures:?}"
            );
            let failure = &failures[0];
            assert_eq!(
                at(failure, "/failureCode"),
                json!(code),
                "case {case:?}: expected failureCode {code}, got {failure:?}"
            );
            assert_eq!(
                at(failure, "/imageId"),
                requested.clone(),
                "case {case:?}: expected the requested identifier echoed, got {failure:?}"
            );
            assert!(
                at(failure, "/failureReason")
                    .as_str()
                    .is_some_and(|reason| !reason.is_empty()),
                "case {case:?}: expected a failureReason, got {failure:?}"
            );
        }
    }
}

/// Each `imageIds` entry resolves against what was pushed — by digest, by tag, or by
/// both — and an entry that resolves to nothing is reported with the code that says
/// why, never as an error.
///
/// The rows differ along which parts the identifier carries, whether each part names a
/// stored image, whether the two parts agree, and which manifest the push stored.
#[tokio::test]
async fn every_identifier_is_answered_from_what_the_push_stored() {
    let router = seeded("resolution").await;
    let docker_digest = manifest_digest(MANIFEST_DOCKER_V2);
    let oci_digest = manifest_digest(MANIFEST_OCI);
    let untyped_digest = manifest_digest(MANIFEST_UNTYPED);
    let absent_digest = manifest_digest(MANIFEST_NEVER_PUSHED);
    let docker_v2_media_type = Some("application/vnd.docker.distribution.manifest.v2+json");

    let cases: Vec<(&str, Value, Expected)> = vec![
        (
            "by a tag that points at the manifest",
            json!({"imageTag": "v1"}),
            Expected::Image {
                manifest: MANIFEST_DOCKER_V2,
                media_type: docker_v2_media_type,
                tag: Some("v1"),
            },
        ),
        (
            "by the second tag on the same manifest",
            json!({"imageTag": "latest"}),
            Expected::Image {
                manifest: MANIFEST_DOCKER_V2,
                media_type: docker_v2_media_type,
                tag: Some("latest"),
            },
        ),
        (
            "by digest alone, reported with no tag",
            json!({"imageDigest": docker_digest.clone()}),
            Expected::Image {
                manifest: MANIFEST_DOCKER_V2,
                media_type: docker_v2_media_type,
                tag: None,
            },
        ),
        (
            "by a digest and a tag that agree",
            json!({"imageDigest": docker_digest.clone(), "imageTag": "latest"}),
            Expected::Image {
                manifest: MANIFEST_DOCKER_V2,
                media_type: docker_v2_media_type,
                tag: Some("latest"),
            },
        ),
        (
            "an OCI manifest keeps its own media type",
            json!({"imageTag": "oci"}),
            Expected::Image {
                manifest: MANIFEST_OCI,
                media_type: Some("application/vnd.oci.image.manifest.v1+json"),
                tag: Some("oci"),
            },
        ),
        (
            "an untagged manifest that declares no media type",
            json!({"imageDigest": untyped_digest.clone()}),
            Expected::Image {
                manifest: MANIFEST_UNTYPED,
                media_type: None,
                tag: None,
            },
        ),
        (
            "a tag nothing in the repository carries",
            json!({"imageTag": "no-such-tag"}),
            Expected::Failure("ImageNotFound"),
        ),
        (
            "a digest nothing was pushed under",
            json!({"imageDigest": absent_digest.clone()}),
            Expected::Failure("ImageNotFound"),
        ),
        (
            "a stored digest with a tag nothing carries",
            json!({"imageDigest": docker_digest.clone(), "imageTag": "no-such-tag"}),
            Expected::Failure("ImageNotFound"),
        ),
        (
            "a digest nothing was pushed under, with a tag that does exist",
            json!({"imageDigest": absent_digest.clone(), "imageTag": "v1"}),
            Expected::Failure("ImageNotFound"),
        ),
        (
            "a stored digest with a tag that is on another image",
            json!({"imageDigest": docker_digest.clone(), "imageTag": "oci"}),
            Expected::Failure("ImageTagDoesNotMatchDigest"),
        ),
        (
            "another stored digest with a tag that is on a different image",
            json!({"imageDigest": oci_digest.clone(), "imageTag": "v1"}),
            Expected::Failure("ImageTagDoesNotMatchDigest"),
        ),
        (
            "a digest that is not a content digest at all",
            json!({"imageDigest": "not-a-digest"}),
            Expected::Failure("InvalidImageDigest"),
        ),
        (
            "a digest with a hex part that is not hex",
            json!({"imageDigest": "sha256:zzzz"}),
            Expected::Failure("InvalidImageDigest"),
        ),
        (
            "a malformed digest beside a tag that does exist",
            json!({"imageDigest": "sha256:", "imageTag": "v1"}),
            Expected::Failure("InvalidImageDigest"),
        ),
        (
            "an entry carrying neither a digest nor a tag",
            json!({}),
            Expected::Failure("MissingDigestAndTag"),
        ),
    ];

    for (case, image_id, expected) in &cases {
        let answer = ok(
            &router,
            case,
            "BatchGetImage",
            &json!({"repositoryName": "resolution", "imageIds": [image_id.clone()]}),
        )
        .await;
        assert_single(case, "resolution", image_id, &answer, expected);
    }
}

/// One call reports on every entry it was handed, in request order: the hits land in
/// `images` and the misses in `failures`, and two entries naming one image are two
/// answers.
#[tokio::test]
async fn a_batch_reports_each_entry_independently() {
    let router = seeded("batching").await;
    let docker_digest = manifest_digest(MANIFEST_DOCKER_V2);
    let absent_digest = manifest_digest(MANIFEST_NEVER_PUSHED);

    // Rows: how many entries, and how many of them resolve.
    let cases: Vec<BatchCase<'_>> = vec![
        (
            "one hit",
            vec![json!({"imageTag": "v1"})],
            vec![MANIFEST_DOCKER_V2],
            vec![],
        ),
        (
            "one miss",
            vec![json!({"imageTag": "ghost"})],
            vec![],
            vec!["ImageNotFound"],
        ),
        (
            "a hit after a miss, and the same image twice",
            vec![
                json!({"imageTag": "ghost"}),
                json!({"imageTag": "oci"}),
                json!({"imageDigest": docker_digest.clone()}),
                json!({"imageTag": "v1"}),
                json!({}),
            ],
            vec![MANIFEST_OCI, MANIFEST_DOCKER_V2, MANIFEST_DOCKER_V2],
            vec!["ImageNotFound", "MissingDigestAndTag"],
        ),
        (
            "every entry missing",
            vec![
                json!({"imageDigest": absent_digest.clone()}),
                json!({"imageTag": "ghost"}),
                json!({"imageDigest": docker_digest.clone(), "imageTag": "oci"}),
            ],
            vec![],
            vec![
                "ImageNotFound",
                "ImageNotFound",
                "ImageTagDoesNotMatchDigest",
            ],
        ),
    ];

    for (case, image_ids, manifests, codes) in &cases {
        let answer = ok(
            &router,
            case,
            "BatchGetImage",
            &json!({"repositoryName": "batching", "imageIds": image_ids.clone()}),
        )
        .await;
        let reported: Vec<String> = answer["images"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|image| image["imageManifest"].as_str().unwrap_or_default().to_string())
            .collect();
        let reported_codes: Vec<String> = answer["failures"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|failure| failure["failureCode"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            reported,
            manifests
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<String>>(),
            "case {case:?}: expected these manifests in request order, got {reported:?}"
        );
        assert_eq!(
            reported_codes,
            codes
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<String>>(),
            "case {case:?}: expected these failure codes in request order, got {reported_codes:?}"
        );
    }
}

/// A repository that holds no image is not a failed call: every entry is reported as a
/// failure and the answer is still 200. The call is a read, so nothing it does changes
/// what a later caller sees.
#[tokio::test]
async fn an_empty_repository_reports_each_entry_as_a_failure() {
    let router = emulator();
    given_repository(&router, "empty", json!({})).await;

    let request = json!({"repositoryName": "empty", "imageIds": [{"imageTag": "v1"}]});
    let first = ok(&router, "empty repository", "BatchGetImage", &request).await;
    assert_single(
        "empty repository",
        "empty",
        &json!({"imageTag": "v1"}),
        &first,
        &Expected::Failure("ImageNotFound"),
    );

    // Reading twice answers the same thing, and the repository still holds nothing: a
    // batch get neither creates nor consumes anything.
    let second = ok(&router, "read twice", "BatchGetImage", &request).await;
    assert_eq!(
        second, first,
        "case \"read twice\": a read must answer the same twice, got {second:?} after {first:?}"
    );
    let listed = ok(
        &router,
        "still empty",
        "ListImages",
        &json!({"repositoryName": "empty"}),
    )
    .await;
    assert_eq!(
        at(&listed, "/imageIds"),
        json!([]),
        "case \"still empty\": a batch get must store no image, got {listed:?}"
    );
}

/// `acceptedMediaTypes` says which manifest schemas the client understands. This
/// registry stores every manifest as it was pushed and converts nothing, so the list
/// never changes which identifier resolves or what is reported for it.
#[tokio::test]
async fn accepted_media_types_do_not_change_the_answer() {
    let router = seeded("media-types").await;

    // The media types the member documents, plus a list naming none of the stored
    // manifests' own type, and a request that sends no list at all.
    // (`smithy_query(query="operation", operation="BatchGetImage")`: acceptedMediaTypes
    // is MediaTypeList, len 1..100; its doc lists the three Valid values below.)
    let cases: Vec<(&str, Value)> = vec![
        ("no list at all", Value::Null),
        (
            "the docker v2 type",
            json!(["application/vnd.docker.distribution.manifest.v2+json"]),
        ),
        (
            "the oci type",
            json!(["application/vnd.oci.image.manifest.v1+json"]),
        ),
        (
            "every documented type",
            json!([
                "application/vnd.docker.distribution.manifest.v1+json",
                "application/vnd.docker.distribution.manifest.v2+json",
                "application/vnd.oci.image.manifest.v1+json"
            ]),
        ),
        (
            "a type no stored manifest declares",
            json!(["application/vnd.docker.distribution.manifest.v1+json"]),
        ),
    ];

    for (case, accepted) in &cases {
        let mut request = json!({"repositoryName": "media-types", "imageIds": [{"imageTag": "v1"}]});
        if !accepted.is_null() {
            request["acceptedMediaTypes"] = accepted.clone();
        }
        let answer = ok(&router, case, "BatchGetImage", &request).await;
        assert_single(
            case,
            "media-types",
            &json!({"imageTag": "v1"}),
            &answer,
            &Expected::Image {
                manifest: MANIFEST_DOCKER_V2,
                media_type: Some("application/vnd.docker.distribution.manifest.v2+json"),
                tag: Some("v1"),
            },
        );
    }
}

/// The identifier resolution is one rule, shared with `DescribeImages`: the difference
/// is only in how an unresolvable entry is reported. `DescribeImages` refuses the whole
/// call, and its message names the requested identifier with both members spelled out —
/// an absent one as `null` — so the rows differ along which members the entry carries.
#[tokio::test]
async fn describe_images_refuses_the_same_identifiers_and_names_them() {
    let router = seeded("shared-resolution").await;
    let absent_digest = manifest_digest(MANIFEST_NEVER_PUSHED);
    let docker_digest = manifest_digest(MANIFEST_DOCKER_V2);

    let cases: Vec<(&str, Value, String)> = vec![
        (
            "a tag nothing carries",
            json!({"imageTag": "ghost"}),
            "imageId {imageDigest:'null', imageTag:'ghost'}".to_string(),
        ),
        (
            "a digest nothing was pushed under",
            json!({"imageDigest": absent_digest.clone()}),
            format!("imageId {{imageDigest:'{absent_digest}', imageTag:'null'}}"),
        ),
        (
            "a stored digest with a tag on another image",
            json!({"imageDigest": docker_digest.clone(), "imageTag": "oci"}),
            format!("imageId {{imageDigest:'{docker_digest}', imageTag:'oci'}}"),
        ),
    ];

    for (case, image_id, named) in &cases {
        let answer = call(
            &router,
            "DescribeImages",
            &json!({"repositoryName": "shared-resolution", "imageIds": [image_id.clone()]}),
        )
        .await;
        assert_eq!(
            answer.error_code(),
            "ImageNotFoundException",
            "case {case:?}: expected ImageNotFoundException, got {:?}",
            answer.body
        );
        let message = answer.body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains(named.as_str()),
            "case {case:?}: expected a message naming {named:?}, got {message:?}"
        );
        assert!(
            message.contains("shared-resolution") && message.contains(REGISTRY_ID),
            "case {case:?}: expected the repository and registry named, got {message:?}"
        );
    }

    // A digest that is not a content digest addresses nothing, so the same resolver
    // reports it as a bad parameter rather than as an absent image.
    for (case, image_id) in [
        ("not a digest at all", json!({"imageDigest": "not-a-digest"})),
        ("a hex part that is not hex", json!({"imageDigest": "sha256:zz"})),
    ] {
        let answer = call(
            &router,
            "DescribeImages",
            &json!({"repositoryName": "shared-resolution", "imageIds": [image_id.clone()]}),
        )
        .await;
        assert_eq!(
            answer.error_code(),
            "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {:?}",
            answer.body
        );
    }
}

/// What makes the whole call fail: a repository this registry does not hold, and a
/// request naming another registry. Neither is reported as a per-entry failure.
#[tokio::test]
async fn the_whole_call_fails_only_on_the_repository_or_the_registry() {
    let router = seeded("whole-call").await;

    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a repository that was never created",
            json!({"repositoryName": "no-such-repository", "imageIds": [{"imageTag": "v1"}]}),
            "RepositoryNotFoundException",
        ),
        (
            "a repository that was never created, with an empty identifier",
            json!({"repositoryName": "no-such-repository", "imageIds": [{}]}),
            "RepositoryNotFoundException",
        ),
        (
            "another account's registry",
            json!({
                "registryId": "123456789012",
                "repositoryName": "whole-call",
                "imageIds": [{"imageTag": "v1"}]
            }),
            "InvalidParameterException",
        ),
    ];

    for (case, request, expected) in &cases {
        let answer = call(&router, "BatchGetImage", request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            *expected,
            "case {case:?}: expected {expected}, got {:?}",
            answer.body
        );
    }

    // This registry's own id is accepted, and answers from the same records.
    let answer = ok(
        &router,
        "this registry named explicitly",
        "BatchGetImage",
        &json!({
            "registryId": REGISTRY_ID,
            "repositoryName": "whole-call",
            "imageIds": [{"imageTag": "v1"}]
        }),
    )
    .await;
    assert_single(
        "this registry named explicitly",
        "whole-call",
        &json!({"imageTag": "v1"}),
        &answer,
        &Expected::Image {
            manifest: MANIFEST_DOCKER_V2,
            media_type: Some("application/vnd.docker.distribution.manifest.v2+json"),
            tag: Some("v1"),
        },
    );
}
