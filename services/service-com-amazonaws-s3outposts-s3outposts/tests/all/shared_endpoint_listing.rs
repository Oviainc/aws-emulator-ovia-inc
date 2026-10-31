// SPDX-License-Identifier: CC0-1.0

//! Which Outposts `ListSharedEndpoints` serves, and what it reports for them, end to end
//! over HTTP.
//!
//! The rule under test: an operation whose documented subject is a resource reached through
//! a cross-account share serves only resources ANOTHER account owns, so an Outpost the
//! calling account owns is outside this listing's scope and is refused with the modelled
//! `ResourceNotFoundException` (404), exactly like an Outpost the inventory does not hold;
//! for an Outpost another account shared with the caller, every endpoint stored against it
//! is reported — the caller's own included, since `CreateEndpoint` and `DeleteEndpoint` are
//! its related actions and operate on exactly those — in the store's own order, and no
//! endpoint of any other Outpost is.
//!
//! The oracle is the persisted records themselves: each case compares the listing with the
//! endpoints the store holds for that Outpost, so no filter the handler invents can pass.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::endpoint_lifecycle::{
    assert_not_found, create, create_endpoints, create_in, emulator_holding_owned,
    emulator_with_engine, list_endpoints, list_endpoints_in, list_shared, list_shared_in,
    minimal_create, owned_outposts, reported_arns, shared_outpost, shared_query, string_at, Answer,
    Listing, SHARING_ACCOUNT_ID,
};

/// A THIRD account, neither the caller nor [`SHARING_ACCOUNT_ID`]: a second account that can
/// share an Outpost, so no case passes by comparing against one other account id. A
/// synthetic, non-routable placeholder in the same all-zero family as
/// `engine::emulator_account_id`.
const THIRD_ACCOUNT_ID: &str = "000000000002";

/// Which Outpost of the fixture a case names: one of the two this account owns, or the one
/// another account shared with it (which already carries that account's own endpoint).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Which {
    FirstOwned,
    SecondOwned,
    Shared,
}

/// Who owns the Outpost of a row.
#[derive(Debug, Clone, Copy)]
enum Owner {
    /// The account this emulator serves — an Outpost it owns, which no account shared with
    /// it.
    Caller,
    /// Another account, which shared the Outpost with this caller.
    Another(&'static str),
}

/// One row of the ownership table: an Outpost to provision, who owns it and the Region it
/// is installed in.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    outpost_id: &'static str,
    owner: Owner,
    region: &'static str,
    /// How many endpoints of this account the row stores on the Outpost before it reads.
    endpoints: usize,
}

/// The rows span what the rule generalises over: who owns the Outpost (the caller, the
/// sharing account, a third account), every branch of `OutpostId`'s
/// `^(op-[a-f0-9]{17}|\d{12}|ec2)$` (from `smithy_query(query="shape", shape="OutpostId")`,
/// the `ec2` and 12-digit branches the issue did not cite included), how many endpoints of
/// this account sit on it (none, one, several) and the Region it is installed in.
const ROWS: &[Row] = &[
    Row {
        case: "the op- branch, owned by this account",
        outpost_id: "op-0a1b2c3d4e5f60718",
        owner: Owner::Caller,
        region: "us-east-1",
        endpoints: 2,
    },
    Row {
        case: "the op- branch, owned by this account, with no endpoint on it",
        outpost_id: "op-1b2c3d4e5f6071829",
        owner: Owner::Caller,
        region: "eu-west-1",
        endpoints: 0,
    },
    Row {
        case: "the op- branch, shared by another account",
        outpost_id: "op-2c3d4e5f607182930",
        owner: Owner::Another(SHARING_ACCOUNT_ID),
        region: "us-east-1",
        endpoints: 2,
    },
    Row {
        case: "the op- branch, shared by a third account, with no endpoint on it",
        outpost_id: "op-3d4e5f6071829304a",
        owner: Owner::Another(THIRD_ACCOUNT_ID),
        region: "eu-west-1",
        endpoints: 0,
    },
    Row {
        case: "the ec2 branch, owned by this account",
        outpost_id: "ec2",
        owner: Owner::Caller,
        region: "us-east-1",
        endpoints: 1,
    },
    Row {
        case: "the ec2 branch, shared by another account",
        outpost_id: "ec2",
        owner: Owner::Another(SHARING_ACCOUNT_ID),
        region: "us-west-2",
        endpoints: 3,
    },
    Row {
        case: "the 12-digit branch, owned by this account",
        outpost_id: "123456789012",
        owner: Owner::Caller,
        region: "us-east-1",
        endpoints: 3,
    },
    Row {
        case: "the 12-digit branch, shared by a third account",
        outpost_id: "999999999999",
        owner: Owner::Another(THIRD_ACCOUNT_ID),
        region: "us-west-2",
        endpoints: 1,
    },
];

/// The queries a row is read with, so no answer depends on the page size: none at all (the
/// model's `@default(0)`), one endpoint per page and the published ceiling of 100
/// (`smithy_query(query="operation", operation="ListSharedEndpoints")`).
const PAGE_SIZES: &[&str] = &["", "&maxResults=1", "&maxResults=100"];

/// The `OutpostId` `which` names, discovered through `ListOutpostsWithS3` the way a client
/// discovers it rather than hardcoded.
async fn outpost_id(router: &MultiServiceRouter, which: Which) -> String {
    match which {
        Which::FirstOwned => owned_outposts(router).await[0].clone(),
        Which::SecondOwned => owned_outposts(router).await[1].clone(),
        Which::Shared => shared_outpost(router).await,
    }
}

/// The listing of `outpost`, whose stored set is every endpoint record held against it,
/// in the order the store keeps them — the answer this listing owes for an Outpost it
/// serves, whoever owns each record.
fn stored_listing(outpost: &str) -> Listing {
    Listing::EndpointsOn(outpost.to_string())
}

/// The `SubnetId`s a listing reports, in the order it reported them. Each endpoint these
/// cases create sits in its own subnet, so the subnet identifies the record whichever ARN
/// members its Outpost's id branch admits (`arn_patterns.rs` is those members' subject).
fn reported_subnets(answer: &Answer) -> Vec<String> {
    answer
        .endpoints()
        .iter()
        .map(|endpoint| string_at(endpoint, "SubnetId"))
        .collect()
}

/// The subnet the `index`-th endpoint of a row sits in, in the 17-hex-digit width
/// `SubnetId`'s `^subnet-([0-9a-f]{8}|[0-9a-f]{17})$` admits.
fn row_subnet(row: usize, index: usize) -> String {
    format!("subnet-{row:09x}{index:08x}")
}

/// Store `count` endpoints of this account on `outpost_id`, through the Region it is
/// installed in, and answer the subnets they sit in.
async fn endpoints_of_this_account(
    router: &MultiServiceRouter,
    region: &str,
    outpost_id: &str,
    row: usize,
    count: usize,
) -> Vec<String> {
    let mut subnets = Vec::with_capacity(count);
    for index in 0..count {
        let subnet_id = row_subnet(row, index);
        let answer = create_in(
            router,
            region,
            json!({
                "OutpostId": outpost_id,
                "SubnetId": subnet_id,
                "SecurityGroupId": format!("sg-{row:09x}{index:08x}"),
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "setup: creating an endpoint on {outpost_id} in {region} must answer 200, got {} \
             with {:?}",
            answer.status, answer.body
        );
        subnets.push(subnet_id);
    }
    subnets
}

/// An Outpost another account shared with this caller is served and every endpoint stored
/// against it is reported; an Outpost the calling account owns is not an Outpost this
/// listing serves, and is the modelled 404 — whatever its id's branch, its Region, and
/// however many endpoints of this account sit on it.
#[tokio::test]
async fn only_an_outpost_another_account_shared_is_one_this_listing_serves() {
    for (index, row) in ROWS.iter().enumerate() {
        let owner = match row.owner {
            Owner::Caller => engine::emulator_account_id().to_string(),
            Owner::Another(account) => account.to_string(),
        };
        // Its own emulator per row, holding exactly the row's Outpost, so what the listing
        // answers is about that one record.
        let (router, _engine) =
            emulator_holding_owned(&[(row.outpost_id, owner.as_str(), row.region)]);
        let stored = endpoints_of_this_account(
            &router,
            row.region,
            row.outpost_id,
            index,
            row.endpoints,
        )
        .await;

        for page in PAGE_SIZES {
            let case = format!("{} read with {page:?}", row.case);
            let query = format!("?outpostId={}{page}", row.outpost_id);
            let answer = list_shared_in(&router, row.region, &query).await;

            match row.owner {
                Owner::Caller => assert_not_found(&case, &answer, &[row.outpost_id]),
                Owner::Another(_) => {
                    assert_eq!(
                        answer.status, 200,
                        "case {case:?}: an Outpost another account shared with this caller must \
                         be served, got {} with {:?}",
                        answer.status, answer.body
                    );
                    // One page per `maxResults`, followed to the end: the whole set, in the
                    // store's own order.
                    let mut walked = reported_subnets(&answer);
                    let mut token = answer.next_token();
                    let mut pages = 1usize;
                    while let Some(cursor) = token {
                        let next =
                            list_shared_in(&router, row.region, &format!("{query}&nextToken={cursor}"))
                                .await;
                        assert_eq!(
                            next.status, 200,
                            "case {case:?}: a page of this listing's own cursor must answer 200, \
                             got {} with {:?}",
                            next.status, next.body
                        );
                        walked.extend(reported_subnets(&next));
                        pages += 1;
                        assert!(
                            pages <= row.endpoints + 1,
                            "case {case:?}: paging must terminate, walked {walked:?}"
                        );
                        token = next.next_token();
                    }
                    assert_eq!(
                        walked, stored,
                        "case {case:?}: the listing must report every endpoint stored against \
                         the Outpost, got {walked:?}"
                    );
                }
            }
        }

        // Whichever answer the row got, this account's OWN listing still reports the
        // endpoints it created: a refusal hides nothing else and stores nothing.
        let own = list_endpoints_in(&router, row.region, "?maxResults=100").await;
        assert_eq!(
            reported_subnets(&own),
            stored,
            "case {:?}: this account's own listing must report the endpoints it created, got {:?}",
            row.case,
            own.body
        );
    }
}

/// The listing of a shared Outpost is exactly the endpoints stored against it — the calling
/// account's own included.
///
/// The rows differ along what the rule generalises over: how many endpoints of this account
/// are on the shared Outpost (none, one, several), beside the endpoint this account does not
/// own that the Outpost already carries.
#[tokio::test]
async fn the_listing_of_a_shared_outpost_is_every_endpoint_stored_against_it() {
    let cases = [
        (
            "a shared Outpost carrying only another account's endpoint",
            Which::Shared,
            0usize,
        ),
        ("a shared Outpost with one endpoint of this account", Which::Shared, 1),
        ("a shared Outpost with four endpoints of this account", Which::Shared, 4),
    ];

    for (index, (case, which, count)) in cases.into_iter().enumerate() {
        // A fresh emulator per row, so each row's store holds only what the row set up.
        let (router, engine) = emulator_with_engine();
        let outpost = outpost_id(&router, which).await;
        let created = create_endpoints(&router, &outpost, count, index).await;

        let answer = list_shared(&router, &shared_query(&outpost)).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: ListSharedEndpoints must answer 200, got {} with {:?}",
            answer.status, answer.body
        );

        let expected = stored_listing(&outpost).stored_arns(&engine);
        let reported = reported_arns(&answer);
        assert_eq!(
            reported, expected,
            "case {case:?}: the listing must be the endpoints stored against {outpost}, \
             got {reported:?}"
        );
        for arn in &created {
            assert!(
                reported.contains(arn),
                "case {case:?}: the endpoint this account created ({arn}) must be listed, \
                 got {reported:?}"
            );
        }
        for endpoint in answer.endpoints() {
            assert_eq!(
                endpoint["OutpostsId"],
                json!(outpost),
                "case {case:?}: every endpoint listed is on the Outpost named, got {:?}",
                endpoint["OutpostsId"]
            );
        }
        assert!(
            answer.next_token().is_none(),
            "case {case:?}: a listing that fits in one page carries no NextToken, got {:?}",
            answer.next_token()
        );
    }
}

/// An endpoint the calling account created on an Outpost it OWNS is reported by this
/// account's own listing and by nothing else: the Outpost-scoped listing does not serve
/// that Outpost at all, so the endpoint is neither hidden from the caller nor reachable
/// through the shared listing.
///
/// The rows differ along which owned Outpost of the fixture carries it and how many
/// endpoints of this account sit on it.
#[tokio::test]
async fn an_endpoint_on_an_owned_outpost_is_reported_by_this_accounts_own_listing_only() {
    for (index, (case, which, count)) in [
        ("one endpoint on the first owned Outpost", Which::FirstOwned, 1usize),
        ("three endpoints on the second owned Outpost", Which::SecondOwned, 3),
    ]
    .into_iter()
    .enumerate()
    {
        let (router, _engine) = emulator_with_engine();
        let outpost = outpost_id(&router, which).await;
        let created = create_endpoints(&router, &outpost, count, index + 0x20).await;

        let refused = list_shared(&router, &shared_query(&outpost)).await;
        assert_not_found(case, &refused, &[outpost.as_str()]);

        let own = reported_arns(&list_endpoints(&router, "?maxResults=100").await);
        for arn in &created {
            assert!(
                own.contains(arn),
                "case {case:?}: this account's own listing must report {arn}, got {own:?}"
            );
        }
    }
}

/// An endpoint the calling account created on a shared Outpost is reported with the very
/// record `ListEndpoints` reports for it: one stored endpoint, two listings, the same
/// members.
///
/// The rows differ along the create request's shape — the members `CreateEndpoint` admits
/// beyond the three required ones, from `smithy_query(query="operation",
/// operation="CreateEndpoint")`: each `EndpointAccessType` value, and a CoIP pool.
#[tokio::test]
async fn an_endpoint_created_on_a_shared_outpost_is_listed_with_its_stored_members() {
    let (router, _engine) = emulator_with_engine();
    let shared = shared_outpost(&router).await;

    let mut requests: Vec<(&str, Value)> = Vec::new();
    for (case, access_type, pool) in [
        ("only the required members", None, None),
        ("AccessType Private", Some("Private"), None),
        (
            "AccessType CustomerOwnedIp with a pool",
            Some("CustomerOwnedIp"),
            Some("ipv4pool-coip-0a1b2c3d4e5f60719"),
        ),
    ] {
        let mut request = minimal_create(&shared);
        request["SubnetId"] = json!(format!("subnet-0000000000000d{:03x}", requests.len()));
        request["SecurityGroupId"] = json!(format!("sg-0000000000000d{:03x}", requests.len()));
        if let Some(access_type) = access_type {
            request["AccessType"] = json!(access_type);
        }
        if let Some(pool) = pool {
            request["CustomerOwnedIpv4Pool"] = json!(pool);
        }
        requests.push((case, request));
    }

    for (case, request) in requests {
        let created = create(&router, request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the create must answer 200, got {} with {:?}",
            created.status, created.body
        );
        let arn = string_at(&created.body, "EndpointArn");

        let shared_listing = list_shared(&router, &shared_query(&shared)).await;
        let from_shared = shared_listing
            .endpoints()
            .iter()
            .find(|endpoint| endpoint["EndpointArn"] == json!(arn))
            .cloned();
        assert!(
            from_shared.is_some(),
            "case {case:?}: the endpoint created on the shared Outpost ({arn}) must be listed, \
             got {:?}",
            reported_arns(&shared_listing)
        );

        let own_listing = list_endpoints(&router, "?maxResults=100").await;
        let from_own = own_listing
            .endpoints()
            .iter()
            .find(|endpoint| endpoint["EndpointArn"] == json!(arn))
            .cloned();
        assert_eq!(
            from_shared, from_own,
            "case {case:?}: both listings report the one stored record for {arn}, got {from_shared:?} \
             and {from_own:?}"
        );
    }
}

/// Paging a shared Outpost's listing walks that same set once, in that same order, whatever
/// page size the caller asks for: the pages concatenated equal the single-page answer, and
/// no page is longer than the size requested.
///
/// The rows differ along the page size, including `MaxResults` left out (the model's
/// `@default(0)`, i.e. no preference) and the published ceiling of 100
/// (`smithy_query(query="operation", operation="ListSharedEndpoints")`).
#[tokio::test]
async fn paging_an_outposts_listing_walks_the_same_set_in_the_same_order() {
    let (router, engine) = emulator_with_engine();
    let shared = shared_outpost(&router).await;
    create_endpoints(&router, &shared, 5, 0xa).await;
    let expected = stored_listing(&shared).stored_arns(&engine);
    assert_eq!(
        expected.len(),
        6,
        "the shared Outpost must carry this account's five endpoints and the other \
         account's one, got {expected:?}"
    );

    for page_size in [None, Some(1usize), Some(2), Some(5), Some(100)] {
        let query = match page_size {
            None => String::new(),
            Some(size) => format!("&maxResults={size}"),
        };
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let resume = token
                .as_deref()
                .map(|value| format!("&nextToken={value}"))
                .unwrap_or_default();
            let answer =
                list_shared(&router, &format!("?outpostId={shared}{query}{resume}")).await;
            assert_eq!(
                answer.status, 200,
                "case {page_size:?}: a page must answer 200, got {} with {:?}",
                answer.status, answer.body
            );
            let page = reported_arns(&answer);
            if let Some(size) = page_size {
                assert!(
                    page.len() <= size,
                    "case {page_size:?}: a page must hold at most {size} endpoints, got {page:?}"
                );
            }
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {page_size:?}: paging must terminate, walked {walked:?}"
            );
            token = answer.next_token();
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, expected,
            "case {page_size:?}: paging must walk every stored endpoint of {shared} once, \
             in order, got {walked:?}"
        );
    }
}
