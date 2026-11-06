// SPDX-License-Identifier: CC0-1.0

//! `GetProducts` answers a filtered query with the offer documents of the products the
//! filters select.
//!
//! The rule under test, over a TABLE of queries rather than one recorded request: for
//! every product the catalog publishes, a filter set naming that product's attribute
//! values is answered with an `aws_v1` offer document that DESCRIBES that product — its
//! own attributes and SKU — priced under the terms it is offered under, with each price
//! dimension stating the rate it charges and the usage it prices; and a matched set
//! longer than the requested page is carried on a `NextToken`.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::pricing_support::{offer_attribute, offer_documents, ok, product_sku, router};

/// A `TERM_MATCH` filter per `(field, value)` pair: the comparison the `FilterType`
/// member documents for an exact value.
fn term_matches(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(field, value)| json!({ "Type": "TERM_MATCH", "Field": field, "Value": value }))
            .collect(),
    )
}

/// The rate a price dimension charges, as its `pricePerUnit.USD` states it.
fn dimension_rate(dimension: &Value) -> String {
    dimension
        .pointer("/pricePerUnit/USD")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The same rate written the way a description writes it: without the trailing zeros of
/// the published ten-place scale, keeping one decimal place.
fn display_rate(rate: &str) -> String {
    let trimmed = rate.trim_end_matches('0');
    if trimmed.ends_with('.') {
        format!("{trimmed}0")
    } else {
        trimmed.to_string()
    }
}

/// One term block's `(offer term code, term)` pairs.
fn terms_of(document: &Value, term_type: &str) -> Vec<(String, Value)> {
    document
        .pointer(&format!("/terms/{term_type}"))
        .and_then(Value::as_object)
        .map(|by_code| {
            by_code
                .iter()
                .map(|(code, term)| (code.clone(), term.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// One term's `(rate code, dimension)` pairs.
fn dimensions_of(term: &Value) -> Vec<(String, Value)> {
    term.get("priceDimensions")
        .and_then(Value::as_object)
        .map(|by_code| {
            by_code
                .iter()
                .map(|(code, dimension)| (code.clone(), dimension.clone()))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_filtered_query_answers_with_the_offer_documents_of_the_products_it_selects() {
    let router = router();

    // Rows differ in service, product family, how many attributes the filter set pins
    // and how many products it selects: a compute instance by instance type and
    // operating system (one per Region), the same instance pinned to one Region, a
    // Windows instance, a storage volume, an object-storage class and a service priced
    // with one usage type per Region.
    let cases: &[(&str, &[(&str, &str)])] = &[
        (
            "AmazonEC2",
            &[
                ("instanceType", "t3.nano"),
                ("operatingSystem", "Linux"),
                ("tenancy", "Shared"),
                ("capacitystatus", "Used"),
                ("preInstalledSw", "NA"),
            ],
        ),
        (
            "AmazonEC2",
            &[
                ("instanceType", "t3.micro"),
                ("operatingSystem", "Linux"),
                ("location", "US East (N. Virginia)"),
            ],
        ),
        (
            "AmazonEC2",
            &[("instanceType", "m5.large"), ("operatingSystem", "Windows")],
        ),
        (
            "AmazonEC2",
            &[("instanceType", "m6g.large"), ("operatingSystem", "Linux")],
        ),
        ("AmazonEC2", &[("volumeApiName", "gp3")]),
        ("AmazonS3", &[("storageClass", "Archive")]),
        ("AWSLambda", &[("regionCode", "us-east-1")]),
    ];

    for (service_code, filters) in cases.iter().copied() {
        let case = format!("{service_code} {filters:?}");
        let response = ok(
            &router,
            "GetProducts",
            &json!({
                "ServiceCode": service_code,
                "Filters": term_matches(filters),
                "MaxResults": 100,
            }),
        )
        .await;
        assert_eq!(
            response.get("FormatVersion").and_then(Value::as_str),
            Some("aws_v1"),
            "case {case}: the answer must report the catalog format version, got {response}"
        );
        let documents = offer_documents(&response);
        assert!(
            !documents.is_empty(),
            "case {case}: a filter set naming values the catalog publishes must select at \
             least one product, got {response}"
        );

        for document in &documents {
            let sku = product_sku(document);
            // The document describes the product the filters selected.
            for (field, value) in filters.iter().copied() {
                assert_eq!(
                    offer_attribute(document, field).as_deref(),
                    Some(value),
                    "case {case}: {sku} must carry {field}={value}, got {document}"
                );
            }
            assert_eq!(
                document.get("serviceCode").and_then(Value::as_str),
                Some(service_code),
                "case {case}: {sku} must name its service, got {document}"
            );
            assert!(
                document
                    .pointer("/product/productFamily")
                    .and_then(Value::as_str)
                    .is_some_and(|family| !family.is_empty()),
                "case {case}: {sku} must report a product family, got {document}"
            );

            // Every term prices THIS product, is keyed by the SKU and its own offer term
            // code, and each of its dimensions states the rate it charges, keyed by the
            // rate code derived from the term's key.
            let term_types: BTreeSet<String> = document
                .get("terms")
                .and_then(Value::as_object)
                .map(|terms| terms.keys().cloned().collect())
                .unwrap_or_default();
            assert!(
                term_types.contains("OnDemand"),
                "case {case}: {sku} must be priced on demand, got {term_types:?}"
            );
            for term_type in &term_types {
                for (code, term) in terms_of(document, term_type) {
                    assert_eq!(
                        term.get("sku").and_then(Value::as_str),
                        Some(sku.as_str()),
                        "case {case}: {term_type} term {code} must price {sku}, got {term}"
                    );
                    let offer_term_code = term
                        .get("offerTermCode")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    assert_eq!(
                        code,
                        format!("{sku}.{offer_term_code}"),
                        "case {case}: a term is keyed by SKU and offer term code, got {code}"
                    );
                    let dimensions = dimensions_of(&term);
                    assert!(
                        !dimensions.is_empty(),
                        "case {case}: {term_type} term {code} must carry price dimensions, \
                         got {term}"
                    );
                    for (rate_code, dimension) in dimensions {
                        assert!(
                            rate_code.starts_with(&format!("{code}.")),
                            "case {case}: a rate code extends its term key, got {rate_code}"
                        );
                        assert_eq!(
                            dimension.get("rateCode").and_then(Value::as_str),
                            Some(rate_code.as_str()),
                            "case {case}: a dimension must state its own rate code, got \
                             {dimension}"
                        );
                        assert!(
                            dimension
                                .get("unit")
                                .and_then(Value::as_str)
                                .is_some_and(|unit| !unit.is_empty()),
                            "case {case}: {rate_code} must state the unit it prices, got \
                             {dimension}"
                        );
                        assert!(
                            dimension_rate(&dimension).parse::<f64>().is_ok(),
                            "case {case}: {rate_code} must state a USD rate, got {dimension}"
                        );
                    }
                }
            }
        }

        // A read does not change what a later caller sees.
        let again = ok(
            &router,
            "GetProducts",
            &json!({
                "ServiceCode": service_code,
                "Filters": term_matches(filters),
                "MaxResults": 100,
            }),
        )
        .await;
        assert_eq!(
            response, again,
            "case {case}: the same query must answer the same way"
        );
    }
}

#[tokio::test]
async fn a_compute_offer_prices_the_instance_it_describes() {
    let router = router();

    // Rows differ in instance family, size and operating system. `factor` is the
    // normalized-unit factor AWS publishes for the size (the documented table: nano
    // 0.25, micro 0.5, small 1, medium 2, large 4, xlarge 8, and `<n>xlarge` n times
    // the xlarge factor) — including sizes beyond the ones the queries above name.
    let cases: &[(&str, &str, &str)] = &[
        ("t3.nano", "Linux", "0.25"),
        ("t3.micro", "Linux", "0.5"),
        ("t3.small", "Linux", "1"),
        ("t3.medium", "Linux", "2"),
        ("m5.large", "Linux", "4"),
        ("m5.xlarge", "Linux", "8"),
        ("m5.2xlarge", "Linux", "16"),
        ("m5.large", "Windows", "4"),
        ("t3.nano", "Windows", "0.25"),
    ];

    for (instance_type, operating_system, factor) in cases.iter().copied() {
        let case = format!("{instance_type}/{operating_system}");
        let response = ok(
            &router,
            "GetProducts",
            &json!({
                "ServiceCode": "AmazonEC2",
                "Filters": term_matches(&[
                    ("instanceType", instance_type),
                    ("operatingSystem", operating_system),
                    ("location", "US East (N. Virginia)"),
                    ("tenancy", "Shared"),
                    ("capacitystatus", "Used"),
                    ("preInstalledSw", "NA"),
                ]),
            }),
        )
        .await;
        let documents = offer_documents(&response);
        assert_eq!(
            documents.len(),
            1,
            "case {case}: the filter set must select exactly one priced SKU, got {}",
            documents.len()
        );
        let document = &documents[0];
        let sku = product_sku(document);

        assert_eq!(
            offer_attribute(document, "normalizationSizeFactor").as_deref(),
            Some(factor),
            "case {case}: expected the published normalization factor {factor}, got \
             {document}"
        );
        assert_eq!(
            offer_attribute(document, "usagetype").as_deref(),
            Some(format!("BoxUsage:{instance_type}").as_str()),
            "case {case}: the usage type must name the instance type, got {document}"
        );
        assert_eq!(
            document
                .pointer("/product/productFamily")
                .and_then(Value::as_str),
            Some("Compute Instance"),
            "case {case}: a priced instance is a compute instance, got {document}"
        );

        // The on-demand term is charged per instance hour, and its description states
        // the rate it charges and the instance it prices.
        let on_demand = terms_of(document, "OnDemand");
        assert_eq!(
            on_demand.len(),
            1,
            "case {case}: an instance is offered on demand once, got {on_demand:?}"
        );
        let dimensions = dimensions_of(&on_demand[0].1);
        assert_eq!(
            dimensions.len(),
            1,
            "case {case}: the on-demand term prices one dimension, got {dimensions:?}"
        );
        let (_, dimension) = &dimensions[0];
        assert_eq!(
            dimension.get("unit").and_then(Value::as_str),
            Some("Hrs"),
            "case {case}: on-demand instance usage is priced per hour, got {dimension}"
        );
        let rate = dimension_rate(dimension);
        assert!(
            rate.parse::<f64>().is_ok_and(|rate| rate > 0.0),
            "case {case}: expected a positive on-demand rate, got {rate:?}"
        );
        assert_eq!(
            dimension.get("description").and_then(Value::as_str),
            Some(
                format!(
                    "${} per On Demand {operating_system} {instance_type} Instance Hour",
                    display_rate(&rate)
                )
                .as_str()
            ),
            "case {case}: the description must state the rate and the instance it prices, \
             got {dimension}"
        );

        // The reservations it is also offered under price the same usage below the
        // on-demand rate, and an option that pays part of the lease up front states that
        // fee as its own dimension.
        let reserved = terms_of(document, "Reserved");
        assert!(
            reserved.len() > 1,
            "case {case}: an instance is offered under more than one reservation, got \
             {reserved:?}"
        );
        let on_demand_rate: f64 = rate.parse().expect("a parsable on-demand rate");
        for (code, term) in &reserved {
            let option = term
                .pointer("/termAttributes/PurchaseOption")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let lease = term
                .pointer("/termAttributes/LeaseContractLength")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                lease == "1yr" || lease == "3yr",
                "case {case}/{code}: expected a published lease length, got {lease:?}"
            );
            let mut usage_dimensions = 0usize;
            let mut fee_dimensions = 0usize;
            for (rate_code, dimension) in dimensions_of(term) {
                let unit = dimension
                    .get("unit")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let amount: f64 = dimension_rate(&dimension)
                    .parse()
                    .expect("a parsable reserved rate");
                if unit == "Hrs" {
                    usage_dimensions += 1;
                    assert!(
                        amount < on_demand_rate,
                        "case {case}/{code}: a reservation must price usage below the \
                         on-demand {on_demand_rate}, got {amount}"
                    );
                } else {
                    fee_dimensions += 1;
                    assert_eq!(
                        unit, "Quantity",
                        "case {case}/{rate_code}: a one-off fee is a quantity, got {dimension}"
                    );
                    assert_eq!(
                        dimension.get("description").and_then(Value::as_str),
                        Some("Upfront Fee"),
                        "case {case}/{rate_code}: expected the upfront-fee description, got \
                         {dimension}"
                    );
                    assert!(
                        amount > 0.0,
                        "case {case}/{rate_code}: an upfront fee must be positive, got {amount}"
                    );
                }
            }
            assert_eq!(
                usage_dimensions, 1,
                "case {case}/{code}: a reserved term prices its usage once"
            );
            assert_eq!(
                fee_dimensions,
                usize::from(option != "No Upfront"),
                "case {case}/{code}: {option} must carry a fee dimension only when it pays \
                 one"
            );
        }
        assert_eq!(
            document
                .pointer("/terms/Reserved")
                .and_then(Value::as_object)
                .map(serde_json::Map::len),
            Some(reserved.len()),
            "case {case}: {sku} must publish each reservation once"
        );
    }
}

#[tokio::test]
async fn a_matched_set_longer_than_the_page_is_carried_on_a_next_token() {
    let router = router();

    // The instance-vocabulary query: an instance type the catalog prices in every
    // Region, so the matched set spans several pages. Rows differ in page size,
    // including the published bounds of MaxResults (1..100) and the whole set in one
    // page.
    let filters = term_matches(&[
        ("instanceType", "t3.nano"),
        ("operatingSystem", "Linux"),
        ("tenancy", "Shared"),
        ("capacitystatus", "Used"),
        ("preInstalledSw", "NA"),
    ]);

    let whole = ok(
        &router,
        "GetProducts",
        &json!({ "ServiceCode": "AmazonEC2", "Filters": filters, "MaxResults": 100 }),
    )
    .await;
    let all: Vec<String> = offer_documents(&whole).iter().map(product_sku).collect();
    assert!(
        all.len() > 10,
        "the catalog must price this instance type in more Regions than one small page, \
         got {}",
        all.len()
    );
    assert!(
        whole.get("NextToken").is_none_or(Value::is_null),
        "case MaxResults=100: a single page must carry no NextToken, got {whole}"
    );

    for page_size in [1_i64, 2, 5, 10] {
        let mut collected: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut body = json!({
                "ServiceCode": "AmazonEC2",
                "Filters": filters,
                "MaxResults": page_size,
            });
            if let Some(token) = &next_token {
                body["NextToken"] = json!(token);
            }
            let page = ok(&router, "GetProducts", &body).await;
            let skus: Vec<String> = offer_documents(&page).iter().map(product_sku).collect();
            assert!(
                skus.len() <= usize::try_from(page_size).expect("a small page size"),
                "case MaxResults={page_size} page {pages}: a page must not exceed the page \
                 size, got {}",
                skus.len()
            );
            collected.extend(skus);
            pages += 1;
            next_token = page
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            // The token is present for exactly as long as products remain unreturned.
            assert_eq!(
                next_token.is_some(),
                collected.len() < all.len(),
                "case MaxResults={page_size} page {pages}: expected a NextToken while \
                 {} of {} products remain, got {page}",
                all.len().saturating_sub(collected.len()),
                all.len()
            );
            if next_token.is_none() || pages > all.len() {
                break;
            }
        }
        assert_eq!(
            collected, all,
            "case MaxResults={page_size}: paging must yield every matched product once, in \
             order"
        );
    }
}
