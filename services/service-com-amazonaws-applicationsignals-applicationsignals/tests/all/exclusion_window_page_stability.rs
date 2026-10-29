// SPDX-License-Identifier: CC0-1.0

//! A page sequence over a STORED collection that is written between two pages.
//!
//! The rule under test: a page cursor over a stored collection is a boundary in the stored
//! ORDER of that collection — a key of the record the previous page ended at, not a count of
//! the records already served — so a write that lands between two pages cannot make the next
//! page skip or repeat a record the store still holds: a caller following `NextToken` sees
//! every record that is stored throughout the sequence exactly once. Here the collection is one
//! SLO's exclusion-window set, written by `BatchUpdateExclusionWindows`
//! (PATCH `/exclusion-windows`) and paged by `ListServiceLevelObjectiveExclusionWindows`
//! (GET `/slo/{Id}/exclusion-windows`).
//!
//! Each rule is asserted over a TABLE whose rows differ along what the rule generalises over:
//! how large the stored set is, how big a page the caller asks for, and WHICH records the write
//! between the pages touches — the record the cursor keys on, one before it, one after it,
//! several at once, and none at all. The window configurations and the request helpers are
//! `exclusion_windows`', so every suite speaks to one harness.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::exclusion_windows::{
    batch, create_slo, keys_of, keys_sent, list, query, window, WindowKey, DURATION_UNITS,
    MAX_WINDOWS_PER_SLO,
};
use crate::slo_lifecycle::router;

/// The ONE message a read of this service answers a `NextToken` it did not issue with.
const EXPECTED_REFUSAL: &str = "NextToken is not a token this service issued";

/// One interleaving of a write with a page sequence.
#[derive(Debug)]
struct Row {
    /// What the row is, named in every assertion.
    label: &'static str,
    /// How many windows the SLO holds before the sequence starts.
    size: usize,
    /// The `MaxResults` every page of the sequence asks for.
    page_size: usize,
    /// The positions in the stored order whose windows the write between the first and the
    /// second page REMOVES.
    removed: &'static [usize],
    /// How many brand-new windows that same write ADDS.
    added: usize,
}

/// `count` window configurations distinct within one SLO, each stating every optional member so
/// what these pages compare is the stored set rather than how a default is settled.
///
/// `Window.Duration` targets `ExclusionDuration`, `range 1..`
/// (`smithy_query(shape="Window")`), so every duration here is at least 1.
fn windows_for(slot: usize, count: usize) -> Vec<Value> {
    (0..count)
        .map(|index| {
            window(
                DURATION_UNITS[index % DURATION_UNITS.len()],
                (slot * 100 + index + 1) as i64,
                Some(1_700_000_000 + (slot * 1_000 + index) as i64),
                Some("cron(0 0 ? * SUN *)"),
                Some(&format!("slot {slot} window {index}")),
            )
        })
        .collect()
}

/// Create the SLO called `name` holding exactly `windows`, failing the test if the setup does
/// not apply.
async fn store(router: &MultiServiceRouter, name: &str, windows: &[Value]) {
    create_slo(router, name).await;
    if windows.is_empty() {
        return;
    }
    let (status, updated) = batch(router, &[name.to_string()], windows, &[]).await;
    assert_eq!(
        status, 200,
        "case {name:?}: expected the setup add of {} windows to succeed, got {status} with {updated}",
        windows.len()
    );
}

/// Apply `row`'s write to the SLO called `name`: its removals and its additions in ONE call, as
/// a caller racing a page sequence makes it.
async fn apply_write(
    router: &MultiServiceRouter,
    name: &str,
    row: &Row,
    stored: &[Value],
    additions: &[Value],
) {
    if row.removed.is_empty() && additions.is_empty() {
        return;
    }
    let remove: Vec<Value> = row
        .removed
        .iter()
        .map(|&at| {
            stored
                .get(at)
                .unwrap_or_else(|| panic!("case {:?}: row names position {at}", row.label))
                .clone()
        })
        .collect();
    let (status, updated) = batch(router, &[name.to_string()], additions, &remove).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected the write between the pages to succeed, got {status} with {updated}",
        row.label
    );
}

/// What a page sequence reported: the configurations it served in order, and the status and body
/// of the page that ended it abnormally (`None` once the sequence ran to exhaustion).
struct Sequence {
    served: Vec<WindowKey>,
    refused: Option<(u16, Value)>,
}

/// Walk the paged read of the SLO called `name` at a bound of `page_size`, running `between`
/// once the FIRST page has been served, and report what the sequence served.
async fn walk_with_write<Write, Fut>(
    router: &MultiServiceRouter,
    name: &str,
    label: &str,
    page_size: usize,
    between: Write,
) -> Sequence
where
    Write: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let mut served: Vec<WindowKey> = Vec::new();
    let mut token: Option<String> = None;
    let mut write = Some(between);
    let mut pages = 0;
    loop {
        let (status, page) = list(router, name, &query(Some(page_size), token.as_deref())).await;
        if status != 200 {
            return Sequence {
                served,
                refused: Some((status, page)),
            };
        }
        let reported = keys_of(&page);
        assert!(
            reported.len() <= page_size,
            "case {label:?} page {pages}: expected at most {page_size} windows, got {}",
            reported.len()
        );
        served.extend(reported);
        token = page["NextToken"].as_str().map(ToString::to_string);
        pages += 1;
        if let Some(between) = write.take() {
            between().await;
        }
        if token.is_none() {
            return Sequence {
                served,
                refused: None,
            };
        }
        assert!(
            pages <= MAX_WINDOWS_PER_SLO + 1,
            "case {label:?}: paging did not terminate after {pages} pages"
        );
    }
}

/// The configurations the SLO called `name` holds, read back on one page.
async fn stored_set(router: &MultiServiceRouter, name: &str, label: &str) -> Vec<WindowKey> {
    let (status, page) = list(router, name, &query(Some(MAX_WINDOWS_PER_SLO), None)).await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected the whole set to be served on one page, got {status} with {page}"
    );
    assert!(
        page["NextToken"].is_null(),
        "case {label:?}: expected the whole set on a page of {MAX_WINDOWS_PER_SLO}, got {page}"
    );
    keys_of(&page)
}

/// Whether `keys` holds any configuration twice.
fn has_duplicate(keys: &[WindowKey]) -> bool {
    keys.iter()
        .enumerate()
        .any(|(at, key)| keys[at + 1..].contains(key))
}

/// The interleavings whose write leaves at least one window the sequence has not served yet, so
/// the rule has a surviving record to be judged on.
fn rows() -> Vec<Row> {
    vec![
        // The recorded shape of the defect: the window BEFORE the one the cursor keys on is
        // removed, which is what shifts every later window down an offset.
        Row {
            label: "the window before the one the cursor keys on",
            size: 5,
            page_size: 2,
            removed: &[0],
            added: 0,
        },
        Row {
            label: "the window the cursor itself keys on",
            size: 5,
            page_size: 2,
            removed: &[1],
            added: 0,
        },
        Row {
            label: "every window the first page served",
            size: 5,
            page_size: 2,
            removed: &[0, 1],
            added: 0,
        },
        Row {
            label: "the next window the sequence would serve",
            size: 5,
            page_size: 2,
            removed: &[2],
            added: 0,
        },
        Row {
            label: "the last window of the set",
            size: 5,
            page_size: 2,
            removed: &[4],
            added: 0,
        },
        Row {
            label: "alternating windows at a page bound of one",
            size: 6,
            page_size: 1,
            removed: &[1, 3],
            added: 0,
        },
        Row {
            label: "one window at a page bound of one",
            size: 4,
            page_size: 1,
            removed: &[1],
            added: 0,
        },
        Row {
            label: "the first page of the largest set the model admits",
            size: MAX_WINDOWS_PER_SLO,
            page_size: 3,
            removed: &[0, 1, 2],
            added: 0,
        },
        Row {
            label: "a window at the largest page bound the model admits",
            size: MAX_WINDOWS_PER_SLO,
            page_size: MAX_WINDOWS_PER_SLO - 1,
            removed: &[0],
            added: 0,
        },
        // No write at all: the same sequence over an untouched set, which is what the rule's
        // other rows are compared against.
        Row {
            label: "nothing at all",
            size: 5,
            page_size: 2,
            removed: &[],
            added: 0,
        },
        // A write that ADDS as well: an addition joins the set after every boundary already
        // handed out, so the rest of the sequence serves it.
        Row {
            label: "a removal and an addition in one call",
            size: 5,
            page_size: 2,
            removed: &[0],
            added: 1,
        },
        Row {
            label: "two additions and no removal",
            size: 4,
            page_size: 2,
            removed: &[],
            added: 2,
        },
        Row {
            label: "a removal ahead of the cursor and an addition",
            size: 6,
            page_size: 1,
            removed: &[2, 3],
            added: 1,
        },
    ]
}

/// A write that lands between two pages leaves the sequence serving every window the SLO still
/// holds exactly once, in the stored order, and serving no window twice.
#[tokio::test]
async fn a_write_between_two_pages_still_serves_every_surviving_window_exactly_once() {
    let router = router();
    for (slot, row) in rows().iter().enumerate() {
        let name = format!("page-stability-{slot}");
        let label = row.label;
        let stored = windows_for(slot + 1, row.size);
        store(&router, &name, &stored).await;
        // The additions are configurations no row of this SLO holds, so they are windows of
        // their own rather than a repeat of one already stored.
        let additions = windows_for(slot + 50, row.added);

        let sequence = walk_with_write(&router, &name, label, row.page_size, || {
            apply_write(&router, &name, row, &stored, &additions)
        })
        .await;

        assert!(
            sequence.refused.is_none(),
            "case {label:?}: expected every page of the sequence to be served, got {:?}",
            sequence
                .refused
                .as_ref()
                .map(|(status, body)| format!("{status} with {body}"))
        );

        // What the sequence SHOULD have served: the first page as it stood, then every window
        // the write left in place beyond it, then the windows the write added.
        let mut expected: Vec<Value> = stored.iter().take(row.page_size).cloned().collect();
        expected.extend(
            stored
                .iter()
                .enumerate()
                .filter(|(at, _)| *at >= row.page_size && !row.removed.contains(at))
                .map(|(_, stored)| stored.clone()),
        );
        expected.extend(additions.iter().cloned());
        assert_eq!(
            sequence.served,
            keys_sent(&expected),
            "case {label:?}: expected the sequence to serve every surviving window once, in the stored order"
        );
        assert!(
            !has_duplicate(&sequence.served),
            "case {label:?}: expected no window to be served twice, got {:?}",
            sequence.served
        );

        // Every window the SLO still holds was served by the sequence, and the paging left the
        // set exactly as the write did.
        let remaining = stored_set(&router, &name, label).await;
        for key in &remaining {
            assert!(
                sequence.served.contains(key),
                "case {label:?}: expected the sequence to serve the stored window {key:?}, got {:?}",
                sequence.served
            );
        }
        let mut survivors: Vec<Value> = stored
            .iter()
            .enumerate()
            .filter(|(at, _)| !row.removed.contains(at))
            .map(|(_, stored)| stored.clone())
            .collect();
        survivors.extend(additions.iter().cloned());
        assert_eq!(
            remaining,
            keys_sent(&survivors),
            "case {label:?}: expected the write and the paging to leave exactly the surviving windows"
        );
    }
}

/// A sequence whose write removes the window the cursor keys on AND every window after it has
/// no record left to serve: the boundary names a position the remaining set cannot bound a page
/// at, so the resumed read is REFUSED with this service's one wording for a token it did not
/// issue — rather than reporting a set it cannot place the boundary in as exhausted — and the
/// set itself still reads back whole from the start.
#[tokio::test]
async fn a_boundary_the_remaining_set_cannot_bound_a_page_at_is_refused() {
    let router = router();
    // The rows differ in set size, page bound and how many windows beyond the boundary the
    // write takes away — every one of them, which is what leaves no record to resume on.
    let table: [(&str, usize, usize); 3] = [
        ("every window beyond the first page of two", 5, 2),
        ("every window beyond the first window", 3, 1),
        ("the one window beyond the first page of many", 5, 4),
    ];
    for (slot, (label, size, page_size)) in table.into_iter().enumerate() {
        let name = format!("page-boundary-lost-{slot}");
        let stored = windows_for(slot + 80, size);
        store(&router, &name, &stored).await;

        let removed: Vec<Value> = stored.iter().skip(page_size).cloned().collect();
        let sequence = walk_with_write(&router, &name, label, page_size, || async {
            let (status, updated) =
                batch(&router, std::slice::from_ref(&name), &[], &removed).await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected the write between the pages to succeed, got {status} with {updated}"
            );
        })
        .await;

        assert_eq!(
            sequence.served,
            keys_sent(&stored[..page_size]),
            "case {label:?}: expected the first page to have served the first {page_size} windows"
        );
        let (status, refusal) = sequence
            .refused
            .unwrap_or_else(|| panic!("case {label:?}: expected the resumed read to be refused"));
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400 for a boundary the remaining set cannot bound a page at, got {status} with {refusal}"
        );
        assert_eq!(
            refusal["message"].as_str(),
            Some(EXPECTED_REFUSAL),
            "case {label:?}: expected the message {EXPECTED_REFUSAL:?}, got {refusal}"
        );

        // The refusal reports no page and leaves the surviving windows readable from the start.
        assert!(
            refusal.get("ExclusionWindows").is_none() && refusal.get("NextToken").is_none(),
            "case {label:?}: a refused cursor carries no page and no cursor, got {refusal}"
        );
        assert_eq!(
            stored_set(&router, &name, label).await,
            keys_sent(&stored[..page_size]),
            "case {label:?}: expected the surviving windows to read back from the start"
        );
    }
}
