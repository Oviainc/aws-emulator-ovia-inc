// SPDX-License-Identifier: CC0-1.0

//! The one listing harness, driven through every listing operation it serves.
//!
//! `ListOutpostsWithS3`, `ListEndpoints` and `ListSharedEndpoints` are paged by the same
//! harness in [`crate::endpoint_lifecycle`]: one request builder, one fixture, one way to
//! read the stored set out of the store, and one table of tokens a listing never issued.
//! This suite drives that single path through each of the three operations and asserts the
//! rule it implements — a listing serves exactly the records the store holds for it, in the
//! store map's own key order, and refuses a cursor it never minted with the one 400
//! `ValidationException` naming itself.
//!
//! The rows are the fixture's listings: the Outpost inventory, this account's own
//! endpoints, an Outpost it owns and an Outpost another account shared with it — so each
//! operation, over sets of different sizes and both cursor forms. The oracle is the
//! persisted records, read through the harness's own store readers.

use service_com_amazonaws_s3outposts_s3outposts::state::PAGE_LIMIT;

use crate::endpoint_lifecycle::{listing_fixture, reported_arns, Listing};

/// One page large enough for the whole fixture: the ceiling `MaxResults` publishes.
fn whole_set() -> Option<i64> {
    Some(i64::from(PAGE_LIMIT))
}

/// Every listing of this service is driven by the one harness: each operation serves the
/// set the harness reads out of the store, in the store's own key order, and refuses every
/// row of the one never-issued-token table with the same worded refusal.
#[tokio::test]
async fn the_one_listing_harness_drives_every_listing_operation() {
    let (router, engine, listings) = listing_fixture().await;

    let driven: Vec<&str> = listings.iter().map(Listing::operation).collect();
    for operation in ["ListOutpostsWithS3", "ListEndpoints", "ListSharedEndpoints"] {
        assert!(
            driven.contains(&operation),
            "the fixture must drive {operation}, got {driven:?}"
        );
    }

    for listing in &listings {
        let case = listing.operation();
        let stored = listing.stored_keys(&engine);
        assert!(
            stored.len() >= 2,
            "case {case:?}: the fixture must store more than one record for {listing:?}, \
             got {stored:?}"
        );

        let answer = listing.page(&router, whole_set(), None).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: one page of the whole set must answer 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            listing.reported_keys(&answer),
            stored,
            "case {case:?}: the listing must serve the stored set in the store's own order, \
             got {:?}",
            answer.body
        );
        assert!(
            answer.next_token().is_none(),
            "case {case:?}: a page holding the whole set carries no token, got {:?}",
            answer.next_token()
        );

        // The harness's ARN reader answers for the endpoint listings only, and agrees
        // record for record with the key reader the inventory shares.
        if listing.reports_endpoints() {
            let arns = listing.stored_arns(&engine);
            assert_eq!(
                arns.len(),
                stored.len(),
                "case {case:?}: one ARN per stored endpoint, got {arns:?} for {stored:?}"
            );
            assert_eq!(
                reported_arns(&answer),
                arns,
                "case {case:?}: the listing must report the stored ARNs in the stored order, \
                 got {:?}",
                answer.body
            );
        }

        // The one table per cursor form, at the size the union of the two former copies
        // has: eight rows for the `<OutpostId>/<EndpointId>` form, six for the
        // inventory's bare `OutpostId`.
        let refusals = listing.never_issued();
        let rows = if listing.reports_endpoints() { 8 } else { 6 };
        assert_eq!(
            refusals.len(),
            rows,
            "case {case:?}: the table keeps every row of this cursor form, got {refusals:?}"
        );
        for (row, token) in refusals {
            let refused = listing.page(&router, Some(1), Some(token)).await;
            assert_eq!(
                (refused.status, refused.error_code()),
                (400, "ValidationException"),
                "case {case:?} row {row:?}: a token this listing never issued must be \
                 refused, got {} with {:?}",
                refused.status,
                refused.body
            );
            assert_eq!(
                refused.message(),
                format!("NextToken {token:?} is not a token returned by {case}"),
                "case {case:?} row {row:?}: the refusal names the listing that never issued \
                 the token, got {:?}",
                refused.message()
            );
        }
    }
}

/// The two endpoint listings refuse the SAME set of tokens: their cursor is the one
/// `<OutpostId>/<EndpointId>` form, so a token one of them never issued is a token the
/// other never issued either — which is what the single table they now share says, and
/// what the two drifted copies did not.
///
/// The set is measured through the router, one call per row and listing, so this is the
/// service's own answer and not a property of the table.
#[tokio::test]
async fn both_endpoint_listings_refuse_the_same_token_set() {
    let (router, _engine, listings) = listing_fixture().await;

    let table: Vec<&str> = Listing::OwnEndpoints
        .never_issued()
        .into_iter()
        .map(|(_, token)| token)
        .collect();

    for listing in listings.iter().filter(|listing| listing.reports_endpoints()) {
        let case = listing.operation();
        let mut refused: Vec<&str> = Vec::new();
        for token in &table {
            let answer = listing.page(&router, Some(1), Some(token)).await;
            if (answer.status, answer.error_code()) == (400, "ValidationException") {
                refused.push(token);
            }
        }
        assert_eq!(
            &refused, &table,
            "case {case:?}: every endpoint listing refuses the one set of tokens no \
             endpoint listing issues, got {refused:?}"
        );
    }
}
