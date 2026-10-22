// SPDX-License-Identifier: CC0-1.0

//! Four rules of the AWS Marketplace Discovery read path, each asserted over a SET of
//! inputs rather than over one recorded request:
//!
//! 1. `sortOrder` is the sort direction of the RESULT, so it reverses the whole ordering
//!    `SearchListings` returns — tie-break included — and a request that sends no sort
//!    parameters gets the page the modelled defaults (`RELEVANCE` / `DESCENDING`) get.
//! 2. `GetListing`'s `associatedEntities` pairs each associated product with the default
//!    offer the listing associates for it, so an `offerId` is discoverable from a listing
//!    and `GetOffer` / `GetOfferTerms` are reachable through it.
//! 3. A `ResourceNotFoundException` message names only the KIND of resource that is
//!    missing: it is the same string for every unknown id and carries no part of the
//!    request and no catalog identifier.
//! 4. A range filter's refusal follows AWS's `"Range filter <filterType> ..."` template,
//!    naming the offending `filterType` and, out of range, the offending value bare.
//!
//! Every suite drives real HTTP requests through the `MultiServiceRouter`, reusing the
//! harness in `catalog_drilldown`.

use serde_json::{json, Value};

use crate::catalog_drilldown::{array, call, ids, ok, router, text, whole_catalog};

/// The `message` an error body carries, under whichever casing the protocol rendered it.
fn message(body: &Value) -> String {
    let lower = text(body, "message");
    if lower.is_empty() {
        text(body, "Message")
    } else {
        lower
    }
}

/// The listing ids `SearchListings` returns for one query, in the order it returns them.
async fn listing_ids(
    router: &smithy_server_runtime::MultiServiceRouter,
    sort_by: Option<&str>,
    sort_order: Option<&str>,
    search_text: &str,
    max_results: u64,
) -> Vec<String> {
    let mut input = json!({ "maxResults": max_results });
    if let Some(sort_by) = sort_by {
        input["sortBy"] = json!(sort_by);
    }
    if let Some(sort_order) = sort_order {
        input["sortOrder"] = json!(sort_order);
    }
    if !search_text.is_empty() {
        input["searchText"] = json!(search_text);
    }
    let body = ok(router, "searchListings", &input).await;
    ids(&array(&body, "listingSummaries"), "listingId")
}

#[tokio::test]
async fn sort_order_reverses_the_whole_ordering() {
    let router = router();

    // Rows span both modelled sort keys (`SearchListingsSortBy`) and queries whose matches
    // tie on the sort key as well as queries whose matches do not: no `searchText` scores
    // every listing alike, so a tie-break that ignores `sortOrder` shows up here.
    let cases: [(&str, &str); 6] = [
        ("RELEVANCE", ""),
        ("RELEVANCE", "a"),
        ("RELEVANCE", "data"),
        ("AVERAGE_CUSTOMER_RATING", ""),
        ("AVERAGE_CUSTOMER_RATING", "data"),
        ("AVERAGE_CUSTOMER_RATING", "cloud"),
    ];
    for (sort_by, search_text) in cases {
        let case = format!("sortBy={sort_by} searchText={search_text:?}");
        let descending = listing_ids(&router, Some(sort_by), Some("DESCENDING"), search_text, 100)
            .await;
        let ascending =
            listing_ids(&router, Some(sort_by), Some("ASCENDING"), search_text, 100).await;

        let mut reversed = ascending.clone();
        reversed.reverse();
        assert_eq!(
            descending, reversed,
            "case {case}: the DESCENDING ordering must be the exact reverse of the ASCENDING \
             one, got descending {descending:?} and ascending {ascending:?}"
        );
        if descending.len() > 1 {
            assert_ne!(
                descending, ascending,
                "case {case}: sortOrder must change the page when more than one listing \
                 matches, got {descending:?} for both directions"
            );
        }

        // A truncated page is the prefix of that direction's own full ordering, so the
        // direction holds page by page and not only over the whole match set.
        for max_results in [1_u64, 2, 3] {
            for (sort_order, full) in [("DESCENDING", &descending), ("ASCENDING", &ascending)] {
                let page =
                    listing_ids(&router, Some(sort_by), Some(sort_order), search_text, max_results)
                        .await;
                let expected: Vec<String> = full
                    .iter()
                    .take(usize::try_from(max_results).unwrap_or(usize::MAX))
                    .cloned()
                    .collect();
                assert_eq!(
                    page, expected,
                    "case {case} sortOrder={sort_order} maxResults={max_results}: the page must \
                     be the prefix of that direction's ordering, expected {expected:?}, got \
                     {page:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn omitting_the_sort_parameters_uses_the_modelled_defaults() {
    let router = router();

    // The model defaults sortBy to RELEVANCE and sortOrder to DESCENDING
    // (smithy_query(query="operation",operation="SearchListings")), so a request that sends
    // neither must get the page those two values get. Rows vary the page size and whether
    // the query carries searchText at all.
    for search_text in ["", "data"] {
        for max_results in [1_u64, 3, 100] {
            let case = format!("searchText={search_text:?} maxResults={max_results}");
            let defaulted = listing_ids(&router, None, None, search_text, max_results).await;
            let explicit = listing_ids(
                &router,
                Some("RELEVANCE"),
                Some("DESCENDING"),
                search_text,
                max_results,
            )
            .await;
            assert_eq!(
                defaulted, explicit,
                "case {case}: the default page must equal the explicit RELEVANCE/DESCENDING \
                 page, got {defaulted:?} and {explicit:?}"
            );

            let ascending = listing_ids(
                &router,
                Some("RELEVANCE"),
                Some("ASCENDING"),
                search_text,
                max_results,
            )
            .await;
            if defaulted.len() > 1 {
                assert_ne!(
                    defaulted, ascending,
                    "case {case}: the default page must differ from the ASCENDING page, got \
                     {defaulted:?} for both"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_listing_pairs_each_product_with_its_default_offer() {
    let router = router();
    // Rows: every listing the catalog holds, discovered through SearchListings the way a
    // client walks the catalog.
    let listing_ids = ids(&whole_catalog(&router).await, "listingId");
    assert!(
        !listing_ids.is_empty(),
        "case setup: the seeded catalog must hold listings"
    );

    for listing_id in listing_ids {
        let listing = ok(&router, "getListing", &json!({ "listingId": &listing_id })).await;
        let entities = array(&listing, "associatedEntities");
        assert!(
            !entities.is_empty(),
            "case {listing_id}: associatedEntities is required and must not be empty, got \
             {listing}"
        );
        for entity in &entities {
            assert!(
                entity.get("product").is_some() || entity.get("offer").is_some(),
                "case {listing_id}: an association must carry a product, an offer or both, got \
                 {entity}"
            );
        }

        // The documented discovery path: an entity populating BOTH product and offer.
        let paired = entities
            .iter()
            .find(|entity| entity.get("product").is_some() && entity.get("offer").is_some())
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "case {listing_id}: expected an association carrying both a product and its \
                     default offer, got {entities:?}"
                )
            });
        let product_id = text(&paired["product"], "productId");
        let offer_id = text(&paired["offer"], "offerId");
        assert!(
            !product_id.is_empty() && !offer_id.is_empty(),
            "case {listing_id}: productId and offerId are required on an association, got \
             {paired}"
        );

        // GetOffer resolves that offerId and cross-references the same productId.
        let offer = ok(&router, "getOffer", &json!({ "offerId": &offer_id })).await;
        assert_eq!(
            text(&offer, "offerId"),
            offer_id,
            "case {listing_id}: getOffer must echo the offerId it resolved, got {offer}"
        );
        let offer_products = ids(
            &array(&offer, "associatedEntities")
                .iter()
                .filter_map(|entity| entity.get("product").cloned())
                .collect::<Vec<Value>>(),
            "productId",
        );
        assert!(
            offer_products.contains(&product_id),
            "case {listing_id}: the default offer must associate the product it was paired \
             with, expected {product_id}, got {offer_products:?}"
        );

        // GetOfferTerms returns a non-empty list, each element a union with exactly one
        // member populated.
        let terms_body = ok(
            &router,
            "getOfferTerms",
            &json!({ "offerId": &offer_id, "maxResults": 25 }),
        )
        .await;
        let terms = array(&terms_body, "offerTerms");
        assert!(
            !terms.is_empty(),
            "case {listing_id}: the default offer's terms must not be empty, got {terms_body}"
        );
        for term in &terms {
            let members = term.as_object().map(serde_json::Map::len).unwrap_or(0);
            assert_eq!(
                members, 1,
                "case {listing_id}: an offer term is a union, so exactly one member must be \
                 populated, got {term}"
            );
        }
    }
}

#[tokio::test]
async fn a_not_found_message_names_only_the_kind_of_resource() {
    let router = router();

    // A known id per operation, discovered from the catalog, so each row has a positive
    // control beside its refusals.
    let listing_id = ids(&whole_catalog(&router).await, "listingId")
        .first()
        .cloned()
        .expect("a seeded listing");
    let listing = ok(&router, "getListing", &json!({ "listingId": &listing_id })).await;
    let paired = array(&listing, "associatedEntities")
        .into_iter()
        .find(|entity| entity.get("product").is_some() && entity.get("offer").is_some())
        .expect("a listing association carrying both a product and an offer");
    let product_id = text(&paired["product"], "productId");
    let offer_id = text(&paired["offer"], "offerId");

    // Rows: every operation that looks a resource up by id, with the resource KIND its
    // message names. Each is tried with several well-formed-but-unknown ids (all matching
    // the modelled ^[\w\-]+$), so a message built from the request fails here.
    let cases: [(&str, &str, &str, &str); 5] = [
        ("getListing", "listingId", "Listing", &listing_id),
        ("getProduct", "productId", "Product", &product_id),
        (
            "listFulfillmentOptions",
            "productId",
            "Product",
            &product_id,
        ),
        ("getOffer", "offerId", "Offer", &offer_id),
        ("getOfferTerms", "offerId", "Offer", &offer_id),
    ];
    for (operation, member, kind, known) in cases {
        let found = ok(&router, operation, &json!({ member: known })).await;
        assert!(
            found.is_object(),
            "case {operation} {member}={known}: the positive control must resolve, got {found}"
        );

        let unknown_ids = [
            "sim2real-nonexistent-fa15928f287b".to_string(),
            "unknown".to_string(),
            format!("{known}-x"),
            "x-00000000000000000000000000000000".to_string(),
        ];
        let mut seen: Vec<String> = Vec::new();
        for unknown in &unknown_ids {
            let (status, body) = call(&router, operation, &json!({ member: unknown })).await;
            assert_eq!(
                status, 404,
                "case {operation} {member}={unknown}: expected HTTP 404, got {status} with {body}"
            );
            assert_eq!(
                text(&body, "__type"),
                "ResourceNotFoundException",
                "case {operation} {member}={unknown}: expected ResourceNotFoundException, got \
                 {body}"
            );
            let actual = message(&body);
            assert_eq!(
                actual,
                format!("{kind} not found"),
                "case {operation} {member}={unknown}: expected the fixed message '{kind} not \
                 found', got {actual:?}"
            );
            assert!(
                !actual.contains(unknown.as_str()),
                "case {operation} {member}={unknown}: the message must carry no part of the \
                 request, got {actual:?}"
            );
            let catalog = text(&found, "catalog");
            assert!(
                catalog.is_empty() || !actual.contains(&catalog),
                "case {operation} {member}={unknown}: the message must not disclose the catalog, \
                 got {actual:?}"
            );
            seen.push(actual);
        }
        assert!(
            seen.windows(2).all(|pair| pair[0] == pair[1]),
            "case {operation}: the message must be identical for every unknown id, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_range_filter_refusal_follows_the_documented_template() {
    let router = router();

    // The two range filter types, from SearchFilter$filterValues: "Range filters
    // (MIN/MAX_AVERAGE_CUSTOMER_RATING) accept a single value between 0.0 and 5.0."
    let range_filters = ["MIN_AVERAGE_CUSTOMER_RATING", "MAX_AVERAGE_CUSTOMER_RATING"];

    // More than one value: refused by count, whatever the values are.
    for filter_type in range_filters {
        for values in [
            vec!["1.0", "2.0"],
            vec!["0.0", "5.0"],
            vec!["3.0", "3.0", "3.0"],
        ] {
            let (status, body) = call(
                &router,
                "searchListings",
                &json!({
                    "filters": [{ "filterType": filter_type, "filterValues": values }],
                    "maxResults": 1,
                }),
            )
            .await;
            let case = format!("{filter_type}={values:?}");
            assert_eq!(
                status, 400,
                "case {case}: a range filter with several values must be refused, got {status} \
                 with {body}"
            );
            assert_eq!(
                text(&body, "__type"),
                "ValidationException",
                "case {case}: expected a ValidationException, got {body}"
            );
            let actual = message(&body);
            assert_eq!(
                actual,
                format!("Range filter {filter_type} requires exactly one value"),
                "case {case}: expected the documented refusal template, got {actual:?}"
            );
        }
    }

    // Outside the documented 0.0-5.0 range: refused by value, which the message carries
    // bare. Rows include one just past each published bound.
    for filter_type in range_filters {
        for value in ["6.0", "5.1", "-0.1", "-1", "100"] {
            let (status, body) = call(
                &router,
                "searchListings",
                &json!({
                    "filters": [{ "filterType": filter_type, "filterValues": [value] }],
                    "maxResults": 1,
                }),
            )
            .await;
            let case = format!("{filter_type}={value}");
            assert_eq!(
                status, 400,
                "case {case}: a rating outside 0.0-5.0 must be refused, got {status} with {body}"
            );
            let actual = message(&body);
            assert_eq!(
                actual,
                format!(
                    "Range filter {filter_type} value must be between 0.0 and 5.0, got: {value}"
                ),
                "case {case}: expected the documented refusal template, got {actual:?}"
            );
        }
    }

    // A value that is not a number is not a rating in range either: same template, and the
    // same error code.
    for filter_type in range_filters {
        // Both rows match the modelled SearchFilterValue pattern ^[\w\-\.]+$, so the
        // boundary validator passes them and the handler is what refuses them.
        for value in ["not-a-number", "NaN"] {
            let (status, body) = call(
                &router,
                "searchListings",
                &json!({
                    "filters": [{ "filterType": filter_type, "filterValues": [value] }],
                    "maxResults": 1,
                }),
            )
            .await;
            let case = format!("{filter_type}={value:?}");
            assert_eq!(
                status, 400,
                "case {case}: a non-numeric range value must be refused, got {status} with {body}"
            );
            let actual = message(&body);
            assert!(
                actual.starts_with(&format!("Range filter {filter_type} ")),
                "case {case}: the refusal must name the offending filterType, got {actual:?}"
            );
        }
    }

    // Positive controls, including both published bounds: a single in-range value is
    // accepted and reports a non-negative totalResults.
    for filter_type in range_filters {
        for value in ["0.0", "1.0", "4.0", "5.0"] {
            let body = ok(
                &router,
                "searchListings",
                &json!({
                    "filters": [{ "filterType": filter_type, "filterValues": [value] }],
                    "maxResults": 100,
                }),
            )
            .await;
            let total = body.get("totalResults").and_then(Value::as_i64);
            assert!(
                total.is_some_and(|total| total >= 0),
                "case {filter_type}={value}: an in-range filter must report a non-negative \
                 totalResults, got {body}"
            );
        }
    }
}
