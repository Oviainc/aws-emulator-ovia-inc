// SPDX-License-Identifier: CC0-1.0

//! What the TYPED members of a catalog record say, and the rules that decide them.
//!
//! A record's prose, logos and ids are the catalog's own, but its typed members are not free
//! choices: each is decided by another record of the same catalog, or by the modelled domain
//! the catalog has to span. The rules under test:
//!
//! * `Product$deployedOnAws` ("Indicates whether the product is deployed on AWS
//!   infrastructure") is decided by HOW the product is fulfilled, so every product reports
//!   the status its own fulfillment options imply and no product contradicts them; the
//!   `DEPLOYED_ON_AWS` filter selects listings by that same status.
//! * A record spans the modelled kinds of `Resource`: `ResourceType` declares two kinds and
//!   `ResourceContentType` four formats ("The format of the resource content, such as a URL,
//!   email address, or text"), and each resource's value is in the format it declares.
//! * A record classified under a category is classified under that category's whole
//!   taxonomy PATH, at every depth the taxonomy nests, outermost ancestor first — and a
//!   record may sit at more than one category.
//! * `ListingFacet$parent` ("The parent facet value for hierarchical facets, such as
//!   subcategories") lets a client refine through EVERY level of that tree: a facet value's
//!   parent is itself a facet value, which may have a parent of its own, and filtering on an
//!   ancestor selects every listing its descendants select.
//! * A facet list is ordered by descending count, so the most common value of a facet type
//!   is the first a refine-search UI shows.
//!
//! The last two rules are asserted of LISTINGS alone by `catalog_breadth_and_hierarchy`
//! (`a_listing_is_classified_under_its_whole_taxonomy_path`,
//! `category_facet_values_report_their_taxonomy_parent`); this suite extends them to every
//! PRODUCT as well, and to what a two-level-deep taxonomy adds: a path three categories
//! long, a record classified under more than one category, and a facet value whose parent
//! has a parent of its own.
//!
//! Every suite drives real HTTP requests through the `MultiServiceRouter` (the harness this
//! crate's `catalog_drilldown` suite owns), so the protocol, the routing table, input
//! validation and the handlers are all exercised.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_marketplacediscovery_awsmarketplacediscovery::ops::types::{
    DEPLOYED_ON_AWS_STATUS_VALUES, FULFILLMENT_OPTION_TYPE_VALUES, RESOURCE_CONTENT_TYPE_VALUES,
    RESOURCE_TYPE_VALUES, SEARCH_FACET_TYPE_VALUES,
};
use service_com_amazonaws_marketplacediscovery_awsmarketplacediscovery::state::{
    category_parent, FULFILLMENT_DEPLOYMENT_STATUS,
};

// The router, the `POST /2026-02-05/<op>` dispatch and the JSON readers are this crate's
// one test harness, owned by the `catalog_drilldown` suite.
use crate::catalog_drilldown::{array, ok, router, text};

/// What deployment each `FulfillmentOptionType` implies for the product it fulfills, written
/// out here independently of the service's own table so the two have to agree.
///
/// Source: `GetProductOutput$deployedOnAws` — "Indicates whether the product is deployed on
/// AWS infrastructure" (`smithy_query(query="shape",shape="GetProductOutput")`) — read
/// against what each `FulfillmentOption` union member delivers: an artifact that RUNS on AWS
/// once fulfilled is `DEPLOYED`; software the buyer only accesses, hosted by its seller, is
/// `NOT_DEPLOYED`; and a delivery that is not deployable software at all (a data set, a
/// services engagement) is `NOT_APPLICABLE`. `the_deployment_table_covers_the_model` below
/// checks this table against the model's `FulfillmentOptionType` and `DeployedOnAwsStatus`
/// enums, so it cannot fall behind either.
const DEPLOYMENT_BY_FULFILLMENT: &[(&str, &str)] = &[
    ("AMAZON_MACHINE_IMAGE", "DEPLOYED"),
    ("API", "NOT_DEPLOYED"),
    ("CLOUDFORMATION_TEMPLATE", "DEPLOYED"),
    ("CONTAINER", "DEPLOYED"),
    ("DATA_EXCHANGE", "NOT_APPLICABLE"),
    ("EC2_IMAGE_BUILDER_COMPONENT", "DEPLOYED"),
    ("EKS_ADD_ON", "DEPLOYED"),
    ("HELM", "DEPLOYED"),
    ("PROFESSIONAL_SERVICES", "NOT_APPLICABLE"),
    ("SAAS", "NOT_DEPLOYED"),
    ("SAGEMAKER_ALGORITHM", "DEPLOYED"),
    ("SAGEMAKER_MODEL", "DEPLOYED"),
];

/// The queries the facet rules are checked over: the whole catalog, a text query, a filtered
/// query and a query nothing matches.
fn facet_queries() -> Vec<(&'static str, Value)> {
    vec![
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
    ]
}

/// Every listing the catalog holds, as `GetListing` returns it, keyed by listing id.
///
/// `maxResults` is the model's largest page (100) and the catalog holds fewer listings than
/// that, so one page is the whole catalog; the assertion says so rather than assuming it.
async fn every_listing(router: &MultiServiceRouter) -> BTreeMap<String, Value> {
    let page = ok(router, "searchListings", &json!({ "maxResults": 100 })).await;
    assert!(
        page.get("nextToken").is_none_or(Value::is_null),
        "case setup: the catalog must fit one 100-item page, got {page}"
    );
    let mut listings: BTreeMap<String, Value> = BTreeMap::new();
    for summary in array(&page, "listingSummaries") {
        let listing_id = text(&summary, "listingId");
        let listing = ok(
            router,
            "getListing",
            &json!({ "listingId": listing_id.clone() }),
        )
        .await;
        listings.insert(listing_id, listing);
    }
    assert!(
        listings.len() > 25,
        "case setup: the catalog must hold more than one default page of listings, got {}",
        listings.len()
    );
    listings
}

/// The products a listing associates, as `GetProduct` returns them.
async fn products_of(router: &MultiServiceRouter, listing: &Value) -> Vec<Value> {
    let mut products: Vec<Value> = Vec::new();
    for entity in array(listing, "associatedEntities") {
        let Some(product) = entity.get("product") else {
            continue;
        };
        let product_id = text(product, "productId");
        products.push(ok(router, "getProduct", &json!({ "productId": product_id })).await);
    }
    products
}

/// The `fulfillmentOptionType` of every fulfillment option a product reports, read from the
/// union member each option carries.
async fn fulfillment_types(router: &MultiServiceRouter, product_id: &str) -> Vec<String> {
    let body = ok(
        router,
        "listFulfillmentOptions",
        &json!({ "productId": product_id, "maxResults": 50 }),
    )
    .await;
    array(&body, "fulfillmentOptions")
        .iter()
        .filter_map(|option| {
            option
                .as_object()
                .and_then(|object| object.values().next().cloned())
        })
        .map(|member| text(&member, "fulfillmentOptionType"))
        .collect()
}

/// The deployment status a set of fulfillment option types implies, per
/// [`DEPLOYMENT_BY_FULFILLMENT`]: the strongest claim they support.
fn implied_status(option_types: &[String]) -> &'static str {
    let statuses: Vec<&str> = option_types
        .iter()
        .map(|option_type| {
            DEPLOYMENT_BY_FULFILLMENT
                .iter()
                .find(|(fulfillment, _)| fulfillment == option_type)
                .map_or("NOT_APPLICABLE", |(_, status)| *status)
        })
        .collect();
    if statuses.contains(&"DEPLOYED") {
        "DEPLOYED"
    } else if statuses.contains(&"NOT_DEPLOYED") {
        "NOT_DEPLOYED"
    } else {
        "NOT_APPLICABLE"
    }
}

/// The facet list of one facet type for one query.
async fn facets(router: &MultiServiceRouter, query: &Value, facet_type: &str) -> Vec<Value> {
    let mut input = query.clone();
    input["facetTypes"] = json!([facet_type]);
    let body = ok(router, "searchFacets", &input).await;
    body.get("listingFacets")
        .and_then(Value::as_object)
        .and_then(|map| map.get(facet_type))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The listing ids a `CATEGORY` filter selects.
async fn listings_in_category(router: &MultiServiceRouter, category_id: &str) -> BTreeSet<String> {
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
        .iter()
        .map(|summary| text(summary, "listingId"))
        .collect()
}

/// The deployment table names every `FulfillmentOptionType` the model declares, uses only
/// `DeployedOnAwsStatus` values, and reaches all three of them — so no modelled fulfillment
/// is unclassified and no status is unreachable.
#[test]
fn the_deployment_table_covers_the_model() {
    let classified: BTreeSet<&str> = DEPLOYMENT_BY_FULFILLMENT
        .iter()
        .map(|(fulfillment, _)| *fulfillment)
        .collect();
    let modelled: BTreeSet<&str> = FULFILLMENT_OPTION_TYPE_VALUES.iter().copied().collect();
    assert_eq!(
        classified, modelled,
        "case FulfillmentOptionType: every modelled fulfillment must imply a deployment \
         status, got {classified:?} against {modelled:?}"
    );
    let claimed: BTreeSet<&str> = DEPLOYMENT_BY_FULFILLMENT
        .iter()
        .map(|(_, status)| *status)
        .collect();
    let statuses: BTreeSet<&str> = DEPLOYED_ON_AWS_STATUS_VALUES.iter().copied().collect();
    assert_eq!(
        claimed, statuses,
        "case DeployedOnAwsStatus: the table must use exactly the modelled statuses and reach \
         each of them, got {claimed:?} against {statuses:?}"
    );

    // And the service's own classification agrees with this one, so a fulfillment type added
    // to the model without a classification is caught even where no seeded product uses it.
    let served: BTreeSet<(&str, &str)> = FULFILLMENT_DEPLOYMENT_STATUS.iter().copied().collect();
    let expected: BTreeSet<(&str, &str)> = DEPLOYMENT_BY_FULFILLMENT.iter().copied().collect();
    assert_eq!(
        served, expected,
        "case FULFILLMENT_DEPLOYMENT_STATUS: the service must class every modelled fulfillment \
         the way its delivery implies, got {served:?}"
    );
}

/// Every product reports the deployment status its OWN fulfillment options imply, in both
/// the detail read and the `DEPLOYED_ON_AWS` facet — over every product in the catalog,
/// which differ in fulfillment type and in how many options they carry.
#[tokio::test]
async fn a_products_deployment_status_is_the_one_its_fulfillment_implies() {
    let router = router();
    let listings = every_listing(&router).await;

    let mut reported: BTreeSet<String> = BTreeSet::new();
    let mut products_seen = 0_usize;
    let mut multi_option_products = 0_usize;
    // The status each listing's products report, read once: the filter rows below are
    // checked against this rather than re-reading every product per status.
    let mut statuses_by_listing: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (listing_id, listing) in &listings {
        for product in products_of(&router, listing).await {
            let product_id = text(&product, "productId");
            let option_types = fulfillment_types(&router, &product_id).await;
            assert!(
                !option_types.is_empty(),
                "case {listing_id} / {product_id}: a product must report how it is fulfilled"
            );
            if option_types.len() > 1 {
                multi_option_products += 1;
            }
            let expected = implied_status(&option_types);
            let actual = text(&product, "deployedOnAws");
            assert_eq!(
                actual, expected,
                "case getProduct {product_id} fulfilled as {option_types:?}: expected \
                 deployedOnAws {expected}, got {actual:?}"
            );
            statuses_by_listing
                .entry(listing_id.clone())
                .or_default()
                .insert(actual.clone());
            reported.insert(actual);
            products_seen += 1;
        }
    }
    assert!(
        products_seen >= listings.len() && multi_option_products > 0,
        "case setup: every listing must have a product and some product must carry several \
         fulfillment options, got {products_seen} products ({multi_option_products} with \
         several options) for {} listings",
        listings.len()
    );
    assert_eq!(
        reported,
        DEPLOYED_ON_AWS_STATUS_VALUES
            .iter()
            .map(|status| (*status).to_string())
            .collect::<BTreeSet<String>>(),
        "case whole catalog: every modelled deployment status must be reachable, got \
         {reported:?}"
    );

    // Rows: each modelled status. The filter selects exactly the listings with a product
    // reporting it, so a client filtering on a status gets the records it would read.
    for status in DEPLOYED_ON_AWS_STATUS_VALUES {
        let body = ok(
            &router,
            "searchListings",
            &json!({
                "maxResults": 100,
                "filters": [{ "filterType": "DEPLOYED_ON_AWS", "filterValues": [status] }],
            }),
        )
        .await;
        let selected: BTreeSet<String> = array(&body, "listingSummaries")
            .iter()
            .map(|summary| text(summary, "listingId"))
            .collect();
        let expected: BTreeSet<String> = statuses_by_listing
            .iter()
            .filter(|(_, statuses)| statuses.contains(*status))
            .map(|(listing_id, _)| listing_id.clone())
            .collect();
        assert_eq!(
            selected, expected,
            "case DEPLOYED_ON_AWS={status}: the filter must select the listings whose products \
             report it, got {selected:?}"
        );
        assert!(
            !expected.is_empty(),
            "case DEPLOYED_ON_AWS={status}: the catalog must hold a listing reporting it"
        );
    }
}

/// Every record's `resources` span both modelled resource types and all four modelled
/// content formats, and each resource's value is in the format it declares — over every
/// listing and every product in the catalog.
#[tokio::test]
async fn every_record_spans_the_modelled_resource_kinds() {
    let router = router();
    let listings = every_listing(&router).await;

    let mut records = 0_usize;
    for (listing_id, listing) in &listings {
        let mut rows: Vec<(String, Value)> = vec![(format!("getListing {listing_id}"), listing.clone())];
        for product in products_of(&router, listing).await {
            rows.push((
                format!("getProduct {}", text(&product, "productId")),
                product,
            ));
        }
        // Rows: the listing and each of its products, which carry resources of their own.
        for (case, record) in rows {
            let resources = array(&record, "resources");
            let types: BTreeSet<String> = resources
                .iter()
                .map(|resource| text(resource, "resourceType"))
                .collect();
            let formats: BTreeSet<String> = resources
                .iter()
                .map(|resource| text(resource, "contentType"))
                .collect();
            assert_eq!(
                types,
                RESOURCE_TYPE_VALUES
                    .iter()
                    .map(|value| (*value).to_string())
                    .collect::<BTreeSet<String>>(),
                "case {case}: resources must cover every modelled ResourceType, got {types:?}"
            );
            assert_eq!(
                formats,
                RESOURCE_CONTENT_TYPE_VALUES
                    .iter()
                    .map(|value| (*value).to_string())
                    .collect::<BTreeSet<String>>(),
                "case {case}: resources must cover every modelled ResourceContentType, got \
                 {formats:?}"
            );
            for resource in &resources {
                let content_type = text(resource, "contentType");
                let value = text(resource, "value");
                let well_formed = match content_type.as_str() {
                    // `Resource$contentType` is "[t]he format of the resource content, such
                    // as a URL, email address, or text", so the value has to be in it.
                    "EMAIL" => value.contains('@') && !value.contains(' '),
                    "LINK" => value.starts_with("http://") || value.starts_with("https://"),
                    "PHONE_NUMBER" => {
                        value.starts_with('+') && value.chars().filter(char::is_ascii_digit).count() >= 7
                    }
                    "OTHER" => !value.trim().is_empty(),
                    _ => false,
                };
                assert!(
                    well_formed,
                    "case {case} / contentType={content_type}: the value must be in that \
                     format, got {value:?}"
                );
                assert!(
                    !text(resource, "displayName").trim().is_empty(),
                    "case {case} / contentType={content_type}: a resource the catalog labels \
                     must carry a non-blank displayName, got {resource}"
                );
            }
            records += 1;
        }
    }
    assert!(
        records > listings.len(),
        "case setup: products as well as listings must be checked, got {records} records for {} \
         listings",
        listings.len()
    );
}

/// A record's `categories` are the whole taxonomy path of each category it is classified
/// under: every entry's parent is in the list and comes before it, nothing repeats, and the
/// catalog holds records at each depth the taxonomy nests — including one classified under
/// two categories at once.
#[tokio::test]
async fn a_records_categories_are_its_whole_taxonomy_path() {
    let router = router();
    let listings = every_listing(&router).await;

    let mut deepest = 0_usize;
    let mut widest = 0_usize;
    for (listing_id, listing) in &listings {
        let mut rows: Vec<(String, Value)> = vec![(format!("getListing {listing_id}"), listing.clone())];
        for product in products_of(&router, listing).await {
            rows.push((
                format!("getProduct {}", text(&product, "productId")),
                product,
            ));
        }
        // Rows: the listing and each of its products, which classify independently.
        for (case, record) in rows {
            let ids: Vec<String> = array(&record, "categories")
                .iter()
                .map(|category| text(category, "categoryId"))
                .collect();
            assert!(
                !ids.is_empty(),
                "case {case}: a record must be classified under at least one category"
            );
            let unique: BTreeSet<&String> = ids.iter().collect();
            assert_eq!(
                unique.len(),
                ids.len(),
                "case {case}: a category must appear once in a path, got {ids:?}"
            );
            for category in array(&record, "categories") {
                let id = text(&category, "categoryId");
                assert!(
                    !text(&category, "displayName").trim().is_empty(),
                    "case {case}: Category$displayName is required and non-blank, got {category}"
                );
                // The path is ancestor-closed and runs outermost first, so a client walking
                // it from the root never meets a category whose parent it has not seen.
                let mut depth = 1_usize;
                let mut cursor = category_parent(&id);
                while let Some(parent) = cursor {
                    let parent_at = ids.iter().position(|held| held == parent);
                    let child_at = ids.iter().position(|held| *held == id);
                    assert!(
                        parent_at.is_some() && parent_at < child_at,
                        "case {case}: {id} sits under {parent}, so {parent} must be in the path \
                         before it, got {ids:?}"
                    );
                    depth += 1;
                    cursor = category_parent(parent);
                }
                deepest = deepest.max(depth);
            }
            widest = widest.max(ids.len());
        }
    }
    assert!(
        deepest >= 3,
        "case whole catalog: the catalog must classify a record at the taxonomy's third \
         level, got a deepest path of {deepest}"
    );
    assert!(
        widest >= 4,
        "case whole catalog: the catalog must hold a record classified under more than one \
         category, got a widest classification of {widest} categories"
    );
}

/// `CATEGORY` facet values refine through every level of the taxonomy: each value reports the
/// parent the taxonomy declares, that parent is itself a facet value of the same response,
/// and filtering on an ancestor selects every listing its descendant selects.
#[tokio::test]
async fn the_category_facet_refines_through_every_level_of_the_taxonomy() {
    let router = router();

    let mut grandchildren = 0_usize;
    // Rows: queries matching the whole catalog, a slice of it and none of it.
    for (case, query) in facet_queries() {
        let values: BTreeMap<String, Value> = facets(&router, &query, "CATEGORY")
            .await
            .into_iter()
            .map(|facet| (text(&facet, "value"), facet))
            .collect();
        for (value, facet) in &values {
            let expected = category_parent(value).map(str::to_string);
            let actual = facet
                .get("parent")
                .filter(|parent| !parent.is_null())
                .and_then(Value::as_str)
                .map(str::to_string);
            assert_eq!(
                actual, expected,
                "case {case} / CATEGORY={value}: expected parent {expected:?}, got {actual:?}"
            );
            if let Some(parent) = &expected {
                assert!(
                    values.contains_key(parent),
                    "case {case} / CATEGORY={value}: its parent {parent} must be a facet value \
                     of the same response, got {:?}",
                    values.keys().collect::<Vec<_>>()
                );
                // A value two levels down: its parent has a parent, which must also be
                // reported, so refinement can run grandparent -> parent -> value.
                if let Some(grandparent) = category_parent(parent) {
                    assert!(
                        values.contains_key(grandparent),
                        "case {case} / CATEGORY={value}: its grandparent {grandparent} must be \
                         a facet value too, got {:?}",
                        values.keys().collect::<Vec<_>>()
                    );
                    grandchildren += 1;

                    // Filtering up the chain selects supersets: browsing an ancestor finds
                    // everything its descendants find.
                    let selected = listings_in_category(&router, value).await;
                    let in_parent = listings_in_category(&router, parent).await;
                    let in_grandparent = listings_in_category(&router, grandparent).await;
                    assert!(
                        !selected.is_empty()
                            && selected.is_subset(&in_parent)
                            && in_parent.is_subset(&in_grandparent),
                        "case {case} / CATEGORY={value}: expected {selected:?} within \
                         {in_parent:?} within {in_grandparent:?}"
                    );
                }
            }
        }
    }
    assert!(
        grandchildren > 0,
        "case queries: the catalog must report a facet value nested two levels deep, so \
         hierarchical refinement is exercised beyond one level"
    );
}

/// Every facet list is ordered by descending count, with the facet value breaking a tie, and
/// no facet value is reported with no matching listing — the order a refine-search UI shows
/// its filters in.
#[tokio::test]
async fn every_facet_list_is_ordered_by_descending_count() {
    let router = router();

    let mut non_empty_lists = 0_usize;
    // Rows: every facet type the model declares, over four queries.
    for (case, query) in facet_queries() {
        for facet_type in SEARCH_FACET_TYPE_VALUES {
            let list = facets(&router, &query, facet_type).await;
            let mut previous: Option<(i64, String)> = None;
            for facet in &list {
                let count = facet.get("count").and_then(Value::as_i64).unwrap_or(-1);
                let value = text(facet, "value");
                assert!(
                    count > 0,
                    "case {case} / {facet_type}={value}: a reported facet value must count at \
                     least one matching listing, got {facet}"
                );
                assert!(
                    !text(facet, "displayName").trim().is_empty(),
                    "case {case} / {facet_type}={value}: ListingFacet$displayName is required \
                     and non-blank, got {facet}"
                );
                if let Some((previous_count, previous_value)) = &previous {
                    let ordered = *previous_count > count
                        || (*previous_count == count && previous_value.as_str() < value.as_str());
                    assert!(
                        ordered,
                        "case {case} / {facet_type}: expected descending counts then ascending \
                         values, got {previous_value} ({previous_count}) before {value} ({count})"
                    );
                }
                previous = Some((count, value));
            }
            if !list.is_empty() {
                non_empty_lists += 1;
            }
        }
    }
    assert!(
        non_empty_lists >= SEARCH_FACET_TYPE_VALUES.len(),
        "case queries: every facet type must report values for some query, got \
         {non_empty_lists} non-empty lists"
    );
}
