// SPDX-License-Identifier: CC0-1.0

//! What the Price List catalog PUBLISHES, over tables of inputs rather than one
//! recorded request.
//!
//! The rules under test:
//!
//! * `GetAttributeValues` enumerates the whole published value set of an attribute, in
//!   one defined order, and carries a `NextToken` whenever that set outruns the
//!   requested page — so a client that pages sees every value exactly once.
//! * every value it publishes is a value some product of the same service carries, so a
//!   `GetProducts` filter built from the enumeration matches something.
//! * a filtered `GetProducts` query whose matched set outruns the requested page carries
//!   a `NextToken` too, and paging it yields that set once.
//! * `ListPriceLists` reports each Region's price list once, at the version in effect at
//!   the requested `EffectiveDate`, advertising exactly the formats the catalog serves a
//!   file for; every one of them resolves through `GetPriceListFileUrl`, and the flat
//!   (CSV) and nested (JSON) forms of a file carry the same priced dimensions.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_pricing_awspricelistservice::state::{
    render_price_list_file, PricingState,
};

use super::pricing_support::{
    all_attribute_values, all_offer_documents, attribute_values, column_of, csv_rows, list_member,
    offer_attribute, ok, product_sku, router, router_with_engine, split_https_url, JAN_1_2024,
    JUL_1_2024, MAX_PAGE,
};

/// Instance types the Price List API publishes `instanceType` values for, across the
/// families and generations the catalog prices: the source is the published instance-type
/// list, and none of these is a type the catalog's smaller seeds named.
const PUBLISHED_INSTANCE_TYPES: &[&str] = &[
    "a1.medium",
    "a1.2xlarge",
    "a1.metal",
    "c4.large",
    "c5.9xlarge",
    "c5a.24xlarge",
    "c5n.metal",
    "c6g.medium",
    "c6i.32xlarge",
    "d2.8xlarge",
    "g4dn.xlarge",
    "i3.metal",
    "m4.10xlarge",
    "m5.metal",
    "m5a.4xlarge",
    "m5n.large",
    "m6g.medium",
    "m6i.32xlarge",
    "p3.16xlarge",
    "r4.16xlarge",
    "r5.24xlarge",
    "r6g.medium",
    "t2.nano",
    "t3a.small",
    "t4g.nano",
    "x1.32xlarge",
    "x1e.xlarge",
    "z1d.3xlarge",
];

/// The `yyyymmddHHMMSS` stamp an instant names, for comparing a price-list version
/// against the `EffectiveDate` it was selected for.
fn version_stamp_at(epoch_secs: i64) -> String {
    ::aws_smithy_types::DateTime::from_secs(epoch_secs)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
        .chars()
        .filter(char::is_ascii_digit)
        .collect()
}

/// The version segment of a price-list ARN (`…/<currency>/<version>/<region>`).
fn arn_version(arn: &str) -> String {
    arn.rsplit('/').nth(1).unwrap_or_default().to_string()
}

#[tokio::test]
async fn an_attribute_enumeration_spans_pages_and_yields_every_value_once() {
    let router = router();

    let query = json!({ "ServiceCode": "AmazonEC2", "AttributeName": "instanceType" });
    let published = all_attribute_values(&router, &query).await;

    // The published value set, not a sample of it: every type the catalog prices is in
    // it, including ones no smaller seed named.
    for instance_type in PUBLISHED_INSTANCE_TYPES.iter().copied() {
        assert!(
            published.iter().any(|value| value == instance_type),
            "case {instance_type}: the published instance-type vocabulary must carry it, \
             got {} values",
            published.len()
        );
    }
    assert!(
        published.len() > usize::try_from(MAX_PAGE).expect("a small page"),
        "the published value set must outrun one page of {MAX_PAGE}, got {}",
        published.len()
    );

    // One defined order: ascending, with no value twice — so a page boundary always
    // names the same place in the set.
    let mut ascending = published.clone();
    ascending.sort();
    ascending.dedup();
    assert_eq!(
        published, ascending,
        "the values must be enumerated in ascending order, without duplicates"
    );

    // Rows differ in page size, including both published bounds of MaxResults on this
    // operation (1..10000, so the whole set in one page) and the largest page the other
    // listings take: each page is the next slice of the same set, and the token is
    // present for exactly as long as values remain.
    for page_size in [1_i64, 2, 5, 10, MAX_PAGE] {
        let mut walked: Vec<String> = Vec::new();
        let mut body = query.clone();
        body["MaxResults"] = json!(page_size);
        let mut pages = 0usize;
        loop {
            let page = ok(&router, "GetAttributeValues", &body).await;
            let values = attribute_values(&page);
            assert!(
                values.len() <= usize::try_from(page_size).expect("a small page"),
                "case MaxResults={page_size} page {pages}: a page must not exceed the \
                 page size, got {}",
                values.len()
            );
            walked.extend(values);
            pages += 1;
            let token = page
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            assert_eq!(
                token.is_some(),
                walked.len() < published.len(),
                "case MaxResults={page_size} page {pages}: expected a NextToken while \
                 {} of {} values remain, got {page}",
                published.len().saturating_sub(walked.len()),
                published.len()
            );
            match token {
                Some(token) => body["NextToken"] = json!(token),
                None => break,
            }
            assert!(
                pages <= published.len(),
                "case MaxResults={page_size}: paging must terminate"
            );
        }
        assert_eq!(
            walked, published,
            "case MaxResults={page_size}: paging must yield every value once, in order"
        );
    }

    // A value set smaller than the page carries no token at all: the token says more
    // remains, not that the operation is paginated.
    let small = ok(
        &router,
        "GetAttributeValues",
        &json!({
            "ServiceCode": "AmazonEC2",
            "AttributeName": "volumeType",
            "MaxResults": MAX_PAGE,
        }),
    )
    .await;
    assert!(
        small.get("NextToken").is_none_or(Value::is_null),
        "case volumeType/MaxResults={MAX_PAGE}: a complete page carries no NextToken, \
         got {small}"
    );
}

#[tokio::test]
async fn every_published_attribute_value_selects_a_priced_product() {
    let router = router();

    // Rows: every value of the attribute whose value set is the largest the catalog
    // publishes. A client that enumerates an attribute and filters on what it read must
    // get products back — so each value selects at least one, and every product it
    // selects carries that value and is priced.
    let published = all_attribute_values(
        &router,
        &json!({ "ServiceCode": "AmazonEC2", "AttributeName": "instanceType" }),
    )
    .await;
    assert!(
        !published.is_empty(),
        "the enumeration must publish values to filter on"
    );

    for instance_type in &published {
        let documents = all_offer_documents(
            &router,
            &json!({
                "ServiceCode": "AmazonEC2",
                "Filters": [
                    { "Type": "TERM_MATCH", "Field": "instanceType", "Value": instance_type },
                ],
            }),
        )
        .await;
        assert!(
            !documents.is_empty(),
            "case instanceType={instance_type}: an enumerated value must select at least \
             one product"
        );
        for document in &documents {
            assert_eq!(
                offer_attribute(document, "instanceType").as_deref(),
                Some(instance_type.as_str()),
                "case instanceType={instance_type}: a selected product must carry the \
                 filtered value, got {document}"
            );
            assert!(
                document.pointer("/terms/OnDemand").is_some(),
                "case instanceType={instance_type}: {} must be priced on demand",
                product_sku(document)
            );
        }
    }
}

#[tokio::test]
async fn a_filtered_query_longer_than_the_page_is_carried_on_a_next_token() {
    let router = router();

    // Rows differ in the filter and in how many products it selects, from a whole
    // service down to one dimension of it — each selecting more than one page of the
    // published catalog, which is what makes the token a requirement rather than an
    // option.
    let filters: &[Value] = &[
        json!([{ "Type": "TERM_MATCH", "Field": "ServiceCode", "Value": "AmazonEC2" }]),
        json!([{ "Type": "TERM_MATCH", "Field": "location", "Value": "US East (N. Virginia)" }]),
        json!([{ "Type": "TERM_MATCH", "Field": "operatingSystem", "Value": "Linux" }]),
        json!([
            { "Type": "TERM_MATCH", "Field": "instanceFamily", "Value": "General purpose" },
            { "Type": "TERM_MATCH", "Field": "tenancy", "Value": "Shared" },
        ]),
        json!([{ "Type": "CONTAINS", "Field": "usagetype", "Value": "BoxUsage" }]),
    ];

    for filter in filters {
        let case = filter.to_string();
        let query = json!({ "ServiceCode": "AmazonEC2", "Filters": filter });
        let matched: Vec<String> = all_offer_documents(&router, &query)
            .await
            .iter()
            .map(product_sku)
            .collect();
        let distinct: BTreeSet<&String> = matched.iter().collect();
        assert_eq!(
            distinct.len(),
            matched.len(),
            "case {case}: paging must not repeat a product"
        );
        assert!(
            matched.len() > usize::try_from(MAX_PAGE).expect("a small page"),
            "case {case}: the matched set must outrun one page of {MAX_PAGE}, got {}",
            matched.len()
        );

        // At the largest page the operation takes, the answer still reports that more
        // remains — and the token walks to exactly the matched set.
        for page_size in [1_i64, 10, MAX_PAGE] {
            let mut body = query.clone();
            body["MaxResults"] = json!(page_size);
            let first = ok(&router, "GetProducts", &body).await;
            assert!(
                first
                    .get("NextToken")
                    .and_then(Value::as_str)
                    .is_some_and(|token| !token.is_empty()),
                "case {case}/MaxResults={page_size}: expected a NextToken while products \
                 remain, got {first}"
            );
        }
    }
}

#[tokio::test]
async fn a_price_list_is_reported_at_the_version_in_effect_and_in_every_format_it_serves() {
    let (router, engine) = router_with_engine();

    // Rows differ in the service, the Region the price list covers and the currency its
    // files are denominated in, including a service priced with one usage type per
    // Region and the partition whose files are not denominated in USD.
    let cases: &[(&str, &str, &str)] = &[
        ("AmazonEC2", "us-east-1", "USD"),
        ("AmazonEC2", "eu-west-1", "USD"),
        ("AmazonEC2", "cn-north-1", "CNY"),
        ("AmazonS3", "us-east-1", "USD"),
        ("AWSLambda", "us-east-1", "USD"),
    ];

    for (service_code, region_code, currency_code) in cases.iter().copied() {
        // Rows over the EffectiveDate too: the instant selects a version, so each
        // instant reports the Region once, at the newest version it reaches.
        let mut by_date: BTreeMap<i64, String> = BTreeMap::new();
        for effective_date in [JAN_1_2024, JUL_1_2024] {
            let case = format!("{service_code}/{region_code}@{effective_date}");
            let listed = ok(
                &router,
                "ListPriceLists",
                &json!({
                    "ServiceCode": service_code,
                    "CurrencyCode": currency_code,
                    "RegionCode": region_code,
                    "EffectiveDate": effective_date,
                }),
            )
            .await;
            let entries = list_member(&listed, "PriceLists");
            assert_eq!(
                entries.len(),
                1,
                "case {case}: the Region's price list must be reported once, got {listed}"
            );
            let entry = &entries[0];
            let arn = entry
                .get("PriceListArn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let version = arn_version(&arn);
            assert!(
                version.len() == 14 && version.chars().all(char::is_numeric),
                "case {case}: the ARN must name a published version, got {arn}"
            );
            assert!(
                version <= version_stamp_at(effective_date),
                "case {case}: a reported version must already be in effect at the \
                 requested date, got {version}"
            );
            by_date.insert(effective_date, version.clone());

            // FileFormats is a contract: exactly the formats the catalog serves a file
            // for, each resolvable through GetPriceListFileUrl and actually served.
            let formats: Vec<String> = list_member(entry, "FileFormats")
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            assert!(
                formats.len() > 1,
                "case {case}: a price list is published in more than one format, got \
                 {formats:?}"
            );

            let mut skus_per_format: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
            let mut dimensions_per_format: BTreeMap<String, usize> = BTreeMap::new();
            for file_format in &formats {
                let resolved = ok(
                    &router,
                    "GetPriceListFileUrl",
                    &json!({ "PriceListArn": arn, "FileFormat": file_format }),
                )
                .await;
                let url = resolved
                    .get("Url")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                assert!(
                    url.ends_with(&format!(".{file_format}")),
                    "case {case}/{file_format}: a listed format must resolve to a URL for \
                     it, got {url:?}"
                );
                let (_, path) = split_https_url(&url).unwrap_or_else(|| {
                    panic!("case {case}/{file_format}: not an https URL: {url:?}")
                });
                let state = engine.state::<PricingState>();
                let (content_type, body) = render_price_list_file(&state, &path)
                    .unwrap_or_else(|| {
                        panic!("case {case}/{file_format}: nothing is served at {path:?}")
                    });
                match file_format.as_str() {
                    "json" => {
                        assert_eq!(
                            content_type, "application/json",
                            "case {case}/{file_format}: wrong media type"
                        );
                        let file: Value = serde_json::from_str(&body).unwrap_or_else(|e| {
                            panic!("case {case}/{file_format}: must be JSON: {e}")
                        });
                        assert_eq!(
                            file.get("version").and_then(Value::as_str),
                            Some(version.as_str()),
                            "case {case}/{file_format}: the file must state the version \
                             its ARN names"
                        );
                        let skus: BTreeSet<String> = file
                            .get("products")
                            .and_then(Value::as_object)
                            .map(|products| products.keys().cloned().collect())
                            .unwrap_or_default();
                        let dimensions = json_dimensions(&file);
                        skus_per_format.insert(file_format.clone(), skus);
                        dimensions_per_format.insert(file_format.clone(), dimensions);
                    }
                    "csv" => {
                        assert_eq!(
                            content_type, "text/csv",
                            "case {case}/{file_format}: wrong media type"
                        );
                        let (header, rows) = csv_rows(&body);
                        assert!(
                            !rows.is_empty(),
                            "case {case}/{file_format}: the file must carry priced rows"
                        );
                        let skus: BTreeSet<String> = rows
                            .iter()
                            .filter_map(|row| column_of(&header, row, "SKU"))
                            .collect();
                        for row in &rows {
                            assert_eq!(
                                column_of(&header, row, "Currency").as_deref(),
                                Some(currency_code),
                                "case {case}/{file_format}: every row is denominated as \
                                 listed, got {row:?}"
                            );
                        }
                        skus_per_format.insert(file_format.clone(), skus);
                        dimensions_per_format.insert(file_format.clone(), rows.len());
                    }
                    other => {
                        panic!("case {case}: {other} is listed but no file is served for it")
                    }
                }
            }
            // Flat and nested are two forms of ONE price list: the same products, priced
            // over the same number of dimensions.
            let distinct_skus: BTreeSet<&BTreeSet<String>> = skus_per_format.values().collect();
            assert_eq!(
                distinct_skus.len(),
                1,
                "case {case}: every format of a price list must carry the same products, \
                 got {skus_per_format:?}"
            );
            let distinct_dimensions: BTreeSet<&usize> = dimensions_per_format.values().collect();
            assert_eq!(
                distinct_dimensions.len(),
                1,
                "case {case}: every format must price the same dimensions, got \
                 {dimensions_per_format:?}"
            );
        }

        // The later instant names a newer version: the ARN a client caches is keyed to
        // the date it asked about.
        assert!(
            by_date[&JUL_1_2024] > by_date[&JAN_1_2024],
            "case {service_code}/{region_code}: a later EffectiveDate must select a newer \
             version, got {by_date:?}"
        );
    }
}

/// How many price dimensions a price-list file's nested form carries: the file nests
/// `terms.<term type>.<SKU>.<offer term code>.priceDimensions.<rate code>`.
fn json_dimensions(file: &Value) -> usize {
    let mut dimensions = 0usize;
    let Some(by_term_type) = file.get("terms").and_then(Value::as_object) else {
        return dimensions;
    };
    for by_sku in by_term_type.values() {
        for by_term_code in by_sku.as_object().into_iter().flat_map(|skus| skus.values()) {
            for term in by_term_code
                .as_object()
                .into_iter()
                .flat_map(|terms| terms.values())
            {
                dimensions += term
                    .get("priceDimensions")
                    .and_then(Value::as_object)
                    .map_or(0, serde_json::Map::len);
            }
        }
    }
    dimensions
}
