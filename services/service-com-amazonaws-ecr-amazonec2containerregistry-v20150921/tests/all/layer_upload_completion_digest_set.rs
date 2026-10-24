// SPDX-License-Identifier: CC0-1.0

//! `CompleteLayerUpload`'s digest ASSERTION, and the push it unblocks, end to end over
//! the wire.
//!
//! The rule under test: `CompleteLayerUpload` closes the upload it names by registering
//! the bytes that upload accepted as a layer of its repository, under the digest the
//! registry computed over those bytes — succeeding exactly when that computed digest is
//! one of the digests the request asserts, whatever the size of the asserted set and
//! whatever position the digest takes in it, and refusing with `InvalidLayerException`
//! when the set does not hold it exactly. The model admits an asserted set of 1 to 100
//! digests (`smithy_query(query="operation", operation="CompleteLayerUpload")`:
//! `layerDigests`, `length {min: 1, max: 100}`), and it does not say which slot the
//! layer's own digest sits in — so the rows span that range and every position in it,
//! where the suite beside this one (`layer_upload_commit`) spans the layer's own bytes
//! and part boundaries with a one-digest set.
//!
//! The last test is the completion's point: a manifest is only pushable once every blob
//! it references has been COMPLETED into the repository, so the same `PutImage` that is
//! refused with `LayersNotFoundException` before the completions succeeds after them and
//! `BatchGetImage` then pulls the image back.
//!
//! Everything goes through the router with the crate's shared harness
//! (`tests/all/wire.rs`), so each case covers routing, deserialization, the handlers, the
//! persisted state and serialization — the same path a push client takes. The two push
//! steps before the completion are driven here rather than through `wire::given_layer`,
//! which completes the upload itself.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::repository_image_lifecycle::{given_repository, manifest_digest};
use crate::wire::{at, emulator, given_layer, layer_digest, ok, refused, REGISTRY_ID};

/// Open a layer upload against `repository` with `InitiateLayerUpload`, answering the
/// upload id the registry handed out.
///
/// `wire::given_layer` drives the whole push sequence and answers the registered digest;
/// this suite is about the COMPLETION, so it opens the upload and sends the part itself
/// and then makes the completion call under test.
async fn given_upload(router: &MultiServiceRouter, case: &str, repository: &str) -> String {
    let opened = ok(
        router,
        case,
        "InitiateLayerUpload",
        &json!({"repositoryName": repository}),
    )
    .await;
    opened["uploadId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: InitiateLayerUpload must hand back an uploadId"))
        .to_string()
}

/// Send `content` into `upload_id` as one whole layer part, the way a push client sends a
/// blob small enough not to be cut up.
async fn upload_whole_part(
    router: &MultiServiceRouter,
    case: &str,
    repository: &str,
    upload_id: &str,
    content: &[u8],
) {
    let last = i64::try_from(content.len()).expect("a test layer fits in an i64") - 1;
    ok(
        router,
        case,
        "UploadLayerPart",
        &json!({
            "repositoryName": repository,
            "uploadId": upload_id,
            "partFirstByte": 0,
            "partLastByte": last,
            // A blob travels base64-encoded on the awsJson1_1 wire, with the codec the
            // wire serializer itself uses.
            "layerPartBlob": aws_smithy_types::base64::encode(content),
        }),
    )
    .await;
}

/// One row of the asserted-set table: the case name, how many digests the request
/// asserts, which slot of that list carries the digest the registry computed, and whether
/// the request names `registryId` explicitly instead of leaving the default registry to
/// be assumed.
type AssertedSetCase<'a> = (&'a str, usize, usize, bool);

/// The bytes row `index` uploads: content that differs per row in both length and value,
/// so every row registers a layer of its own rather than re-registering one another row
/// already holds. The `% 256` is what makes the byte cast exact.
fn content_for(index: usize) -> Vec<u8> {
    (0..(17 + index * 29))
        .map(|byte| ((byte * 7 + index * 13 + 5) % 256) as u8)
        .collect()
}

/// `count` digests to assert, carrying `digest` in slot `position` and, in every other
/// slot, the content digest of bytes no upload ever sent. `position` outside `0..count`
/// is a set that does not carry `digest` at all — see [`asserted_digests_without`].
fn asserted_digests(count: usize, position: usize, digest: &str) -> Vec<String> {
    (0..count)
        .map(|slot| {
            if slot == position {
                digest.to_string()
            } else {
                layer_digest(format!("bytes uploaded nowhere, slot {slot}").as_bytes())
            }
        })
        .collect()
}

/// `count` digests to assert, none of them the registry's own: every slot is the content
/// digest of bytes no upload ever sent.
fn asserted_digests_without(count: usize) -> Vec<String> {
    asserted_digests(count, usize::MAX, "")
}

/// The completion request for `upload_id`, asserting `digests`.
fn completion(repository: &str, upload_id: &str, digests: &[String]) -> Value {
    json!({
        "repositoryName": repository,
        "uploadId": upload_id,
        "layerDigests": digests,
    })
}

/// Ask what `repository` holds for `digest`.
async fn availability(
    router: &MultiServiceRouter,
    case: &str,
    repository: &str,
    digest: &str,
) -> Value {
    ok(
        router,
        case,
        "BatchCheckLayerAvailability",
        &json!({"repositoryName": repository, "layerDigests": [digest]}),
    )
    .await
}

/// A completion whose asserted set holds the digest the registry computed registers the
/// layer and reports the upload it closed, for every size the model admits for that set
/// and every position the digest can take in it.
#[tokio::test]
async fn a_completion_succeeds_when_the_asserted_set_holds_the_registry_digest() {
    let cases: Vec<AssertedSetCase<'_>> = vec![
        ("one digest, the model's minimum", 1, 0, false),
        ("one digest, with the registry named", 1, 0, true),
        ("two digests, the registry's first", 2, 0, false),
        ("two digests, the registry's last", 2, 1, false),
        ("ten digests, the registry's in the middle", 10, 5, false),
        ("the model's maximum of 100, the registry's first", 100, 0, false),
        (
            "the model's maximum of 100, the registry's in the middle",
            100,
            49,
            false,
        ),
        ("the model's maximum of 100, the registry's last", 100, 99, false),
        ("the model's maximum of 100, with the registry named", 100, 73, true),
    ];

    let router = emulator();
    for (index, (case, asserted, position, name_registry)) in cases.into_iter().enumerate() {
        let repository = format!("asserted-set-{index}");
        given_repository(&router, &repository, json!({})).await;

        let content = content_for(index);
        let digest = layer_digest(&content);
        let size = i64::try_from(content.len()).expect("a test layer fits in an i64");
        let upload_id = given_upload(&router, case, &repository).await;
        upload_whole_part(&router, case, &repository, &upload_id, &content).await;

        let digests = asserted_digests(asserted, position, &digest);
        let mut request = completion(&repository, &upload_id, &digests);
        if name_registry {
            // "If you do not specify a registry, the default registry is assumed" — so
            // naming this registry explicitly is the same request.
            request["registryId"] = json!(REGISTRY_ID);
        }
        let completed = ok(&router, case, "CompleteLayerUpload", &request).await;
        assert_eq!(
            completed,
            json!({
                "registryId": REGISTRY_ID,
                "repositoryName": repository,
                "uploadId": upload_id,
                "layerDigest": digest,
            }),
            "case {case:?}: the completion reports this registry, the repository and upload \
             it closed and the digest it registered the layer under, got {completed:?}"
        );

        // The read-back: the layer is in the repository, with the bytes the registry
        // received — not a count any request stated.
        let held = availability(&router, case, &repository, &digest).await;
        assert_eq!(
            held["layers"],
            json!([{
                "layerDigest": digest,
                "layerAvailability": "AVAILABLE",
                "layerSize": size,
            }]),
            "case {case:?}: the registered layer is available with its {size} bytes, got \
             {held:?}"
        );

        // Closed means closed: the upload id resolves to nothing afterwards, however many
        // digests the second attempt asserts.
        refused(
            &router,
            case,
            "CompleteLayerUpload",
            &completion(&repository, &upload_id, &digests),
            "UploadNotFoundException",
        )
        .await;
    }
}

/// A completion whose asserted set does not hold the digest the registry computed is
/// refused, registers nothing, and leaves the upload open — so the client can assert the
/// right digest afterwards.
///
/// The rows differ along the form of the set that misses: a set of one, a set of the
/// model's maximum, and the near misses a broken client sends — the same hex in upper
/// case, the same hex under another algorithm label, one hex character changed, the
/// digest of a prefix of the bytes, and a digest carrying the right hex twice over.
#[tokio::test]
async fn a_completion_whose_asserted_set_misses_the_registry_digest_is_refused() {
    let router = emulator();
    let repository = "asserted-set-misses";
    given_repository(&router, repository, json!({})).await;

    let content = content_for(3);
    let digest = layer_digest(&content);
    let size = i64::try_from(content.len()).expect("a test layer fits in an i64");
    let hex = digest
        .split_once(':')
        .map(|(_, hex)| hex.to_string())
        .unwrap_or_else(|| panic!("the test digest is 'sha256:<hex>', got {digest:?}"));

    let upload_id = given_upload(&router, "misses", repository).await;
    upload_whole_part(&router, "misses", repository, &upload_id, &content).await;

    let mut cases: Vec<(&str, Vec<String>)> = vec![
        (
            "one digest, of bytes uploaded nowhere",
            asserted_digests_without(1),
        ),
        (
            "the model's maximum of 100, none of them the layer's",
            asserted_digests_without(100),
        ),
        ("the same hex in upper case", vec![format!("sha256:{}", hex.to_uppercase())]),
        ("the same hex under another algorithm", vec![format!("sha512:{hex}")]),
        (
            "one hex character changed",
            vec![format!(
                "sha256:{}{}",
                &hex[..hex.len() - 1],
                if hex.ends_with('0') { '1' } else { '0' }
            )],
        ),
        (
            "the digest of a prefix of the bytes",
            vec![layer_digest(&content[..content.len() / 2])],
        ),
        ("the right hex twice over", vec![format!("sha256:{hex}{hex}")]),
    ];
    // The near misses again, hidden in a set of the size the model admits: a set that
    // holds everything except the layer's own digest still misses it.
    let mut crowded: Vec<(&str, Vec<String>)> = Vec::new();
    for (case, digests) in &cases {
        let mut padded = asserted_digests_without(99);
        padded.extend(digests.iter().cloned());
        padded.truncate(100);
        crowded.push((*case, padded));
    }
    cases.extend(crowded);

    for (case, digests) in &cases {
        refused(
            &router,
            case,
            "CompleteLayerUpload",
            &completion(repository, &upload_id, digests),
            "InvalidLayerException",
        )
        .await;
        let held = availability(&router, case, repository, &digest).await;
        assert_eq!(
            at(&held, "/layers/0/layerAvailability"),
            json!("UNAVAILABLE"),
            "case {case:?}: a refused completion registers no layer, got {held:?}"
        );
    }

    // The upload survived every refusal with its bytes intact, so the digest the registry
    // computed still completes it.
    let completed = ok(
        &router,
        "the digest the registry computed, after the refusals",
        "CompleteLayerUpload",
        &completion(repository, &upload_id, std::slice::from_ref(&digest)),
    )
    .await;
    assert_eq!(
        at(&completed, "/layerDigest"),
        json!(digest),
        "the refused attempts cost the upload nothing, got {completed:?}"
    );
    let held = availability(&router, "after the refusals", repository, &digest).await;
    assert_eq!(
        held["layers"],
        json!([{
            "layerDigest": digest,
            "layerAvailability": "AVAILABLE",
            "layerSize": size,
        }]),
        "the layer carries every byte the upload received, got {held:?}"
    );
}

/// What the completion is for: a manifest is only pushable once every blob it references
/// has been completed into the repository, and the pushed image is then pulled back by
/// tag and by digest.
///
/// The rows differ along how many blobs the image is assembled from: a configuration blob
/// alone, and one, two or three filesystem layers beside it.
#[tokio::test]
async fn a_manifest_is_pushable_and_pullable_once_its_blobs_are_completed() {
    let cases: [(&str, usize); 4] = [
        ("a configuration blob alone", 0),
        ("one filesystem layer", 1),
        ("two filesystem layers", 2),
        ("three filesystem layers", 3),
    ];

    let router = emulator();
    for (index, (case, layers)) in cases.into_iter().enumerate() {
        let repository = format!("completed-push-{index}");
        given_repository(&router, &repository, json!({})).await;

        // The blobs of this image, as a push client prepares them: a configuration
        // document and `layers` filesystem layers, each with content of its own.
        let config = format!("{{\"architecture\":\"arm64\",\"case\":{index}}}").into_bytes();
        let layer_contents: Vec<Vec<u8>> = (0..layers)
            .map(|layer| content_for(index * 7 + layer + 1))
            .collect();

        let manifest = serde_json::to_string(&json!({
            "schemaVersion": 2,
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "config": {
                "mediaType": "application/vnd.oci.image.config.v1+json",
                "size": config.len(),
                "digest": layer_digest(&config),
            },
            "layers": layer_contents
                .iter()
                .map(|content| json!({
                    "mediaType": "application/vnd.oci.image.layer.v1.tar+gzip",
                    "size": content.len(),
                    "digest": layer_digest(content),
                }))
                .collect::<Vec<Value>>(),
        }))
        .expect("the manifest serialises");
        let push = json!({
            "repositoryName": repository,
            "imageManifest": manifest,
            "imageTag": "v1",
        });

        // Before the completions the repository holds none of the blobs, so the manifest
        // names layers it has not got.
        refused(
            &router,
            case,
            "PutImage",
            &push,
            "LayersNotFoundException",
        )
        .await;

        // Each blob through the documented push sequence, completion included.
        let mut digests: Vec<String> = Vec::new();
        for content in std::iter::once(&config).chain(layer_contents.iter()) {
            let registered = given_layer(&router, case, &repository, content).await;
            assert_eq!(
                registered,
                layer_digest(content),
                "case {case:?}: the blob is registered under the digest of its bytes, got \
                 {registered:?}"
            );
            digests.push(registered);
        }

        // With every blob completed, the very same manifest is pushable.
        ok(&router, case, "PutImage", &push).await;

        // And the image pulls back — by the tag it was pushed under and by its digest,
        // with the manifest the push stored.
        let digest = manifest_digest(&manifest);
        for (by, image_id) in [
            ("by tag", json!({"imageTag": "v1"})),
            ("by digest", json!({"imageDigest": digest})),
        ] {
            let case = format!("{case} ({by})");
            let pulled = ok(
                &router,
                &case,
                "BatchGetImage",
                &json!({"repositoryName": repository, "imageIds": [image_id]}),
            )
            .await;
            assert_eq!(
                at(&pulled, "/failures"),
                json!([]),
                "case {case:?}: the pushed image resolves, got {pulled:?}"
            );
            assert_eq!(
                at(&pulled, "/images/0/imageManifest"),
                json!(manifest),
                "case {case:?}: the pull reports the manifest the push stored, got {pulled:?}"
            );
            assert_eq!(
                at(&pulled, "/images/0/imageId/imageDigest"),
                json!(digest),
                "case {case:?}: the image is addressed by its manifest's digest, got {pulled:?}"
            );
            assert_eq!(
                at(&pulled, "/images/0/registryId"),
                json!(REGISTRY_ID),
                "case {case:?}: the image belongs to this registry, got {pulled:?}"
            );
        }

        // Every blob the manifest references is a layer of the repository, in the bytes
        // the registry received.
        for (slot, content) in std::iter::once(&config)
            .chain(layer_contents.iter())
            .enumerate()
        {
            let size = i64::try_from(content.len()).expect("a test blob fits in an i64");
            let held = availability(&router, case, &repository, &digests[slot]).await;
            assert_eq!(
                at(&held, "/layers/0/layerAvailability"),
                json!("AVAILABLE"),
                "case {case:?}: blob {slot} is a layer of the repository, got {held:?}"
            );
            assert_eq!(
                at(&held, "/layers/0/layerSize"),
                json!(size),
                "case {case:?}: blob {slot} carries the {size} bytes the registry received, \
                 got {held:?}"
            );
        }
    }
}
