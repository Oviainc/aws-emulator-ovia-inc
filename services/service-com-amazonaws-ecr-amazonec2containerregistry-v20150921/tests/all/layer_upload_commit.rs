// SPDX-License-Identifier: CC0-1.0

//! Committing an uploaded image layer, and reading it back, end to end over the wire.
//!
//! The rule under test: `CompleteLayerUpload` registers the bytes an upload assembled as
//! a layer of the repository the upload was opened against, under the content digest the
//! registry computes over those bytes, with the byte count it received — and
//! `BatchCheckLayerAvailability` answers for every digest it is asked about from that
//! record: `AVAILABLE` with the stored size for a layer the repository holds,
//! `UNAVAILABLE` for a content digest it holds no layer for, and a `failures` entry only
//! for a request entry that addresses no layer at all.
//!
//! The wire harness is `crate::wire`'s: every case goes through the router, so it
//! exercises routing, deserialization, the handlers, the persisted state and
//! serialization — the same path a push client takes.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{emulator, ok, refused, REGISTRY_ID};

/// A content digest over `bytes`, computed here independently of the crate's own helper:
/// the `sha256:<hex>` form a registry addresses content by.
fn content_digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

/// `len` layer bytes that depend on `seed`: a deterministic spread over the whole byte
/// range, so a layer carries `0x00` and `0xff` like real compressed content and two
/// layers of one test do not carry the same bytes.
fn layer_bytes(seed: usize, len: usize) -> Vec<u8> {
    (0..len)
        .map(|index| u8::try_from((index * 31 + seed * 97 + 13) % 256).unwrap_or_default())
        .collect()
}

/// A `layerPartBlob` member: a blob travels base64-encoded on the awsJson1_1 wire, with
/// the codec `aws_smithy_types::base64` the wire serializer itself uses.
fn blob(bytes: &[u8]) -> Value {
    Value::String(aws_smithy_types::base64::encode(bytes))
}

/// Create the repository `name`, answering nothing: the repository is the container every
/// upload is opened against.
async fn given_repository(router: &MultiServiceRouter, case: &str, name: &str) {
    ok(
        router,
        case,
        "CreateRepository",
        &json!({"repositoryName": name}),
    )
    .await;
}

/// Open an upload against `name`, answering the upload id the registry handed out.
async fn given_upload(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let opened = ok(
        router,
        case,
        "InitiateLayerUpload",
        &json!({"repositoryName": name}),
    )
    .await;
    opened["uploadId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: InitiateLayerUpload must hand back an uploadId"))
        .to_string()
}

/// Stream `parts` into `upload_id` as consecutive layer parts.
async fn upload_parts(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    upload_id: &str,
    parts: &[Vec<u8>],
) {
    let mut sent = 0_i64;
    for part in parts {
        let last = sent + i64::try_from(part.len()).expect("a test part fits in an i64") - 1;
        ok(
            router,
            case,
            "UploadLayerPart",
            &json!({
                "repositoryName": name,
                "uploadId": upload_id,
                "partFirstByte": sent,
                "partLastByte": last,
                "layerPartBlob": blob(part),
            }),
        )
        .await;
        sent = last + 1;
    }
}

/// Ask about `digests` in `repository`.
async fn check(
    router: &MultiServiceRouter,
    case: &str,
    repository: &str,
    digests: &[String],
) -> Value {
    ok(
        router,
        case,
        "BatchCheckLayerAvailability",
        &json!({"repositoryName": repository, "layerDigests": digests}),
    )
    .await
}

/// A digest that is not of the hex this test's content ever digests to, so no upload can
/// have produced it: 64 `f`s, the one a push client uses to probe a registry.
fn absent_digest() -> String {
    format!("sha256:{}", "f".repeat(64))
}

/// A completed upload registers its layer, and the check reports it `AVAILABLE` with the
/// byte count the registry received.
///
/// The rows differ along what the rule generalises over: how many bytes the layer
/// carries and how the client cut them into parts — a one-byte layer, the few-byte and
/// few-hundred-byte layers a tiny image pushes, a layer sent in equal parts, in parts of
/// unequal sizes, in many small parts, and one of tens of kilobytes. The part boundaries
/// are not part of the content, so the digest and the size follow from the bytes alone.
#[tokio::test]
async fn a_completed_upload_registers_its_layer_under_the_digest_of_its_bytes() {
    let cases: Vec<(&str, &str, Vec<usize>)> = vec![
        ("a one-byte layer", "one-byte", vec![1]),
        ("a nineteen-byte layer", "nineteen-bytes", vec![19]),
        ("a 256-byte layer in one part", "small-layer", vec![256]),
        ("a layer in three equal parts", "equal-parts", vec![1024, 1024, 1024]),
        ("a layer in parts of unequal sizes", "uneven-parts", vec![7, 1, 4095, 13]),
        ("a layer in many small parts", "many-parts", vec![3; 32]),
        ("a layer of tens of kilobytes", "big-layer", vec![65_536, 17]),
    ];

    let router = emulator();
    for (case, repository, sizes) in cases {
        given_repository(&router, case, repository).await;
        let upload_id = given_upload(&router, case, repository).await;

        let parts: Vec<Vec<u8>> = sizes
            .iter()
            .copied()
            .enumerate()
            .map(|(index, size)| layer_bytes(index, size))
            .collect();
        let whole: Vec<u8> = parts.concat();
        let digest = content_digest(&whole);
        let size = i64::try_from(whole.len()).expect("the test layer fits in an i64");

        upload_parts(&router, case, repository, &upload_id, &parts).await;

        let completed = ok(
            &router,
            case,
            "CompleteLayerUpload",
            &json!({
                "repositoryName": repository,
                "uploadId": upload_id,
                "layerDigests": [digest.clone()],
            }),
        )
        .await;
        assert_eq!(
            completed,
            json!({
                "registryId": REGISTRY_ID,
                "repositoryName": repository,
                "uploadId": upload_id,
                "layerDigest": digest,
            }),
            "case {case:?}: the completion reports the upload it closed and the layer it \
             registered, got {completed:?}"
        );

        // The read-back: the registry holds the layer, with the byte count it received.
        let answer = check(&router, case, repository, std::slice::from_ref(&digest)).await;
        assert_eq!(
            answer,
            json!({
                "layers": [{
                    "layerDigest": digest,
                    "layerAvailability": "AVAILABLE",
                    "layerSize": size,
                }],
                "failures": [],
            }),
            "case {case:?}: the committed layer is available with its {size} bytes, got \
             {answer:?}"
        );

        // The upload is finished: its id addresses nothing afterwards.
        refused(
            &router,
            case,
            "CompleteLayerUpload",
            &json!({
                "repositoryName": repository,
                "uploadId": upload_id,
                "layerDigests": [digest.clone()],
            }),
            "UploadNotFoundException",
        )
        .await;

        // And layers are per-repository: the same digest is not a layer of another one.
        let elsewhere = format!("{repository}-elsewhere");
        given_repository(&router, case, &elsewhere).await;
        let absent = check(&router, case, &elsewhere, std::slice::from_ref(&digest)).await;
        assert_eq!(
            absent,
            json!({
                "layers": [{"layerDigest": digest, "layerAvailability": "UNAVAILABLE"}],
                "failures": [],
            }),
            "case {case:?}: a repository the layer was not uploaded to has not got it, got \
             {absent:?}"
        );
    }
}

/// A batch mixing a committed layer with digests the repository holds no layer for is
/// answered digest by digest: each gets a `layers` entry, available or not, and nothing
/// is a failure.
///
/// The rows differ along the form of the absent digest: the all-`f` probe, the same hex
/// in upper case, another algorithm, a short hex, and the digest of bytes that were
/// uploaded to no repository.
#[tokio::test]
async fn a_mixed_batch_reports_each_digest_as_available_or_unavailable() {
    let repository = "mixed";
    let router = emulator();
    given_repository(&router, "mixed", repository).await;
    let upload_id = given_upload(&router, "mixed", repository).await;
    let whole = layer_bytes(5, 19);
    let held = content_digest(&whole);
    upload_parts(
        &router,
        "mixed",
        repository,
        &upload_id,
        std::slice::from_ref(&whole),
    )
    .await;
    ok(
        &router,
        "mixed",
        "CompleteLayerUpload",
        &json!({
            "repositoryName": repository,
            "uploadId": upload_id,
            "layerDigests": [held.clone()],
        }),
    )
    .await;

    let absent: Vec<(&str, String)> = vec![
        ("the all-f probe", absent_digest()),
        (
            "the same hex in upper case",
            format!("sha256:{}", "F".repeat(64)),
        ),
        ("another algorithm", format!("sha512:{}", "a".repeat(128))),
        ("a short hex", "sha256:abc123".to_string()),
        (
            "the digest of bytes uploaded nowhere",
            content_digest(&layer_bytes(6, 64)),
        ),
    ];

    for (case, digest) in &absent {
        // Asked in both orders, so neither half of the batch depends on its position.
        for (order, digests) in [
            ("held first", vec![held.clone(), digest.clone()]),
            ("held last", vec![digest.clone(), held.clone()]),
        ] {
            let case = format!("{case} ({order})");
            let answer = check(&router, &case, repository, &digests).await;
            let layers = answer["layers"]
                .as_array()
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: no layers list in {answer:?}"));
            assert_eq!(
                layers.len(),
                2,
                "case {case:?}: both digests are answered as layers, got {layers:?}"
            );
            let (available, unavailable) = if order == "held first" {
                (&layers[0], &layers[1])
            } else {
                (&layers[1], &layers[0])
            };
            assert_eq!(
                available,
                &json!({
                    "layerDigest": held,
                    "layerAvailability": "AVAILABLE",
                    "layerSize": 19,
                }),
                "case {case:?}: the committed layer is available with its 19 bytes, got \
                 {available:?}"
            );
            assert_eq!(
                unavailable,
                &json!({"layerDigest": digest, "layerAvailability": "UNAVAILABLE"}),
                "case {case:?}: a digest the repository holds no layer for is unavailable \
                 with no size, got {unavailable:?}"
            );
            assert_eq!(
                answer["failures"],
                json!([]),
                "case {case:?}: every entry addresses a layer, so none is a failure, got {:?}",
                answer["failures"]
            );
        }
    }
}

/// A completion the registry cannot honour is refused with the modelled error, and
/// nothing is registered.
///
/// The rows differ along what makes the call impossible: the repository, the upload id,
/// an upload that received nothing, and a claimed digest that is not the digest of the
/// bytes received.
#[tokio::test]
async fn a_completion_the_registry_cannot_honour_is_refused() {
    let router = emulator();
    let repository = "refusals";
    let other = "refusals-other";
    given_repository(&router, "refusals", repository).await;
    given_repository(&router, "refusals", other).await;

    // An upload id this registry never handed out, in the layout the model requires.
    let unknown_upload = "00000000-0000-0000-0000-000000000000";
    let whole = layer_bytes(7, 48);
    let digest = content_digest(&whole);

    let upload_id = given_upload(&router, "refusals", repository).await;
    upload_parts(
        &router,
        "refusals",
        repository,
        &upload_id,
        std::slice::from_ref(&whole),
    )
    .await;
    let empty_upload = given_upload(&router, "refusals", repository).await;
    let other_upload = given_upload(&router, "refusals", other).await;

    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a repository this registry has not got",
            json!({
                "repositoryName": "no-such-repo",
                "uploadId": upload_id,
                "layerDigests": [digest.clone()],
            }),
            "RepositoryNotFoundException",
        ),
        (
            "another account's registry",
            json!({
                "repositoryName": repository,
                "registryId": "123456789012",
                "uploadId": upload_id,
                "layerDigests": [digest.clone()],
            }),
            "InvalidParameterException",
        ),
        (
            "an upload id this registry never issued",
            json!({
                "repositoryName": repository,
                "uploadId": unknown_upload,
                "layerDigests": [digest.clone()],
            }),
            "UploadNotFoundException",
        ),
        (
            "an upload opened against another repository",
            json!({
                "repositoryName": repository,
                "uploadId": other_upload,
                "layerDigests": [digest.clone()],
            }),
            "UploadNotFoundException",
        ),
        (
            "an upload that received no part",
            json!({
                "repositoryName": repository,
                "uploadId": empty_upload,
                "layerDigests": [digest.clone()],
            }),
            "EmptyUploadException",
        ),
        (
            "a claimed digest of other bytes",
            json!({
                "repositoryName": repository,
                "uploadId": upload_id,
                "layerDigests": [content_digest(&layer_bytes(8, 48))],
            }),
            "InvalidLayerException",
        ),
        (
            "a claimed digest of a prefix of the bytes received",
            json!({
                "repositoryName": repository,
                "uploadId": upload_id,
                "layerDigests": [content_digest(&whole[..24])],
            }),
            "InvalidLayerException",
        ),
        (
            "a claimed digest no content digests to",
            json!({
                "repositoryName": repository,
                "uploadId": upload_id,
                "layerDigests": [absent_digest()],
            }),
            "InvalidLayerException",
        ),
    ];

    for (case, request, expected) in cases {
        refused(&router, case, "CompleteLayerUpload", &request, expected).await;
        // A refused completion registers nothing: the layer is still not in either
        // repository.
        for name in [repository, other] {
            let answer = check(&router, case, name, std::slice::from_ref(&digest)).await;
            assert_eq!(
                answer["layers"][0]["layerAvailability"],
                json!("UNAVAILABLE"),
                "case {case:?}: no layer is registered in {name:?} by a refused completion, \
                 got {answer:?}"
            );
        }
    }

    // The upload the refused calls named survives them, so the client can complete it
    // with the digest the registry really computed.
    let completed = ok(
        &router,
        "the digest the registry computed",
        "CompleteLayerUpload",
        &json!({
            "repositoryName": repository,
            "uploadId": upload_id,
            "layerDigests": [digest.clone()],
        }),
    )
    .await;
    assert_eq!(
        completed["layerDigest"],
        json!(digest),
        "the layer is registered under the digest the registry computed, got {completed:?}"
    );

    // And the content is in the repository once: a second upload of the same bytes has
    // nothing to add.
    let again = given_upload(&router, "already there", repository).await;
    upload_parts(
        &router,
        "already there",
        repository,
        &again,
        std::slice::from_ref(&whole),
    )
    .await;
    refused(
        &router,
        "the same content a second time",
        "CompleteLayerUpload",
        &json!({
            "repositoryName": repository,
            "uploadId": again,
            "layerDigests": [digest.clone()],
        }),
        "LayerAlreadyExistsException",
    )
    .await;
    let answer = check(
        &router,
        "after the refused second push",
        repository,
        std::slice::from_ref(&digest),
    )
    .await;
    assert_eq!(
        answer["layers"][0]["layerSize"],
        json!(48),
        "the layer still reports the bytes the registry received, got {answer:?}"
    );
}

/// A manifest pushed over committed layers adds what only a descriptor knows — the media
/// type — while the size stays the byte count the registry received.
#[tokio::test]
async fn a_manifest_over_committed_layers_adds_the_media_type_it_declares() {
    let router = emulator();
    let repository = "push";
    given_repository(&router, "push", repository).await;

    // Two blobs of different sizes: an image's configuration and its one filesystem
    // layer, each uploaded the way a push client uploads it.
    let config_bytes = layer_bytes(1, 73);
    let layer_bytes_sent = layer_bytes(2, 512);
    let mut committed: Vec<(String, i64)> = Vec::new();
    for bytes in [&config_bytes, &layer_bytes_sent] {
        let upload_id = given_upload(&router, "push", repository).await;
        upload_parts(
            &router,
            "push",
            repository,
            &upload_id,
            std::slice::from_ref(bytes),
        )
        .await;
        let digest = content_digest(bytes);
        ok(
            &router,
            "push",
            "CompleteLayerUpload",
            &json!({
                "repositoryName": repository,
                "uploadId": upload_id,
                "layerDigests": [digest.clone()],
            }),
        )
        .await;
        committed.push((
            digest,
            i64::try_from(bytes.len()).expect("the test blob fits in an i64"),
        ));
    }

    let config_media = "application/vnd.oci.image.config.v1+json";
    let layer_media = "application/vnd.oci.image.layer.v1.tar+gzip";
    let manifest = serde_json::to_string(&json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {
            "mediaType": config_media,
            // A descriptor's size is the manifest author's claim; the registry reports
            // the bytes it received.
            "size": 1,
            "digest": committed[0].0,
        },
        "layers": [{
            "mediaType": layer_media,
            "size": 1,
            "digest": committed[1].0,
        }],
    }))
    .expect("the manifest serialises");
    ok(
        &router,
        "push",
        "PutImage",
        &json!({
            "repositoryName": repository,
            "imageManifest": manifest,
            "imageTag": "v1",
        }),
    )
    .await;

    let digests: Vec<String> = committed.iter().map(|(digest, _)| digest.clone()).collect();
    let answer = check(&router, "after the manifest push", repository, &digests).await;
    for (index, media_type) in [config_media, layer_media].iter().enumerate() {
        let (digest, size) = &committed[index];
        assert_eq!(
            answer["layers"][index],
            json!({
                "layerDigest": digest,
                "layerAvailability": "AVAILABLE",
                "layerSize": size,
                "mediaType": media_type,
            }),
            "case {media_type:?}: the layer keeps the size the registry received and takes \
             the media type the manifest declares, got {:?}",
            answer["layers"][index]
        );
    }
}
