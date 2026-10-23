// SPDX-License-Identifier: CC0-1.0

//! Uploading the parts of an ECR Public image layer, driven end to end over the wire.
//!
//! The rule under test: `UploadLayerPart` appends ONE part to an upload
//! `InitiateLayerUpload` opened against a repository of this registry, accepting it only
//! when the request is self-consistent (the blob carries exactly the bytes its
//! `partFirstByte`..`partLastByte` range spans, and that range is no wider than the
//! published maximum for one part) AND its first byte is consecutive to the last byte the
//! upload already received — so the bytes an upload holds are always the layer's leading
//! bytes, byte for byte, in layer order. An accepted part reports the position of its
//! last byte together with the registry, repository and upload the request named; a part
//! that does not fit is refused with the modelled `InvalidLayerPartException` carrying
//! the last byte that IS valid, and stores nothing; an upload id this repository was not
//! issued is `UploadNotFoundException`.
//!
//! Each table below spans the class the rule generalises over — how many parts a layer
//! arrives in and how large each is, every byte value a layer can carry, every way a
//! part's first byte can fail to be consecutive (a gap, an overlap, a resend, a restart,
//! a first part that does not begin at byte 0), every way a blob can disagree with its
//! declared range, and every way a request can name the registry — rather than one
//! recorded request.
//!
//! The wire plumbing and the store reader are the session suite's shared harness
//! (`tests/all/layer_upload_sessions.rs`): an in-flight upload is reported by no operation
//! of this model (`BatchCheckLayerAvailability` reports COMPLETED layers), so what was
//! persisted is read back from the store through the engine the router was built over.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_ecrpublic_spencerfrontendservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::layer_upload_sessions::{
    call, error_code, given_repository, ok, service_with_engine, upload_bytes, REGISTRY_ALIAS,
    REGISTRY_ID,
};

/// An upload id with the modelled `UploadId` layout that this registry never minted, so
/// it names no upload (`^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`).
const UNKNOWN_UPLOAD_ID: &str = "0123abcd-4567-89ef-0123-456789abcdef";

/// One row of the scoping table: the case, the `repositoryName` and `uploadId` the
/// request names, the `registryId` it names (if any), and the modelled error code
/// expected — `None` for a part that must be accepted.
type ScopeRow<'a> = (&'a str, &'a str, &'a str, Option<&'a str>, Option<&'a str>);

/// `layerPartBlob` as awsJson1_1 carries a blob: base64 of the raw bytes, through the
/// SDK's own standard-alphabet codec (`aws_smithy_types::base64`) — the one the service's
/// own deserializer uses.
fn blob(bytes: &[u8]) -> Value {
    Value::String(aws_smithy_types::base64::encode(bytes))
}

/// `len` deterministic bytes that depend on `seed`, so two layers of one case differ and
/// a round trip that drops or reorders bytes cannot pass.
fn layer_bytes(len: usize, seed: usize) -> Vec<u8> {
    (0..len)
        .map(|index| u8::try_from((index * 31 + seed * 7) % 256).unwrap_or_default())
        .collect()
}

/// One `UploadLayerPart` call: the status and body of uploading `bytes` as the part that
/// claims to start at `first_byte` and end at `last_byte`.
async fn upload_part(
    router: &MultiServiceRouter,
    repository: &str,
    upload_id: &str,
    first_byte: i64,
    last_byte: i64,
    bytes: &[u8],
) -> (u16, Value) {
    call(
        router,
        "UploadLayerPart",
        json!({
            "repositoryName": repository,
            "uploadId": upload_id,
            "partFirstByte": first_byte,
            "partLastByte": last_byte,
            "layerPartBlob": blob(bytes),
        }),
    )
    .await
}

/// A repository of this registry plus one upload opened against it, as every case needs.
async fn given_upload(router: &MultiServiceRouter, case: &str, repository: &str) -> String {
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

/// The layer bytes the store holds for `upload_id`.
fn received(engine: &Arc<engine::InMemoryEngine>, upload_id: &str, case: &str) -> Vec<u8> {
    upload_bytes(engine, upload_id)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds no upload {upload_id:?}"))
}

/// Send `bytes` as the next consecutive part of `upload_id`, which must be accepted: the
/// precondition the refusal cases are measured against.
async fn given_received(
    router: &MultiServiceRouter,
    repository: &str,
    upload_id: &str,
    bytes: &[u8],
    case: &str,
) {
    if bytes.is_empty() {
        return;
    }
    let last = i64::try_from(bytes.len()).unwrap_or(i64::MAX) - 1;
    let (status, parsed) = upload_part(router, repository, upload_id, 0, last, bytes).await;
    assert_eq!(
        status, 200,
        "case {case:?}: seeding {} bytes must be accepted, got {status} with {parsed}",
        bytes.len()
    );
}

/// A layer arrives as a sequence of parts and the upload holds exactly the layer: each
/// accepted part reports the position of its own last byte together with the registry,
/// repository and upload the request named, and the bytes the store holds afterwards are
/// the layer's bytes in order.
///
/// The rows differ along what the rule generalises over: how many parts the layer arrives
/// in, how large each part is (one byte, uneven, a mebibyte), and which byte values the
/// layer carries (including every one of the 256).
#[tokio::test]
async fn parts_accepted_in_order_reproduce_the_layer_byte_for_byte() {
    // (case, repositoryName, the layer's bytes, the part lengths it arrives in)
    let every_byte_value: Vec<u8> = (0..=255_u8).collect();
    let rows: Vec<(&str, &str, Vec<u8>, Vec<usize>)> = vec![
        ("one byte in one part", "single-byte", vec![0x00], vec![1]),
        (
            "the whole layer in one part",
            "one-part",
            layer_bytes(4096, 1),
            vec![4096],
        ),
        (
            "equal parts",
            "equal-parts",
            layer_bytes(64, 2),
            vec![16, 16, 16, 16],
        ),
        (
            "an uneven last part",
            "uneven-parts",
            layer_bytes(256, 3),
            vec![100, 100, 56],
        ),
        (
            "one byte at a time",
            "byte-at-a-time",
            vec![0xff, 0x00, 0x7f, 0x80],
            vec![1, 1, 1, 1],
        ),
        (
            "every byte value, in parts of differing size",
            "all-byte-values",
            every_byte_value.clone(),
            vec![1, 2, 13, 240],
        ),
        (
            "a mebibyte in two parts",
            "org/team/large-layer",
            layer_bytes(1 << 20, 4),
            vec![1 << 19, 1 << 19],
        ),
    ];

    let (router, engine) = service_with_engine();
    for (case, repository, layer, part_lengths) in rows {
        assert_eq!(
            part_lengths.iter().sum::<usize>(),
            layer.len(),
            "case {case:?}: the row's parts must add up to its layer"
        );
        let upload_id = given_upload(&router, case, repository).await;

        let mut offset = 0_usize;
        for (index, length) in part_lengths.iter().enumerate() {
            let first = i64::try_from(offset).expect("the offset fits in a long");
            let last = first + i64::try_from(*length).expect("the part length fits in a long") - 1;
            let (status, parsed) = upload_part(
                &router,
                repository,
                &upload_id,
                first,
                last,
                &layer[offset..offset + length],
            )
            .await;
            offset += length;

            assert_eq!(
                status, 200,
                "case {case:?} part {index}: expected HTTP 200, got {status} with {parsed}"
            );
            assert_eq!(
                parsed["lastByteReceived"].as_i64(),
                Some(last),
                "case {case:?} part {index}: lastByteReceived is the position of this \
                 part's last byte, got {parsed}"
            );
            assert_eq!(
                parsed["registryId"].as_str(),
                Some(REGISTRY_ID),
                "case {case:?} part {index}: the response names this registry, got {parsed}"
            );
            assert_eq!(
                parsed["repositoryName"].as_str(),
                Some(repository),
                "case {case:?} part {index}: the response names the repository the request \
                 carried, got {parsed}"
            );
            assert_eq!(
                parsed["uploadId"].as_str(),
                Some(upload_id.as_str()),
                "case {case:?} part {index}: the response names the upload the request \
                 carried, got {parsed}"
            );

            let held = received(&engine, &upload_id, case);
            assert_eq!(
                held,
                layer[..offset].to_vec(),
                "case {case:?} part {index}: the upload holds the layer's first {offset} \
                 bytes, got {} bytes",
                held.len()
            );
        }

        let held = received(&engine, &upload_id, case);
        assert_eq!(
            held, layer,
            "case {case:?}: the uploaded parts reproduce the layer byte for byte, got {} \
             bytes",
            held.len()
        );
    }
}

/// A part whose first byte is not consecutive to the last byte received is refused with
/// `InvalidLayerPartException`, which reports the last byte that IS valid (absent while
/// the upload has received nothing), stores nothing — and leaves the upload able to
/// accept the part that does come next.
///
/// The rows span every way a first byte can fail to be consecutive: a gap, a gap far
/// ahead, an overlap, a resend of what was received, a restart of the layer, and a first
/// part of a fresh upload that does not begin at byte 0.
#[tokio::test]
async fn a_part_that_is_not_consecutive_is_refused_and_stores_nothing() {
    // (case, repositoryName, how many bytes the upload already received, the refused
    // part's partFirstByte, the refused part's length)
    let rows: Vec<(&str, &str, usize, i64, usize)> = vec![
        ("fresh upload, first part begins at byte 1", "gap-at-one", 0, 1, 4),
        (
            "fresh upload, first part begins far into the layer",
            "gap-far-in",
            0,
            4096,
            1,
        ),
        ("a one-byte gap", "gap-of-one", 10, 11, 4),
        ("a gap far ahead", "gap-ahead", 10, 100, 1),
        ("overlapping the bytes received", "overlap", 10, 5, 10),
        ("resending the part received", "resend", 10, 0, 10),
        ("restarting the layer", "restart", 10, 0, 1),
        (
            "continuing one byte before the end",
            "one-byte-back",
            32,
            31,
            4,
        ),
    ];

    let (router, engine) = service_with_engine();
    for (case, repository, already, first_byte, part_length) in rows {
        let upload_id = given_upload(&router, case, repository).await;
        let seeded = layer_bytes(already, 5);
        given_received(&router, repository, &upload_id, &seeded, case).await;

        let part = layer_bytes(part_length, 6);
        let last_byte = first_byte + i64::try_from(part_length).unwrap_or(i64::MAX) - 1;
        let (status, parsed) =
            upload_part(&router, repository, &upload_id, first_byte, last_byte, &part).await;

        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 InvalidLayerPartException, got {status} with \
             {parsed}"
        );
        let code = error_code(&parsed);
        assert!(
            code.contains("InvalidLayerPartException"),
            "case {case:?}: expected InvalidLayerPartException, got {code:?} in {parsed}"
        );
        // `lastValidByteReceived` is the position of the last byte accepted so far, which
        // an upload that has received nothing does not have.
        let expected_last_valid = already.checked_sub(1).map(|position| {
            i64::try_from(position).expect("the seeded byte count fits in a long")
        });
        assert_eq!(
            parsed["lastValidByteReceived"].as_i64(),
            expected_last_valid,
            "case {case:?}: expected lastValidByteReceived {expected_last_valid:?}, got \
             {parsed}"
        );
        assert_eq!(
            parsed["uploadId"].as_str(),
            Some(upload_id.as_str()),
            "case {case:?}: the refusal names the upload the request carried, got {parsed}"
        );
        assert_eq!(
            parsed["repositoryName"].as_str(),
            Some(repository),
            "case {case:?}: the refusal names the repository the request carried, got {parsed}"
        );
        assert_eq!(
            parsed["registryId"].as_str(),
            Some(REGISTRY_ID),
            "case {case:?}: the refusal names this registry, got {parsed}"
        );

        let held = received(&engine, &upload_id, case);
        assert_eq!(
            held, seeded,
            "case {case:?}: a refused part changes nothing, so the upload still holds the \
             {already} bytes it had, got {} bytes",
            held.len()
        );

        // The upload is still usable: the part that IS consecutive is accepted next.
        let next = layer_bytes(8, 7);
        let next_first = i64::try_from(already).expect("the seeded byte count fits in a long");
        let (status, parsed) = upload_part(
            &router,
            repository,
            &upload_id,
            next_first,
            next_first + 7,
            &next,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the consecutive part after a refusal is accepted, got {status} \
             with {parsed}"
        );
        let held = received(&engine, &upload_id, case);
        let mut expected = seeded.clone();
        expected.extend_from_slice(&next);
        assert_eq!(
            held, expected,
            "case {case:?}: the accepted part appends to the bytes already received, got {} \
             bytes",
            held.len()
        );
    }
}

/// A part has to agree with itself: the blob carries exactly the bytes its declared
/// `partFirstByte`..`partLastByte` range spans, and that range is no wider than the
/// published maximum for one part (`state::MAX_LAYER_PART_BYTES`, the model's
/// `layerPartBlob` `@length(0..20971520)`). Every disagreement is
/// `InvalidLayerPartException` and stores nothing.
#[tokio::test]
async fn a_part_whose_blob_disagrees_with_its_declared_range_is_refused() {
    // (case, repositoryName, partFirstByte, partLastByte, the blob's length)
    let max = state::MAX_LAYER_PART_BYTES;
    let rows: Vec<(&str, &str, i64, i64, usize)> = vec![
        ("a blob shorter than its range", "short-blob", 0, 9, 5),
        ("a blob longer than its range", "long-blob", 0, 3, 10),
        ("an empty blob, one declared byte", "empty-blob", 0, 0, 0),
        ("two bytes declared as one", "two-as-one", 0, 0, 2),
        ("partLastByte before partFirstByte", "reversed", 5, 4, 1),
        ("a range that spans nothing at all", "zero-span", 7, 6, 0),
        // The published bound and one past it: a range of exactly the maximum is an
        // admissible part (it is refused only for the blob that does not fill it), while
        // a range one byte wider is refused as more than one part may carry.
        ("a range of exactly the maximum part", "at-the-max", 0, max - 1, 10),
        ("a range one byte past the maximum part", "past-the-max", 0, max, 10),
    ];

    let (router, engine) = service_with_engine();
    for (case, repository, first_byte, last_byte, part_length) in rows {
        let upload_id = given_upload(&router, case, repository).await;
        let part = layer_bytes(part_length, 8);
        let (status, parsed) = upload_part(
            &router,
            repository,
            &upload_id,
            first_byte,
            last_byte,
            &part,
        )
        .await;

        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 InvalidLayerPartException, got {status} with \
             {parsed}"
        );
        let code = error_code(&parsed);
        assert!(
            code.contains("InvalidLayerPartException"),
            "case {case:?}: expected InvalidLayerPartException, got {code:?} in {parsed}"
        );
        assert_eq!(
            parsed["lastValidByteReceived"].as_i64(),
            None,
            "case {case:?}: the upload has received nothing, so there is no last valid \
             byte, got {parsed}"
        );
        let held = received(&engine, &upload_id, case);
        assert!(
            held.is_empty(),
            "case {case:?}: a refused part stores nothing, got {} bytes",
            held.len()
        );
    }
}

/// What one upload accumulates is bounded: the part that fills
/// [`state::MAX_LAYER_UPLOAD_BYTES`] is still accepted, and the part that would carry the
/// upload past it is refused with the `LimitExceededException` this operation models —
/// this registry's own capacity rather than a fault in the request — storing nothing.
///
/// The ceiling is read from the constant, so the boundary tested is wherever the
/// implementation puts it. The bytes below it are seeded straight into the store (this
/// suite holds the engine): what is under test is the boundary, not a gibibyte of round
/// trips.
#[tokio::test]
async fn a_part_past_the_upload_byte_ceiling_is_refused() {
    let case = "the per-upload byte ceiling";
    let (router, engine) = service_with_engine();
    let upload_id = given_upload(&router, case, "capped-bytes").await;

    // One byte short of the ceiling, so the next wire call is the one that fills it.
    let seeded = state::MAX_LAYER_UPLOAD_BYTES - 1;
    {
        let store = state::service_state(&*engine).expect("the ecr-public store");
        let mut store = store.lock();
        let upload = store
            .uploads
            .get_mut(&upload_id)
            .expect("the upload just opened");
        upload.received = vec![0_u8; seeded];
    }
    let next_first = i64::try_from(seeded).expect("the seeded byte count fits in a long");

    // Two bytes do not fit under the ceiling; the part is refused and stores neither.
    let (status, parsed) = upload_part(
        &router,
        "capped-bytes",
        &upload_id,
        next_first,
        next_first + 1,
        &[0xaa, 0xbb],
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 LimitExceededException, got {status} with {parsed}"
    );
    let code = error_code(&parsed);
    assert!(
        code.contains("LimitExceededException"),
        "case {case:?}: expected LimitExceededException past {} bytes, got {code:?} in \
         {parsed}",
        state::MAX_LAYER_UPLOAD_BYTES
    );
    assert_eq!(
        received(&engine, &upload_id, case).len(),
        seeded,
        "case {case:?}: a refused part stores nothing, so the upload stays one byte short \
         of the ceiling"
    );

    // The one byte that fits exactly at the ceiling is still accepted.
    let (status, parsed) = upload_part(
        &router,
        "capped-bytes",
        &upload_id,
        next_first,
        next_first,
        &[0xcc],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the part that fills the ceiling is accepted, got {status} with \
         {parsed}"
    );
    let held = received(&engine, &upload_id, case);
    assert_eq!(
        held.len(),
        state::MAX_LAYER_UPLOAD_BYTES,
        "case {case:?}: the upload holds every byte accepted up to the ceiling"
    );
    assert_eq!(
        held.last(),
        Some(&0xcc),
        "case {case:?}: the accepted part's byte is the layer's last"
    );
}

/// An upload id is only valid for the repository it was issued for, and a layer part can
/// only reach a repository of this registry: a well-formed id this registry never minted
/// and one minted for another repository are both `UploadNotFoundException`, a repository
/// this registry no longer holds is `RepositoryNotFoundException`, and a `registryId`
/// naming anything but this registry is `RegistryNotFoundException` — while naming this
/// registry by account id, by alias or not at all all work.
#[tokio::test]
async fn a_part_reaches_only_an_upload_this_repository_was_issued() {
    let (router, engine) = service_with_engine();
    let upload_id = given_upload(&router, "upload scoping", "scoped").await;
    let other_upload_id = given_upload(&router, "upload scoping", "other-repo").await;
    // A repository created, opened an upload against, and then deleted.
    let gone_upload_id = given_upload(&router, "deleted repository", "gone").await;
    ok(
        &router,
        "DeleteRepository",
        json!({"repositoryName": "gone"}),
        "deleted repository",
    )
    .await;

    // (case, the repositoryName sent, the uploadId sent, the registryId sent, the
    // modelled error code expected — `None` for an accepted part)
    let rows: Vec<ScopeRow> = vec![
        (
            "no registryId: the default public registry is assumed",
            "scoped",
            upload_id.as_str(),
            None,
            None,
        ),
        (
            "this registry, by account id",
            "scoped",
            upload_id.as_str(),
            Some(REGISTRY_ID),
            None,
        ),
        (
            "this registry, by alias",
            "scoped",
            upload_id.as_str(),
            Some(REGISTRY_ALIAS),
            None,
        ),
        (
            "an upload id this registry never minted",
            "scoped",
            UNKNOWN_UPLOAD_ID,
            None,
            Some("UploadNotFoundException"),
        ),
        (
            "an upload id issued for another repository",
            "scoped",
            other_upload_id.as_str(),
            None,
            Some("UploadNotFoundException"),
        ),
        (
            "a repository this registry no longer holds",
            "gone",
            gone_upload_id.as_str(),
            None,
            Some("RepositoryNotFoundException"),
        ),
        (
            "a repository this registry never held",
            "never-created",
            upload_id.as_str(),
            None,
            Some("RepositoryNotFoundException"),
        ),
        (
            "another account's registry",
            "scoped",
            upload_id.as_str(),
            Some("123456789012"),
            Some("RegistryNotFoundException"),
        ),
        (
            "another registry's alias",
            "scoped",
            upload_id.as_str(),
            Some("someoneelse"),
            Some("RegistryNotFoundException"),
        ),
    ];

    // Each accepted row appends its part to the one upload the rows share, so the
    // expectation walks with the bytes it holds.
    let mut accepted: Vec<u8> = Vec::new();
    for (case, repository, requested_upload, registry, expected_error) in rows {
        let part = layer_bytes(4, 9);
        let first = i64::try_from(accepted.len()).expect("the byte count fits in a long");
        let mut request = json!({
            "repositoryName": repository,
            "uploadId": requested_upload,
            "partFirstByte": first,
            "partLastByte": first + 3,
            "layerPartBlob": blob(&part),
        });
        if let Some(registry) = registry {
            request["registryId"] = json!(registry);
        }
        let (status, parsed) = call(&router, "UploadLayerPart", request).await;

        match expected_error {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} with {parsed}"
                );
                accepted.extend_from_slice(&part);
            }
            Some(code) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400 {code}, got {status} with {parsed}"
                );
                let actual = error_code(&parsed);
                assert!(
                    actual.contains(code),
                    "case {case:?}: expected {code}, got {actual:?} in {parsed}"
                );
            }
        }

        let held = received(&engine, &upload_id, case);
        assert_eq!(
            held, accepted,
            "case {case:?}: only an accepted part changes what the upload holds, got {} \
             bytes",
            held.len()
        );
        let untouched = received(&engine, &other_upload_id, case);
        assert!(
            untouched.is_empty(),
            "case {case:?}: no part reached the other repository's upload, got {} bytes",
            untouched.len()
        );
    }
}
