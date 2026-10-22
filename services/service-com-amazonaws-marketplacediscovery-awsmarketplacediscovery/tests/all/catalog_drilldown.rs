// SPDX-License-Identifier: CC0-1.0

//! End-to-end drill-down over the seeded AWS Marketplace Discovery catalog:
//! `SearchListings` -> `GetListing` -> `GetProduct` -> `ListPurchaseOptions` -> `GetOffer`
//! -> `GetOfferTerms`, plus the rules those operations follow (search matching, ordering,
//! pagination, purchase-option filtering, locale resolution and unknown-id handling).
//!
//! Every suite drives real HTTP requests through the `MultiServiceRouter`, so the
//! protocol, the routing table, input validation and the handlers are all exercised.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_marketplacediscovery_awsmarketplacediscovery::EmulatedService;

/// The model's `POST /2026-02-05/<op>` route prefix (`OPERATIONS.md`).
pub(crate) const PREFIX: &str = "http://discovery-marketplace.us-east-1.amazonaws.com/2026-02-05";

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name, which is what the router's `can_handle` routes on.
pub(crate) const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260205/us-east-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=0000";

pub(crate) fn router() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The suite signs nothing, so request authentication is off for this router only.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation and return its HTTP status and JSON body.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    input: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("{PREFIX}/{operation}"))
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(input.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, body)
}

/// Call one operation, requiring HTTP 200.
pub(crate) async fn ok(router: &MultiServiceRouter, operation: &str, input: &Value) -> Value {
    let (status, body) = call(router, operation, input).await;
    assert_eq!(
        status, 200,
        "case {operation} {input}: expected HTTP 200, got {status} with body {body}"
    );
    body
}

pub(crate) fn array(body: &Value, member: &str) -> Vec<Value> {
    body.get(member)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

pub(crate) fn text(value: &Value, member: &str) -> String {
    value
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

pub(crate) fn ids(summaries: &[Value], member: &str) -> Vec<String> {
    summaries.iter().map(|item| text(item, member)).collect()
}

/// The review-count-weighted average rating a listing summary reports, which is what
/// `sortBy=AVERAGE_CUSTOMER_RATING` orders by.
fn average_rating(summary: &Value) -> f64 {
    let sources = summary
        .get("reviewSummary")
        .map(|review| array(review, "reviewSourceSummaries"))
        .unwrap_or_default();
    let mut weighted = 0.0;
    let mut reviews = 0.0;
    for source in &sources {
        let rating: f64 = text(source, "averageRating").parse().unwrap_or(0.0);
        let count = source
            .get("totalReviews")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        weighted += rating * count;
        reviews += count;
    }
    if reviews > 0.0 {
        weighted / reviews
    } else {
        0.0
    }
}

/// The emulator's relevance ordering: a hit in the name outranks a hit in the description.
fn relevance(summary: &Value, needle: &str) -> u32 {
    if needle.is_empty() {
        return 0;
    }
    let needle = needle.to_lowercase();
    let name = text(summary, "listingName").to_lowercase();
    let haystack = format!(
        "{} {}",
        name,
        text(summary, "shortDescription").to_lowercase()
    );
    u32::from(name.contains(&needle)) * 2 + u32::from(haystack.contains(&needle))
}

/// Every listing in the catalog, as search summaries.
pub(crate) async fn whole_catalog(router: &MultiServiceRouter) -> Vec<Value> {
    let body = ok(router, "searchListings", &json!({ "maxResults": 100 })).await;
    let summaries = array(&body, "listingSummaries");
    assert!(
        !summaries.is_empty(),
        "case whole catalog: the seeded catalog must not be empty, got {body}"
    );
    assert_eq!(
        body.get("totalResults").and_then(Value::as_u64),
        Some(summaries.len() as u64),
        "case whole catalog: totalResults must count the matches, got {body}"
    );
    summaries
}

fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

// ---------------------------------------------------------------------------
// The drill-down the issue describes, asserting the ids agree across the chain.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_drill_down_chain_agrees_on_ids_and_names() {
    let router = router();

    // 1. SearchListings, one result per page, so page 2 exists.
    let page1 = ok(
        &router,
        "searchListings",
        &json!({ "searchText": "analytics", "maxResults": 1 }),
    )
    .await;
    let total = page1
        .get("totalResults")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    assert!(
        total >= 2,
        "case searchListings page 1: expected at least 2 matches for 'analytics', got {total}"
    );
    let first = array(&page1, "listingSummaries");
    assert_eq!(
        first.len(),
        1,
        "case searchListings page 1: maxResults=1 must truncate the page, got {page1}"
    );
    let token = text(&page1, "nextToken");
    assert!(
        !token.is_empty(),
        "case searchListings page 1: more matches remain, so nextToken must be set, got {page1}"
    );

    // 2. The token resumes after the last returned item.
    let page2 = ok(
        &router,
        "searchListings",
        &json!({ "searchText": "analytics", "maxResults": 1, "nextToken": token }),
    )
    .await;
    let second = array(&page2, "listingSummaries");
    assert_eq!(
        second.len(),
        1,
        "case searchListings page 2: expected one item, got {page2}"
    );
    assert_ne!(
        text(&first[0], "listingId"),
        text(&second[0], "listingId"),
        "case searchListings page 2: must not repeat page 1's listing"
    );
    assert_eq!(
        page2.get("totalResults").and_then(Value::as_u64),
        Some(total),
        "case searchListings page 2: totalResults counts matches, not the page, got {page2}"
    );

    // 3. GetListing agrees with the search summary.
    let summary = &first[0];
    let listing_id = text(summary, "listingId");
    let listing = ok(
        &router,
        "getListing",
        &json!({ "listingId": listing_id.clone() }),
    )
    .await;
    for member in ["listingId", "listingName", "catalog", "shortDescription"] {
        assert_eq!(
            text(&listing, member),
            text(summary, member),
            "case getListing {listing_id}: {member} must match the search summary"
        );
    }
    for member in [
        "associatedEntities",
        "badges",
        "categories",
        "fulfillmentOptionSummaries",
        "highlights",
        "pricingModels",
        "pricingUnits",
        "promotionalMedia",
        "resources",
        "sellerEngagements",
        "useCases",
    ] {
        assert!(
            listing.get(member).is_some_and(Value::is_array),
            "case getListing {listing_id}: required list member {member} must be present, got {listing}"
        );
    }
    assert!(
        listing.get("publisher").is_some(),
        "case getListing {listing_id}: required member publisher must be present, got {listing}"
    );

    // 4. GetProduct points back at the listing that associates it.
    let product_id = array(&listing, "associatedEntities")
        .iter()
        .filter_map(|entity| entity.get("product").cloned())
        .map(|product| text(&product, "productId"))
        .next()
        .unwrap_or_else(|| {
            panic!("case getListing {listing_id}: expected an associated product in {listing}")
        });
    let product = ok(
        &router,
        "getProduct",
        &json!({ "productId": product_id.clone() }),
    )
    .await;
    assert_eq!(
        text(&product, "listingId"),
        listing_id,
        "case getProduct {product_id}: listingId must name the listing that associates it"
    );
    assert_eq!(
        text(&product, "catalog"),
        text(&listing, "catalog"),
        "case getProduct {product_id}: catalog must match the listing's"
    );

    // 5. ListPurchaseOptions, filtered to that product. The model's
    //    PurchaseOptionFilterType has no listing filter, so the product id is how a
    //    caller narrows to one listing's purchase options.
    let options_page1 = ok(
        &router,
        "listPurchaseOptions",
        &json!({
            "filters": [{ "filterType": "PRODUCT_ID", "filterValues": [product_id.clone()] }],
            "maxResults": 1,
        }),
    )
    .await;
    let options = array(&options_page1, "purchaseOptions");
    assert_eq!(
        options.len(),
        1,
        "case listPurchaseOptions {product_id}: maxResults=1 must truncate the page, got {options_page1}"
    );
    let option = &options[0];
    let option_products: Vec<String> = array(option, "associatedEntities")
        .iter()
        .filter_map(|entity| entity.get("product").cloned())
        .map(|product| text(&product, "productId"))
        .collect();
    assert!(
        option_products.contains(&product_id),
        "case listPurchaseOptions {product_id}: the row must associate the filtered product, got {option_products:?}"
    );

    // 6. GetOffer resolves an offer the purchase option surfaced.
    let offer_id = array(option, "associatedEntities")
        .iter()
        .filter_map(|entity| entity.get("offer").cloned())
        .map(|offer| text(&offer, "offerId"))
        .next()
        .unwrap_or_else(|| {
            panic!("case listPurchaseOptions {product_id}: expected an offer in {option}")
        });
    let offer = ok(&router, "getOffer", &json!({ "offerId": offer_id.clone() })).await;
    assert_eq!(
        text(&offer, "offerId"),
        offer_id,
        "case getOffer {offer_id}: offerId must echo the resolved offer"
    );
    assert!(
        !text(&offer, "agreementProposalId").is_empty(),
        "case getOffer {offer_id}: agreementProposalId is required, got {offer}"
    );
    assert!(
        offer
            .get("pricingModel")
            .is_some_and(|model| !text(model, "pricingModelType").is_empty()),
        "case getOffer {offer_id}: pricingModel is required, got {offer}"
    );
    let offer_products: Vec<String> = array(&offer, "associatedEntities")
        .iter()
        .filter_map(|entity| entity.get("product").cloned())
        .map(|product| text(&product, "productId"))
        .collect();
    assert!(
        offer_products.contains(&product_id),
        "case getOffer {offer_id}: the offer must associate the product it was reached through, got {offer_products:?}"
    );

    // 7. GetOfferTerms pages over the terms stored against that offer.
    let terms_page1 = ok(
        &router,
        "getOfferTerms",
        &json!({ "offerId": offer_id.clone(), "maxResults": 1 }),
    )
    .await;
    let first_terms = array(&terms_page1, "offerTerms");
    assert_eq!(
        first_terms.len(),
        1,
        "case getOfferTerms {offer_id}: maxResults=1 must truncate the page, got {terms_page1}"
    );
    let terms_token = text(&terms_page1, "nextToken");
    assert!(
        !terms_token.is_empty(),
        "case getOfferTerms {offer_id}: more terms remain, so nextToken must be set, got {terms_page1}"
    );
    let terms_page2 = ok(
        &router,
        "getOfferTerms",
        &json!({ "offerId": offer_id.clone(), "maxResults": 1, "nextToken": terms_token }),
    )
    .await;
    let second_terms = array(&terms_page2, "offerTerms");
    assert_eq!(
        second_terms.len(),
        1,
        "case getOfferTerms {offer_id} page 2: expected one term, got {terms_page2}"
    );
    assert_ne!(
        first_terms[0], second_terms[0],
        "case getOfferTerms {offer_id} page 2: must not repeat page 1's term"
    );
}

// ---------------------------------------------------------------------------
// The rules, each over a set of inputs.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn search_text_matches_name_or_short_description_case_insensitively() {
    let router = router();
    let catalog = whole_catalog(&router).await;

    // Rows differ in case, in length, in how many listings they select, and in whether
    // the term appears in the name or only in the description.
    for term in [
        "analytics",
        "ANALYTICS",
        "Analytics Suite",
        "firewall",
        "data",
        "a",
        "zzz-not-in-the-catalog",
    ] {
        let needle = term.to_lowercase();
        let mut expected: Vec<String> = catalog
            .iter()
            .filter(|summary| {
                format!(
                    "{} {}",
                    text(summary, "listingName"),
                    text(summary, "shortDescription")
                )
                .to_lowercase()
                .contains(&needle)
            })
            .map(|summary| text(summary, "listingId"))
            .collect();
        expected.sort();

        let body = ok(
            &router,
            "searchListings",
            &json!({ "searchText": term, "maxResults": 100 }),
        )
        .await;
        let mut actual = ids(&array(&body, "listingSummaries"), "listingId");
        actual.sort();
        assert_eq!(
            actual, expected,
            "case searchText {term:?}: expected {expected:?}, got {actual:?}"
        );
        assert_eq!(
            body.get("totalResults").and_then(Value::as_u64),
            Some(expected.len() as u64),
            "case searchText {term:?}: totalResults must count the matches, got {body}"
        );
        assert!(
            body.get("nextToken").is_none(),
            "case searchText {term:?}: no matches remain, so nextToken must be absent, got {body}"
        );
    }
}

#[tokio::test]
async fn sort_by_and_sort_order_order_the_whole_match_set() {
    let router = router();
    let catalog = whole_catalog(&router).await;
    let mut all_ids = ids(&catalog, "listingId");
    all_ids.sort();

    // Rows span both sort keys, both directions, and a query with and without searchText.
    let cases = [
        ("AVERAGE_CUSTOMER_RATING", "DESCENDING", ""),
        ("AVERAGE_CUSTOMER_RATING", "ASCENDING", ""),
        ("RELEVANCE", "DESCENDING", "data"),
        ("RELEVANCE", "ASCENDING", "data"),
    ];
    for (sort_by, sort_order, search_text) in cases {
        let mut input = json!({
            "maxResults": 100,
            "sortBy": sort_by,
            "sortOrder": sort_order,
        });
        if !search_text.is_empty() {
            input["searchText"] = json!(search_text);
        }
        let body = ok(&router, "searchListings", &input).await;
        let summaries = array(&body, "listingSummaries");
        let case = format!("sortBy={sort_by} sortOrder={sort_order} searchText={search_text:?}");

        if search_text.is_empty() {
            let mut returned = ids(&summaries, "listingId");
            returned.sort();
            assert_eq!(
                returned, all_ids,
                "case {case}: an unfiltered search must return every listing"
            );
        }

        let keys: Vec<f64> = summaries
            .iter()
            .map(|summary| {
                if sort_by == "AVERAGE_CUSTOMER_RATING" {
                    average_rating(summary)
                } else {
                    f64::from(relevance(summary, search_text))
                }
            })
            .collect();
        assert!(
            keys.len() > 1,
            "case {case}: need at least two results to assert an ordering, got {keys:?}"
        );
        for window in keys.windows(2) {
            let ordered = if sort_order == "ASCENDING" {
                window[0] <= window[1]
            } else {
                window[0] >= window[1]
            };
            assert!(
                ordered,
                "case {case}: sort keys must run {sort_order}, got {keys:?}"
            );
        }
    }
}

#[tokio::test]
async fn paging_a_search_covers_every_match_exactly_once() {
    let router = router();
    let single_page = whole_catalog(&router).await;
    let expected = ids(&single_page, "listingId");

    // Rows span page sizes below, equal to and above the match count.
    for max_results in [1_u64, 2, 3, 5, expected.len() as u64, 100] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut input = json!({ "maxResults": max_results });
            if let Some(token) = &token {
                input["nextToken"] = json!(token);
            }
            let body = ok(&router, "searchListings", &input).await;
            let summaries = array(&body, "listingSummaries");
            assert!(
                summaries.len() as u64 <= max_results,
                "case maxResults={max_results}: a page must not exceed maxResults, got {body}"
            );
            collected.extend(ids(&summaries, "listingId"));
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case maxResults={max_results}: pagination must terminate, collected {collected:?}"
            );
            match body.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            collected, expected,
            "case maxResults={max_results}: paging must yield every match once, in order"
        );
    }
}

#[tokio::test]
async fn a_next_token_that_was_not_issued_for_this_query_is_refused() {
    let router = router();

    let analytics = ok(
        &router,
        "searchListings",
        &json!({ "searchText": "analytics", "maxResults": 1 }),
    )
    .await;
    let analytics_token = text(&analytics, "nextToken");
    assert!(
        !analytics_token.is_empty(),
        "case setup: the analytics search must return a nextToken, got {analytics}"
    );

    // Rows: a token issued for another searchText, for another sort order, a base64 blob
    // this service never minted, and a token whose payload is not a cursor at all.
    let cases: [(&str, Value); 4] = [
        (
            "token from another searchText",
            json!({ "searchText": "firewall", "maxResults": 1, "nextToken": analytics_token.clone() }),
        ),
        (
            "token from another sortOrder",
            json!({ "searchText": "analytics", "maxResults": 1, "sortOrder": "ASCENDING", "nextToken": analytics_token.clone() }),
        ),
        (
            "foreign base64 token",
            json!({ "searchText": "analytics", "maxResults": 1, "nextToken": "Zm9yZWlnbg==" }),
        ),
        (
            "token with a non-numeric offset",
            json!({ "searchText": "analytics", "maxResults": 1, "nextToken": "eDpTZWFyY2hMaXN0aW5ncw==" }),
        ),
    ];
    for (case, input) in cases {
        let (status, body) = call(&router, "searchListings", &input).await;
        assert_eq!(
            status, 400,
            "case {case}: a nextToken this service did not issue for this query must be refused, got {status} with {body}"
        );
        assert_eq!(
            text(&body, "__type"),
            "ValidationException",
            "case {case}: expected a ValidationException, got {body}"
        );
    }

    // The same token on the same query still works, so the refusal is about identity.
    let resumed = ok(
        &router,
        "searchListings",
        &json!({ "searchText": "analytics", "maxResults": 1, "nextToken": analytics_token }),
    )
    .await;
    assert_eq!(
        array(&resumed, "listingSummaries").len(),
        1,
        "case resume the same query: expected one item, got {resumed}"
    );
}

#[tokio::test]
async fn purchase_option_filters_select_rows_from_state() {
    let router = router();
    let all = ok(&router, "listPurchaseOptions", &json!({ "maxResults": 100 })).await;
    let all_rows = array(&all, "purchaseOptions");
    assert!(
        all_rows.len() > 2,
        "case setup: the seeded catalog must hold several purchase options, got {all}"
    );
    let all_ids = ids(&all_rows, "purchaseOptionId");

    let a_product = array(&all_rows[0], "associatedEntities")
        .first()
        .and_then(|entity| entity.get("product").cloned())
        .map(|product| text(&product, "productId"))
        .unwrap_or_else(|| panic!("case setup: expected an associated product in {all}"));
    let a_seller = all_rows[0]
        .get("sellerOfRecord")
        .map(|seller| text(seller, "sellerProfileId"))
        .unwrap_or_default();

    // One row per documented filter type (PurchaseOptionFilterType), including both
    // values of the two-valued ones.
    let cases: [(&str, Vec<String>); 7] = [
        ("PRODUCT_ID", vec![a_product.clone()]),
        ("SELLER_OF_RECORD_PROFILE_ID", vec![a_seller.clone()]),
        ("PURCHASE_OPTION_TYPE", vec!["OFFER".to_string()]),
        ("PURCHASE_OPTION_TYPE", vec!["OFFERSET".to_string()]),
        ("VISIBILITY_SCOPE", vec!["PRIVATE".to_string()]),
        ("AVAILABILITY_STATUS", vec!["AVAILABLE".to_string()]),
        ("AVAILABILITY_STATUS", vec!["EXPIRED".to_string()]),
    ];
    for (filter_type, filter_values) in cases {
        let case = format!("{filter_type}={filter_values:?}");
        let body = ok(
            &router,
            "listPurchaseOptions",
            &json!({
                "filters": [{ "filterType": filter_type, "filterValues": filter_values }],
                "maxResults": 100,
            }),
        )
        .await;
        let rows = array(&body, "purchaseOptions");
        for row in &rows {
            let id = text(row, "purchaseOptionId");
            assert!(
                all_ids.contains(&id),
                "case {case}: every filtered row must also appear unfiltered, got {id}"
            );
            match filter_type {
                "PRODUCT_ID" => {
                    let products: Vec<String> = array(row, "associatedEntities")
                        .iter()
                        .filter_map(|entity| entity.get("product").cloned())
                        .map(|product| text(&product, "productId"))
                        .collect();
                    assert!(
                        products.contains(&a_product),
                        "case {case}: row {id} must associate the filtered product, got {products:?}"
                    );
                }
                "SELLER_OF_RECORD_PROFILE_ID" => {
                    let seller = row
                        .get("sellerOfRecord")
                        .map(|seller| text(seller, "sellerProfileId"))
                        .unwrap_or_default();
                    assert_eq!(
                        seller, a_seller,
                        "case {case}: row {id} must be sold by the filtered seller"
                    );
                }
                "PURCHASE_OPTION_TYPE" => assert_eq!(
                    text(row, "purchaseOptionType"),
                    filter_values[0],
                    "case {case}: row {id} must have the filtered type"
                ),
                "AVAILABILITY_STATUS" => {
                    let expires = row.get("expirationTime").and_then(Value::as_f64);
                    let expired = expires.is_some_and(|expires| expires <= now_secs());
                    assert_eq!(
                        expired,
                        filter_values[0] == "EXPIRED",
                        "case {case}: row {id} expirationTime {expires:?} disagrees with the filtered status"
                    );
                }
                _ => {}
            }
        }
        // Each of these filters selects at least one seeded row, except OFFERSET/PRIVATE
        // which the assertions above already pin when present.
        if filter_type != "VISIBILITY_SCOPE" {
            assert!(
                !rows.is_empty(),
                "case {case}: expected at least one matching row, got {body}"
            );
        }
    }

    // A filter nothing matches is an empty list, not an error.
    let empty = ok(
        &router,
        "listPurchaseOptions",
        &json!({
            "filters": [{ "filterType": "PRODUCT_ID", "filterValues": ["p-does-not-exist"] }],
            "maxResults": 100,
        }),
    )
    .await;
    assert_eq!(
        array(&empty, "purchaseOptions").len(),
        0,
        "case PRODUCT_ID=[p-does-not-exist]: expected an empty list, got {empty}"
    );

    // A value outside the set its filter type documents is refused.
    for (filter_type, value) in [
        ("PURCHASE_OPTION_TYPE", "SUBSCRIPTION"),
        ("VISIBILITY_SCOPE", "PUBLIC"),
        ("AVAILABILITY_STATUS", "PENDING"),
    ] {
        let (status, body) = call(
            &router,
            "listPurchaseOptions",
            &json!({
                "filters": [{ "filterType": filter_type, "filterValues": [value] }],
                "maxResults": 100,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {filter_type}={value}: an undocumented filter value must be refused, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn search_filters_narrow_the_catalog_by_the_filtered_attribute() {
    let router = router();
    let catalog = whole_catalog(&router).await;

    // Rows: one per documented SearchFilterType that a listing summary carries, so the
    // expectation is computed from the catalog rather than written by hand.
    let cases: [(&str, &str, &str, &str); 5] = [
        ("CATEGORY", "categories", "categoryId", ""),
        ("PUBLISHER", "", "sellerProfileId", "publisher"),
        (
            "FULFILLMENT_OPTION_TYPE",
            "fulfillmentOptionSummaries",
            "fulfillmentOptionType",
            "",
        ),
        ("PRICING_MODEL", "pricingModels", "pricingModelType", ""),
        ("PRICING_UNIT", "pricingUnits", "pricingUnitType", ""),
    ];
    for (filter_type, list_member, key, struct_member) in cases {
        // Take the value off the LAST listing in the catalog, so the case is not pinned
        // to whichever listing happens to sort first.
        let last = catalog.last().expect("a non-empty catalog");
        let value = if struct_member.is_empty() {
            array(last, list_member)
                .first()
                .map(|item| text(item, key))
                .unwrap_or_default()
        } else {
            last.get(struct_member)
                .map(|item| text(item, key))
                .unwrap_or_default()
        };
        assert!(
            !value.is_empty(),
            "case {filter_type}: the catalog must carry a value to filter on, got {last}"
        );

        let mut expected: Vec<String> = catalog
            .iter()
            .filter(|summary| {
                if struct_member.is_empty() {
                    array(summary, list_member)
                        .iter()
                        .any(|item| text(item, key) == value)
                } else {
                    summary
                        .get(struct_member)
                        .map(|item| text(item, key))
                        .as_deref()
                        == Some(value.as_str())
                }
            })
            .map(|summary| text(summary, "listingId"))
            .collect();
        expected.sort();

        let body = ok(
            &router,
            "searchListings",
            &json!({
                "filters": [{ "filterType": filter_type, "filterValues": [value.clone()] }],
                "maxResults": 100,
            }),
        )
        .await;
        let mut actual = ids(&array(&body, "listingSummaries"), "listingId");
        actual.sort();
        assert_eq!(
            actual, expected,
            "case {filter_type}={value}: expected {expected:?}, got {actual:?}"
        );
    }

    // The range filters: each published bound, and one past it.
    for (filter_type, value, expect_refusal) in [
        ("MIN_AVERAGE_CUSTOMER_RATING", "0.0", false),
        ("MIN_AVERAGE_CUSTOMER_RATING", "5.0", false),
        ("MIN_AVERAGE_CUSTOMER_RATING", "4.5", false),
        ("MIN_AVERAGE_CUSTOMER_RATING", "5.1", true),
        ("MAX_AVERAGE_CUSTOMER_RATING", "0.0", false),
        ("MAX_AVERAGE_CUSTOMER_RATING", "5.0", false),
        ("MAX_AVERAGE_CUSTOMER_RATING", "-0.1", true),
        ("MAX_AVERAGE_CUSTOMER_RATING", "not-a-number", true),
    ] {
        let input = json!({
            "filters": [{ "filterType": filter_type, "filterValues": [value] }],
            "maxResults": 100,
        });
        let (status, body) = call(&router, "searchListings", &input).await;
        if expect_refusal {
            assert_eq!(
                status, 400,
                "case {filter_type}={value}: a value outside 0.0-5.0 must be refused, got {status} with {body}"
            );
            continue;
        }
        assert_eq!(
            status, 200,
            "case {filter_type}={value}: expected HTTP 200, got {status} with {body}"
        );
        let bound: f64 = value.parse().expect("a numeric bound");
        for summary in array(&body, "listingSummaries") {
            let rating = average_rating(&summary);
            let ordered = if filter_type == "MIN_AVERAGE_CUSTOMER_RATING" {
                rating >= bound
            } else {
                rating <= bound
            };
            assert!(
                ordered,
                "case {filter_type}={value}: listing {} rates {rating}, outside the filter",
                text(&summary, "listingId")
            );
        }
    }

    // A range filter takes a single value.
    let (status, body) = call(
        &router,
        "searchListings",
        &json!({
            "filters": [{
                "filterType": "MIN_AVERAGE_CUSTOMER_RATING",
                "filterValues": ["1.0", "2.0"],
            }],
            "maxResults": 100,
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case MIN_AVERAGE_CUSTOMER_RATING with two values: must be refused, got {status} with {body}"
    );
}

#[tokio::test]
async fn the_locale_a_response_reports_is_the_locale_of_its_content() {
    let router = router();

    // The listing the catalog has translations for, and one it does not: discovered from
    // state rather than assumed, by asking for each supported locale.
    let catalog = whole_catalog(&router).await;
    let listing_ids = ids(&catalog, "listingId");
    let mut translated: Option<(String, String)> = None;
    let mut untranslated: Option<String> = None;
    for listing_id in &listing_ids {
        let default = ok(&router, "getListing", &json!({ "listingId": listing_id })).await;
        let mut has_translation = false;
        for locale in ["fr", "es", "ko", "ja"] {
            let body = ok(
                &router,
                "getListing",
                &json!({ "listingId": listing_id, "locale": locale }),
            )
            .await;
            let reported = text(&body, "locale");
            if reported == locale {
                has_translation = true;
                assert_ne!(
                    text(&body, "listingName"),
                    text(&default, "listingName"),
                    "case getListing {listing_id} locale={locale}: a response reporting {locale} must carry {locale} content"
                );
                if translated.is_none() {
                    translated = Some((listing_id.clone(), locale.to_string()));
                }
            } else {
                assert_eq!(
                    reported, "en-US",
                    "case getListing {listing_id} locale={locale}: with no translation the response must report the default locale"
                );
                assert_eq!(
                    text(&body, "listingName"),
                    text(&default, "listingName"),
                    "case getListing {listing_id} locale={locale}: a response reporting en-US must carry en-US content"
                );
            }
        }
        if !has_translation && untranslated.is_none() {
            untranslated = Some(listing_id.clone());
        }
    }
    let (translated_id, translated_locale) =
        translated.expect("case setup: the catalog must hold at least one translated listing");
    let untranslated_id =
        untranslated.expect("case setup: the catalog must hold at least one untranslated listing");

    // Rows: absent locale, the `en` alias, a translated locale, an unsupported tag with a
    // supported fallback, and a case-insensitive tag.
    let cases: [(Option<&str>, &str, &str); 6] = [
        (None, &translated_id, "en-US"),
        (Some("en"), &translated_id, "en-US"),
        (Some("en-US"), &translated_id, "en-US"),
        (Some(&translated_locale), &translated_id, &translated_locale),
        (Some(&translated_locale), &untranslated_id, "en-US"),
        (Some("EN-us"), &translated_id, "en-US"),
    ];
    for (locale, listing_id, expected) in cases {
        let mut input = json!({ "listingId": listing_id });
        if let Some(locale) = locale {
            input["locale"] = json!(locale);
        }
        let body = ok(&router, "getListing", &input).await;
        assert_eq!(
            text(&body, "locale"),
            expected,
            "case getListing {listing_id} locale={locale:?}: expected the response to report {expected}, got {body}"
        );
    }

    // An unsupported preferred tag falls back to a supported second tag, and is refused
    // when there is none.
    let fallback = ok(
        &router,
        "getListing",
        &json!({ "listingId": &translated_id, "locale": "xx, en-US" }),
    )
    .await;
    assert_eq!(
        text(&fallback, "locale"),
        "en-US",
        "case locale='xx, en-US': an unsupported tag with the default as fallback returns the default locale, got {fallback}"
    );
    for operation in [
        "getListing",
        "getProduct",
        "getOffer",
        "getOfferTerms",
        "searchListings",
        "listPurchaseOptions",
    ] {
        let mut input = json!({ "locale": "xx" });
        match operation {
            "getListing" => input["listingId"] = json!(&translated_id),
            "getProduct" => {
                input["productId"] = json!(a_product_id(&router).await);
            }
            "getOffer" | "getOfferTerms" => {
                input["offerId"] = json!(an_offer_id(&router).await);
            }
            _ => {}
        }
        let (status, body) = call(&router, operation, &input).await;
        assert_eq!(
            status, 400,
            "case {operation} locale='xx': an unsupported locale with no fallback must be refused, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn an_id_the_catalog_does_not_hold_is_not_found() {
    let router = router();
    let product_id = a_product_id(&router).await;
    let offer_id = an_offer_id(&router).await;
    let listing_id = ids(&whole_catalog(&router).await, "listingId")
        .first()
        .cloned()
        .expect("a seeded listing");

    // Rows: every by-id operation, with an id that is absent, one that belongs to another
    // kind of resource, and the one that does exist.
    let cases: [(&str, &str, &str); 4] = [
        ("getListing", "listingId", &listing_id),
        ("getProduct", "productId", &product_id),
        ("getOffer", "offerId", &offer_id),
        ("getOfferTerms", "offerId", &offer_id),
    ];
    for (operation, member, known) in cases {
        let found = ok(&router, operation, &json!({ member: known })).await;
        assert!(
            found.is_object(),
            "case {operation} {member}={known}: expected a body, got {found}"
        );

        for unknown in ["does-not-exist", "l-0001-analytics-but-wrong-kind", known] {
            // The third row is the known id with a suffix, so it differs from a real id
            // only in the last character.
            let unknown = if unknown == known {
                format!("{known}-x")
            } else {
                unknown.to_string()
            };
            let (status, body) = call(&router, operation, &json!({ member: unknown })).await;
            assert_eq!(
                status, 404,
                "case {operation} {member}={unknown}: expected HTTP 404, got {status} with {body}"
            );
            assert_eq!(
                text(&body, "__type"),
                "ResourceNotFoundException",
                "case {operation} {member}={unknown}: expected ResourceNotFoundException, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn every_modelled_operation_answers_from_the_catalog_rather_than_the_scaffold() {
    let router = router();
    // One well-formed request per operation the service models (`OPERATIONS.md`). The ids
    // are well formed but need not exist: what no operation may answer is the scaffold's
    // 501 "not written yet", whether the catalog holds the record (200) or not (404).
    let cases: [(&str, Value); 9] = [
        ("searchListings", json!({ "maxResults": 1 })),
        ("searchFacets", json!({})),
        ("getListing", json!({ "listingId": "l-0001-analytics" })),
        ("getProduct", json!({ "productId": "p-0001-analytics-core" })),
        (
            "listFulfillmentOptions",
            json!({ "productId": "p-0001-analytics-core" }),
        ),
        ("listPurchaseOptions", json!({})),
        ("getOffer", json!({ "offerId": "o-0001-analytics-annual" })),
        (
            "getOfferTerms",
            json!({ "offerId": "o-0001-analytics-annual" }),
        ),
        ("getOfferSet", json!({ "offerSetId": "os-0001-analytics" })),
    ];
    for (operation, input) in cases {
        let (status, body) = call(&router, operation, &input).await;
        assert_ne!(
            status, 501,
            "case {operation}: every operation is implemented, got {status} with {body}"
        );
        assert_eq!(
            status, 200,
            "case {operation}: the request names seeded records, got {status} with {body}"
        );
    }
}

/// A product id the catalog holds, read back out of a listing's associated entities.
async fn a_product_id(router: &MultiServiceRouter) -> String {
    for listing_id in ids(&whole_catalog(router).await, "listingId") {
        let listing = ok(router, "getListing", &json!({ "listingId": listing_id })).await;
        if let Some(product_id) = array(&listing, "associatedEntities")
            .iter()
            .filter_map(|entity| entity.get("product").cloned())
            .map(|product| text(&product, "productId"))
            .next()
        {
            return product_id;
        }
    }
    panic!("case setup: the catalog must hold a product");
}

/// An offer id the catalog holds, read back out of a purchase option.
async fn an_offer_id(router: &MultiServiceRouter) -> String {
    let body = ok(router, "listPurchaseOptions", &json!({ "maxResults": 100 })).await;
    for row in array(&body, "purchaseOptions") {
        if let Some(offer_id) = array(&row, "associatedEntities")
            .iter()
            .filter_map(|entity| entity.get("offer").cloned())
            .map(|offer| text(&offer, "offerId"))
            .next()
        {
            return offer_id;
        }
    }
    panic!("case setup: the catalog must hold an offer");
}
