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

use std::sync::Arc;

use serde_json::json;
use service_com_amazonaws_s3outposts_s3outposts::ComAmazonawsS3outpostsS3outpostsState;
use smithy_server_runtime::MultiServiceRouter;

use crate::endpoint_lifecycle::{
    create, emulator_with_engine, list_endpoints, list_shared, owned_outposts, shared_outpost,
    string_at, Answer, REGION,
};

/// Which listing a row drives: this account's own endpoints, or the endpoints on one named
/// Outpost.
#[derive(Debug, Clone)]
enum Listing {
    Own,
    OnOutpost(String),
}

impl Listing {
    /// The operation name the listing's refusals name.
    fn operation(&self) -> &'static str {
        match self {
            Listing::Own => "ListEndpoints",
            Listing::OnOutpost(_) => "ListSharedEndpoints",
        }
    }
}

/// Call `listing` with the page size and cursor a case asks for. `None` for the page size
/// means the request carries no `MaxResults` member at all (the model's `@default(0)`).
async fn page(
    router: &MultiServiceRouter,
    listing: &Listing,
    page_size: Option<i64>,
    token: Option<&str>,
) -> Answer {
    let mut params: Vec<String> = Vec::new();
    if let Listing::OnOutpost(outpost) = listing {
        params.push(format!("outpostId={outpost}"));
    }
    if let Some(size) = page_size {
        params.push(format!("maxResults={size}"));
    }
    if let Some(token) = token {
        params.push(format!("nextToken={token}"));
    }
    let query = if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    };
    match listing {
        Listing::Own => list_endpoints(router, &query).await,
        Listing::OnOutpost(_) => list_shared(router, &query).await,
    }
}

/// The `EndpointArn`s the STORE holds for `listing`, in the store's own key order: the
/// whole set the listing owes, whatever page size walks it.
fn stored_arns(engine: &Arc<engine::InMemoryEngine>, listing: &Listing) -> Vec<String> {
    let state = engine.state::<ComAmazonawsS3outpostsS3outpostsState>();
    let store = state.lock();
    store
        .endpoints
        .values()
        .filter(|record| match listing {
            Listing::Own => record.owner_account == engine::emulator_account_id(),
            Listing::OnOutpost(outpost) => &record.outpost_id == outpost,
        })
        .map(|record| record.endpoint_arn(REGION))
        .collect()
}

/// The `EndpointArn`s an answer reports, in the order it reported them.
fn reported_arns(answer: &Answer) -> Vec<String> {
    answer
        .endpoints()
        .iter()
        .map(|endpoint| string_at(endpoint, "EndpointArn"))
        .collect()
}

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

/// Create `count` endpoints of this account on `outpost`, each in its own subnet, so
/// nothing is shared between them but the Outpost.
async fn create_endpoints(router: &MultiServiceRouter, outpost: &str, count: usize, tag: usize) {
    for index in 0..count {
        let request = json!({
            "OutpostId": outpost,
            "SubnetId": format!("subnet-{:05x}{:012x}", tag, index),
            "SecurityGroupId": format!("sg-{:05x}{:012x}", tag, index),
        });
        let answer = create(router, request).await;
        assert_eq!(
            answer.status, 200,
            "creating endpoint {index} on {outpost} must answer 200, got {} with {:?}",
            answer.status, answer.body
        );
    }
}

/// An emulator whose store holds endpoints of this account on both Outposts it owns and on
/// the Outpost another account shared with it (which already carries that account's own
/// endpoint), beside the three listings this test pages over.
async fn paged_fixture() -> (
    MultiServiceRouter,
    Arc<engine::InMemoryEngine>,
    Vec<Listing>,
) {
    let (router, engine) = emulator_with_engine();
    let owned = owned_outposts(&router).await;
    let shared = shared_outpost(&router).await;
    create_endpoints(&router, &owned[0], 3, 0x1).await;
    create_endpoints(&router, &owned[1], 2, 0x2).await;
    create_endpoints(&router, &shared, 2, 0x3).await;

    let listings = vec![
        Listing::Own,
        Listing::OnOutpost(owned[0].clone()),
        Listing::OnOutpost(shared),
    ];
    (router, engine, listings)
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
    let (router, engine, listings) = paged_fixture().await;

    for listing in &listings {
        let expected = stored_arns(&engine, listing);
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
                let answer = page(&router, listing, page_size, token.as_deref()).await;
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
/// The rows differ along the way the token fails the cursor's own form
/// (`<OutpostId>/<EndpointId>`, whose parts are `OutpostId`'s
/// `^(op-[a-f0-9]{17}|\d{12}|ec2)$` and `EndpointId`'s `^[a-zA-Z0-9]{19}$`, from
/// `smithy_query(query="shape", shape="OutpostId,EndpointId")`): no separator at all, only
/// one of the two parts, a part that is empty, an Outpost id that is not of any published
/// form, and an endpoint id of the wrong width.
#[tokio::test]
async fn both_endpoint_listings_refuse_a_token_they_never_issued() {
    let (router, engine, listings) = paged_fixture().await;

    // The one valid cursor these cases are perturbations of: a real pair of this fixture.
    let valid = cursor_of(&stored_arns(&engine, &Listing::Own)[1]);
    let refused = [
        ("no separator at all", "not-a-cursor".to_string()),
        ("only the Outpost id", "op-0a1b2c3d4e5f60718".to_string()),
        ("only the endpoint id", "0000000000000000001".to_string()),
        ("an empty Outpost id", "/0000000000000000001".to_string()),
        ("an empty endpoint id", "op-0a1b2c3d4e5f60718/".to_string()),
        ("neither part of a published form", "aaaa/bbbb".to_string()),
        (
            "an Outpost id of no published form",
            "op-zzzzzzzzzzzzzzzzz/0000000000000000001".to_string(),
        ),
        (
            "an endpoint id one character short",
            "op-0a1b2c3d4e5f60718/000000000000000000".to_string(),
        ),
    ];

    for listing in &listings {
        // The pair the perturbations are built from IS served, so the rows below are
        // refused for their form and not because the fixture has nothing to serve.
        let accepted = page(&router, listing, Some(1), Some(&valid)).await;
        assert_eq!(
            accepted.status, 200,
            "{}: the cursor {valid:?} this service issued must be served, got {} with {:?}",
            listing.operation(),
            accepted.status,
            accepted.body
        );

        for (case, token) in &refused {
            let answer = page(&router, listing, Some(1), Some(token)).await;
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
    let (router, _engine, listings) = paged_fixture().await;

    for listing in &listings {
        // 100 is the ceiling `MaxResults`'s `@range(min: 0, max: 100)` publishes; 101 is
        // one past it.
        let served = page(&router, listing, Some(100), None).await;
        assert_eq!(
            served.status, 200,
            "{}: MaxResults 100 is the published ceiling and must be served, got {} with {:?}",
            listing.operation(),
            served.status,
            served.body
        );

        let answer = page(&router, listing, Some(101), None).await;
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
