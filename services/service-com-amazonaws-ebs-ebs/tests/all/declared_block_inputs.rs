// SPDX-License-Identifier: CC0-1.0

//! What the EBS direct APIs do with the values a write request DECLARES about itself:
//! `PutSnapshotBlock`'s `DataLength` and `BlockIndex`, and `CompleteSnapshot`'s
//! `ChangedBlocksCount`.
//!
//! RULE 1: a declared value this service does not accept is refused with the template
//! this service words that refusal in — it names the rejected value taken from the
//! request, plus the single value the service does accept where there is exactly one, and
//! interpolates no other quantity (not the volume it was measured against, not a bound
//! derived from it). `DataLength` has one accepted value, `524288`
//! (`smithy_query(query="doc",shape="com.amazonaws.ebs#PutSnapshotBlockRequest",member="DataLength")`:
//! "Currently, the only supported size is 524288 bytes. Valid values: 524288"), so its
//! refusal names the rejected length and that one; `BlockIndex` is "a logical index in
//! units of 512 KiB blocks … (logical offset of data/524288)" (same query, member
//! `BlockIndex`), so a volume of N GiB addresses `N * 2048` indexes and the refusal of an
//! index at or past that extent names the index only.
//!
//! RULE 2: `ChangedBlocksCount` is the caller's own accounting of its writes ("The number
//! of blocks that were written to the snapshot",
//! `smithy_query(query="doc",shape="com.amazonaws.ebs#CompleteSnapshotRequest",member="ChangedBlocksCount")`),
//! constrained only by the model's `range` min 0
//! (`smithy_query(query="operation",operation="CompleteSnapshot")`): the service does not
//! verify it against the blocks the snapshot holds, so a request carrying any modelled
//! count is accepted and seals the snapshot.
//!
//! Each rule is asserted over a TABLE: the lengths and indexes a request can declare
//! (including each volume's last addressable index and the first one past it, over several
//! volume sizes), and counts above, below and equal to the blocks actually written.

use crate::snapshot_block_loop::{
    block_bytes, checksum, complete_snapshot, list_blocks, listed_blocks, member, put_block,
    router, start_snapshot, Reply, BLOCK_SIZE,
};

/// Block indexes one GiB of volume addresses: one GiB divided by the `524288`-byte block
/// `BlockIndex` counts in.
const BLOCKS_PER_GIB: i32 = (1024 * 1024 * 1024) / BLOCK_SIZE;

/// The template `PutSnapshotBlock` refuses an unsupported `DataLength` with.
fn invalid_length_message(data_length: i32) -> String {
    format!("Block data length {data_length} is not valid, expected {BLOCK_SIZE}")
}

/// The template `PutSnapshotBlock` refuses an out-of-extent `BlockIndex` with.
fn beyond_extent_message(block_index: i32) -> String {
    format!("Block index {block_index} is beyond the volume size extent")
}

/// Start a snapshot of `volume_size` GiB and return the router it was started on with
/// its id.
async fn pending_snapshot(
    case: &str,
    volume_size: i64,
) -> (smithy_server_runtime::MultiServiceRouter, String) {
    let router = router();
    let started = start_snapshot(&router, &serde_json::json!({ "VolumeSize": volume_size })).await;
    assert_eq!(
        started.status, 201,
        "case {case:?}: StartSnapshot must answer 201, got {} with body {}",
        started.status,
        String::from_utf8_lossy(&started.body)
    );
    let snapshot_id = member(&started, "SnapshotId");
    assert!(
        snapshot_id.starts_with("snap-"),
        "case {case:?}: StartSnapshot must mint a snapshot id, got {snapshot_id:?}"
    );
    (router, snapshot_id)
}

/// Assert `reply` is the modelled `ValidationException` carrying `reason` and `message`.
fn assert_validation(case: &str, reply: &Reply, reason: &str, message: &str) {
    assert_eq!(
        reply.status, 400,
        "case {case:?}: a declared value the service does not accept must be refused 400, \
         got {} with body {}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    assert!(
        reply.error_code().ends_with("ValidationException"),
        "case {case:?}: expected the ValidationException error code, got {:?}",
        reply.error_code()
    );
    assert_eq!(
        member(reply, "Reason"),
        reason,
        "case {case:?}: expected Reason {reason}, got {}",
        reply.json()
    );
    assert_eq!(
        member(reply, "Message"),
        message,
        "case {case:?}: the refusal must be worded as this service words it, got {}",
        reply.json()
    );
}

/// RULE 1, `DataLength`: every declared length other than the one supported size is
/// refused with the template naming the rejected length and that size — whatever the
/// payload the request carries, which here is always a full, correctly checksummed block.
/// The last rows pin the precedence: the snapshot the request names is resolved, and its
/// ability to take a block judged, before the declared length is.
#[tokio::test]
async fn an_unsupported_declared_data_length_is_refused_by_the_length_it_declared() {
    let (router, snapshot_id) = pending_snapshot("declared lengths", 1).await;
    let data = block_bytes(71);
    let data_checksum = checksum(&data);

    // Rows differ along what the rule generalises over — the declared length itself:
    // negative, zero, one byte, a round value no block has, half a block, one short of a
    // block, one over, and two blocks.
    let declared_lengths: &[i32] = &[
        -1,
        0,
        1,
        1000,
        BLOCK_SIZE / 2,
        BLOCK_SIZE - 1,
        BLOCK_SIZE + 1,
        BLOCK_SIZE * 2,
    ];

    for declared in declared_lengths {
        let case = format!("a declared DataLength of {declared}");
        let reply = put_block(
            &router,
            &snapshot_id,
            0,
            data.clone(),
            *declared,
            &data_checksum,
        )
        .await;
        assert_validation(
            &case,
            &reply,
            "INVALID_PARAMETER_VALUE",
            &invalid_length_message(*declared),
        );
    }

    // The one supported size is still written, so the refusals above are about the
    // declared length and not about the operation.
    let accepted = put_block(
        &router,
        &snapshot_id,
        0,
        data.clone(),
        BLOCK_SIZE,
        &data_checksum,
    )
    .await;
    assert_eq!(
        accepted.status, 201,
        "case \"a declared DataLength of {BLOCK_SIZE}\": the supported size must answer 201, \
         got {} with body {}",
        accepted.status,
        String::from_utf8_lossy(&accepted.body)
    );

    // Precedence: the declared length is judged only once the request has resolved to a
    // snapshot that can take a block, so the snapshot a request names answers first.
    let unknown = put_block(
        &router,
        "snap-deadbeef",
        0,
        data.clone(),
        1000,
        &data_checksum,
    )
    .await;
    assert_eq!(
        unknown.status, 404,
        "case \"an unsupported length against an unknown snapshot\": the snapshot must be \
         reported first, got {} with body {}",
        unknown.status,
        String::from_utf8_lossy(&unknown.body)
    );
    assert!(
        unknown.error_code().ends_with("ResourceNotFoundException"),
        "case \"an unsupported length against an unknown snapshot\": expected the \
         ResourceNotFoundException error code, got {:?}",
        unknown.error_code()
    );

    let sealed = complete_snapshot(&router, &snapshot_id, 1).await;
    assert_eq!(
        sealed.status, 202,
        "case \"an unsupported length against a sealed snapshot\": the fixture must seal, got \
         {} with body {}",
        sealed.status,
        String::from_utf8_lossy(&sealed.body)
    );
    let on_sealed = put_block(
        &router,
        &snapshot_id,
        0,
        data.clone(),
        1000,
        &data_checksum,
    )
    .await;
    assert_eq!(
        member(&on_sealed, "Reason"),
        "INVALID_SNAPSHOT_ID",
        "case \"an unsupported length against a sealed snapshot\": a snapshot that takes no \
         further block must be reported before the declared length, got {}",
        on_sealed.json()
    );
    assert_ne!(
        member(&on_sealed, "Message"),
        invalid_length_message(1000),
        "case \"an unsupported length against a sealed snapshot\": the refusal must not be the \
         length one, got {}",
        on_sealed.json()
    );
}

/// RULE 1, `BlockIndex`: an index at or past the extent of the snapshot's own volume is
/// refused with the template naming the index and nothing else; the last index the volume
/// does address is written.
#[tokio::test]
async fn an_index_past_the_volume_extent_is_refused_by_the_index_alone() {
    // Rows differ along the volume size, which is what moves the extent: the smallest
    // volume the model admits (`VolumeSize` `range` min 1) and two larger ones.
    for volume_size in [1_i64, 2, 5] {
        let case_volume = format!("a {volume_size} GiB volume");
        let (router, snapshot_id) = pending_snapshot(&case_volume, volume_size).await;
        let extent =
            i32::try_from(volume_size).expect("the volume size fits an i32") * BLOCKS_PER_GIB;
        let data = block_bytes(72);
        let data_checksum = checksum(&data);

        // The bound itself and one past it, plus an index far outside every volume.
        for beyond in [extent, extent + 1, 99_999] {
            let case = format!("{case_volume}: BlockIndex {beyond}");
            let reply = put_block(
                &router,
                &snapshot_id,
                beyond,
                data.clone(),
                BLOCK_SIZE,
                &data_checksum,
            )
            .await;
            assert_validation(
                &case,
                &reply,
                "INVALID_BLOCK",
                &beyond_extent_message(beyond),
            );
        }

        // The last index inside the extent is accepted, so the refusals above are about
        // the extent and not about the index being large.
        let last = extent - 1;
        let case = format!("{case_volume}: BlockIndex {last}");
        let accepted = put_block(
            &router,
            &snapshot_id,
            last,
            data.clone(),
            BLOCK_SIZE,
            &data_checksum,
        )
        .await;
        assert_eq!(
            accepted.status, 201,
            "case {case:?}: the last index the volume addresses must answer 201, got {} with \
             body {}",
            accepted.status,
            String::from_utf8_lossy(&accepted.body)
        );
    }
}

/// One row of the `ChangedBlocksCount` table: the blocks written into the snapshot and
/// the count the `CompleteSnapshot` request then declares.
struct CountRow {
    case: &'static str,
    blocks: &'static [(i32, u8)],
    declared_count: i32,
}

/// RULE 2: `CompleteSnapshot` accepts the `ChangedBlocksCount` the request declares
/// whatever the snapshot holds — more, fewer or exactly as many blocks as were written —
/// and seals the snapshot, which the listing of the written indexes then proves.
#[tokio::test]
async fn any_modelled_changed_blocks_count_seals_the_snapshot() {
    // Rows differ along the declared count against the blocks actually written: equal,
    // zero against writes, one short, one over, and the largest count the header can
    // carry.
    let rows: &[CountRow] = &[
        CountRow {
            case: "a count that matches the writes",
            blocks: &[(0, 81), (1, 82)],
            declared_count: 2,
        },
        CountRow {
            case: "a count of zero against two writes",
            blocks: &[(0, 83), (1, 84)],
            declared_count: 0,
        },
        CountRow {
            case: "a count one short of the writes",
            blocks: &[(0, 85), (3, 86)],
            declared_count: 1,
        },
        CountRow {
            case: "a count one over the writes",
            blocks: &[(0, 87)],
            declared_count: 2,
        },
        CountRow {
            case: "a count far over the writes",
            blocks: &[(0, 88)],
            declared_count: 7,
        },
        CountRow {
            case: "the largest count the header carries",
            blocks: &[(2, 89)],
            declared_count: i32::MAX,
        },
        CountRow {
            case: "a count against a snapshot with no writes",
            blocks: &[],
            declared_count: 4,
        },
    ];

    for row in rows {
        let CountRow {
            case,
            blocks,
            declared_count,
        } = row;
        let (router, snapshot_id) = pending_snapshot(case, 1).await;
        for (block_index, seed) in *blocks {
            let data = block_bytes(*seed);
            let data_checksum = checksum(&data);
            let written = put_block(
                &router,
                &snapshot_id,
                *block_index,
                data,
                BLOCK_SIZE,
                &data_checksum,
            )
            .await;
            assert_eq!(
                written.status, 201,
                "case {case:?}: writing block {block_index} must answer 201, got {} with body {}",
                written.status,
                String::from_utf8_lossy(&written.body)
            );
        }

        let sealed = complete_snapshot(&router, &snapshot_id, *declared_count).await;
        assert_eq!(
            sealed.status, 202,
            "case {case:?}: CompleteSnapshot with a declared count of {declared_count} must be \
             accepted 202, got {} with body {}",
            sealed.status,
            String::from_utf8_lossy(&sealed.body)
        );
        assert_eq!(
            member(&sealed, "Status"),
            "pending",
            "case {case:?}: the acknowledgement reports the status the seal was applied to, got {}",
            sealed.json()
        );

        // The seal really happened: the snapshot is readable, and the listing holds
        // exactly the indexes written to it — the declared count changed nothing about
        // what is stored.
        let listing = list_blocks(&router, &snapshot_id, "").await;
        assert_eq!(
            listing.status, 200,
            "case {case:?}: a sealed snapshot must be listable, got {} with body {}",
            listing.status,
            String::from_utf8_lossy(&listing.body)
        );
        let listed: Vec<i32> = listed_blocks(&listing)
            .iter()
            .map(|(index, _)| *index)
            .collect();
        let expected: Vec<i32> = blocks.iter().map(|(index, _)| *index).collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: the listing must hold exactly the written indexes, got {listed:?}"
        );
        assert_eq!(
            member(
                &complete_snapshot(&router, &snapshot_id, *declared_count).await,
                "Status"
            ),
            "completed",
            "case {case:?}: a later call must read the sealed status back off the record"
        );
    }
}
