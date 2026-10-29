// SPDX-License-Identifier: CC0-1.0

//! The ONE cursor/page rule every listing of this service serves.
//!
//! `ListOutpostsWithS3`, `ListEndpoints` and `ListSharedEndpoints` differ only in the
//! store map they walk, the form of their cursor, which records they report and the
//! shape they project each record into. Everything else is one rule: a `NextToken`
//! this service never issued is refused with a 400 `ValidationException` naming the
//! listing that never issued it; otherwise the walk resumes at the first key at or
//! after the cursor, serves at most `MaxResults` records in the store map's own key
//! order, and carries back the key of the first record it did NOT serve as
//! `NextToken`.
//!
//! All three operations are driven here through one table, so the shared walk is
//! exercised from the Outpost inventory and from each endpoint listing. The oracle is
//! the persisted records: every expectation is read out of the store, so no order,
//! filter or cursor a handler invents can pass.
//!
//! The harness those rows are built from — which listing a row drives, the request it
//! builds, the fixture it drives, the keys it reads out of the store and the cursor
//! tables — lives once in [`crate::endpoint_lifecycle`].

use crate::endpoint_lifecycle::listing_fixture;

/// Every listing walks its whole set exactly once, in the store map's own key order,
/// whatever page size the caller asks for — and the token an unfinished page carries
/// names the first record that page did not serve.
///
/// The rows differ along what the rule generalises over: which listing is walked (the
/// Outpost inventory, the account's own endpoints, and an Outpost another account shared
/// with it — so each of the three operations, over sets of different sizes and two cursor
/// forms), and the page size, including `MaxResults` left out, both bounds
/// of its published `@range(min: 0, max: 100)` and sizes between them.
#[tokio::test]
async fn every_listing_pages_over_one_stable_order() {
    let (router, engine, listings) = listing_fixture().await;

    for listing in &listings {
        let expected = listing.stored_keys(&engine);
        assert!(
            expected.len() >= 2,
            "listing {listing:?}: the fixture must store more than one record for it, \
             got {expected:?}"
        );

        for page_size in [None, Some(0i64), Some(1), Some(2), Some(3), Some(100)] {
            let case = format!("{} with maxResults={page_size:?}", listing.operation());
            let bound = match page_size {
                None | Some(0) => 100usize,
                Some(size) => usize::try_from(size).expect("a positive page size fits in usize"),
            };

            let mut walked: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0usize;
            loop {
                let answer = listing.page(&router, page_size, token.as_deref()).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: a page must answer 200, got {} with {:?}",
                    answer.status, answer.body
                );
                let served = listing.reported_keys(&answer);
                assert!(
                    served.len() <= bound,
                    "case {case:?}: a page holds at most {bound} records, got {served:?}"
                );
                walked.extend(served);
                pages += 1;
                assert!(
                    pages <= expected.len() + 1,
                    "case {case:?}: paging must terminate, walked {walked:?}"
                );

                token = answer.next_token();
                match &token {
                    None => break,
                    Some(token) => {
                        assert!(
                            walked.len() < expected.len(),
                            "case {case:?}: a token is served only while records are left \
                             unserved, got {token:?} after walking {walked:?}"
                        );
                        assert_eq!(
                            token.as_str(),
                            expected[walked.len()].as_str(),
                            "case {case:?}: the token must name the first record not served, \
                             got {token:?}"
                        );
                    }
                }
            }

            assert_eq!(
                walked, expected,
                "case {case:?}: paging walks every stored record once, in order, \
                 got {walked:?}"
            );
        }
    }
}

/// A cursor every listing DOES issue resumes the walk at the record it names, and a
/// deleted cursor record does not strand the rest: the walk resumes at the first key at
/// or after the cursor.
///
/// The rows are each listing and each position in its own set, so the resume point is
/// exercised at the start, in the middle and at the last record.
#[tokio::test]
async fn every_listing_resumes_at_the_key_its_cursor_names() {
    let (router, engine, listings) = listing_fixture().await;

    for listing in &listings {
        let expected = listing.stored_keys(&engine);
        for (index, cursor) in expected.iter().enumerate() {
            let case = format!(
                "{} resuming at {cursor:?} (record {index})",
                listing.operation()
            );
            let answer = listing.page(&router, Some(100), Some(cursor)).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: a cursor this listing issued must be served, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                listing.reported_keys(&answer),
                expected[index..].to_vec(),
                "case {case:?}: the page must hold the records at or after the cursor"
            );
            assert!(
                answer.next_token().is_none(),
                "case {case:?}: a page that reaches the end carries no token, got {:?}",
                answer.body
            );
        }
    }
}

/// A `NextToken` no listing of this service issued is refused by EVERY listing with the
/// same 400 `ValidationException`, worded alike and naming the operation that never
/// issued it — never served as an arbitrary page.
///
/// The rows differ along the way the token fails the listing's own cursor form, the
/// other listing's cursor form included (see
/// [`crate::endpoint_lifecycle::Listing::never_issued`]).
#[tokio::test]
async fn every_listing_refuses_a_token_it_never_issued() {
    let (router, engine, listings) = listing_fixture().await;

    for listing in &listings {
        // A cursor the listing DOES issue is served, so the rows below are refused for
        // their form and not because the fixture has nothing to serve.
        let issued = listing.stored_keys(&engine)[1].clone();
        let accepted = listing.page(&router, Some(1), Some(&issued)).await;
        assert_eq!(
            accepted.status,
            200,
            "{}: the cursor {issued:?} this listing issued must be served, got {} with {:?}",
            listing.operation(),
            accepted.status,
            accepted.body
        );

        for (case, token) in listing.never_issued() {
            let answer = listing.page(&router, Some(1), Some(token)).await;
            assert_eq!(
                (answer.status, answer.error_code()),
                (400, "ValidationException"),
                "case {case:?} on {}: a token this listing never issued must be refused, \
                 got {} with {:?}",
                listing.operation(),
                answer.status,
                answer.body
            );
            assert_eq!(
                answer.message(),
                format!(
                    "NextToken {token:?} is not a token returned by {}",
                    listing.operation()
                ),
                "case {case:?} on {}: every listing words the refusal alike and names \
                 itself, got {:?}",
                listing.operation(),
                answer.message()
            );
        }
    }
}
