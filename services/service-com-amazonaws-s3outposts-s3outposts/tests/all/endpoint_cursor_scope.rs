// SPDX-License-Identifier: CC0-1.0

//! The scope an endpoint listing's cursor is read in.
//!
//! A `NextToken` is a position in ONE walk, so it is only a token a listing issued when it
//! names a place that walk has an order over. Both endpoint listings walk the endpoints
//! whose parent Outpost is installed in the Region the request was signed for, in
//! `(OutpostId, EndpointId)` order — the `OutpostId` half dominating it — and
//! `ListSharedEndpoints` walks only the endpoints of the one Outpost its required
//! `OutpostId` names. A token whose Outpost half names a place outside the scope the
//! request asks for — an Outpost of ANOTHER Region, none the inventory holds at all, or
//! (for the Outpost-scoped listing) another Outpost of the same Region — is a token that
//! request's walk could never have minted and is refused with the one 400
//! `ValidationException`, rather than served as a page of whatever happens to sort after it
//! (an empty one, for a scope whose untokened listing reports several endpoints) or
//! silently ignored so the first page is re-served.
//!
//! The rows span what the rule generalises over: which of the two endpoint listings is
//! walked, which Region reads (so each Region is in turn the one whose cursors are foreign),
//! which Outpost of a Region reads (so each is in turn the one whose cursors are foreign to
//! its neighbours), where the foreign Outpost sorts against the reading scope's own keys
//! (before, between, after), whether the cursor was really minted by another scope's listing
//! or names an Outpost no endpoint hangs off, and which branch of `OutpostId`
//! (`^(op-[a-f0-9]{17}|\d{12}|ec2)$`, from `smithy_query(query="shape", shape="OutpostId")`)
//! names it. The positive controls are each scope's OWN cursors, which must still walk its
//! whole set, and the Region-wide listing, whose walk spans every Outpost of the Region and
//! so still resumes at a cursor naming any of them.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::endpoint_lifecycle::{
    create_in, emulator_holding_shared, endpoint_id_of, list_endpoints_in, list_shared_in,
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
///
/// Every Outpost is one another account shared with this caller, so BOTH endpoint listings
/// serve it: `ListSharedEndpoints` reports the endpoints of exactly such an Outpost
/// (`shared_endpoint_listing.rs`), and the endpoints on it are this account's own, which is
/// what `ListEndpoints` reports.
async fn multi_region_fixture() -> (MultiServiceRouter, Vec<(&'static str, String)>) {
    let declared: Vec<(String, &str)> = SITES
        .iter()
        .flat_map(|(region, outposts)| {
            outposts
                .iter()
                .map(move |outpost_id| ((*outpost_id).to_string(), *region))
        })
        .collect();
    let router = emulator_holding_shared(&declared);

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

/// The Region the Outpost-scope cases below all read in: one Region, so nothing those
/// cases refuse can be refused for the Region half of the scope.
const ONE_REGION: &str = "us-west-2";

/// The Outposts [`ONE_REGION`] holds and how many endpoints each carries.
///
/// The ids sort apart on purpose: read from the middle Outpost, a foreign cursor sorts
/// both BEFORE and AFTER its own keys — the two ways a cursor of another Outpost falls
/// against the order it is replayed in. The endpoint-less ones span the branches of
/// `OutpostId`'s `^(op-[a-f0-9]{17}|\d{12}|ec2)$`, and no endpoint hangs off them because
/// `EndpointArn`'s own `…:outpost/(op-[a-f0-9]{17}|ec2)/endpoint/…` cannot name one on the
/// account-id branch at all.
const OUTPOSTS_IN_ONE_REGION: &[(&str, usize)] = &[
    ("000000000012", 0),
    ("ec2", 0),
    ("op-00000000000000000", 2),
    ("op-55555555555555555", 2),
    ("op-99999999999999999", 0),
    ("op-fffffffffffffffff", 2),
];

/// The store the Outpost-scope cases read: every Outpost of [`OUTPOSTS_IN_ONE_REGION`] in
/// the one Region, with the endpoints each is declared to carry, beside the
/// `(OutpostId, EndpointArn)` pair of every endpoint created.
///
/// Every Outpost is one another account shared with this caller, so the Outpost-scoped
/// listing serves each of them (`shared_endpoint_listing.rs`), while the endpoints on them
/// are this account's own, which is what the Region-wide listing reports.
async fn one_region_fixture() -> (MultiServiceRouter, Vec<(&'static str, String)>) {
    let declared: Vec<(String, &str)> = OUTPOSTS_IN_ONE_REGION
        .iter()
        .map(|(outpost_id, _)| ((*outpost_id).to_string(), ONE_REGION))
        .collect();
    let router = emulator_holding_shared(&declared);

    let mut created: Vec<(&'static str, String)> = Vec::new();
    let mut seed = 0x101usize;
    for (outpost_id, count) in OUTPOSTS_IN_ONE_REGION {
        for _ in 0..*count {
            let arn = create_endpoint(&router, ONE_REGION, outpost_id, seed).await;
            seed += 1;
            created.push((*outpost_id, arn));
        }
    }
    (router, created)
}

/// The endpoints `ListSharedEndpoints` reports on `outpost_id`, read with a page big
/// enough for the whole set.
async fn endpoints_on(router: &MultiServiceRouter, outpost_id: &str) -> Vec<String> {
    let answer = list_shared_in(
        router,
        ONE_REGION,
        &format!("?outpostId={outpost_id}&maxResults=100"),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {outpost_id}: its own listing must answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    reported_arns(&answer)
}

/// `ListSharedEndpoints` is scoped by the one Outpost its `OutpostId` names, so the only
/// cursors its walk can mint are that Outpost's own: a token whose Outpost half names a
/// DIFFERENT Outpost of the same Region is refused, however it sorts against the keys
/// being walked — not answered as the empty page everything of the requested Outpost
/// sorting before it would make, nor as the first page again.
///
/// The rows differ along which Outpost reads (so each is in turn the one whose neighbours'
/// cursors are foreign), which other Outpost minted the token and where it sorts against
/// the reader's keys, whether the service really minted the token or it names an Outpost
/// no endpoint hangs off, and the branch of `OutpostId` that names that Outpost. The
/// positive control is the reader's OWN cursors, which must still resume its walk.
#[tokio::test]
async fn the_outpost_scoped_listing_refuses_a_cursor_of_another_outpost_of_its_region() {
    let (router, created) = one_region_fixture().await;

    for (outpost_id, count) in OUTPOSTS_IN_ONE_REGION {
        let served = endpoints_on(&router, outpost_id).await;
        assert_eq!(
            served.len(),
            *count,
            "case {outpost_id}: its untokened listing must report the endpoints the fixture \
             created on it, got {served:?}"
        );

        // Every cursor the listing of another Outpost of this Region could have minted,
        // each with the way it falls outside this request's scope.
        let mut foreign: Vec<(String, String)> = created
            .iter()
            .filter(|(home, _)| home != outpost_id)
            .map(|(home, arn)| {
                (
                    format!("a cursor the listing of {home} minted ({arn})"),
                    cursor_of(home, arn),
                )
            })
            .collect();
        // And one naming each Outpost of this Region that carries no endpoint at all: a
        // place no page of any Outpost here could have stopped at.
        foreign.extend(
            OUTPOSTS_IN_ONE_REGION
                .iter()
                .filter(|(other, count)| other != outpost_id && *count == 0)
                .map(|(other, _)| {
                    (
                        format!("an endpoint-less Outpost of this Region ({other})"),
                        format!("{other}/{ENDPOINT_ID}"),
                    )
                }),
        );
        assert!(
            foreign.len() >= 6,
            "case {outpost_id}: the table must span the Region's other Outposts, got \
             {foreign:?}"
        );

        for (case, token) in &foreign {
            assert_refused(
                case,
                "ListSharedEndpoints",
                token,
                &list_shared_in(
                    &router,
                    ONE_REGION,
                    &format!("?outpostId={outpost_id}&maxResults=100&nextToken={token}"),
                )
                .await,
            );

            // The refusal changed nothing: the Outpost still reports exactly its own set.
            assert_eq!(
                endpoints_on(&router, outpost_id).await,
                served,
                "case {case:?} on {outpost_id}: a refused cursor must leave the Outpost's \
                 own endpoints exactly as they were"
            );
        }

        // The positive control: its OWN cursors still resume its walk at the key they
        // name.
        for (index, arn) in served.iter().enumerate() {
            let token = cursor_of(outpost_id, arn);
            let answer = list_shared_in(
                &router,
                ONE_REGION,
                &format!("?outpostId={outpost_id}&maxResults=100&nextToken={token}"),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {outpost_id} resuming at {token:?}: a cursor of its own walk must be \
                 served, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                reported_arns(&answer),
                served[index..].to_vec(),
                "case {outpost_id} resuming at {token:?}: the page must hold its endpoints \
                 at or after the cursor"
            );
        }
    }
}

/// The Region-wide listing is scoped to no single Outpost — its walk spans every Outpost of
/// the Region — so a cursor naming ANY of them is a position in it and resumes the walk
/// there. This is what keeps the rule the scope the REQUEST names rather than a blanket
/// requirement that a cursor's Outpost half be the one id a request mentions.
///
/// The rows are every endpoint of the fixture, so the cursor's Outpost half is in turn each
/// Outpost the Region holds endpoints on, at each position in that Outpost's own keys.
#[tokio::test]
async fn the_region_wide_listing_serves_a_cursor_of_every_outpost_it_walks() {
    let (router, created) = one_region_fixture().await;

    let whole = reported_arns(&list_endpoints_in(&router, ONE_REGION, "?maxResults=100").await);
    assert_eq!(
        whole.len(),
        created.len(),
        "the Region-wide listing must report every endpoint the fixture created, got \
         {whole:?}"
    );

    for (outpost_id, arn) in &created {
        let token = cursor_of(outpost_id, arn);
        let index = whole
            .iter()
            .position(|served| served == arn)
            .unwrap_or_else(|| panic!("{arn} must be one of the reported endpoints {whole:?}"));
        let answer = list_endpoints_in(
            &router,
            ONE_REGION,
            &format!("?maxResults=100&nextToken={token}"),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {token:?}: a cursor of this walk must be served, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            reported_arns(&answer),
            whole[index..].to_vec(),
            "case {token:?}: the page must hold the Region's endpoints at or after the cursor"
        );
    }
}
