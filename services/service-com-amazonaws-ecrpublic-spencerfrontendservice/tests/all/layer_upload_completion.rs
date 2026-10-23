// SPDX-License-Identifier: CC0-1.0

//! Completing an ECR Public layer upload and reading the committed layer back, driven end
//! to end over the wire.
//!
//! The rule under test: `CompleteLayerUpload` CONSUMES the upload a client filled and
//! commits the layer its parts made up to the named repository, whose identity is the
//! `sha256` digest the registry computes over those bytes — reported back as
//! `layerDigest` beside the registry, repository and upload the request named — and
//! `BatchCheckLayerAvailability` then answers every requested digest from that committed
//! set: a digest of the `<algorithm>:<hex>` form the model's `LayerDigest` declares
//! addresses a layer, so it is answered with a layer — `AVAILABLE` with the layer's byte
//! count when this repository holds it and `UNAVAILABLE` when it does not — while an
//! entry outside that form addresses no layer and is an `InvalidLayerDigest` failure. An
//! upload that received no part commits nothing and is
//! refused with `EmptyUploadException`; so is a completion whose requested digests name a
//! different layer (`InvalidLayerException`), one against a layer the repository already
//! holds (`LayerAlreadyExistsException`), and one naming an upload, repository or registry
//! that is not there.
//!
//! Every case goes through the front door a client uses: `POST /` with
//! `X-Amz-Target: SpencerFrontendService.<Operation>` and an `awsJson1_1` body
//! (`OPERATIONS.md`, "Wire protocol"). The digests the cases expect are computed in the
//! test with its own `sha2`, never copied from a recording or read out of the response, so
//! a registry that digested something other than the uploaded bytes cannot pass.
//!
//! The wire plumbing and the upload-store reader are the session suite's shared harness
//! (`tests/all/layer_upload_sessions.rs`).

use std::sync::Arc;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use smithy_server_runtime::MultiServiceRouter;

use crate::layer_upload_sessions::{
    call, error_code, given_repository, ok, service_with_engine, stored_uploads, REGISTRY_ALIAS,
    REGISTRY_ID,
};

/// An upload id with the modelled `UploadId` layout
/// (`^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`) that
/// this registry never minted.
const UNKNOWN_UPLOAD_ID: &str = "0123abcd-4567-89ef-0123-456789abcdef";

/// The `sha256:<hex>` digest of `bytes`, computed in the test: the derivation
/// `CompleteLayerUpload$layerDigest` ("the sha256 digest of the image layer") states.
pub(crate) fn digest_of(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

/// `len` deterministic bytes that depend on `seed`, so two layers of one case differ and a
/// commit that dropped or reordered bytes cannot digest to the expected value.
pub(crate) fn layer_bytes(len: usize, seed: usize) -> Vec<u8> {
    (0..len)
        .map(|index| u8::try_from((index * 37 + seed * 11) % 256).unwrap_or_default())
        .collect()
}

/// A repository of this registry plus one upload opened against it.
pub(crate) async fn given_upload(
    router: &MultiServiceRouter,
    case: &str,
    repository: &str,
) -> String {
    given_repository(router, case, repository).await;
    let answer = ok(
        router,
        "InitiateLayerUpload",
        json!({"repositoryName": repository}),
        case,
    )
    .await;
    answer["uploadId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: InitiateLayerUpload answered {answer}"))
        .to_string()
}

/// Send `layer` to `upload_id` as `part_count` consecutive parts, each of which must be
/// accepted: the precondition a completion is measured against.
pub(crate) async fn given_parts(
    router: &MultiServiceRouter,
    repository: &str,
    upload_id: &str,
    layer: &[u8],
    part_count: usize,
    case: &str,
) {
    let part_len = layer.len().div_ceil(part_count.max(1));
    let mut offset = 0usize;
    while offset < layer.len() {
        let end = (offset + part_len).min(layer.len());
        let body = json!({
            "repositoryName": repository,
            "uploadId": upload_id,
            "partFirstByte": i64::try_from(offset).unwrap_or(i64::MAX),
            "partLastByte": i64::try_from(end - 1).unwrap_or(i64::MAX),
            // awsJson1_1 carries a blob as base64 of its bytes, through the SDK's own
            // standard-alphabet codec — the one the service's deserializer uses.
            "layerPartBlob": aws_smithy_types::base64::encode(&layer[offset..end]),
        });
        let (status, parsed) = call(router, "UploadLayerPart", body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: seeding bytes {offset}..{end} must be accepted, got {status} \
             with {parsed}"
        );
        offset = end;
    }
}

/// One `BatchCheckLayerAvailability` answer for `digests` against `repository`.
pub(crate) async fn availability(
    router: &MultiServiceRouter,
    repository: &str,
    digests: &[String],
    case: &str,
) -> Value {
    ok(
        router,
        "BatchCheckLayerAvailability",
        json!({"repositoryName": repository, "layerDigests": digests}),
        case,
    )
    .await
}

/// The `(layerDigest, layerAvailability, layerSize)` triples a report lists under
/// `layers`, in the order it listed them.
pub(crate) fn reported_layers(report: &Value, case: &str) -> Vec<(String, String, Option<i64>)> {
    report["layers"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a layers array, got {report}"))
        .iter()
        .map(|layer| {
            (
                layer["layerDigest"].as_str().unwrap_or_default().to_string(),
                layer["layerAvailability"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                layer["layerSize"].as_i64(),
            )
        })
        .collect()
}

/// The `(layerDigest, failureCode)` pairs a report lists under `failures`, in order.
fn reported_failures(report: &Value, case: &str) -> Vec<(String, String)> {
    report["failures"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a failures array, got {report}"))
        .iter()
        .map(|failure| {
            (
                failure["layerDigest"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                failure["failureCode"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// A completed upload commits exactly the layer its parts carried: the response reports
/// the digest the registry computed over those bytes beside the registry, repository and
/// upload the request named; the layer is then `AVAILABLE` with its own byte count; and
/// the upload id is consumed, so completing it again finds no upload.
///
/// The rows differ along what the rule generalises over — how many bytes the layer carries
/// (one byte, odd sizes, a kibibyte, a mebibyte), how many parts they arrived in, how the
/// request names the registry (not at all, by account id, by alias), and how the requested
/// digest is spelled (lower-case hex, upper-case hex, named among other digests).
#[tokio::test]
async fn completing_an_upload_commits_the_layer_its_parts_carried() {
    // (case, repositoryName, layer length, how many parts, the `registryId` named)
    let rows: Vec<(&str, &str, usize, usize, Option<&str>)> = vec![
        ("one byte in one part, no registryId", "one-byte", 1, 1, None),
        (
            "an odd size in one part, registryId by account id",
            "odd-size",
            7,
            1,
            Some(REGISTRY_ID),
        ),
        (
            "a kibibyte in two parts, registryId by alias",
            "two-parts",
            1024,
            2,
            Some(REGISTRY_ALIAS),
        ),
        (
            "an uneven size in three parts",
            "team/three-parts",
            3001,
            3,
            None,
        ),
        ("a mebibyte in four parts", "big-layer", 1 << 20, 4, None),
    ];

    let (router, engine) = service_with_engine();
    for (index, (case, repository, length, part_count, registry)) in rows.into_iter().enumerate() {
        let layer = layer_bytes(length, index + 1);
        let expected_digest = digest_of(&layer);
        let upload_id = given_upload(&router, case, repository).await;
        given_parts(&router, repository, &upload_id, &layer, part_count, case).await;

        let mut request = json!({
            "repositoryName": repository,
            "uploadId": upload_id,
            // The digest the client claims, beside one that names a different layer: a
            // completion is accepted when the uploaded layer is among the digests named.
            "layerDigests": [digest_of(b"some other layer"), expected_digest.clone()],
        });
        if let Some(registry) = registry {
            request["registryId"] = json!(registry);
        }
        let completed = ok(&router, "CompleteLayerUpload", request, case).await;

        assert_eq!(
            completed["layerDigest"],
            json!(expected_digest),
            "case {case:?}: expected the digest of the uploaded bytes, got {completed}"
        );
        assert_eq!(
            completed["registryId"],
            json!(REGISTRY_ID),
            "case {case:?}: the completion names the registry it resolved to, got {completed}"
        );
        assert_eq!(
            completed["repositoryName"],
            json!(repository),
            "case {case:?}: the completion echoes the repository named, got {completed}"
        );
        assert_eq!(
            completed["uploadId"],
            json!(upload_id),
            "case {case:?}: the completion echoes the upload named, got {completed}"
        );

        // Read the committed layer back through the reporting operation, twice: a report
        // must not change what the next caller sees.
        for read in 1..=2 {
            let report =
                availability(&router, repository, std::slice::from_ref(&expected_digest), case)
                    .await;
            assert_eq!(
                reported_layers(&report, case),
                vec![(
                    expected_digest.clone(),
                    "AVAILABLE".to_string(),
                    Some(i64::try_from(length).unwrap_or(i64::MAX))
                )],
                "case {case:?}: read {read} must report the committed layer, got {report}"
            );
            assert_eq!(
                reported_failures(&report, case),
                Vec::<(String, String)>::new(),
                "case {case:?}: read {read} must report no failure, got {report}"
            );
            assert!(
                report["layers"][0].get("mediaType").is_none()
                    || report["layers"][0]["mediaType"].is_null(),
                "case {case:?}: a layer upload states no media type, got {report}"
            );
        }

        // The upload id is consumed by the completion.
        assert!(
            !stored_uploads(&engine).contains_key(&upload_id),
            "case {case:?}: the completed upload {upload_id:?} must be gone from the store"
        );
        let (status, parsed) = call(
            &router,
            "CompleteLayerUpload",
            json!({
                "repositoryName": repository,
                "uploadId": upload_id,
                "layerDigests": [expected_digest.clone()],
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: completing a consumed upload is refused, got {status} with {parsed}"
        );
        assert!(
            error_code(&parsed).contains("UploadNotFoundException"),
            "case {case:?}: expected UploadNotFoundException, got {parsed}"
        );
    }

    // An upper-case spelling of the same digest names the same value, so the layer a
    // repository holds is reported for it too.
    let case = "an upper-case hex digest of a committed layer";
    let layer = layer_bytes(64, 99);
    let repository = "upper-case-hex";
    let upload_id = given_upload(&router, case, repository).await;
    given_parts(&router, repository, &upload_id, &layer, 1, case).await;
    let lower = digest_of(&layer);
    let upper = format!(
        "sha256:{}",
        lower.trim_start_matches("sha256:").to_ascii_uppercase()
    );
    let completed = ok(
        &router,
        "CompleteLayerUpload",
        json!({
            "repositoryName": repository,
            "uploadId": upload_id,
            "layerDigests": [upper.clone()],
        }),
        case,
    )
    .await;
    assert_eq!(
        completed["layerDigest"],
        json!(lower),
        "case {case:?}: the committed digest is the one the registry computed, got {completed}"
    );
    let report = availability(&router, repository, std::slice::from_ref(&upper), case).await;
    assert_eq!(
        reported_layers(&report, case),
        vec![(upper, "AVAILABLE".to_string(), Some(64))],
        "case {case:?}: the requested spelling is echoed for the layer it names, got {report}"
    );

    // The ALGORITHM half is a token, not a case-insensitive rendering: a differently cased
    // algorithm names another algorithm, so the layer this repository holds is not its.
    let other_algorithm = format!(
        "SHA256:{}",
        lower.trim_start_matches("sha256:")
    );
    let report = availability(
        &router,
        repository,
        std::slice::from_ref(&other_algorithm),
        case,
    )
    .await;
    assert_eq!(
        reported_layers(&report, case),
        vec![(other_algorithm, "UNAVAILABLE".to_string(), None)],
        "case {case:?}: a differently cased algorithm names no held layer, so that layer is \
         reported unavailable, got {report}"
    );
    assert_eq!(
        reported_failures(&report, case),
        Vec::new(),
        "case {case:?}: the entry addresses a layer, so it is no failure, got {report}"
    );
}

/// An upload that received no layer part completes nothing: it is refused with
/// `EmptyUploadException`, the digest never enters the repository's layer set, and the
/// upload is still open — the parts that never came can still be uploaded and the same id
/// then completes.
#[tokio::test]
async fn an_upload_with_no_parts_is_refused_as_empty_and_commits_nothing() {
    let (router, engine) = service_with_engine();

    // The rows differ along the repository's name form and along how many digests the
    // request names: an empty upload has no layer whatever the request claims.
    let rows: Vec<(&str, &str, Vec<String>)> = vec![
        (
            "one claimed digest",
            "empty-one",
            vec![format!("sha256:{}", "1".repeat(64))],
        ),
        (
            "the digest of the empty byte string",
            "empty-of-nothing",
            vec![digest_of(b"")],
        ),
        (
            "a nested name and several claimed digests",
            "team/empty-many",
            vec![
                digest_of(b"a"),
                digest_of(b"b"),
                format!("sha256:{}", "f".repeat(64)),
            ],
        ),
    ];

    for (case, repository, claimed) in rows {
        let upload_id = given_upload(&router, case, repository).await;
        let (status, parsed) = call(
            &router,
            "CompleteLayerUpload",
            json!({
                "repositoryName": repository,
                "uploadId": upload_id,
                "layerDigests": claimed,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: an empty upload is refused, got {status} with {parsed}"
        );
        assert!(
            error_code(&parsed).contains("EmptyUploadException"),
            "case {case:?}: expected EmptyUploadException, got {parsed}"
        );
        let actual = parsed["message"].as_str().unwrap_or_default().to_string();
        let expected = format!(
            "The upload with id '{upload_id}' in the repository with name '{repository}' in \
             registry with id '{REGISTRY_ID}' cannot be completed because it does not have \
             any parts"
        );
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );

        // Nothing entered the layer set: every claimed digest is still unavailable.
        let report = availability(&router, repository, &claimed, case).await;
        assert_eq!(
            reported_layers(&report, case),
            claimed
                .iter()
                .map(|digest| (digest.clone(), "UNAVAILABLE".to_string(), None))
                .collect::<Vec<_>>(),
            "case {case:?}: a refused completion commits no layer, got {report}"
        );
        assert_eq!(
            reported_failures(&report, case).len(),
            0,
            "case {case:?}: every claimed digest addresses a layer, so none is a failure, \
             got {report}"
        );

        // The upload is still open: it is the client's parts that are missing, not the
        // upload. Uploading them and completing again succeeds.
        assert!(
            stored_uploads(&engine).contains_key(&upload_id),
            "case {case:?}: the refused upload {upload_id:?} must still be open"
        );
        let layer = layer_bytes(33, repository.len());
        given_parts(&router, repository, &upload_id, &layer, 1, case).await;
        let completed = ok(
            &router,
            "CompleteLayerUpload",
            json!({
                "repositoryName": repository,
                "uploadId": upload_id,
                "layerDigests": [digest_of(&layer)],
            }),
            case,
        )
        .await;
        assert_eq!(
            completed["layerDigest"],
            json!(digest_of(&layer)),
            "case {case:?}: the same upload completes once its parts arrive, got {completed}"
        );
    }
}

/// A completion whose requested digests name a layer other than the uploaded one is
/// refused with `InvalidLayerException`, and commits nothing: the registry's own digest of
/// the bytes it received is what decides.
#[tokio::test]
async fn a_completion_that_names_another_layer_is_refused_as_an_invalid_layer() {
    let (router, _engine) = service_with_engine();
    let layer = layer_bytes(128, 5);
    let truth = digest_of(&layer);

    // Rows: the ways a claimed digest can differ from the layer's — a different layer's
    // digest, an all-zero digest, a digest of the same hex under another algorithm name,
    // a truncated hex string, and several wrong digests at once.
    let rows: Vec<(&str, Vec<String>)> = vec![
        ("another layer's digest", vec![digest_of(b"not this layer")]),
        (
            "an all-zero digest",
            vec![format!("sha256:{}", "0".repeat(64))],
        ),
        (
            "the right hex under another algorithm",
            vec![format!(
                "sha512:{}",
                truth.trim_start_matches("sha256:")
            )],
        ),
        (
            "the right hex, truncated",
            vec![truth.trim_start_matches("sha256:")[..32].to_string()]
                .into_iter()
                .map(|hex| format!("sha256:{hex}"))
                .collect(),
        ),
        (
            "several wrong digests",
            vec![digest_of(b"x"), digest_of(b"y"), digest_of(b"z")],
        ),
    ];

    for (index, (case, claimed)) in rows.into_iter().enumerate() {
        let repository = format!("mismatch-{index}");
        let upload_id = given_upload(&router, case, &repository).await;
        given_parts(&router, &repository, &upload_id, &layer, 1, case).await;

        let (status, parsed) = call(
            &router,
            "CompleteLayerUpload",
            json!({
                "repositoryName": repository,
                "uploadId": upload_id,
                "layerDigests": claimed,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a digest disagreement is refused, got {status} with {parsed}"
        );
        assert!(
            error_code(&parsed).contains("InvalidLayerException"),
            "case {case:?}: expected InvalidLayerException, got {parsed}"
        );

        // Neither the claimed digests nor the layer's own digest were committed.
        let report =
            availability(&router, &repository, std::slice::from_ref(&truth), case).await;
        assert_eq!(
            reported_layers(&report, case),
            vec![(truth.clone(), "UNAVAILABLE".to_string(), None)],
            "case {case:?}: a refused completion commits no layer, got {report}"
        );
    }
}

/// Committing a layer a repository already holds is refused with
/// `LayerAlreadyExistsException`, and the layer the repository holds is untouched.
#[tokio::test]
async fn completing_a_layer_the_repository_already_holds_is_refused() {
    let (router, _engine) = service_with_engine();
    let case = "the same layer uploaded twice";
    let repository = "already-held";
    let layer = layer_bytes(256, 3);
    let digest = digest_of(&layer);

    let first = given_upload(&router, case, repository).await;
    given_parts(&router, repository, &first, &layer, 2, case).await;
    ok(
        &router,
        "CompleteLayerUpload",
        json!({
            "repositoryName": repository,
            "uploadId": first,
            "layerDigests": [digest.clone()],
        }),
        case,
    )
    .await;

    // A second upload of the same bytes digests to the same layer, which the repository
    // already holds.
    let second = ok(
        &router,
        "InitiateLayerUpload",
        json!({"repositoryName": repository}),
        case,
    )
    .await["uploadId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    given_parts(&router, repository, &second, &layer, 1, case).await;
    let (status, parsed) = call(
        &router,
        "CompleteLayerUpload",
        json!({
            "repositoryName": repository,
            "uploadId": second,
            "layerDigests": [digest.clone()],
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: a layer already held is refused, got {status} with {parsed}"
    );
    assert!(
        error_code(&parsed).contains("LayerAlreadyExistsException"),
        "case {case:?}: expected LayerAlreadyExistsException, got {parsed}"
    );

    // The layer the first completion committed still reports its own size.
    let report = availability(&router, repository, std::slice::from_ref(&digest), case).await;
    assert_eq!(
        reported_layers(&report, case),
        vec![(digest, "AVAILABLE".to_string(), Some(256))],
        "case {case:?}: the held layer is unchanged by the refusal, got {report}"
    );
}

/// One row of the scoping table: the case, the `repositoryName` and `uploadId` the
/// request names, the `registryId` it names (if any), and the modelled error code
/// expected — `None` for a completion that must be accepted.
type ScopeRow<'a> = (&'a str, &'a str, &'a str, Option<&'a str>, Option<&'a str>);

/// What a completion is addressed to has to exist: the upload id, the repository it was
/// issued for, and the registry the request names. Each row names one of them wrongly and
/// expects the modelled refusal for it; the last rows name them all correctly.
#[tokio::test]
async fn a_completion_is_scoped_to_the_upload_repository_and_registry_it_names() {
    let (router, _engine) = service_with_engine();
    let case = "the scoping table";
    let layer = layer_bytes(48, 8);
    let digest = digest_of(&layer);

    // Two repositories, each with a filled upload, so an upload id can be addressed
    // against the repository it was NOT issued for.
    let filled = given_upload(&router, case, "scoped-one").await;
    given_parts(&router, "scoped-one", &filled, &layer, 1, case).await;
    let other = given_upload(&router, case, "scoped-two").await;
    given_parts(&router, "scoped-two", &other, &layer, 1, case).await;

    // (case, repositoryName, uploadId, registryId, expected error code — `None` for a
    // completion that must be accepted)
    let rows: Vec<ScopeRow<'_>> = vec![
        (
            "a repository this registry does not hold",
            "no-such-repository",
            &filled,
            None,
            Some("RepositoryNotFoundException"),
        ),
        (
            "a registry that is not this one",
            "scoped-one",
            &filled,
            Some("123456789012"),
            Some("RegistryNotFoundException"),
        ),
        (
            "a registry alias that is not this one",
            "scoped-one",
            &filled,
            Some("someone-else"),
            Some("RegistryNotFoundException"),
        ),
        (
            "an upload id this registry never minted",
            "scoped-one",
            UNKNOWN_UPLOAD_ID,
            None,
            Some("UploadNotFoundException"),
        ),
        (
            "an upload issued for another repository",
            "scoped-one",
            &other,
            None,
            Some("UploadNotFoundException"),
        ),
        (
            "every name correct, registry by account id",
            "scoped-one",
            &filled,
            Some(REGISTRY_ID),
            None,
        ),
        (
            "every name correct, registry by alias",
            "scoped-two",
            &other,
            Some(REGISTRY_ALIAS),
            None,
        ),
    ];

    for (row, repository, upload_id, registry, expected) in rows {
        let mut request = json!({
            "repositoryName": repository,
            "uploadId": upload_id,
            "layerDigests": [digest.clone()],
        });
        if let Some(registry) = registry {
            request["registryId"] = json!(registry);
        }
        let (status, parsed) = call(&router, "CompleteLayerUpload", request).await;
        match expected {
            Some(code) => {
                assert_eq!(
                    status, 400,
                    "case {row:?}: expected 400 {code}, got {status} with {parsed}"
                );
                assert!(
                    error_code(&parsed).contains(code),
                    "case {row:?}: expected {code}, got {parsed}"
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {row:?}: expected the completion to be accepted, got {status} \
                     with {parsed}"
                );
                assert_eq!(
                    parsed["layerDigest"],
                    json!(digest),
                    "case {row:?}: the completion reports the uploaded layer, got {parsed}"
                );
            }
        }
    }
}

/// The layer report answers EVERY requested digest, in request order: a digest of the
/// modelled `<algorithm>:<hex>` form is a layer — `AVAILABLE` with its size when this
/// repository holds it and `UNAVAILABLE` when it does not — and only an entry outside
/// that form is a failure, `InvalidLayerDigest`.
#[tokio::test]
async fn the_layer_report_answers_every_requested_digest_in_request_order() {
    let (router, _engine) = service_with_engine();
    let case = "a mixed batch of requested digests";
    let repository = "reported";

    // Two committed layers of different sizes, so a reported size cannot be a constant.
    let small = layer_bytes(16, 1);
    let large = layer_bytes(4096, 2);
    given_repository(&router, case, repository).await;
    for layer in [&small, &large] {
        let opened = ok(
            &router,
            "InitiateLayerUpload",
            json!({"repositoryName": repository}),
            case,
        )
        .await;
        let upload_id = opened["uploadId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: InitiateLayerUpload answered {opened}"))
            .to_string();
        given_parts(&router, repository, &upload_id, layer, 1, case).await;
        ok(
            &router,
            "CompleteLayerUpload",
            json!({
                "repositoryName": repository,
                "uploadId": upload_id,
                "layerDigests": [digest_of(layer)],
            }),
            case,
        )
        .await;
    }
    let held_small = digest_of(&small);
    let held_large = digest_of(&large);
    let missing = digest_of(b"never uploaded");

    // `BatchCheckLayerAvailability$layerDigests` targets `BatchedOperationLayerDigest`,
    // which carries a length bound and NO pattern
    // (`smithy_query(query="operation", operation="BatchCheckLayerAvailability")`), so a
    // digest that is not of the modelled `LayerDigest` form reaches the handler.
    let malformed = ["not-a-digest", "sha256:", ":abcdef", "sha256:zzzz", "sha256"];

    // Held digests, in request order, with their sizes.
    let report = availability(
        &router,
        repository,
        &[held_large.clone(), held_small.clone(), held_large.clone()],
        case,
    )
    .await;
    assert_eq!(
        reported_layers(&report, case),
        vec![
            (held_large.clone(), "AVAILABLE".to_string(), Some(4096)),
            (held_small.clone(), "AVAILABLE".to_string(), Some(16)),
            (held_large.clone(), "AVAILABLE".to_string(), Some(4096)),
        ],
        "case {case:?}: every requested digest is answered in order, got {report}"
    );
    assert_eq!(
        reported_failures(&report, case),
        Vec::new(),
        "case {case:?}: a held digest is no failure, got {report}"
    );

    // A mixed batch: held, unheld and malformed digests together.
    let mut requested = vec![held_small.clone(), missing.clone()];
    requested.extend(malformed.iter().map(ToString::to_string));
    requested.push(held_large.clone());
    let report = availability(&router, repository, &requested, case).await;
    assert_eq!(
        reported_layers(&report, case),
        vec![
            (held_small.clone(), "AVAILABLE".to_string(), Some(16)),
            (missing.clone(), "UNAVAILABLE".to_string(), None),
            (held_large.clone(), "AVAILABLE".to_string(), Some(4096)),
        ],
        "case {case:?}: every digest addressing a layer is a layer entry, got {report}"
    );
    let expected_failures: Vec<(String, String)> = malformed
        .iter()
        .map(|digest| ((*digest).to_string(), "InvalidLayerDigest".to_string()))
        .collect();
    assert_eq!(
        reported_failures(&report, case),
        expected_failures,
        "case {case:?}: only an entry addressing no layer is a failure, got {report}"
    );

    // The report is about one repository's layers: a second repository holding nothing
    // reports the same digests as unavailable.
    let empty = "reported-empty";
    given_repository(&router, case, empty).await;
    let report = availability(
        &router,
        empty,
        &[held_small.clone(), held_large.clone()],
        case,
    )
    .await;
    assert_eq!(
        reported_layers(&report, case),
        vec![
            (held_small, "UNAVAILABLE".to_string(), None),
            (held_large, "UNAVAILABLE".to_string(), None),
        ],
        "case {case:?}: another repository holds no layer of its own, got {report}"
    );
    assert_eq!(
        reported_failures(&report, case),
        Vec::new(),
        "case {case:?}: every entry addresses a layer, so none is a failure, got {report}"
    );
}

/// The layer report is scoped like the completion it reports on: a repository this
/// registry does not hold, and a registry that is not this one, are refused rather than
/// answered with every digest missing.
#[tokio::test]
async fn the_layer_report_refuses_a_repository_or_registry_that_is_not_there() {
    let (router, _engine): (MultiServiceRouter, Arc<engine::InMemoryEngine>) =
        service_with_engine();
    let case = "the report's scoping";
    given_repository(&router, case, "report-scope").await;
    let digest = digest_of(b"anything");

    // (case, repositoryName, registryId, expected code)
    let rows: Vec<(&str, &str, Option<&str>, &str)> = vec![
        (
            "a repository this registry does not hold",
            "no-such-repository",
            None,
            "RepositoryNotFoundException",
        ),
        (
            "a registry that is not this one",
            "report-scope",
            Some("123456789012"),
            "RegistryNotFoundException",
        ),
        (
            "a registry alias that is not this one",
            "report-scope",
            Some("someone-else"),
            "RegistryNotFoundException",
        ),
    ];

    for (row, repository, registry, code) in rows {
        let mut request = json!({"repositoryName": repository, "layerDigests": [digest.clone()]});
        if let Some(registry) = registry {
            request["registryId"] = json!(registry);
        }
        let (status, parsed) = call(&router, "BatchCheckLayerAvailability", request).await;
        assert_eq!(
            status, 400,
            "case {row:?}: expected 400 {code}, got {status} with {parsed}"
        );
        assert!(
            error_code(&parsed).contains(code),
            "case {row:?}: expected {code}, got {parsed}"
        );
    }

    // Both naming legs correct: the report answers, with the layer unavailable.
    let report =
        availability(&router, "report-scope", std::slice::from_ref(&digest), case).await;
    assert_eq!(
        reported_layers(&report, case),
        vec![(digest, "UNAVAILABLE".to_string(), None)],
        "case {case:?}: a repository with no layers reports the layer unavailable, got {report}"
    );
}
