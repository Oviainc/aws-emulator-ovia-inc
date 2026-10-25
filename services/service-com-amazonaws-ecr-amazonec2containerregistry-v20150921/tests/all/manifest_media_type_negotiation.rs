// SPDX-License-Identifier: CC0-1.0

//! `BatchGetImage`'s manifest media-type negotiation, end to end over the wire.
//!
//! The rule under test: `acceptedMediaTypes` names the manifest schemas the client
//! understands, so the registry serves each image in one of them — the stored manifest when
//! the client accepts its media type, and otherwise the same manifest rendered in an
//! accepted schema, with `imageManifestMediaType` reporting the schema actually served. The
//! Docker image manifest V2 schema 2 and the OCI image manifest describe an image
//! identically and differ only in the media types they spell its parts with, so rendering
//! one in the other re-spells those names and changes no digest, size or layer order. A
//! document nothing can render in an accepted schema is served as stored, under the media
//! type it really has.
//!
//! The wire harness is `crate::wire`'s: every case goes through the router, so it exercises
//! routing, deserialization, the handlers, the persisted state and serialization.

use serde_json::{json, Value};
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921 as ecr;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{at, emulator, given_layer, ok};

use ecr::state;

/// The media types `acceptedMediaTypes` documents: "Valid values:
/// application/vnd.docker.distribution.manifest.v1+json |
/// application/vnd.docker.distribution.manifest.v2+json |
/// application/vnd.oci.image.manifest.v1+json"
/// (`smithy_query(query="doc", shape="BatchGetImageRequest", member="acceptedMediaTypes")`).
const DOCKER_V1_MANIFEST: &str = "application/vnd.docker.distribution.manifest.v1+json";
/// See [`DOCKER_V1_MANIFEST`].
const DOCKER_V2_MANIFEST: &str = "application/vnd.docker.distribution.manifest.v2+json";
/// See [`DOCKER_V1_MANIFEST`].
const OCI_MANIFEST: &str = "application/vnd.oci.image.manifest.v1+json";

/// A Docker manifest LIST, which is not an image manifest at all: it references other
/// manifests, so no image-manifest schema renders it (Docker Registry HTTP API V2).
const DOCKER_V2_MANIFEST_LIST: &str = "application/vnd.docker.distribution.manifest.list.v2+json";

/// The media types the two interchangeable schemas spell the same part of an image with, as
/// `(Docker, OCI)` pairs: the image configuration blob and the gzipped filesystem layer.
/// From the Docker image manifest V2 schema 2 and the OCI image-spec media types.
const CONFIG_MEDIA_TYPES: (&str, &str) = (
    "application/vnd.docker.container.image.v1+json",
    "application/vnd.oci.image.config.v1+json",
);
/// See [`CONFIG_MEDIA_TYPES`].
const LAYER_MEDIA_TYPES: (&str, &str) = (
    "application/vnd.docker.image.rootfs.diff.tar.gzip",
    "application/vnd.oci.image.layer.v1.tar+gzip",
);

/// One row of the negotiation table: what the row is about, the stored image it asks for,
/// what the request accepts, the document that must be served and the media type reported
/// for it.
type NegotiationRow<'a> = (
    &'a str,
    &'a String,
    Option<Vec<&'a str>>,
    &'a Value,
    Option<&'a str>,
);

/// The image this suite negotiates over, as the two interchangeable schemas each spell it:
/// the same configuration blob, the same two layers in the same order, the same sizes.
struct Image {
    /// The manifest as the Docker image manifest V2 schema 2 spells it.
    docker: Value,
    /// The same image as the OCI image manifest spells it.
    oci: Value,
}

/// Upload the blobs of one image into `repository` and build its manifest in both schemas.
async fn given_image_blobs(router: &MultiServiceRouter, case: &str, repository: &str) -> Image {
    let config_bytes = format!("{repository}-config").into_bytes();
    let config = given_layer(router, case, repository, &config_bytes).await;
    let mut layers: Vec<(String, usize)> = Vec::new();
    for index in 0..2 {
        let bytes = format!("{repository}-layer-{index}").into_bytes();
        let digest = given_layer(router, case, repository, &bytes).await;
        layers.push((digest, bytes.len()));
    }

    let manifest = |manifest_type: &str, config_type: &str, layer_type: &str| -> Value {
        json!({
            "schemaVersion": 2,
            "mediaType": manifest_type,
            "config": {
                "mediaType": config_type,
                "digest": config,
                "size": config_bytes.len(),
            },
            "layers": layers
                .iter()
                .map(|(digest, size)| json!({
                    "mediaType": layer_type,
                    "digest": digest,
                    "size": size,
                }))
                .collect::<Vec<Value>>(),
        })
    };

    Image {
        docker: manifest(DOCKER_V2_MANIFEST, CONFIG_MEDIA_TYPES.0, LAYER_MEDIA_TYPES.0),
        oci: manifest(OCI_MANIFEST, CONFIG_MEDIA_TYPES.1, LAYER_MEDIA_TYPES.1),
    }
}

/// Push `manifest` into `repository` under `tag`, answering the digest the registry
/// addresses it by.
async fn given_pushed(
    router: &MultiServiceRouter,
    case: &str,
    repository: &str,
    manifest: &Value,
    tag: &str,
) -> String {
    let document = manifest.to_string();
    ok(
        router,
        case,
        "PutImage",
        &json!({
            "repositoryName": repository,
            "imageManifest": document,
            "imageTag": tag,
        }),
    )
    .await;
    state::manifest_digest(&document)
}

/// Ask for one image, answering the single `images` entry.
async fn got(
    router: &MultiServiceRouter,
    case: &str,
    repository: &str,
    digest: &str,
    accepted: Option<Vec<&str>>,
) -> Value {
    let mut request = json!({
        "repositoryName": repository,
        "imageIds": [{"imageDigest": digest}],
    });
    if let Some(accepted) = accepted {
        request["acceptedMediaTypes"] = json!(accepted);
    }
    let answer = ok(router, case, "BatchGetImage", &request).await;
    assert_eq!(
        at(&answer, "/failures"),
        json!([]),
        "case {case:?}: a stored image is never a failure, got {answer:?}"
    );
    at(&answer, "/images/0")
}

/// Each image is served in a schema the request accepts, and serving it in the other schema
/// changes nothing but the media-type names.
///
/// The rows differ along what the rule generalises over: which schema the image was pushed
/// in, which schemas the client accepts (none named, the stored one among several, only the
/// other one, only a schema nothing renders), and what the stored document is (an image
/// manifest, a manifest list, a document stored under no media type). The report's own case
/// is the OCI manifest asked for as Docker v2.
#[tokio::test]
async fn an_image_is_served_in_a_schema_the_request_accepts() {
    let router = emulator();
    let repository = "negotiation";
    let case = "seed";
    ok(
        &router,
        case,
        "CreateRepository",
        &json!({"repositoryName": repository}),
    )
    .await;

    let image = given_image_blobs(&router, case, repository).await;
    let docker_digest = given_pushed(&router, case, repository, &image.docker, "docker").await;
    let oci_digest = given_pushed(&router, case, repository, &image.oci, "oci").await;

    // A manifest list references other manifests rather than blobs, and a schema 1
    // document carries different members, so neither is renderable as an image manifest.
    let list = json!({
        "schemaVersion": 2,
        "mediaType": DOCKER_V2_MANIFEST_LIST,
        "manifests": [{
            "mediaType": DOCKER_V2_MANIFEST,
            "digest": docker_digest,
            "size": image.docker.to_string().len(),
        }],
    });
    let list_digest = given_pushed(&router, case, repository, &list, "list").await;
    let untyped = json!({"schemaVersion": 1, "name": repository});
    let untyped_digest = given_pushed(&router, case, repository, &untyped, "untyped").await;

    // (case, the stored image, what the request accepts, the document served, the media
    // type reported for it)
    let rows: Vec<NegotiationRow<'_>> = vec![
        (
            "no accepted list at all",
            &oci_digest,
            None,
            &image.oci,
            Some(OCI_MANIFEST),
        ),
        (
            "the stored schema among the accepted ones",
            &oci_digest,
            Some(vec![DOCKER_V2_MANIFEST, OCI_MANIFEST]),
            &image.oci,
            Some(OCI_MANIFEST),
        ),
        (
            "an OCI manifest for a client that accepts only docker v2",
            &oci_digest,
            Some(vec![DOCKER_V2_MANIFEST]),
            &image.docker,
            Some(DOCKER_V2_MANIFEST),
        ),
        (
            "a docker v2 manifest for a client that accepts only OCI",
            &docker_digest,
            Some(vec![OCI_MANIFEST]),
            &image.oci,
            Some(OCI_MANIFEST),
        ),
        (
            "a docker v2 manifest for a client that accepts only schema 1",
            &docker_digest,
            Some(vec![DOCKER_V1_MANIFEST]),
            &image.docker,
            Some(DOCKER_V2_MANIFEST),
        ),
        (
            "a manifest list, which no image-manifest schema renders",
            &list_digest,
            Some(vec![DOCKER_V2_MANIFEST, OCI_MANIFEST]),
            &list,
            Some(DOCKER_V2_MANIFEST_LIST),
        ),
        (
            "a document stored under no media type",
            &untyped_digest,
            Some(vec![DOCKER_V2_MANIFEST]),
            &untyped,
            None,
        ),
    ];

    for (case, digest, accepted, served, media_type) in rows {
        let entry = got(&router, case, repository, digest, accepted).await;
        assert_eq!(
            at(&entry, "/imageManifestMediaType"),
            media_type.map_or(Value::Null, |value| json!(value)),
            "case {case:?}: expected the media type {media_type:?} reported, got {:?}",
            at(&entry, "/imageManifestMediaType")
        );
        let document: Value = serde_json::from_str(
            at(&entry, "/imageManifest")
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a manifest, got {entry:?}")),
        )
        .unwrap_or_else(|error| {
            panic!("case {case:?}: the served manifest must be JSON ({error}): {entry:?}")
        });
        assert_eq!(
            document, *served,
            "case {case:?}: expected the image served as {served:?}, got {document:?}"
        );
        // The identity of the image in the repository is the digest of what was PUSHED,
        // whichever schema the document is served in.
        assert_eq!(
            at(&entry, "/imageId/imageDigest"),
            json!(digest),
            "case {case:?}: expected the stored image's own digest, got {:?}",
            at(&entry, "/imageId")
        );
        // The advertised size is the length of the document this answer carries.
        let length = at(&entry, "/imageManifest")
            .as_str()
            .unwrap_or_default()
            .len();
        assert_eq!(
            at(&entry, "/imageManifestSize"),
            json!(length),
            "case {case:?}: expected the served document's own length {length}, got {:?}",
            at(&entry, "/imageManifestSize")
        );
    }
}

/// Negotiation is per entry and changes nothing in the store: one call asking for both
/// images in one schema serves both in it, and the stored manifests are what a later read
/// still reports.
#[tokio::test]
async fn negotiation_is_per_entry_and_stores_nothing() {
    let router = emulator();
    let repository = "per-entry";
    let case = "both images as docker v2";
    ok(
        &router,
        case,
        "CreateRepository",
        &json!({"repositoryName": repository}),
    )
    .await;
    let image = given_image_blobs(&router, case, repository).await;
    let docker_digest = given_pushed(&router, case, repository, &image.docker, "docker").await;
    let oci_digest = given_pushed(&router, case, repository, &image.oci, "oci").await;

    let answer = ok(
        &router,
        case,
        "BatchGetImage",
        &json!({
            "repositoryName": repository,
            "imageIds": [{"imageDigest": oci_digest}, {"imageDigest": docker_digest}],
            "acceptedMediaTypes": [DOCKER_V2_MANIFEST],
        }),
    )
    .await;
    for index in 0..2 {
        assert_eq!(
            at(&answer, &format!("/images/{index}/imageManifestMediaType")),
            json!(DOCKER_V2_MANIFEST),
            "case {case:?}: entry {index} must be served as docker v2, got {answer:?}"
        );
    }

    // What the push stored is untouched: a read that accepts the stored schema gets the
    // document as pushed, and the image is still addressed by that document's digest.
    let entry = got(
        &router,
        "the stored manifest is unchanged",
        repository,
        &oci_digest,
        Some(vec![OCI_MANIFEST]),
    )
    .await;
    assert_eq!(
        at(&entry, "/imageManifest"),
        json!(image.oci.to_string()),
        "case \"the stored manifest is unchanged\": expected the pushed document, got {:?}",
        at(&entry, "/imageManifest")
    );
}
