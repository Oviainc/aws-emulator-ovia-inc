// SPDX-License-Identifier: CC0-1.0

//! Paging a REFINED discovery search: the rule that a client which narrowed a search by a
//! facet value can still reach past the first page of what it narrowed to.
//!
//! The rule: `SearchListings` is `@paginated` with `maxResults` `@range 1..100` (default 25)
//! and its `nextToken` is documented "[i]f nextToken is returned, there are more results
//! available. Make the call again using the returned token to retrieve the next page"
//! (`smithy_query(query="operation",operation="SearchListings")`). A token can therefore
//! exist only while the query's match set is larger than the page served, which makes the
//! BREADTH of the catalog part of the contract a paginating client sees: the catalog a
//! search pages over has to hold more listings than one page — for a search over the whole
//! catalog and for one a client refined by a facet value `SearchFacets` offered it, which
//! is the narrowest query a discovery client builds.
//!
//! So this suite asserts, over a TABLE of queries spanning the facet types a client can
//! refine by and every page size the model allows:
//!
//! * a token is handed out exactly while matches remain, and is accepted back as the cursor
//!   for the same query, so the pages walk the whole match set once, in order;
//! * every token is a `NextToken`: non-empty, within `@length 0..8192` and matching
//!   `@pattern ^[a-zA-Z0-9+/=]+$`;
//! * the catalog holds more than one default-sized page of listings, more than one listing
//!   in every category of its taxonomy, and more than a default page in the category a
//!   client refining a search reaches first — so refining a search does not make its result
//!   set unpageable.
//!
//! Every request goes through the `MultiServiceRouter`, so the protocol, the routing table,
//! input validation and the handlers are exercised end to end.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_marketplacediscovery_awsmarketplacediscovery::state::category_parent;

use crate::catalog_drilldown::{array, ids, ok, router, text};

/// `SearchListingsInput$maxResults` is `@range 1..100`
/// (`smithy_query(query="operation",operation="SearchListings")`), so this is the largest
/// page a client can ask for.
const MAX_PAGE: u64 = 100;

/// `SearchListingsInput$maxResults` defaults to 25, which is the page a request naming no
/// `maxResults` is served.
const DEFAULT_MAX_RESULTS: u64 = 25;

/// The page sizes every query below is walked at: the smallest the model allows, two in
/// between, the default and the largest it allows.
const PAGE_SIZES: [u64; 5] = [1, 2, 7, DEFAULT_MAX_RESULTS, MAX_PAGE];

/// `NextToken` is `@length 0..8192`
/// (`smithy_query(query="shape",shape="NextToken")`).
const MAX_TOKEN_LENGTH: usize = 8192;

/// One page of a query: the listing ids it carries, the match count it reports and the
/// continuation token it handed out.
async fn search(
    router: &MultiServiceRouter,
    query: &Value,
    max_results: u64,
    next_token: Option<&str>,
) -> (Vec<String>, u64, Option<String>) {
    let mut input = query.clone();
    input["maxResults"] = json!(max_results);
    if let Some(token) = next_token {
        input["nextToken"] = json!(token);
    }
    let body = ok(router, "searchListings", &input).await;
    let total = body
        .get("totalResults")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| panic!("case {input}: totalResults is required, got {body}"));
    let token = body
        .get("nextToken")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    if let Some(token) = &token {
        assert!(
            !token.is_empty()
                && token.len() <= MAX_TOKEN_LENGTH
                && token
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric()
                        || matches!(character, '+' | '/' | '=')),
            "case {input}: nextToken must be a NextToken within @length 0..8192 and matching \
             ^[a-zA-Z0-9+/=]+$, got {token:?}"
        );
    }
    (
        ids(&array(&body, "listingSummaries"), "listingId"),
        total,
        token,
    )
}

/// One facet type's values over the whole catalog, as `(value, count)` pairs, most listings
/// first — the refinements `SearchFacets` offers a client.
async fn facet_values(router: &MultiServiceRouter, facet_type: &str) -> Vec<(String, u64)> {
    let body = ok(
        router,
        "searchFacets",
        &json!({ "facetTypes": [facet_type] }),
    )
    .await;
    let values = body
        .get("listingFacets")
        .and_then(Value::as_object)
        .and_then(|facets| facets.get(facet_type))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        !values.is_empty(),
        "case {facet_type}: the catalog must offer facet values to refine by, got {body}"
    );
    values
        .iter()
        .map(|facet| {
            (
                text(facet, "value"),
                facet.get("count").and_then(Value::as_u64).unwrap_or(0),
            )
        })
        .collect()
}

/// A `filters` list naming one value of one filter type.
fn filtered_by(filter_type: &str, value: &str) -> Value {
    json!({ "filters": [{ "filterType": filter_type, "filterValues": [value] }] })
}

/// Walk one query page by page at `max_results`, asserting the token rule, and return the
/// ids the walk visited.
async fn walk(
    router: &MultiServiceRouter,
    case: &str,
    query: &Value,
    max_results: u64,
    total: u64,
) -> Vec<String> {
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // One page per match plus the terminating page is the most a correct walk takes, so a
    // token that never retires fails here instead of looping forever.
    for _ in 0..=total + 1 {
        let (page, page_total, next) = search(router, query, max_results, token.as_deref()).await;
        assert_eq!(
            page_total, total,
            "case {case} maxResults={max_results}: totalResults counts the matches, so every \
             page reports the same {total}, got {page_total}"
        );
        let remaining = total - walked.len() as u64;
        assert_eq!(
            page.len() as u64,
            remaining.min(max_results),
            "case {case} maxResults={max_results}: a page carries min(remaining, maxResults) \
             entries, expected {} with {remaining} left, got {page:?}",
            remaining.min(max_results)
        );
        walked.extend(page);
        let seen = walked.len() as u64;
        assert_eq!(
            next.is_some(),
            seen < total,
            "case {case} maxResults={max_results}: after {seen} of {total} matches a token \
             must be present exactly while matches remain, got {next:?}"
        );
        token = next;
        if token.is_none() {
            break;
        }
    }
    assert!(
        token.is_none(),
        "case {case} maxResults={max_results}: the walk must terminate, got a token after {} \
         entries",
        walked.len()
    );
    let unique: BTreeSet<&String> = walked.iter().collect();
    assert_eq!(
        unique.len(),
        walked.len(),
        "case {case} maxResults={max_results}: no listing may be returned twice, got {walked:?}"
    );
    walked
}

/// The rule over a table of queries: a refined search is continuable exactly while its own
/// matches remain, at every page size the model allows.
#[tokio::test]
async fn a_refined_search_is_continuable_at_every_page_size() {
    let router = router();

    // The refinements the service itself offers, read off `SearchFacets` rather than written
    // out here, so the table spans the catalog the client can actually see.
    let categories = facet_values(&router, "CATEGORY").await;
    let (top_category, top_count) = categories
        .first()
        .cloned()
        .expect("the catalog offers a CATEGORY facet value");
    let (nested_category, _) = categories
        .iter()
        .find(|(value, _)| category_parent(value).is_some())
        .cloned()
        .expect("the catalog offers a nested CATEGORY facet value");
    let (narrow_category, _) = categories
        .iter()
        .next_back()
        .cloned()
        .expect("the catalog offers a CATEGORY facet value");
    let pricing_models = facet_values(&router, "PRICING_MODEL").await;
    let (fulfillment, _) = facet_values(&router, "FULFILLMENT_OPTION_TYPE")
        .await
        .first()
        .cloned()
        .expect("the catalog offers a FULFILLMENT_OPTION_TYPE facet value");
    let (publisher, _) = facet_values(&router, "PUBLISHER")
        .await
        .first()
        .cloned()
        .expect("the catalog offers a PUBLISHER facet value");

    // Rows: the whole catalog; keyword queries matching many and none; a search refined by
    // the category a client reaches first, by a nested category and by the narrowest one; by
    // each pricing model; by a fulfillment type; by a publisher; and a keyword search
    // refined by a category, which is the facet-refinement flow itself.
    let mut cases: Vec<(String, Value)> = vec![
        ("whole catalog".to_string(), json!({})),
        ("searchText=data".to_string(), json!({ "searchText": "data" })),
        (
            "searchText=platform monitoring".to_string(),
            json!({ "searchText": "platform monitoring" }),
        ),
        (
            "searchText=zzz-not-in-the-catalog".to_string(),
            json!({ "searchText": "zzz-not-in-the-catalog" }),
        ),
        (
            format!("CATEGORY={top_category}"),
            filtered_by("CATEGORY", &top_category),
        ),
        (
            format!("CATEGORY={nested_category}"),
            filtered_by("CATEGORY", &nested_category),
        ),
        (
            format!("CATEGORY={narrow_category}"),
            filtered_by("CATEGORY", &narrow_category),
        ),
        (
            format!("FULFILLMENT_OPTION_TYPE={fulfillment}"),
            filtered_by("FULFILLMENT_OPTION_TYPE", &fulfillment),
        ),
        (
            format!("PUBLISHER={publisher}"),
            filtered_by("PUBLISHER", &publisher),
        ),
    ];
    for (value, _) in &pricing_models {
        cases.push((
            format!("PRICING_MODEL={value}"),
            filtered_by("PRICING_MODEL", value),
        ));
    }
    let mut refined = filtered_by("CATEGORY", &top_category);
    refined["searchText"] = json!("data");
    cases.push((
        format!("searchText=data refined by CATEGORY={top_category}"),
        refined,
    ));

    let mut continuable_at_default = 0_usize;
    for (case, query) in &cases {
        // What the rule says the whole match set is, read at the largest page the model
        // allows: the catalog holds fewer listings than that, so one such page is all of it.
        let (whole, total, token) = search(&router, query, MAX_PAGE, None).await;
        assert_eq!(
            whole.len() as u64,
            total.min(MAX_PAGE),
            "case {case}: one largest page must carry every match, got {} of {total}",
            whole.len()
        );
        assert_eq!(
            token.is_some(),
            total > MAX_PAGE,
            "case {case}: a token must be present only while matches remain past the page, got \
             {token:?} for {total} matches"
        );

        for max_results in PAGE_SIZES {
            let walked = walk(&router, case, query, max_results, total).await;
            assert_eq!(
                walked, whole,
                "case {case} maxResults={max_results}: the pages must walk the whole match set \
                 in order, got {walked:?} against {whole:?}"
            );
            if max_results == DEFAULT_MAX_RESULTS && total > DEFAULT_MAX_RESULTS {
                continuable_at_default += 1;
            }
        }
    }

    // The table is not vacuous: some of these queries — the whole catalog and the category a
    // refining client reaches first among them — really do carry more matches than a
    // default-sized page, which is the case a paginating client cannot otherwise get past.
    assert!(
        continuable_at_default >= 2,
        "case table: queries matching more than the default page of {DEFAULT_MAX_RESULTS} must \
         exist, got {continuable_at_default} (top CATEGORY={top_category} counts {top_count})"
    );
}

/// The catalog's breadth, which is what makes the rule above reachable at all: more listings
/// than one default page, more than one listing in every category of the taxonomy, and more
/// than one default page in the category a refining client reaches first.
#[tokio::test]
async fn the_catalog_holds_more_than_a_page_of_listings_overall_and_in_its_first_refinement() {
    let router = router();

    let (_, total, _) = search(&router, &json!({}), MAX_PAGE, None).await;
    assert!(
        total > DEFAULT_MAX_RESULTS,
        "case whole catalog: the catalog must hold more than one default page of listings, got \
         {total}"
    );

    let categories = facet_values(&router, "CATEGORY").await;
    for (value, count) in &categories {
        assert!(
            *count > 1,
            "case CATEGORY={value}: every category must hold more than one listing, so refining \
             a search by it leaves something to page, got {count}"
        );
        // The count is the match set the value selects when it is passed back as a filter
        // ("[t]he internal value used for filtering when passed back in a search filter"),
        // so a refined search really does have that many matches to page over.
        let (_, refined_total, _) = search(
            &router,
            &filtered_by("CATEGORY", value),
            MAX_PAGE,
            None,
        )
        .await;
        assert_eq!(
            refined_total, *count,
            "case CATEGORY={value}: refining by a facet value must select the listings it \
             counted, got {refined_total} against {count}"
        );
    }

    let (top_category, top_count) = categories
        .first()
        .cloned()
        .expect("the catalog offers a CATEGORY facet value");
    assert!(
        top_count > DEFAULT_MAX_RESULTS,
        "case CATEGORY={top_category}: the first refinement a client takes must leave more than \
         one default page to walk, got {top_count}"
    );
    // A refined search served at the default page size hands back a token, which is the
    // divergence this suite exists for.
    let mut query = filtered_by("CATEGORY", &top_category);
    query["maxResults"] = json!(DEFAULT_MAX_RESULTS);
    let body = ok(&router, "searchListings", &query).await;
    assert!(
        body.get("nextToken").and_then(Value::as_str).is_some(),
        "case CATEGORY={top_category} maxResults={DEFAULT_MAX_RESULTS}: {top_count} matches \
         remain past the page, so nextToken must be present, got {body}"
    );
}
