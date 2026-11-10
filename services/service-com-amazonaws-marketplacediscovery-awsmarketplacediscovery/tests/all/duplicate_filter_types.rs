// SPDX-License-Identifier: CC0-1.0

//! A `filters` list that names one `filterType` twice.
//!
//! THE RULE UNDER TEST: a filter entry of this service carries the values to filter its type
//! by, and both filter shapes document those values as OR-combined —
//! `SearchFilter$filterValues` "Term filters accept multiple values (OR logic)" and
//! `PurchaseOptionFilter$filterValues` "OR logic combines multiple values within the same
//! filter". So a `filterType` may appear at most ONCE per request: a list that repeats one
//! is refused with the modelled `ValidationException` (HTTP 400) and the message
//! `Duplicate filter types: [<the repeated types>]`, while a list naming each type once is
//! served. The rule is independent of a list's own `@length` bound (1..30 for a search,
//! 1..10 for purchase options): two entries are well inside either.
//!
//! The rows span what the rule generalises over: the operation (every one of the three that
//! carries a `filters` member), the filter type repeated, whether the repeated entries carry
//! the same values or different ones, the POSITION of the repeat in the list, the number of
//! repeats of one type, and a list repeating two types at once.
//!
//! Every request goes through the `MultiServiceRouter`, so the protocol, the routing table
//! and the handlers are all exercised end to end.

use serde_json::{json, Value};

use crate::catalog_drilldown::{array, call, ok, router, text};
use crate::validation_message_rules::refused;

/// One `filters` entry.
fn filter(filter_type: &str, values: &[&str]) -> Value {
    json!({ "filterType": filter_type, "filterValues": values })
}

/// A request body carrying `filters` for `operation`, with the `maxResults`
/// `SearchListings` needs to page a reply.
fn with_filters(operation: &str, filters: Vec<Value>) -> Value {
    let mut input = json!({ "filters": filters });
    if operation == "searchListings" {
        input["maxResults"] = json!(5);
    }
    input
}

/// The rows the refusal is asserted over: `(case, operation, the filters list, the types the
/// message must name)`.
fn duplicate_cases() -> Vec<(&'static str, &'static str, Vec<Value>, &'static str)> {
    vec![
        (
            "SearchListings repeats CATEGORY with two different values",
            "searchListings",
            vec![
                filter("CATEGORY", &["security"]),
                filter("CATEGORY", &["storage"]),
            ],
            "CATEGORY",
        ),
        (
            "SearchListings repeats CATEGORY with the SAME value",
            "searchListings",
            vec![
                filter("CATEGORY", &["security"]),
                filter("CATEGORY", &["security"]),
            ],
            "CATEGORY",
        ),
        (
            "SearchListings repeats PUBLISHER after a distinct entry",
            "searchListings",
            vec![
                filter("CATEGORY", &["security"]),
                filter("PUBLISHER", &["sp-aurora"]),
                filter("PUBLISHER", &["sp-cirrus"]),
            ],
            "PUBLISHER",
        ),
        (
            "SearchListings names PRICING_MODEL three times",
            "searchListings",
            vec![
                filter("PRICING_MODEL", &["USAGE"]),
                filter("PRICING_MODEL", &["CONTRACT"]),
                filter("PRICING_MODEL", &["FREE"]),
            ],
            "PRICING_MODEL",
        ),
        (
            "SearchListings repeats a RANGE filter",
            "searchListings",
            vec![
                filter("MIN_AVERAGE_CUSTOMER_RATING", &["3.0"]),
                filter("MIN_AVERAGE_CUSTOMER_RATING", &["4.0"]),
            ],
            "MIN_AVERAGE_CUSTOMER_RATING",
        ),
        (
            "SearchListings repeats TWO types at once",
            "searchListings",
            vec![
                filter("CATEGORY", &["security"]),
                filter("PUBLISHER", &["sp-aurora"]),
                filter("CATEGORY", &["storage"]),
                filter("PUBLISHER", &["sp-cirrus"]),
            ],
            "CATEGORY, PUBLISHER",
        ),
        (
            "SearchFacets repeats FULFILLMENT_OPTION_TYPE",
            "searchFacets",
            vec![
                filter("FULFILLMENT_OPTION_TYPE", &["CONTAINER"]),
                filter("FULFILLMENT_OPTION_TYPE", &["HELM"]),
            ],
            "FULFILLMENT_OPTION_TYPE",
        ),
        (
            "SearchFacets repeats DEPLOYED_ON_AWS with the same value",
            "searchFacets",
            vec![
                filter("DEPLOYED_ON_AWS", &["DEPLOYED"]),
                filter("DEPLOYED_ON_AWS", &["DEPLOYED"]),
            ],
            "DEPLOYED_ON_AWS",
        ),
        (
            "ListPurchaseOptions repeats PRODUCT_ID",
            "listPurchaseOptions",
            vec![
                filter("PRODUCT_ID", &["p-0001-analytics"]),
                filter("PRODUCT_ID", &["p-0003-firewall"]),
            ],
            "PRODUCT_ID",
        ),
        (
            "ListPurchaseOptions repeats AVAILABILITY_STATUS after a distinct entry",
            "listPurchaseOptions",
            vec![
                filter("PURCHASE_OPTION_TYPE", &["OFFER"]),
                filter("AVAILABILITY_STATUS", &["AVAILABLE"]),
                filter("AVAILABILITY_STATUS", &["EXPIRED"]),
            ],
            "AVAILABILITY_STATUS",
        ),
    ]
}

#[tokio::test]
async fn a_repeated_filter_type_is_refused_naming_the_types_it_repeats() {
    let router = router();
    for (case, operation, filters, types) in duplicate_cases() {
        let input = with_filters(operation, filters);
        let message = refused(&router, case, operation, &input).await;
        let expected = format!("Duplicate filter types: [{types}]");
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}

/// The positive control, over the same three operations: a list naming each `filterType`
/// ONCE is served, however many entries it holds and however many values one entry carries —
/// so the refusal above is about the repeat and not about filtering at all.
#[tokio::test]
async fn a_list_naming_each_filter_type_once_is_served() {
    let router = router();

    // Rows: one entry, several distinct entries, an entry carrying several values (the OR
    // the rule says belongs there), and the range filters beside term filters.
    let cases: Vec<(&str, &str, Vec<Value>)> = vec![
        (
            "SearchListings with one entry",
            "searchListings",
            vec![filter("CATEGORY", &["security"])],
        ),
        (
            "SearchListings ORs two values inside ONE entry",
            "searchListings",
            vec![filter("CATEGORY", &["security", "storage"])],
        ),
        (
            "SearchListings with four distinct types, both range filters among them",
            "searchListings",
            vec![
                filter("CATEGORY", &["security"]),
                filter("PRICING_MODEL", &["USAGE", "CONTRACT"]),
                filter("MIN_AVERAGE_CUSTOMER_RATING", &["1.0"]),
                filter("MAX_AVERAGE_CUSTOMER_RATING", &["5.0"]),
            ],
        ),
        (
            "SearchFacets with two distinct types",
            "searchFacets",
            vec![
                filter("FULFILLMENT_OPTION_TYPE", &["CONTAINER", "HELM"]),
                filter("DEPLOYED_ON_AWS", &["DEPLOYED"]),
            ],
        ),
        (
            "ListPurchaseOptions with two distinct types",
            "listPurchaseOptions",
            vec![
                filter("PURCHASE_OPTION_TYPE", &["OFFER"]),
                filter("AVAILABILITY_STATUS", &["AVAILABLE"]),
            ],
        ),
    ];
    for (case, operation, filters) in cases {
        let input = with_filters(operation, filters);
        let (status, body) = call(&router, operation, &input).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {body}"
        );
    }
}

/// The listing ids one `PUBLISHER` entry carrying `values` selects.
async fn listings_by_publisher(
    router: &smithy_server_runtime::MultiServiceRouter,
    values: &[&str],
) -> std::collections::BTreeSet<String> {
    let body = ok(
        router,
        "searchListings",
        &json!({
            "maxResults": 100,
            "filters": [{ "filterType": "PUBLISHER", "filterValues": values }],
        }),
    )
    .await;
    array(&body, "listingSummaries")
        .iter()
        .map(|summary| text(summary, "listingId"))
        .collect()
}

/// The OR a repeated type tried to express belongs inside ONE entry, and there it WORKS: the
/// listings one entry naming two values selects are exactly the union of what each value
/// selects on its own.
#[tokio::test]
async fn one_entry_with_two_values_selects_the_union_of_both() {
    let router = router();

    // The two most common publishers of the catalog, read off the facet counts rather than
    // named, so the case is not pinned to one seeded record.
    let facets = ok(&router, "searchFacets", &json!({ "facetTypes": ["PUBLISHER"] })).await;
    let publishers: Vec<String> = facets
        .get("listingFacets")
        .and_then(|map| map.get("PUBLISHER"))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .map(|facet| text(facet, "value"))
                .take(2)
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        publishers.len(),
        2,
        "case setup: the catalog must report two publishers, got {facets}"
    );

    let first = listings_by_publisher(&router, &[publishers[0].as_str()]).await;
    let second = listings_by_publisher(&router, &[publishers[1].as_str()]).await;
    let both = listings_by_publisher(
        &router,
        &[publishers[0].as_str(), publishers[1].as_str()],
    )
    .await;
    let union: std::collections::BTreeSet<String> = first.union(&second).cloned().collect();
    assert_eq!(
        both, union,
        "case PUBLISHER with two values: one entry must select the union of both values"
    );
    assert!(
        both.len() > first.len(),
        "case setup: the two publishers must select different listings, got {both:?}"
    );
}
