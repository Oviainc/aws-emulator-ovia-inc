// SPDX-License-Identifier: CC0-1.0

//! Two rules of the paginated discovery entry points, each asserted over a SET of inputs
//! rather than over one recorded request:
//!
//! 1. `searchText` is a KEYWORD query: `SearchListings` "can search by keyword … and sort
//!    results by relevance" (`smithy_query(query="doc",shape="SearchListings")`), so a
//!    listing matches when ANY of the query's terms occurs, case-insensitively, in the
//!    content that listing presents — the whole query string need not occur verbatim in one
//!    member — and `RELEVANCE` ranks a listing by how many of the terms it carries and
//!    where. The match set of a two-keyword query is therefore the UNION of its keywords'
//!    match sets.
//! 2. A paginated read is continuable exactly while matches remain: `nextToken` is returned
//!    "[i]f … there are more results available", so for every page size a client walks the
//!    whole match set page by page without a repeat, a gap or a token on the last page —
//!    for `SearchListings` (whose `totalResults` counts the matches, not the page) and for
//!    `ListPurchaseOptions` alike.
//!
//! The second rule is what makes a truncated page recoverable at all: without the token a
//! paginating client stops at page one. The first is why a page truncates in the first
//! place — a query whose keywords are only matched as one literal string selects far fewer
//! listings than it should, so nothing is left to page over.
//!
//! A third rule is pinned beside them because it is the same paged path: every product the
//! catalog lists surfaces its offers through `ListPurchaseOptions` when filtered by that
//! product and the seller of record that sells it, as an `OFFER` or `OFFERSET` purchase
//! option carrying the entities a buyer drills into next.
//!
//! Every request goes through the `MultiServiceRouter`, so the protocol, the routing table,
//! input validation and the handlers are exercised end to end.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::catalog_drilldown::{
    array, ids, matches_search_text, ok, relevance, router, search_terms, summary_haystack, text,
    whole_catalog,
};

/// `maxResults`' modelled maximum (`smithy_query(query="operation",operation="SearchListings")`:
/// `MaxResults` is `@range 1..100`), which is the biggest single page a client can ask for.
const MAX_PAGE: u64 = 100;

/// The page sizes every paging rule is asserted over: the smallest page the model allows,
/// a few in between, the modelled default and the biggest page it allows.
const PAGE_SIZES: [u64; 6] = [1, 2, 3, 7, 25, MAX_PAGE];

/// The listing ids one whole `SearchListings` page carries, in the order it returns them,
/// with the match count it reports.
async fn search_page(
    router: &MultiServiceRouter,
    search_text: &str,
    max_results: u64,
    next_token: Option<&str>,
) -> (Vec<String>, u64, Option<String>) {
    let mut input = json!({ "maxResults": max_results });
    if !search_text.is_empty() {
        input["searchText"] = json!(search_text);
    }
    if let Some(token) = next_token {
        input["nextToken"] = json!(token);
    }
    let body = ok(router, "searchListings", &input).await;
    let total = body
        .get("totalResults")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| panic!("case searchText={search_text:?}: totalResults is required, got {body}"));
    let token = body
        .get("nextToken")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    (
        ids(&array(&body, "listingSummaries"), "listingId"),
        total,
        token,
    )
}

/// Rule 1: a keyword query selects the UNION of its keywords' matches, whatever case,
/// punctuation or order the request wrote them in.
#[tokio::test]
async fn a_keyword_query_selects_the_union_of_its_keywords_matches() {
    let router = router();
    let catalog = whole_catalog(&router).await;

    // Rows span the ways a query can carry keywords: one word; two words that sit together
    // in a name; two words whose matches are DIFFERENT listings; a word repeated; keywords
    // separated by punctuation and extra space; one keyword nothing carries beside one many
    // do; and a query nothing matches at all.
    let cases: [&str; 10] = [
        "firewall",
        "FIREWALL",
        "cloud firewall",
        "firewall analytics",
        "analytics storage security",
        "firewall firewall",
        "  firewall,   analytics!  ",
        "zzz-not-in-the-catalog analytics",
        "zzz-not-in-the-catalog",
        "data",
    ];
    for search_text in cases {
        let terms = search_terms(search_text);

        // What the rule says the answer is, computed from the catalog the client can see.
        let mut expected: BTreeSet<String> = catalog
            .iter()
            .filter(|summary| matches_search_text(summary, search_text))
            .map(|summary| text(summary, "listingId"))
            .collect();

        let (returned, total, token) = search_page(&router, search_text, MAX_PAGE, None).await;
        let actual: BTreeSet<String> = returned.iter().cloned().collect();
        assert_eq!(
            actual, expected,
            "case searchText={search_text:?}: expected the union of the keywords' matches \
             {expected:?}, got {actual:?}"
        );
        assert_eq!(
            total,
            expected.len() as u64,
            "case searchText={search_text:?}: totalResults must count the matches, got {total}"
        );
        assert!(
            token.is_none() || total > MAX_PAGE,
            "case searchText={search_text:?}: the whole match set fits one page, so nextToken \
             must be absent, got {token:?}"
        );

        // The union, keyword by keyword: each keyword's own match set is part of the query's,
        // and together they are all of it.
        let mut union: BTreeSet<String> = BTreeSet::new();
        for term in &terms {
            let (one_term, _, _) = search_page(&router, term, MAX_PAGE, None).await;
            let one_term: BTreeSet<String> = one_term.into_iter().collect();
            assert!(
                one_term.is_subset(&expected),
                "case searchText={search_text:?} keyword {term:?}: a keyword's matches must all \
                 be matches of the query, got {one_term:?} outside {expected:?}"
            );
            union.extend(one_term);
        }
        if !terms.is_empty() {
            assert_eq!(
                union, expected,
                "case searchText={search_text:?}: the query must match exactly what its \
                 keywords match together, got {union:?} and {expected:?}"
            );
        }

        // Every returned listing really carries one of the keywords, read off its own summary.
        for listing_id in &returned {
            let summary = catalog
                .iter()
                .find(|summary| &text(summary, "listingId") == listing_id)
                .unwrap_or_else(|| panic!("case searchText={search_text:?}: {listing_id} is not a catalog listing"));
            let haystack = summary_haystack(summary);
            assert!(
                terms.is_empty() || terms.iter().any(|term| haystack.contains(term)),
                "case searchText={search_text:?}: {listing_id} carries none of {terms:?} in \
                 {haystack:?}"
            );
        }
        expected.clear();
    }
}

/// Rule 1's ordering half: a listing carrying MORE of the query's keywords outranks one
/// carrying fewer, so `RELEVANCE` orders by the keywords and not merely by who matched.
#[tokio::test]
async fn relevance_ranks_a_listing_by_how_many_keywords_it_carries() {
    let router = router();

    // Rows: multi-keyword queries whose matches do NOT all carry the same number of
    // keywords, so an ordering that ignored the per-keyword score would show up here.
    for search_text in [
        "cloud firewall",
        "firewall analytics",
        "analytics storage security",
        "data forecasting telemetry",
    ] {
        for sort_order in ["DESCENDING", "ASCENDING"] {
            let body = ok(
                &router,
                "searchListings",
                &json!({
                    "searchText": search_text,
                    "sortBy": "RELEVANCE",
                    "sortOrder": sort_order,
                    "maxResults": MAX_PAGE,
                }),
            )
            .await;
            let summaries = array(&body, "listingSummaries");
            let case = format!("searchText={search_text:?} sortOrder={sort_order}");
            assert!(
                summaries.len() > 1,
                "case {case}: the query must match several listings to order them, got {body}"
            );

            let scores: Vec<u32> = summaries
                .iter()
                .map(|summary| relevance(summary, search_text))
                .collect();
            for (index, window) in scores.windows(2).enumerate() {
                let ordered = if sort_order == "ASCENDING" {
                    window[0] <= window[1]
                } else {
                    window[0] >= window[1]
                };
                assert!(
                    ordered,
                    "case {case}: keyword scores must run {sort_order} at position {index}, got \
                     {scores:?}"
                );
            }
            assert!(
                scores.iter().any(|score| *score > 0),
                "case {case}: a match carries at least one keyword, got {scores:?}"
            );
            // The query's keywords do separate the matches: the best and the worst match
            // differ, which is what makes the ordering above an assertion about keywords.
            let best = scores.iter().max().copied().unwrap_or(0);
            let worst = scores.iter().min().copied().unwrap_or(0);
            assert!(
                best > worst,
                "case {case}: this query must rank its matches differently, got {scores:?}"
            );
        }
    }
}

/// Rule 2 for `SearchListings`: for every page size, the pages walk the whole match set and
/// a token is handed out exactly while matches remain.
#[tokio::test]
async fn every_search_page_is_continuable_while_matches_remain() {
    let router = router();

    // Rows: a query matching the whole catalog, keyword queries matching many, a few and
    // one, and one matching nothing — each walked at every page size.
    for search_text in ["", "a", "data", "cloud firewall", "firewall", "zzz-nothing"] {
        let (whole, total, _) = search_page(&router, search_text, MAX_PAGE, None).await;
        assert_eq!(
            whole.len() as u64,
            total.min(MAX_PAGE),
            "case searchText={search_text:?}: one page must carry every match the catalog has, \
             got {whole:?} of {total}"
        );

        for max_results in PAGE_SIZES {
            let case = format!("searchText={search_text:?} maxResults={max_results}");
            let mut walked: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            // One page per match plus the terminating page is the most a correct walk takes,
            // so a token that never retires fails here instead of looping forever.
            for _ in 0..=total + 1 {
                let (page, page_total, next) =
                    search_page(&router, search_text, max_results, token.as_deref()).await;
                assert_eq!(
                    page_total, total,
                    "case {case}: totalResults counts the matches, so every page reports the \
                     same {total}, got {page_total}"
                );
                let remaining = total - walked.len() as u64;
                assert_eq!(
                    page.len() as u64,
                    remaining.min(max_results),
                    "case {case}: a page carries min(remaining, maxResults) entries, expected \
                     {} with {remaining} left, got {page:?}",
                    remaining.min(max_results)
                );
                walked.extend(page);
                let seen = walked.len() as u64;
                assert_eq!(
                    next.is_some(),
                    seen < total,
                    "case {case}: after {seen} of {total} matches, a token must be present \
                     exactly while matches remain, got {next:?}"
                );
                token = next;
                if token.is_none() {
                    break;
                }
            }
            assert!(
                token.is_none(),
                "case {case}: the walk must terminate, got a token after {} entries",
                walked.len()
            );
            assert_eq!(
                walked, whole,
                "case {case}: the pages must walk the whole match set in order, got {walked:?} \
                 against {whole:?}"
            );
            let unique: BTreeSet<&String> = walked.iter().collect();
            assert_eq!(
                unique.len(),
                walked.len(),
                "case {case}: no listing may be returned twice, got {walked:?}"
            );
        }
    }
}

/// Rule 2 for `ListPurchaseOptions`: the same walk over the purchase options a filter
/// selects, which is the paged read a buyer drills into an offer through.
#[tokio::test]
async fn every_purchase_option_page_is_continuable_while_rows_remain() {
    let router = router();

    // Rows: no filter at all, each purchase option TYPE the model declares, and a product
    // filter no record carries (nothing to page).
    let cases: [(&str, Value); 4] = [
        ("no filter", Value::Null),
        (
            "PURCHASE_OPTION_TYPE=OFFER",
            json!([{ "filterType": "PURCHASE_OPTION_TYPE", "filterValues": ["OFFER"] }]),
        ),
        (
            "PURCHASE_OPTION_TYPE=OFFERSET",
            json!([{ "filterType": "PURCHASE_OPTION_TYPE", "filterValues": ["OFFERSET"] }]),
        ),
        (
            "PRODUCT_ID=p-no-such-product",
            json!([{ "filterType": "PRODUCT_ID", "filterValues": ["p-no-such-product"] }]),
        ),
    ];

    for (case, filters) in cases {
        let page_input = |max_results: u64, token: Option<String>| {
            let mut input = json!({ "maxResults": max_results });
            if !filters.is_null() {
                input["filters"] = filters.clone();
            }
            if let Some(token) = token {
                input["nextToken"] = json!(token);
            }
            input
        };

        let whole = ok(
            &router,
            "listPurchaseOptions",
            &page_input(MAX_PAGE, None),
        )
        .await;
        let expected = ids(&array(&whole, "purchaseOptions"), "purchaseOptionId");

        for max_results in PAGE_SIZES {
            let case = format!("{case} maxResults={max_results}");
            let mut walked: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            for _ in 0..=expected.len() + 1 {
                let body = ok(
                    &router,
                    "listPurchaseOptions",
                    &page_input(max_results, token.clone()),
                )
                .await;
                let page = ids(&array(&body, "purchaseOptions"), "purchaseOptionId");
                let remaining = expected.len() - walked.len();
                assert_eq!(
                    page.len(),
                    remaining.min(usize::try_from(max_results).unwrap_or(usize::MAX)),
                    "case {case}: a page carries min(remaining, maxResults) rows with \
                     {remaining} left, got {page:?}"
                );
                walked.extend(page);
                let next = body
                    .get("nextToken")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                assert_eq!(
                    next.is_some(),
                    walked.len() < expected.len(),
                    "case {case}: after {} of {} rows, a token must be present exactly while \
                     rows remain, got {next:?}",
                    walked.len(),
                    expected.len()
                );
                token = next;
                if token.is_none() {
                    break;
                }
            }
            assert!(
                token.is_none(),
                "case {case}: the walk must terminate, got a token after {} rows",
                walked.len()
            );
            assert_eq!(
                walked, expected,
                "case {case}: the pages must walk the whole result set in order, got {walked:?} \
                 against {expected:?}"
            );
        }
    }
}

/// Rule 3, over EVERY product the catalog lists: a product that has an offer surfaces it
/// through `ListPurchaseOptions` when filtered by that product and by the seller of record
/// that sells it (AND across the two filters), as a purchase option of a modelled type
/// carrying the catalog, its id, its seller and the (product, offer) pairs a buyer drills
/// into next.
#[tokio::test]
async fn every_listed_product_surfaces_its_purchase_options_for_its_seller() {
    let router = router();

    // The products a client reaches through search, which is how it learns a product id at
    // all: every associated product of every listing in the catalog.
    let mut products: Vec<String> = Vec::new();
    for summary in whole_catalog(&router).await {
        for entity in array(&summary, "associatedEntities") {
            if let Some(product) = entity.get("product") {
                let product_id = text(product, "productId");
                if !product_id.is_empty() && !products.contains(&product_id) {
                    products.push(product_id);
                }
            }
        }
    }
    assert!(
        products.len() > 5,
        "case setup: the catalog must list several products, got {products:?}"
    );

    let mut types_seen: BTreeSet<String> = BTreeSet::new();
    for product_id in &products {
        // The product's own purchase options, then the same query narrowed by the seller
        // each of them names: the pair must still select it.
        let by_product = ok(
            &router,
            "listPurchaseOptions",
            &json!({
                "maxResults": MAX_PAGE,
                "filters": [{ "filterType": "PRODUCT_ID", "filterValues": [product_id] }],
            }),
        )
        .await;
        let rows = array(&by_product, "purchaseOptions");
        assert!(
            !rows.is_empty(),
            "case product {product_id}: a listed product's offers must be reachable, got \
             {by_product}"
        );

        for row in &rows {
            let option_id = text(row, "purchaseOptionId");
            let option_type = text(row, "purchaseOptionType");
            let case = format!("product {product_id} option {option_id}");
            assert!(
                ["OFFER", "OFFERSET"].contains(&option_type.as_str()),
                "case {case}: purchaseOptionType must be a modelled PurchaseOptionType, got \
                 {option_type:?}"
            );
            assert!(
                !text(row, "catalog").is_empty() && !option_id.is_empty(),
                "case {case}: catalog and purchaseOptionId must be populated, got {row}"
            );
            let seller = row
                .get("sellerOfRecord")
                .map(|seller| text(seller, "sellerProfileId"))
                .unwrap_or_default();
            assert!(
                !seller.is_empty(),
                "case {case}: sellerOfRecord must name a seller profile, got {row}"
            );
            assert!(
                row.get("badges").is_some_and(Value::is_array),
                "case {case}: badges must be reported, got {row}"
            );
            let entities = array(row, "associatedEntities");
            assert!(
                entities.iter().any(|entity| {
                    entity.get("offer").is_some()
                        && entity
                            .get("product")
                            .is_some_and(|product| &text(product, "productId") == product_id)
                }),
                "case {case}: the option must pair the filtered product with an offer, got \
                 {entities:?}"
            );

            // PRODUCT_ID AND SELLER_OF_RECORD_PROFILE_ID: the pair the drill-down sends.
            let narrowed = ok(
                &router,
                "listPurchaseOptions",
                &json!({
                    "maxResults": MAX_PAGE,
                    "filters": [
                        { "filterType": "PRODUCT_ID", "filterValues": [product_id] },
                        { "filterType": "SELLER_OF_RECORD_PROFILE_ID", "filterValues": [seller] },
                    ],
                }),
            )
            .await;
            let narrowed_ids = ids(&array(&narrowed, "purchaseOptions"), "purchaseOptionId");
            assert!(
                narrowed_ids.contains(&option_id),
                "case {case}: the product and its own seller must still select it, got \
                 {narrowed_ids:?}"
            );

            // The option reads back as the record it was projected from.
            let (operation, member) = if option_type == "OFFERSET" {
                ("getOfferSet", "offerSetId")
            } else {
                ("getOffer", "offerId")
            };
            let detail = ok(&router, operation, &json!({ member: &option_id })).await;
            assert_eq!(
                text(&detail, member),
                option_id,
                "case {case}: {operation} must read back the option's id, got {detail}"
            );
            types_seen.insert(option_type);
        }
    }

    // Both kinds of purchase option are reachable this way, so the rule is not asserted
    // only over offers.
    assert_eq!(
        types_seen,
        ["OFFER".to_string(), "OFFERSET".to_string()]
            .into_iter()
            .collect::<BTreeSet<String>>(),
        "case whole catalog: both OFFER and OFFERSET must be reachable through a product \
         filter, got {types_seen:?}"
    );
}
