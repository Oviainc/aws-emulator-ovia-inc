// SPDX-License-Identifier: CC0-1.0

//! The `$.message` each layer-upload refusal carries, driven end to end over the wire.
//!
//! The rules under test, both about text a client parses:
//!
//! * a layer part whose `partFirstByte` is not the next byte after the last byte the
//!   upload received is refused `InvalidLayerPartException` ("the layer part size isn't
//!   valid, or the first byte specified isn't consecutive to the last byte of a previous
//!   layer part upload", client, httpError 400) whose message states exactly the two
//!   offsets — the first byte the request declared, then the first byte the upload's ledger
//!   expects — because the expected offset is what a resumable push reads out of it;
//! * an upload id that addresses no open upload of the named repository is refused
//!   `UploadNotFoundException` with the clause the service states for an upload: the upload
//!   id, the repository it was issued for and the registry that holds it, in that order;
//!   the same clause `CompleteLayerUpload` words its empty-upload and already-exists
//!   refusals over, and the layer-digest disagreement is one fixed sentence that reports
//!   neither the digests claimed nor the digest the registry computed.
//!
//! Every clause is a deterministic function of the request, so each row's expectation is
//! BUILT from the request it sends rather than copied. The tables span what each rule
//! generalises over: the ways a first byte can fail to be consecutive (a gap, a far gap, an
//! overlap, a resend, a fresh upload that does not start at byte 0) at several ledger
//! positions, and each way an upload id can fail to address an upload (never minted,
//! issued for another repository, already consumed) over both operations that take one.

use serde_json::json;

use crate::harness::{error_message, given_layer, layer_digest};
use crate::layer_part_uploads::upload_part;
use crate::layer_upload_completion::{given_upload, layer_bytes};
use crate::repository_image_lifecycle::{ok, refused, service, text, REGISTRY_ID};

/// An `uploadId` with the layout the model gives `UploadId`
/// (`^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$`) that
/// this registry never minted.
const UNMINTED_UPLOAD_ID: &str = "0123abcd-4567-89ef-0123-456789abcdef";

/// Open one more upload against a repository that already exists.
async fn another_upload(
    router: &smithy_server_runtime::MultiServiceRouter,
    repository: &str,
    case: &str,
) -> String {
    let initiated = ok(
        router,
        "InitiateLayerUpload",
        json!({"repositoryName": repository}),
        case,
    )
    .await;
    text(&initiated, &["uploadId"], case)
}

#[tokio::test]
async fn a_part_that_is_not_consecutive_is_refused_with_the_two_offsets() {
    let router = service();

    // (case, bytes the upload already received, the refused part's partFirstByte, how many
    // bytes that part carries)
    let rows: Vec<(&str, usize, i64, usize)> = vec![
        ("a fresh upload whose first part begins at byte 1", 0, 1, 4),
        ("a fresh upload whose first part begins far in", 0, 4096, 1),
        ("a one-byte gap after one part", 1000, 1001, 24),
        ("a gap far ahead of the ledger", 10, 100, 1),
        ("a part overlapping the bytes received", 10, 5, 10),
        ("a resend of the part received", 10, 0, 10),
        ("a part one byte before the end of the ledger", 32, 31, 4),
    ];

    for (index, (case, already, first_byte, part_length)) in rows.into_iter().enumerate() {
        let repository = format!("offsets/case{index:02}");
        let upload_id = given_upload(&router, case, &repository).await;
        if already > 0 {
            let seeded = layer_bytes(already, index + 1);
            let last = i64::try_from(already).expect("the seeded length fits in a long") - 1;
            let (status, parsed) =
                upload_part(&router, &repository, &upload_id, 0, last, &seeded).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the seeding part is accepted, got {status} with {parsed}"
            );
        }

        let part = layer_bytes(part_length, index + 9);
        let last_byte = first_byte + i64::try_from(part_length).expect("a long") - 1;
        let (status, parsed) =
            upload_part(&router, &repository, &upload_id, first_byte, last_byte, &part).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {parsed}"
        );
        assert!(
            parsed["__type"]
                .as_str()
                .unwrap_or_default()
                .contains("InvalidLayerPartException"),
            "case {case:?}: expected InvalidLayerPartException, got {parsed}"
        );
        // The expected first byte is the request's own ledger position: the byte after the
        // last one the upload received, and byte 0 for an upload that received nothing.
        let expected_first = i64::try_from(already).expect("the ledger position fits in a long");
        let expected = format!(
            "First byte of the layer part is {first_byte} instead of {expected_first}"
        );
        let actual = error_message(&parsed);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
        assert!(
            !actual.contains(&upload_id),
            "case {case:?}: the two-offset message states no upload id, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn an_upload_id_that_addresses_nothing_is_refused_with_the_upload_clause() {
    let router = service();

    // One repository with an open upload, a second repository, and an upload id consumed by
    // a completion: the three ways an id can fail to address an open upload of the named
    // repository, crossed with both operations that take one.
    let repository = "uploads/named";
    let other = "uploads/other";
    let open_elsewhere = given_upload(&router, "an id issued for another repository", other).await;
    let upload_id = given_upload(&router, "an upload to consume", repository).await;
    let layer = layer_bytes(64, 3);
    let last = i64::try_from(layer.len()).expect("a long") - 1;
    let (status, parsed) = upload_part(&router, repository, &upload_id, 0, last, &layer).await;
    assert_eq!(
        status, 200,
        "the part of the upload to consume is accepted, got {status} with {parsed}"
    );
    ok(
        &router,
        "CompleteLayerUpload",
        json!({
            "repositoryName": repository,
            "uploadId": upload_id,
            "layerDigests": [layer_digest(&layer)],
        }),
        "an upload to consume",
    )
    .await;

    let rows: Vec<(&str, String)> = vec![
        (
            "an upload id this registry never minted",
            UNMINTED_UPLOAD_ID.to_string(),
        ),
        (
            "an upload id issued for another repository",
            open_elsewhere,
        ),
        ("an upload id a completion consumed", upload_id),
    ];

    for (case, id) in rows {
        let expected = format!(
            "The upload with id '{id}' in the repository with name '{repository}' in \
             registry with id '{REGISTRY_ID}' does not exist"
        );

        let part = layer_bytes(8, 5);
        let (status, parsed) = upload_part(&router, repository, &id, 0, 7, &part).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected UploadLayerPart to refuse, got {status} with {parsed}"
        );
        assert!(
            parsed["__type"]
                .as_str()
                .unwrap_or_default()
                .contains("UploadNotFoundException"),
            "case {case:?}: expected UploadNotFoundException from UploadLayerPart, got {parsed}"
        );
        let actual = error_message(&parsed);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?} from UploadLayerPart, got {actual:?}"
        );

        let body = refused(
            &router,
            "CompleteLayerUpload",
            json!({
                "repositoryName": repository,
                "uploadId": id,
                "layerDigests": [layer_digest(&part)],
            }),
            "UploadNotFoundException",
            case,
        )
        .await;
        let actual = error_message(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?} from CompleteLayerUpload, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn completing_an_upload_states_the_upload_clause_for_every_refusal_it_words() {
    let router = service();

    // An upload with no parts: the completion states the same upload clause, and says what
    // is wrong with it.
    let empty_repository = "clauses/empty";
    let empty_upload = given_upload(&router, "an upload with no parts", empty_repository).await;
    let body = refused(
        &router,
        "CompleteLayerUpload",
        json!({
            "repositoryName": empty_repository,
            "uploadId": empty_upload,
            "layerDigests": [format!("sha256:{}", "1".repeat(64))],
        }),
        "EmptyUploadException",
        "an upload with no parts",
    )
    .await;
    let expected = format!(
        "The upload with id '{empty_upload}' in the repository with name \
         '{empty_repository}' in registry with id '{REGISTRY_ID}' cannot be completed \
         because it does not have any parts"
    );
    let actual = error_message(&body);
    assert_eq!(
        actual, expected,
        "expected {expected:?} for an upload with no parts, got {actual:?}"
    );

    // A layer the repository already holds, completed again through a second upload of the
    // same bytes: the refusal names the upload the caller asked to complete.
    let repository = "clauses/duplicate";
    let layer = layer_bytes(108, 11);
    ok(
        &router,
        "CreateRepository",
        json!({"repositoryName": repository}),
        "a layer already held",
    )
    .await;
    given_layer(&router, repository, &layer, "a layer already held").await;
    let second = another_upload(&router, repository, "a second upload of the same bytes").await;
    let last = i64::try_from(layer.len()).expect("a long") - 1;
    let (status, parsed) = upload_part(&router, repository, &second, 0, last, &layer).await;
    assert_eq!(
        status, 200,
        "the second upload's part is accepted, got {status} with {parsed}"
    );
    let body = refused(
        &router,
        "CompleteLayerUpload",
        json!({
            "repositoryName": repository,
            "uploadId": second,
            "layerDigests": [layer_digest(&layer)],
        }),
        "LayerAlreadyExistsException",
        "a layer already held",
    )
    .await;
    let expected = format!(
        "Layer with upload id '{second}' already exists in the repository with name \
         '{repository}' in registry with id '{REGISTRY_ID}'"
    );
    let actual = error_message(&body);
    assert_eq!(
        actual, expected,
        "expected {expected:?} for a layer already held, got {actual:?}"
    );

    // A completion claiming digests the uploaded bytes do not hash to: one sentence, which
    // reports neither the claimed digests nor the digest the registry computed.
    let mismatch_repository = "clauses/mismatch";
    let mismatch_upload =
        given_upload(&router, "a digest disagreement", mismatch_repository).await;
    let bytes = layer_bytes(40, 13);
    let last = i64::try_from(bytes.len()).expect("a long") - 1;
    let (status, parsed) =
        upload_part(&router, mismatch_repository, &mismatch_upload, 0, last, &bytes).await;
    assert_eq!(
        status, 200,
        "the part of the disagreeing upload is accepted, got {status} with {parsed}"
    );
    let claimed = [
        format!("sha256:{}", "4".repeat(64)),
        layer_digest(&layer_bytes(41, 13)),
    ];
    let body = refused(
        &router,
        "CompleteLayerUpload",
        json!({
            "repositoryName": mismatch_repository,
            "uploadId": mismatch_upload,
            "layerDigests": claimed,
        }),
        "InvalidLayerException",
        "a digest disagreement",
    )
    .await;
    let actual = error_message(&body);
    assert_eq!(
        actual, "None of the provided layer digests match the calculated layer digest",
        "expected the one sentence for a digest disagreement, got {actual:?}"
    );
    for leaked in claimed.iter().chain([&layer_digest(&bytes)]) {
        assert!(
            !actual.contains(leaked.as_str()),
            "the sentence states no digest, got {actual:?} carrying {leaked:?}"
        );
    }
}
