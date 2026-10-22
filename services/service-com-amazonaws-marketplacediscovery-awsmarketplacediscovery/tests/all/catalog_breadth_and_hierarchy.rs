// SPDX-License-Identifier: CC0-1.0

//! What the seeded AWS Marketplace Discovery catalog has to make reachable, and the rules
//! the read paths follow over it.
//!
//! The rules under test:
//!
//! * the catalog's categories form a TREE, a record is classified under a category's whole
//!   path, and a `CATEGORY` facet value reports the value it is nested under
//!   (`ListingFacet$parent`: "The parent facet value for hierarchical facets, such as
//!   subcategories") — `None` only at the top of the tree;
//! * `SearchListings` returns a `nextToken` exactly while more matches remain, at every
//!   `maxResults` including the model's default of 25 — so the catalog has to hold more
//!   listings than one default-sized page;
//! * every `FulfillmentOptionType` the model declares is reachable from some product, and
//!   the option a product reports is the `FulfillmentOption` union member of its own type;
//! * a listing's badges are the special attributes its own records have
//!   (`GetListingOutput$badges`: "Badges indicating special attributes of the listing, such
//!   as free tier eligibility, free trial availability, or Quick Launch support");
//! * a purchase option is returned by a `PRODUCT_ID` + `SELLER_OF_RECORD_PROFILE_ID`
//!   filter naming its own product and seller of record.
//!
//! Every suite drives real HTTP requests through the `MultiServiceRouter`, so the protocol,
//! the routing table, input validation and the handlers are all exercised.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_marketplacediscovery_awsmarketplacediscovery::ops::types::FULFILLMENT_OPTION_TYPE_VALUES;
use service_com_amazonaws_marketplacediscovery_awsmarketplacediscovery::state::{
    category_parent, CATEGORY_TAXONOMY,
};
use service_com_amazonaws_marketplacediscovery_awsmarketplacediscovery::EmulatedService;

/// The model's `POST /2026-02-05/<op>` route prefix (`OPERATIONS.md`).
const PREFIX: &str = "http://discovery-marketplace.us-east-1.amazonaws.com/2026-02-05";

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name, which is what the router's `can_handle` routes on.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260205/us-east-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// `SearchListingsInput$maxResults` defaults to 25
/// (`smithy_query(query="operation",operation="SearchListings")`), so a request that names
/// no `maxResults` is paged at this size.
const DEFAULT_MAX_RESULTS: usize = 25;

/// Subcategory/parent pairs of the catalog's taxonomy that listings sit at, each checked
/// against what [`CATEGORY_TAXONOMY`] says and against what the service reports.
const NESTED_CATEGORIES: &[(&str, &str)] = &[
    ("storage", "infrastructure-software"),
    ("network-infrastructure", "infrastructure-software"),
    ("iot-analytics", "iot"),
    ("financial-services-data", "data-products"),
    ("crm", "business-applications"),
    ("training", "professional-services"),
];

fn router() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The suite signs nothing, so request authentication is off for this router only.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation and return its HTTP status and JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, input: &Value) -> (u16, Value) {
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
async fn ok(router: &MultiServiceRouter, operation: &str, input: &Value) -> Value {
    let (status, body) = call(router, operation, input).await;
    assert_eq!(
        status, 200,
        "case {operation} {input}: expected HTTP 200, got {status} with body {body}"
    );
    body
}

fn array(body: &Value, member: &str) -> Vec<Value> {
    body.get(member)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn text(value: &Value, member: &str) -> String {
    value
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn total_results(body: &Value) -> usize {
    usize::try_from(body.get("totalResults").and_then(Value::as_i64).unwrap_or(-1))
        .unwrap_or_default()
}

/// The `CATEGORY` facet values of a query, keyed by value.
async fn category_facets(router: &MultiServiceRouter, query: &Value) -> BTreeMap<String, Value> {
    let mut input = query.clone();
    input["facetTypes"] = json!(["CATEGORY"]);
    let body = ok(router, "searchFacets", &input).await;
    body.get("listingFacets")
        .and_then(Value::as_object)
        .and_then(|map| map.get("CATEGORY"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|facet| (text(&facet, "value"), facet))
        .collect()
}

/// The listings a `CATEGORY` filter selects.
async fn listings_in_category(router: &MultiServiceRouter, category_id: &str) -> Vec<Value> {
    let body = ok(
        router,
        "searchListings",
        &json!({
            "maxResults": 100,
            "filters": [{ "filterType": "CATEGORY", "filterValues": [category_id] }],
        }),
    )
    .await;
    array(&body, "listingSummaries")
}

/// Every purchase option the catalog holds, paged to exhaustion.
async fn every_purchase_option(router: &MultiServiceRouter) -> Vec<Value> {
    let mut rows: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut input = json!({ "maxResults": 100 });
        if let Some(token) = &token {
            input["nextToken"] = json!(token);
        }
        let body = ok(router, "listPurchaseOptions", &input).await;
        rows.extend(array(&body, "purchaseOptions"));
        match body.get("nextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
        assert!(
            rows.len() < 10_000,
            "case listPurchaseOptions: paging must terminate, collected {}",
            rows.len()
        );
    }
    rows
}

/// The catalog's taxonomy is a tree: every id is declared once, every parent is itself a
/// declared category, no category is its own ancestor, and every row has a display name.
#[test]
fn the_category_taxonomy_is_a_tree() {
    let ids: BTreeSet<&str> = CATEGORY_TAXONOMY.iter().map(|(id, _, _)| *id).collect();
    assert_eq!(
        ids.len(),
        CATEGORY_TAXONOMY.len(),
        "case CATEGORY_TAXONOMY: every categoryId must be declared once, got {} ids for {} rows",
        ids.len(),
        CATEGORY_TAXONOMY.len()
    );
    let mut roots = 0_usize;
    let mut children = 0_usize;
    for (id, display_name, parent) in CATEGORY_TAXONOMY {
        assert!(
            !display_name.trim().is_empty(),
            "case {id}: Category$displayName is required and non-blank, got {display_name:?}"
        );
        match parent {
            None => roots += 1,
            Some(parent) => {
                children += 1;
                assert!(
                    ids.contains(parent),
                    "case {id}: its parent {parent} must be a declared category, declared are {ids:?}"
                );
                // Walking up from a child reaches a root, so no row is its own ancestor.
                let mut cursor = Some(*parent);
                let mut depth = 0_usize;
                while let Some(current) = cursor {
                    assert_ne!(
                        current, *id,
                        "case {id}: a category must not be its own ancestor"
                    );
                    depth += 1;
                    assert!(
                        depth <= CATEGORY_TAXONOMY.len(),
                        "case {id}: walking up the taxonomy must terminate, got depth {depth}"
                    );
                    cursor = category_parent(current);
                }
            }
        }
    }
    assert!(
        roots > 0 && children > 0,
        "case CATEGORY_TAXONOMY: the taxonomy must have both top-level categories and \
         subcategories, got {roots} roots and {children} children"
    );
    for (child, parent) in NESTED_CATEGORIES {
        assert_eq!(
            category_parent(child),
            Some(*parent),
            "case {child}: expected parent {parent}, got {:?}",
            category_parent(child)
        );
    }
    assert_eq!(
        category_parent("no-such-category"),
        None,
        "case no-such-category: an id the taxonomy does not hold has no parent, got {:?}",
        category_parent("no-such-category")
    );
}

/// Every `CATEGORY` facet value reports the category it is nested under, and nothing when it
/// is top-level — over queries that match the whole catalog, a slice of it and nothing.
#[tokio::test]
async fn category_facet_values_report_their_taxonomy_parent() {
    let router = router();

    // Rows: no criteria, a text query, a filtered query, and a query nothing matches.
    let queries: [(&str, Value); 4] = [
        ("whole catalog", json!({})),
        ("searchText=data", json!({ "searchText": "data" })),
        (
            "CATEGORY=infrastructure-software",
            json!({ "filters": [{ "filterType": "CATEGORY", "filterValues": ["infrastructure-software"] }] }),
        ),
        (
            "searchText=zzz-not-in-the-catalog",
            json!({ "searchText": "zzz-not-in-the-catalog" }),
        ),
    ];
    let mut with_parent = 0_usize;
    let mut without_parent = 0_usize;
    for (case, query) in queries {
        for (value, facet) in category_facets(&router, &query).await {
            let expected = category_parent(&value).map(str::to_string);
            let actual = facet
                .get("parent")
                .filter(|parent| !parent.is_null())
                .and_then(Value::as_str)
                .map(str::to_string);
            assert_eq!(
                actual, expected,
                "case {case} / CATEGORY={value}: expected parent {expected:?}, got {actual:?} in {facet}"
            );
            if expected.is_some() {
                with_parent += 1;
            } else {
                without_parent += 1;
            }
        }
    }
    assert!(
        with_parent > 0 && without_parent > 0,
        "case queries: the catalog must report both nested and top-level category facets, got \
         {with_parent} with a parent and {without_parent} without"
    );

    // Each nested pair is reported as a facet value whose parent is the one the taxonomy
    // declares, and the parent is itself a facet value of the same response — so a client
    // can refine from the parent to the child with the same CATEGORY filter.
    let facets = category_facets(&router, &json!({})).await;
    for (child, parent) in NESTED_CATEGORIES {
        let facet = facets.get(*child).unwrap_or_else(|| {
            panic!(
                "case CATEGORY={child}: the catalog must hold a listing in it, got {:?}",
                facets.keys().collect::<Vec<_>>()
            )
        });
        assert_eq!(
            text(facet, "parent"),
            *parent,
            "case CATEGORY={child}: expected parent {parent}, got {facet}"
        );
        let parent_facet = facets.get(*parent).unwrap_or_else(|| {
            panic!("case CATEGORY={parent}: the parent must be a facet value too, got {facets:?}")
        });
        let child_count = facet.get("count").and_then(Value::as_i64).unwrap_or(-1);
        let parent_count = parent_facet.get("count").and_then(Value::as_i64).unwrap_or(-1);
        assert!(
            parent_count >= child_count && child_count > 0,
            "case CATEGORY={child}: a parent counts at least its children's listings, got \
             {parent_count} against {child_count}"
        );
    }
}

/// A listing classified under a subcategory carries that subcategory's whole path, so a
/// client browsing the parent finds it — asserted through `GetListing`, the detail read.
#[tokio::test]
async fn a_listing_is_classified_under_its_whole_taxonomy_path() {
    let router = router();
    for (child, parent) in NESTED_CATEGORIES {
        let summaries = listings_in_category(&router, child).await;
        assert!(
            !summaries.is_empty(),
            "case CATEGORY={child}: the catalog must hold a listing in it"
        );
        for summary in &summaries {
            let listing_id = text(summary, "listingId");
            let listing = ok(
                &router,
                "getListing",
                &json!({ "listingId": listing_id.clone() }),
            )
            .await;
            let categories: Vec<String> = array(&listing, "categories")
                .iter()
                .map(|category| text(category, "categoryId"))
                .collect();
            for required in [*child, *parent] {
                assert!(
                    categories.contains(&required.to_string()),
                    "case getListing {listing_id} in {child}: categories must carry {required}, \
                     got {categories:?}"
                );
            }
            // The parent comes before the child: the path runs outermost first.
            let parent_at = categories.iter().position(|id| id == parent);
            let child_at = categories.iter().position(|id| id == child);
            assert!(
                parent_at < child_at,
                "case getListing {listing_id}: expected {parent} before {child}, got {categories:?}"
            );
            // Browsing the parent finds the listing the child selected.
            let in_parent = listings_in_category(&router, parent).await;
            assert!(
                in_parent
                    .iter()
                    .any(|found| text(found, "listingId") == listing_id),
                "case CATEGORY={parent}: expected to find {listing_id}, got {:?}",
                in_parent
                    .iter()
                    .map(|found| text(found, "listingId"))
                    .collect::<Vec<_>>()
            );
        }
    }
}

/// `SearchListings` returns a `nextToken` exactly while more matches remain — at every page
/// size, including the request that names no `maxResults` at all and is paged at the model's
/// default of 25, which the catalog must be large enough to truncate.
#[tokio::test]
async fn search_listings_mints_a_next_token_exactly_while_more_matches_remain() {
    let router = router();

    // Rows: `maxResults` absent (the model's default), the smallest page, sizes either side
    // of the default, and the largest page the model allows.
    let cases: [(&str, Option<usize>); 7] = [
        ("maxResults absent", None),
        ("maxResults=1", Some(1)),
        ("maxResults=2", Some(2)),
        ("maxResults=10", Some(10)),
        ("maxResults=25", Some(DEFAULT_MAX_RESULTS)),
        ("maxResults=26", Some(DEFAULT_MAX_RESULTS + 1)),
        ("maxResults=100", Some(100)),
    ];
    let mut truncated_cases = 0_usize;
    for (case, max_results) in cases {
        let mut input = json!({});
        if let Some(max_results) = max_results {
            input["maxResults"] = json!(max_results);
        }
        let body = ok(&router, "searchListings", &input).await;
        let page = array(&body, "listingSummaries");
        let total = total_results(&body);
        let page_size = max_results.unwrap_or(DEFAULT_MAX_RESULTS);
        assert_eq!(
            page.len(),
            page_size.min(total),
            "case {case}: a page holds min(maxResults, totalResults) items, got {}",
            page.len()
        );
        let token = body
            .get("nextToken")
            .filter(|token| !token.is_null())
            .map(|token| token.to_string());
        assert_eq!(
            token.is_some(),
            page.len() < total,
            "case {case}: a token must be present exactly while more of the {total} matches \
             remain, got {token:?} after {} items",
            page.len()
        );
        if token.is_some() {
            truncated_cases += 1;
        }
    }
    assert!(
        truncated_cases >= 5,
        "case maxResults rows: the catalog must exceed the default page size, so every page \
         size below the match count truncates, got only {truncated_cases} truncated pages"
    );

    // Following the token visits every match exactly once, in the result set's order.
    let whole = ok(&router, "searchListings", &json!({ "maxResults": 100 })).await;
    let expected: Vec<String> = array(&whole, "listingSummaries")
        .iter()
        .map(|summary| text(summary, "listingId"))
        .collect();
    for page_size in [1_usize, 7, DEFAULT_MAX_RESULTS] {
        let mut collected: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut input = json!({ "maxResults": page_size });
            if let Some(token) = &token {
                input["nextToken"] = json!(token);
            }
            let body = ok(&router, "searchListings", &input).await;
            collected.extend(
                array(&body, "listingSummaries")
                    .iter()
                    .map(|summary| text(summary, "listingId")),
            );
            match body.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
            assert!(
                collected.len() <= expected.len(),
                "case maxResults={page_size}: paging must terminate, collected {collected:?}"
            );
        }
        assert_eq!(
            collected, expected,
            "case maxResults={page_size}: paging must yield every match once, in order"
        );
    }
}

/// Every `FulfillmentOptionType` the model declares is reachable from a product in the
/// catalog, and the option that product reports is the `FulfillmentOption` union member of
/// that very type — so a client that follows one fulfillment path finds its own variant.
#[tokio::test]
async fn every_modelled_fulfillment_option_type_is_reachable_as_its_own_union_member() {
    let router = router();

    // Rows: every type of the model's `FulfillmentOptionType` enum.
    for option_type in FULFILLMENT_OPTION_TYPE_VALUES {
        let body = ok(
            &router,
            "searchListings",
            &json!({
                "maxResults": 100,
                "filters": [{
                    "filterType": "FULFILLMENT_OPTION_TYPE",
                    "filterValues": [option_type],
                }],
            }),
        )
        .await;
        let summaries = array(&body, "listingSummaries");
        assert!(
            !summaries.is_empty(),
            "case FULFILLMENT_OPTION_TYPE={option_type}: the catalog must hold a listing \
             fulfilled that way, got {body}"
        );
        let summary = &summaries[0];
        let listing_id = text(summary, "listingId");
        let types: Vec<String> = array(summary, "fulfillmentOptionSummaries")
            .iter()
            .map(|option| text(option, "fulfillmentOptionType"))
            .collect();
        assert!(
            types.contains(&(*option_type).to_string()),
            "case {listing_id}: a listing the filter selected must summarise {option_type}, got \
             {types:?}"
        );

        // The product behind the summary reports the same type, as the union member of that
        // type.
        let listing = ok(
            &router,
            "getListing",
            &json!({ "listingId": listing_id.clone() }),
        )
        .await;
        let mut found = false;
        for entity in array(&listing, "associatedEntities") {
            let Some(product) = entity.get("product") else {
                continue;
            };
            let product_id = text(product, "productId");
            let options = ok(
                &router,
                "listFulfillmentOptions",
                &json!({ "productId": product_id.clone(), "maxResults": 50 }),
            )
            .await;
            for option in array(&options, "fulfillmentOptions") {
                let object = option.as_object().unwrap_or_else(|| {
                    panic!("case {product_id}: a fulfillmentOptions element is an object, got {option}")
                });
                assert_eq!(
                    object.len(),
                    1,
                    "case {product_id}: a FulfillmentOption carries one union member, got {option}"
                );
                let (member_name, member) = object.iter().next().expect("one union member");
                if text(member, "fulfillmentOptionType") == *option_type {
                    found = true;
                    assert!(
                        !text(member, "fulfillmentOptionId").is_empty(),
                        "case {product_id} / {member_name}: fulfillmentOptionId is required, got \
                         {member}"
                    );
                }
            }
        }
        assert!(
            found,
            "case FULFILLMENT_OPTION_TYPE={option_type}: a product of {listing_id} must report an \
             option of that type"
        );
    }
}

/// A listing's badges state attributes of its OWN records: each badge the service reports is
/// one the listing's products, offers and pricing models back, and every attribute they back
/// is reported.
#[tokio::test]
async fn a_listings_badges_are_the_attributes_its_own_records_have() {
    let router = router();
    let catalog = ok(&router, "searchListings", &json!({ "maxResults": 100 })).await;

    // Rows: every listing in the catalog, which differ in product count, deployment status,
    // fulfillment type, pricing models and whether an offer carries a free trial.
    let mut reported: BTreeSet<String> = BTreeSet::new();
    for summary in array(&catalog, "listingSummaries") {
        let listing_id = text(&summary, "listingId");
        let listing = ok(
            &router,
            "getListing",
            &json!({ "listingId": listing_id.clone() }),
        )
        .await;

        // What the listing's own records say.
        let products: Vec<String> = array(&listing, "associatedEntities")
            .iter()
            .filter_map(|entity| entity.get("product").cloned())
            .map(|product| text(&product, "productId"))
            .collect();
        let mut deployed = false;
        let mut quick_launch = false;
        for product_id in &products {
            let product = ok(
                &router,
                "getProduct",
                &json!({ "productId": product_id.clone() }),
            )
            .await;
            deployed |= text(&product, "deployedOnAws") == "DEPLOYED";
            let options = ok(
                &router,
                "listFulfillmentOptions",
                &json!({ "productId": product_id.clone(), "maxResults": 50 }),
            )
            .await;
            for option in array(&options, "fulfillmentOptions") {
                let Some((_, member)) = option.as_object().and_then(|object| {
                    object
                        .iter()
                        .next()
                        .map(|(name, value)| (name.clone(), value.clone()))
                }) else {
                    continue;
                };
                quick_launch |= text(&member, "quickLaunch") == "ENABLED";
            }
        }
        let free_tier = array(&listing, "pricingModels")
            .iter()
            .any(|model| text(model, "pricingModelType") == "FREE");
        let mut free_trial = false;
        for entity in array(&listing, "associatedEntities") {
            let Some(offer) = entity.get("offer") else {
                continue;
            };
            let offer_id = text(offer, "offerId");
            let terms = ok(
                &router,
                "getOfferTerms",
                &json!({ "offerId": offer_id, "maxResults": 25 }),
            )
            .await;
            free_trial |= array(&terms, "offerTerms").iter().any(|term| {
                term.as_object()
                    .is_some_and(|object| object.contains_key("freeTrialPricingTerm"))
            });
        }

        let expected: BTreeSet<String> = [
            ("AWS_FREE_TIER", free_tier),
            ("DEPLOYED_ON_AWS", deployed),
            ("FREE_TRIAL", free_trial),
            ("MULTI_PRODUCT", products.len() > 1),
            ("QUICK_LAUNCH", quick_launch),
        ]
        .iter()
        .filter(|(_, holds)| *holds)
        .map(|(badge_type, _)| (*badge_type).to_string())
        .collect();

        // The detail read and the search summary report the same badges.
        for (source, body) in [("getListing", &listing), ("searchListings", &summary)] {
            let actual: BTreeSet<String> = array(body, "badges")
                .iter()
                .map(|badge| text(badge, "badgeType"))
                .collect();
            assert_eq!(
                actual, expected,
                "case {source} {listing_id}: expected badges {expected:?}, got {actual:?}"
            );
            for badge in array(body, "badges") {
                assert!(
                    !text(&badge, "displayName").trim().is_empty(),
                    "case {source} {listing_id}: ListingBadge$displayName is required and \
                     non-blank, got {badge}"
                );
            }
        }
        reported.extend(expected);
    }

    // The catalog exercises every badge type the model declares, so none of the rules above
    // is asserted only in the negative.
    assert_eq!(
        reported,
        [
            "AWS_FREE_TIER",
            "DEPLOYED_ON_AWS",
            "FREE_TRIAL",
            "MULTI_PRODUCT",
            "QUICK_LAUNCH",
        ]
        .iter()
        .map(|badge| (*badge).to_string())
        .collect::<BTreeSet<String>>(),
        "case whole catalog: every modelled listing badge must be reachable, got {reported:?}"
    );
}

/// A purchase option is returned by a filter naming its own product and its own seller of
/// record — for every purchase option the catalog holds, of either purchase option type.
#[tokio::test]
async fn a_purchase_option_is_found_by_a_filter_on_its_product_and_seller_of_record() {
    let router = router();
    let rows = every_purchase_option(&router).await;
    assert!(
        rows.len() > 2,
        "case setup: the catalog must hold several purchase options, got {rows:?}"
    );

    // Rows: every purchase option in the catalog, which differ in type (OFFER and
    // OFFERSET), in seller of record, and in how many products they cover.
    let mut types: BTreeSet<String> = BTreeSet::new();
    for row in &rows {
        let purchase_option_id = text(row, "purchaseOptionId");
        types.insert(text(row, "purchaseOptionType"));
        let seller = row
            .get("sellerOfRecord")
            .map(|seller| text(seller, "sellerProfileId"))
            .unwrap_or_default();
        let products: Vec<String> = array(row, "associatedEntities")
            .iter()
            .filter_map(|entity| entity.get("product").cloned())
            .map(|product| text(&product, "productId"))
            .collect();
        assert!(
            !seller.is_empty() && !products.is_empty(),
            "case {purchase_option_id}: a purchase option names its seller of record and its \
             products, got {row}"
        );

        for product_id in &products {
            let case = format!("PRODUCT_ID={product_id} SELLER_OF_RECORD_PROFILE_ID={seller}");
            let body = ok(
                &router,
                "listPurchaseOptions",
                &json!({
                    "maxResults": 100,
                    "filters": [
                        { "filterType": "PRODUCT_ID", "filterValues": [product_id] },
                        { "filterType": "SELLER_OF_RECORD_PROFILE_ID", "filterValues": [seller] },
                    ],
                }),
            )
            .await;
            let matched = array(&body, "purchaseOptions");
            assert!(
                matched
                    .iter()
                    .any(|found| text(found, "purchaseOptionId") == purchase_option_id),
                "case {case}: expected {purchase_option_id} among the results, got {:?}",
                matched
                    .iter()
                    .map(|found| text(found, "purchaseOptionId"))
                    .collect::<Vec<_>>()
            );
            // Every row the filter returns satisfies BOTH filters (AND across filters).
            for found in &matched {
                let found_id = text(found, "purchaseOptionId");
                let found_products: Vec<String> = array(found, "associatedEntities")
                    .iter()
                    .filter_map(|entity| entity.get("product").cloned())
                    .map(|product| text(&product, "productId"))
                    .collect();
                assert!(
                    found_products.contains(product_id),
                    "case {case}: row {found_id} must cover the filtered product, got \
                     {found_products:?}"
                );
                assert_eq!(
                    found
                        .get("sellerOfRecord")
                        .map(|seller| text(seller, "sellerProfileId"))
                        .unwrap_or_default(),
                    seller,
                    "case {case}: row {found_id} must be sold by the filtered seller"
                );
            }
        }
    }
    assert_eq!(
        types,
        ["OFFER", "OFFERSET"]
            .iter()
            .map(|kind| (*kind).to_string())
            .collect::<BTreeSet<String>>(),
        "case setup: the catalog must hold both purchase option types, got {types:?}"
    );

    // A product the catalog does not hold matches nothing, and is an empty list rather than
    // an error.
    let empty = ok(
        &router,
        "listPurchaseOptions",
        &json!({
            "maxResults": 100,
            "filters": [
                { "filterType": "PRODUCT_ID", "filterValues": ["p-no-such-product"] },
                { "filterType": "SELLER_OF_RECORD_PROFILE_ID", "filterValues": ["sp-no-such-seller"] },
            ],
        }),
    )
    .await;
    assert!(
        array(&empty, "purchaseOptions").is_empty(),
        "case PRODUCT_ID=p-no-such-product: expected an empty list, got {empty}"
    );
}
