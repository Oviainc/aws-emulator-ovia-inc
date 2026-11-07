// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Price List catalog, driven over the wire.
//!
//! The assertions are RULES over a table of inputs, not one recorded case:
//! `DescribeServices` names the services the catalog holds, `GetAttributeValues`
//! returns the distinct values the SAME products carry, each `Filter` type selects
//! exactly the products its documented comparison selects, a page boundary is carried
//! by an opaque `NextToken`, and the `PriceListArn` `ListPriceLists` reports resolves
//! to a stable URL serving a file rendered from the same catalog.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_pricing_awspricelistservice::state::{
    render_price_list_file, PricingState,
};

use super::pricing_support::{
    attribute_values, call, err, is_loopback, list_member, offer_attribute, offer_documents, ok,
    product_sku, product_skus, router, router_with_engine, service_codes, service_entry,
    split_https_url, ARN_PREFIX, JAN_1_2022, JAN_1_2024, JUL_1_2024,
};

#[tokio::test]
async fn describe_services_reports_the_catalog_and_its_attribute_names() {
    let router = router();

    // Every seeded service, with its format version.
    let all = ok(&router, "DescribeServices", &json!({})).await;
    assert_eq!(
        all.get("FormatVersion").and_then(Value::as_str),
        Some("aws_v1"),
        "unfiltered DescribeServices must report the catalog format version, got {all}"
    );
    let codes = service_codes(&all);
    assert!(
        codes.contains(&"AmazonEC2".to_string()) && codes.contains(&"AmazonS3".to_string()),
        "case unfiltered: expected the seeded services, got {codes:?}"
    );

    // With a ServiceCode: exactly that service, with the attribute names the catalog
    // holds for it — and each is the same entry the unfiltered call reported.
    for service_code in &codes {
        let one = ok(
            &router,
            "DescribeServices",
            &json!({ "ServiceCode": service_code }),
        )
        .await;
        let services = list_member(&one, "Services");
        assert_eq!(
            services.len(),
            1,
            "case {service_code}: expected exactly one Service, got {one}"
        );
        assert_eq!(
            Some(services[0].clone()),
            service_entry(&all, service_code),
            "case {service_code}: the filtered entry must equal the unfiltered one"
        );
        let names: Vec<String> = list_member(&services[0], "AttributeNames")
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        assert!(
            !names.is_empty(),
            "case {service_code}: expected a non-empty AttributeNames, got {one}"
        );

        // Every attribute name reported must have values, and those values must be the
        // ones the service's own products carry.
        let products = ok(
            &router,
            "GetProducts",
            &json!({ "ServiceCode": service_code }),
        )
        .await;
        let documents = offer_documents(&products);
        for name in &names {
            let values = attribute_values(
                &ok(
                    &router,
                    "GetAttributeValues",
                    &json!({ "ServiceCode": service_code, "AttributeName": name }),
                )
                .await,
            );
            assert!(
                !values.is_empty(),
                "case {service_code}/{name}: an attribute DescribeServices reports must \
                 have values, got {values:?}"
            );
            let from_products: BTreeSet<String> = documents
                .iter()
                .filter_map(|document| offer_attribute(document, name))
                .collect();
            assert_eq!(
                values.iter().cloned().collect::<BTreeSet<String>>(),
                from_products,
                "case {service_code}/{name}: GetAttributeValues must return the values \
                 the products GetProducts returns actually carry"
            );
        }
    }

    // A read leaves the catalog as it was: the same call answers the same way.
    let again = ok(&router, "DescribeServices", &json!({})).await;
    assert_eq!(
        all, again,
        "DescribeServices must not change what a later caller sees"
    );

    // The catalog is published in one format version; another one is a bad parameter.
    err(
        &router,
        "DescribeServices",
        &json!({ "FormatVersion": "aws_v2" }),
        400,
        "InvalidParameterException",
    )
    .await;
}

#[tokio::test]
async fn get_attribute_values_returns_the_distinct_values_of_the_attribute() {
    let router = router();

    // Rows differ in service, attribute and the number of distinct values the seeded
    // products carry for it; the expected sets come from those products, which the
    // previous test pins against GetProducts. Each list is sorted, which is the order
    // the catalog reports its values in.
    let cases: &[(&str, &str, &[&str])] = &[
        (
            "AmazonEC2",
            "instanceType",
            &[
                "c5.large",
                "c5.xlarge",
                "m5.2xlarge",
                "m5.large",
                "m5.xlarge",
                "m6g.large",
                "r5.large",
                "r5.xlarge",
                "t2.medium",
                "t2.micro",
                "t2.small",
                "t3.large",
                "t3.medium",
                "t3.micro",
                "t3.nano",
                "t3.small",
            ],
        ),
        ("AmazonEC2", "operatingSystem", &["Linux", "Windows"]),
        (
            // The EBS volume-type labels the Price List API publishes for EC2.
            "AmazonEC2",
            "volumeType",
            &[
                "Cold HDD",
                "General Purpose",
                "Magnetic",
                "Provisioned IOPS",
                "Throughput Optimized HDD",
            ],
        ),
        ("AmazonEC2", "locationType", &["AWS Region"]),
        ("AmazonEC2", "tenancy", &["Shared"]),
        (
            "AmazonS3",
            "storageClass",
            &["Archive", "General Purpose", "Infrequent Access"],
        ),
        (
            "AmazonS3",
            "regionCode",
            &["ap-northeast-1", "eu-west-1", "us-east-1", "us-west-2"],
        ),
        (
            "AmazonSNS",
            "locationType",
            &["AWS Region"],
        ),
    ];

    for (service_code, attribute_name, expected) in cases.iter().copied() {
        let response = ok(
            &router,
            "GetAttributeValues",
            &json!({ "ServiceCode": service_code, "AttributeName": attribute_name }),
        )
        .await;
        let actual = attribute_values(&response);
        assert_eq!(
            actual,
            expected.iter().map(|v| (*v).to_string()).collect::<Vec<_>>(),
            "case {service_code}/{attribute_name}: expected {expected:?}, got {actual:?}"
        );
        assert!(
            response.get("NextToken").is_none() || response["NextToken"].is_null(),
            "case {service_code}/{attribute_name}: a single page must carry no NextToken, \
             got {response}"
        );

        // Each value selects at least one product, and every product it selects carries
        // exactly that value.
        for value in &actual {
            let products = ok(
                &router,
                "GetProducts",
                &json!({
                    "ServiceCode": service_code,
                    "Filters": [{ "Type": "TERM_MATCH", "Field": attribute_name, "Value": value }],
                }),
            )
            .await;
            let documents = offer_documents(&products);
            assert!(
                !documents.is_empty(),
                "case {service_code}/{attribute_name}={value}: expected at least one \
                 product, got {products}"
            );
            for document in &documents {
                assert_eq!(
                    offer_attribute(document, attribute_name).as_deref(),
                    Some(value.as_str()),
                    "case {service_code}/{attribute_name}={value}: a filtered product must \
                     carry the filtered value, got {document}"
                );
            }
        }
    }

    // An unknown service, and an attribute the service does not publish: both SELECT no
    // value, which is an empty list rather than a refusal (see
    // `a_selection_that_matches_nothing_is_an_empty_page`).
    for body in [
        json!({ "ServiceCode": "NoSuchService", "AttributeName": "instanceType" }),
        json!({ "ServiceCode": "AmazonEC2", "AttributeName": "noSuchAttribute" }),
    ] {
        let response = ok(&router, "GetAttributeValues", &body).await;
        let actual = attribute_values(&response);
        assert!(
            actual.is_empty(),
            "case {body}: expected no values, got {actual:?}"
        );
    }
}

/// Whether `document` satisfies the filter, by the comparison the `FilterType` member
/// documents: `TERM_MATCH`/`EQUALS` an exact value, `CONTAINS` a substring, and
/// `ANY_OF`/`NONE_OF` membership of the comma-separated list. `ServiceCode` is the
/// pseudo-field the `Filter.Field` doc names beside the attribute names.
fn selects(filter_type: &str, field: &str, value: &str, document: &Value) -> bool {
    let field_value = if field == "ServiceCode" {
        document
            .get("serviceCode")
            .and_then(Value::as_str)
            .map(str::to_string)
    } else {
        offer_attribute(document, field)
    };
    let field_value = field_value.as_deref();
    let listed = |candidate: &str| {
        value
            .split(',')
            .map(str::trim)
            .any(|alternative| alternative == candidate)
    };
    match filter_type {
        "TERM_MATCH" | "EQUALS" => field_value == Some(value),
        "CONTAINS" => field_value.is_some_and(|actual| actual.contains(value)),
        "ANY_OF" => field_value.is_some_and(listed),
        "NONE_OF" => field_value.is_none_or(|actual| !listed(actual)),
        other => panic!("case {field}={value}: unexpected filter type {other}"),
    }
}

#[tokio::test]
async fn get_products_applies_every_filter_type() {
    let router = router();

    let unfiltered = ok(
        &router,
        "GetProducts",
        &json!({ "ServiceCode": "AmazonEC2" }),
    )
    .await;
    assert_eq!(
        unfiltered.get("FormatVersion").and_then(Value::as_str),
        Some("aws_v1"),
        "GetProducts must echo the catalog format version, got {unfiltered}"
    );
    let documents = offer_documents(&unfiltered);

    // One row per filter type the model declares (FilterType: TERM_MATCH, EQUALS,
    // CONTAINS, ANY_OF, NONE_OF), over fields that differ in how many products they
    // select, plus the `ServiceCode` pseudo-field. `min_matches` is what the row must
    // actually exercise, so a row can never pass by matching nothing by accident.
    let cases: &[(&str, &str, &str, usize)] = &[
        ("TERM_MATCH", "instanceType", "t2.micro", 1),
        ("TERM_MATCH", "location", "Asia Pacific (Tokyo)", 3),
        ("EQUALS", "operatingSystem", "Windows", 2),
        ("EQUALS", "volumeApiName", "gp3", 3),
        ("CONTAINS", "usagetype", "BoxUsage:t3", 4),
        ("CONTAINS", "volumeType", "HDD", 2),
        ("ANY_OF", "instanceType", "t2.micro,t2.small,t2.medium", 3),
        ("ANY_OF", "regionCode", "eu-west-1,ap-northeast-1", 5),
        ("NONE_OF", "instanceType", "t2.micro,t2.small", 10),
        ("NONE_OF", "operatingSystem", "Windows", 10),
        ("TERM_MATCH", "ServiceCode", "AmazonEC2", 20),
        // A filter on another service's code selects nothing in this one.
        ("TERM_MATCH", "ServiceCode", "AmazonS3", 0),
    ];

    for (filter_type, field, value, min_matches) in cases.iter().copied() {
        // The documented comparison, applied here to the unfiltered result set: the
        // handler must return exactly the products it selects, no more and no fewer.
        let expected: BTreeSet<String> = documents
            .iter()
            .filter(|document| selects(filter_type, field, value, document))
            .map(product_sku)
            .collect();
        assert!(
            expected.len() >= min_matches,
            "case {filter_type} {field}={value}: the catalog must exercise this filter, \
             only {} of the {} products match",
            expected.len(),
            documents.len()
        );
        let response = ok(
            &router,
            "GetProducts",
            &json!({
                "ServiceCode": "AmazonEC2",
                "Filters": [{ "Type": filter_type, "Field": field, "Value": value }],
            }),
        )
        .await;
        let actual: BTreeSet<String> = product_skus(&response).into_iter().collect();
        assert_eq!(
            actual, expected,
            "case {filter_type} {field}={value}: expected {expected:?}, got {actual:?}"
        );
    }

    // Two filters that must BOTH hold select the intersection.
    let both = ok(
        &router,
        "GetProducts",
        &json!({
            "ServiceCode": "AmazonEC2",
            "Filters": [
                { "Type": "TERM_MATCH", "Field": "instanceType", "Value": "m5.large" },
                { "Type": "TERM_MATCH", "Field": "operatingSystem", "Value": "Windows" },
            ],
        }),
    )
    .await;
    let both: BTreeSet<String> = product_skus(&both).into_iter().collect();
    let expected: BTreeSet<String> = documents
        .iter()
        .filter(|document| {
            selects("TERM_MATCH", "instanceType", "m5.large", document)
                && selects("TERM_MATCH", "operatingSystem", "Windows", document)
        })
        .map(product_sku)
        .collect();
    assert!(
        !expected.is_empty(),
        "the catalog must hold a Windows m5.large for this row to mean anything"
    );
    assert_eq!(
        both, expected,
        "case two filters: expected {expected:?}, got {both:?}"
    );

    // A ServiceCode the catalog publishes no offers for selects no product, which is an
    // empty PriceList rather than a refusal.
    let unknown = ok(
        &router,
        "GetProducts",
        &json!({ "ServiceCode": "NoSuchService" }),
    )
    .await;
    assert!(
        list_member(&unknown, "PriceList").is_empty(),
        "case an unknown ServiceCode: expected an empty PriceList, got {unknown}"
    );
    err(
        &router,
        "GetProducts",
        &json!({ "ServiceCode": "AmazonEC2", "FormatVersion": "aws_v2" }),
        400,
        "InvalidParameterException",
    )
    .await;
}

#[tokio::test]
async fn paginated_reads_walk_the_whole_result_set_once() {
    let router = router();

    // Rows differ in operation, page size and result-set size; each must yield every
    // item of the unpaged call exactly once, in the same order.
    let cases: &[(&str, Value, i64, &str)] = &[
        (
            "GetProducts",
            json!({ "ServiceCode": "AmazonS3" }),
            1,
            "PriceList",
        ),
        (
            "GetProducts",
            json!({ "ServiceCode": "AmazonS3" }),
            5,
            "PriceList",
        ),
        ("DescribeServices", json!({}), 25, "Services"),
        (
            "GetAttributeValues",
            json!({ "ServiceCode": "AmazonEC2", "AttributeName": "instanceType" }),
            2,
            "AttributeValues",
        ),
        (
            "ListPriceLists",
            json!({
                "ServiceCode": "AmazonS3",
                "CurrencyCode": "USD",
                "EffectiveDate": JUL_1_2024,
            }),
            3,
            "PriceLists",
        ),
    ];

    for (operation, mut body, page_size, items_member) in cases.iter().cloned() {
        let unpaged = ok(&router, operation, &body).await;
        let expected = list_member(&unpaged, items_member);
        assert!(
            expected.len() > usize::try_from(page_size).expect("a small page size"),
            "case {operation}: the test needs more items than one page to page over, got \
             {}",
            expected.len()
        );

        let mut collected: Vec<Value> = Vec::new();
        let mut next_token: Option<String> = None;
        // Bounded so a token that never advances fails the test instead of hanging.
        for _ in 0..=expected.len() {
            body["MaxResults"] = json!(page_size);
            match &next_token {
                Some(token) => body["NextToken"] = json!(token),
                None => {
                    body.as_object_mut()
                        .expect("an object body")
                        .remove("NextToken");
                }
            }
            let page = ok(&router, operation, &body).await;
            let items = list_member(&page, items_member);
            assert!(
                items.len() <= usize::try_from(page_size).expect("a small page size"),
                "case {operation} page size {page_size}: a page must not exceed \
                 MaxResults, got {}",
                items.len()
            );
            collected.extend(items);
            next_token = page
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if next_token.is_none() {
                break;
            }
        }
        assert!(
            next_token.is_none(),
            "case {operation} page size {page_size}: paging must terminate, still holding \
             {next_token:?}"
        );
        assert_eq!(
            collected, expected,
            "case {operation} page size {page_size}: paging must yield every item once, \
             in order"
        );
    }

    // A token minted for one query cannot be replayed against another: the same offset
    // would name an unrelated item.
    let first_page = ok(
        &router,
        "GetProducts",
        &json!({ "ServiceCode": "AmazonEC2", "MaxResults": 1 }),
    )
    .await;
    let ec2_token = first_page
        .get("NextToken")
        .and_then(Value::as_str)
        .expect("a first page of one has a NextToken")
        .to_string();
    let (status, parsed) = call(
        &router,
        "GetProducts",
        &json!({ "ServiceCode": "AmazonS3", "NextToken": ec2_token }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case token minted for another query: expected HTTP 400, got {status} with {parsed}"
    );
    assert_eq!(
        parsed.get("__type").and_then(Value::as_str),
        Some("InvalidNextTokenException"),
        "case token minted for another query: expected InvalidNextTokenException, got \
         {parsed}"
    );
}

#[tokio::test]
async fn list_price_lists_selects_by_service_currency_region_and_effective_date() {
    let router = router();

    // Rows differ in the member that selects: service, currency, Region and the
    // effective instant. `expect_any` says whether the catalog publishes anything for
    // the row at all.
    let cases: &[(&str, &str, Option<&str>, i64, bool)] = &[
        ("AmazonEC2", "USD", None, JUL_1_2024, true),
        ("AmazonEC2", "USD", Some("eu-west-1"), JUL_1_2024, true),
        ("AmazonEC2", "USD", Some("ap-northeast-1"), JAN_1_2024, true),
        ("AmazonEC2", "CNY", Some("cn-north-1"), JUL_1_2024, true),
        ("AmazonS3", "USD", None, JAN_1_2024, true),
        // The service the catalog prices with one usage type per Region is just as
        // reachable as the two it prices in depth.
        ("AmazonSNS", "USD", Some("us-east-1"), JUL_1_2024, true),
        // No version is in effect before the earliest one.
        ("AmazonEC2", "USD", None, JAN_1_2022, false),
        // A Region the catalog publishes nothing in, and a currency/Region mismatch.
        ("AmazonEC2", "USD", Some("us-east-9"), JUL_1_2024, false),
        ("AmazonEC2", "CNY", Some("us-east-1"), JUL_1_2024, false),
    ];

    for (service_code, currency, region, effective, expect_any) in cases.iter().copied() {
        let mut body = json!({
            "ServiceCode": service_code,
            "CurrencyCode": currency,
            "EffectiveDate": effective,
        });
        if let Some(region) = region {
            body["RegionCode"] = json!(region);
        }
        let case = format!("{service_code}/{currency}/{region:?}@{effective}");
        let response = ok(&router, "ListPriceLists", &body).await;
        let lists = list_member(&response, "PriceLists");
        assert_eq!(
            !lists.is_empty(),
            expect_any,
            "case {case}: expected any={expect_any}, got {response}"
        );
        for entry in &lists {
            assert_eq!(
                entry.get("CurrencyCode").and_then(Value::as_str),
                Some(currency),
                "case {case}: every entry must be denominated in the requested currency, \
                 got {entry}"
            );
            if let Some(region) = region {
                assert_eq!(
                    entry.get("RegionCode").and_then(Value::as_str),
                    Some(region),
                    "case {case}: every entry must be for the requested Region, got {entry}"
                );
            }
            let arn = entry
                .get("PriceListArn")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                arn.starts_with(ARN_PREFIX) && arn.len() >= 18,
                "case {case}: the ARN must match the modelled pattern, got {arn:?}"
            );
            assert!(
                !list_member(entry, "FileFormats").is_empty(),
                "case {case}: every entry must report its file formats, got {entry}"
            );
        }
    }

    // A later instant sees every version an earlier one did, and more where a newer
    // version was published since.
    let price_list_arns = |response: &Value| -> BTreeSet<String> {
        list_member(response, "PriceLists")
            .iter()
            .filter_map(|entry| entry.get("PriceListArn").and_then(Value::as_str))
            .map(str::to_string)
            .collect()
    };
    for service_code in ["AmazonEC2", "AmazonS3", "AWSLambda"] {
        let query = |effective: i64| {
            json!({
                "ServiceCode": service_code,
                "CurrencyCode": "USD",
                "EffectiveDate": effective,
            })
        };
        let earlier = price_list_arns(&ok(&router, "ListPriceLists", &query(JAN_1_2024)).await);
        let later = price_list_arns(&ok(&router, "ListPriceLists", &query(JUL_1_2024)).await);
        assert!(
            earlier.is_subset(&later) && later.len() > earlier.len(),
            "case {service_code}: a later EffectiveDate must see every earlier version and \
             the newer one too, {earlier:?} then {later:?}"
        );
    }

    // The ARNs are stored, not minted per call: the same query answers identically.
    let query = json!({
        "ServiceCode": "AmazonEC2",
        "CurrencyCode": "USD",
        "EffectiveDate": JUL_1_2024,
    });
    assert_eq!(
        ok(&router, "ListPriceLists", &query).await,
        ok(&router, "ListPriceLists", &query).await,
        "ListPriceLists must report the stored ARNs, not freshly randomized ones"
    );

    // An unfiltered listing covers exactly the Regions the service reports products
    // in, within the partition the currency belongs to — a count derived from the
    // catalog rather than written down here, so a stray record fails the row.
    for service_code in ["AmazonEC2", "AmazonS3", "AWSLambda"] {
        let product_regions: BTreeSet<String> = attribute_values(
            &ok(
                &router,
                "GetAttributeValues",
                &json!({ "ServiceCode": service_code, "AttributeName": "regionCode" }),
            )
            .await,
        )
        .into_iter()
        .filter(|region| !region.starts_with("cn-"))
        .collect();
        let listed = ok(
            &router,
            "ListPriceLists",
            &json!({
                "ServiceCode": service_code,
                "CurrencyCode": "USD",
                "EffectiveDate": JUL_1_2024,
            }),
        )
        .await;
        let listed_regions: BTreeSet<String> = list_member(&listed, "PriceLists")
            .iter()
            .filter_map(|entry| entry.get("RegionCode").and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        assert_eq!(
            listed_regions, product_regions,
            "case {service_code}: the USD price lists must cover exactly the Regions the \
             service reports products in"
        );
    }

    // Every Region the catalog reports for a service has a price list for it, in the
    // currency of that Region's partition — the linkage the model documents on
    // `PriceList.RegionCode`.
    let regions = attribute_values(
        &ok(
            &router,
            "GetAttributeValues",
            &json!({ "ServiceCode": "AmazonEC2", "AttributeName": "regionCode" }),
        )
        .await,
    );
    assert!(
        regions.len() > 10,
        "the catalog must price EC2 in more Regions than one page of ten, got {}",
        regions.len()
    );
    for region in &regions {
        let currency = if region.starts_with("cn-") { "CNY" } else { "USD" };
        let response = ok(
            &router,
            "ListPriceLists",
            &json!({
                "ServiceCode": "AmazonEC2",
                "CurrencyCode": currency,
                "RegionCode": region,
                "EffectiveDate": JUL_1_2024,
            }),
        )
        .await;
        let lists = list_member(&response, "PriceLists");
        assert!(
            !lists.is_empty(),
            "case RegionCode {region}: a Region the catalog reports must have a price \
             list, got {response}"
        );
        for entry in &lists {
            assert_eq!(
                entry.get("RegionCode").and_then(Value::as_str),
                Some(region.as_str()),
                "case RegionCode {region}: every entry must be for that Region, got {entry}"
            );
        }
    }

    // A code the catalog publishes no price lists for selects no record, which is an
    // empty PriceLists list rather than a refusal.
    let unknown = ok(
        &router,
        "ListPriceLists",
        &json!({
            "ServiceCode": "NoSuchService",
            "CurrencyCode": "USD",
            "EffectiveDate": JAN_1_2024,
        }),
    )
    .await;
    assert!(
        list_member(&unknown, "PriceLists").is_empty(),
        "case an unknown ServiceCode: expected an empty PriceLists, got {unknown}"
    );
}

/// The currencies every `pricePerUnit` in a price-list file is keyed by.
///
/// The file nests `terms.<term type>.<SKU>.<offer term code>.priceDimensions.<rate
/// code>.pricePerUnit`, so this walks all five levels.
fn price_per_unit_currencies(file: &Value) -> BTreeSet<String> {
    let mut currencies = BTreeSet::new();
    let Some(by_term_type) = file.get("terms").and_then(Value::as_object) else {
        return currencies;
    };
    for by_sku in by_term_type.values() {
        for by_term_code in by_sku.as_object().into_iter().flat_map(|skus| skus.values()) {
            for term in by_term_code
                .as_object()
                .into_iter()
                .flat_map(|terms| terms.values())
            {
                for dimension in term
                    .get("priceDimensions")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flat_map(|dimensions| dimensions.values())
                {
                    for currency in dimension
                        .get("pricePerUnit")
                        .and_then(Value::as_object)
                        .into_iter()
                        .flat_map(|rates| rates.keys())
                    {
                        currencies.insert(currency.clone());
                    }
                }
            }
        }
    }
    currencies
}

#[tokio::test]
async fn every_listed_price_list_arn_resolves_to_a_stable_file_url() {
    let (router, engine) = router_with_engine();

    // Rows differ in the service, the Region the price list covers and the currency it
    // is denominated in, including a service the catalog prices with a single usage
    // type per Region and the partition whose files are not denominated in USD.
    let cases: &[(&str, &str, &str)] = &[
        ("AmazonEC2", "us-east-1", "USD"),
        ("AmazonEC2", "eu-west-1", "USD"),
        ("AmazonEC2", "cn-north-1", "CNY"),
        ("AmazonS3", "us-east-1", "USD"),
        ("AmazonSNS", "us-east-1", "USD"),
    ];

    let mut urls: BTreeSet<String> = BTreeSet::new();
    for (service_code, region_code, currency_code) in cases.iter().copied() {
        let listed = ok(
            &router,
            "ListPriceLists",
            &json!({
                "ServiceCode": service_code,
                "CurrencyCode": currency_code,
                "RegionCode": region_code,
                "EffectiveDate": JUL_1_2024,
            }),
        )
        .await;
        let entries = list_member(&listed, "PriceLists");
        assert!(
            !entries.is_empty(),
            "case {service_code}/{region_code}: the round trip needs a listed price list, \
             got {listed}"
        );

        // The products the price list covers: the ones GetProducts returns for its
        // Region. The file must carry exactly these, priced as their offer documents
        // price them.
        let covered = ok(
            &router,
            "GetProducts",
            &json!({
                "ServiceCode": service_code,
                "Filters": [{
                    "Type": "TERM_MATCH",
                    "Field": "regionCode",
                    "Value": region_code,
                }],
            }),
        )
        .await;
        let in_region: BTreeSet<String> = product_skus(&covered).into_iter().collect();
        assert!(
            !in_region.is_empty(),
            "case {service_code}/{region_code}: the Region must have products"
        );

        for entry in &entries {
            let arn = entry
                .get("PriceListArn")
                .and_then(Value::as_str)
                .expect("a listed price list carries its ARN");
            let formats: Vec<String> = list_member(entry, "FileFormats")
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            assert!(
                !formats.is_empty(),
                "case {arn}: a listed price list carries its file formats"
            );
            for file_format in &formats {
                let request = json!({ "PriceListArn": arn, "FileFormat": file_format });
                let first = ok(&router, "GetPriceListFileUrl", &request).await;
                let url = first
                    .get("Url")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                assert!(
                    url.ends_with(&format!(".{file_format}")),
                    "case {arn}/{file_format}: expected a URL for the requested format, \
                     got {url:?}"
                );
                // The URL must address the emulator itself, on loopback — never a host
                // off this machine.
                let (host, path) = split_https_url(&url).unwrap_or_else(|| {
                    panic!("case {arn}/{file_format}: not an https URL: {url:?}")
                });
                assert!(
                    is_loopback(&host),
                    "case {arn}/{file_format}: the file host must be loopback, got \
                     {host:?} in {url:?}"
                );

                // And something must actually be served there: the same renderer the
                // emulator's file listener runs produces the file from the catalog.
                let state = engine.state::<PricingState>();
                let (content_type, body) =
                    render_price_list_file(&state, &path).unwrap_or_else(|| {
                        panic!("case {arn}/{file_format}: nothing is served at {path:?}")
                    });
                assert_eq!(
                    file_format, "json",
                    "case {arn}: unexpected file format {file_format:?}"
                );
                assert_eq!(
                    content_type, "application/json",
                    "case {arn}/{file_format}: wrong content type"
                );
                let file: Value = serde_json::from_str(&body).unwrap_or_else(|e| {
                    panic!("case {arn}/{file_format}: the file must be JSON: {e}")
                });
                assert_eq!(
                    file.get("offerCode").and_then(Value::as_str),
                    Some(service_code),
                    "case {arn}/{file_format}: the file must name its service, got {file}"
                );
                assert_eq!(
                    file.get("currencyCode").and_then(Value::as_str),
                    Some(currency_code),
                    "case {arn}/{file_format}: the file must be denominated as listed"
                );
                // And denominated in that currency all the way down: a file priced in
                // another currency than the one its header names cannot be read as a
                // price list at all.
                let rates = price_per_unit_currencies(&file);
                assert!(
                    !rates.is_empty(),
                    "case {arn}/{file_format}: the file must carry priced dimensions"
                );
                assert_eq!(
                    rates,
                    BTreeSet::from([currency_code.to_string()]),
                    "case {arn}/{file_format}: every rate must be in the file's currency"
                );
                let in_file: BTreeSet<String> = file
                    .get("products")
                    .and_then(Value::as_object)
                    .map(|products| products.keys().cloned().collect())
                    .unwrap_or_default();
                assert_eq!(
                    in_file, in_region,
                    "case {arn}/{file_format}: the file must carry the products of its \
                     Region"
                );
                for sku in &in_region {
                    assert!(
                        file.pointer(&format!("/terms/OnDemand/{sku}")).is_some(),
                        "case {arn}/{file_format}: every product in the file must be \
                         priced, {sku} is not"
                    );
                }

                // The same ARN and format must resolve to the same URL on every call.
                let second = ok(&router, "GetPriceListFileUrl", &request).await;
                assert_eq!(
                    first, second,
                    "case {arn}/{file_format}: the URL must be the same on every call"
                );
                assert!(
                    urls.insert(url.clone()),
                    "case {arn}/{file_format}: each price-list file must have its own URL, \
                     {url:?} was already seen"
                );
            }
        }
    }

    // A path no price list publishes is served nothing, so a stray fetch cannot read
    // some other file.
    let state = engine.state::<PricingState>();
    assert!(
        render_price_list_file(&state, "/offers/v1.0/aws/AmazonEC2/nope/index.json").is_none(),
        "an unknown file path must serve nothing"
    );
}
