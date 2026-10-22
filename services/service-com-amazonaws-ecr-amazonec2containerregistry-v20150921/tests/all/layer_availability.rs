// SPDX-License-Identifier: CC0-1.0

//! `BatchCheckLayerAvailability` over the wire.
//!
//! The rule under test: for each layer digest a request names, the answer is the
//! repository's own record of that blob — `AVAILABLE` with the size and media type the
//! repository stores for it when the repository holds it, a `MissingLayerDigest`
//! failure when no blob of that repository answers to the digest, and an
//! `InvalidLayerDigest` failure when the requested digest is not a content digest at
//! all. The blobs a repository holds are the ones its stored manifests reference: a
//! manifest cannot be pushed before its blobs are in the repository.
//!
//! Every case goes through the router, so it exercises routing, deserialization, the
//! handler, the persisted state and serialization — the same path a client takes.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request to ECR.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/ecr/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The account id every generated emulator vends, and therefore this registry's id.
const REGISTRY_ID: &str = "000000000000";

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire protocol").
const TARGET_PREFIX: &str = "AmazonEC2ContainerRegistry_V20150921";

/// The repository holding every pushed manifest in the availability table.
const HOLDER: &str = "app";

/// A repository that holds nothing: blobs are per-repository, so nothing is available
/// here.
const EMPTY: &str = "other";

/// What the service must answer for one requested digest.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Expected {
    /// The repository holds this blob, with the size and media type its descriptor
    /// declares (`None` where the manifest format carries neither).
    Available {
        size: Option<i64>,
        media_type: Option<&'static str>,
    },
    /// A well-formed digest no blob of the repository answers to.
    Missing,
    /// A requested digest that is not of the form `<algorithm>:<hex>`.
    Invalid,
}

/// One row of the availability table: the case name, the digest asked about, and the
/// answer the rule requires.
type Row = (String, String, Expected);

/// The `LayerFailureCode` values, from the model's `LayerFailureCode` enum
/// (`smithy_query(query="shape", shape="LayerFailureCode")` →
/// `["InvalidLayerDigest", "MissingLayerDigest"]`).
const MISSING_LAYER_DIGEST: &str = "MissingLayerDigest";
/// See [`MISSING_LAYER_DIGEST`].
const INVALID_LAYER_DIGEST: &str = "InvalidLayerDigest";

/// A content digest over `seed`: the `sha256:<hex>` form a registry addresses blobs by.
fn blob_digest(seed: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("sha256:{}", hex::encode(Sha256::digest(seed.as_bytes())))
}

/// The digest a registry addresses a manifest by, computed here independently of the
/// crate's own helper.
fn manifest_digest(manifest: &str) -> String {
    blob_digest(manifest)
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }
}

/// Call one operation of this service on the awsJson1_1 wire: `POST /` with
/// `X-Amz-Target: <prefix>.<operation>` and a JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> Answer {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "api.ecr.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_EU_WEST_1)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// A successful call, or a panic naming the case and what came back instead.
async fn ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let answer = call(router, operation, body).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from {operation}, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// Ask about `digests` in `repository`.
async fn check(router: &MultiServiceRouter, case: &str, repository: &str, digests: &[String]) -> Value {
    ok(
        router,
        case,
        "BatchCheckLayerAvailability",
        &json!({"repositoryName": repository, "layerDigests": digests}),
    )
    .await
}

/// The manifests pushed into [`HOLDER`], and the availability table they imply.
///
/// The manifest formats are the ones a registry is handed, and each spells its blobs
/// differently: the Docker image manifest V2 schema 2 and the OCI image manifest carry
/// a `config` descriptor plus `layers` descriptors (`digest`, `size`, `mediaType`); the
/// V2 schema 1 lists `fsLayers[].blobSum`, a digest and nothing else; an OCI image
/// index lists `manifests`, which are other images rather than this one's blobs.
fn pushed_manifests() -> (Vec<String>, Vec<Row>) {
    let shared = blob_digest("shared-layer");
    let schema2_config = blob_digest("schema2-config");
    let schema2_layer = blob_digest("schema2-layer");
    let oci_config = blob_digest("oci-config");
    let oci_layer = blob_digest("oci-layer");
    let schema1_only = blob_digest("schema1-only");
    let child_manifest = blob_digest("child-manifest");

    let schema2 = serde_json::to_string(&json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.docker.distribution.manifest.v2+json",
        "config": {
            "mediaType": "application/vnd.docker.container.image.v1+json",
            "size": 7023,
            "digest": schema2_config,
        },
        "layers": [
            {
                "mediaType": "application/vnd.docker.image.rootfs.diff.tar.gzip",
                "size": 1,
                "digest": shared,
            },
            {
                "mediaType": "application/vnd.docker.image.rootfs.diff.tar.gzip",
                "size": 1_048_576,
                "digest": schema2_layer,
            },
        ],
    }))
    .expect("the schema 2 manifest serialises");
    let oci = serde_json::to_string(&json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.manifest.v1+json",
        "config": {
            "mediaType": "application/vnd.oci.image.config.v1+json",
            "size": 581,
            "digest": oci_config,
        },
        "layers": [{
            "mediaType": "application/vnd.oci.image.layer.v1.tar+zstd",
            "size": 32_768,
            "digest": oci_layer,
        }],
    }))
    .expect("the OCI manifest serialises");
    let schema1 = serde_json::to_string(&json!({
        "schemaVersion": 1,
        "name": "app",
        "tag": "schema1",
        "fsLayers": [{"blobSum": shared}, {"blobSum": schema1_only}],
    }))
    .expect("the schema 1 manifest serialises");
    let index = serde_json::to_string(&json!({
        "schemaVersion": 2,
        "mediaType": "application/vnd.oci.image.index.v1+json",
        "manifests": [{
            "mediaType": "application/vnd.oci.image.manifest.v1+json",
            "size": 7143,
            "digest": child_manifest,
        }],
    }))
    .expect("the index serialises");

    let rows: Vec<Row> = vec![
        (
            "the schema 2 config blob".to_string(),
            schema2_config,
            Expected::Available {
                size: Some(7023),
                media_type: Some("application/vnd.docker.container.image.v1+json"),
            },
        ),
        (
            "a schema 2 layer".to_string(),
            schema2_layer,
            Expected::Available {
                size: Some(1_048_576),
                media_type: Some("application/vnd.docker.image.rootfs.diff.tar.gzip"),
            },
        ),
        (
            // Two manifests reference this blob, and only one of them says how big it
            // is: the record that knows the size is the one reported.
            "a blob a schema 2 and a schema 1 manifest share".to_string(),
            shared,
            Expected::Available {
                size: Some(1),
                media_type: Some("application/vnd.docker.image.rootfs.diff.tar.gzip"),
            },
        ),
        (
            "the OCI config blob".to_string(),
            oci_config,
            Expected::Available {
                size: Some(581),
                media_type: Some("application/vnd.oci.image.config.v1+json"),
            },
        ),
        (
            "an OCI layer".to_string(),
            oci_layer,
            Expected::Available {
                size: Some(32_768),
                media_type: Some("application/vnd.oci.image.layer.v1.tar+zstd"),
            },
        ),
        (
            // A `blobSum` names the blob and nothing else, so neither the size nor the
            // media type is reported for it.
            "a schema 1 fsLayers blob".to_string(),
            schema1_only,
            Expected::Available {
                size: None,
                media_type: None,
            },
        ),
        (
            // An index entry is another manifest, not a blob of this image.
            "a manifest an index references".to_string(),
            child_manifest,
            Expected::Missing,
        ),
        (
            "a well-formed sha256 digest nothing references".to_string(),
            blob_digest("never-pushed"),
            Expected::Missing,
        ),
        (
            "a digest of another algorithm, nothing references".to_string(),
            "sha512:0123456789abcdefABCDEF".to_string(),
            Expected::Missing,
        ),
        (
            "the digest of a stored manifest, which is not a blob of it".to_string(),
            manifest_digest(&schema2),
            Expected::Missing,
        ),
        (
            "a digest with no algorithm separator".to_string(),
            "not-a-digest".to_string(),
            Expected::Invalid,
        ),
        (
            "an empty digest".to_string(),
            String::new(),
            Expected::Invalid,
        ),
        (
            "an algorithm with no encoded value".to_string(),
            "sha256:".to_string(),
            Expected::Invalid,
        ),
        (
            "an encoded value with no algorithm".to_string(),
            ":abcdef".to_string(),
            Expected::Invalid,
        ),
        (
            "an encoded value that is not hexadecimal".to_string(),
            "sha256:zzzzzz".to_string(),
            Expected::Invalid,
        ),
    ];

    (vec![schema2, oci, schema1, index], rows)
}

/// Every requested digest is answered from the repository's stored blobs, in the order
/// it was asked about.
///
/// The rows differ along what the rule generalises over: which manifest format declares
/// the blob, whether that format carries a size and a media type, whether the digest is
/// referenced at all, which algorithm it names, and the ways a digest can be malformed.
#[tokio::test]
async fn every_requested_digest_is_answered_from_the_repositorys_stored_blobs() {
    let router = emulator();
    let (manifests, rows) = pushed_manifests();
    for repository in [HOLDER, EMPTY] {
        ok(
            &router,
            repository,
            "CreateRepository",
            &json!({"repositoryName": repository}),
        )
        .await;
    }
    for (index, manifest) in manifests.iter().enumerate() {
        ok(
            &router,
            "seed",
            "PutImage",
            &json!({
                "repositoryName": HOLDER,
                "imageManifest": manifest,
                "imageTag": format!("v{index}"),
            }),
        )
        .await;
    }

    let digests: Vec<String> = rows.iter().map(|(_, digest, _)| digest.clone()).collect();
    let answer = check(&router, "the whole table in one batch", HOLDER, &digests).await;
    let layers = answer["layers"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("the answer must carry a layers list, got {answer:?}"));
    let failures = answer["failures"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("the answer must carry a failures list, got {answer:?}"));

    // Each list reports the references it answers in the order they were asked about,
    // so the n-th available digest is the n-th entry of `layers`.
    let mut next_layer = 0;
    let mut next_failure = 0;
    for (case, digest, expected) in &rows {
        match expected {
            Expected::Available { size, media_type } => {
                let mut wanted = json!({"layerDigest": digest, "layerAvailability": "AVAILABLE"});
                if let Some(size) = size {
                    wanted["layerSize"] = json!(size);
                }
                if let Some(media_type) = media_type {
                    wanted["mediaType"] = json!(media_type);
                }
                let got = layers.get(next_layer).cloned().unwrap_or(Value::Null);
                next_layer += 1;
                assert_eq!(
                    got, wanted,
                    "case {case:?}: expected layers[{}] to be {wanted}, got {got:?}",
                    next_layer - 1
                );
            }
            Expected::Missing | Expected::Invalid => {
                let code = if *expected == Expected::Missing {
                    MISSING_LAYER_DIGEST
                } else {
                    INVALID_LAYER_DIGEST
                };
                let got = failures.get(next_failure).cloned().unwrap_or(Value::Null);
                next_failure += 1;
                assert_eq!(
                    got["layerDigest"],
                    json!(digest),
                    "case {case:?}: expected failures[{}] to name {digest:?}, got {got:?}",
                    next_failure - 1
                );
                assert_eq!(
                    got["failureCode"],
                    json!(code),
                    "case {case:?}: expected failureCode {code}, got {got:?}"
                );
            }
        }
    }
    assert_eq!(
        layers.len(),
        next_layer,
        "every available digest is reported exactly once, got {layers:?}"
    );
    assert_eq!(
        failures.len(),
        next_failure,
        "every unavailable digest is reported exactly once, got {failures:?}"
    );

    // Blobs belong to the repository they were pushed to: the very same digests are
    // missing from a repository that holds no manifest referencing them.
    let elsewhere = check(&router, "another repository", EMPTY, &digests).await;
    assert_eq!(
        elsewhere["layers"],
        json!([]),
        "a repository holding no image holds no blob, got {:?}",
        elsewhere["layers"]
    );
    assert_eq!(
        elsewhere["failures"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        digests.len(),
        "every digest must be reported as a failure there, got {:?}",
        elsewhere["failures"]
    );

    // Checking availability reports; it changes nothing a later caller sees.
    let described = ok(
        &router,
        "after the check",
        "DescribeImages",
        &json!({"repositoryName": HOLDER}),
    )
    .await;
    let again = check(&router, "the same batch again", HOLDER, &digests).await;
    assert_eq!(
        again, answer,
        "a repeated check must answer the same, got {again:?}"
    );
    let described_again = ok(
        &router,
        "after the second check",
        "DescribeImages",
        &json!({"repositoryName": HOLDER}),
    )
    .await;
    assert_eq!(
        described_again, described,
        "checking layer availability must not change the stored images"
    );
}

/// One answer per reference, whatever the batch's size — and the modelled bounds on
/// `layerDigests` (length 1..100) are what decides which batches are accepted at all.
#[tokio::test]
async fn a_batch_is_answered_reference_by_reference_within_the_modelled_bounds() {
    let router = emulator();
    ok(
        &router,
        HOLDER,
        "CreateRepository",
        &json!({"repositoryName": HOLDER}),
    )
    .await;
    let held = blob_digest("held-layer");
    let manifest = serde_json::to_string(&json!({
        "schemaVersion": 2,
        "layers": [{"size": 12, "digest": held}],
    }))
    .expect("the manifest serialises");
    ok(
        &router,
        "seed",
        "PutImage",
        &json!({"repositoryName": HOLDER, "imageManifest": manifest, "imageTag": "v1"}),
    )
    .await;

    // A digest named more than once is answered once per reference.
    for repeats in [1_usize, 2, 7] {
        let case = format!("the same digest {repeats} time(s)");
        let digests = vec![held.clone(); repeats];
        let answer = check(&router, &case, HOLDER, &digests).await;
        assert_eq!(
            answer["layers"].as_array().map(Vec::len).unwrap_or_default(),
            repeats,
            "case {case:?}: every reference is answered, got {:?}",
            answer["layers"]
        );
    }

    // The published maximum batch: one held digest and 99 that nothing references.
    let mut full = vec![held.clone()];
    full.extend((1..100).map(|n| blob_digest(&format!("absent-{n}"))));
    let answer = check(&router, "a batch of 100", HOLDER, &full).await;
    assert_eq!(
        answer["layers"].as_array().map(Vec::len).unwrap_or_default(),
        1,
        "the one held digest is available, got {:?}",
        answer["layers"]
    );
    assert_eq!(
        answer["failures"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        99,
        "the other 99 are failures, got {:?}",
        answer["failures"]
    );

    // And one past either published bound is refused by the modelled constraint
    // (`layerDigests` is `@length(min: 1, max: 100)`), not answered.
    let mut over = full.clone();
    over.push(blob_digest("absent-100"));
    for (case, digests) in [("an empty batch", Vec::new()), ("a batch of 101", over)] {
        let answer = call(
            &router,
            "BatchCheckLayerAvailability",
            &json!({"repositoryName": HOLDER, "layerDigests": digests}),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
    }
}

/// A call that names no repository of this registry is refused with the modelled error,
/// whatever the digests it asks about.
#[tokio::test]
async fn a_check_against_a_repository_this_registry_has_not_got_is_refused() {
    let router = emulator();
    ok(
        &router,
        HOLDER,
        "CreateRepository",
        &json!({"repositoryName": HOLDER}),
    )
    .await;
    let digest = blob_digest("whatever");

    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a repository never created",
            json!({"repositoryName": "no-such-repo", "layerDigests": [digest]}),
            "RepositoryNotFoundException",
        ),
        (
            "a namespaced repository never created",
            json!({"repositoryName": "team-a/no-such-repo", "layerDigests": [digest]}),
            "RepositoryNotFoundException",
        ),
        (
            "an existing repository in another account's registry",
            json!({
                "repositoryName": HOLDER,
                "registryId": "123456789012",
                "layerDigests": [digest],
            }),
            "InvalidParameterException",
        ),
    ];

    for (case, request, expected) in cases {
        let answer = call(&router, "BatchCheckLayerAvailability", &request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.error_code().ends_with(expected),
            "case {case:?}: expected {expected}, got {:?}",
            answer.body
        );
    }

    // This registry's own id names this registry, so it is accepted.
    let answer = ok(
        &router,
        "this registry named explicitly",
        "BatchCheckLayerAvailability",
        &json!({
            "repositoryName": HOLDER,
            "registryId": REGISTRY_ID,
            "layerDigests": [digest.clone()],
        }),
    )
    .await;
    assert_eq!(
        answer["failures"][0]["layerDigest"],
        json!(digest),
        "the empty repository holds no blob, got {answer:?}"
    );
}
