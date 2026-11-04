// SPDX-License-Identifier: CC0-1.0

//! The opaque tokens this service MINTS for a caller to hand back to it in a URI query.
//!
//! RULE: `GetSnapshotBlock`'s `BlockToken` and the listings' `NextToken` are `@httpQuery`
//! members — `blockToken` and `pageToken`
//! (`smithy_query(query="operation",operation="GetSnapshotBlock,ListSnapshotBlocks")`) — so
//! every value this service mints for one is spelled with characters RFC 3986 §2.3 leaves
//! UNRESERVED (`A-Z`, `a-z`, `0-9`, `-`, `.`, `_`, `~`). A query value drawn only from
//! those has a single wire spelling: escaped or not, by whichever client, it travels as
//! the bytes that were minted and arrives as the bytes the service compares. A token
//! spelled with characters a query reserves (`+`, `/`, `=`) has several wire spellings
//! instead, so the same token reaches the service as different bytes from different
//! callers — and the bytes a request signature is computed over stop agreeing with the
//! bytes the token is read from, which refuses the read before it ever reaches the
//! handler.
//!
//! Both halves of the rule are asserted over a TABLE of snapshots — differing in how many
//! blocks they hold, at which indexes, over which volume size, and whether the block was
//! written by `PutSnapshotBlock` or inherited from a `ParentSnapshotId` — plus a paginated
//! listing, which is where a `NextToken` is minted:
//!
//! 1. every minted `BlockToken` and `NextToken` is drawn from the unreserved set (and
//!    stays inside the modelled `^[A-Za-z0-9+/=]+$` and `@length` 0..256);
//! 2. a minted token placed in the query VERBATIM, with no escaping at all, names exactly
//!    the block (or page) it was minted for.

use smithy_server_runtime::MultiServiceRouter;

use crate::snapshot_block_loop::{
    block_bytes, checksum, complete_snapshot, list_blocks, listed_blocks, router, send,
    start_snapshot, write_snapshot, Reply, BLOCK_SIZE,
};

/// Whether every byte of `value` is unreserved in a URI (RFC 3986 §2.3: `ALPHA / DIGIT /
/// "-" / "." / "_" / "~"`), which is what makes the value its own percent-encoding.
fn is_uri_unreserved(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~'))
}

/// Whether `value` satisfies the modelled `BlockToken`/`PageToken` constraints: pattern
/// `^[A-Za-z0-9+/=]+$` with `@length` 0..256
/// (`smithy_query(query="shape",shape="com.amazonaws.ebs#BlockToken,com.amazonaws.ebs#PageToken")`).
fn is_modelled_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
}

/// One snapshot of the table: the case that names it, its volume size in GiB, and the
/// blocks written into it as `(BlockIndex, seed)`.
struct Fixture {
    case: &'static str,
    volume_size: i64,
    blocks: &'static [(i32, u8)],
    /// Whether the snapshot under test is a CHILD of the written one, which inherits the
    /// parent's blocks and mints a fresh token for each of them.
    inherited: bool,
}

/// The rows the rule is asserted over: they differ in block count, in the indexes used
/// (including the last index a 1 GiB volume addresses, 2047 = 1 GiB / 524288 - 1), in
/// volume size, and in whether the token was minted by a write or by inheritance.
const FIXTURES: &[Fixture] = &[
    Fixture {
        case: "one block at index 0",
        volume_size: 1,
        blocks: &[(0, 11)],
        inherited: false,
    },
    Fixture {
        case: "two blocks at non-adjacent indexes",
        volume_size: 1,
        blocks: &[(0, 12), (5, 13)],
        inherited: false,
    },
    Fixture {
        case: "a block at the last index the volume addresses",
        volume_size: 1,
        blocks: &[(2047, 14)],
        inherited: false,
    },
    Fixture {
        case: "four blocks over a larger volume",
        volume_size: 8,
        blocks: &[(1, 15), (2, 16), (3, 17), (4096, 18)],
        inherited: false,
    },
    Fixture {
        case: "blocks inherited from a ParentSnapshotId",
        volume_size: 2,
        blocks: &[(0, 19), (7, 20)],
        inherited: true,
    },
];

/// Store `fixture`'s blocks and return the id of the snapshot whose tokens are under
/// test: the written snapshot itself, or the child that inherited its blocks.
///
/// A child is sealed before it is read: the read path sees only a snapshot whose content
/// is final, and the child wrote nothing of its own, so its `ChangedBlocksCount` is zero.
async fn stored_snapshot(router: &MultiServiceRouter, fixture: &Fixture) -> String {
    let parent = write_snapshot(router, fixture.case, fixture.volume_size, fixture.blocks).await;
    if !fixture.inherited {
        return parent;
    }
    let started = start_snapshot(
        router,
        &serde_json::json!({ "VolumeSize": fixture.volume_size, "ParentSnapshotId": parent }),
    )
    .await;
    assert_eq!(
        started.status, 201,
        "case {:?}: StartSnapshot from a parent must answer 201, got {} with body {}",
        fixture.case,
        started.status,
        String::from_utf8_lossy(&started.body)
    );
    let child = started
        .json()
        .get("SnapshotId")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    let sealed = complete_snapshot(router, &child, 0).await;
    assert_eq!(
        sealed.status, 202,
        "case {:?}: CompleteSnapshot on the child must answer 202, got {} with body {}",
        fixture.case,
        sealed.status,
        String::from_utf8_lossy(&sealed.body)
    );
    child
}

/// `GetSnapshotBlock` with `query` used as the request's query string EXACTLY as given, so
/// a caller that escapes nothing can be modelled beside one that escapes everything.
async fn get_block_with_query(
    router: &MultiServiceRouter,
    snapshot_id: &str,
    block_index: i32,
    query: &str,
) -> Reply {
    send(
        router,
        "GET",
        &format!("/snapshots/{snapshot_id}/blocks/{block_index}?{query}"),
        &[],
        Vec::new(),
    )
    .await
}

/// RULE 1: every token this service mints for a URI query is spelled with unreserved
/// characters only, so the value needs no escaping to travel in one.
#[tokio::test]
async fn every_minted_query_token_is_spelled_with_unreserved_characters() {
    let router = router();

    for fixture in FIXTURES {
        let snapshot_id = stored_snapshot(&router, fixture).await;
        let listed = list_blocks(&router, &snapshot_id, "").await;
        assert_eq!(
            listed.status, 200,
            "case {:?}: ListSnapshotBlocks must answer 200, got {} with body {}",
            fixture.case,
            listed.status,
            String::from_utf8_lossy(&listed.body)
        );
        let blocks = listed_blocks(&listed);
        let reported: Vec<i32> = blocks.iter().map(|(index, _)| *index).collect();
        let expected: Vec<i32> = fixture.blocks.iter().map(|(index, _)| *index).collect();
        assert_eq!(
            reported, expected,
            "case {:?}: the listing must report the stored indexes, got {reported:?}",
            fixture.case
        );
        for (block_index, token) in &blocks {
            assert!(
                is_uri_unreserved(token),
                "case {:?}: the BlockToken minted for block {block_index} must be spelled \
                 with URI-unreserved characters only, got {token:?}",
                fixture.case
            );
            assert!(
                is_modelled_token(token),
                "case {:?}: the BlockToken minted for block {block_index} must satisfy the \
                 modelled ^[A-Za-z0-9+/=]+$ and length 0..256, got {token:?}",
                fixture.case
            );
        }
    }
}

/// RULE 2: a minted `BlockToken` put into the query VERBATIM — escaped nowhere, by a
/// caller that assumes the token needs no escaping — reads back exactly the block it was
/// minted for, with the length and checksum derived from the persisted bytes.
#[tokio::test]
async fn a_minted_block_token_reads_its_block_back_unescaped() {
    let router = router();

    for fixture in FIXTURES {
        let snapshot_id = stored_snapshot(&router, fixture).await;
        let listed = list_blocks(&router, &snapshot_id, "").await;
        let blocks = listed_blocks(&listed);
        assert_eq!(
            blocks.len(),
            fixture.blocks.len(),
            "case {:?}: the listing must report every stored block, got {blocks:?}",
            fixture.case
        );
        for ((block_index, token), (_, seed)) in blocks.iter().zip(fixture.blocks) {
            let expected = block_bytes(*seed);
            let read = get_block_with_query(
                &router,
                &snapshot_id,
                *block_index,
                &format!("blockToken={token}"),
            )
            .await;
            assert_eq!(
                read.status, 200,
                "case {:?}: GetSnapshotBlock with the minted token for block {block_index} \
                 spelled verbatim must answer 200, got {} with body {}",
                fixture.case,
                read.status,
                String::from_utf8_lossy(&read.body)
            );
            assert!(
                read.body == expected,
                "case {:?}: block {block_index} must read back byte-identical ({} bytes), \
                 got {} bytes",
                fixture.case,
                expected.len(),
                read.body.len()
            );
            assert_eq!(
                read.header("x-amz-Data-Length"),
                BLOCK_SIZE.to_string(),
                "case {:?}: block {block_index} must report the persisted byte count",
                fixture.case
            );
            assert_eq!(
                read.header("x-amz-Checksum"),
                checksum(&expected),
                "case {:?}: block {block_index} must report the base64 SHA-256 of the \
                 persisted bytes",
                fixture.case
            );
            assert_eq!(
                read.header("x-amz-Checksum-Algorithm"),
                "SHA256",
                "case {:?}: block {block_index} must report the only modelled algorithm",
                fixture.case
            );
            assert_eq!(
                read.header("content-type"),
                "application/octet-stream",
                "case {:?}: block {block_index} must be served as octets",
                fixture.case
            );
        }
    }
}

/// RULE 2, for the other minted query token: a `NextToken` put into the query VERBATIM
/// resumes the listing at the first block the previous page did not report.
#[tokio::test]
async fn a_minted_page_token_resumes_a_listing_unescaped() {
    // `MaxResults` is modelled `range 100..10000`, so 100 is the smallest page a client
    // may ask for and 101 blocks is the smallest input that has to paginate.
    let (page_size, total) = (100_i32, 101_i32);
    let router = router();
    let blocks: Vec<(i32, u8)> = (0..total)
        .map(|index| (index, u8::try_from(index % 251).expect("below 251")))
        .collect();
    let snapshot_id = write_snapshot(&router, "a paginated listing", 128, &blocks).await;

    let first = list_blocks(&router, &snapshot_id, &format!("maxResults={page_size}")).await;
    let next_token = first
        .json()
        .get("NextToken")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        is_uri_unreserved(&next_token) && is_modelled_token(&next_token),
        "case \"a paginated listing\": the minted NextToken must be spelled with \
         URI-unreserved characters only and satisfy the modelled pattern, got {next_token:?}"
    );

    let second = list_blocks(
        &router,
        &snapshot_id,
        &format!("maxResults={page_size}&pageToken={next_token}"),
    )
    .await;
    assert_eq!(
        second.status, 200,
        "case \"a paginated listing\": following the minted NextToken spelled verbatim must \
         answer 200, got {} with body {}",
        second.status,
        String::from_utf8_lossy(&second.body)
    );
    let resumed: Vec<i32> = listed_blocks(&second).iter().map(|(i, _)| *i).collect();
    assert_eq!(
        resumed,
        vec![total - 1],
        "case \"a paginated listing\": the resumed page must report the one unreported \
         block, got {resumed:?}"
    );
}
