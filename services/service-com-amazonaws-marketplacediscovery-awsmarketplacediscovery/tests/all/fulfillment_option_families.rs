// SPDX-License-Identifier: CC0-1.0

//! How MANY fulfillment options a catalog product offers, and what follows from that.
//!
//! THE RULE UNDER TEST: a product of this catalog is fulfilled one-to-MANY — `Product`
//! carries a `FulfillmentOptionsList` and `ListFulfillmentOptions` is `@paginated` with a
//! `maxResults` of its own — so a product is offered through every route of its delivery
//! family: the fulfillment option types that deliver it the same way
//! (`state::fulfilled_through`). Three things follow, and each is asserted here:
//!
//! 1. a product the catalog exposes through `SearchListings` offers SEVERAL fulfillment
//!    options, so the truncation and continuation paths of this paginated operation are
//!    reachable at all;
//! 2. a page is bounded by `maxResults` — exactly `maxResults` items while items remain,
//!    with a `nextToken` for the remainder — and walking the tokens yields every option
//!    once, in the order one whole page reports them;
//! 3. the options of one product are one DEPLOYMENT: every type of a delivery family
//!    implies the same `deployedOnAws`, so offering a product its whole family says nothing
//!    new about where it runs.
//!
//! Every request goes through the `MultiServiceRouter`, so the protocol, the routing table
//! and the handlers are all exercised end to end.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_marketplacediscovery_awsmarketplacediscovery::ops::types::{
    FULFILLMENT_OPTION_TYPE_VALUES,
};
use service_com_amazonaws_marketplacediscovery_awsmarketplacediscovery::state::{
    deployment_status, fulfilled_through,
};

use crate::catalog_drilldown::{array, call, ok, router, text};

/// How many listing summaries the scan below reads, which is the page a client takes first.
const SCANNED_LISTINGS: i32 = 20;

/// The `fulfillmentOptionType` of every option on one page, read from the union member each
/// option carries.
fn option_types(page: &Value) -> Vec<String> {
    array(page, "fulfillmentOptions")
        .iter()
        .filter_map(|option| {
            option
                .as_object()
                .and_then(|object| object.values().next().cloned())
        })
        .map(|member| text(&member, "fulfillmentOptionType"))
        .collect()
}

/// Every product id the first page of `SearchListings` reaches, through the listings it
/// summarises.
async fn scanned_product_ids(router: &MultiServiceRouter) -> Vec<String> {
    let page = ok(
        router,
        "searchListings",
        &json!({ "maxResults": SCANNED_LISTINGS }),
    )
    .await;
    let mut product_ids: Vec<String> = Vec::new();
    for summary in array(&page, "listingSummaries") {
        for entity in array(&summary, "associatedEntities") {
            let Some(product) = entity.get("product") else {
                continue;
            };
            let product_id = text(product, "productId");
            if !product_id.is_empty() && !product_ids.contains(&product_id) {
                product_ids.push(product_id);
            }
        }
    }
    assert!(
        !product_ids.is_empty(),
        "case setup: the scan must reach a product, got {page}"
    );
    product_ids
}

/// RULE 1: the catalog a client sees first exposes products with SEVERAL fulfillment
/// options, so a paginating client is exercised at all.
#[tokio::test]
async fn the_catalog_exposes_products_with_several_fulfillment_options() {
    let router = router();
    let product_ids = scanned_product_ids(&router).await;

    let mut multi_option = 0_usize;
    for product_id in &product_ids {
        // The model's largest page (50) is more than any product offers, so one page is all
        // of them.
        let page = ok(
            &router,
            "listFulfillmentOptions",
            &json!({ "productId": product_id, "maxResults": 50 }),
        )
        .await;
        let types = option_types(&page);
        assert!(
            !types.is_empty(),
            "case {product_id}: a catalog product must report how it is fulfilled, got {page}"
        );
        let distinct: BTreeSet<&String> = types.iter().collect();
        assert_eq!(
            distinct.len(),
            types.len(),
            "case {product_id}: a product offers each delivery route once, got {types:?}"
        );
        if types.len() > 1 {
            multi_option += 1;
        }
    }
    assert_eq!(
        multi_option,
        product_ids.len(),
        "case the first page of the catalog: every product it reaches must offer 2 or more \
         fulfillment options, got {multi_option} of {} products",
        product_ids.len()
    );
}

/// RULE 2: `maxResults` bounds a page and `nextToken` walks the rest, over a table of page
/// sizes against a product offering several options.
#[tokio::test]
async fn max_results_truncates_a_page_and_next_token_walks_the_remainder() {
    let router = router();
    let product_ids = scanned_product_ids(&router).await;

    let mut exercised = 0_usize;
    for product_id in product_ids.iter().take(4) {
        let whole = ok(
            &router,
            "listFulfillmentOptions",
            &json!({ "productId": product_id, "maxResults": 50 }),
        )
        .await;
        let all = array(&whole, "fulfillmentOptions");
        assert!(
            whole.get("nextToken").is_none_or(Value::is_null),
            "case {product_id}: a page holding every option carries no nextToken, got {whole}"
        );
        let total = all.len();
        assert!(
            total > 1,
            "case {product_id}: this rule needs a product with several options, got {total}"
        );

        // Rows: the model's `maxResults` minimum, every size short of the whole set, the set
        // itself and one past it (`smithy_query(query="operation",
        // operation="ListFulfillmentOptions")` declares @range 1..50).
        for page_size in 1..=total + 1 {
            let mut seen: Vec<Value> = Vec::new();
            let mut next_token: Option<String> = None;
            let mut pages = 0_usize;
            loop {
                let mut input = json!({ "productId": product_id, "maxResults": page_size });
                if let Some(token) = &next_token {
                    input["nextToken"] = json!(token);
                }
                let body = ok(&router, "listFulfillmentOptions", &input).await;
                let page = array(&body, "fulfillmentOptions");
                let expected = page_size.min(total - seen.len());
                assert_eq!(
                    page.len(),
                    expected,
                    "case {product_id} maxResults={page_size} page {pages}: expected {expected} \
                     options, got {}",
                    page.len()
                );
                seen.extend(page);
                pages += 1;
                next_token = body
                    .get("nextToken")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                // A token is returned exactly while options remain.
                assert_eq!(
                    next_token.is_some(),
                    seen.len() < total,
                    "case {product_id} maxResults={page_size} page {pages}: a nextToken must be \
                     returned if and only if options remain, got {body}"
                );
                if next_token.is_none() {
                    break;
                }
            }
            assert_eq!(
                seen, all,
                "case {product_id} maxResults={page_size}: paging must yield every option once, \
                 in the order one whole page reports them"
            );
            assert_eq!(
                pages,
                total.div_ceil(page_size),
                "case {product_id} maxResults={page_size}: expected {} pages, got {pages}",
                total.div_ceil(page_size)
            );
            exercised += 1;
        }
    }
    assert!(
        exercised > 4,
        "case the scanned products: expected several page sizes per product, exercised only \
         {exercised}"
    );
}

/// RULE 3: a delivery family is ONE deployment — every type it names implies the same
/// `deployedOnAws` — and a family starts with the type it was derived from, naming it once.
///
/// Asserted over EVERY `FulfillmentOptionType` the model declares, which is a row the issue
/// never cited for most of them.
#[test]
fn a_delivery_family_names_one_deployment_and_its_own_type_first() {
    for option_type in FULFILLMENT_OPTION_TYPE_VALUES {
        let family = fulfilled_through(option_type);
        assert_eq!(
            family.first().map(String::as_str),
            Some(*option_type),
            "case {option_type}: a product's own fulfillment type comes first, got {family:?}"
        );
        let distinct: BTreeSet<&String> = family.iter().collect();
        assert_eq!(
            distinct.len(),
            family.len(),
            "case {option_type}: a family names each type once, got {family:?}"
        );
        let expected = deployment_status(option_type);
        for member in &family {
            assert_eq!(
                deployment_status(member),
                expected,
                "case {option_type}: every type of the family must imply the deployment \
                 {expected}, but {member} implies {}",
                deployment_status(member)
            );
        }
    }
}

/// RULE 3 end to end: every product's `deployedOnAws` is the one its own options imply, and
/// every modelled status is still reachable across the catalog — so offering a product its
/// whole delivery family changed no product's deployment claim.
#[tokio::test]
async fn a_products_deployment_is_still_the_one_its_options_imply() {
    let router = router();
    let mut reported: BTreeSet<String> = BTreeSet::new();
    // The whole catalog: the model's largest `SearchListings` page is 100 and the catalog
    // fits one, which `catalog_record_breadth` asserts.
    let page = ok(&router, "searchListings", &json!({ "maxResults": 100 })).await;
    for summary in array(&page, "listingSummaries") {
        for entity in array(&summary, "associatedEntities") {
            let Some(product) = entity.get("product") else {
                continue;
            };
            let product_id = text(product, "productId");
            let detail = ok(
                &router,
                "getProduct",
                &json!({ "productId": product_id.clone() }),
            )
            .await;
            let options = ok(
                &router,
                "listFulfillmentOptions",
                &json!({ "productId": product_id.clone(), "maxResults": 50 }),
            )
            .await;
            let types = option_types(&options);
            // The strongest claim the product's own options support, which is what
            // `Product$deployedOnAws` reports.
            let expected = if types.iter().any(|t| deployment_status(t) == "DEPLOYED") {
                "DEPLOYED"
            } else if types.iter().any(|t| deployment_status(t) == "NOT_DEPLOYED") {
                "NOT_DEPLOYED"
            } else {
                "NOT_APPLICABLE"
            };
            let actual = text(&detail, "deployedOnAws");
            assert_eq!(
                actual, expected,
                "case {product_id} fulfilled as {types:?}: expected deployedOnAws {expected}, \
                 got {actual:?}"
            );
            reported.insert(actual);
        }
    }
    assert_eq!(
        reported,
        ["DEPLOYED", "NOT_APPLICABLE", "NOT_DEPLOYED"]
            .iter()
            .map(|status| (*status).to_string())
            .collect::<BTreeSet<String>>(),
        "case the whole catalog: every modelled deployment status must stay reachable, got \
         {reported:?}"
    );
}

/// A `maxResults` outside the modelled range is still refused, so the truncation above is
/// the operation honouring a bound rather than ignoring one.
#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    let router = router();
    let product_id = scanned_product_ids(&router).await[0].clone();

    // `maxResults` is @range 1..50, so each published bound is accepted and the value one
    // past it is refused.
    for (max_results, expected) in [(1, 200), (50, 200), (0, 400), (51, 400)] {
        let (status, body) = call(
            &router,
            "listFulfillmentOptions",
            &json!({ "productId": product_id, "maxResults": max_results }),
        )
        .await;
        assert_eq!(
            status, expected,
            "case maxResults={max_results}: expected HTTP {expected}, got {status} with {body}"
        );
    }
}
