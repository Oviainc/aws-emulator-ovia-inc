// SPDX-License-Identifier: CC0-1.0

//! Which refusal a request that breaks more than one rule is answered with.
//!
//! RULE: every refusal of the EBS direct APIs is decided at a fixed point of the request
//! walk, so a request carrying several defects is answered by the FIRST gate it reaches
//! and the later gates are never consulted. Two orderings follow from what each gate is
//! about:
//!
//! 1. `PutSnapshotBlock` carries its block as an UNSIGNED payload and `x-amz-Checksum` is
//!    what binds those bytes to the signed request (`BlockData` is the `@httpPayload` and
//!    `Checksum` a `@required` `@httpHeader`,
//!    `smithy_query(query="operation",operation="PutSnapshotBlock")`), so a payload whose
//!    checksum names other bytes fails the request's INTEGRITY — HTTP 403
//!    `InvalidSignatureException`, stating the checksum computed over the bytes that
//!    arrived and the one the request supplied — before the request is allowed to say
//!    anything about a snapshot or about the block's own declared values.
//! 2. The read path resolves the SNAPSHOT before it resolves a block of it, so a snapshot
//!    the read path cannot see is reported as the absent snapshot — HTTP 404
//!    `ResourceNotFoundException` with `Reason` `SNAPSHOT_NOT_FOUND`
//!    (`smithy_query(query="shape",shape="com.amazonaws.ebs#ResourceNotFoundExceptionReason")`)
//!    — whatever `BlockToken` the request presented, one the service never minted
//!    included.
//!
//! The order is behaviour, not an implementation detail, because a caller branches on the
//! class: a write refused 404 instead of 403 tells a client its snapshot vanished when the
//! truth is that its payload arrived corrupted, and a read of a half-written snapshot
//! refused for its token tells a client to re-list for a fresh token when the snapshot is
//! not readable at all.
//!
//! Each ordering is asserted over a table of targets and defects, and every row is driven
//! a second time WITHOUT the earlier defect, so the later gate's own answer is observed
//! too: without those controls the rows would also pass against a service that only ever
//! answered one class.

use crate::declared_block_inputs::{
    assert_validation, beyond_extent_message, invalid_length_message,
};
use crate::payload_checksum_integrity::{lowercase_message, mismatch_message};
use crate::read_path_requires_a_completed_snapshot::assert_not_found;
use crate::snapshot_block_loop::{
    block_bytes, checksum, complete_snapshot, get_block, list_blocks, listed_blocks, member,
    put_block, router, start_snapshot, write_snapshot, Reply, BLOCK_SIZE,
};

/// A well-formed snapshot id nothing holds: 17 hex digits, one of the two widths this
/// service's ids carry, so the row exercises "no such snapshot" and not "not an id".
const UNSTORED_ID: &str = "snap-0123456789abcdef0";

/// A value shaped like a snapshot id but of a width no id of this service has (7 hex
/// digits), so it is a malformed parameter rather than a snapshot that is not stored. It
/// still satisfies the modelled `SnapshotId` pattern `^snap-[0-9a-f]+$`, so it reaches the
/// handler.
const MALFORMED_ID: &str = "snap-aaaaaaa";

/// A declared `DataLength` the service never writes: `PutSnapshotBlockRequest$DataLength`
/// documents "Currently, the only supported size is 524288 bytes".
const UNSUPPORTED_DATA_LENGTH: i32 = 1000;

/// A block index past the extent of a 1 GiB volume, which addresses the indexes below
/// 1 GiB / 524288.
const INDEX_PAST_EXTENT: i32 = 99_999;

/// What a request earns once no earlier gate refuses it. Each refusal class is asserted
/// by the suite that owns it — `declared_block_inputs::assert_validation` for the modelled
/// `ValidationException` and `read_path_requires_a_completed_snapshot::assert_not_found`
/// for the absent snapshot — so this suite adds no third copy of either expectation.
enum Expected {
    /// The write is stored (HTTP 201).
    Accepted,
    /// The snapshot does not resolve: the not-found refusal naming this id.
    SnapshotNotFound(String),
    /// The modelled `ValidationException` carrying this `Reason` and `Message`.
    Validation {
        reason: &'static str,
        message: String,
    },
}

/// Assert `reply` is `expected`.
fn assert_expected(case: &str, reply: &Reply, expected: &Expected) {
    match expected {
        Expected::Accepted => assert_eq!(
            reply.status, 201,
            "case {case:?}: expected the write to be stored with HTTP 201, got {} with body {}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        ),
        Expected::SnapshotNotFound(snapshot_id) => assert_not_found(case, reply, snapshot_id),
        Expected::Validation { reason, message } => {
            assert_validation(case, reply, reason, message);
        }
    }
}

/// Assert `reply` is the integrity refusal for a payload of `data` presented as
/// `supplied`.
fn assert_integrity_refusal(case: &str, reply: &Reply, data: &[u8], supplied: &str) {
    assert_eq!(
        reply.status, 403,
        "case {case:?}: a payload the checksum does not name must be refused 403, got {} with \
         body {}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    assert!(
        reply.error_code().ends_with("InvalidSignatureException"),
        "case {case:?}: expected the InvalidSignatureException error code, got {:?}",
        reply.error_code()
    );
    assert_eq!(
        lowercase_message(reply),
        mismatch_message(&checksum(data), supplied),
        "case {case:?}: the refusal must state the checksum computed over the bytes that \
         arrived and the one supplied, got {}",
        reply.json()
    );
}

/// One row of the second defect dimension: what else the write gets wrong beside its
/// checksum.
struct Defect {
    case: &'static str,
    data_length: i32,
    block_index: i32,
}

/// Rows differ along the other things a write can get wrong — nothing, the length it
/// declares, and the index it names — none of which is consulted once the payload's
/// integrity has failed.
const DEFECTS: &[Defect] = &[
    Defect {
        case: "the checksum alone",
        data_length: BLOCK_SIZE,
        block_index: 0,
    },
    Defect {
        case: "the checksum and a declared length the service never writes",
        data_length: UNSUPPORTED_DATA_LENGTH,
        block_index: 0,
    },
    Defect {
        case: "the checksum and an index past the volume extent",
        data_length: BLOCK_SIZE,
        block_index: INDEX_PAST_EXTENT,
    },
];

/// One row of the read table: a snapshot the read path cannot resolve, and the
/// `ChangedBlocksCount` that seals it for the control — `None` for the row that names no
/// snapshot at all and so cannot be sealed.
struct Unreadable {
    case: &'static str,
    snapshot_id: String,
    sealable_with: Option<i32>,
}

#[tokio::test]
async fn an_integrity_failure_is_answered_before_the_snapshot_and_the_block_are_judged() {
    let router = router();

    // The targets a write can name, each with the answer it earns on its own: one that
    // accepts the write, one that has been sealed, a well-formed id nothing holds, and a
    // value that is not shaped like an id of this service.
    let pending = start_snapshot(&router, &serde_json::json!({ "VolumeSize": 1 })).await;
    let pending_id = member(&pending, "SnapshotId");
    assert_eq!(
        pending.status, 201,
        "the fixture snapshot must start, got {} with body {}",
        pending.status,
        String::from_utf8_lossy(&pending.body)
    );
    let sealed_id = write_snapshot(&router, "a sealed snapshot", 1, &[(0, 73)]).await;

    let targets: Vec<(&str, String, Expected)> = vec![
        (
            "a snapshot still being written",
            pending_id.clone(),
            Expected::Accepted,
        ),
        (
            "a snapshot that has been sealed",
            sealed_id,
            Expected::Validation {
                reason: "INVALID_SNAPSHOT_ID",
                message: "Snapshot is already completed".to_string(),
            },
        ),
        (
            "a well-formed id nothing holds",
            UNSTORED_ID.to_string(),
            Expected::SnapshotNotFound(UNSTORED_ID.to_string()),
        ),
        (
            "a value that is not shaped like a snapshot id",
            MALFORMED_ID.to_string(),
            Expected::Validation {
                reason: "INVALID_SNAPSHOT_ID",
                message: format!(
                    "Value ({MALFORMED_ID}) for parameter snapshotId is invalid. Expected: \
                     'snap-...'."
                ),
            },
        ),
    ];

    // Every target x every defect: the integrity refusal, which says nothing about the
    // snapshot or the block, however much else the request got wrong.
    for (target, snapshot_id, _) in &targets {
        for defect in DEFECTS {
            let data = block_bytes(71);
            let supplied = checksum(&block_bytes(72));
            let reply = put_block(
                &router,
                snapshot_id,
                defect.block_index,
                data.clone(),
                defect.data_length,
                &supplied,
            )
            .await;
            assert_integrity_refusal(
                &format!("{target}, refused for {}", defect.case),
                &reply,
                &data,
                &supplied,
            );
        }
    }

    // The controls that make the rows above mean something: the SAME targets, with a
    // checksum that does name the payload, are answered by the gate that comes next.
    for (target, snapshot_id, expected) in &targets {
        let data = block_bytes(71);
        let supplied = checksum(&data);
        let reply = put_block(&router, snapshot_id, 0, data, BLOCK_SIZE, &supplied).await;
        assert_expected(
            &format!("{target}, with the checksum of its own payload"),
            &reply,
            expected,
        );
    }

    // And the controls for the other two defects, on the target that accepts a write:
    // each is really refused once the integrity gate lets the request past, with the
    // template that names the rejected value.
    let declared = put_block(
        &router,
        &pending_id,
        1,
        block_bytes(74),
        UNSUPPORTED_DATA_LENGTH,
        &checksum(&block_bytes(74)),
    )
    .await;
    assert_expected(
        "a declared length the service never writes, with a matching checksum",
        &declared,
        &Expected::Validation {
            reason: "INVALID_PARAMETER_VALUE",
            message: invalid_length_message(UNSUPPORTED_DATA_LENGTH),
        },
    );
    let beyond = put_block(
        &router,
        &pending_id,
        INDEX_PAST_EXTENT,
        block_bytes(75),
        BLOCK_SIZE,
        &checksum(&block_bytes(75)),
    )
    .await;
    assert_expected(
        "an index past the volume extent, with a matching checksum",
        &beyond,
        &Expected::Validation {
            reason: "INVALID_BLOCK",
            message: beyond_extent_message(INDEX_PAST_EXTENT),
        },
    );
}

#[tokio::test]
async fn an_unreadable_snapshot_is_reported_before_a_block_token_is_resolved() {
    let router = router();

    // A token this service really minted, for another, readable snapshot: the one row
    // whose token names a block of SOMETHING, so the refusals below cannot be explained
    // by the token being unresolvable everywhere.
    let other = write_snapshot(&router, "another snapshot", 1, &[(0, 88)]).await;
    let foreign_token = listed_blocks(&list_blocks(&router, &other, "").await)
        .first()
        .map(|(_, token)| token.clone())
        .expect("the other snapshot lists its block");

    // Rows differ in the token's provenance and in its spelling: one the service never
    // minted, a longer one it never minted, and one it minted for another snapshot.
    let tokens: Vec<(&str, String)> = vec![
        ("a token this service never minted", "QUJD".to_string()),
        (
            "a longer token this service never minted",
            "QUJDREVGR0hJSg==".to_string(),
        ),
        ("a token minted for another snapshot", foreign_token),
    ];

    // Rows differ in why the snapshot is unreadable and in what it holds: one still being
    // written with a block at index 0, one still being written with no block at all, and
    // an id nothing holds.
    let open_with_block = start_snapshot(&router, &serde_json::json!({ "VolumeSize": 1 })).await;
    let open_with_block = member(&open_with_block, "SnapshotId");
    let data = block_bytes(89);
    let written = put_block(
        &router,
        &open_with_block,
        0,
        data.clone(),
        BLOCK_SIZE,
        &checksum(&data),
    )
    .await;
    assert_eq!(
        written.status, 201,
        "the block of the open fixture must be written, got {} with body {}",
        written.status,
        String::from_utf8_lossy(&written.body)
    );
    let started_empty = start_snapshot(&router, &serde_json::json!({ "VolumeSize": 1 })).await;
    assert_eq!(
        started_empty.status, 201,
        "the empty open fixture must start, got {} with body {}",
        started_empty.status,
        String::from_utf8_lossy(&started_empty.body)
    );
    let open_empty = member(&started_empty, "SnapshotId");
    for (fixture, snapshot_id) in [
        ("the open fixture holding a block", &open_with_block),
        ("the empty open fixture", &open_empty),
    ] {
        assert!(
            snapshot_id.starts_with("snap-"),
            "{fixture} must report a minted snapshot id, got {snapshot_id:?}"
        );
    }

    let unreadable: Vec<Unreadable> = vec![
        Unreadable {
            case: "a snapshot still being written, holding a block",
            snapshot_id: open_with_block.clone(),
            sealable_with: Some(1),
        },
        Unreadable {
            case: "a snapshot still being written, holding nothing",
            snapshot_id: open_empty.clone(),
            sealable_with: Some(0),
        },
        Unreadable {
            case: "a well-formed id nothing holds",
            snapshot_id: UNSTORED_ID.to_string(),
            sealable_with: None,
        },
    ];

    // Every unreadable snapshot x every token x the index the token is presented at: the
    // snapshot refusal, which reports the snapshot and not the token.
    for Unreadable {
        case: target,
        snapshot_id,
        ..
    } in &unreadable
    {
        for (provenance, token) in &tokens {
            for block_index in [0, 7] {
                let reply = get_block(&router, snapshot_id, block_index, token).await;
                let case = format!("{target}, read with {provenance} at index {block_index}");
                assert_expected(
                    &case,
                    &reply,
                    &Expected::SnapshotNotFound(snapshot_id.clone()),
                );
                assert!(
                    reply.json().get("BlockToken").is_none(),
                    "case {case:?}: the refusal must report nothing about the token, got {}",
                    reply.json()
                );
            }
        }
    }

    // The control: sealing is the only thing that changes, and the very same reads are
    // then answered by the gate that comes next — the token refusal, which names the
    // token.
    for Unreadable {
        case: target,
        snapshot_id,
        sealable_with,
    } in &unreadable
    {
        // The id nothing holds has no snapshot to seal, so it is the one row this control
        // cannot be driven for.
        let Some(changed_blocks) = sealable_with else {
            continue;
        };
        let sealed = complete_snapshot(&router, snapshot_id, *changed_blocks).await;
        assert_eq!(
            sealed.status, 202,
            "case {target:?}: CompleteSnapshot must answer 202, got {} with body {}",
            sealed.status,
            String::from_utf8_lossy(&sealed.body)
        );
        for (provenance, token) in &tokens {
            for block_index in [0, 7] {
                let reply = get_block(&router, snapshot_id, block_index, token).await;
                assert_expected(
                    &format!(
                        "{target}, sealed, read with {provenance} at index {block_index}"
                    ),
                    &reply,
                    &Expected::Validation {
                        reason: "INVALID_BLOCK_TOKEN",
                        message: format!("The block token {token} is invalid."),
                    },
                );
            }
        }
    }

    // And the other half of the control: once sealed, the block the snapshot really holds
    // is served to the token the service minted for it, so the refusals above were about
    // the snapshot's phase and not about the read path refusing everything.
    let listing = list_blocks(&router, &open_with_block, "").await;
    let listed = listed_blocks(&listing);
    assert_eq!(
        listed.iter().map(|(index, _)| *index).collect::<Vec<i32>>(),
        vec![0],
        "the sealed snapshot must list the block written into it, got {}",
        listing.json()
    );
    for (block_index, token) in &listed {
        let fetched = get_block(&router, &open_with_block, *block_index, token).await;
        assert_eq!(
            fetched.status, 200,
            "case \"the snapshot's own token at index {block_index}\": expected the block to be \
             served, got {} with body {}",
            fetched.status,
            String::from_utf8_lossy(&fetched.body)
        );
        assert_eq!(
            fetched.body, data,
            "case \"the snapshot's own token at index {block_index}\": expected the bytes written \
             into the block"
        );
    }
}
