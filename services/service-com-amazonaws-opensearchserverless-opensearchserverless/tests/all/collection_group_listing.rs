// SPDX-License-Identifier: CC0-1.0

//! `ListCollectionGroups` end to end over the wire.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch` with `POST /`,
//! `X-Amz-Target: OpenSearchServerless.<Operation>` and an `awsJson1_0` body — through the
//! harness `collection_lifecycle` owns, so the boundary validator, the protocol codec and
//! the handler all take part exactly as they do for an SDK client.
//!
//! The rules under test:
//!
//! 1. the listing reports the collection groups the account has stored, each as the STORED
//!    record's own summary — the minted id, its ARN, its name, its creation instant, its
//!    effective capacity limits, its generation, and the number of collections that name
//!    it, counted from the collections themselves — in one stable order, and it carries no
//!    member `CollectionGroupSummary` does not model;
//! 2. the page `maxResults` and `nextToken` select is a window on that one order: a page
//!    holds at most `maxResults` rows (20 when the request names none), a `nextToken`
//!    comes back exactly while rows remain after the page, and resuming with it walks the
//!    whole listing once;
//! 3. the listing follows the store: a group a create added is in it, a group a delete
//!    removed is not, each row's count follows the collections that name the group, and
//!    reading it changes nothing a later caller sees.
//!
//! The tables span what those rules generalise over: every `ServerlessGeneration` member
//! plus the omitted case, both `StandbyReplicas` members, capacity limits whole / partial
//! / absent, the `CollectionGroupName` length at both published bounds, a description and
//! tags present as well as absent, group counts from none to past the default page, how
//! many collections name a group (0, 1, 2), and the page bound from 1 to past the stored
//! count and up to `maxResults`' own published maximum.

use serde_json::{json, Value};

use crate::collection_group_capacity::expected_capacity_limits;
use crate::collection_group_lifecycle::{batch_get, create_group, detail_at};
use crate::collection_lifecycle::{at, call, emulator, is_collection_id, text, ACCOUNT, CLIENT_ERROR};

/// The members a `CollectionGroupDetail` carries that `CollectionGroupSummary` does NOT
/// model (`smithy_query(query="shape",
/// shape="CollectionGroupSummary,CollectionGroupDetail")`).
const DETAIL_ONLY_MEMBERS: &[&str] = &[
    "standbyReplicas",
    "description",
    "tags",
    "currentCapacity",
    "storageConfiguration",
];

/// The members `CollectionGroupSummary` and `CollectionGroupDetail` BOTH model, which a
/// row of the listing must therefore report exactly as a read of the same group does.
const SHARED_MEMBERS: &[&str] = &[
    "id",
    "arn",
    "name",
    "createdDate",
    "capacityLimits",
    "generation",
    "numberOfCollections",
];

/// One page of `ListCollectionGroups`: its summaries and the token that resumes after
/// them.
async fn list_page(
    router: &smithy_server_runtime::MultiServiceRouter,
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> (Vec<Value>, Option<String>) {
    let mut request = json!({});
    if let Some(max_results) = max_results {
        request["maxResults"] = json!(max_results);
    }
    if let Some(next_token) = next_token {
        request["nextToken"] = json!(next_token);
    }
    let case = request.to_string();
    let (status, code, answer) = call(router, "ListCollectionGroups", request).await;
    assert_eq!(
        status, 200,
        "case ListCollectionGroups({case}): expected 200, got {status} {code} with {answer}"
    );
    let summaries = at(&answer, "collectionGroupSummaries")
        .as_array()
        .cloned()
        .unwrap_or_default();
    let token = at(&answer, "nextToken").as_str().map(str::to_string);
    (summaries, token)
}

/// Every summary the listing reports, across pages of the largest accepted bound.
async fn list_all(router: &smithy_server_runtime::MultiServiceRouter) -> Vec<Value> {
    let mut summaries = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let (page, next) = list_page(router, Some(100), token.as_deref()).await;
        summaries.extend(page);
        match next {
            Some(next) => token = Some(next),
            None => return summaries,
        }
    }
}

/// The `id` of every summary, in the order the listing reported them.
fn ids(summaries: &[Value]) -> Vec<String> {
    summaries.iter().map(|summary| text(summary, "id")).collect()
}

/// Create one collection named `name` in the group `group`, asserting it was accepted.
///
/// A collection's encryption setting has to come from somewhere — the request's
/// `encryptionConfig` or an `encryption` policy covering its name — and these cases are
/// about the group it names, so each carries the AWS-owned key.
async fn create_collection_in(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
    group: &str,
) {
    let (status, code, answer) = call(
        router,
        "CreateCollection",
        json!({
            "name": name,
            "collectionGroupName": group,
            "encryptionConfig": { "aWSOwnedKey": true },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case CreateCollection({name} in {group}): expected 200, got {status} {code} with {answer}"
    );
}

/// The create requests the tables below drive: every `ServerlessGeneration` member and the
/// omitted case, both `StandbyReplicas` members, capacity limits whole / partial / absent,
/// the `CollectionGroupName` length at both published bounds, and a description and tags
/// present as well as absent.
///
/// `CollectionGroupName` is `@length(min: 3, max: 32)` with `^[a-z][a-z0-9-]+$`,
/// `ServerlessGeneration` is `CLASSIC | NEXTGEN` and `StandbyReplicas` is
/// `ENABLED | DISABLED` (`smithy_query(query="shape",
/// shape="CollectionGroupName,ServerlessGeneration,StandbyReplicas")`).
fn group_requests() -> Vec<Value> {
    let longest_name = format!("l{}", "o".repeat(31));
    vec![
        json!({
            "name": "listing-whole",
            "standbyReplicas": "ENABLED",
            "generation": "CLASSIC",
            "description": "every member named",
            "capacityLimits": {
                "minIndexingCapacityInOCU": 2.0,
                "maxIndexingCapacityInOCU": 10.0,
                "minSearchCapacityInOCU": 2.0,
                "maxSearchCapacityInOCU": 20.0,
            },
            "tags": [{ "key": "team", "value": "search" }],
        }),
        json!({
            "name": "listing-partial",
            "standbyReplicas": "DISABLED",
            "generation": "NEXTGEN",
            // Only the ceilings: a partial `capacityLimits` is stored as given.
            "capacityLimits": {
                "maxIndexingCapacityInOCU": 4.0,
                "maxSearchCapacityInOCU": 4.0,
            },
            "tags": [{ "key": "owner", "value": "数据团队" }],
        }),
        // Every optional member omitted, so this row pins the default generation and the
        // capacity limits the service defaults in for a group that named none.
        json!({ "name": "listing-bare", "standbyReplicas": "ENABLED" }),
        // The name at the published minimum of three characters.
        json!({ "name": "lis", "standbyReplicas": "DISABLED", "tags": [] }),
        // The name at the published maximum of 32 characters.
        json!({
            "name": longest_name,
            "standbyReplicas": "ENABLED",
            "description": "the longest name the model admits",
        }),
    ]
}

// ---------------------------------------------------------------------------
// Rule 1: every row is the stored record's own summary.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_listing_reports_each_stored_group_as_its_own_summary() {
    let router = emulator();

    // Nothing stored yet: an empty listing, never a refusal and never a `nextToken`.
    let (summaries, token) = list_page(&router, None, None).await;
    assert!(
        summaries.is_empty() && token.is_none(),
        "case ListCollectionGroups(empty store): expected no summaries and no nextToken, got {summaries:?} and {token:?}"
    );

    // The groups of the table, plus the collections that name two of them, so the rows
    // differ in how many collections they count: 0, 1 and 2.
    let requests = group_requests();
    let mut created: Vec<(String, Value)> = Vec::new();
    for request in &requests {
        let detail = create_group(&router, "us-east-1", request.clone()).await;
        created.push((text(&detail, "id"), request.clone()));
    }
    create_collection_in(&router, "one-collection", "listing-whole").await;
    create_collection_in(&router, "two-collection", "listing-partial").await;
    create_collection_in(&router, "three-collection", "listing-partial").await;

    let expected_counts = |name: &str| match name {
        "listing-whole" => 1_i64,
        "listing-partial" => 2,
        _ => 0,
    };

    let summaries = list_all(&router).await;
    let mut expected_ids: Vec<String> = created.iter().map(|(id, _)| id.clone()).collect();
    expected_ids.sort();
    assert_eq!(
        ids(&summaries),
        expected_ids,
        "case ListCollectionGroups(every stored group): expected every created group in id order, got {summaries:?}"
    );

    for (id, request) in &created {
        let name = text(request, "name");
        let case = format!("the summary of {name}");
        let summary = summaries
            .iter()
            .find(|summary| &text(summary, "id") == id)
            .cloned()
            .unwrap_or(Value::Null);

        assert!(
            is_collection_id(&text(&summary, "id")),
            "case {case}: id must match ^[a-z0-9]{{3,40}}$, got {summary}"
        );
        assert_eq!(
            text(&summary, "name"),
            name,
            "case {case}: expected the name the create named, got {summary}"
        );
        assert_eq!(
            text(&summary, "arn"),
            format!("arn:aws:aoss:us-east-1:{ACCOUNT}:collection-group/{id}"),
            "case {case}: the ARN is built from this group's own id and Region, got {summary}"
        );
        assert_eq!(
            at(&summary, "capacityLimits"),
            &expected_capacity_limits(at(request, "capacityLimits")),
            "case {case}: expected the effective capacity limits, with the service default \
             for each ceiling the create omitted, got {summary}"
        );
        let expected_generation = match at(request, "generation") {
            Value::Null => "CLASSIC".to_string(),
            named => named.as_str().unwrap_or_default().to_string(),
        };
        assert_eq!(
            text(&summary, "generation"),
            expected_generation,
            "case {case}: expected generation {expected_generation}, got {summary}"
        );
        assert_eq!(
            at(&summary, "numberOfCollections").as_i64(),
            Some(expected_counts(&name)),
            "case {case}: expected the number of collections that name this group, got {summary}"
        );
        let created_date = at(&summary, "createdDate").as_i64().unwrap_or_default();
        assert!(
            created_date > 1_600_000_000_000,
            "case {case}: createdDate must be epoch milliseconds, got {summary}"
        );

        // A row reports exactly what a read of the same group reports, for every member
        // the two shapes share — and nothing a summary does not model.
        let read = detail_at(&batch_get(&router, "us-east-1", json!({ "ids": [id] })).await, 0);
        for member in SHARED_MEMBERS {
            assert_eq!(
                at(&summary, member),
                at(&read, member),
                "case {case}: {member} must be what the detail reader reports, got {summary} beside {read}"
            );
        }
        for member in DETAIL_ONLY_MEMBERS {
            assert!(
                summary.get(member).is_none(),
                "case {case}: a summary carries no {member}, got {summary}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the page bound is a window on that one order.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_page_bound_walks_the_one_order_once() {
    let router = emulator();

    // One group past the documented default page of 20, so the default itself is a bound
    // the listing cuts at rather than the whole store.
    let stored = 21;
    for index in 0..stored {
        create_group(
            &router,
            "us-east-1",
            json!({ "name": format!("paged-group-{index:02}"), "standbyReplicas": "ENABLED" }),
        )
        .await;
    }
    let expected = ids(&list_all(&router).await);
    assert_eq!(
        expected.len(),
        stored,
        "case the paged store: expected {stored} groups, got {expected:?}"
    );
    let mut sorted = expected.clone();
    sorted.sort();
    assert_eq!(
        expected, sorted,
        "case the paged store: the listing's order is the id order, got {expected:?}"
    );

    // `maxResults` is `@range(min: 1, max: 100)`: 1, a bound inside the store, one under
    // the stored count, exactly it, one past it, and the published maximum.
    for bound in [1_i64, 7, 20, 21, 22, 100] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let (page, next) = list_page(&router, Some(bound), token.as_deref()).await;
            let bound_size = usize::try_from(bound).expect("the bound is positive");
            let wanted = (stored - collected.len()).min(bound_size);
            assert_eq!(
                page.len(),
                wanted,
                "case ListCollectionGroups(maxResults={bound}, after {} rows): expected {wanted} summaries, got {page:?}",
                collected.len()
            );
            collected.extend(ids(&page));
            assert_eq!(
                next.is_some(),
                collected.len() < stored,
                "case ListCollectionGroups(maxResults={bound}, after {} rows): a nextToken comes back exactly while rows remain, got {next:?}",
                collected.len()
            );
            match next {
                Some(next) => token = Some(next),
                None => break,
            }
        }
        assert_eq!(
            collected, expected,
            "case ListCollectionGroups(maxResults={bound}): paging walks the whole listing in order, expected {expected:?}, got {collected:?}"
        );
    }

    // A request that names no bound takes the documented default of 20, and its token
    // resumes after exactly those rows.
    let (page, token) = list_page(&router, None, None).await;
    assert_eq!(
        ids(&page),
        expected[..20],
        "case ListCollectionGroups(no maxResults): expected the default page of 20, got {page:?}"
    );
    let token = token.unwrap_or_else(|| {
        panic!("case ListCollectionGroups(no maxResults): expected a nextToken while a row remains")
    });
    let (rest, after) = list_page(&router, None, Some(&token)).await;
    assert_eq!(
        (ids(&rest), after.clone()),
        (expected[20..].to_vec(), None),
        "case ListCollectionGroups(no maxResults, resumed): expected the last row and no token, got {rest:?} and {after:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: the listing follows the store, and reading it changes nothing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_listing_follows_the_store_and_reading_it_changes_nothing() {
    let router = emulator();

    for name in ["kept-group", "doomed-group", "counted-group"] {
        create_group(
            &router,
            "us-east-1",
            json!({ "name": name, "standbyReplicas": "ENABLED" }),
        )
        .await;
    }
    let names = |summaries: &[Value]| {
        let mut names: Vec<String> = summaries
            .iter()
            .map(|summary| text(summary, "name"))
            .collect();
        names.sort();
        names
    };
    assert_eq!(
        names(&list_all(&router).await),
        vec!["counted-group", "doomed-group", "kept-group"],
        "case the created groups: expected every one of them in the listing"
    );

    // The count of one row follows the collections that name its group, one collection at
    // a time.
    let count_of = |summaries: &[Value], group: &str| {
        summaries
            .iter()
            .find(|summary| text(summary, "name") == group)
            .and_then(|summary| at(summary, "numberOfCollections").as_i64())
    };
    for (index, collection) in ["first-counted", "second-counted"].into_iter().enumerate() {
        create_collection_in(&router, collection, "counted-group").await;
        let expected = i64::try_from(index + 1).expect("the index is small");
        let summaries = list_all(&router).await;
        assert_eq!(
            count_of(&summaries, "counted-group"),
            Some(expected),
            "case numberOfCollections after {collection}: expected {expected}, got {summaries:?}"
        );
        assert_eq!(
            count_of(&summaries, "kept-group"),
            Some(0),
            "case numberOfCollections of another group after {collection}: expected 0, got {summaries:?}"
        );
    }

    // Reading the listing is a read: two identical calls agree, and the groups and their
    // counts are unchanged by having been listed.
    let first = list_all(&router).await;
    let second = list_all(&router).await;
    assert_eq!(
        first, second,
        "case two identical listings: a read changes nothing a later caller sees, got {first:?} then {second:?}"
    );

    // A deleted group leaves the listing; the groups the delete left alone stay.
    let doomed = first
        .iter()
        .find(|summary| text(summary, "name") == "doomed-group")
        .map(|summary| text(summary, "id"))
        .unwrap_or_default();
    let (status, code, answer) =
        call(&router, "DeleteCollectionGroup", json!({ "id": doomed })).await;
    assert_eq!(
        status, 200,
        "case DeleteCollectionGroup({doomed}): expected 200, got {status} {code} with {answer}"
    );
    assert_eq!(
        names(&list_all(&router).await),
        vec!["counted-group", "kept-group"],
        "case after the delete: expected the deleted group to be gone and the rest to stay"
    );
}

// ---------------------------------------------------------------------------
// What the listing refuses, and what it serves.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_listing_refuses_what_it_must() {
    let router = emulator();

    // `maxResults` is `@range(min: 1, max: 100)`, so one under and one past each published
    // bound is refused; a `nextToken` this service never issued is not a pagination token.
    let refusals: Vec<(&str, Value)> = vec![
        ("maxResults one below the 1 minimum", json!({ "maxResults": 0 })),
        ("a negative maxResults", json!({ "maxResults": -1 })),
        (
            "maxResults one past the 100 maximum",
            json!({ "maxResults": 101 }),
        ),
        (
            "a nextToken this service did not issue",
            json!({ "nextToken": "not-a-token" }),
        ),
    ];
    for (case, request) in refusals {
        let (status, code, answer) = call(&router, "ListCollectionGroups", request).await;
        assert_eq!(
            (status, code.as_str()),
            (CLIENT_ERROR, "ValidationException"),
            "case ListCollectionGroups({case}): expected {CLIENT_ERROR} ValidationException, got {status} {code} with {answer}"
        );
    }

    // Each published bound of `maxResults` is served, not refused.
    for bound in [1_i64, 100] {
        let (summaries, token) = list_page(&router, Some(bound), None).await;
        assert!(
            summaries.is_empty() && token.is_none(),
            "case ListCollectionGroups(maxResults={bound}, empty store): expected no summaries, got {summaries:?} and {token:?}"
        );
    }
}
