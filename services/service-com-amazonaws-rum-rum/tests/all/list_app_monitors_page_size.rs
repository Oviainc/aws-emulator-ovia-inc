// SPDX-License-Identifier: CC0-1.0

//! The page size `ListAppMonitors` applies, over the whole class of `MaxResults` values
//! a client may send — including not sending it at all.
//!
//! The rule under test: a paginated listing reports at most as many entries as the
//! request's `MaxResults` asks for, and when the request names none it reports the page
//! size that member's documentation gives as its default — not the member's modelled
//! maximum. Each row of the table differs along the value the rule generalises over (the
//! requested page size, absent included); the store is one entry larger than the
//! modelled maximum so every row's page is a strict prefix of it.

use std::collections::BTreeSet;

use serde_json::json;

use crate::support::{call, create, emulator};

/// The page `ListAppMonitors` returns when the request names no `MaxResults`.
/// From the model: `ListAppMonitorsRequest$MaxResults` documents "The maximum number of
/// results to return in one operation. The default is 50. The maximum that you can
/// specify is 100."
/// Source: `smithy_query(query="doc", shape="ListAppMonitorsRequest", member="MaxResults")`.
const DOCUMENTED_DEFAULT: usize = 50;

/// The largest page the same member admits: `MaxResultsInteger` is `@range(min: 1,
/// max: 100)`.
const MODELLED_MAXIMUM: usize = 100;

/// The page sizes the pages of a listing hold when `store_size` entries are paged at
/// `page_size` each: full pages until the remainder, which is the last page.
fn expected_page_sizes(store_size: usize, page_size: usize) -> Vec<usize> {
    let mut sizes = Vec::new();
    let mut left = store_size;
    loop {
        let take = left.min(page_size);
        sizes.push(take);
        left -= take;
        if left == 0 {
            return sizes;
        }
    }
}

/// The `Name` of every summary of a page, in the order the page reported them.
fn page_names(page: &[serde_json::Value], case: &str) -> Vec<String> {
    page.iter()
        .map(|summary| {
            summary["Name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a summary with no Name: {summary}"))
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn list_app_monitors_pages_at_max_results_and_at_the_documented_default() {
    let (router, _engine) = emulator();

    // One monitor MORE than the modelled maximum, so no row can satisfy its assertion by
    // simply returning the whole store.
    let store_size = MODELLED_MAXIMUM + 1;
    let mut names = BTreeSet::new();
    for index in 0..store_size {
        // 37 and 101 are coprime, so the names are unique and deliberately NOT created
        // in sorted order: the order the listing reports cannot be an accident of
        // insertion order.
        let name = format!("page-size-monitor-{:03}", (index * 37) % store_size);
        create(
            &router,
            "seed",
            json!({"Name": name, "Domain": "example.com"}),
        )
        .await;
        names.insert(name);
    }
    assert_eq!(
        names.len(),
        store_size,
        "case \"seed\": expected {store_size} distinct monitor names, got {}",
        names.len()
    );
    let ordered: Vec<String> = names.into_iter().collect();

    // The table: an absent `MaxResults`, both sides of the documented default, the
    // default itself, the modelled minimum and the modelled maximum.
    for requested in [
        None,
        Some(1_usize),
        Some(7),
        Some(DOCUMENTED_DEFAULT - 1),
        Some(DOCUMENTED_DEFAULT),
        Some(DOCUMENTED_DEFAULT + 1),
        Some(MODELLED_MAXIMUM),
    ] {
        let case = format!("maxResults={requested:?}");
        let query = match requested {
            Some(max) => format!("?maxResults={max}"),
            None => String::new(),
        };
        let reply = call(&router, "POST", &format!("/appmonitors{query}"), None).await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected ListAppMonitors to answer 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let body = reply.json(&case);
        let page = body["AppMonitorSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no AppMonitorSummaries in {body}"))
            .clone();

        let expected = requested.unwrap_or(DOCUMENTED_DEFAULT).min(store_size);
        assert_eq!(
            page.len(),
            expected,
            "case {case:?}: expected a page of {expected} summaries, got {}",
            page.len()
        );
        let reported = page_names(&page, &case);
        assert_eq!(
            reported.as_slice(),
            &ordered[..expected],
            "case {case:?}: expected the first {expected} monitors in name order, got \
             {reported:?}"
        );
        assert!(
            body["NextToken"].as_str().is_some(),
            "case {case:?}: a page that left {} monitor(s) unreported must carry a \
             NextToken, got {:?}",
            store_size - expected,
            body["NextToken"]
        );
    }

    // And the same default applies to EVERY page of a listing paged with no `MaxResults`,
    // not only the first: paging to exhaustion reports each stored monitor exactly once,
    // in full default-sized pages until the remainder.
    let mut sizes = Vec::new();
    let mut reported = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let case = format!("default paging, page {}", sizes.len() + 1);
        let query = match &next_token {
            Some(token) => format!("?nextToken={token}"),
            None => String::new(),
        };
        let reply = call(&router, "POST", &format!("/appmonitors{query}"), None).await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected ListAppMonitors to answer 200, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let body = reply.json(&case);
        let page = body["AppMonitorSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no AppMonitorSummaries in {body}"))
            .clone();
        sizes.push(page.len());
        reported.extend(page_names(&page, &case));
        assert!(
            sizes.len() <= store_size,
            "case {case:?}: paging a store of {store_size} must terminate, page sizes so \
             far {sizes:?}"
        );
        match body["NextToken"].as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => break,
        }
    }
    let expected_sizes = expected_page_sizes(store_size, DOCUMENTED_DEFAULT);
    assert_eq!(
        sizes, expected_sizes,
        "case \"default paging\": expected page sizes {expected_sizes:?}, got {sizes:?}"
    );
    assert_eq!(
        reported, ordered,
        "case \"default paging\": expected the pages to report every stored monitor once \
         in name order, got {reported:?}"
    );
}
