// SPDX-License-Identifier: CC0-1.0

//! `ListFulfillmentOptions` end to end, over EVERY product the catalog holds.
//!
//! The rule under test: `ListFulfillmentOptions` reports the fulfillment options of the
//! product named by `productId`, one `FulfillmentOption` union member per option with
//! exactly one member set, paged `maxResults` at a time in one stable order, and the set
//! of `fulfillmentOptionType`s it reports is the same set the product's
//! `fulfillmentOptionSummaries` summarise.
//!
//! Every suite drives real HTTP requests through the `MultiServiceRouter`, so the
//! protocol, the routing table, input validation and the handlers are all exercised.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_marketplacediscovery_awsmarketplacediscovery::EmulatedService;

/// The model's `POST /2026-02-05/<op>` route prefix (`OPERATIONS.md`).
const PREFIX: &str = "http://discovery-marketplace.us-east-1.amazonaws.com/2026-02-05";

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name, which is what the router's `can_handle` routes on.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260205/us-east-1/aws-marketplace/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// Every value of the model's `FulfillmentOptionType` enum, and the `FulfillmentOption`
/// union member that carries an option of that type.
///
/// Source: `smithy_query(query="shape",shape="FulfillmentOptionType")` for the values and
/// `smithy_query(query="shape",shape="FulfillmentOption")` for the union members.
const FULFILLMENT_OPTION_TYPES: &[(&str, &str)] = &[
    ("AMAZON_MACHINE_IMAGE", "amazonMachineImageFulfillmentOption"),
    ("API", "apiFulfillmentOption"),
    ("CLOUDFORMATION_TEMPLATE", "cloudFormationFulfillmentOption"),
    ("CONTAINER", "containerFulfillmentOption"),
    ("HELM", "helmFulfillmentOption"),
    ("EKS_ADD_ON", "eksAddOnFulfillmentOption"),
    (
        "EC2_IMAGE_BUILDER_COMPONENT",
        "ec2ImageBuilderComponentFulfillmentOption",
    ),
    ("DATA_EXCHANGE", "dataExchangeFulfillmentOption"),
    (
        "PROFESSIONAL_SERVICES",
        "professionalServicesFulfillmentOption",
    ),
    ("SAAS", "saasFulfillmentOption"),
    ("SAGEMAKER_ALGORITHM", "sageMakerAlgorithmFulfillmentOption"),
    ("SAGEMAKER_MODEL", "sageMakerModelFulfillmentOption"),
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

/// The single union member of a `FulfillmentOption`: its name and its value.
///
/// A union serialises as an object with exactly one member set
/// (`FulfillmentOption`: "Each element contains exactly one fulfillment option type"),
/// which this asserts before unwrapping it.
fn union_member(case: &str, option: &Value) -> (String, Value) {
    let object = option.as_object().unwrap_or_else(|| {
        panic!("case {case}: a fulfillmentOptions element must be an object, got {option}")
    });
    assert_eq!(
        object.len(),
        1,
        "case {case}: a FulfillmentOption carries exactly one union member, got {option}"
    );
    let (name, value) = object.iter().next().expect("one member");
    (name.clone(), value.clone())
}

/// Every product id the catalog holds, read back out of the listings that associate them.
async fn every_product_id(router: &MultiServiceRouter) -> Vec<String> {
    let catalog = ok(router, "searchListings", &json!({ "maxResults": 100 })).await;
    let mut product_ids: Vec<String> = Vec::new();
    for listing in array(&catalog, "listingSummaries") {
        for entity in array(&listing, "associatedEntities") {
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
        "case setup: the seeded catalog must hold at least one product, got {catalog}"
    );
    product_ids
}

/// Every fulfillment option of `product_id`, read one whole page at a time.
async fn all_options(router: &MultiServiceRouter, product_id: &str) -> Vec<Value> {
    let body = ok(
        router,
        "listFulfillmentOptions",
        &json!({ "productId": product_id, "maxResults": 50 }),
    )
    .await;
    assert!(
        body.get("nextToken").is_none(),
        "case {product_id}: a page holding every option must not carry a nextToken, got {body}"
    );
    array(&body, "fulfillmentOptions")
}

#[tokio::test]
async fn every_product_reports_its_fulfillment_options_as_single_member_unions() {
    let router = router();

    // Rows: every product in the catalog, which differ in how many options they have
    // (one, two and three) and in which union members those options are.
    for product_id in every_product_id(&router).await {
        let options = all_options(&router, &product_id).await;
        assert!(
            !options.is_empty(),
            "case {product_id}: a catalog product must offer at least one fulfillment option"
        );

        let mut ids: Vec<String> = Vec::new();
        for option in &options {
            let (member, detail) = union_member(&product_id, option);
            let option_type = text(&detail, "fulfillmentOptionType");
            let expected_member = FULFILLMENT_OPTION_TYPES
                .iter()
                .find(|(value, _)| *value == option_type)
                .map(|(_, member)| *member)
                .unwrap_or_else(|| {
                    panic!(
                        "case {product_id}: fulfillmentOptionType {option_type:?} is not a \
                         FulfillmentOptionType value, in {detail}"
                    )
                });
            assert_eq!(
                member, expected_member,
                "case {product_id}: an option of type {option_type} must arrive in \
                 {expected_member}, got {member}"
            );

            let option_id = text(&detail, "fulfillmentOptionId");
            assert!(
                !option_id.is_empty(),
                "case {product_id}: every option carries a fulfillmentOptionId, got {detail}"
            );
            assert!(
                !ids.contains(&option_id),
                "case {product_id}: fulfillmentOptionId {option_id} is reported twice in \
                 {options:?}"
            );
            ids.push(option_id);
            assert!(
                !text(&detail, "fulfillmentOptionDisplayName").is_empty(),
                "case {product_id}: every option carries a fulfillmentOptionDisplayName, got \
                 {detail}"
            );
        }

        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(
            ids, sorted,
            "case {product_id}: options are reported in one stable order, by \
             fulfillmentOptionId"
        );
    }
}

#[tokio::test]
async fn the_reported_option_types_are_the_products_summarised_types() {
    let router = router();

    // Rows: every product, so a product with one option type, one with two and one whose
    // two options share no type are all covered.
    for product_id in every_product_id(&router).await {
        let options = all_options(&router, &product_id).await;
        let detailed: BTreeSet<String> = options
            .iter()
            .map(|option| {
                let (_, detail) = union_member(&product_id, option);
                text(&detail, "fulfillmentOptionType")
            })
            .collect();

        let product = ok(
            &router,
            "getProduct",
            &json!({ "productId": product_id.clone() }),
        )
        .await;
        let summaries = array(&product, "fulfillmentOptionSummaries");
        let summarised: BTreeSet<String> = summaries
            .iter()
            .map(|summary| text(summary, "fulfillmentOptionType"))
            .collect();
        assert_eq!(
            detailed, summarised,
            "case {product_id}: GetProduct's fulfillmentOptionSummaries must summarise exactly \
             the types ListFulfillmentOptions reports, got {summaries:?} for {detailed:?}"
        );
        assert_eq!(
            summaries.len(),
            summarised.len(),
            "case {product_id}: a type is summarised once, got {summaries:?}"
        );

        // A type's display name is a property of the TYPE, so the detail and the summary
        // of one type agree on it.
        for option in &options {
            let (_, detail) = union_member(&product_id, option);
            let option_type = text(&detail, "fulfillmentOptionType");
            let summary = summaries
                .iter()
                .find(|summary| text(summary, "fulfillmentOptionType") == option_type)
                .unwrap_or_else(|| panic!("case {product_id}: {option_type} is summarised"));
            assert_eq!(
                text(&detail, "fulfillmentOptionDisplayName"),
                text(summary, "displayName"),
                "case {product_id}/{option_type}: the option's display name must be the type's, \
                 got {detail} against {summary}"
            );
        }
    }
}

#[tokio::test]
async fn a_listing_summarises_the_types_its_products_offer() {
    let router = router();
    let catalog = ok(&router, "searchListings", &json!({ "maxResults": 100 })).await;

    // Rows: every listing, which differ in how many products they associate (one and two).
    for summary in array(&catalog, "listingSummaries") {
        let listing_id = text(&summary, "listingId");
        let listing = ok(
            &router,
            "getListing",
            &json!({ "listingId": listing_id.clone() }),
        )
        .await;

        let mut expected: BTreeSet<String> = BTreeSet::new();
        for entity in array(&listing, "associatedEntities") {
            let Some(product) = entity.get("product") else {
                continue;
            };
            for option in all_options(&router, &text(product, "productId")).await {
                let (_, detail) = union_member(&listing_id, &option);
                expected.insert(text(&detail, "fulfillmentOptionType"));
            }
        }

        for (source, body) in [("getListing", &listing), ("searchListings", &summary)] {
            let reported: BTreeSet<String> = array(body, "fulfillmentOptionSummaries")
                .iter()
                .map(|entry| text(entry, "fulfillmentOptionType"))
                .collect();
            assert_eq!(
                reported, expected,
                "case {source} {listing_id}: a listing summarises the fulfillment option types \
                 of its products, got {reported:?} for {expected:?}"
            );
        }
    }
}

#[tokio::test]
async fn max_results_bounds_the_page_and_next_token_walks_the_rest() {
    let router = router();

    for product_id in every_product_id(&router).await {
        let all = all_options(&router, &product_id).await;
        let total = all.len();

        // A request that omits `maxResults` takes the modelled default page size, 25
        // (`smithy_query(query="operation",operation="ListFulfillmentOptions")`), which is
        // more than any one product offers.
        let defaulted = ok(
            &router,
            "listFulfillmentOptions",
            &json!({ "productId": product_id }),
        )
        .await;
        assert_eq!(
            array(&defaulted, "fulfillmentOptions").len(),
            total.min(25),
            "case {product_id} maxResults omitted: the default page size is 25, got {defaulted}"
        );

        // Rows: a page smaller than the result set, one the size of it, and one larger,
        // plus the model's `maxResults` bounds (1 and 50).
        let mut sizes: Vec<usize> = vec![1, total, total + 1, 50];
        sizes.dedup();
        for page_size in sizes {
            let mut seen: Vec<Value> = Vec::new();
            let mut next_token: Option<String> = None;
            let mut pages = 0;
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
                if next_token.is_none() {
                    break;
                }
                assert!(
                    pages <= total,
                    "case {product_id} maxResults={page_size}: pagination must terminate, \
                     {pages} pages for {total} options"
                );
            }
            assert_eq!(
                seen, all,
                "case {product_id} maxResults={page_size}: paging must yield every option once, \
                 in the same order as one whole page"
            );
            let expected_pages = total.div_ceil(page_size);
            assert_eq!(
                pages, expected_pages,
                "case {product_id} maxResults={page_size}: expected {expected_pages} pages, got \
                 {pages}"
            );
        }
    }
}

#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    let router = router();
    let product_id = every_product_id(&router).await[0].clone();

    // `maxResults` has range 1..50 (`smithy_query(query="operation",
    // operation="ListFulfillmentOptions")`), so each published bound is accepted and the
    // value one past it is refused.
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

#[tokio::test]
async fn a_token_from_another_query_or_past_the_end_is_refused() {
    let router = router();
    let product_ids = every_product_id(&router).await;

    // A product with MORE than one fulfillment option, so a one-option page truncates and
    // the service mints a token at all; plus any other product, whose query that token was
    // not minted for.
    let mut multi_option: Option<String> = None;
    for candidate in &product_ids {
        if all_options(&router, candidate).await.len() > 1 {
            multi_option = Some(candidate.clone());
            break;
        }
    }
    let product_id = multi_option.unwrap_or_else(|| {
        panic!(
            "case setup: the catalog must hold a product with several fulfillment options, got \
             {product_ids:?}"
        )
    });
    let other_product_id = product_ids
        .iter()
        .find(|candidate| **candidate != product_id)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case setup: expected a second product in the catalog, got {product_ids:?}")
        });

    let first = ok(
        &router,
        "listFulfillmentOptions",
        &json!({ "productId": product_id, "maxResults": 1 }),
    )
    .await;
    let token = text(&first, "nextToken");
    assert!(
        !token.is_empty(),
        "case setup: a one-option page of a multi-option product carries a nextToken, got {first}"
    );

    // Rows: a token this service never minted, and one it minted for another product.
    for (case, bad_token) in [
        ("not a token", "bm90LWEtdG9rZW4=".to_string()),
        ("another product's token", token.clone()),
    ] {
        let product = if case == "another product's token" {
            other_product_id.clone()
        } else {
            product_id.clone()
        };
        let (status, body) = call(
            &router,
            "listFulfillmentOptions",
            &json!({ "productId": product, "nextToken": bad_token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: a token that is not this query's must be refused, got {status} with \
             {body}"
        );
    }

    // The token this query DID mint is accepted.
    let second = ok(
        &router,
        "listFulfillmentOptions",
        &json!({ "productId": product_id, "nextToken": token, "maxResults": 1 }),
    )
    .await;
    assert_ne!(
        array(&second, "fulfillmentOptions"),
        array(&first, "fulfillmentOptions"),
        "case page 2: the next page must not repeat page 1"
    );
}

#[tokio::test]
async fn a_product_the_catalog_does_not_hold_is_not_found() {
    let router = router();
    let known = every_product_id(&router).await[0].clone();

    // Rows: an absent id, an id of another kind of resource, and the known id with a
    // suffix, so it differs from a real one only in its last characters.
    for unknown in [
        "does-not-exist".to_string(),
        "l-0001-analytics".to_string(),
        format!("{known}-x"),
    ] {
        let (status, body) = call(
            &router,
            "listFulfillmentOptions",
            &json!({ "productId": unknown }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case productId={unknown}: expected HTTP 404, got {status} with {body}"
        );
        assert_eq!(
            text(&body, "__type"),
            "ResourceNotFoundException",
            "case productId={unknown}: expected ResourceNotFoundException, got {body}"
        );
    }
}

#[tokio::test]
async fn the_reported_locale_is_the_one_the_content_is_returned_in() {
    let router = router();

    // Rows: the locales the service supports (`Locale` documentation), `en` which resolves
    // to the default locale, an unsupported tag with a supported fallback, and an
    // unsupported tag with none.
    let cases: [(&str, Option<&str>); 8] = [
        ("en-US", Some("en-US")),
        ("en", Some("en-US")),
        ("fr", Some("fr")),
        ("es", Some("es")),
        ("ko", Some("ko")),
        ("ja", Some("ja")),
        ("xx, fr", Some("fr")),
        ("xx", None),
    ];
    for product_id in every_product_id(&router).await {
        // The locale the product's own content is returned in, which is what the
        // fulfillment options of that product are returned in too.
        for (requested, resolved) in cases {
            let (status, body) = call(
                &router,
                "listFulfillmentOptions",
                &json!({ "productId": product_id, "locale": requested }),
            )
            .await;
            let Some(resolved) = resolved else {
                assert_eq!(
                    status, 400,
                    "case {product_id} locale={requested:?}: an unsupported locale with no \
                     fallback must be refused, got {status} with {body}"
                );
                continue;
            };
            assert_eq!(
                status, 200,
                "case {product_id} locale={requested:?}: expected HTTP 200, got {status} with \
                 {body}"
            );
            let product = ok(
                &router,
                "getProduct",
                &json!({ "productId": product_id, "locale": requested }),
            )
            .await;
            assert_eq!(
                text(&body, "locale"),
                text(&product, "locale"),
                "case {product_id} locale={requested:?}: the options are returned in the locale \
                 the product's content is, got {body}"
            );
            assert!(
                [resolved, "en-US"].contains(&text(&body, "locale").as_str()),
                "case {product_id} locale={requested:?}: expected {resolved} or the default \
                 locale, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn translated_usage_instructions_are_returned_in_the_translated_locale() {
    let router = router();

    // The catalog holds a French translation of at least one option's usage instructions;
    // find it rather than naming it, so the case is not pinned to one seeded product.
    let mut translated = 0;
    for product_id in every_product_id(&router).await {
        let default_locale = all_options(&router, &product_id).await;
        let french = ok(
            &router,
            "listFulfillmentOptions",
            &json!({ "productId": product_id, "locale": "fr", "maxResults": 50 }),
        )
        .await;
        let french = array(&french, "fulfillmentOptions");
        assert_eq!(
            french.len(),
            default_locale.len(),
            "case {product_id}: a locale changes the text of the options, never how many \
             there are"
        );
        for (en, fr) in default_locale.iter().zip(french.iter()) {
            let (en_member, en_detail) = union_member(&product_id, en);
            let (fr_member, fr_detail) = union_member(&product_id, fr);
            assert_eq!(
                en_member, fr_member,
                "case {product_id}: a locale changes no option's union member"
            );
            assert_eq!(
                text(&en_detail, "fulfillmentOptionId"),
                text(&fr_detail, "fulfillmentOptionId"),
                "case {product_id}: a locale changes no fulfillmentOptionId"
            );
            assert_eq!(
                text(&en_detail, "fulfillmentOptionType"),
                text(&fr_detail, "fulfillmentOptionType"),
                "case {product_id}: a locale changes no fulfillmentOptionType"
            );
            if text(&en_detail, "usageInstructions") != text(&fr_detail, "usageInstructions") {
                translated += 1;
            }
        }
    }
    assert!(
        translated > 0,
        "case fr: the catalog must hold at least one translated option, so a locale is \
         observable in the response text"
    );
}
