// SPDX-License-Identifier: CC0-1.0

//! What `ListSharedEndpoints` reports, end to end over HTTP.
//!
//! The rule under test: the listing is scoped by the Outpost its `OutpostId` names and by
//! nothing else — `OutpostId` is the only scoping member the model declares, and the
//! operation documents "all endpoints associated with an Outpost", whose related actions
//! `CreateEndpoint` and `DeleteEndpoint` operate on the caller's own endpoints. So every
//! endpoint record stored against the named Outpost is reported, in the store's own order,
//! and no endpoint of any other Outpost is — in particular an endpoint the calling account
//! created on an Outpost another account shared with it is listed, not hidden.
//!
//! The oracle is the persisted records themselves: each case compares the listing with the
//! endpoints the store holds for that Outpost, so no filter the handler invents can pass.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_s3outposts_s3outposts::ComAmazonawsS3outpostsS3outpostsState;
use smithy_server_runtime::MultiServiceRouter;

use crate::endpoint_lifecycle::{
    create, emulator_with_engine, list_endpoints, list_shared, minimal_create, owned_outposts,
    shared_outpost, string_at, Answer, REGION,
};

/// Which Outpost of the fixture a case names: one of the two this account owns, or the one
/// another account shared with it (which already carries that account's own endpoint).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Which {
    FirstOwned,
    SecondOwned,
    Shared,
}

/// The `OutpostId` `which` names, discovered through `ListOutpostsWithS3` the way a client
/// discovers it rather than hardcoded.
async fn outpost_id(router: &MultiServiceRouter, which: Which) -> String {
    match which {
        Which::FirstOwned => owned_outposts(router).await[0].clone(),
        Which::SecondOwned => owned_outposts(router).await[1].clone(),
        Which::Shared => shared_outpost(router).await,
    }
}

/// The `EndpointArn`s of the endpoints the STORE holds against `outpost`, in the order the
/// store keeps them: the answer every listing of that Outpost owes, whoever owns each
/// record.
fn stored_arns(engine: &Arc<engine::InMemoryEngine>, outpost: &str) -> Vec<String> {
    let state = engine.state::<ComAmazonawsS3outpostsS3outpostsState>();
    let store = state.lock();
    store
        .endpoints
        .values()
        .filter(|record| record.outpost_id == outpost)
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

/// Create `count` endpoints of this account on `outpost`, answering their ARNs. Each one
/// sits in its own subnet so nothing is shared between them but the Outpost.
async fn create_endpoints(
    router: &MultiServiceRouter,
    outpost: &str,
    count: usize,
    tag: usize,
) -> Vec<String> {
    let mut arns = Vec::with_capacity(count);
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
        arns.push(string_at(&answer.body, "EndpointArn"));
    }
    arns
}

/// The listing of an Outpost is exactly the endpoints stored against it — the calling
/// account's own included.
///
/// The rows differ along what the rule generalises over: which Outpost is named (one this
/// account owns, and one another account shared with it), how many endpoints of this
/// account are on it (none, one, several), and whether the Outpost also carries an endpoint
/// this account does not own.
#[tokio::test]
async fn the_listing_of_an_outpost_is_every_endpoint_stored_against_it() {
    let cases = [
        ("an owned Outpost with no endpoint at all", Which::FirstOwned, 0usize),
        ("an owned Outpost with one endpoint of this account", Which::FirstOwned, 1),
        ("an owned Outpost with three endpoints of this account", Which::SecondOwned, 3),
        ("a shared Outpost carrying only another account's endpoint", Which::Shared, 0),
        ("a shared Outpost with one endpoint of this account", Which::Shared, 1),
        ("a shared Outpost with four endpoints of this account", Which::Shared, 4),
    ];

    for (index, (case, which, count)) in cases.into_iter().enumerate() {
        // A fresh emulator per row, so each row's store holds only what the row set up.
        let (router, engine) = emulator_with_engine();
        let outpost = outpost_id(&router, which).await;
        let created = create_endpoints(&router, &outpost, count, index).await;

        let answer = list_shared(&router, &format!("?outpostId={outpost}&maxResults=100")).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: ListSharedEndpoints must answer 200, got {} with {:?}",
            answer.status, answer.body
        );

        let expected = stored_arns(&engine, &outpost);
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

        let shared_listing = list_shared(&router, &format!("?outpostId={shared}&maxResults=100"))
            .await;
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

/// Paging an Outpost's listing walks that same set once, in that same order, whatever page
/// size the caller asks for: the pages concatenated equal the single-page answer, and no
/// page is longer than the size requested.
///
/// The rows differ along the page size, including `MaxResults` left out (the model's
/// `@default(0)`, i.e. no preference) and the published ceiling of 100
/// (`smithy_query(query="operation", operation="ListSharedEndpoints")`).
#[tokio::test]
async fn paging_an_outposts_listing_walks_the_same_set_in_the_same_order() {
    let (router, engine) = emulator_with_engine();
    let shared = shared_outpost(&router).await;
    create_endpoints(&router, &shared, 5, 0xa).await;
    let expected = stored_arns(&engine, &shared);
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
