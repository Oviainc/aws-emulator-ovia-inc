// SPDX-License-Identifier: CC0-1.0

//! The VALUES the EBS direct APIs put inside a response whose status is already right: a
//! snapshot's lifecycle status, a `ValidationExceptionReason`, and a refusal's fixed
//! message text.
//!
//! Every test drives real HTTP through the same `MultiServiceRouter` the production
//! router uses, over the harness `snapshot_block_loop` owns, and asserts a RULE over a
//! table of inputs:
//!
//! 1. An operation the spec documents as CHANGING a resource's status applies that
//!    change when it runs, so every later reader — including a repeat of the call — sees
//!    the changed record, while an `Accepted` (202) response acknowledges the status the
//!    record was in when the request arrived.
//! 2. A payload the service cannot read as the request declared it is a bad request
//!    parameter — `INVALID_PARAMETER_VALUE` with the service's fixed sentence for a
//!    failed read — while `INVALID_BLOCK` stays with a block that WAS read and is then
//!    rejected on its content.
//! 3. A refusal whose sentence is fixed text is byte-identical whatever the request
//!    carried: it interpolates no request value.

use crate::snapshot_block_loop::{
    block_bytes, checksum, complete_snapshot, get_block, list_blocks, listed_blocks, member,
    put_block, router, start_snapshot, WriteCase, BLOCK_SIZE,
};

/// The fixed sentence the service answers with when the declared block payload cannot be
/// read, mirroring `crate::state::FAILED_TO_READ_BLOCK_DATA_MESSAGE`.
const FAILED_TO_READ_BLOCK_DATA: &str = "Failed to read block data";

/// The fixed sentence `StartSnapshot` answers with when a request asks for both a parent
/// snapshot and encryption, mirroring
/// `crate::state::PARENT_SNAPSHOT_ID_AND_ENCRYPT_MESSAGE`.
const PARENT_AND_ENCRYPT_REFUSAL: &str = "ParentSnapshotId and Encrypt cannot be specified together";

/// The `SnapshotId` of a 201 `StartSnapshot`, with the status it reported.
async fn started_snapshot(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    request: &serde_json::Value,
) -> String {
    let started = start_snapshot(router, request).await;
    assert_eq!(
        started.status, 201,
        "case {case:?}: StartSnapshot must answer 201, got {} with body {}",
        started.status,
        String::from_utf8_lossy(&started.body)
    );
    assert_eq!(
        member(&started, "Status"),
        "pending",
        "case {case:?}: a new snapshot enters the pending state, got {}",
        started.json()
    );
    let snapshot_id = member(&started, "SnapshotId");
    assert!(
        !snapshot_id.is_empty(),
        "case {case:?}: StartSnapshot must report a SnapshotId, got {}",
        started.json()
    );
    snapshot_id
}

/// RULE 1: `CompleteSnapshot` "[s]eals and completes the snapshot … Completing the
/// snapshot changes the status to completed", so the seal is applied by the call: its
/// `Accepted` (202) answer carries the status the record was in when the request arrived,
/// and every later observation shows the sealed snapshot — a repeat of the call reads
/// `completed`, no further block is accepted, the snapshot may be used as a parent (only
/// a sealed snapshot may), and everything written into it is still served.
#[tokio::test]
async fn completing_a_snapshot_applies_the_seal_every_later_reader_then_sees() {
    // Rows differ along what the rule generalises over: how many blocks the snapshot
    // holds when it is sealed, which indexes they sit at, and the volume they sit in.
    let cases: &[WriteCase<'_>] = &[
        ("no blocks written", 1, &[]),
        ("one block at index 0", 1, &[(0, 41)]),
        ("two adjacent blocks", 2, &[(0, 42), (1, 43)]),
        ("sparse indexes in a larger volume", 8, &[(3, 44), (11, 45), (4, 46)]),
    ];

    for (case, volume_size, blocks) in cases {
        let router = router();
        let snapshot_id = started_snapshot(
            &router,
            case,
            &serde_json::json!({ "VolumeSize": volume_size }),
        )
        .await;

        for (block_index, seed) in *blocks {
            let data = block_bytes(*seed);
            let expected = checksum(&data);
            let written =
                put_block(&router, &snapshot_id, *block_index, data, BLOCK_SIZE, &expected).await;
            assert_eq!(
                written.status, 201,
                "case {case:?}: PutSnapshotBlock at {block_index} must answer 201, got {} with body {}",
                written.status,
                String::from_utf8_lossy(&written.body)
            );
        }

        let count = i32::try_from(blocks.len()).expect("a small block count");
        let sealed = complete_snapshot(&router, &snapshot_id, count).await;
        assert_eq!(
            sealed.status, 202,
            "case {case:?}: CompleteSnapshot must answer 202, got {} with body {}",
            sealed.status,
            String::from_utf8_lossy(&sealed.body)
        );
        assert_eq!(
            member(&sealed, "Status"),
            "pending",
            "case {case:?}: the 202 must acknowledge the status the seal was applied to, got {}",
            sealed.json()
        );

        // The sealed status through a second call: the seal was applied by the first one,
        // so nothing is left outstanding.
        let again = complete_snapshot(&router, &snapshot_id, count).await;
        assert_eq!(
            member(&again, "Status"),
            "completed",
            "case {case:?}: a later call must read the sealed status back, got {}",
            again.json()
        );

        // Observed through the operations that branch on the status rather than report
        // it. A write is refused ("You cannot write new blocks to a snapshot after it has
        // been completed") …
        let extra = block_bytes(47);
        let refused = put_block(
            &router,
            &snapshot_id,
            2,
            extra.clone(),
            BLOCK_SIZE,
            &checksum(&extra),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a write to a sealed snapshot must be refused, got {} with body {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        // The refusal must be about the snapshot's state, not any other 400.
        assert_eq!(
            member(&refused, "Reason"),
            "INVALID_SNAPSHOT_ID",
            "case {case:?}: the refusal must name the snapshot as the rejected input, got {}",
            refused.json()
        );
        // Rule 3: the sentence is fixed text — it interpolates neither the snapshot id nor
        // the block the write carried.
        assert_eq!(
            member(&refused, "Message"),
            "Snapshot is already completed",
            "case {case:?}: the refusal must carry the sealed-snapshot sentence, got {}",
            refused.json()
        );

        // … and the snapshot is now acceptable as a `ParentSnapshotId`, which a snapshot
        // still being written is not.
        let child = start_snapshot(
            &router,
            &serde_json::json!({
                "VolumeSize": volume_size,
                "ParentSnapshotId": snapshot_id.clone(),
            }),
        )
        .await;
        assert_eq!(
            child.status, 201,
            "case {case:?}: a sealed snapshot must be usable as a parent, got {} with body {}",
            child.status,
            String::from_utf8_lossy(&child.body)
        );

        // Everything written into it is still served, with the same indexes.
        let listed = listed_blocks(&list_blocks(&router, &snapshot_id, "").await);
        let mut expected_indexes: Vec<i32> = blocks.iter().map(|(index, _)| *index).collect();
        expected_indexes.sort_unstable();
        let indexes: Vec<i32> = listed.iter().map(|(index, _)| *index).collect();
        assert_eq!(
            indexes, expected_indexes,
            "case {case:?}: a sealed snapshot must still list the blocks written, got {indexes:?}"
        );
        for (block_index, token) in &listed {
            let fetched = get_block(&router, &snapshot_id, *block_index, token).await;
            assert_eq!(
                fetched.status, 200,
                "case {case:?}: block {block_index} of a sealed snapshot must still be fetchable, got {}",
                fetched.status
            );
        }
    }
}

/// RULE 2: the request DECLARES how many bytes of block data it carries, so a payload
/// that does not deliver them leaves the service with no block to store: the declared
/// block data could not be read, which is refused HTTP 400 `ValidationException` with
/// `Reason` `INVALID_PARAMETER_VALUE` and the fixed sentence for a failed read, naming no
/// byte count. `INVALID_BLOCK` stays with a block that WAS read and is then rejected on
/// its content.
///
/// Every row presents the checksum OF THE BYTES IT SENDS, so the request's integrity
/// binding holds and the declared length is what is left to judge: the binding is judged
/// first, and `payload_checksum_integrity.rs` asserts that.
#[tokio::test]
async fn an_unreadable_block_payload_is_refused_as_a_bad_parameter() {
    let router = router();
    let snapshot_id = started_snapshot(
        &router,
        "payload reads",
        &serde_json::json!({ "VolumeSize": 1 }),
    )
    .await;

    let full = block_bytes(51);
    let full_checksum = checksum(&full);
    let block_size = usize::try_from(BLOCK_SIZE).expect("the block size fits a usize");

    // Rows differ along the only thing the rule generalises over: how many bytes the
    // payload actually carries against the `524288` the request declared — none, a
    // single byte, half a block, one byte short and one byte long.
    let payload_lengths: &[usize] = &[0, 1, block_size / 2, block_size - 1, block_size + 1];

    for length in payload_lengths {
        let case = format!("a payload of {length} bytes against a declared {BLOCK_SIZE}");
        let mut payload = full.clone();
        payload.resize(*length, 0);
        let sent_checksum = checksum(&payload);
        let reply = put_block(
            &router,
            &snapshot_id,
            0,
            payload,
            BLOCK_SIZE,
            &sent_checksum,
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: an unreadable payload must be refused 400, got {} with body {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert!(
            reply.error_code().ends_with("ValidationException"),
            "case {case:?}: expected the ValidationException error code, got {:?}",
            reply.error_code()
        );
        assert_eq!(
            member(&reply, "Reason"),
            "INVALID_PARAMETER_VALUE",
            "case {case:?}: a payload the service cannot read is a bad parameter, got {}",
            reply.json()
        );
        assert_eq!(
            member(&reply, "Message"),
            FAILED_TO_READ_BLOCK_DATA,
            "case {case:?}: the refusal's sentence is fixed text, got {}",
            reply.json()
        );
    }

    // A block that IS read in full and then rejected on its content keeps the content
    // reason: the two members of the enum must stay distinguishable. (A payload whose
    // CHECKSUM does not match is not in this table: the checksum header is the request's
    // integrity binding, so a mismatch is the signing-integrity refusal rather than a
    // validation reason at all — `payload_checksum_integrity.rs` asserts it.)
    let content_cases: &[(&str, i32, String)] = &[(
        // A 1 GiB volume addresses block indexes 0..2048 (1 GiB / 524288).
        "an index outside the snapshot's volume",
        2048,
        full_checksum.clone(),
    )];
    for (case, block_index, checksum_value) in content_cases {
        let reply = put_block(
            &router,
            &snapshot_id,
            *block_index,
            full.clone(),
            BLOCK_SIZE,
            checksum_value,
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a rejected block must be refused 400, got {} with body {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            member(&reply, "Reason"),
            "INVALID_BLOCK",
            "case {case:?}: a block that was read and rejected keeps the block reason, got {}",
            reply.json()
        );
        assert_ne!(
            member(&reply, "Message"),
            FAILED_TO_READ_BLOCK_DATA,
            "case {case:?}: a block that was read must not report a failed read, got {}",
            reply.json()
        );
    }

    // The payload that DOES deliver the declared bytes is still written, so the refusals
    // above are about the payload and not about the operation.
    let accepted = put_block(
        &router,
        &snapshot_id,
        0,
        full.clone(),
        BLOCK_SIZE,
        &full_checksum,
    )
    .await;
    assert_eq!(
        accepted.status, 201,
        "case \"a payload of exactly the declared length\": must answer 201, got {} with body {}",
        accepted.status,
        String::from_utf8_lossy(&accepted.body)
    );
}

/// RULE 3: `StartSnapshot`'s refusal of a request that asks for both a parent snapshot
/// and encryption is FIXED text — the same sentence whatever else the request carried,
/// interpolating neither the parent id nor any other request value.
#[tokio::test]
async fn the_parent_and_encryption_refusal_is_the_same_fixed_sentence_for_every_request() {
    let router = router();
    let parent = started_snapshot(
        &router,
        "a sealed parent",
        &serde_json::json!({ "VolumeSize": 1 }),
    )
    .await;
    assert_eq!(
        member(&complete_snapshot(&router, &parent, 0).await, "Status"),
        "pending",
        "the parent fixture's seal must be acknowledged against the status it sealed"
    );
    assert_eq!(
        member(&complete_snapshot(&router, &parent, 0).await, "Status"),
        "completed",
        "the parent fixture must be sealed before it is used as a parent"
    );

    // Rows differ along every member the request may carry beside the two that conflict:
    // which parent id (one the service holds, and one it does not), the volume size, and
    // the optional description, tags, timeout and client token.
    let cases: &[(&str, serde_json::Value)] = &[
        (
            "a stored parent, nothing else",
            serde_json::json!({
                "VolumeSize": 1,
                "Encrypted": true,
                "ParentSnapshotId": parent.clone(),
            }),
        ),
        (
            "a parent the service does not hold",
            serde_json::json!({
                "VolumeSize": 64,
                "Encrypted": true,
                "ParentSnapshotId": "snap-0123456789abcdef0",
            }),
        ),
        (
            "with a description, tags and a timeout",
            serde_json::json!({
                "VolumeSize": 4,
                "Encrypted": true,
                "ParentSnapshotId": parent.clone(),
                "Description": "an encrypted child",
                "Tags": [{ "Key": "k", "Value": "v" }],
                "Timeout": 30,
            }),
        ),
        (
            "with a client token and a KMS key",
            serde_json::json!({
                "VolumeSize": 2,
                "Encrypted": true,
                "ParentSnapshotId": parent.clone(),
                "ClientToken": "token-parent-and-encrypt",
                "KmsKeyArn": "arn:aws:kms:us-east-1:000000000000:key/11111111-2222-3333-4444-555555555555",
            }),
        ),
    ];

    for (case, request) in cases {
        let reply = start_snapshot(&router, request).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: the conflicting request must be refused 400, got {} with body {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert!(
            reply.error_code().ends_with("ValidationException"),
            "case {case:?}: expected the ValidationException error code, got {:?}",
            reply.error_code()
        );
        assert_eq!(
            member(&reply, "Reason"),
            "INVALID_PARAMETER_VALUE",
            "case {case:?}: the refusal must carry the parameter-value reason, got {}",
            reply.json()
        );
        let message = member(&reply, "Message");
        assert_eq!(
            message, PARENT_AND_ENCRYPT_REFUSAL,
            "case {case:?}: the refusal's sentence is fixed text, got {message:?}"
        );
        assert!(
            !message.contains("snap-"),
            "case {case:?}: fixed text interpolates no request value, got {message:?}"
        );
    }

    // Either member ALONE is accepted, so the refusal is about the combination and not
    // about one of the members.
    let encrypted_only = start_snapshot(
        &router,
        &serde_json::json!({ "VolumeSize": 1, "Encrypted": true }),
    )
    .await;
    assert_eq!(
        encrypted_only.status, 201,
        "case \"Encrypted alone\": must be accepted, got {} with body {}",
        encrypted_only.status,
        String::from_utf8_lossy(&encrypted_only.body)
    );
    let parent_only = start_snapshot(
        &router,
        &serde_json::json!({ "VolumeSize": 1, "ParentSnapshotId": parent.clone() }),
    )
    .await;
    assert_eq!(
        parent_only.status, 201,
        "case \"ParentSnapshotId alone\": must be accepted, got {} with body {}",
        parent_only.status,
        String::from_utf8_lossy(&parent_only.body)
    );
}
