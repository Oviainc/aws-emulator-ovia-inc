// SPDX-License-Identifier: CC0-1.0

//! The image metadata the read operations report, end to end over the wire.
//!
//! Three rules are under test, each over a table of inputs:
//!
//! 1. An `ImageDetail`'s `artifactMediaType` is the media type the stored manifest's
//!    `config` descriptor declares, and its `imageSizeInBytes` is the total of the sizes
//!    of the DISTINCT blobs that manifest references — its layers and its image
//!    configuration, a blob named twice counted once — the max of its children's totals
//!    for a manifest list, and never the length of the manifest document.
//! 2. An `Image`'s `imageManifestSize` is the byte length of the manifest that same
//!    answer carries.
//! 3. An image's storage class is per-image state: `UpdateImageStorageClass` moves the
//!    named image between Amazon ECR standard and Amazon ECR archive, stamps
//!    `lastArchivedAt` / `lastActivatedAt` with the instant of the transition, and leaves
//!    every other image in the repository where it was; a restore reports `ACTIVATING`
//!    until the completion the clock owes falls due.
//!
//! The wire plumbing is the crate's shared harness (`tests/all/wire.rs`); the repository
//! seeding helper is the lifecycle suite's.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::repository_image_lifecycle::{given_repository, manifest_digest};
use crate::wire::{at, emulator, given_layer, ok, refused};

/// The media type a Docker image manifest V2 schema 2 declares for its configuration
/// blob — the artifact media type of a container image
/// (`ImageDetail$artifactMediaType`: "The artifact media type of the image").
const DOCKER_CONFIG_MEDIA_TYPE: &str = "application/vnd.docker.container.image.v1+json";

/// The media type the OCI image-spec gives an image configuration blob.
const OCI_CONFIG_MEDIA_TYPE: &str = "application/vnd.oci.image.config.v1+json";

/// A non-image OCI artifact's config media type: an SBOM, which is exactly what
/// `artifactMediaType` lets a client tell apart from a container image without fetching
/// the manifest.
const SBOM_CONFIG_MEDIA_TYPE: &str = "application/spdx+json";

/// The media type of a compressed filesystem layer.
const LAYER_MEDIA_TYPE: &str = "application/vnd.oci.image.layer.v1.tar+gzip";

/// A Docker image manifest V2 schema 2's own media type.
const DOCKER_MANIFEST_MEDIA_TYPE: &str = "application/vnd.docker.distribution.manifest.v2+json";

/// An OCI image manifest's own media type.
const OCI_MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";

/// Long enough past [`state::IMAGE_RESTORE_DELAY`] that the activation the clock owes is
/// certainly due. Derived from the span itself, never a bare number.
const PAST_THE_RESTORE: Duration =
    state::IMAGE_RESTORE_DELAY.saturating_add(Duration::from_millis(150));

/// One blob descriptor a seeded manifest carries: the NAME of the blob it references (two
/// descriptors naming the same blob reference ONE blob in the repository), the blob's byte
/// length, and whether the descriptor declares that length.
type SeededBlob<'a> = (&'a str, usize, bool);

/// The `config` descriptor a seeded manifest carries.
#[derive(Debug, Clone)]
enum SeededConfig<'a> {
    /// No `config` descriptor at all.
    Absent,
    /// A config descriptor naming a blob, with the media type it declares.
    Present(&'a str, SeededBlob<'a>),
}

/// One row of the artifact/size table: a manifest to seed, and what the stored image must
/// then report.
struct ManifestSpec<'a> {
    /// What the row is about.
    case: &'a str,
    /// Makes every blob this row uploads unique in the repository, so two rows never
    /// collide on a digest.
    salt: &'a str,
    /// The manifest's own `mediaType`.
    manifest_media_type: &'a str,
    /// Its `config` descriptor.
    config: SeededConfig<'a>,
    /// Its `layers` descriptors, in manifest order.
    layers: Vec<SeededBlob<'a>>,
    /// The `artifactMediaType` the stored image must report.
    artifact_media_type: Option<&'a str>,
    /// The `imageSizeInBytes` the stored image must report.
    size: Option<i64>,
}

/// A blob of exactly `len` bytes that no other blob of this suite equals: `name` is
/// written at the front and the rest padded, so a row controls the byte count its
/// descriptor declares while still owning its own digest.
fn blob_of(name: &str, len: usize) -> Vec<u8> {
    assert!(
        name.len() <= len,
        "case {name:?}: a {len}-byte blob cannot carry its own name"
    );
    let mut content = name.as_bytes().to_vec();
    content.resize(len, b'.');
    content
}

/// Seed `spec`'s manifest into `repository`: upload each distinct blob it references
/// through the documented layer-upload sequence, push the manifest, and answer its digest.
///
/// A manifest cannot be pushed before every blob it references is in the repository
/// (`PutImage`'s `LayersNotFoundException`), which is why the blobs go in first.
async fn seed_image(
    router: &MultiServiceRouter,
    repository: &str,
    spec: &ManifestSpec<'_>,
    tag: Option<&str>,
) -> String {
    let mut referenced: Vec<SeededBlob<'_>> = spec.layers.clone();
    if let SeededConfig::Present(_, blob) = spec.config {
        referenced.push(blob);
    }
    let mut digests: BTreeMap<&str, String> = BTreeMap::new();
    for (name, len, _) in referenced {
        if digests.contains_key(name) {
            continue;
        }
        let content = blob_of(&format!("{}-{name}", spec.salt), len);
        let digest = given_layer(router, spec.case, repository, &content).await;
        digests.insert(name, digest);
    }

    let descriptor = |media_type: &str, (name, len, declares): SeededBlob<'_>| -> Value {
        let mut descriptor = json!({"mediaType": media_type, "digest": digests[name]});
        if declares {
            descriptor["size"] = json!(len);
        }
        descriptor
    };

    let mut document = json!({"schemaVersion": 2, "mediaType": spec.manifest_media_type});
    if let SeededConfig::Present(media_type, blob) = spec.config {
        document["config"] = descriptor(media_type, blob);
    }
    document["layers"] = Value::Array(
        spec.layers
            .iter()
            .map(|blob| descriptor(LAYER_MEDIA_TYPE, *blob))
            .collect(),
    );
    let manifest = document.to_string();

    let mut request = json!({"repositoryName": repository, "imageManifest": manifest});
    if let Some(tag) = tag {
        request["imageTag"] = json!(tag);
    }
    ok(router, spec.case, "PutImage", &request).await;
    manifest_digest(&manifest)
}

/// One image of a repository that a case only needs to exist: a tagged OCI image with its
/// own configuration blob and one layer.
fn plain_image<'a>(case: &'a str, salt: &'a str) -> ManifestSpec<'a> {
    ManifestSpec {
        case,
        salt,
        manifest_media_type: OCI_MANIFEST_MEDIA_TYPE,
        config: SeededConfig::Present(OCI_CONFIG_MEDIA_TYPE, ("cfg", 24, true)),
        layers: vec![("layer", 32, true)],
        artifact_media_type: Some(OCI_CONFIG_MEDIA_TYPE),
        size: Some(56),
    }
}

/// The `ImageDetail` `DescribeImages` reports for one digest, asking for images of any
/// status so the row under test is read whatever storage class it is in.
async fn detail_of(
    router: &MultiServiceRouter,
    case: &str,
    repository: &str,
    digest: &str,
) -> Value {
    let described = ok(
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
    let details = described["imageDetails"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        details.len(),
        1,
        "case {case:?}: expected exactly one imageDetail for {digest}, got {described:?}"
    );
    details[0].clone()
}

/// `artifactMediaType` comes from the manifest's `config` descriptor, and
/// `imageSizeInBytes` is the total of the distinct blobs the manifest references.
///
/// The rows differ along what the rule generalises over: how many layers the manifest
/// references (none, one, several), how big they are, whether the configuration blob is
/// its own blob or the very blob a layer already names, which artifact the config
/// descriptor names (a Docker image configuration, an OCI image configuration, an SBOM),
/// whether there is a config descriptor at all, and whether every descriptor declares a
/// usable size.
#[tokio::test]
async fn an_image_detail_reports_the_manifest_s_artifact_type_and_its_blob_total() {
    let router = emulator();
    let repository = "artifacts";
    given_repository(&router, repository, json!({})).await;

    let rows = vec![
        ManifestSpec {
            case: "a docker v2 image: one layer beside its own configuration blob",
            salt: "r1",
            manifest_media_type: DOCKER_MANIFEST_MEDIA_TYPE,
            config: SeededConfig::Present(DOCKER_CONFIG_MEDIA_TYPE, ("cfg", 64, true)),
            layers: vec![("layer", 128, true)],
            artifact_media_type: Some(DOCKER_CONFIG_MEDIA_TYPE),
            size: Some(192),
        },
        ManifestSpec {
            case: "an oci image with three layers of different sizes",
            salt: "r2",
            manifest_media_type: OCI_MANIFEST_MEDIA_TYPE,
            config: SeededConfig::Present(OCI_CONFIG_MEDIA_TYPE, ("cfg", 32, true)),
            layers: vec![("one", 8, true), ("two", 16, true), ("three", 64, true)],
            artifact_media_type: Some(OCI_CONFIG_MEDIA_TYPE),
            size: Some(120),
        },
        ManifestSpec {
            // One blob, named twice: the repository holds it once, so it is counted once.
            case: "a configuration blob that is also the image's only layer",
            salt: "r3",
            manifest_media_type: DOCKER_MANIFEST_MEDIA_TYPE,
            config: SeededConfig::Present(DOCKER_CONFIG_MEDIA_TYPE, ("both", 256, true)),
            layers: vec![("both", 256, true)],
            artifact_media_type: Some(DOCKER_CONFIG_MEDIA_TYPE),
            size: Some(256),
        },
        ManifestSpec {
            case: "the same layer referenced twice by one manifest",
            salt: "r4",
            manifest_media_type: OCI_MANIFEST_MEDIA_TYPE,
            config: SeededConfig::Present(OCI_CONFIG_MEDIA_TYPE, ("cfg", 20, true)),
            layers: vec![("layer", 30, true), ("layer", 30, true)],
            artifact_media_type: Some(OCI_CONFIG_MEDIA_TYPE),
            size: Some(50),
        },
        ManifestSpec {
            case: "an sbom artifact, which is not a container image",
            salt: "r5",
            manifest_media_type: OCI_MANIFEST_MEDIA_TYPE,
            config: SeededConfig::Present(SBOM_CONFIG_MEDIA_TYPE, ("cfg", 40, true)),
            layers: vec![("doc", 26, true)],
            artifact_media_type: Some(SBOM_CONFIG_MEDIA_TYPE),
            size: Some(66),
        },
        ManifestSpec {
            // An EMPTY `layers` array is a statement, not a gap: the image's content is
            // its configuration blob and nothing else.
            case: "an image with no layers at all, just its configuration",
            salt: "r6",
            manifest_media_type: OCI_MANIFEST_MEDIA_TYPE,
            config: SeededConfig::Present(OCI_CONFIG_MEDIA_TYPE, ("cfg", 70, true)),
            layers: vec![],
            artifact_media_type: Some(OCI_CONFIG_MEDIA_TYPE),
            size: Some(70),
        },
        ManifestSpec {
            case: "a manifest carrying no config descriptor, so no artifact type",
            salt: "r7",
            manifest_media_type: OCI_MANIFEST_MEDIA_TYPE,
            config: SeededConfig::Absent,
            layers: vec![("layer", 16, true)],
            artifact_media_type: None,
            size: Some(16),
        },
        ManifestSpec {
            // What the descriptor does not state, the registry's own count of the blob
            // it holds does.
            case: "a layer descriptor that declares no size",
            salt: "r8",
            manifest_media_type: OCI_MANIFEST_MEDIA_TYPE,
            config: SeededConfig::Present(OCI_CONFIG_MEDIA_TYPE, ("cfg", 24, true)),
            layers: vec![("layer", 10, false)],
            artifact_media_type: Some(OCI_CONFIG_MEDIA_TYPE),
            size: Some(34),
        },
        ManifestSpec {
            case: "a config descriptor that declares no size",
            salt: "r9",
            manifest_media_type: OCI_MANIFEST_MEDIA_TYPE,
            config: SeededConfig::Present(OCI_CONFIG_MEDIA_TYPE, ("cfg", 24, false)),
            layers: vec![("layer", 10, true)],
            artifact_media_type: Some(OCI_CONFIG_MEDIA_TYPE),
            size: Some(34),
        },
    ];

    for spec in &rows {
        let digest = seed_image(&router, repository, spec, None).await;
        let detail = detail_of(&router, spec.case, repository, &digest).await;

        assert_eq!(
            at(&detail, "/artifactMediaType"),
            spec.artifact_media_type
                .map_or(Value::Null, |media_type| json!(media_type)),
            "case {:?}: expected artifactMediaType {:?}, got {:?}",
            spec.case,
            spec.artifact_media_type,
            at(&detail, "/artifactMediaType")
        );
        assert_eq!(
            at(&detail, "/imageSizeInBytes"),
            spec.size.map_or(Value::Null, |size| json!(size)),
            "case {:?}: expected imageSizeInBytes {:?}, got {:?}",
            spec.case,
            spec.size,
            at(&detail, "/imageSizeInBytes")
        );
    }
}

/// The size is the image's CONTENT, never the manifest document: the same manifest padded
/// with annotations of growing length reports one size while its own byte length grows.
#[tokio::test]
async fn the_reported_size_does_not_track_the_manifest_document_s_length() {
    let router = emulator();
    let repository = "padding";
    given_repository(&router, repository, json!({})).await;

    let content = blob_of("the one layer every row of this case references", 48);
    let layer_digest = given_layer(&router, "the shared layer", repository, &content).await;

    // case, how much annotation padding the manifest carries
    for (case, padding) in [
        ("no padding", 0usize),
        ("a short annotation", 32),
        ("an annotation far longer than the image", 4096),
    ] {
        let manifest = json!({
            "schemaVersion": 2,
            "mediaType": OCI_MANIFEST_MEDIA_TYPE,
            "annotations": {"org.example.padding": "p".repeat(padding)},
            "layers": [{
                "mediaType": LAYER_MEDIA_TYPE,
                "digest": layer_digest,
                "size": 48,
            }],
        })
        .to_string();
        ok(
            &router,
            case,
            "PutImage",
            &json!({"repositoryName": repository, "imageManifest": manifest}),
        )
        .await;
        let detail = detail_of(&router, case, repository, &manifest_digest(&manifest)).await;
        assert_eq!(
            at(&detail, "/imageSizeInBytes"),
            json!(48),
            "case {case:?}: the layer total stands whatever the manifest's own {} bytes \
             say, got {:?}",
            manifest.len(),
            at(&detail, "/imageSizeInBytes")
        );
    }
}

/// "If the image is a manifest list, this will be the max size of all manifests in the
/// list" (`ImageDetail$imageSizeInBytes`) — and a list whose children the repository does
/// not hold states no size at all.
#[tokio::test]
async fn a_manifest_list_reports_the_largest_of_the_images_it_references() {
    let router = emulator();
    let repository = "indexes";
    given_repository(&router, repository, json!({})).await;

    let small = ManifestSpec {
        case: "the small child",
        salt: "small",
        manifest_media_type: OCI_MANIFEST_MEDIA_TYPE,
        config: SeededConfig::Present(OCI_CONFIG_MEDIA_TYPE, ("cfg", 16, true)),
        layers: vec![("layer", 24, true)],
        artifact_media_type: Some(OCI_CONFIG_MEDIA_TYPE),
        size: Some(40),
    };
    let large = ManifestSpec {
        case: "the large child",
        salt: "large",
        manifest_media_type: OCI_MANIFEST_MEDIA_TYPE,
        config: SeededConfig::Present(OCI_CONFIG_MEDIA_TYPE, ("cfg", 16, true)),
        layers: vec![("one", 32, true), ("two", 32, true)],
        artifact_media_type: Some(OCI_CONFIG_MEDIA_TYPE),
        size: Some(80),
    };
    let small_digest = seed_image(&router, repository, &small, None).await;
    let large_digest = seed_image(&router, repository, &large, None).await;

    // case, the digests the index references, the size it must report
    let rows: Vec<(&str, Vec<&str>, Option<i64>)> = vec![
        (
            "an index over both children",
            vec![small_digest.as_str(), large_digest.as_str()],
            Some(80),
        ),
        (
            "an index over the smaller child alone",
            vec![small_digest.as_str()],
            Some(40),
        ),
        (
            "an index over a manifest this repository does not hold",
            vec!["sha256:0000000000000000000000000000000000000000000000000000000000000000"],
            None,
        ),
    ];

    for (case, children, size_expected) in rows {
        let index = json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.index.v1+json",
            "manifests": children
                .iter()
                .map(|digest| json!({
                    "mediaType": OCI_MANIFEST_MEDIA_TYPE,
                    "digest": digest,
                    "size": 1,
                }))
                .collect::<Vec<Value>>(),
        })
        .to_string();
        ok(
            &router,
            case,
            "PutImage",
            &json!({"repositoryName": repository, "imageManifest": index}),
        )
        .await;
        let detail = detail_of(&router, case, repository, &manifest_digest(&index)).await;
        assert_eq!(
            at(&detail, "/imageSizeInBytes"),
            size_expected.map_or(Value::Null, |size| json!(size)),
            "case {case:?}: expected imageSizeInBytes {size_expected:?}, got {:?}",
            at(&detail, "/imageSizeInBytes")
        );
        // An index describes no artifact of its own: it carries no config descriptor.
        assert_eq!(
            at(&detail, "/artifactMediaType"),
            Value::Null,
            "case {case:?}: an index declares no artifactMediaType, got {:?}",
            at(&detail, "/artifactMediaType")
        );
    }
}

/// Every answer that carries a manifest advertises that manifest's byte length:
/// `BatchGetImage`'s `images[].imageManifestSize`, and `PutImage`'s own `image`.
///
/// The rows differ in manifest length and content, which is what the rule generalises
/// over.
#[tokio::test]
async fn an_answer_carrying_a_manifest_advertises_its_byte_length() {
    let router = emulator();
    let repository = "manifest-sizes";
    given_repository(&router, repository, json!({})).await;

    // case, manifest, tag
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "a one-member manifest",
            r#"{"schemaVersion":2}"#.to_string(),
            "tiny",
        ),
        (
            "a docker v2 manifest with a media type",
            json!({
                "schemaVersion": 2,
                "mediaType": DOCKER_MANIFEST_MEDIA_TYPE,
                "layers": [],
            })
            .to_string(),
            "docker",
        ),
        (
            "a manifest padded past a kilobyte",
            json!({"schemaVersion": 2, "history": "x".repeat(2048)}).to_string(),
            "padded",
        ),
        (
            "a manifest carrying multi-byte characters, counted in BYTES",
            json!({"schemaVersion": 2, "annotation": "日本語のテキスト"}).to_string(),
            "utf8",
        ),
    ];

    for (case, manifest, tag) in rows {
        let expected = i64::try_from(manifest.len()).expect("a test manifest fits in an i64");
        let pushed = ok(
            &router,
            case,
            "PutImage",
            &json!({
                "repositoryName": repository,
                "imageManifest": manifest,
                "imageTag": tag,
            }),
        )
        .await;
        assert_eq!(
            at(&pushed, "/image/imageManifestSize"),
            json!(expected),
            "case {case:?}: PutImage must advertise the manifest's {expected} bytes, got {:?}",
            at(&pushed, "/image/imageManifestSize")
        );

        for (reader, requested) in [
            ("by tag", json!({"imageTag": tag})),
            (
                "by digest",
                json!({"imageDigest": manifest_digest(&manifest)}),
            ),
        ] {
            let got = ok(
                &router,
                case,
                "BatchGetImage",
                &json!({"repositoryName": repository, "imageIds": [requested]}),
            )
            .await;
            assert_eq!(
                at(&got, "/images/0/imageManifestSize"),
                json!(expected),
                "case {case:?} ({reader}): expected imageManifestSize {expected}, got {:?}",
                at(&got, "/images/0/imageManifestSize")
            );
            assert_eq!(
                at(&got, "/images/0/imageManifest")
                    .as_str()
                    .map(|document| i64::try_from(document.len()).unwrap_or_default()),
                Some(expected),
                "case {case:?} ({reader}): the advertised size must be the size of the \
                 manifest this very answer carries"
            );
        }
    }
}

/// `UpdateImageStorageClass` moves the storage class of the image it names and of no
/// other: the named image reports `ARCHIVED` with a `lastArchivedAt`, a restore puts it
/// back through `ACTIVATING` to `ACTIVE` with a `lastActivatedAt`, and its neighbours stay
/// `ACTIVE` with neither stamp.
///
/// The rows differ in the identifier form the transition names the image by, which is what
/// the resolution rule generalises over.
#[tokio::test]
async fn archiving_one_image_moves_that_image_s_status_and_no_other() {
    let router = emulator();
    let repository = "shared";
    given_repository(&router, repository, json!({})).await;

    // Three images in one repository, so per-image isolation is observable.
    let mut digests = Vec::new();
    for (index, tag) in ["one", "two", "three"].iter().enumerate() {
        let salt = format!("img{index}");
        let spec = plain_image(tag, &salt);
        digests.push(seed_image(&router, repository, &spec, Some(tag)).await);
    }

    let rows: Vec<(&str, Value, usize)> = vec![
        ("archive by tag", json!({"imageTag": "one"}), 0),
        (
            "archive by digest",
            json!({"imageDigest": digests[1].clone()}),
            1,
        ),
        (
            "archive by digest and tag together",
            json!({"imageDigest": digests[2].clone(), "imageTag": "three"}),
            2,
        ),
    ];

    for (case, image_id, index) in rows {
        let archived = ok(
            &router,
            case,
            "UpdateImageStorageClass",
            &json!({
                "repositoryName": repository,
                "imageId": image_id,
                "targetStorageClass": "ARCHIVE",
            }),
        )
        .await;
        assert_eq!(
            at(&archived, "/imageStatus"),
            json!("ARCHIVED"),
            "case {case:?}: the transition must report the status the image is now in, got {:?}",
            at(&archived, "/imageStatus")
        );
        assert_eq!(
            at(&archived, "/imageId/imageDigest"),
            json!(digests[index].clone()),
            "case {case:?}: the answer names the stored image's own digest, got {:?}",
            at(&archived, "/imageId/imageDigest")
        );

        let detail = detail_of(&router, case, repository, &digests[index]).await;
        assert_eq!(
            at(&detail, "/imageStatus"),
            json!("ARCHIVED"),
            "case {case:?}: DescribeImages must read the stored status back, got {:?}",
            at(&detail, "/imageStatus")
        );
        let archived_at = at(&detail, "/lastArchivedAt").as_f64();
        assert!(
            archived_at.is_some_and(|instant| instant > 0.0),
            "case {case:?}: expected a lastArchivedAt instant, got {:?}",
            at(&detail, "/lastArchivedAt")
        );
        assert_eq!(
            at(&detail, "/lastActivatedAt"),
            Value::Null,
            "case {case:?}: an image that was never restored has no lastActivatedAt, got {:?}",
            at(&detail, "/lastActivatedAt")
        );

        // Every image this row did not name is where it was.
        for (other, other_digest) in digests.iter().enumerate() {
            if other <= index {
                continue;
            }
            let untouched = detail_of(&router, case, repository, other_digest).await;
            assert_eq!(
                at(&untouched, "/imageStatus"),
                json!("ACTIVE"),
                "case {case:?}: image {other} was not named and must stay ACTIVE, got {:?}",
                at(&untouched, "/imageStatus")
            );
            assert_eq!(
                at(&untouched, "/lastArchivedAt"),
                Value::Null,
                "case {case:?}: image {other} was never archived, got {:?}",
                at(&untouched, "/lastArchivedAt")
            );
        }

        // Archiving an image that is already archived transitions nothing: the request is
        // already satisfied, and the recorded instant is the real transition's.
        let again = ok(
            &router,
            case,
            "UpdateImageStorageClass",
            &json!({
                "repositoryName": repository,
                "imageId": {"imageDigest": digests[index].clone()},
                "targetStorageClass": "ARCHIVE",
            }),
        )
        .await;
        assert_eq!(
            at(&again, "/imageStatus"),
            json!("ARCHIVED"),
            "case {case:?}: a repeated archive still reports ARCHIVED, got {:?}",
            at(&again, "/imageStatus")
        );
        let after = detail_of(&router, case, repository, &digests[index]).await;
        assert_eq!(
            at(&after, "/lastArchivedAt").as_f64(),
            archived_at,
            "case {case:?}: a repeated archive must not re-stamp lastArchivedAt"
        );

        // The restore starts the image activating, stamping the instant it was restored
        // while keeping the record of when it was archived.
        let restored = ok(
            &router,
            case,
            "UpdateImageStorageClass",
            &json!({
                "repositoryName": repository,
                "imageId": {"imageDigest": digests[index].clone()},
                "targetStorageClass": "STANDARD",
            }),
        )
        .await;
        assert_eq!(
            at(&restored, "/imageStatus"),
            json!("ACTIVATING"),
            "case {case:?}: a restore out of archive is not instantaneous, got {:?}",
            at(&restored, "/imageStatus")
        );
        let activating = detail_of(&router, case, repository, &digests[index]).await;
        assert_eq!(
            at(&activating, "/imageStatus"),
            json!("ACTIVATING"),
            "case {case:?}: a read taken straight after the restore still sees it \
             activating, got {:?}",
            at(&activating, "/imageStatus")
        );
        assert_eq!(
            at(&activating, "/lastArchivedAt").as_f64(),
            archived_at,
            "case {case:?}: the restore keeps the record of when the image was archived"
        );
        let activated_at = at(&activating, "/lastActivatedAt").as_f64();
        assert!(
            activated_at.is_some_and(|instant| Some(instant) >= archived_at),
            "case {case:?}: expected a lastActivatedAt at or after {archived_at:?}, got {:?}",
            at(&activating, "/lastActivatedAt")
        );

        // And once the clock has made the activation due, the image is back in standard
        // storage — without another call having to ask for it.
        tokio::time::sleep(PAST_THE_RESTORE).await;
        let active = detail_of(&router, case, repository, &digests[index]).await;
        assert_eq!(
            at(&active, "/imageStatus"),
            json!("ACTIVE"),
            "case {case:?}: the activation the clock owes must be materialised, got {:?}",
            at(&active, "/imageStatus")
        );
        assert_eq!(
            at(&active, "/lastActivatedAt").as_f64(),
            activated_at,
            "case {case:?}: the completion does not re-stamp lastActivatedAt"
        );
    }
}

/// The status filter reads each image's own status: `ANY` reports every image, and a
/// status named in the filter reports exactly the images in it.
///
/// The model's `imageStatus` doc claims an unfiltered read returns only `ACTIVE` images,
/// but the recorded AWS responses report an archived image to a read that named no
/// `imageStatus` at all — so an absent filter filters on nothing, and that is the row this
/// pins.
#[tokio::test]
async fn the_image_status_filter_selects_on_each_image_s_own_status() {
    let router = emulator();
    let repository = "filtered";
    given_repository(&router, repository, json!({})).await;

    for (index, tag) in ["kept", "cold"].iter().enumerate() {
        let salt = format!("f{index}");
        let spec = plain_image(tag, &salt);
        seed_image(&router, repository, &spec, Some(tag)).await;
    }
    ok(
        &router,
        "archive the cold image",
        "UpdateImageStorageClass",
        &json!({
            "repositoryName": repository,
            "imageId": {"imageTag": "cold"},
            "targetStorageClass": "ARCHIVE",
        }),
    )
    .await;

    // case, the filter the read carries, the tags it must report
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        ("no filter at all", json!({}), vec!["cold", "kept"]),
        (
            "a filter naming no image status",
            json!({"filter": {"tagStatus": "TAGGED"}}),
            vec!["cold", "kept"],
        ),
        (
            "ACTIVE named explicitly",
            json!({"filter": {"imageStatus": "ACTIVE"}}),
            vec!["kept"],
        ),
        (
            "ARCHIVED",
            json!({"filter": {"imageStatus": "ARCHIVED"}}),
            vec!["cold"],
        ),
        (
            "ACTIVATING, which nothing in this repository is",
            json!({"filter": {"imageStatus": "ACTIVATING"}}),
            vec![],
        ),
        (
            "ANY",
            json!({"filter": {"imageStatus": "ANY"}}),
            vec!["cold", "kept"],
        ),
    ];

    for (case, filter, expected) in rows {
        let mut request = filter.clone();
        request["repositoryName"] = json!(repository);
        let expected: Vec<String> = expected.iter().map(ToString::to_string).collect();

        let described = ok(&router, case, "DescribeImages", &request).await;
        let mut described_tags: Vec<String> = described["imageDetails"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|detail| at(detail, "/imageTags/0").as_str().map(ToString::to_string))
            .collect();
        described_tags.sort();
        assert_eq!(
            described_tags, expected,
            "case {case:?}: DescribeImages reported {described_tags:?} for {request:?}"
        );

        let listed = ok(&router, case, "ListImages", &request).await;
        let mut listed_tags: Vec<String> = listed["imageIds"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|id| at(id, "/imageTag").as_str().map(ToString::to_string))
            .collect();
        listed_tags.sort();
        assert_eq!(
            listed_tags, expected,
            "case {case:?}: ListImages reported {listed_tags:?} for {request:?}"
        );
    }
}

/// The blob descriptors this service reads out of ONE manifest are bounded: a document at
/// the bound is read, a document one past it states no size at all.
///
/// `state::manifest_content` is the parse a push stores, so the bound is asserted on it
/// directly — a manifest naming a thousand blobs cannot be pushed over the wire, because
/// every blob it names has to be in the repository first.
#[test]
fn the_blobs_read_out_of_one_manifest_are_bounded() {
    // case, how many layer descriptors the manifest carries, whether they are read
    for (case, count, read) in [
        ("a single layer", 1usize, true),
        ("at the bound", state::MAX_MANIFEST_BLOB_ENTRIES, true),
        (
            "one past the bound",
            state::MAX_MANIFEST_BLOB_ENTRIES + 1,
            false,
        ),
    ] {
        let manifest = json!({
            "schemaVersion": 2,
            "mediaType": OCI_MANIFEST_MEDIA_TYPE,
            "layers": (0..count)
                .map(|index| json!({
                    "mediaType": LAYER_MEDIA_TYPE,
                    "digest": format!("sha256:{index:064x}"),
                    "size": 1,
                }))
                .collect::<Vec<Value>>(),
        })
        .to_string();
        let content = state::manifest_content(&manifest);
        let blobs_read = matches!(content, state::ManifestContent::Blobs(_));
        assert_eq!(
            blobs_read, read,
            "case {case:?}: expected the {count} descriptors to be read == {read}, got \
             {content:?}"
        );
    }
}

/// A transition names an image exactly as a read does, so an identifier that names no
/// stored image, and a repository that is not in this registry, are refused with the
/// errors the operation models.
#[tokio::test]
async fn a_transition_of_an_image_that_is_not_there_is_refused() {
    let router = emulator();
    let repository = "refusals";
    given_repository(&router, repository, json!({})).await;
    let spec = plain_image("the one stored image", "only");
    let digest = seed_image(&router, repository, &spec, Some("v1")).await;

    // case, repository the call names, the imageId it names, the error code it must get
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "a repository this registry does not hold",
            "absent-repository",
            json!({"imageTag": "v1"}),
            "RepositoryNotFoundException",
        ),
        (
            "a tag nothing in the repository carries",
            repository,
            json!({"imageTag": "never-pushed"}),
            "ImageNotFoundException",
        ),
        (
            "a digest nothing in the repository holds",
            repository,
            json!({
                "imageDigest":
                    "sha256:1111111111111111111111111111111111111111111111111111111111111111"
            }),
            "ImageNotFoundException",
        ),
        (
            "a digest and a tag that name different images",
            repository,
            json!({"imageDigest": digest.clone(), "imageTag": "never-pushed"}),
            "ImageNotFoundException",
        ),
        (
            "an identifier carrying neither a digest nor a tag",
            repository,
            json!({}),
            "InvalidParameterException",
        ),
        (
            "a digest that is not a content digest at all",
            repository,
            json!({"imageDigest": "not-a-digest"}),
            "InvalidParameterException",
        ),
    ];

    for (case, named_repository, image_id, code) in rows {
        refused(
            &router,
            case,
            "UpdateImageStorageClass",
            &json!({
                "repositoryName": named_repository,
                "imageId": image_id,
                "targetStorageClass": "ARCHIVE",
            }),
            code,
        )
        .await;
    }

    // Nothing was transitioned by any of the refusals.
    let detail = detail_of(&router, "after the refusals", repository, &digest).await;
    assert_eq!(
        at(&detail, "/imageStatus"),
        json!("ACTIVE"),
        "a refused transition must leave the stored image where it was, got {:?}",
        at(&detail, "/imageStatus")
    );
}
