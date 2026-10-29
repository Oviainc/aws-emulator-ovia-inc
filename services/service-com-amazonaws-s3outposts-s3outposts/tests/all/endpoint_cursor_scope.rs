// SPDX-License-Identifier: CC0-1.0

//! The scope an endpoint listing's cursor is read in.
//!
//! A `NextToken` is a position in ONE walk, so it is only a token a listing issued when it
//! names a place that walk has an order over. Both endpoint listings walk the endpoints
//! whose parent Outpost is installed in the Region the request was signed for, in
//! `(OutpostId, EndpointId)` order — the `OutpostId` half dominating it — so a token whose
//! Outpost half names an Outpost of ANOTHER Region, or none the inventory holds at all, is
//! a token this Region's listing could never have minted and is refused with the one 400
//! `ValidationException`, rather than served as a page of whatever happens to sort after it
//! (an empty one, for an account whose untokened listing reports several endpoints) or
//! silently ignored so the first page is re-served.
//!
//! The rows span what the rule generalises over: which of the two endpoint listings is
//! walked, which Region reads (so each Region is in turn the one whose cursors are foreign),
//! where the foreign Outpost sorts against the reading Region's own keys (before, between,
//! after), whether the cursor was really minted by another Region's listing or names an
//! Outpost the inventory holds nowhere, and which branch of `OutpostId`
//! (`^(op-[a-f0-9]{17}|\d{12}|ec2)$`, from `smithy_query(query="shape", shape="OutpostId")`)
//! names it. The positive control is each Region's OWN cursors, which must still walk its
//! whole set.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::endpoint_lifecycle::{
    create_in, emulator_holding, endpoint_id_of, list_endpoints_in, list_shared_in,
    reported_arns, string_at, Answer,
};

/// The Regions this store holds Outposts in, and the Outposts installed in each.
///
/// The ids sort apart on purpose: read from `us-east-1`, the foreign Outposts sort BEFORE
/// and AFTER its own; read from `us-west-2`, the foreign one sorts BETWEEN its two. Those
/// are the three ways a cursor from another Region can fall against the order it is
/// replayed in.
const SITES: &[(&str, &[&str])] = &[
    ("us-east-1", &["op-11111111111111111"]),
    ("us-west-2", &["op-00000000000000000", "op-fffffffffffffffff"]),
];

/// Outpost ids the model's `OutpostId` admits that NO Region of this store holds — one per
/// branch of `^(op-[a-f0-9]{17}|\d{12}|ec2)$`, each with the way it fails.
const UNHELD_OUTPOSTS: &[(&str, &str)] = &[
    ("an op- id the inventory holds nowhere", "op-aaaaaaaaaaaaaaaaa"),
    ("the ec2 branch, which no Outpost here uses", "ec2"),
    ("the account-id branch, which no Outpost here uses", "000000000000"),
];

/// An `EndpointId` of the published width (`^[a-zA-Z0-9]{19}$`), so the only thing wrong
/// with the hand-crafted tokens is the place their Outpost half names.
const ENDPOINT_ID: &str = "0000000000000000001";

/// One endpoint on `outpost_id`, created through `region`. `seed` keeps every endpoint's
/// subnet and security group its own, so no create conflicts with another.
async fn create_endpoint(
    router: &MultiServiceRouter,
    region: &str,
    outpost_id: &str,
    seed: usize,
) -> String {
    let answer = create_in(
        router,
        region,
        json!({
            "OutpostId": outpost_id,
            "SubnetId": format!("subnet-{seed:017x}"),
            "SecurityGroupId": format!("sg-{seed:017x}"),
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {outpost_id} in {region}: CreateEndpoint must answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    string_at(&answer.body, "EndpointArn")
}

/// The cursor an endpoint listing mints for the endpoint `arn` names:
/// `<OutpostId>/<EndpointId>`, read out of the ARN's own
/// `…:outpost/<outpost>/endpoint/<endpoint>` tail (`EndpointArn`'s pattern).
fn cursor_of(outpost_id: &str, arn: &str) -> String {
    format!("{outpost_id}/{}", endpoint_id_of(arn))
}

/// The store every case here reads: two endpoints on every Outpost of every Region, each
/// created through the Region its Outpost is installed in, beside the cursor each one is
/// named by.
async fn multi_region_fixture() -> (MultiServiceRouter, Vec<(&'static str, String)>) {
    let declared: Vec<(String, &str)> = SITES
        .iter()
        .flat_map(|(region, outposts)| {
            outposts
                .iter()
                .map(move |outpost_id| ((*outpost_id).to_string(), *region))
        })
        .collect();
    let router = emulator_holding(&declared);

    let mut cursors: Vec<(&'static str, String)> = Vec::new();
    let mut seed = 1usize;
    for (region, outposts) in SITES {
        for outpost_id in *outposts {
            for _ in 0..2 {
                let arn = create_endpoint(&router, region, outpost_id, seed).await;
                seed += 1;
                cursors.push((region, cursor_of(outpost_id, &arn)));
            }
        }
    }
    (router, cursors)
}

/// The refusal both listings answer a token they never issued with.
fn assert_refused(case: &str, operation: &str, token: &str, answer: &Answer) {
    assert_eq!(
        (answer.status, answer.error_code()),
        (400, "ValidationException"),
        "case {case:?} on {operation}: a token this listing never issued must be refused, \
         got {} with {:?}",
        answer.status,
        answer.body
    );
    assert_eq!(
        answer.message(),
        format!("NextToken {token:?} is not a token returned by {operation}"),
        "case {case:?} on {operation}: the refusal names the listing that never issued the \
         token, got {:?}",
        answer.message()
    );
}

/// Every page of the listing `query` serves in `region`, walked one endpoint at a time by
/// following the `NextToken` it carries.
async fn walk_one_at_a_time(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    outpost_id: Option<&str>,
) -> Vec<String> {
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let scope = outpost_id
            .map(|id| format!("outpostId={id}&"))
            .unwrap_or_default();
        let query = match &token {
            None => format!("?{scope}maxResults=1"),
            Some(token) => format!("?{scope}maxResults=1&nextToken={token}"),
        };
        let answer = match outpost_id {
            None => list_endpoints_in(router, region, &query).await,
            Some(_) => list_shared_in(router, region, &query).await,
        };
        assert_eq!(
            answer.status, 200,
            "case {operation} in {region}: a page of its own walk must answer 200, got {} \
             with {:?}",
            answer.status, answer.body
        );
        walked.extend(reported_arns(&answer));
        pages += 1;
        assert!(
            pages <= 16,
            "case {operation} in {region}: paging must terminate, walked {walked:?}"
        );
        token = answer.next_token();
        if token.is_none() {
            return walked;
        }
    }
}

/// A cursor whose Outpost half names a place the reading Region's walk has no order over —
/// an Outpost of another Region, or one the inventory holds nowhere — is refused by BOTH
/// endpoint listings, and the refusal leaves what the Region reports untouched.
#[tokio::test]
async fn both_endpoint_listings_refuse_a_cursor_outside_the_region_they_walk() {
    let (router, cursors) = multi_region_fixture().await;

    for (region, outposts) in SITES {
        // What this Region really reports: the refusals below are the alternative to
        // answering one of these pages, so the set must not be empty.
        let served = reported_arns(&list_endpoints_in(&router, region, "?maxResults=100").await);
        assert!(
            served.len() >= 2,
            "case {region:?}: the fixture must report more than one endpoint here, got \
             {served:?}"
        );

        // Every cursor another Region's listing really minted, plus one per branch of
        // `OutpostId` that no Region of this store holds an Outpost under.
        let mut foreign: Vec<(String, String)> = cursors
            .iter()
            .filter(|(home, _)| home != region)
            .map(|(home, cursor)| {
                (
                    format!("a cursor {home}'s own listing minted ({cursor})"),
                    cursor.clone(),
                )
            })
            .collect();
        foreign.extend(UNHELD_OUTPOSTS.iter().map(|(case, outpost_id)| {
            (
                (*case).to_string(),
                format!("{outpost_id}/{ENDPOINT_ID}"),
            )
        }));
        assert!(
            foreign.len() >= 4,
            "case {region:?}: the table must span both foreign-cursor kinds, got {foreign:?}"
        );

        for (case, token) in &foreign {
            assert_refused(
                case,
                "ListEndpoints",
                token,
                &list_endpoints_in(&router, region, &format!("?maxResults=100&nextToken={token}"))
                    .await,
            );

            for outpost_id in *outposts {
                assert_refused(
                    case,
                    "ListSharedEndpoints",
                    token,
                    &list_shared_in(
                        &router,
                        region,
                        &format!("?outpostId={outpost_id}&maxResults=100&nextToken={token}"),
                    )
                    .await,
                );
            }

            // The refusal changed nothing: the Region still reports exactly its own set.
            assert_eq!(
                reported_arns(&list_endpoints_in(&router, region, "?maxResults=100").await),
                served,
                "case {case:?} in {region}: a refused cursor must leave the Region's own \
                 endpoints exactly as they were"
            );
        }
    }
}

/// The positive control: each Region's own cursors still walk its whole set, one endpoint
/// per page, even though the store holds Outposts of other Regions whose cursors are
/// refused here.
#[tokio::test]
async fn each_region_walks_its_whole_set_with_the_cursors_it_mints() {
    let (router, _cursors) = multi_region_fixture().await;

    for (region, outposts) in SITES {
        let whole = reported_arns(&list_endpoints_in(&router, region, "?maxResults=100").await);
        assert_eq!(
            walk_one_at_a_time(&router, region, "ListEndpoints", None).await,
            whole,
            "case {region:?}: following its own ListEndpoints cursors walks its whole set, \
             which is {whole:?}"
        );

        for outpost_id in *outposts {
            let on_outpost = reported_arns(
                &list_shared_in(
                    &router,
                    region,
                    &format!("?outpostId={outpost_id}&maxResults=100"),
                )
                .await,
            );
            assert!(
                !on_outpost.is_empty(),
                "case {region:?}/{outpost_id}: the fixture must hold endpoints on it"
            );
            assert_eq!(
                walk_one_at_a_time(&router, region, "ListSharedEndpoints", Some(outpost_id))
                    .await,
                on_outpost,
                "case {region:?}/{outpost_id}: following its own ListSharedEndpoints cursors \
                 walks every endpoint on it, which is {on_outpost:?}"
            );
        }
    }
}
