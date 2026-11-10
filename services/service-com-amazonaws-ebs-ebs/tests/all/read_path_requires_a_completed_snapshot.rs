// SPDX-License-Identifier: CC0-1.0

//! What the READ path of the EBS direct APIs can see.
//!
//! RULE: a snapshot is visible to the read operations only once its content is FINAL —
//! `CompleteSnapshot` "seals and completes the snapshot after all of the required blocks
//! of data have been written to it"
//! (`smithy_query(query="operation",operation="CompleteSnapshot")`) — so a read of a
//! snapshot that is still being written resolves nothing, exactly as a read of an id no
//! snapshot was ever minted for does: HTTP 404 `ResourceNotFoundException` with `Reason`
//! `SNAPSHOT_NOT_FOUND` and the sentence naming the snapshot. An empty HTTP 200 is a
//! different claim — "this snapshot exists and holds no block" — and a caller that reads
//! readability as the completion signal concludes from it that a half-written snapshot is
//! complete.
//!
//! Asserted over a table whose rows differ in how many blocks the snapshot holds, at which
//! indexes and over which volume size, and across BOTH read operations, each row checked
//! before the seal (not found) and after it (served). The gate is the snapshot's status,
//! so it also covers a snapshot its `Timeout` cancelled — the one other status the read
//! path cannot see — which no request can reach inside a test: the documented minimum
//! `Timeout` is 10 minutes
//! (`smithy_query(query="shape",shape="com.amazonaws.ebs#StartSnapshotRequest")`, member
//! `Timeout` `range` 10..4320).

use crate::snapshot_block_loop::{
    block_bytes, checksum, complete_snapshot, get_block, list_blocks, listed_blocks, member,
    put_block, router, start_snapshot, write_snapshot, Reply, BLOCK_SIZE,
};

/// The `Reason` a snapshot the read path cannot resolve is reported under, a member of
/// `ResourceNotFoundExceptionReason`
/// (`smithy_query(query="shape",shape="com.amazonaws.ebs#ResourceNotFoundExceptionReason")`).
const SNAPSHOT_NOT_FOUND_REASON: &str = "SNAPSHOT_NOT_FOUND";

/// The sentence every snapshot not-found refusal carries, naming the snapshot.
pub(crate) fn not_found_message(snapshot_id: &str) -> String {
    format!("The snapshot '{snapshot_id}' does not exist.")
}

/// Assert `reply` is the snapshot not-found refusal for `snapshot_id`.
pub(crate) fn assert_not_found(case: &str, reply: &Reply, snapshot_id: &str) {
    assert_eq!(
        reply.status, 404,
        "case {case:?}: a snapshot the read path cannot see must be refused 404, got {} with \
         body {}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    assert!(
        reply.error_code().ends_with("ResourceNotFoundException"),
        "case {case:?}: expected the ResourceNotFoundException error code, got {:?}",
        reply.error_code()
    );
    assert_eq!(
        member(reply, "Reason"),
        SNAPSHOT_NOT_FOUND_REASON,
        "case {case:?}: expected Reason {SNAPSHOT_NOT_FOUND_REASON}, got {}",
        reply.json()
    );
    assert_eq!(
        member(reply, "Message"),
        not_found_message(snapshot_id),
        "case {case:?}: the refusal must name the snapshot, got {}",
        reply.json()
    );
    assert!(
        reply.json().get("Blocks").is_none(),
        "case {case:?}: a refusal must report no listing, got {}",
        reply.json()
    );
}

/// One snapshot of the table: the volume size to start it with and the blocks to write
/// into it before (and after) the seal.
struct Fixture {
    case: &'static str,
    volume_size: i64,
    blocks: &'static [(i32, u8)],
}

/// Rows differ along what the rule generalises over: a snapshot with no block at all, one
/// with a single block, one with several at non-adjacent indexes, and one over a larger
/// volume with a high index.
const FIXTURES: &[Fixture] = &[
    Fixture {
        case: "a snapshot with no block written",
        volume_size: 1,
        blocks: &[],
    },
    Fixture {
        case: "a snapshot with one block",
        volume_size: 1,
        blocks: &[(0, 81)],
    },
    Fixture {
        case: "a snapshot with blocks at non-adjacent indexes",
        volume_size: 2,
        blocks: &[(0, 82), (3, 83)],
    },
    Fixture {
        case: "a snapshot over a larger volume",
        volume_size: 8,
        blocks: &[(4096, 84)],
    },
];

#[tokio::test]
async fn a_snapshot_that_is_still_being_written_is_not_visible_to_the_read_path() {
    for Fixture {
        case,
        volume_size,
        blocks,
    } in FIXTURES
    {
        let router = router();
        let started =
            start_snapshot(&router, &serde_json::json!({ "VolumeSize": volume_size })).await;
        let snapshot_id = started
            .json()
            .get("SnapshotId")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            started
                .json()
                .get("Status")
                .and_then(serde_json::Value::as_str),
            Some("pending"),
            "case {case:?}: a new snapshot enters the pending state, got {}",
            started.json()
        );

        for (block_index, seed) in *blocks {
            let data = block_bytes(*seed);
            let expected = checksum(&data);
            let written =
                put_block(&router, &snapshot_id, *block_index, data, BLOCK_SIZE, &expected).await;
            assert_eq!(
                written.status, 201,
                "case {case:?}: PutSnapshotBlock at index {block_index} must answer 201, got {} \
                 with body {}",
                written.status,
                String::from_utf8_lossy(&written.body)
            );
        }

        // BEFORE the seal: neither read resolves the snapshot, however many blocks the
        // write path put into it.
        let listed = list_blocks(&router, &snapshot_id, "").await;
        assert_not_found(
            &format!("{case}, before the seal: ListSnapshotBlocks"),
            &listed,
            &snapshot_id,
        );

        // A token minted for ANOTHER, readable snapshot: the snapshot gate is reached
        // before the token is resolved, so the answer is still the snapshot refusal.
        let readable = write_snapshot(&router, case, 1, &[(0, 99)]).await;
        let other_token = listed_blocks(&list_blocks(&router, &readable, "").await)
            .first()
            .map(|(_, token)| token.clone())
            .expect("the readable fixture lists its block");
        let fetched = get_block(&router, &snapshot_id, 0, &other_token).await;
        assert_not_found(
            &format!("{case}, before the seal: GetSnapshotBlock"),
            &fetched,
            &snapshot_id,
        );

        // AFTER the seal: the very same reads are served, so the gate is the snapshot's
        // status and nothing else.
        let count = i32::try_from(blocks.len()).expect("a small block count");
        let sealed = complete_snapshot(&router, &snapshot_id, count).await;
        assert_eq!(
            sealed.status, 202,
            "case {case:?}: CompleteSnapshot must answer 202, got {} with body {}",
            sealed.status,
            String::from_utf8_lossy(&sealed.body)
        );

        let listed = list_blocks(&router, &snapshot_id, "").await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}, after the seal: ListSnapshotBlocks must answer 200, got {} with \
             body {}",
            listed.status,
            String::from_utf8_lossy(&listed.body)
        );
        let reported: Vec<i32> = listed_blocks(&listed)
            .iter()
            .map(|(index, _)| *index)
            .collect();
        let mut expected: Vec<i32> = blocks.iter().map(|(index, _)| *index).collect();
        expected.sort_unstable();
        assert_eq!(
            reported, expected,
            "case {case:?}, after the seal: the listing must report the blocks written, got \
             {reported:?}"
        );
        for (block_index, token) in &listed_blocks(&listed) {
            let fetched = get_block(&router, &snapshot_id, *block_index, token).await;
            assert_eq!(
                fetched.status, 200,
                "case {case:?}, after the seal: block {block_index} must be fetchable, got {} \
                 with body {}",
                fetched.status,
                String::from_utf8_lossy(&fetched.body)
            );
        }
    }
}

/// On a snapshot the read path CAN see, a token that names no block of it is the token
/// refusal rather than the snapshot one: the two refusals stay distinguishable, which is
/// what tells a caller to re-list for a fresh token instead of abandoning the snapshot.
#[tokio::test]
async fn a_readable_snapshot_refuses_a_token_that_names_none_of_its_blocks() {
    let router = router();
    let snapshot_id = write_snapshot(&router, "token refusals", 1, &[(0, 91)]).await;
    let other = write_snapshot(&router, "another snapshot", 1, &[(0, 92)]).await;
    let other_token = listed_blocks(&list_blocks(&router, &other, "").await)
        .first()
        .map(|(_, token)| token.clone())
        .expect("the other snapshot lists its block");

    // Rows: a token this service never minted, one minted for another snapshot, and one
    // minted for this snapshot but presented at an index it does not name.
    let own_token = listed_blocks(&list_blocks(&router, &snapshot_id, "").await)
        .first()
        .map(|(_, token)| token.clone())
        .expect("the snapshot lists its block");
    let cases: &[(&str, i32, String)] = &[
        (
            "a token this service never minted",
            0,
            "QUJD".to_string(),
        ),
        (
            "a token minted for another snapshot",
            0,
            other_token,
        ),
        (
            "a token of this snapshot at an index it does not name",
            1,
            own_token,
        ),
    ];

    for (case, block_index, token) in cases {
        let reply = get_block(&router, &snapshot_id, *block_index, token).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: an unresolvable token must be refused 400, got {} with body {}",
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
            "INVALID_BLOCK_TOKEN",
            "case {case:?}: a rejected token is not a missing snapshot, got {}",
            reply.json()
        );
        assert_eq!(
            member(&reply, "Message"),
            format!("The block token {token} is invalid."),
            "case {case:?}: the refusal must name the rejected token, got {}",
            reply.json()
        );
    }
}
