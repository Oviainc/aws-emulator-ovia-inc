// SPDX-License-Identifier: CC0-1.0

//! A Price List query that selects nothing is a successful empty page.
//!
//! The rule under test: in the catalog LISTING operations every input member is a
//! SELECTOR over the catalog — `DescribeServices$ServiceCode`,
//! `GetProducts$ServiceCode`/`Filters`, `GetAttributeValues$ServiceCode`/`AttributeName`
//! and `ListPriceLists$ServiceCode`/`RegionCode`/`EffectiveDate` — so a selection that
//! matches no item answers HTTP 200 with that operation's list member present, empty,
//! and no `NextToken`; the not-found errors these operations model belong to a request
//! that names ONE resource to resolve, which only `GetPriceListFileUrl` does.
//!
//! The rows span what the rule generalises over: which operation, which selector
//! misses, how many selectors miss at once, and the kind of miss (an unknown value, an
//! unknown field, a value of the wrong case, a bound no item reaches).

use serde_json::{json, Value};

use super::pricing_support::{
    attribute_values, call, err, list_member, ok, router, service_codes, JAN_1_2022, JUL_1_2024,
};

/// A service code no catalog publishes offers for. Nothing in the API reserves it; it
/// is simply absent, which is what makes every selector naming it select nothing.
const ABSENT_SERVICE_CODE: &str = "NotARealServiceXYZ";

#[tokio::test]
async fn a_selection_that_matches_nothing_is_an_empty_page() {
    let router = router();

    // (case, operation, request, the list member the operation pages)
    let cases: &[(&str, &str, Value, &str)] = &[
        (
            "GetProducts for a service the catalog has no offers for",
            "GetProducts",
            json!({ "ServiceCode": ABSENT_SERVICE_CODE }),
            "PriceList",
        ),
        (
            "GetProducts for an absent service with filters as well",
            "GetProducts",
            json!({
                "ServiceCode": ABSENT_SERVICE_CODE,
                "Filters": [
                    { "Type": "TERM_MATCH", "Field": "instanceType", "Value": "t3.micro" },
                ],
            }),
            "PriceList",
        ),
        (
            "GetProducts with a filter field no product carries",
            "GetProducts",
            json!({
                "ServiceCode": "AmazonEC2",
                "Filters": [
                    { "Type": "TERM_MATCH", "Field": "noSuchField", "Value": "anything" },
                ],
            }),
            "PriceList",
        ),
        (
            "GetProducts with a known field and a value no product takes",
            "GetProducts",
            json!({
                "ServiceCode": "AmazonEC2",
                "Filters": [
                    { "Type": "TERM_MATCH", "Field": "instanceType", "Value": "zz99.nano" },
                ],
            }),
            "PriceList",
        ),
        (
            "GetProducts with a value that differs only in case",
            "GetProducts",
            json!({
                "ServiceCode": "AmazonEC2",
                "Filters": [
                    { "Type": "TERM_MATCH", "Field": "operatingSystem", "Value": "LINUX" },
                ],
            }),
            "PriceList",
        ),
        (
            "GetProducts with two filters that cannot both hold",
            "GetProducts",
            json!({
                "ServiceCode": "AmazonEC2",
                "Filters": [
                    { "Type": "TERM_MATCH", "Field": "instanceType", "Value": "t3.micro" },
                    { "Type": "NONE_OF", "Field": "instanceType", "Value": "t3.micro" },
                ],
            }),
            "PriceList",
        ),
        (
            "GetProducts with an ANY_OF list of values no product takes",
            "GetProducts",
            json!({
                "ServiceCode": "AmazonS3",
                "Filters": [
                    { "Type": "ANY_OF", "Field": "regionCode", "Value": "us-east-9,eu-west-9" },
                ],
            }),
            "PriceList",
        ),
        (
            "GetProducts whose ServiceCode filter names another service",
            "GetProducts",
            json!({
                "ServiceCode": "AmazonS3",
                "Filters": [
                    { "Type": "TERM_MATCH", "Field": "ServiceCode", "Value": "AmazonEC2" },
                ],
            }),
            "PriceList",
        ),
        (
            "GetAttributeValues for a service the catalog has no offers for",
            "GetAttributeValues",
            json!({ "ServiceCode": ABSENT_SERVICE_CODE, "AttributeName": "instanceType" }),
            "AttributeValues",
        ),
        (
            "GetAttributeValues for an attribute the service does not publish",
            "GetAttributeValues",
            json!({ "ServiceCode": "AmazonEC2", "AttributeName": "noSuchAttribute" }),
            "AttributeValues",
        ),
        (
            "GetAttributeValues for an attribute of another service",
            "GetAttributeValues",
            json!({ "ServiceCode": "AmazonS3", "AttributeName": "instanceType" }),
            "AttributeValues",
        ),
        (
            "GetAttributeValues for a published attribute in the wrong case",
            "GetAttributeValues",
            json!({ "ServiceCode": "AmazonEC2", "AttributeName": "InstanceType" }),
            "AttributeValues",
        ),
        (
            "ListPriceLists for a service the catalog has no price lists for",
            "ListPriceLists",
            json!({
                "ServiceCode": ABSENT_SERVICE_CODE,
                "CurrencyCode": "USD",
                "EffectiveDate": JUL_1_2024,
            }),
            "PriceLists",
        ),
        (
            "ListPriceLists for a Region no price list covers",
            "ListPriceLists",
            json!({
                "ServiceCode": "AmazonEC2",
                "CurrencyCode": "USD",
                "RegionCode": "us-east-9",
                "EffectiveDate": JUL_1_2024,
            }),
            "PriceLists",
        ),
        (
            "ListPriceLists at an instant before any price list took effect",
            "ListPriceLists",
            json!({
                "ServiceCode": "AmazonEC2",
                "CurrencyCode": "USD",
                "EffectiveDate": JAN_1_2022,
            }),
            "PriceLists",
        ),
        (
            "ListPriceLists in a published currency the service has no list in",
            "ListPriceLists",
            json!({
                "ServiceCode": "AmazonS3",
                "CurrencyCode": "CNY",
                "RegionCode": "us-east-1",
                "EffectiveDate": JUL_1_2024,
            }),
            "PriceLists",
        ),
        (
            "DescribeServices for a service the catalog does not hold",
            "DescribeServices",
            json!({ "ServiceCode": ABSENT_SERVICE_CODE }),
            "Services",
        ),
    ];

    for (case, operation, request, member) in cases {
        let (status, parsed) = call(&router, operation, request).await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} with {parsed}"
        );
        let selected = parsed.get(member);
        assert!(
            selected.is_some_and(|value| value.is_array()),
            "case {case}: expected the {member} member, got {parsed}"
        );
        let selected = list_member(&parsed, member);
        assert!(
            selected.is_empty(),
            "case {case}: expected no {member} items, got {selected:?}"
        );
        assert!(
            parsed.get("NextToken").is_none_or(Value::is_null),
            "case {case}: an empty page must carry no NextToken, got {parsed}"
        );
        // The operations that report the catalog's format version report it on an empty
        // page too: the page is a successful answer, not a partial one.
        if matches!(*operation, "GetProducts" | "DescribeServices") {
            assert_eq!(
                parsed.get("FormatVersion").and_then(Value::as_str),
                Some("aws_v1"),
                "case {case}: expected the catalog format version, got {parsed}"
            );
        }
    }

    // The rows above must be empty because they select nothing, not because the
    // operation answers empty to everything: the same operations still return items for
    // a selection the catalog does satisfy, after all the empty calls above.
    let products = ok(
        &router,
        "GetProducts",
        &json!({
            "ServiceCode": "AmazonEC2",
            "Filters": [
                { "Type": "TERM_MATCH", "Field": "instanceType", "Value": "t3.micro" },
            ],
        }),
    )
    .await;
    assert!(
        !list_member(&products, "PriceList").is_empty(),
        "case a selection the catalog satisfies: expected products, got {products}"
    );
    let values = ok(
        &router,
        "GetAttributeValues",
        &json!({ "ServiceCode": "AmazonEC2", "AttributeName": "instanceType" }),
    )
    .await;
    assert!(
        !attribute_values(&values).is_empty(),
        "case a published attribute: expected values, got {values}"
    );
    let lists = ok(
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
    assert!(
        !list_member(&lists, "PriceLists").is_empty(),
        "case a Region with price lists: expected price lists, got {lists}"
    );
}

#[tokio::test]
async fn every_attribute_a_service_does_not_publish_yields_an_empty_list() {
    let router = router();

    // The catalog's own services, read from DescribeServices rather than written down,
    // each probed with an attribute name that service does not publish — derived by
    // suffixing a published name, so the row is absent whatever the catalog holds.
    let all = ok(&router, "DescribeServices", &json!({})).await;
    let codes = service_codes(&all);
    assert!(
        codes.len() > 1,
        "the catalog must publish more than one service for this rule to span, got {codes:?}"
    );

    for service_code in &codes {
        let published: Vec<String> = list_member(&all, "Services")
            .into_iter()
            .find(|entry| entry.get("ServiceCode").and_then(Value::as_str) == Some(service_code))
            .map(|entry| {
                list_member(&entry, "AttributeNames")
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let first = published
            .first()
            .unwrap_or_else(|| panic!("case {service_code}: expected published attributes"));
        let absent = format!("{first}NotPublished");

        let response = ok(
            &router,
            "GetAttributeValues",
            &json!({ "ServiceCode": service_code, "AttributeName": absent }),
        )
        .await;
        let actual = attribute_values(&response);
        assert!(
            actual.is_empty(),
            "case {service_code}/{absent}: expected no values, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_request_that_names_one_resource_still_refuses_an_unresolvable_one() {
    let router = router();

    // The other side of the rule: GetPriceListFileUrl resolves ONE named price-list
    // file, so an ARN no price list has is a not-found resource rather than an empty
    // selection — the empty-page rule must not swallow it.
    err(
        &router,
        "GetPriceListFileUrl",
        &json!({
            "PriceListArn": format!(
                "arn:aws:pricing:::price-list/aws/{ABSENT_SERVICE_CODE}/USD/20240101000000/us-east-1"
            ),
            "FileFormat": "json",
        }),
        400,
        "NotFoundException",
    )
    .await;

    // A listing for the same absent service is still an empty page, not that refusal.
    let listed = ok(
        &router,
        "ListPriceLists",
        &json!({
            "ServiceCode": ABSENT_SERVICE_CODE,
            "CurrencyCode": "USD",
            "EffectiveDate": JUL_1_2024,
        }),
    )
    .await;
    assert!(
        list_member(&listed, "PriceLists").is_empty(),
        "case the absent service's listing: expected an empty PriceLists, got {listed}"
    );
}
