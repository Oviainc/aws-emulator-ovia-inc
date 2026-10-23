// SPDX-License-Identifier: CC0-1.0

//! `DescribeUsers` pagination COMPOSED with the narrowing parameters.
//!
//! The `rbac_users` suite walks the pages of an unfiltered describe; the rule
//! this suite pins is that a page walk reports exactly the users the request's
//! `Engine` and `Filters` keep, at every page size, each user once and in the
//! service's order.
//!
//! Every assertion goes over the wire, through the same router the production
//! emulator mounts, and the users are put in the store by `CreateUser` over
//! that same wire; the harness and the seeded users are the `users` suite's, so
//! this target keeps one of each rather than a second copy that could drift.

use crate::harness::{params, Emulator};
use crate::users::{all_users, describe, emulator_with_users};

/// Walk every page of a describe, asserting each page holds at most
/// `max_records` users, and return the users in the order the pages reported
/// them.
async fn walk_pages(
    emulator: &Emulator,
    case: &str,
    max_records: usize,
    extra: &[(String, String)],
) -> Vec<(String, String)> {
    let mut collected: Vec<(String, String)> = Vec::new();
    let mut marker: Option<String> = None;
    for page_number in 1..=32 {
        let mut pairs = extra.to_vec();
        pairs.push(("MaxRecords".to_string(), max_records.to_string()));
        if let Some(token) = &marker {
            pairs.push(("Marker".to_string(), token.clone()));
        }
        let (reply, page) = describe(emulator, case, &pairs).await;
        assert!(
            page.len() <= max_records,
            "case {case}: page {page_number} must hold at most {max_records} users, got {}",
            page.len()
        );
        collected.extend(page);
        match reply.marker() {
            Some(token) => {
                // A `Marker` carries the caller from one page to the next, so
                // it is a token to be sent back unchanged — what it says is not
                // this suite's business; that the walk it drives covers the
                // kept users exactly once, in order, is.
                assert!(
                    !token.is_empty(),
                    "case {case}: a Marker must be a token, got an empty one"
                );
                marker = Some(token.to_string());
            }
            None => return collected,
        }
    }
    panic!("case {case}: pagination did not terminate");
}

#[tokio::test]
async fn a_page_walk_reports_exactly_the_users_the_request_keeps() {
    let emulator = emulator_with_users().await;
    let stored = all_users(&emulator, "unfiltered").await;

    // The axis: the page size, from one user per page to more than the store
    // holds, crossed with what narrows the walk — nothing, each engine the
    // stored users run, and a filter on one user's id.
    let engines: Vec<String> = {
        let mut engines: Vec<String> = stored.iter().map(|(_, engine)| engine.clone()).collect();
        engines.sort();
        engines.dedup();
        engines
    };
    let one_id = stored
        .first()
        .expect("the store holds at least the built-in user")
        .0
        .clone();

    let mut narrowings: Vec<(String, Vec<(String, String)>)> =
        vec![("no narrowing".to_string(), Vec::new())];
    for engine in &engines {
        narrowings.push((
            format!("engine {engine}"),
            params(&[("Engine", engine.as_str())]),
        ));
    }
    narrowings.push((
        format!("filter UserId {one_id}"),
        params(&[
            ("Filters.member.1.Name", "UserId"),
            ("Filters.member.1.Values.member.1", one_id.as_str()),
        ]),
    ));

    for (narrowing, pairs) in &narrowings {
        // What the narrowing alone reports is the answer the walk must cover,
        // whatever page size it is served in.
        let (_, expected) = describe(&emulator, narrowing, pairs).await;
        for max_records in 1..=stored.len() + 1 {
            let case = format!("{narrowing} at MaxRecords {max_records}");
            let walked = walk_pages(&emulator, &case, max_records, pairs).await;
            assert_eq!(
                walked, expected,
                "case {case}: the walk must report every kept user once, in order"
            );
        }
    }
}
