// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListResourceSets`: the paginated reporter of the
//! Firewall Manager resource sets `PutResourceSet` writes.
//!
//! Every test drives the service the way a client does: an awsJson1_1 `POST /`
//! through [`MultiServiceRouter::dispatch`], with the `X-Amz-Target` header and
//! a SigV4 credential scope, so the whole deserialize → handle → serialize walk
//! runs and the assertions are about the WIRE response.
//!
//! The rules under test:
//!
//! 1. `ListResourceSets` reports the `ResourceSetSummary` projection of EVERY
//!    stored resource set and of nothing else, in the stable `Id` order the
//!    store keeps them in. The projection carries exactly the members
//!    `ResourceSetSummary` models (`Id`, `Name`, `Description`,
//!    `LastUpdateTime`, `ResourceSetStatus`) read off the stored resource set,
//!    so it agrees with what `GetResourceSet` reports for the same id and
//!    leaks no member — `UpdateToken`, `ResourceTypeList` — the summary shape
//!    does not declare.
//! 2. `MaxResults` bounds the page, and so does the model's own maximum when the
//!    caller sends no `MaxResults` at all: a `NextToken` appears only while
//!    resource sets remain after the page, and round-trips into the next page.
//! 3. It is a REPORTING operation: listing changes nothing a later caller sees.
//! 4. It is administrator-gated, and a `NextToken` this service did not mint for
//!    a resource-set page is refused rather than served as some other page.
//! 5. The listing follows the store: a resource set `DeleteResourceSet` removed
//!    is on no page afterwards.
//!
//! The table in [`cases`] is the class rules 1-3 range over: the rows differ in
//! name length (both published bounds of `com.amazonaws.fms#Name`, 1..128), in
//! whether a `Description` is present and in its length (both published bounds
//! of `com.amazonaws.fms#Description`, 0..256), in how many resource types the
//! set holds, and in tag count. The page sizes span the published bounds of
//! `com.amazonaws.fms#PaginationMaxResults` (1..100), one value in between, the
//! absent case and one past each bound.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_fms_awsfms_20180101::state;

use crate::administrator_gated_operations::{associate_admin, with_store};
use crate::policy_lifecycle::{call, emulator_with_engine, error_code};

/// One row of the resource-set table.
struct Case {
    /// Names the row in every assertion message.
    name: &'static str,
    /// The `ResourceSet` document the create sends.
    resource_set: Value,
    /// The `TagList` the create sends.
    tags: Value,
}

/// The resource-set class rules 1-3 are asserted over.
///
/// Member bounds are the model's own (`smithy_query(query="shape",
/// shape="ResourceSet")`): `Name` is `com.amazonaws.fms#Name`, 1..128
/// characters; `Description` is `com.amazonaws.fms#Description`, 0..256.
fn cases() -> Vec<Case> {
    vec![
        Case {
            // The minimum a resource set can be: a one-character name, one
            // resource type, no description and no tags.
            name: "shortest name, no description",
            resource_set: json!({
                "Name": "a",
                "ResourceTypeList": ["AWS::NetworkFirewall::Firewall"],
            }),
            tags: json!([]),
        },
        Case {
            name: "described, three resource types",
            resource_set: json!({
                "Name": "set.with-three_types",
                "Description": "resources imported into a Firewall Manager policy",
                "ResourceTypeList": [
                    "AWS::EC2::Instance",
                    "AWS::EC2::VPC",
                    "AWS::ElasticLoadBalancingV2::LoadBalancer",
                ],
            }),
            tags: json!([{ "Key": "team", "Value": "security" }]),
        },
        Case {
            // An empty description is inside the modelled 0..256, and is not the
            // same thing as an absent one.
            name: "empty description",
            resource_set: json!({
                "Name": "empty-description",
                "Description": "",
                "ResourceTypeList": ["AWS::EC2::Instance"],
            }),
            tags: json!([]),
        },
        Case {
            // The upper published bounds: a 128-character name and a
            // 256-character description.
            name: "longest modelled name and description",
            resource_set: json!({
                "Name": "n".repeat(128),
                "Description": "d".repeat(256),
                "ResourceTypeList": ["AWS::EC2::Instance", "AWS::EC2::VPC"],
            }),
            tags: json!([
                { "Key": "a", "Value": "1" },
                { "Key": "b", "Value": "2" },
                { "Key": "c", "Value": "3" },
            ]),
        },
        Case {
            name: "five resource types",
            resource_set: json!({
                "Name": "wide",
                "Description": "many types",
                "ResourceTypeList": [
                    "AWS::ApiGateway::Stage",
                    "AWS::CloudFront::Distribution",
                    "AWS::EC2::EIP",
                    "AWS::EC2::Instance",
                    "AWS::EC2::NetworkInterface",
                ],
            }),
            tags: json!([]),
        },
    ]
}

/// Create a row's resource set and return the `PutResourceSet` response.
async fn create(router: &MultiServiceRouter, case: &Case) -> Value {
    let (status, body) = call(
        router,
        "PutResourceSet",
        &json!({ "ResourceSet": case.resource_set, "TagList": case.tags }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected PutResourceSet to answer 200, got {status} with {body}",
        case.name
    );
    body
}

/// The `ResourceSetSummary` a stored resource set must project to: the summary's
/// own modelled members, read off the `ResourceSet` the writer stored. An absent
/// `Description` stays absent, so the summary never invents one.
fn expected_summary(stored: &Value) -> Value {
    let mut want = json!({
        "Id": stored["Id"],
        "Name": stored["Name"],
        "LastUpdateTime": stored["LastUpdateTime"],
        "ResourceSetStatus": stored["ResourceSetStatus"],
    });
    if let Some(description) = stored.get("Description") {
        want["Description"] = description.clone();
    }
    want
}

/// Walk every page of `ListResourceSets` for one `MaxResults` setting and return
/// the summaries in the order the pages reported them.
///
/// Asserts rule 2's page bound and termination on the way: no page exceeds the
/// requested bound (or the model's maximum when none is requested), and every
/// page but the last carries a `NextToken`.
async fn list_all(
    router: &MultiServiceRouter,
    max_results: Option<usize>,
    expected_total: usize,
) -> Vec<Value> {
    // The model's `com.amazonaws.fms#PaginationMaxResults` maximum, which bounds
    // a page the caller set no bound for.
    const MAX_PAGE_SIZE: usize = 100;

    let bound = max_results.unwrap_or(MAX_PAGE_SIZE);
    let mut seen: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut request = json!({});
        if let Some(max) = max_results {
            request["MaxResults"] = json!(max);
        }
        if let Some(token) = &token {
            request["NextToken"] = json!(token);
        }
        let (status, body) = call(router, "ListResourceSets", &request).await;
        assert_eq!(
            status, 200,
            "case MaxResults={max_results:?}: expected 200, got {status} with {body}"
        );
        let page = body["ResourceSets"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case MaxResults={max_results:?}: no ResourceSets array in {body}")
            })
            .clone();
        assert!(
            page.len() <= bound,
            "case MaxResults={max_results:?}: a page carried {} summaries, more than the {bound} \
             it is bounded at",
            page.len()
        );
        let next = body["NextToken"].as_str().map(str::to_string);
        if next.is_some() {
            // Only the LAST page may be short: a non-final page that is not full
            // would mean the token does not follow the page bound.
            assert_eq!(
                page.len(),
                bound,
                "case MaxResults={max_results:?} page {pages}: a page followed by a NextToken \
                 must be full, got {}",
                page.len()
            );
        }
        seen.extend(page);
        pages += 1;
        assert!(
            pages <= expected_total + 1,
            "case MaxResults={max_results:?}: paging did not terminate after {pages} pages"
        );
        match next {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    seen
}

/// Rules 1 and 2: every stored resource set is reported once, as its summary
/// projection, in `Id` order, across every page size — and the reader
/// `GetResourceSet` agrees with each summary.
#[tokio::test]
async fn list_resource_sets_projects_every_stored_resource_set_and_pages_it() {
    let (router, _engine) = emulator_with_engine();
    associate_admin(&router).await;

    // Store the whole table, keeping what each row must project to.
    let mut expected: Vec<(String, Value)> = Vec::new();
    for case in cases() {
        let put = create(&router, &case).await;
        let stored = put["ResourceSet"].clone();
        let id = stored["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: the create minted an Id, got {put}", case.name))
            .to_string();
        expected.push((id, expected_summary(&stored)));
    }
    // The store is keyed by the resource set's Id, which is the order pages
    // follow.
    expected.sort_by(|a, b| a.0.cmp(&b.0));

    for max_results in [Some(1), Some(2), Some(3), Some(100), None] {
        let seen = list_all(&router, max_results, expected.len()).await;
        assert_eq!(
            seen.len(),
            expected.len(),
            "case MaxResults={max_results:?}: expected {} summaries across all pages, got {}",
            expected.len(),
            seen.len()
        );
        for (summary, (id, want)) in seen.iter().zip(&expected) {
            assert_eq!(
                summary, want,
                "case MaxResults={max_results:?}: the summary of {id} is not the stored resource \
                 set's projection, got {summary}"
            );
        }
        let ids: Vec<&str> = seen
            .iter()
            .map(|summary| summary["Id"].as_str().unwrap_or_default())
            .collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(
            ids, sorted,
            "case MaxResults={max_results:?}: pages must report resource sets in Id order, got \
             {ids:?}"
        );
    }

    // The summary is a projection of the SAME record the reader reports, so
    // every member it carries equals that reader's, and it carries nothing the
    // summary shape does not model.
    for (id, want) in &expected {
        let (status, got) = call(&router, "GetResourceSet", &json!({ "Identifier": id })).await;
        assert_eq!(
            status, 200,
            "case {id}: expected GetResourceSet to answer 200, got {status} with {got}"
        );
        let stored = &got["ResourceSet"];
        assert_eq!(
            *want,
            expected_summary(stored),
            "case {id}: the listed summary must agree with GetResourceSet, got {stored}"
        );
        for leaked in ["UpdateToken", "ResourceTypeList"] {
            assert!(
                want.get(leaked).is_none(),
                "case {id}: ResourceSetSummary models no {leaked}, got {want}"
            );
        }
    }
}

/// Rule 1 on the empty store: no resource sets is an empty page, not an error
/// and not a token.
#[tokio::test]
async fn an_empty_store_lists_no_resource_sets_and_no_token() {
    let (router, _engine) = emulator_with_engine();
    associate_admin(&router).await;

    for request in [
        json!({}),
        json!({ "MaxResults": 1 }),
        json!({ "MaxResults": 100 }),
    ] {
        let (status, body) = call(&router, "ListResourceSets", &request).await;
        assert_eq!(
            status, 200,
            "case {request}: expected 200 on an empty store, got {status} with {body}"
        );
        assert_eq!(
            body["ResourceSets"],
            json!([]),
            "case {request}: an empty store must list no resource sets, got {body}"
        );
        assert!(
            body["NextToken"].is_null(),
            "case {request}: an empty page must carry no NextToken, got {body}"
        );
    }
}

/// Rule 3: listing is a report. Two identical listings agree, the stored records
/// are untouched (same ids, same `LastUpdateTime`, same update tokens), and a
/// following `GetResourceSet` reports what it did before.
#[tokio::test]
async fn listing_resource_sets_changes_nothing_a_later_caller_sees() {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;

    let mut before: Vec<Value> = Vec::new();
    for case in cases() {
        before.push(create(&router, &case).await);
    }

    let stored_before = with_store(&engine, |store| {
        store
            .resource_sets
            .iter()
            .map(|(id, record)| {
                (
                    id.clone(),
                    record.update_token.clone(),
                    record.updated_at,
                    record.resources.len(),
                )
            })
            .collect::<Vec<_>>()
    });

    let first = list_all(&router, Some(2), before.len()).await;
    let second = list_all(&router, Some(2), before.len()).await;
    assert_eq!(
        first, second,
        "a report must be repeatable: two identical listings differ, got {first:?} and {second:?}"
    );

    let stored_after = with_store(&engine, |store| {
        store
            .resource_sets
            .iter()
            .map(|(id, record)| {
                (
                    id.clone(),
                    record.update_token.clone(),
                    record.updated_at,
                    record.resources.len(),
                )
            })
            .collect::<Vec<_>>()
    });
    assert_eq!(
        stored_before, stored_after,
        "listing must not write: the stored resource sets moved"
    );

    for put in &before {
        let id = put["ResourceSet"]["Id"].as_str().expect("a minted Id");
        let (status, got) = call(&router, "GetResourceSet", &json!({ "Identifier": id })).await;
        assert_eq!(
            status, 200,
            "case {id}: expected GetResourceSet to answer 200 after listing, got {status} with \
             {got}"
        );
        assert_eq!(
            got["ResourceSet"], put["ResourceSet"],
            "case {id}: listing must not change what GetResourceSet reports, got {got}"
        );
    }
}

/// Rule 2 at the bounds, and rule 4's token refusal: a `MaxResults` outside the
/// modelled range is refused by the boundary, and a `NextToken` this service did
/// not mint for a resource-set page is refused rather than resumed from as an
/// arbitrary key.
#[tokio::test]
async fn an_out_of_range_page_size_or_a_foreign_token_is_refused() {
    let (router, _engine) = emulator_with_engine();
    associate_admin(&router).await;
    for case in cases() {
        create(&router, &case).await;
    }

    // One past each published bound of MaxResults (1..100).
    for out_of_range in [0, 101] {
        let (status, body) = call(
            &router,
            "ListResourceSets",
            &json!({ "MaxResults": out_of_range }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case MaxResults={out_of_range}: a value outside the modelled 1..100 must be refused, \
             got {status} with {body}"
        );
    }

    // Tokens that are not a resource-set page boundary: a value that is not a
    // token at all, the empty-ish hex of a short key, and a well-formed token
    // for ANOTHER page family (a 36-character `PolicyId`-shaped store key),
    // which must not be resumed from as a resource-set id.
    let foreign = [
        "not-a-token-this-service-minted".to_string(),
        state::encode_key_token("tooshort"),
        state::encode_key_token("00000000-0000-0000-0000-000000000000"),
        state::encode_key_token(&"z".repeat(23)),
    ];
    for token in &foreign {
        let (status, body) = call(
            &router,
            "ListResourceSets",
            &json!({ "NextToken": token }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidInputException"),
            "case {token:?}: a NextToken this service did not mint for a resource-set page must \
             be refused, got {status} with {body}"
        );
    }

    // A token the service DID mint round-trips, so the refusals above are not
    // refusing every token.
    let (status, body) = call(&router, "ListResourceSets", &json!({ "MaxResults": 1 })).await;
    assert_eq!(
        status, 200,
        "the first page must be served, got {status} with {body}"
    );
    let minted = body["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a bounded first page must carry a NextToken, got {body}"))
        .to_string();
    let (status, body) = call(
        &router,
        "ListResourceSets",
        &json!({ "MaxResults": 1, "NextToken": minted }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {minted:?}: a minted NextToken must name the next page, got {status} with {body}"
    );
}

/// Rule 4's gate: with no Firewall Manager administrator association the call is
/// refused with HTTP 400 `AccessDeniedException` — not the scaffold's 501, and
/// not an empty page — and the refusal is decided before the request's own
/// members are examined, so a bad `NextToken` gets the same answer.
#[tokio::test]
async fn listing_resource_sets_is_refused_without_an_administrator() {
    let (router, _engine) = emulator_with_engine();

    for request in [
        json!({}),
        json!({ "MaxResults": 5 }),
        json!({ "NextToken": "not-a-token-this-service-minted" }),
    ] {
        let (status, body) = call(&router, "ListResourceSets", &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "AccessDeniedException"),
            "case {request}: with no administrator association the call must be refused, got \
             {status} with {body}"
        );
    }
}

/// Rule 1 against the deleter: the listing reports exactly what is stored, so a
/// resource set `DeleteResourceSet` removed is gone from every page while the
/// others still project unchanged.
#[tokio::test]
async fn a_deleted_resource_set_is_no_longer_listed() {
    let (router, _engine) = emulator_with_engine();
    associate_admin(&router).await;

    let mut remaining: Vec<(String, Value)> = Vec::new();
    for case in cases() {
        let stored = create(&router, &case).await["ResourceSet"].clone();
        let id = stored["Id"].as_str().expect("a minted Id").to_string();
        remaining.push((id, expected_summary(&stored)));
    }
    remaining.sort_by(|a, b| a.0.cmp(&b.0));

    // Delete the rows one at a time, re-listing after each: the listing follows
    // the store rather than a cached page.
    while let Some((id, _)) = remaining.first().cloned() {
        let (status, body) = call(&router, "DeleteResourceSet", &json!({ "Identifier": id })).await;
        assert_eq!(
            status, 200,
            "case {id}: expected DeleteResourceSet to answer 200, got {status} with {body}"
        );
        remaining.retain(|(stored_id, _)| *stored_id != id);

        let seen = list_all(&router, Some(2), remaining.len()).await;
        let want: Vec<Value> = remaining.iter().map(|(_, want)| want.clone()).collect();
        assert_eq!(
            seen, want,
            "case {id}: after the delete the listing must report exactly the resource sets that \
             are still stored, got {seen:?}"
        );
    }

    let (status, body) = call(&router, "ListResourceSets", &json!({})).await;
    assert_eq!(
        (status, body["ResourceSets"].clone()),
        (200, json!([])),
        "with every resource set deleted the listing must be empty, got {status} with {body}"
    );
}

/// Rule 2's default bound: with more resource sets stored than one page holds,
/// a caller who sends no `MaxResults` still gets a bounded page and a
/// `NextToken`, and every resource set is reachable by following it.
#[tokio::test]
async fn a_page_is_bounded_even_when_the_caller_sets_no_bound() {
    // The model's `com.amazonaws.fms#PaginationMaxResults` maximum (range
    // 1..100), which is the largest page this operation serves.
    const MAX_PAGE_SIZE: usize = 100;
    // More than one page, and below `MAX_RESOURCE_SETS` in
    // `src/ops/put_resource_set.rs`, so every create succeeds.
    const STORED: usize = MAX_PAGE_SIZE + 5;

    let (router, _engine) = emulator_with_engine();
    associate_admin(&router).await;

    let case = &cases()[0];
    for created in 0..STORED {
        let (status, body) = call(
            &router,
            "PutResourceSet",
            &json!({ "ResourceSet": case.resource_set, "TagList": case.tags }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case create #{created}: expected PutResourceSet to answer 200, got {status} with \
             {body}"
        );
    }

    let seen = list_all(&router, None, STORED).await;
    assert_eq!(
        seen.len(),
        STORED,
        "case unbounded-list: expected {STORED} summaries across all pages, got {}",
        seen.len()
    );
    let mut ids: Vec<&str> = seen
        .iter()
        .map(|summary| summary["Id"].as_str().unwrap_or_default())
        .collect();
    let total = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(
        ids.len(),
        total,
        "case unbounded-list: every resource set must be reported exactly once, got {} distinct \
         ids out of {total}",
        ids.len()
    );
}
