// SPDX-License-Identifier: CC0-1.0

//! What `PutImage` resolves and checks before it commits a manifest, driven end to end
//! over the wire.
//!
//! Two rules are under test, both about a document the caller supplies:
//!
//! 1. A manifest is committed only over blobs the repository already holds. "When an image
//!    is pushed and all new image layers have been uploaded, the PutImage API is called"
//!    (`PutImage`), so every blob digest the document names — its `config` descriptor's and
//!    each `layers` descriptor's — has to resolve to a completed layer of that repository
//!    (`CompleteLayerUpload` is what adds one). A push naming any blob that is absent is
//!    refused with the modelled `LayersNotFoundException` (client, httpError 400, "the
//!    specified layers can't be found"), and neither the image nor the tag is created.
//! 2. A manifest is only a manifest of the kind it declares itself to be. The schema named
//!    by the media type declared for the document — the one the request states, else the
//!    document's own ("if you push an image manifest that doesn't contain the mediaType
//!    field, you must specify the imageManifestMediaType in the request") — fixes which
//!    content member the document must carry: `manifests` for a manifest list / OCI index,
//!    `layers` for an image manifest, `fsLayers` for the Docker V2 Schema 1 form. A
//!    document that declares one schema while carrying the content of another is refused
//!    with the modelled `InvalidParameterException` (400) and nothing is stored for it.
//!
//! The tables span what each rule generalises over: which blob is absent (a layer, the
//! config, several at once, all of them), how many are named, the spelling of a digest's
//! hex, and whether the document names blobs at all; then every media type the registry
//! knows crossed with the content a document can carry, plus a media type it does not know,
//! no media type at all, one declared by the request instead of the document, and one
//! carrying media-type parameters.

use serde_json::{json, Value};
use service_com_amazonaws_ecrpublic_spencerfrontendservice::state::MAX_MANIFEST_LAYER_ENTRIES;

use crate::harness::{blob_digest, error_message, given_layer, layer_digest};
use crate::repository_image_lifecycle::{call, digest_of, ok, refused, service, REGISTRY_ID};

/// An OCI/Docker content descriptor over the blob `body`: what a `config` member and a
/// `layers` entry both are.
fn descriptor(body: &str) -> Value {
    json!({
        "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
        "digest": blob_digest(body),
        "size": body.len(),
    })
}

/// An image manifest naming `config` and `layers` by digest, annotated with `case` so two
/// rows never hash to one manifest digest.
fn manifest_over(case: &str, config: Option<&str>, layers: &[&str]) -> String {
    let mut document = json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "annotations": {"case": case},
        "layers": layers.iter().map(|body| descriptor(body)).collect::<Vec<Value>>(),
    });
    if let Some(config) = config {
        document["config"] = descriptor(config);
    }
    document.to_string()
}

/// One push whose manifest names blobs the repository does not all hold.
struct MissingBlobCase {
    case: &'static str,
    /// The `config` blob the manifest names, if it names one.
    config: Option<&'static str>,
    /// The `layers` blobs the manifest names, in document order.
    layers: &'static [&'static str],
    /// The blobs uploaded into the repository before the push.
    uploaded: &'static [&'static str],
    /// The blobs the refusal must report as absent, in document order (config first).
    missing: &'static [&'static str],
}

fn missing_blob_cases() -> Vec<MissingBlobCase> {
    vec![
        MissingBlobCase {
            case: "one layer never uploaded, beside a config that was",
            config: Some("case1-config"),
            layers: &["case1-layer"],
            uploaded: &["case1-config"],
            missing: &["case1-layer"],
        },
        MissingBlobCase {
            case: "the config blob never uploaded, beside a layer that was",
            config: Some("case2-config"),
            layers: &["case2-layer"],
            uploaded: &["case2-layer"],
            missing: &["case2-config"],
        },
        MissingBlobCase {
            case: "three layers, only the middle one missing",
            config: Some("case3-config"),
            layers: &["case3-a", "case3-b", "case3-c"],
            uploaded: &["case3-config", "case3-a", "case3-c"],
            missing: &["case3-b"],
        },
        MissingBlobCase {
            case: "nothing uploaded at all, so every named blob is absent",
            config: Some("case4-config"),
            layers: &["case4-a", "case4-b"],
            uploaded: &[],
            missing: &["case4-config", "case4-a", "case4-b"],
        },
        MissingBlobCase {
            case: "one absent layer named twice, reported once",
            config: Some("case5-config"),
            layers: &["case5-layer", "case5-layer"],
            uploaded: &["case5-config"],
            missing: &["case5-layer"],
        },
        MissingBlobCase {
            case: "no config member at all, one absent layer",
            config: None,
            layers: &["case6-layer"],
            uploaded: &[],
            missing: &["case6-layer"],
        },
    ]
}
#[tokio::test]
async fn a_push_naming_a_blob_the_repository_does_not_hold_is_refused_and_stores_nothing() {
    let router = service();

    for (index, row) in missing_blob_cases().into_iter().enumerate() {
        let case = row.case;
        let repository = format!("layers/case{index:02}");
        ok(
            &router,
            "CreateRepository",
            json!({"repositoryName": repository}),
            case,
        )
        .await;
        for body in row.uploaded {
            given_layer(&router, &repository, body.as_bytes(), case).await;
        }

        let manifest = manifest_over(case, row.config, row.layers);
        let tag = "pushed";
        let body = refused(
            &router,
            "PutImage",
            json!({
                "repositoryName": repository,
                "imageManifest": manifest,
                "imageTag": tag,
            }),
            "LayersNotFoundException",
            case,
        )
        .await;

        let absent: Vec<String> = row
            .missing
            .iter()
            .map(|body| layer_digest(body.as_bytes()))
            .collect();
        let expected = format!(
            "Layers with digests '[{}]' required for pushing image into repository with \
             name '{repository}' in the registry with id '{REGISTRY_ID}' do not exist",
            absent.join(", ")
        );
        let actual = error_message(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );

        // Neither the image nor the tag was created: both readers answer as they do for a
        // repository that holds nothing.
        for identifier in [
            json!({"imageTag": tag}),
            json!({"imageDigest": digest_of(&manifest)}),
        ] {
            refused(
                &router,
                "DescribeImages",
                json!({"repositoryName": repository, "imageIds": [identifier]}),
                "ImageNotFoundException",
                case,
            )
            .await;
        }
        let held = ok(
            &router,
            "DescribeImages",
            json!({"repositoryName": repository}),
            case,
        )
        .await;
        assert_eq!(
            held["imageDetails"],
            json!([]),
            "case {case:?}: a refused push stores no image, got {held}"
        );

        // The repair path: upload what was missing and the IDENTICAL manifest is committed.
        for body in row.missing {
            given_layer(&router, &repository, body.as_bytes(), case).await;
        }
        let pushed = ok(
            &router,
            "PutImage",
            json!({
                "repositoryName": repository,
                "imageManifest": manifest,
                "imageTag": tag,
            }),
            case,
        )
        .await;
        assert_eq!(
            pushed["image"]["imageId"]["imageDigest"],
            json!(digest_of(&manifest)),
            "case {case:?}: the identical manifest is committed once its blobs are held, \
             got {pushed}"
        );
    }
}

#[tokio::test]
async fn a_layers_array_beside_a_manifests_one_still_has_its_blobs_resolved() {
    // What a document names is read from the members it carries: a `manifests` member
    // beside the layer descriptors does not make the layer blobs unnamed, so a push whose
    // layer was never uploaded is refused exactly as it is without that member.
    let case = "a layers array beside a manifests one";
    let router = service();
    let repository = "layers/hybrid";
    ok(
        &router,
        "CreateRepository",
        json!({"repositoryName": repository}),
        case,
    )
    .await;

    let blob = "hybrid-missing-blob";
    let manifest = json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "manifests": [],
        "layers": [{"digest": blob_digest(blob), "size": blob.len()}],
    })
    .to_string();
    let body = refused(
        &router,
        "PutImage",
        json!({
            "repositoryName": repository,
            "imageManifest": manifest,
            "imageTag": "hybrid",
        }),
        "LayersNotFoundException",
        case,
    )
    .await;
    let expected = format!(
        "Layers with digests '[{}]' required for pushing image into repository with name \
         '{repository}' in the registry with id '{REGISTRY_ID}' do not exist",
        layer_digest(blob.as_bytes())
    );
    let actual = error_message(&body);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {expected:?}, got {actual:?}"
    );
    let held = ok(
        &router,
        "DescribeImages",
        json!({"repositoryName": repository}),
        case,
    )
    .await;
    assert_eq!(
        held["imageDetails"],
        json!([]),
        "case {case:?}: a refused push stores no image, got {held}"
    );

    // And once the blob is held the same document is committed.
    given_layer(&router, repository, blob.as_bytes(), case).await;
    let pushed = ok(
        &router,
        "PutImage",
        json!({
            "repositoryName": repository,
            "imageManifest": manifest,
            "imageTag": "hybrid",
        }),
        case,
    )
    .await;
    assert_eq!(
        pushed["image"]["imageId"]["imageDigest"],
        json!(digest_of(&manifest)),
        "case {case:?}: expected the manifest's digest once its blob is held, got {pushed}"
    );
}

#[tokio::test]
async fn a_push_whose_named_blobs_all_resolve_is_committed() {
    let router = service();

    // Rows that differ along how a document names blobs: by digest in both members, in a
    // hex spelling other than the stored one (hex is a case-insensitive rendering of the
    // same bytes), with descriptors that name no digest at all, as a manifest list naming
    // CHILD MANIFESTS rather than blobs, and as a Schema 1 document whose `fsLayers` name
    // no `digest` member.
    let held = "held-blob";
    let upper_hex = {
        let digest = layer_digest("upper-hex-blob".as_bytes());
        let (algorithm, hex) = digest.split_once(':').expect("a two-part digest");
        format!("{algorithm}:{}", hex.to_ascii_uppercase())
    };
    let rows: Vec<(&str, Vec<&str>, String)> = vec![
        (
            "both members name blobs the repository holds",
            vec![held, "second-held-blob"],
            json!({
                "schemaVersion": 2,
                "mediaType": "application/vnd.oci.image.manifest.v1+json",
                "config": {"digest": blob_digest(held), "size": 9},
                "layers": [{"digest": blob_digest("second-held-blob"), "size": 16}],
            })
            .to_string(),
        ),
        (
            "a layer named in upper-case hex, stored in lower case",
            vec!["upper-hex-blob"],
            json!({
                "schemaVersion": 2,
                "mediaType": "application/vnd.oci.image.manifest.v1+json",
                "layers": [{"digest": upper_hex, "size": 14}],
            })
            .to_string(),
        ),
        (
            // A document carrying BOTH content members: the layer blob is still named, so
            // it still has to be held — the `manifests` member beside it skips nothing.
            "a layers array beside a manifests one",
            vec!["hybrid-held-blob"],
            json!({
                "schemaVersion": 2,
                "mediaType": "application/vnd.oci.image.manifest.v1+json",
                "manifests": [],
                "layers": [{"digest": blob_digest("hybrid-held-blob"), "size": 16}],
            })
            .to_string(),
        ),
        (
            "descriptors that name no digest, so they name no blob",
            vec![],
            json!({
                "schemaVersion": 2,
                "mediaType": "application/vnd.oci.image.manifest.v1+json",
                "config": {"mediaType": "application/vnd.oci.image.config.v1+json"},
                "layers": [{"mediaType": "application/vnd.oci.image.layer.v1.tar+gzip"}],
            })
            .to_string(),
        ),
        (
            "a manifest list names child manifests, not blobs",
            vec![],
            json!({
                "schemaVersion": 2,
                "mediaType": "application/vnd.docker.distribution.manifest.list.v2+json",
                "manifests": [{
                    "mediaType": "application/vnd.docker.distribution.manifest.v2+json",
                    "digest": format!("sha256:{}", "7".repeat(64)),
                    "size": 1,
                }],
            })
            .to_string(),
        ),
        (
            "a schema 1 document, whose fsLayers name no descriptor digest",
            vec![],
            json!({
                "schemaVersion": 1,
                "mediaType": "application/vnd.docker.distribution.manifest.v1+json",
                "name": "legacy",
                "fsLayers": [{"blobSum": format!("sha256:{}", "8".repeat(64))}],
            })
            .to_string(),
        ),
    ];

    for (index, (case, blobs, manifest)) in rows.into_iter().enumerate() {
        let repository = format!("resolves/case{index:02}");
        ok(
            &router,
            "CreateRepository",
            json!({"repositoryName": repository}),
            case,
        )
        .await;
        for blob in &blobs {
            given_layer(&router, &repository, blob.as_bytes(), case).await;
        }

        let tag = format!("v{index}");
        let pushed = ok(
            &router,
            "PutImage",
            json!({
                "repositoryName": repository,
                "imageManifest": manifest,
                "imageTag": tag,
            }),
            case,
        )
        .await;
        assert_eq!(
            pushed["image"]["imageId"]["imageDigest"],
            json!(digest_of(&manifest)),
            "case {case:?}: expected the manifest's digest, got {pushed}"
        );

        // Read it back: the image is in the repository under the tag that was pushed.
        let described = ok(
            &router,
            "DescribeImages",
            json!({"repositoryName": repository, "imageIds": [{"imageTag": tag}]}),
            case,
        )
        .await;
        assert_eq!(
            described["imageDetails"][0]["imageDigest"],
            json!(digest_of(&manifest)),
            "case {case:?}: expected the pushed image to read back, got {described}"
        );
    }
}

#[tokio::test]
async fn a_manifest_is_read_up_to_the_layer_descriptors_the_registry_reads_and_no_further() {
    // The bound on how many `layers[*]` descriptors one document may name. Source:
    // `state::MAX_MANIFEST_LAYER_ENTRIES`, which a repository's own layer ceiling fixes.
    // Each descriptor names no blob, so what a row turns on is the count alone: at the
    // bound the document is read and committed, one past it the push is refused and
    // nothing is stored.
    let router = service();
    let document_naming = |entries: usize| -> String {
        json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "annotations": {"entries": entries.to_string()},
            "layers": (0..entries).map(|size| json!({"size": size})).collect::<Vec<Value>>(),
        })
        .to_string()
    };

    for (case, entries, accepted) in [
        ("descriptors exactly at the bound", MAX_MANIFEST_LAYER_ENTRIES, true),
        ("one descriptor past the bound", MAX_MANIFEST_LAYER_ENTRIES + 1, false),
    ] {
        let repository = format!("counts/{entries}");
        ok(
            &router,
            "CreateRepository",
            json!({"repositoryName": repository}),
            case,
        )
        .await;
        let manifest = document_naming(entries);
        let request = json!({
            "repositoryName": repository,
            "imageManifest": manifest,
            "imageTag": "counted",
        });
        if accepted {
            let (status, pushed) = call(&router, "PutImage", request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the push to be served, got {status} with {pushed}"
            );
            assert_eq!(
                pushed["image"]["imageId"]["imageDigest"],
                json!(digest_of(&manifest)),
                "case {case:?}: expected the manifest's digest, got {pushed}"
            );
        } else {
            let body = refused(
                &router,
                "PutImage",
                request,
                "InvalidParameterException",
                case,
            )
            .await;
            let actual = error_message(&body);
            // The parameter clause for the count rule, built from the bound itself.
            let expected = format!(
                "Invalid parameter at 'ImageManifest' failed to satisfy constraint: 'Member \
                 must have no more than {MAX_MANIFEST_LAYER_ENTRIES} layers'"
            );
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
            let held = ok(
                &router,
                "DescribeImages",
                json!({"repositoryName": repository}),
                case,
            )
            .await;
            assert_eq!(
                held["imageDetails"],
                json!([]),
                "case {case:?}: a refused push stores no image, got {held}"
            );
        }
    }
}

/// One push whose manifest declares a schema, and whether the document satisfies it.
struct SchemaCase {
    case: &'static str,
    /// The `mediaType` the document itself states, if any.
    embedded: Option<&'static str>,
    /// The `imageManifestMediaType` the request declares, if any.
    declared: Option<&'static str>,
    /// The content members the document carries.
    content: Value,
    /// Whether the push must be refused.
    refused: bool,
}

/// Every media type whose schema this registry knows, with the member that schema requires:
/// `manifests` for a list / index, `layers` for an image manifest, `fsLayers` for the
/// Docker V2 Schema 1 form.
fn schema_cases() -> Vec<SchemaCase> {
    // The content a document can carry, each naming no blob digest, so what a row turns on
    // is the schema check alone.
    let manifests = json!({"manifests": []});
    let layers = json!({"config": {"size": 7}, "layers": [{"size": 12}]});
    let fs_layers = json!({"fsLayers": [{"blobSum": "sha256:0"}]});
    let nothing = json!({"annotations": {"note": "no content member at all"}});
    vec![
        SchemaCase {
            case: "a docker manifest list carrying manifests",
            embedded: Some("application/vnd.docker.distribution.manifest.list.v2+json"),
            declared: None,
            content: manifests.clone(),
            refused: false,
        },
        SchemaCase {
            case: "a docker manifest list carrying single-image members",
            embedded: Some("application/vnd.docker.distribution.manifest.list.v2+json"),
            declared: None,
            content: layers.clone(),
            refused: true,
        },
        SchemaCase {
            case: "an oci index carrying manifests",
            embedded: Some("application/vnd.oci.image.index.v1+json"),
            declared: None,
            content: manifests.clone(),
            refused: false,
        },
        SchemaCase {
            case: "an oci index carrying nothing",
            embedded: Some("application/vnd.oci.image.index.v1+json"),
            declared: None,
            content: nothing.clone(),
            refused: true,
        },
        SchemaCase {
            case: "a docker v2 image manifest carrying layers",
            embedded: Some("application/vnd.docker.distribution.manifest.v2+json"),
            declared: None,
            content: layers.clone(),
            refused: false,
        },
        SchemaCase {
            case: "a docker v2 image manifest carrying a manifest list's members",
            embedded: Some("application/vnd.docker.distribution.manifest.v2+json"),
            declared: None,
            content: manifests.clone(),
            refused: true,
        },
        SchemaCase {
            case: "an oci image manifest carrying layers and no config",
            embedded: Some("application/vnd.oci.image.manifest.v1+json"),
            declared: None,
            content: json!({"layers": []}),
            refused: false,
        },
        SchemaCase {
            case: "a schema 1 manifest carrying fsLayers",
            embedded: Some("application/vnd.docker.distribution.manifest.v1+json"),
            declared: None,
            content: fs_layers.clone(),
            refused: false,
        },
        SchemaCase {
            case: "a signed schema 1 manifest carrying fsLayers",
            embedded: Some("application/vnd.docker.distribution.manifest.v1+prettyjws"),
            declared: None,
            content: fs_layers.clone(),
            refused: false,
        },
        SchemaCase {
            case: "a schema 1 manifest carrying v2 layers instead",
            embedded: Some("application/vnd.docker.distribution.manifest.v1+json"),
            declared: None,
            content: layers.clone(),
            refused: true,
        },
        SchemaCase {
            case: "a media type this registry knows no schema for",
            embedded: Some("application/octet-stream"),
            declared: None,
            content: nothing.clone(),
            refused: false,
        },
        SchemaCase {
            case: "no media type at all, so no schema is declared",
            embedded: None,
            declared: None,
            content: nothing.clone(),
            refused: false,
        },
        SchemaCase {
            case: "the index schema declared by the request over single-image content",
            embedded: None,
            declared: Some("application/vnd.oci.image.index.v1+json"),
            content: layers.clone(),
            refused: true,
        },
        SchemaCase {
            case: "the index schema declared by the request over an index",
            embedded: None,
            declared: Some("application/vnd.oci.image.index.v1+json"),
            content: manifests.clone(),
            refused: false,
        },
        SchemaCase {
            case: "the request's media type over the document's own",
            embedded: Some("application/vnd.oci.image.index.v1+json"),
            declared: Some("application/vnd.oci.image.manifest.v1+json"),
            content: layers.clone(),
            refused: false,
        },
        SchemaCase {
            case: "a media type carrying parameters after its subtype",
            embedded: Some("application/vnd.oci.image.index.v1+json; charset=utf-8"),
            declared: None,
            content: layers.clone(),
            refused: true,
        },
    ]
}

#[tokio::test]
async fn a_manifest_is_checked_against_the_schema_its_media_type_declares() {
    let router = service();

    for (index, row) in schema_cases().into_iter().enumerate() {
        let case = row.case;
        let repository = format!("schemas/case{index:02}");
        ok(
            &router,
            "CreateRepository",
            json!({"repositoryName": repository}),
            case,
        )
        .await;

        let mut document = json!({"schemaVersion": 2, "annotations": {"case": case}});
        if let Some(embedded) = row.embedded {
            document["mediaType"] = json!(embedded);
        }
        for (member, value) in row
            .content
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: the row's content is an object"))
        {
            document[member] = value.clone();
        }
        let manifest = document.to_string();

        let tag = "declared";
        let mut request = json!({
            "repositoryName": repository,
            "imageManifest": manifest,
            "imageTag": tag,
        });
        if let Some(declared) = row.declared {
            request["imageManifestMediaType"] = json!(declared);
        }

        if row.refused {
            let body = refused(
                &router,
                "PutImage",
                request,
                "InvalidParameterException",
                case,
            )
            .await;
            let actual = error_message(&body);
            let expected = "Invalid parameter at 'ImageManifest' failed to satisfy \
                            constraint: 'Invalid JSON syntax'";
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
            // Nothing was stored under the tag the refused push named.
            refused(
                &router,
                "DescribeImages",
                json!({"repositoryName": repository, "imageIds": [{"imageTag": tag}]}),
                "ImageNotFoundException",
                case,
            )
            .await;
            let held = ok(
                &router,
                "DescribeImages",
                json!({"repositoryName": repository}),
                case,
            )
            .await;
            assert_eq!(
                held["imageDetails"],
                json!([]),
                "case {case:?}: a refused push stores no image, got {held}"
            );
        } else {
            let (status, pushed) = call(&router, "PutImage", request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the push to be served, got {status} with {pushed}"
            );
            let described = ok(
                &router,
                "DescribeImages",
                json!({"repositoryName": repository, "imageIds": [{"imageTag": tag}]}),
                case,
            )
            .await;
            assert_eq!(
                described["imageDetails"][0]["imageDigest"],
                json!(digest_of(&manifest)),
                "case {case:?}: expected the pushed image to read back, got {described}"
            );
        }
    }
}
