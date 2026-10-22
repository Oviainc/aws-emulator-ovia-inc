// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Price List catalog, driven over the wire.
//!
//! Every call goes through the same `MultiServiceRouter::dispatch` the emulator's front
//! door uses, so these exercise routing, awsJson1_1 (de)serialization, modelled
//! constraint validation and the handlers together.
//!
//! The assertions are RULES over a table of inputs, not one recorded case:
//! `DescribeServices` names services the catalog holds, `GetAttributeValues` returns the
//! distinct values the SAME products carry, each `Filter` type selects the products its
//! documented comparison selects, a page boundary is carried by an opaque `NextToken`,
//! and the `PriceListArn` `ListPriceLists` reports resolves to a stable URL.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_pricing_awspricelistservice::state::{
    render_price_list_file, PricingState,
};
use service_com_amazonaws_pricing_awspricelistservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Epoch-seconds instants used as `EffectiveDate`. awsJson1_1 carries a timestamp as
/// epoch seconds, so these are the wire form of the dates in the comments.
const JAN_1_2022: i64 = 1_640_995_200; // 2022-01-01T00:00:00Z
const JAN_1_2024: i64 = 1_704_067_200; // 2024-01-01T00:00:00Z
const JUL_1_2024: i64 = 1_719_705_600; // 2024-07-01T00:00:00Z

/// The ARN prefix the model's `PriceListArn` pattern requires
/// (`^arn:[A-Za-z0-9][-.A-Za-z0-9]{0,62}:pricing:::price-list/…$`).
const ARN_PREFIX: &str = "arn:aws:pricing:::price-list/";

/// The emulator's front door with only this service mounted.
fn router() -> MultiServiceRouter {
    router_with_engine().0
}

/// The front door and the shared engine behind it, for a test that also reads the
/// catalog directly (the price-list file renderer).
fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These tests exercise the service, not SigV4: the requests below carry a credential
    // scope so the router can name the service, but no real signature.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Call one operation over the wire, returning the HTTP status and the parsed body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSPriceListService.{operation}"))
        // The SigV4 credential scope names the service an AWS SDK addressed the request
        // to; it is what the router routes on.
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20240101/us-east-1/pricing/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0000",
        )
        .body(Bytes::from(body.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// Call one operation and require it to succeed.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation} {body}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// Call one operation and require the modelled failure `error_code` with `status`.
async fn err(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    status: u16,
    error_code: &str,
) {
    let (actual_status, parsed) = call(router, operation, body).await;
    assert_eq!(
        actual_status, status,
        "case {operation} {body}: expected HTTP {status}, got {actual_status} with {parsed}"
    );
    assert_eq!(
        parsed.get("__type").and_then(Value::as_str),
        Some(error_code),
        "case {operation} {body}: expected __type {error_code}, got {parsed}"
    );
}

/// The `Services` entry for `service_code`, from a `DescribeServices` response.
fn service_entry<'a>(response: &'a Value, service_code: &str) -> Option<&'a Value> {
    response
        .get("Services")?
        .as_array()?
        .iter()
        .find(|entry| entry.get("ServiceCode").and_then(Value::as_str) == Some(service_code))
}

/// The string values of a `GetAttributeValues` response's `AttributeValues`.
fn attribute_values(response: &Value) -> Vec<String> {
    response
        .get("AttributeValues")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.get("Value").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The SKUs of the offer documents in a `GetProducts` response, parsed out of the
/// returned JSON documents.
fn product_skus(response: &Value) -> Vec<String> {
    offer_documents(response)
        .iter()
        .map(|document| {
            document
                .get("product")
                .and_then(|product| product.get("sku"))
                .and_then(Value::as_str)
                .expect("every offer document carries product.sku")
                .to_string()
        })
        .collect()
}

/// The offer documents in a `GetProducts` response: each `PriceList` element is a JSON
/// document carried as a string.
fn offer_documents(response: &Value) -> Vec<Value> {
    response
        .get("PriceList")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    let text = item
                        .as_str()
                        .expect("every PriceList element is a JSON string");
                    serde_json::from_str(text).expect("every PriceList element parses as JSON")
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One product attribute from an offer document.
fn offer_attribute(document: &Value, name: &str) -> Option<String> {
    document
        .get("product")?
        .get("attributes")?
        .get(name)?
        .as_str()
        .map(str::to_string)
}

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
    let codes: Vec<String> = all
        .get("Services")
        .and_then(Value::as_array)
        .expect("Services is a list")
        .iter()
        .filter_map(|entry| entry.get("ServiceCode").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    assert!(
        codes.len() >= 2 && codes.contains(&"AmazonEC2".to_string()),
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
        let services = one
            .get("Services")
            .and_then(Value::as_array)
            .expect("Services is a list");
        assert_eq!(
            services.len(),
            1,
            "case {service_code}: expected exactly one Service, got {one}"
        );
        assert_eq!(
            Some(&services[0]),
            service_entry(&all, service_code),
            "case {service_code}: the filtered entry must equal the unfiltered one"
        );
        let names: Vec<&str> = services[0]
            .get("AttributeNames")
            .and_then(Value::as_array)
            .expect("AttributeNames is a list")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(
            !names.is_empty(),
            "case {service_code}: expected a non-empty AttributeNames, got {one}"
        );

        // Every attribute name reported must have values, and those values must be the
        // ones the service's products carry.
        for name in names {
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
            let products = ok(
                &router,
                "GetProducts",
                &json!({ "ServiceCode": service_code }),
            )
            .await;
            let from_products: BTreeSet<String> = offer_documents(&products)
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

    // A service code the catalog does not hold is NotFound, never an empty list.
    err(
        &router,
        "DescribeServices",
        &json!({ "ServiceCode": "NoSuchService" }),
        400,
        "NotFoundException",
    )
    .await;
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
    // previous test pins against GetProducts.
    let cases: &[(&str, &str, &[&str])] = &[
        (
            "AmazonEC2",
            "instanceType",
            &["m5.large", "t3.micro", "t3.small"],
        ),
        ("AmazonEC2", "operatingSystem", &["Linux", "Windows"]),
        (
            "AmazonEC2",
            "regionCode",
            &["eu-west-1", "us-east-1", "us-west-2"],
        ),
        ("AmazonEC2", "locationType", &["AWS Region"]),
        (
            "AmazonS3",
            "volumeType",
            &["Standard", "Standard - Infrequent Access"],
        ),
        ("AmazonS3", "regionCode", &["eu-west-1", "us-east-1"]),
    ];

    for (service_code, attribute_name, expected) in cases {
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

    // An unknown service, and an attribute the service does not publish.
    err(
        &router,
        "GetAttributeValues",
        &json!({ "ServiceCode": "NoSuchService", "AttributeName": "instanceType" }),
        400,
        "NotFoundException",
    )
    .await;
    err(
        &router,
        "GetAttributeValues",
        &json!({ "ServiceCode": "AmazonEC2", "AttributeName": "noSuchAttribute" }),
        400,
        "NotFoundException",
    )
    .await;
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
    let all_skus = product_skus(&unfiltered);
    assert!(
        all_skus.len() >= 5,
        "expected the seeded EC2 products, got {all_skus:?}"
    );

    // One row per filter type the model declares (FilterType: TERM_MATCH, EQUALS,
    // CONTAINS, ANY_OF, NONE_OF), plus the `ServiceCode` pseudo-field the Filter.Field
    // doc names, plus two filters that must BOTH hold.
    let cases: &[(&str, Value, Vec<&str>)] = &[
        (
            "TERM_MATCH on instanceType",
            json!([{ "Type": "TERM_MATCH", "Field": "instanceType", "Value": "t3.micro" }]),
            vec!["T3MICROEUW1LINUX", "T3MICROUSE1LINUX"],
        ),
        (
            "EQUALS on operatingSystem",
            json!([{ "Type": "EQUALS", "Field": "operatingSystem", "Value": "Windows" }]),
            vec!["M5LARGEUSW2WINDOWS"],
        ),
        (
            "CONTAINS on usagetype",
            json!([{ "Type": "CONTAINS", "Field": "usagetype", "Value": "BoxUsage:t3" }]),
            vec!["T3MICROEUW1LINUX", "T3MICROUSE1LINUX", "T3SMALLUSE1LINUX"],
        ),
        (
            "ANY_OF on instanceType",
            json!([{ "Type": "ANY_OF", "Field": "instanceType", "Value": "t3.small,m5.large" }]),
            vec![
                "M5LARGEUSE1LINUX",
                "M5LARGEUSW2WINDOWS",
                "T3SMALLUSE1LINUX",
            ],
        ),
        (
            "NONE_OF on instanceType",
            json!([{ "Type": "NONE_OF", "Field": "instanceType", "Value": "t3.micro,t3.small" }]),
            vec!["M5LARGEUSE1LINUX", "M5LARGEUSW2WINDOWS"],
        ),
        (
            "TERM_MATCH on the ServiceCode field",
            json!([{ "Type": "TERM_MATCH", "Field": "ServiceCode", "Value": "AmazonS3" }]),
            vec![],
        ),
        (
            "two filters, both of which must hold",
            json!([
                { "Type": "TERM_MATCH", "Field": "instanceType", "Value": "t3.micro" },
                { "Type": "TERM_MATCH", "Field": "regionCode", "Value": "us-east-1" },
            ]),
            vec!["T3MICROUSE1LINUX"],
        ),
    ];

    for (case, filters, expected) in cases {
        let response = ok(
            &router,
            "GetProducts",
            &json!({ "ServiceCode": "AmazonEC2", "Filters": filters }),
        )
        .await;
        let mut actual = product_skus(&response);
        actual.sort();
        assert_eq!(
            actual,
            expected.iter().map(|s| (*s).to_string()).collect::<Vec<_>>(),
            "case {case}: expected {expected:?}, got {actual:?}"
        );
        assert!(
            actual.len() < all_skus.len(),
            "case {case}: a filter must return strictly fewer products than no filter \
             ({} of {})",
            actual.len(),
            all_skus.len()
        );
    }

    // A filter type outside the modelled `FilterType` enum is refused as invalid before
    // the handler runs, by the framework's own constraint validation — never a 501 and
    // never a silent pass-through that returns everything.
    err(
        &router,
        "GetProducts",
        &json!({
            "ServiceCode": "AmazonEC2",
            "Filters": [{ "Type": "STARTS_WITH", "Field": "instanceType", "Value": "t3" }],
        }),
        400,
        "ValidationException",
    )
    .await;
    err(
        &router,
        "GetProducts",
        &json!({ "ServiceCode": "NoSuchService" }),
        400,
        "NotFoundException",
    )
    .await;
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
        ("GetProducts", json!({ "ServiceCode": "AmazonEC2" }), 1, "PriceList"),
        ("GetProducts", json!({ "ServiceCode": "AmazonEC2" }), 2, "PriceList"),
        ("DescribeServices", json!({}), 1, "Services"),
        (
            "GetAttributeValues",
            json!({ "ServiceCode": "AmazonEC2", "AttributeName": "instanceType" }),
            2,
            "AttributeValues",
        ),
        (
            "ListPriceLists",
            json!({
                "ServiceCode": "AmazonEC2",
                "CurrencyCode": "USD",
                "EffectiveDate": JUL_1_2024,
            }),
            1,
            "PriceLists",
        ),
    ];

    for (operation, mut body, page_size, items_member) in cases.iter().cloned() {
        let unpaged = ok(&router, operation, &body).await;
        let expected = unpaged
            .get(items_member)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            expected.len() > 1,
            "case {operation}: the test needs more than one item to page over, got \
             {unpaged}"
        );

        let mut collected: Vec<Value> = Vec::new();
        let mut next_token: Option<String> = None;
        // Bounded so a token that never advances fails the test instead of hanging.
        for _ in 0..=expected.len() {
            body["MaxResults"] = json!(page_size);
            match &next_token {
                Some(token) => body["NextToken"] = json!(token),
                None => {
                    body.as_object_mut().expect("an object body").remove("NextToken");
                }
            }
            let page = ok(&router, operation, &body).await;
            let items = page
                .get(items_member)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
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

    // A token that does not decode, and one minted for a different query, are both
    // InvalidNextToken — never a page of some other result set.
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
    for (case, token) in [
        ("undecodable", "not-a-token".to_string()),
        ("minted for another query", ec2_token),
    ] {
        let (status, parsed) = call(
            &router,
            "GetProducts",
            &json!({ "ServiceCode": "AmazonS3", "NextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {parsed}"
        );
        assert_eq!(
            parsed.get("__type").and_then(Value::as_str),
            Some("InvalidNextTokenException"),
            "case {case}: expected InvalidNextTokenException, got {parsed}"
        );
    }
}

#[tokio::test]
async fn list_price_lists_selects_by_service_currency_region_and_effective_date() {
    let router = router();

    // Rows differ in the member that selects: effective date (before, between and after
    // the stored versions), Region, currency and service.
    let cases: &[(&str, Value, usize)] = &[
        (
            "every USD EC2 price list in effect on 2024-01-01",
            json!({
                "ServiceCode": "AmazonEC2",
                "CurrencyCode": "USD",
                "EffectiveDate": JAN_1_2024,
            }),
            3,
        ),
        (
            "a later effective date also sees the newer version",
            json!({
                "ServiceCode": "AmazonEC2",
                "CurrencyCode": "USD",
                "EffectiveDate": JUL_1_2024,
            }),
            4,
        ),
        (
            "no price list is in effect before the earliest one",
            json!({
                "ServiceCode": "AmazonEC2",
                "CurrencyCode": "USD",
                "EffectiveDate": JAN_1_2022,
            }),
            0,
        ),
        (
            "a RegionCode narrows the list to that Region",
            json!({
                "ServiceCode": "AmazonEC2",
                "CurrencyCode": "USD",
                "RegionCode": "eu-west-1",
                "EffectiveDate": JUL_1_2024,
            }),
            1,
        ),
        (
            "a Region with no price list for the service",
            json!({
                "ServiceCode": "AmazonS3",
                "CurrencyCode": "USD",
                "RegionCode": "us-west-2",
                "EffectiveDate": JUL_1_2024,
            }),
            0,
        ),
        (
            "another currency selects its own price lists",
            json!({
                "ServiceCode": "AmazonEC2",
                "CurrencyCode": "CNY",
                "EffectiveDate": JAN_1_2024,
            }),
            1,
        ),
        (
            "another service selects its own price lists",
            json!({
                "ServiceCode": "AmazonS3",
                "CurrencyCode": "USD",
                "EffectiveDate": JAN_1_2024,
            }),
            2,
        ),
    ];

    for (case, body, expected) in cases {
        let response = ok(&router, "ListPriceLists", body).await;
        let lists = response
            .get("PriceLists")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            lists.len(),
            *expected,
            "case {case}: expected {expected} price list(s), got {response}"
        );
        for entry in &lists {
            assert_eq!(
                entry.get("CurrencyCode"),
                body.get("CurrencyCode"),
                "case {case}: every entry must be denominated in the requested currency, \
                 got {entry}"
            );
            if let Some(region) = body.get("RegionCode") {
                assert_eq!(
                    entry.get("RegionCode"),
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
                entry
                    .get("FileFormats")
                    .and_then(Value::as_array)
                    .is_some_and(|formats| !formats.is_empty()),
                "case {case}: every entry must report its file formats, got {entry}"
            );
        }
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

    // The `RegionCode` values GetAttributeValues reports for the service are the ones
    // ListPriceLists selects by — the linkage the model documents on
    // `PriceList.RegionCode`.
    let regions = attribute_values(
        &ok(
            &router,
            "GetAttributeValues",
            &json!({ "ServiceCode": "AmazonEC2", "AttributeName": "regionCode" }),
        )
        .await,
    );
    for region in &regions {
        let response = ok(
            &router,
            "ListPriceLists",
            &json!({
                "ServiceCode": "AmazonEC2",
                "CurrencyCode": "USD",
                "RegionCode": region,
                "EffectiveDate": JUL_1_2024,
            }),
        )
        .await;
        let lists = response
            .get("PriceLists")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
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

    // A well-formed ISO-4217 code the Price List API does not publish price lists in
    // is a bad parameter, whichever service is asked for.
    for currency in ["EUR", "GBP", "JPY"] {
        err(
            &router,
            "ListPriceLists",
            &json!({
                "ServiceCode": "AmazonEC2",
                "CurrencyCode": currency,
                "EffectiveDate": JAN_1_2024,
            }),
            400,
            "InvalidParameterException",
        )
        .await;
    }
    // A currency the API DOES publish is never refused for being unmatched: every
    // supported currency answers 200, with only that currency's lists (possibly none).
    for service_code in ["AmazonEC2", "AmazonS3"] {
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
            for entry in response
                .get("PriceLists")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                assert_eq!(
                    entry.get("CurrencyCode").and_then(Value::as_str),
                    Some(currency),
                    "case {service_code}/{currency}: every entry must be denominated in \
                     the requested currency, got {entry}"
                );
            }
        }
    }
    // A code outside the modelled `^[A-Z]{3}$` pattern is refused before the handler
    // runs, by the framework's own constraint validation.
    err(
        &router,
        "ListPriceLists",
        &json!({
            "ServiceCode": "AmazonEC2",
            "CurrencyCode": "usd",
            "EffectiveDate": JAN_1_2024,
        }),
        400,
        "ValidationException",
    )
    .await;
    err(
        &router,
        "ListPriceLists",
        &json!({
            "ServiceCode": "NoSuchService",
            "CurrencyCode": "USD",
            "EffectiveDate": JAN_1_2024,
        }),
        400,
        "NotFoundException",
    )
    .await;
}

#[tokio::test]
async fn every_listed_price_list_arn_resolves_to_a_stable_file_url() {
    let (router, engine) = router_with_engine();

    let listed = ok(
        &router,
        "ListPriceLists",
        &json!({
            "ServiceCode": "AmazonEC2",
            "CurrencyCode": "USD",
            "EffectiveDate": JUL_1_2024,
        }),
    )
    .await;
    let entries = listed
        .get("PriceLists")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        !entries.is_empty(),
        "the round trip needs a listed price list, got {listed}"
    );

    let mut urls: BTreeSet<String> = BTreeSet::new();
    for entry in &entries {
        let arn = entry
            .get("PriceListArn")
            .and_then(Value::as_str)
            .expect("a listed price list carries its ARN");
        let formats: Vec<&str> = entry
            .get("FileFormats")
            .and_then(Value::as_array)
            .expect("a listed price list carries its file formats")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        for file_format in formats {
            let request = json!({ "PriceListArn": arn, "FileFormat": file_format });
            let first = ok(&router, "GetPriceListFileUrl", &request).await;
            let url = first
                .get("Url")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert!(
                url.ends_with(&format!(".{file_format}")),
                "case {arn}/{file_format}: expected a URL for the requested format, got \
                 {url:?}"
            );
            // The URL must address the emulator itself, on loopback — never a host off
            // this machine.
            let (host, path) = split_https_url(&url)
                .unwrap_or_else(|| panic!("case {arn}/{file_format}: not an https URL: {url:?}"));
            assert!(
                is_loopback(&host),
                "case {arn}/{file_format}: the file host must be loopback, got {host:?} \
                 in {url:?}"
            );

            // And something must actually be served there: the same renderer the
            // emulator's file listener runs produces the file from the catalog.
            let state = engine.state::<PricingState>();
            let (content_type, body) = render_price_list_file(&state, &path).unwrap_or_else(|| {
                panic!("case {arn}/{file_format}: nothing is served at {path:?}")
            });
            // The products the price list covers: the ones GetProducts returns for its
            // Region. The file must carry exactly these, priced as their offer
            // documents price them.
            let region = entry
                .get("RegionCode")
                .and_then(Value::as_str)
                .expect("a listed price list carries its Region");
            let covered = ok(
                &router,
                "GetProducts",
                &json!({
                    "ServiceCode": "AmazonEC2",
                    "Filters": [{
                        "Type": "TERM_MATCH",
                        "Field": "regionCode",
                        "Value": region,
                    }],
                }),
            )
            .await;
            let in_region = product_skus(&covered);
            match file_format {
                "json" => {
                    assert_eq!(
                        content_type, "application/json",
                        "case {arn}/{file_format}: wrong content type"
                    );
                    let file: Value = serde_json::from_str(&body).unwrap_or_else(|e| {
                        panic!("case {arn}/{file_format}: the file must be JSON: {e}")
                    });
                    assert_eq!(
                        file.get("offerCode").and_then(Value::as_str),
                        Some("AmazonEC2"),
                        "case {arn}/{file_format}: the file must name its service, got {file}"
                    );
                    assert_eq!(
                        file.get("currencyCode"),
                        entry.get("CurrencyCode"),
                        "case {arn}/{file_format}: the file must be denominated as listed"
                    );
                    let in_file: BTreeSet<String> = file
                        .get("products")
                        .and_then(Value::as_object)
                        .map(|products| products.keys().cloned().collect())
                        .unwrap_or_default();
                    assert_eq!(
                        in_file,
                        in_region.iter().cloned().collect::<BTreeSet<String>>(),
                        "case {arn}/{file_format}: the file must carry the products of \
                         its Region"
                    );
                    for sku in &in_region {
                        assert!(
                            file.pointer(&format!("/terms/OnDemand/{sku}")).is_some(),
                            "case {arn}/{file_format}: every product in the file must be \
                             priced, {sku} is not"
                        );
                    }
                }
                other => panic!("case {arn}: unexpected file format {other:?}"),
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

        // A format this price list is not published in is a bad parameter, not a 404.
        err(
            &router,
            "GetPriceListFileUrl",
            &json!({ "PriceListArn": arn, "FileFormat": "xml" }),
            400,
            "InvalidParameterException",
        )
        .await;
    }

    // An ARN the catalog does not hold names no resource.
    err(
        &router,
        "GetPriceListFileUrl",
        &json!({
            "PriceListArn": "arn:aws:pricing:::price-list/does-not-exist",
            "FileFormat": "json",
        }),
        404,
        "ResourceNotFoundException",
    )
    .await;
    // An ARN outside the modelled pattern is refused before the handler runs.
    err(
        &router,
        "GetPriceListFileUrl",
        &json!({ "PriceListArn": "not-an-arn", "FileFormat": "json" }),
        400,
        "ValidationException",
    )
    .await;

    // A path no price list publishes is served nothing, so a stray fetch cannot read
    // some other file.
    let state = engine.state::<PricingState>();
    assert!(
        render_price_list_file(&state, "/offers/v1.0/aws/AmazonEC2/nope/index.json").is_none(),
        "an unknown file path must serve nothing"
    );
}

/// Split an `https://host[:port]/path` URL into its authority and its path.
fn split_https_url(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("https://")?;
    let (authority, path) = rest.split_once('/')?;
    Some((authority.to_string(), format!("/{path}")))
}

/// Whether an `host:port` authority names this machine.
fn is_loopback(authority: &str) -> bool {
    let host = authority
        .rsplit_once(':')
        .map_or(authority, |(host, _port)| host);
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host == "127.0.0.1" || host == "::1" || host == "localhost"
}
