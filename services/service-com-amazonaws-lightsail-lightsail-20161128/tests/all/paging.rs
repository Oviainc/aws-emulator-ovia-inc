// SPDX-License-Identifier: CC0-1.0

//! The paged-listing walk every listing suite in this test target drives, and
//! the two helpers that go with it.
//!
//! Not a suite of its own: a neutral home for the ONE checked walk over a
//! Lightsail listing's pages, so a suite about a resource asserts what that
//! listing reports rather than carrying a copy of the paging contract. A suite
//! wraps it in a one-line `walk` naming its own operation, output member and
//! page size (see `distribution_cdn.rs` and `load_balancer_listing.rs`).
//!
//! Suites not yet folded in walk their own pages — `disk_listing.rs`, whose local
//! helpers take a page BODY where the ones here take a slice of records,
//! `load_balancer_delete.rs`, which collects names per Region without asserting
//! page sizes, and the catalog and per-resource suites that assert a listing's
//! CONTENTS with a walk of their own. Each means rewriting that suite's own walk
//! and assertions, so folding them in belongs in a refactor of its own rather than
//! inside a fix about another operation.

use serde_json::{json, Value};

use crate::instance_lifecycle::Emulator;

/// A walk over the pages of one listing cannot need more pages than this:
/// anything past it means the walk is not advancing.
const MAX_PAGES: usize = 20;

/// Walk every page of the `member` listing `operation` answers, in `region` when
/// one is given, carrying `page_size` records per page — answering the records in
/// the order reported and the size of each page.
///
/// Checks the paging contract as it goes: no page carries more than `page_size`
/// records, a `nextPageToken` names the FIRST record of the page after it (every
/// Lightsail listing this walks is of records addressed by `name`), a page that
/// is not the last one is full, and the walk terminates because the token is
/// absent rather than because it ran out of patience.
pub(crate) async fn walk_pages(
    emulator: &Emulator,
    case: &str,
    region: Option<&str>,
    operation: &str,
    member: &str,
    page_size: usize,
) -> (Vec<Value>, Vec<usize>) {
    let mut records: Vec<Value> = Vec::new();
    let mut page_sizes = Vec::new();
    let mut token: Option<String> = None;
    for page_index in 0..MAX_PAGES {
        let request = match &token {
            Some(token) => json!({"pageToken": token}),
            None => json!({}),
        };
        let body = emulator.ok_at(None, region, operation, request).await;
        let page = body[member]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a {member} array, got {body:?}"))
            .clone();
        assert!(
            page.len() <= page_size,
            "case {case}: page {page_index} carries more than the {page_size} of a page, got \
             {page:?}"
        );
        if let Some(token) = &token {
            assert_eq!(
                page.first().and_then(|record| record["name"].as_str()),
                Some(token.as_str()),
                "case {case}: a page token names the first record of the page after it, got \
                 {page:?}"
            );
        }
        page_sizes.push(page.len());
        records.extend(page.iter().cloned());
        match body["nextPageToken"].as_str() {
            Some(next) => {
                assert_eq!(
                    page.len(),
                    page_size,
                    "case {case}: a page that is not the last one is full, got {page:?}"
                );
                token = Some(next.to_string());
            }
            None => return (records, page_sizes),
        }
    }
    panic!("case {case}: the walk did not terminate within {MAX_PAGES} pages");
}

/// The names a set of records carries, in the order they were reported.
pub(crate) fn names(records: &[Value]) -> Vec<String> {
    records
        .iter()
        .map(|record| {
            record["name"]
                .as_str()
                .unwrap_or_else(|| panic!("every record carries a name, got {record:?}"))
                .to_string()
        })
        .collect()
}

/// The page sizes a listing of `total` records carried `page_size` at a time must
/// report: full pages until the remainder, and always at least one page — a store
/// that holds none of that resource still answers once.
pub(crate) fn expected_page_sizes(total: usize, page_size: usize) -> Vec<usize> {
    if total == 0 {
        return vec![0];
    }
    let mut sizes = Vec::new();
    let mut left = total;
    while left > 0 {
        let page = left.min(page_size);
        sizes.push(page);
        left -= page;
    }
    sizes
}
