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

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_s3outposts_s3outposts::ComAmazonawsS3outpostsS3outpostsState;
use smithy_server_runtime::MultiServiceRouter;

use crate::endpoint_lifecycle::{
    create, emulator_with_engine, endpoint_id_of, list_endpoints, list_outposts, list_shared,
    owned_outposts, shared_outpost, string_at, Answer,
};

/// Which listing a row drives: the Outpost inventory, this account's own endpoints, or
/// the endpoints on one named Outpost.
#[derive(Debug, Clone)]
enum Listing {
    Outposts,
    OwnEndpoints,
    EndpointsOn(String),
}

impl Listing {
    /// The operation name the listing's refusals name.
    fn operation(&self) -> &'static str {
        match self {
            Listing::Outposts => "ListOutpostsWithS3",
            Listing::OwnEndpoints => "ListEndpoints",
            Listing::EndpointsOn(_) => "ListSharedEndpoints",
        }
    }

    /// Call the listing with the page size and cursor a case asks for. `None` for the
    /// page size means the request carries no `MaxResults` member at all (the model's
    /// `@default(0)`).
    async fn page(
        &self,
        router: &MultiServiceRouter,
        page_size: Option<i64>,
        token: Option<&str>,
    ) -> Answer {
        let mut params: Vec<String> = Vec::new();
        if let Listing::EndpointsOn(outpost) = self {
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
        match self {
            Listing::Outposts => list_outposts(router, &query).await,
            Listing::OwnEndpoints => list_endpoints(router, &query).await,
            Listing::EndpointsOn(_) => list_shared(router, &query).await,
        }
    }

    /// The cursor keys an answer reports, in the order it reported them: the key each
    /// record is stored under, read back off the record the answer carries.
    fn reported_keys(&self, answer: &Answer) -> Vec<String> {
        match self {
            Listing::Outposts => answer
                .outposts()
                .iter()
                .map(|outpost| string_at(outpost, "OutpostId"))
                .collect(),
            Listing::OwnEndpoints | Listing::EndpointsOn(_) => answer
                .endpoints()
                .iter()
                .map(endpoint_key)
                .collect(),
        }
    }

    /// The cursor keys the STORE holds for this listing, in the store map's own key
    /// order: the whole set the listing owes, whatever page size walks it.
    fn stored_keys(&self, engine: &Arc<engine::InMemoryEngine>) -> Vec<String> {
        let state = engine.state::<ComAmazonawsS3outpostsS3outpostsState>();
        let store = state.lock();
        match self {
            Listing::Outposts => store.outposts.keys().cloned().collect(),
            Listing::OwnEndpoints => store
                .endpoints
                .iter()
                .filter(|(_, record)| record.owner_account == engine::emulator_account_id())
                .map(|(key, _)| format!("{}/{}", key.0, key.1))
                .collect(),
            Listing::EndpointsOn(outpost) => store
                .endpoints
                .keys()
                .filter(|(outpost_id, _)| outpost_id == outpost)
                .map(|(outpost_id, endpoint_id)| format!("{outpost_id}/{endpoint_id}"))
                .collect(),
        }
    }

    /// Tokens this listing never issues, each with the way it fails the listing's own
    /// cursor form.
    ///
    /// The inventory's cursor is an `OutpostId` (`^(op-[a-f0-9]{17}|\d{12}|ec2)$`) and
    /// an endpoint listing's is `<OutpostId>/<EndpointId>` with `EndpointId`'s
    /// `^[a-zA-Z0-9]{19}$` (from
    /// `smithy_query(query="shape", shape="OutpostId,EndpointId")`), so each listing
    /// also refuses the OTHER's form: no listing serves a cursor it did not mint.
    fn never_issued(&self) -> Vec<(&'static str, &'static str)> {
        match self {
            Listing::Outposts => vec![
                ("no published Outpost id form at all", "not-a-cursor"),
                (
                    "the other listing's endpoint cursor",
                    "op-0a1b2c3d4e5f60718/0000000000000000001",
                ),
                ("an Outpost id with non-hex digits", "op-zzzzzzzzzzzzzzzzz"),
                ("an Outpost id one character short", "op-0a1b2c3d4e5f6071"),
                ("an account id one digit short", "00000000000"),
                ("the ec2 form with a character added", "ec22"),
            ],
            Listing::OwnEndpoints | Listing::EndpointsOn(_) => vec![
                ("no separator at all", "not-a-cursor"),
                ("the other listing's Outpost cursor", "op-0a1b2c3d4e5f60718"),
                ("an empty Outpost id", "/0000000000000000001"),
                ("an empty endpoint id", "op-0a1b2c3d4e5f60718/"),
                (
                    "an Outpost id of no published form",
                    "op-zzzzzzzzzzzzzzzzz/0000000000000000001",
                ),
                (
                    "an endpoint id one character short",
                    "op-0a1b2c3d4e5f60718/000000000000000000",
                ),
            ],
        }
    }
}

/// The cursor key of a reported endpoint: `<OutpostId>/<EndpointId>`, built from the
/// `OutpostsId` member and the id at the tail of `EndpointArn`.
fn endpoint_key(endpoint: &Value) -> String {
    format!(
        "{}/{}",
        string_at(endpoint, "OutpostsId"),
        endpoint_id_of(&string_at(endpoint, "EndpointArn"))
    )
}

/// Create `count` endpoints of this account on `outpost`, each in its own subnet.
async fn create_endpoints(router: &MultiServiceRouter, outpost: &str, count: usize, tag: usize) {
    for index in 0..count {
        let answer = create(
            router,
            json!({
                "OutpostId": outpost,
                "SubnetId": format!("subnet-{:05x}{:012x}", tag, index),
                "SecurityGroupId": format!("sg-{:05x}{:012x}", tag, index),
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "creating endpoint {index} on {outpost} must answer 200, got {} with {:?}",
            answer.status, answer.body
        );
    }
}

/// An emulator whose store holds Outposts and endpoints enough for every listing to
/// page over more than one record, beside the three listings these cases drive.
async fn fixture() -> (
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
        Listing::Outposts,
        Listing::OwnEndpoints,
        Listing::EndpointsOn(owned[0].clone()),
        Listing::EndpointsOn(shared),
    ];
    (router, engine, listings)
}

/// Every listing walks its whole set exactly once, in the store map's own key order,
/// whatever page size the caller asks for — and the token an unfinished page carries
/// names the first record that page did not serve.
///
/// The rows differ along what the rule generalises over: which listing is walked (the
/// Outpost inventory, the account's own endpoints, an Outpost it owns and an Outpost
/// shared with it — so each of the three operations, over sets of different sizes and
/// two cursor forms), and the page size, including `MaxResults` left out, both bounds
/// of its published `@range(min: 0, max: 100)` and sizes between them.
#[tokio::test]
async fn every_listing_pages_over_one_stable_order() {
    let (router, engine, listings) = fixture().await;

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
    let (router, engine, listings) = fixture().await;

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
/// other listing's cursor form included (see [`Listing::never_issued`]).
#[tokio::test]
async fn every_listing_refuses_a_token_it_never_issued() {
    let (router, engine, listings) = fixture().await;

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
