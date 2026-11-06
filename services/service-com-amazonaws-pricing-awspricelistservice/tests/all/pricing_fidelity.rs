// SPDX-License-Identifier: CC0-1.0

//! The Price List Query API's catalog breadth and its client-error classification.
//!
//! Each test states one rule over a TABLE of inputs rather than one recorded request:
//! the catalog publishes a service code for every service and pages its listings, the
//! documented EC2 on-demand filter set selects a priced SKU, a reservable product's
//! offer document carries every term type it is offered under, an unmatched
//! `DescribeServices` filter is an empty list, an input-constraint violation is the
//! only client-input error this API models, an unresolvable price-list file is
//! `NotFoundException`, an unpublished currency is an authorization outcome, and an
//! unissued `NextToken` is refused with this API's own invariant message.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::pricing_support::{
    attribute_values, err, error_message, list_member, offer_attribute, offer_documents, ok,
    product_skus, router, service_codes, service_entry, JAN_1_2024, JUL_1_2024,
};

/// The documented EC2 on-demand filter set: the members a cost-estimation client pins
/// to select exactly one priced compute SKU.
fn on_demand_filters(instance_type: &str) -> Value {
    json!([
        { "Type": "TERM_MATCH", "Field": "instanceType", "Value": instance_type },
        { "Type": "TERM_MATCH", "Field": "location", "Value": "US East (N. Virginia)" },
        { "Type": "TERM_MATCH", "Field": "operatingSystem", "Value": "Linux" },
        { "Type": "TERM_MATCH", "Field": "tenancy", "Value": "Shared" },
        { "Type": "TERM_MATCH", "Field": "preInstalledSw", "Value": "NA" },
        { "Type": "TERM_MATCH", "Field": "capacitystatus", "Value": "Used" },
        { "Type": "TERM_MATCH", "Field": "licenseModel", "Value": "No License required" },
    ])
}

/// Every `pricePerUnit.USD` rate stated in `unit` by one term's price dimensions.
///
/// A term's usage dimensions and the one-off fee an upfront purchase option pays are
/// different units, so a rule about one must not read the other.
fn term_rates_in_unit(term: &Value, unit: &str) -> Vec<f64> {
    term.get("priceDimensions")
        .and_then(Value::as_object)
        .map(|dimensions| {
            dimensions
                .values()
                .filter(|dimension| {
                    dimension.get("unit").and_then(Value::as_str) == Some(unit)
                })
                .filter_map(|dimension| {
                    dimension
                        .pointer("/pricePerUnit/USD")?
                        .as_str()?
                        .parse::<f64>()
                        .ok()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every `pricePerUnit.USD` rate stated in `unit` across a term block keyed by offer
/// term code.
fn rates_in_unit(terms: &Value, unit: &str) -> Vec<f64> {
    terms
        .as_object()
        .map(|by_term| {
            by_term
                .values()
                .flat_map(|term| term_rates_in_unit(term, unit))
                .collect()
        })
        .unwrap_or_default()
}

/// Every `pricePerUnit.USD` rate in one term block of an offer document.
fn usd_rates(terms: &Value) -> Vec<f64> {
    terms
        .as_object()
        .map(|by_term| {
            by_term
                .values()
                .filter_map(|term| term.get("priceDimensions")?.as_object())
                .flat_map(|dimensions| dimensions.values())
                .filter_map(|dimension| {
                    dimension
                        .pointer("/pricePerUnit/USD")?
                        .as_str()?
                        .parse::<f64>()
                        .ok()
                })
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn the_catalog_publishes_every_service_code_and_pages_its_listings() {
    let router = router();

    // DescribeServices is @paginated over Services with MaxResults 1..100: the catalog
    // must genuinely span a page, and the traversal must yield each code once.
    let page_size = 25;
    let mut codes: Vec<String> = Vec::new();
    let mut pages = 0usize;
    let mut next_token: Option<String> = None;
    loop {
        let mut body = json!({ "MaxResults": page_size });
        if let Some(token) = &next_token {
            body["NextToken"] = json!(token);
        }
        let page = ok(&router, "DescribeServices", &body).await;
        let page_codes = service_codes(&page);
        assert!(
            page_codes.len() <= page_size,
            "case page {pages}: a page must not exceed MaxResults, got {}",
            page_codes.len()
        );
        codes.extend(page_codes);
        pages += 1;
        next_token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() || pages > 50 {
            break;
        }
    }
    let distinct: BTreeSet<&String> = codes.iter().collect();
    assert_eq!(
        distinct.len(),
        codes.len(),
        "the traversal must not repeat a service code, got {} of {} distinct",
        distinct.len(),
        codes.len()
    );
    assert!(
        pages > 1 && codes.len() > page_size,
        "the catalog must span more than one page of {page_size}, got {} codes over \
         {pages} page(s)",
        codes.len()
    );
    // A client priced against a service outside the two priced in depth must find it.
    for service_code in ["AmazonSNS", "AWSLambda", "AmazonRDS", "AWSQueueService"] {
        assert!(
            codes.contains(&service_code.to_string()),
            "case {service_code}: the catalog must publish this service code, got {} codes",
            codes.len()
        );
    }

    // An attribute whose value set is a published catalog — the Region display names —
    // must enumerate it, not a sample of three. The rows are Regions the Price List
    // API publishes `location` values for.
    let locations = attribute_values(
        &ok(
            &router,
            "GetAttributeValues",
            &json!({ "ServiceCode": "AmazonEC2", "AttributeName": "location" }),
        )
        .await,
    );
    assert!(
        locations.len() > 10,
        "case location: expected more than one page of ten Region display names, got \
         {locations:?}"
    );
    for location in [
        "Asia Pacific (Tokyo)",
        "Asia Pacific (Seoul)",
        "EU (Frankfurt)",
        "South America (Sao Paulo)",
        "US East (Ohio)",
    ] {
        assert!(
            locations.contains(&location.to_string()),
            "case location={location}: expected the Region in the published catalog, got \
             {locations:?}"
        );
    }

    // An attribute with more values than one page carries a NextToken, and the next
    // page is non-empty and disjoint from the first.
    for (attribute_name, page_size) in [("instanceType", 10), ("location", 5)] {
        let query = json!({
            "ServiceCode": "AmazonEC2",
            "AttributeName": attribute_name,
            "MaxResults": page_size,
        });
        let first = ok(&router, "GetAttributeValues", &query).await;
        let token = first
            .get("NextToken")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!(
                    "case {attribute_name}/MaxResults={page_size}: expected a NextToken, \
                     got {first}"
                )
            })
            .to_string();
        let mut next_query = query.clone();
        next_query["NextToken"] = json!(token);
        let second = ok(&router, "GetAttributeValues", &next_query).await;
        let first_values: BTreeSet<String> = attribute_values(&first).into_iter().collect();
        let second_values: BTreeSet<String> = attribute_values(&second).into_iter().collect();
        assert!(
            !second_values.is_empty(),
            "case {attribute_name}/MaxResults={page_size}: the second page must not be \
             empty, got {second}"
        );
        assert!(
            first_values.is_disjoint(&second_values),
            "case {attribute_name}/MaxResults={page_size}: the pages must be disjoint, \
             {first_values:?} and {second_values:?}"
        );
    }
}

#[tokio::test]
async fn ec2_answers_the_documented_on_demand_filter_set_with_one_priced_sku() {
    let router = router();

    // The attribute names DescribeServices reports for the service must include the
    // ones the model's own docs name for EC2 — `volumeType` among them, which is the
    // attribute the `Filter` doc uses as its worked example.
    let described = ok(
        &router,
        "DescribeServices",
        &json!({ "ServiceCode": "AmazonEC2" }),
    )
    .await;
    let entry = service_entry(&described, "AmazonEC2").expect("AmazonEC2 is described");
    let names: BTreeSet<String> = list_member(&entry, "AttributeNames")
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    for name in [
        "instanceType",
        "volumeType",
        "location",
        "operatingSystem",
        "tenancy",
        "licenseModel",
    ] {
        assert!(
            names.contains(name),
            "case {name}: AmazonEC2 must publish this attribute name, got {names:?}"
        );
    }

    // The documented filter set selects exactly one priced SKU, for every instance
    // type the catalog prices in that Region under that operating system. The rows
    // differ in instance family and size.
    for instance_type in [
        "t2.micro",
        "t2.small",
        "t2.medium",
        "t3.micro",
        "m5.large",
        "c5.xlarge",
        "r5.large",
        "m6g.large",
    ] {
        let response = ok(
            &router,
            "GetProducts",
            &json!({
                "ServiceCode": "AmazonEC2",
                "Filters": on_demand_filters(instance_type),
                "MaxResults": 10,
            }),
        )
        .await;
        let documents = offer_documents(&response);
        assert_eq!(
            documents.len(),
            1,
            "case {instance_type}: expected exactly 1 matching SKU, got {}",
            documents.len()
        );
        assert_eq!(
            offer_attribute(&documents[0], "instanceType").as_deref(),
            Some(instance_type),
            "case {instance_type}: the matched product must be that type, got {}",
            documents[0]
        );
        let rates = usd_rates(
            documents[0]
                .pointer("/terms/OnDemand")
                .unwrap_or(&Value::Null),
        );
        assert!(
            !rates.is_empty() && rates.iter().all(|rate| *rate > 0.0),
            "case {instance_type}: expected a positive USD on-demand rate, got {rates:?}"
        );
    }

    // `volumeType` has values, and they are the EBS volume-type labels the Price List
    // API publishes — including ones outside the three the on-demand filter set names.
    let volume_types = attribute_values(
        &ok(
            &router,
            "GetAttributeValues",
            &json!({
                "ServiceCode": "AmazonEC2",
                "AttributeName": "volumeType",
                "MaxResults": 100,
            }),
        )
        .await,
    );
    for label in [
        "General Purpose",
        "Provisioned IOPS",
        "Magnetic",
        "Cold HDD",
        "Throughput Optimized HDD",
    ] {
        assert!(
            volume_types.contains(&label.to_string()),
            "case volumeType={label}: expected the published label, got {volume_types:?}"
        );
    }

    // ANY_OF over a family selects every type it names; CONTAINS over a substring of a
    // label selects every product whose label contains it.
    let any_of = ok(
        &router,
        "GetProducts",
        &json!({
            "ServiceCode": "AmazonEC2",
            "Filters": [
                { "Type": "ANY_OF", "Field": "instanceType", "Value": "t2.micro,t2.small,t2.medium" },
                { "Type": "TERM_MATCH", "Field": "operatingSystem", "Value": "Linux" },
            ],
        }),
    )
    .await;
    let found: BTreeSet<String> = offer_documents(&any_of)
        .iter()
        .filter_map(|document| offer_attribute(document, "instanceType"))
        .collect();
    let expected: BTreeSet<String> = ["t2.medium", "t2.micro", "t2.small"]
        .iter()
        .map(|value| (*value).to_string())
        .collect();
    assert_eq!(
        found, expected,
        "case ANY_OF t2 family: expected {expected:?}, got {found:?}"
    );

    for substring in ["HDD", "Purpose", "IOPS"] {
        let response = ok(
            &router,
            "GetProducts",
            &json!({
                "ServiceCode": "AmazonEC2",
                "Filters": [{ "Type": "CONTAINS", "Field": "volumeType", "Value": substring }],
            }),
        )
        .await;
        let documents = offer_documents(&response);
        assert!(
            !documents.is_empty(),
            "case CONTAINS volumeType={substring}: expected at least one volume product, \
             got {response}"
        );
        for document in &documents {
            let label = offer_attribute(document, "volumeType").unwrap_or_default();
            assert!(
                label.contains(substring),
                "case CONTAINS volumeType={substring}: a matched product must carry a \
                 label containing it, got {label:?}"
            );
        }
    }

    // NONE_OF excludes exactly the listed values: the positive control is there, and
    // the exclusion removes it and nothing else.
    for excluded in ["t2.micro", "m5.large"] {
        let control = ok(
            &router,
            "GetProducts",
            &json!({
                "ServiceCode": "AmazonEC2",
                "Filters": on_demand_filters(excluded),
            }),
        )
        .await;
        let control_skus: BTreeSet<String> = product_skus(&control).into_iter().collect();
        assert_eq!(
            control_skus.len(),
            1,
            "case NONE_OF {excluded}: positive control must exist, got {control_skus:?}"
        );
        let response = ok(
            &router,
            "GetProducts",
            &json!({
                "ServiceCode": "AmazonEC2",
                "Filters": [{ "Type": "NONE_OF", "Field": "instanceType", "Value": excluded }],
            }),
        )
        .await;
        let kept: BTreeSet<String> = product_skus(&response).into_iter().collect();
        assert!(
            kept.is_disjoint(&control_skus),
            "case NONE_OF {excluded}: the excluded SKU must not be returned, got {kept:?}"
        );
        for document in offer_documents(&response) {
            assert_ne!(
                offer_attribute(&document, "instanceType").as_deref(),
                Some(excluded),
                "case NONE_OF {excluded}: no returned product may carry the excluded value"
            );
        }
    }
}

#[tokio::test]
async fn a_reservable_product_carries_every_term_type_it_is_offered_under() {
    let router = router();

    // A compute instance is offered both on demand and under a reservation, so its
    // offer document carries both term types; each reserved term states the lease
    // length the Price List API publishes reserved pricing for.
    let compute = ok(
        &router,
        "GetProducts",
        &json!({
            "ServiceCode": "AmazonEC2",
            "Filters": [{ "Type": "TERM_MATCH", "Field": "productFamily", "Value": "nothing" }],
        }),
    )
    .await;
    assert!(
        offer_documents(&compute).is_empty(),
        "a filter on a field no product carries selects nothing, got {compute}"
    );

    let compute = ok(
        &router,
        "GetProducts",
        &json!({
            "ServiceCode": "AmazonEC2",
            "Filters": [{ "Type": "TERM_MATCH", "Field": "location", "Value": "US East (N. Virginia)" }],
        }),
    )
    .await;
    let documents = offer_documents(&compute);
    assert!(
        documents.len() > 10,
        "the Region must price enough products for this rule to span a set, got {}",
        documents.len()
    );

    let mut reservable = 0usize;
    let mut on_demand_only = 0usize;
    for document in &documents {
        let sku = document
            .pointer("/product/sku")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let on_demand = document
            .pointer("/terms/OnDemand")
            .unwrap_or_else(|| panic!("case {sku}: every product must be priced on demand"));
        let on_demand_rates = usd_rates(on_demand);
        assert!(
            !on_demand_rates.is_empty() && on_demand_rates.iter().all(|rate| *rate > 0.0),
            "case {sku}: expected a positive on-demand rate, got {on_demand_rates:?}"
        );

        let family = offer_attribute(document, "instanceType");
        match document.pointer("/terms/Reserved") {
            Some(reserved) => {
                assert!(
                    family.is_some(),
                    "case {sku}: only a compute instance is offered under a reservation, \
                     got {document}"
                );
                let terms = reserved
                    .as_object()
                    .unwrap_or_else(|| panic!("case {sku}: terms.Reserved must be a map"));
                assert!(
                    !terms.is_empty(),
                    "case {sku}: terms.Reserved must not be empty"
                );
                for (key, term) in terms {
                    let lease = term
                        .pointer("/termAttributes/LeaseContractLength")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    assert!(
                        lease == "1yr" || lease == "3yr",
                        "case {sku}/{key}: expected a published lease length, got {lease:?}"
                    );
                }
                // Reserved pricing is published per lease length, offering class AND
                // purchase option, so every term states its own combination of the three.
                let offers: BTreeSet<(String, String, String)> = terms
                    .values()
                    .filter_map(|term| {
                        let attributes = term.pointer("/termAttributes")?;
                        let text = |name: &str| {
                            attributes
                                .get(name)
                                .and_then(Value::as_str)
                                .map(str::to_string)
                        };
                        Some((
                            text("LeaseContractLength")?,
                            text("OfferingClass")?,
                            text("PurchaseOption")?,
                        ))
                    })
                    .collect();
                assert_eq!(
                    offers.len(),
                    terms.len(),
                    "case {sku}: each reserved term must state its own lease, offering class \
                     and purchase option, got {offers:?}"
                );

                // The usage a reservation is billed for is priced in the same unit as the
                // on-demand term, and below it.
                let reserved_rates = rates_in_unit(reserved, "Hrs");
                let cheapest_on_demand = on_demand_rates
                    .iter()
                    .copied()
                    .fold(f64::INFINITY, f64::min);
                assert!(
                    !reserved_rates.is_empty()
                        && reserved_rates
                            .iter()
                            .all(|rate| *rate < cheapest_on_demand),
                    "case {sku}: a reservation must price its usage below on demand, \
                     {reserved_rates:?} against {cheapest_on_demand}"
                );

                // An upfront payment is a one-off fee stated as a Quantity, so only a term
                // whose purchase option pays one carries that dimension.
                for (key, term) in terms {
                    let option = term
                        .pointer("/termAttributes/PurchaseOption")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let fees = term_rates_in_unit(term, "Quantity");
                    if option == "No Upfront" {
                        assert!(
                            fees.is_empty(),
                            "case {sku}/{key}: a no-upfront term pays no upfront fee, got \
                             {fees:?}"
                        );
                    } else {
                        assert!(
                            fees.len() == 1 && fees[0] > 0.0,
                            "case {sku}/{key}: {option} must pay exactly one positive \
                             upfront fee, got {fees:?}"
                        );
                    }
                }
                reservable += 1;
            }
            None => {
                assert!(
                    family.is_none(),
                    "case {sku}: a compute instance must carry a reserved term, got \
                     {document}"
                );
                on_demand_only += 1;
            }
        }
    }
    assert!(
        reservable > 0 && on_demand_only > 0,
        "the Region must hold both reservable and on-demand-only products, got \
         {reservable} and {on_demand_only}"
    );
}

#[tokio::test]
async fn describe_services_with_an_unmatched_service_code_returns_an_empty_list() {
    let router = router();

    // `ServiceCode` is an unconstrained optional FILTER on this listing and the
    // operation models no not-found error of its own, so a code the catalog does not
    // hold selects nothing: HTTP 200 with an empty Services list. The rows differ in
    // the shape of the unmatched code.
    for service_code in [
        "NotARealPricingServiceXYZ123",
        "amazonec2",
        "AmazonEC2 ",
        "Amazon",
        "a",
    ] {
        let response = ok(
            &router,
            "DescribeServices",
            &json!({ "ServiceCode": service_code }),
        )
        .await;
        assert!(
            list_member(&response, "Services").is_empty(),
            "case {service_code:?}: expected an empty Services list, got {response}"
        );
        assert_eq!(
            response.get("FormatVersion").and_then(Value::as_str),
            Some("aws_v1"),
            "case {service_code:?}: the answer must still report the format version, got \
             {response}"
        );
    }

    // A code the catalog does hold still selects exactly that service, and a blank
    // code is the same as naming none ("To retrieve a list of all services, leave this
    // blank").
    for service_code in ["AmazonEC2", "AmazonS3", "AmazonSNS"] {
        let response = ok(
            &router,
            "DescribeServices",
            &json!({ "ServiceCode": service_code }),
        )
        .await;
        assert_eq!(
            service_codes(&response),
            vec![service_code.to_string()],
            "case {service_code}: expected exactly that service, got {response}"
        );
    }
    let blank = ok(&router, "DescribeServices", &json!({ "ServiceCode": "" })).await;
    let unfiltered = ok(&router, "DescribeServices", &json!({})).await;
    assert_eq!(
        service_codes(&blank),
        service_codes(&unfiltered),
        "case blank ServiceCode: expected every service, got {} codes",
        service_codes(&blank).len()
    );
}

#[tokio::test]
async fn an_input_constraint_violation_is_an_invalid_parameter_exception() {
    let router = router();

    // The service models `InvalidParameterException` (HTTP 400) as a common error and
    // models no `ValidationException` at all, so EVERY constraint the model declares —
    // a pattern, a length, a range, an enumerated value, a required member — is
    // refused with that code, and the message names the member and the value.
    // `expected_in_message` is the member path the refusal must name.
    let cases: &[(&str, &str, Value, &str)] = &[
        (
            "PriceListArn outside its pattern",
            "GetPriceListFileUrl",
            json!({
                "PriceListArn": "arn:aws:pricing:::not-a-price-list/AmazonEC2/some-fake-id",
                "FileFormat": "json",
            }),
            "priceListArn",
        ),
        (
            "PriceListArn shorter than its length",
            "GetPriceListFileUrl",
            json!({ "PriceListArn": "arn:aws:pricing", "FileFormat": "json" }),
            "priceListArn",
        ),
        (
            "FileFormat shorter than its length",
            "GetPriceListFileUrl",
            json!({
                "PriceListArn": "arn:aws:pricing:::price-list/aws/AmazonEC2/USD/20230101000000/us-east-1",
                "FileFormat": "",
            }),
            "fileFormat",
        ),
        (
            "CurrencyCode outside its pattern",
            "ListPriceLists",
            json!({
                "ServiceCode": "AmazonEC2",
                "CurrencyCode": "US",
                "EffectiveDate": JAN_1_2024,
            }),
            "currencyCode",
        ),
        (
            "CurrencyCode in the wrong case",
            "ListPriceLists",
            json!({
                "ServiceCode": "AmazonEC2",
                "CurrencyCode": "usd",
                "EffectiveDate": JAN_1_2024,
            }),
            "currencyCode",
        ),
        (
            "a required member absent",
            "ListPriceLists",
            json!({ "ServiceCode": "AmazonEC2", "EffectiveDate": JAN_1_2024 }),
            "currencyCode",
        ),
        (
            "MaxResults below its range",
            "DescribeServices",
            json!({ "MaxResults": 0 }),
            "maxResults",
        ),
        (
            "MaxResults above its range",
            "DescribeServices",
            json!({ "MaxResults": 101 }),
            "maxResults",
        ),
        (
            "a Filter type outside the modelled enum",
            "GetProducts",
            json!({
                "ServiceCode": "AmazonEC2",
                "Filters": [{ "Type": "STARTS_WITH", "Field": "instanceType", "Value": "t3" }],
            }),
            "type",
        ),
    ];

    for (case, operation, body, member) in cases.iter().cloned() {
        let parsed = err(&router, operation, &body, 400, "InvalidParameterException").await;
        let message = error_message(&parsed);
        assert!(
            message.contains("validation error") && message.contains(member),
            "case {case}: the message must name the member {member}, got {message:?}"
        );
    }

    // And the bounds themselves hold: a value AT each published bound is accepted.
    for max_results in [1, 100] {
        let response = ok(
            &router,
            "DescribeServices",
            &json!({ "MaxResults": max_results }),
        )
        .await;
        let codes = service_codes(&response);
        assert!(
            !codes.is_empty() && codes.len() <= max_results,
            "case MaxResults={max_results}: expected at most that many services, got {}",
            codes.len()
        );
    }
}

#[tokio::test]
async fn a_price_list_file_that_resolves_to_nothing_is_not_found() {
    let router = router();

    // A known (ARN, format) pair, for the rows below to contrast with.
    let listed = ok(
        &router,
        "ListPriceLists",
        &json!({
            "ServiceCode": "AmazonEC2",
            "CurrencyCode": "USD",
            "RegionCode": "us-east-1",
            "EffectiveDate": JUL_1_2024,
        }),
    )
    .await;
    let known_arn = list_member(&listed, "PriceLists")
        .first()
        .and_then(|entry| entry.get("PriceListArn").and_then(Value::as_str))
        .expect("a listed price list carries its ARN")
        .to_string();

    // An (ARN, FileFormat) pair that resolves to no price-list file is refused with the
    // modelled NotFoundException (HTTP 400), not the 404 ResourceNotFoundException this
    // operation also models; the message is built from the ARN and the format asked
    // for. The rows differ in which half of the pair is unresolvable.
    let cases: &[(&str, &str, &str)] = &[
        (
            "an ARN no price list has",
            "arn:aws:pricing:::price-list/aws/AmazonEC2/USD/99999999999999/us-east-1",
            "json",
        ),
        (
            "an ARN for a Region with no price list",
            "arn:aws:pricing:::price-list/aws/AmazonEC2/USD/20230101000000/us-east-9",
            "json",
        ),
        (
            "an ARN for an unpriced service",
            "arn:aws:pricing:::price-list/aws/NotAService/USD/20230101000000/us-east-1",
            "json",
        ),
        ("a known ARN in a format it is not published in", &known_arn, "csv"),
        ("a known ARN in another unpublished format", &known_arn, "xml"),
    ];

    for (case, arn, file_format) in cases.iter().copied() {
        let parsed = err(
            &router,
            "GetPriceListFileUrl",
            &json!({ "PriceListArn": arn, "FileFormat": file_format }),
            400,
            "NotFoundException",
        )
        .await;
        let message = error_message(&parsed);
        assert!(
            message.contains(arn) && message.contains(file_format),
            "case {case}: the message must name the ARN and the format, got {message:?}"
        );
    }

    // The same pair in a format it IS published in resolves.
    let resolved = ok(
        &router,
        "GetPriceListFileUrl",
        &json!({ "PriceListArn": known_arn, "FileFormat": "json" }),
    )
    .await;
    assert!(
        resolved
            .get("Url")
            .and_then(Value::as_str)
            .is_some_and(|url| url.ends_with(".json")),
        "case the published format: expected a URL, got {resolved}"
    );
}

#[tokio::test]
async fn a_currency_the_api_does_not_publish_is_an_authorization_refusal() {
    let router = router();

    // A pattern-valid ISO-4217 code the Price List API publishes no price lists in is
    // an authorization outcome, with a fixed sentence carrying no request data — not a
    // bad parameter. The rows differ in which unpublished currency is asked for.
    for currency in ["ZZZ", "EUR", "GBP", "JPY", "AUD"] {
        let parsed = err(
            &router,
            "ListPriceLists",
            &json!({
                "ServiceCode": "AmazonEC2",
                "CurrencyCode": currency,
                "EffectiveDate": JAN_1_2024,
            }),
            401,
            "AccessDeniedException",
        )
        .await;
        assert_eq!(
            error_message(&parsed),
            "Caller is not authorized to access requested price lists",
            "case {currency}: expected the API's fixed sentence"
        );
    }

    // A currency the API DOES publish is never refused for being unmatched: every
    // supported currency answers 200, with only that currency's lists (possibly none).
    for service_code in ["AmazonEC2", "AmazonS3", "AmazonSNS"] {
        for currency in ["USD", "CNY"] {
            let response = ok(
                &router,
                "ListPriceLists",
                &json!({
                    "ServiceCode": service_code,
                    "CurrencyCode": currency,
                    "EffectiveDate": JAN_1_2024,
                }),
            )
            .await;
            for entry in list_member(&response, "PriceLists") {
                assert_eq!(
                    entry.get("CurrencyCode").and_then(Value::as_str),
                    Some(currency),
                    "case {service_code}/{currency}: every entry must be denominated in \
                     the requested currency, got {entry}"
                );
            }
        }
    }
}

#[tokio::test]
async fn an_unissued_next_token_is_refused_with_the_api_message() {
    let router = router();

    // A NextToken this service never issued is refused with the modelled
    // InvalidNextTokenException (HTTP 400) and this API's invariant message, which
    // carries no request data and so does not vary with the token or the operation.
    // The rows cover every paginated operation and every way a token can be unusable.
    let operations: &[(&str, Value)] = &[
        ("DescribeServices", json!({})),
        (
            "GetAttributeValues",
            json!({ "ServiceCode": "AmazonEC2", "AttributeName": "instanceType" }),
        ),
        ("GetProducts", json!({ "ServiceCode": "AmazonEC2" })),
        (
            "ListPriceLists",
            json!({
                "ServiceCode": "AmazonEC2",
                "CurrencyCode": "USD",
                "EffectiveDate": JUL_1_2024,
            }),
        ),
    ];
    let tokens = [
        "this-was-never-issued-by-pricing",
        // A blank token is NOT "no token": unlike `DescribeServices$ServiceCode`,
        // whose doc says to leave it blank to get every service, `NextToken` carries
        // an opaque cursor this service minted, and the empty string is not one.
        "",
        "00",
        "ffffffffffffffffffffffff",
    ];

    for (operation, body) in operations.iter().cloned() {
        for token in tokens {
            let mut body = body.clone();
            body["NextToken"] = json!(token);
            let parsed = err(
                &router,
                operation,
                &body,
                400,
                "InvalidNextTokenException",
            )
            .await;
            assert_eq!(
                error_message(&parsed),
                "NextToken is invalid. ",
                "case {operation}/{token:?}: expected this API's invariant message"
            );
        }
    }
}
