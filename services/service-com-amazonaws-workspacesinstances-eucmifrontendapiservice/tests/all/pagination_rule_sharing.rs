// SPDX-License-Identifier: CC0-1.0

//! The cursor-and-page-bound rule both token-minting listings of this service follow,
//! driven over the wire it really speaks (awsJson1_0, `POST /`,
//! `X-Amz-Target: EUCMIFrontendAPIService.<Operation>`, the Region in the SigV4
//! credential scope).
//!
//! The rule, stated once and asserted through BOTH listings:
//!
//! > a paginated list reports the collection it pages in that collection's own order,
//! > at most `MaxResults` members to a page — that member's own modelled maximum when
//! > the request carries none — and mints a `NextToken` exactly when the collection
//! > holds one more member, the token carrying that member's key, which is where the
//! > next page resumes; so walking the tokens reports every member exactly once, and a
//! > `NextToken` resolving to no member of the collection a list pages is refused with
//! > HTTP 400 `ValidationException` naming `NextToken`.
//!
//! The rows differ along what that rule generalises over: the listing (both lists of
//! this service that mint a token — one paging fixed service data, one paging the
//! account's stored records), the page size (one member to a page, sizes in between,
//! either side of the collection's own length, the modelled maximum, and the member
//! left out altogether), and a token's resolvability (one each list minted for itself,
//! and one its sibling list minted, which names nothing the other pages).
//!
//! Nothing here can pass against text written in this file: every page boundary and
//! every token is checked against the collection the service itself reported.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::workspace_instance_lifecycle::{call, create, error_code, minimal_create, ok, router};

/// One listing of this service that mints a `NextToken`: the operation, the member it
/// reports a page in, the member of one reported item the token is minted from, and
/// the modelled ceiling of its own `MaxResults`.
///
/// Both listings are the service's `@paginated` operations that truncate — the crate's
/// `OPERATIONS.md` input lines name each `MaxResults` shape and its range
/// (`ListWorkspaceInstancesMaxResults`, `range 1..25`; `ListInstanceTypesMaxResults`,
/// `range 1..600`).
struct Listing {
    operation: &'static str,
    items: &'static str,
    key: &'static str,
    max_results: i64,
}

const LISTINGS: &[Listing] = &[
    Listing {
        operation: "ListWorkspaceInstances",
        items: "WorkspaceInstances",
        key: "WorkspaceInstanceId",
        max_results: 25,
    },
    Listing {
        operation: "ListInstanceTypes",
        items: "InstanceTypes",
        key: "InstanceType",
        max_results: 600,
    },
];

/// How many instances the stored-record listing pages over: enough that page sizes of
/// 1, 2, 3 and 7 all behave differently and that a resume point has members both
/// before and after it, and well inside that listing's modelled ceiling so an absent
/// `MaxResults` still reports the whole set in one page.
const STORED_INSTANCES: usize = 8;

/// A router whose store holds [`STORED_INSTANCES`] instances, so both listings page a
/// collection with members in it.
async fn fixture() -> MultiServiceRouter {
    let router = router();
    for _ in 0..STORED_INSTANCES {
        create(&router, minimal_create()).await;
    }
    router
}

/// The keys one page of `listing` reports, with that page's `NextToken`.
async fn page(
    router: &MultiServiceRouter,
    listing: &Listing,
    request: &Value,
) -> (Vec<String>, Option<String>) {
    let payload = ok(router, listing.operation, request.clone()).await;
    let reported = payload[listing.items]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {request} on {}: no {} in {payload}",
                listing.operation, listing.items
            )
        })
        .iter()
        .map(|item| {
            item[listing.key]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {request} on {}: no {} in {item}",
                        listing.operation, listing.key
                    )
                })
                .to_string()
        })
        .collect();
    let next = payload
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (reported, next)
}

/// Every key `listing` pages, in the order it pages them: read in ONE page of that
/// listing's modelled maximum, which is the whole collection for both lists here.
async fn whole_collection(router: &MultiServiceRouter, listing: &Listing) -> Vec<String> {
    let request = json!({ "MaxResults": listing.max_results });
    let (reported, next) = page(router, listing, &request).await;
    assert_eq!(
        next, None,
        "fixture {}: the collection must fit inside its modelled maximum page, got a token \
         after {} members",
        listing.operation,
        reported.len()
    );
    assert!(
        reported.len() >= 4,
        "fixture {}: expected a collection long enough to truncate at several page sizes, \
         got {reported:?}",
        listing.operation
    );
    reported
}

/// The page sizes one listing is walked at: the sizes the rule generalises over, with
/// `None` standing for a request that leaves `MaxResults` out (the modelled maximum).
fn page_sizes(listing: &Listing, collection: usize) -> Vec<Option<i64>> {
    let collection = collection as i64;
    let mut sizes: Vec<Option<i64>> = [
        1,
        2,
        3,
        7,
        collection - 1,
        collection,
        collection + 1,
        listing.max_results,
    ]
    .into_iter()
    // Outside the modelled range the boundary validator refuses the request, which is
    // another rule's business: a size past the ceiling is covered by the sibling
    // validation suites.
    .filter(|size| (1..=listing.max_results).contains(size))
    .map(Some)
    .collect();
    sizes.sort_unstable();
    sizes.dedup();
    sizes.push(None);
    sizes
}

/// Assert `payload` is this service's refusal of a token it did not issue: the modelled
/// `ValidationException`, HTTP 400, naming the `NextToken` member.
fn assert_token_refused(case: &str, status: u16, payload: &Value) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {payload}"
    );
    assert_eq!(
        error_code(payload),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {payload}"
    );
    let names_member = payload
        .get("FieldList")
        .and_then(Value::as_array)
        .is_some_and(|fields| {
            fields
                .iter()
                .any(|field| field.get("Name").and_then(Value::as_str) == Some("NextToken"))
        });
    assert!(
        names_member,
        "case {case:?}: the refusal must name the NextToken member, got {payload}"
    );
}

#[tokio::test]
async fn both_listings_bound_the_page_and_mint_the_token_from_the_member_one_past_it() {
    let router = fixture().await;

    for listing in LISTINGS {
        let full = whole_collection(&router, listing).await;
        let bound_without_member = listing.max_results as usize;

        for size in page_sizes(listing, full.len()) {
            let case = match size {
                Some(size) => format!("{} at MaxResults={size}", listing.operation),
                None => format!("{} with no MaxResults", listing.operation),
            };
            let bound = size.map_or(bound_without_member, |size| size as usize);
            let mut request = match size {
                Some(size) => json!({ "MaxResults": size }),
                None => json!({}),
            };

            let mut walked: Vec<String> = Vec::new();
            let mut pages = 0usize;
            loop {
                let (reported, next) = page(&router, listing, &request).await;
                pages += 1;
                assert!(
                    pages <= full.len() + 1,
                    "case {case:?}: paging must terminate, got {pages} pages over a collection \
                     of {}",
                    full.len()
                );

                // A page carries the whole bound while the collection holds that many
                // more members, and what is left over on the page that ends the walk.
                let expected = &full[walked.len()..(walked.len() + bound).min(full.len())];
                assert_eq!(
                    reported, expected,
                    "case {case:?}: expected page {pages} to report {expected:?}, got {reported:?}"
                );
                walked.extend(reported);

                match next {
                    Some(token) => {
                        assert!(
                            walked.len() < full.len(),
                            "case {case:?}: a token was minted after the whole collection of {} \
                             members was reported, got {token:?}",
                            full.len()
                        );
                        // The token carries the key of the member one past the page, in
                        // this service's one token encoding.
                        assert_eq!(
                            token,
                            hex::encode(&full[walked.len()]),
                            "case {case:?}: expected the token to carry {:?}, the member one \
                             past page {pages}, got {token:?}",
                            full[walked.len()]
                        );
                        request["NextToken"] = json!(token);
                    }
                    None => {
                        assert_eq!(
                            walked.len(),
                            full.len(),
                            "case {case:?}: page {pages} minted no token with {} of {} members \
                             still unreported",
                            full.len() - walked.len(),
                            full.len()
                        );
                        break;
                    }
                }
            }

            assert_eq!(
                walked, full,
                "case {case:?}: walking the tokens must report every member exactly once"
            );
        }
    }
}

#[tokio::test]
async fn each_listing_refuses_the_resume_point_the_other_listing_issued() {
    let router = fixture().await;

    // A token each listing minted itself, with the collection it pages: the member the
    // token names is a resume point for that list alone.
    let mut minted: Vec<(&Listing, Vec<String>, String)> = Vec::new();
    for listing in LISTINGS {
        let full = whole_collection(&router, listing).await;
        let (_, next) = page(&router, listing, &json!({ "MaxResults": 1 })).await;
        let token = next.unwrap_or_else(|| {
            panic!(
                "fixture {}: a page of one over {} members must mint a token",
                listing.operation,
                full.len()
            )
        });
        minted.push((listing, full, token));
    }

    for (issuer, full, token) in &minted {
        // The list that issued it resumes exactly at the member it names.
        let (reported, _) = page(
            &router,
            issuer,
            &json!({ "MaxResults": 1, "NextToken": token }),
        )
        .await;
        assert_eq!(
            reported,
            full[1..2].to_vec(),
            "case {:?} resuming at its own token {token:?}: expected the member one past the \
             first page, got {reported:?}",
            issuer.operation
        );

        // Every other list pages a collection that member is not in, so the same token
        // resolves to nothing there and is refused — whatever the rest of the request
        // asks for.
        for foreign in LISTINGS
            .iter()
            .filter(|listing| listing.operation != issuer.operation)
        {
            for request in [
                json!({ "NextToken": token }),
                json!({ "NextToken": token, "MaxResults": 1 }),
            ] {
                let (status, payload) = call(&router, foreign.operation, request.clone()).await;
                assert_token_refused(
                    &format!(
                        "{token:?} issued by {}, presented to {} with {request}",
                        issuer.operation, foreign.operation
                    ),
                    status,
                    &payload,
                );
            }
        }
    }

    // The refusals changed nothing: both listings still page their whole collection.
    for (listing, full, _) in &minted {
        assert_eq!(
            &whole_collection(&router, listing).await,
            full,
            "case {} after the refusals: expected the collection to be reported unchanged",
            listing.operation
        );
    }
}
