// SPDX-License-Identifier: CC0-1.0

//! How `ListHostKeys` PAGES the host keys a server holds.
//!
//! RULE: the listing is ordered by `HostKeyId` and resumable: whatever page size the
//! caller asks for, the pages concatenate to the whole stored set in that one order,
//! no page is longer than the bound asked for, no key is repeated, and the last page
//! carries no `NextToken`.
//!
//! The rows are the page sizes — one key per page, two, the exact count, one past it,
//! the modelled maximum (`MaxResults` is `range 1..1000`), and no bound at all. What a
//! listed key REPORTS is [`crate::host_key_listing`]'s subject; this suite reuses its
//! import and paging walk so the two assert one listing rather than two.

use serde_json::json;

use crate::harness::{generated_host_key, member_of, Emulator};
use crate::host_key_listing::{import_host_key, walk_host_keys};

/// RULE (see the module docs): every page size answers the same keys in the same
/// `HostKeyId` order as an unbounded listing.
#[tokio::test]
async fn host_keys_page_in_host_key_id_order() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;

    // Four stored keys, so a page size below, at and above the count are all distinct
    // cases.
    let mut imported: Vec<String> = Vec::new();
    for index in 0..4 {
        let (material, _) = generated_host_key();
        let case = format!("key {index}");
        imported.push(
            import_host_key(
                &emulator,
                &case,
                json!({"ServerId": &server_id, "HostKeyBody": material}),
            )
            .await,
        );
    }

    let unbounded = walk_host_keys(&emulator, &server_id, None).await;
    assert_eq!(
        unbounded.len(),
        imported.len(),
        "case \"unbounded\": expected the {} stored host keys, got {unbounded:?}",
        imported.len()
    );
    let ids: Vec<&str> = unbounded
        .iter()
        .map(|key| member_of(key, "HostKeyId").as_str().unwrap_or_default())
        .collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(
        ids, sorted,
        "case \"unbounded\": expected the listing in HostKeyId order, got {ids:?}"
    );
    for host_key_id in &imported {
        assert!(
            ids.contains(&host_key_id.as_str()),
            "case \"unbounded\": expected {host_key_id} among the listed keys {ids:?}"
        );
    }

    for limit in [1_u32, 2, 3, 4, 5, 1000] {
        let paged = walk_host_keys(&emulator, &server_id, Some(limit)).await;
        let paged_ids: Vec<&str> = paged
            .iter()
            .map(|key| member_of(key, "HostKeyId").as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            paged_ids, ids,
            "case \"MaxResults {limit}\": expected the same keys in the same order as the \
             unbounded listing {ids:?}, got {paged_ids:?}"
        );
    }
}
