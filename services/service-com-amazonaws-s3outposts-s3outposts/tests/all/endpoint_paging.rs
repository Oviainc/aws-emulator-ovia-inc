// SPDX-License-Identifier: CC0-1.0

//! The one cursor/page walk BOTH endpoint listings serve.
//!
//! `ListEndpoints` and `ListSharedEndpoints` differ only in which endpoints they report —
//! the caller's own across every Outpost, versus every endpoint on the Outpost the request
//! names. Everything else about the listing is one rule: the endpoints are served in the
//! store's own `(OutpostId, EndpointId)` order, a page holds at most `MaxResults` of them,
//! the `NextToken` an unfinished page carries is the `(OutpostId, EndpointId)` pair of the
//! first endpoint it did NOT serve, and a `NextToken` this service never issued is refused
//! with `ValidationException` instead of being served as an arbitrary page.
//!
//! Both operations are driven here through the same table, so the shared walk is exercised
//! from each listing. The oracle is the persisted records: every expectation is read out of
//! the store, so no filter or order a handler invents can pass.
//!
//! The listing harness itself — which listing a row drives, the request it builds, the
//! fixture it drives and the cursor tables — lives once in [`crate::endpoint_lifecycle`].

use std::sync::Arc;

use smithy_server_runtime::MultiServiceRouter;

use crate::endpoint_lifecycle::{listing_fixture, reported_arns, Listing};

/// The cursor that names the endpoint `arn`: `<OutpostId>/<EndpointId>`, read out of the
/// ARN's own `…:outpost/<outpost>/endpoint/<endpoint>` tail (`EndpointArn`'s pattern).
fn cursor_of(arn: &str) -> String {
    let (outpost, endpoint_id) = arn
        .rsplit_once("/endpoint/")
        .unwrap_or_else(|| panic!("{arn} is not an EndpointArn"));
    let (_, outpost_id) = outpost
        .rsplit_once("outpost/")
        .unwrap_or_else(|| panic!("{arn} is not an EndpointArn"));
    format!("{outpost_id}/{endpoint_id}")
}

/// The listings of the shared fixture that report ENDPOINTS: this account's own endpoints,
/// an Outpost it owns and an Outpost shared with it — so each of the two endpoint
/// listings, over sets of different sizes.
async fn endpoint_listings() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>, Vec<Listing>) {
    let (router, engine, listings) = listing_fixture().await;
    let endpoint_listings: Vec<Listing> = listings
        .into_iter()
        .filter(Listing::reports_endpoints)
        .collect();
    (router, engine, endpoint_listings)
}

/// Following `NextToken` walks the listing's whole set exactly once, in the store's order,
/// whatever page size the caller asks for — and the token an unfinished page carries names
/// the first endpoint that page did not serve.
///
/// The rows differ along both of the things the rule generalises over: which listing is
/// walked (the account's own endpoints, an Outpost it owns, an Outpost shared with it — so
/// each of the two operations, over sets of different sizes), and the page size, including
/// `MaxResults` left out, both bounds of its published `@range(min: 0, max: 100)` and sizes
/// between them.
#[tokio::test]
async fn both_endpoint_listings_page_over_one_stable_order() {
    let (router, engine, listings) = endpoint_listings().await;

    for listing in &listings {
        let expected = listing.stored_arns(&engine);
        assert!(
            !expected.is_empty(),
            "listing {listing:?}: the fixture must store endpoints for it"
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
                let served = reported_arns(&answer);
                assert!(
                    served.len() <= bound,
                    "case {case:?}: a page holds at most {bound} endpoints, got {served:?}"
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
                            "case {case:?}: a token is served only while endpoints are left \
                             unserved, got {token:?} after walking {walked:?}"
                        );
                        assert_eq!(
                            token.as_str(),
                            cursor_of(&expected[walked.len()]),
                            "case {case:?}: the token must name the first endpoint not \
                             served, got {token:?}"
                        );
                    }
                }
            }

            assert_eq!(
                walked, expected,
                "case {case:?}: paging walks every stored endpoint once, in order, \
                 got {walked:?}"
            );
        }
    }
}

/// A `NextToken` this service never issued is refused by BOTH listings with the same 400
/// `ValidationException`, naming the operation that never issued it — never served as an
/// arbitrary page.
///
/// The rows are the one table of tokens an endpoint listing never issues
/// ([`Listing::never_issued`]), which differ along the way the token fails the cursor's own
/// form (`<OutpostId>/<EndpointId>`, whose parts are `OutpostId`'s
/// `^(op-[a-f0-9]{17}|\d{12}|ec2)$` and `EndpointId`'s `^[a-zA-Z0-9]{19}$`, from
/// `smithy_query(query="shape", shape="OutpostId,EndpointId")`): no separator at all, only
/// one of the two parts, a part that is empty, an Outpost id that is not of any published
/// form, and an endpoint id of the wrong width.
#[tokio::test]
async fn both_endpoint_listings_refuse_a_token_they_never_issued() {
    let (router, engine, listings) = endpoint_listings().await;

    // The one valid cursor these cases are perturbations of: a real pair of this fixture.
    let valid = cursor_of(&Listing::OwnEndpoints.stored_arns(&engine)[1]);

    for listing in &listings {
        // The pair the perturbations are built from IS served, so the rows below are
        // refused for their form and not because the fixture has nothing to serve.
        let accepted = listing.page(&router, Some(1), Some(&valid)).await;
        assert_eq!(
            accepted.status, 200,
            "{}: the cursor {valid:?} this service issued must be served, got {} with {:?}",
            listing.operation(),
            accepted.status,
            accepted.body
        );

        for (case, token) in listing.never_issued() {
            let answer = listing.page(&router, Some(1), Some(token)).await;
            assert_eq!(
                (answer.status, answer.error_code()),
                (400, "ValidationException"),
                "case {case:?} on {}: a token this service never issued must be refused, \
                 got {} with {:?}",
                listing.operation(),
                answer.status,
                answer.body
            );
            assert!(
                answer.message().contains(listing.operation()),
                "case {case:?} on {}: the refusal must name the listing that never issued \
                 the token, got {:?}",
                listing.operation(),
                answer.message()
            );
        }
    }
}

/// A page size past the ceiling `MaxResults` publishes is refused at the boundary by both
/// listings, rather than answered with a page.
#[tokio::test]
async fn both_endpoint_listings_refuse_a_page_size_past_the_published_ceiling() {
    let (router, _engine, listings) = endpoint_listings().await;

    for listing in &listings {
        // 100 is the ceiling `MaxResults`'s `@range(min: 0, max: 100)` publishes; 101 is
        // one past it.
        let served = listing.page(&router, Some(100), None).await;
        assert_eq!(
            served.status, 200,
            "{}: MaxResults 100 is the published ceiling and must be served, got {} with {:?}",
            listing.operation(),
            served.status,
            served.body
        );

        let answer = listing.page(&router, Some(101), None).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (400, "ValidationException"),
            "{}: MaxResults 101 is past the published range, got {} with {:?}",
            listing.operation(),
            answer.status,
            answer.body
        );
    }
}
