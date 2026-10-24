// SPDX-License-Identifier: CC0-1.0

//! What a row of the offering-RATE catalog reports, and how the rate search
//! filters and pages it.
//!
//! Four rules, each asserted over a table of searches and over whole rate sets
//! rather than over the one row a recording happened to carry:
//!
//! * a rate names the offering it is priced under and the usage dimension it
//!   prices, and an offering prices exactly the products it covers;
//! * a rate is the product's on-demand price less the discount its parent
//!   offering earns, so the same usage dimension is priced strictly lower as the
//!   term lengthens, as more of the commitment is paid up front and as the plan
//!   type commits to more — and, within one offering, a rate scales with the
//!   size of the usage it prices;
//! * every filter the request carries narrows the result, and a rate that is not
//!   dimensioned by the property a filter names cannot satisfy it;
//! * the search answers with a resumable cursor whenever it returned anything,
//!   and paging yields the result set once, in order.
//!
//! Everything goes through the same `MultiServiceRouter` the emulator boots, so
//! what is pinned is the HTTP behaviour. The harness (`Emulator`, `text`) is the
//! one the sibling lifecycle suite defines.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_savingsplans_awssavingsplan::ops::types::{
    SAVINGS_PLAN_RATE_PROPERTY_KEY_VALUES, SAVINGS_PLAN_RATE_SERVICE_CODE_VALUES,
    SAVINGS_PLAN_RATE_UNIT_VALUES,
};
use service_com_amazonaws_savingsplans_awssavingsplan::state;

use crate::savings_plans_lifecycle::{text, Emulator};

/// The operation under test.
const OPERATION: &str = "DescribeSavingsPlansOfferingRates";

/// One row of the narrowing table: the case, the search, and what every rate the
/// search admits must then report, as `(locator, value)` pairs addressed by
/// [`field`].
type NarrowingCase = (&'static str, Value, &'static [(&'static str, &'static str)]);

/// The terms the catalog sells, in seconds (`SavingsPlanOffering$durationSeconds`).
const ONE_YEAR: i64 = 31_536_000;
/// See [`ONE_YEAR`].
const THREE_YEARS: i64 = 94_608_000;

/// The parent offering a rate is priced under
/// (`SavingsPlanOfferingRate$savingsPlanOffering`).
fn parent(rate: &Value) -> &Value {
    rate.get("savingsPlanOffering").unwrap_or(&Value::Null)
}

/// The value of a rate property (`SavingsPlanRatePropertyKey`), or `""` when the
/// rate is not dimensioned by it.
fn property(rate: &Value, name: &str) -> String {
    rate.get("properties")
        .and_then(Value::as_array)
        .and_then(|properties| {
            properties
                .iter()
                .find(|property| text(property, "name") == name)
        })
        .map(|property| text(property, "value"))
        .unwrap_or_default()
}

/// A field of a rate addressed by a table row: a bare name is the rate's own
/// member, `parent.<name>` a member of the offering it is priced under, and
/// `property.<name>` one of its dimensions.
fn field(rate: &Value, locator: &str) -> String {
    match locator.split_once('.') {
        Some(("parent", name)) => text(parent(rate), name),
        Some(("property", name)) => property(rate, name),
        _ => text(rate, locator),
    }
}

/// What distinguishes one rate from another: the offering it is priced under and
/// the line item it prices.
fn key(rate: &Value) -> String {
    format!(
        "{}|{}|{}|{}",
        text(parent(rate), "offeringId"),
        text(rate, "productType"),
        text(rate, "serviceCode"),
        text(rate, "usageType")
    )
}

/// A rate as a whole number of [`state::AMOUNT_SCALE`] units, which is also an
/// assertion that it is spelled at the service's one fixed scale.
fn rate_units(case: &str, rate: &Value) -> i128 {
    let rendered = text(rate, "rate");
    let Some((whole, fraction)) = rendered.split_once('.') else {
        panic!("case {case:?}: a rate must be a decimal amount, got {rendered:?}");
    };
    assert_eq!(
        fraction.len(),
        state::AMOUNT_WIRE_DIGITS,
        "case {case:?}: a rate must carry {} fractional digits, got {rendered:?}",
        state::AMOUNT_WIRE_DIGITS
    );
    let whole: i128 = whole
        .parse()
        .unwrap_or_else(|_| panic!("case {case:?}: {rendered:?} is not a decimal amount"));
    let fraction: i128 = fraction
        .parse()
        .unwrap_or_else(|_| panic!("case {case:?}: {rendered:?} is not a decimal amount"));
    whole * state::AMOUNT_SCALE + fraction
}

/// Every page of a rate search, as `(rows, nextToken)`.
///
/// `request` is sent as supplied for the first page and with `nextToken` added
/// for each page after it.
async fn pages(
    emulator: &Emulator,
    case: &str,
    request: &Value,
) -> Vec<(Vec<Value>, Option<String>)> {
    let mut pages: Vec<(Vec<Value>, Option<String>)> = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut page_request = request.clone();
        if let Some(token) = &next {
            page_request["nextToken"] = json!(token);
        }
        let answer = emulator.call(OPERATION, page_request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 for {page_request}, got {} {:?}",
            answer.status, answer.body
        );
        next = answer
            .get("nextToken")
            .as_str()
            .map(std::string::ToString::to_string);
        pages.push((answer.list("searchResults").to_vec(), next.clone()));
        assert!(
            pages.len() <= 400,
            "case {case:?}: paging did not terminate after {} pages",
            pages.len()
        );
        if next.is_none() {
            return pages;
        }
    }
}

/// Every rate a search admits, walked through the search's own cursor.
async fn all_rates(emulator: &Emulator, case: &str, request: &Value) -> Vec<Value> {
    pages(emulator, case, request)
        .await
        .into_iter()
        .flat_map(|(rows, _)| rows)
        .collect()
}

/// The `productTypes` an offering reports.
fn product_types(offering: &Value) -> BTreeSet<String> {
    offering
        .get("productTypes")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|product| product.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `token` is a `PaginationToken` the model admits (length 0..1024,
/// pattern `^[A-Za-z0-9/=\+]+$`).
fn is_pagination_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 1024
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'=' | b'+'))
}

/// Every rate names the offering it is priced under and the dimension it
/// prices, and an offering prices exactly the products it covers.
///
/// The rows span the plan types, terms and payment options the catalog sells —
/// a plan type scoped to no region, one scoped to a region and an instance
/// family, one scoped to a region, and one that covers a whole family of
/// services — so the rule is not asserted at one catalog row.
#[tokio::test]
async fn every_rate_names_its_parent_offering_and_the_dimension_it_prices() {
    let emulator = Emulator::new();
    // The model's own value sets, which every reported value must come from.
    let service_codes: BTreeSet<&str> = SAVINGS_PLAN_RATE_SERVICE_CODE_VALUES
        .iter()
        .copied()
        .collect();
    let units: BTreeSet<&str> = SAVINGS_PLAN_RATE_UNIT_VALUES.iter().copied().collect();
    let property_keys: BTreeSet<&str> = SAVINGS_PLAN_RATE_PROPERTY_KEY_VALUES
        .iter()
        .copied()
        .collect();

    for (plan_type, payment_option, duration) in [
        ("Compute", "No Upfront", ONE_YEAR),
        ("EC2Instance", "All Upfront", THREE_YEARS),
        ("SageMaker", "Partial Upfront", ONE_YEAR),
        ("Database", "No Upfront", THREE_YEARS),
    ] {
        let offering = emulator.offering(plan_type, payment_option, duration).await;
        let offering_id = text(&offering, "offeringId");
        let case = format!("{plan_type}/{payment_option}/{duration}");
        let rates = all_rates(
            &emulator,
            &case,
            &json!({ "savingsPlanOfferingIds": [offering_id.clone()], "maxResults": 1000 }),
        )
        .await;
        assert!(
            !rates.is_empty(),
            "case {case:?}: an offering must price the usage it covers, got no rate"
        );

        let mut keys: BTreeSet<String> = BTreeSet::new();
        let mut priced: BTreeSet<String> = BTreeSet::new();
        for rate in &rates {
            let row = format!("{case} {}", key(rate));

            // The parent offering, reported in full.
            assert_eq!(
                text(parent(rate), "offeringId"),
                offering_id,
                "case {row:?}: a rate must name the offering it is priced under"
            );
            assert_eq!(
                text(parent(rate), "paymentOption"),
                payment_option,
                "case {row:?}: the parent offering's payment option must be the offering's"
            );
            assert_eq!(
                text(parent(rate), "planType"),
                plan_type,
                "case {row:?}: the parent offering's plan type must be the offering's"
            );
            assert_eq!(
                parent(rate).get("durationSeconds").and_then(Value::as_i64),
                Some(duration),
                "case {row:?}: the parent offering's term must be the offering's, got {:?}",
                parent(rate).get("durationSeconds")
            );
            assert_eq!(
                text(parent(rate), "currency"),
                text(&offering, "currency"),
                "case {row:?}: the parent offering's currency must be the offering's"
            );
            assert_eq!(
                text(parent(rate), "planDescription"),
                text(&offering, "description"),
                "case {row:?}: the parent offering's description must be the offering's"
            );

            // The line item, from the model's own value sets.
            let product_type = text(rate, "productType");
            assert!(
                product_types(&offering).contains(&product_type),
                "case {row:?}: a rate must price a product the offering covers, got {product_type:?} against {:?}",
                product_types(&offering)
            );
            let service_code = text(rate, "serviceCode");
            assert!(
                service_codes.contains(service_code.as_str()),
                "case {row:?}: serviceCode {service_code:?} is not a SavingsPlanRateServiceCode"
            );
            let unit = text(rate, "unit");
            assert!(
                units.contains(unit.as_str()),
                "case {row:?}: unit {unit:?} is not a SavingsPlanRateUnit"
            );
            // SavingsPlanRateUsageType: length 0..255, pattern ^[a-zA-Z0-9_ \/.:-]+$.
            let usage_type = text(rate, "usageType");
            assert!(
                !usage_type.is_empty()
                    && usage_type.len() <= 255
                    && usage_type.bytes().all(|b| b.is_ascii_alphanumeric()
                        || matches!(b, b'_' | b' ' | b'/' | b'.' | b':' | b'-')),
                "case {row:?}: usageType {usage_type:?} must satisfy the model's constraint"
            );
            assert!(
                rate_units(&row, rate) > 0,
                "case {row:?}: a discounted price is still a price, got {:?}",
                text(rate, "rate")
            );

            // The dimensions, each a modelled property key, each reported once,
            // and the usage type naming the region the line item is billed in.
            let mut seen: BTreeSet<String> = BTreeSet::new();
            for entry in rate.get("properties").and_then(Value::as_array).unwrap_or(&Vec::new()) {
                let name = text(entry, "name");
                let value = text(entry, "value");
                assert!(
                    property_keys.contains(name.as_str()),
                    "case {row:?}: property {name:?} is not a SavingsPlanRatePropertyKey"
                );
                assert!(
                    !value.is_empty(),
                    "case {row:?}: property {name:?} must carry a value"
                );
                assert!(
                    seen.insert(name.clone()),
                    "case {row:?}: property {name:?} is reported twice"
                );
            }
            let region = property(rate, "region");
            assert!(
                !region.is_empty(),
                "case {row:?}: a line item is billed in a region, got no region property"
            );
            assert!(
                usage_type.starts_with(&format!("{region}-")),
                "case {row:?}: a usage type billed in {region:?} must name it, got {usage_type:?}"
            );
            // An offering scoped to a region or an instance family prices only
            // what it is scoped to.
            for (scope, actual) in [
                ("region", region.clone()),
                ("instanceFamily", property(rate, "instanceFamily")),
            ] {
                let scoped = offering
                    .get("properties")
                    .and_then(Value::as_array)
                    .and_then(|properties| {
                        properties
                            .iter()
                            .find(|property| text(property, "name") == scope)
                    })
                    .map(|property| text(property, "value"));
                if let Some(scoped) = scoped {
                    assert_eq!(
                        actual, scoped,
                        "case {row:?}: an offering scoped to {scope} {scoped:?} must price only that {scope}"
                    );
                }
            }

            assert!(
                keys.insert(key(rate)),
                "case {row:?}: a line item must be priced once under one offering"
            );
            priced.insert(product_type);
        }

        assert_eq!(
            priced,
            product_types(&offering),
            "case {case:?}: an offering must price every product it covers and no other"
        );
    }
}

/// A rate is the product's on-demand price less the discount its parent
/// offering earns: the same usage dimension is priced strictly lower as the term
/// lengthens, as more of the commitment is paid up front, and as the plan type
/// commits to more.
///
/// The rows are two usage dimensions that differ in region and in instance size,
/// each priced by every offering that sells it.
#[tokio::test]
async fn a_deeper_commitment_prices_the_same_usage_lower() {
    let emulator = Emulator::new();
    let payment_options = ["All Upfront", "Partial Upfront", "No Upfront"];

    for (instance_type, region) in [("m5.large", "us-east-1"), ("m6i.2xlarge", "eu-west-1")] {
        let case = format!("{instance_type} in {region}");
        let rates = all_rates(
            &emulator,
            &case,
            &json!({
                "products": ["EC2"],
                "filters": [
                    { "name": "instanceType", "values": [instance_type] },
                    { "name": "region", "values": [region] },
                ],
                "maxResults": 1000,
            }),
        )
        .await;
        assert!(
            !rates.is_empty(),
            "case {case:?}: expected the offerings that price this usage, got none"
        );

        let mut priced: BTreeMap<(String, i64, String), i128> = BTreeMap::new();
        for rate in &rates {
            let parent_key = (
                text(parent(rate), "planType"),
                parent(rate)
                    .get("durationSeconds")
                    .and_then(Value::as_i64)
                    .unwrap_or_default(),
                text(parent(rate), "paymentOption"),
            );
            let units = rate_units(&case, rate);
            if let Some(earlier) = priced.insert(parent_key.clone(), units) {
                assert_eq!(
                    earlier, units,
                    "case {case:?} {parent_key:?}: one offering must price one usage dimension once"
                );
            }
        }

        // Every plan type that sells this usage, at every term and payment
        // option: a Compute plan and an EC2 Instance plan both cover EC2 usage.
        for plan_type in ["Compute", "EC2Instance"] {
            for payment_option in payment_options {
                let one_year = priced
                    .get(&(plan_type.to_string(), ONE_YEAR, payment_option.to_string()))
                    .copied();
                let three_years = priced
                    .get(&(plan_type.to_string(), THREE_YEARS, payment_option.to_string()))
                    .copied();
                let (Some(one_year), Some(three_years)) = (one_year, three_years) else {
                    panic!(
                        "case {case:?} {plan_type}/{payment_option}: expected both terms to price it, got {one_year:?} and {three_years:?}"
                    );
                };
                assert!(
                    three_years < one_year,
                    "case {case:?} {plan_type}/{payment_option}: a three-year term must price it below a one-year term, got {three_years} and {one_year}"
                );
            }
            for duration in [ONE_YEAR, THREE_YEARS] {
                let ladder: Vec<i128> = payment_options
                    .iter()
                    .map(|payment_option| {
                        *priced
                            .get(&(plan_type.to_string(), duration, (*payment_option).to_string()))
                            .unwrap_or_else(|| {
                                panic!("case {case:?} {plan_type}/{duration}/{payment_option}: expected a rate, got none")
                            })
                    })
                    .collect();
                assert!(
                    ladder[0] < ladder[1] && ladder[1] < ladder[2],
                    "case {case:?} {plan_type}/{duration}: paying more up front must price it lower, got {payment_options:?} at {ladder:?}"
                );
            }
        }
        for duration in [ONE_YEAR, THREE_YEARS] {
            for payment_option in payment_options {
                let compute = priced[&("Compute".to_string(), duration, payment_option.to_string())];
                let instance =
                    priced[&("EC2Instance".to_string(), duration, payment_option.to_string())];
                assert!(
                    instance < compute,
                    "case {case:?} {duration}/{payment_option}: a plan committed to one instance family must price it below a plan committed to nothing but spend, got {instance} and {compute}"
                );
            }
        }
    }
}

/// Within one offering, a rate scales with the size of the usage it prices: an
/// instance size with twice the vCPUs is priced at twice the rate.
#[tokio::test]
async fn a_rate_scales_with_the_size_of_the_usage_it_prices() {
    let emulator = Emulator::new();
    // Rows differ in plan type, term and payment option, so the proportionality
    // is not asserted under one discount.
    for (plan_type, payment_option, duration) in [
        ("Compute", "All Upfront", ONE_YEAR),
        ("Compute", "No Upfront", THREE_YEARS),
        ("EC2Instance", "Partial Upfront", THREE_YEARS),
    ] {
        let offering = emulator.offering(plan_type, payment_option, duration).await;
        let offering_id = text(&offering, "offeringId");
        let case = format!("{plan_type}/{payment_option}/{duration}");
        let rates = all_rates(
            &emulator,
            &case,
            &json!({
                "savingsPlanOfferingIds": [offering_id],
                "products": ["EC2"],
                "filters": [
                    { "name": "instanceFamily", "values": ["m5"] },
                    { "name": "region", "values": ["us-east-1"] },
                ],
                "maxResults": 1000,
            }),
        )
        .await;

        let mut by_type: BTreeMap<String, i128> = BTreeMap::new();
        for rate in &rates {
            by_type.insert(property(rate, "instanceType"), rate_units(&case, rate));
        }
        // The size ladder doubles the vCPUs at each step, so it doubles the
        // price: `large` is the unit of comparison, and the sizes above it are
        // its multiples.
        let large = *by_type.get("m5.large").unwrap_or_else(|| {
            panic!("case {case:?}: expected a rate for m5.large, got {:?}", by_type.keys())
        });
        for (instance_type, multiple) in [("m5.xlarge", 2), ("m5.2xlarge", 4)] {
            let actual = *by_type.get(instance_type).unwrap_or_else(|| {
                panic!("case {case:?}: expected a rate for {instance_type}, got {:?}", by_type.keys())
            });
            assert_eq!(
                actual,
                large * multiple,
                "case {case:?} {instance_type}: {multiple} times the vCPUs of m5.large must be priced at {multiple} times its rate ({large}), got {actual}"
            );
        }
    }
}

/// Every filter the request carries narrows the result, and a rate that is not
/// dimensioned by the property a filter names cannot satisfy it.
#[tokio::test]
async fn each_filter_narrows_the_search() {
    let emulator = Emulator::new();

    // Each row names a filter and what every admitted rate must then report:
    // one row per filter the input models, spanning the rate's own members, its
    // parent offering's and its dimensions.
    let narrowing: [NarrowingCase; 11] = [
        (
            "products Lambda",
            json!({ "products": ["Lambda"], "maxResults": 50 }),
            &[
                ("productType", "Lambda"),
                ("serviceCode", "AWSLambda"),
                ("unit", "Lambda-GB-Second"),
            ],
        ),
        (
            "serviceCodes AmazonEKS",
            json!({ "serviceCodes": ["AmazonEKS"], "maxResults": 50 }),
            &[("productType", "Fargate"), ("serviceCode", "AmazonEKS")],
        ),
        (
            "savingsPlanTypes Database",
            json!({ "savingsPlanTypes": ["Database"], "maxResults": 50 }),
            &[("parent.planType", "Database")],
        ),
        (
            "savingsPlanPaymentOptions All Upfront",
            json!({ "savingsPlanPaymentOptions": ["All Upfront"], "maxResults": 50 }),
            &[("parent.paymentOption", "All Upfront")],
        ),
        (
            "usageTypes one EC2 line item",
            json!({ "usageTypes": ["us-east-1-BoxUsage:c5.xlarge"], "maxResults": 1000 }),
            &[
                ("usageType", "us-east-1-BoxUsage:c5.xlarge"),
                ("productType", "EC2"),
                ("property.instanceType", "c5.xlarge"),
                ("property.region", "us-east-1"),
            ],
        ),
        (
            "operations FargateTask",
            json!({ "operations": ["FargateTask"], "maxResults": 50 }),
            &[("operation", "FargateTask"), ("productType", "Fargate")],
        ),
        (
            "filters region us-west-2",
            json!({
                "filters": [{ "name": "region", "values": ["us-west-2"] }],
                "maxResults": 50,
            }),
            &[("property.region", "us-west-2")],
        ),
        (
            "filters instanceFamily r6i",
            json!({
                "filters": [{ "name": "instanceFamily", "values": ["r6i"] }],
                "maxResults": 50,
            }),
            &[("property.instanceFamily", "r6i")],
        ),
        (
            "filters productDescription Linux/UNIX",
            json!({
                "filters": [{ "name": "productDescription", "values": ["Linux/UNIX"] }],
                "maxResults": 50,
            }),
            &[
                ("property.productDescription", "Linux/UNIX"),
                ("productType", "EC2"),
            ],
        ),
        (
            "filters tenancy shared",
            json!({
                "filters": [{ "name": "tenancy", "values": ["shared"] }],
                "maxResults": 50,
            }),
            &[("property.tenancy", "shared")],
        ),
        (
            "savingsPlanTypes and products SageMaker",
            json!({
                "savingsPlanTypes": ["SageMaker"],
                "products": ["SageMaker"],
                "maxResults": 50,
            }),
            &[
                ("parent.planType", "SageMaker"),
                ("productType", "SageMaker"),
                ("serviceCode", "AmazonSageMaker"),
                ("unit", "Hrs"),
            ],
        ),
    ];

    for (case, request, expected) in narrowing {
        let answer = emulator.call(OPERATION, request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 for {request}, got {} {:?}",
            answer.status, answer.body
        );
        let rows = answer.list("searchResults");
        assert!(
            !rows.is_empty(),
            "case {case:?}: the catalog prices this usage, so the search must answer with it, got {:?}",
            answer.body
        );
        for rate in rows {
            for (locator, value) in expected {
                assert_eq!(
                    field(rate, locator).as_str(),
                    *value,
                    "case {case:?}: every admitted rate must report {locator} {value:?}, got {:?}",
                    field(rate, locator)
                );
            }
        }
    }

    // And a filter nothing in the catalog satisfies admits nothing — whether it
    // names a value no rate carries, a dimension no rate is priced along, a
    // combination no one line item can satisfy, or an offering that does not
    // exist.
    let empty: [(&str, Value); 5] = [
        (
            "filters tenancy dedicated",
            json!({
                "filters": [{ "name": "tenancy", "values": ["dedicated"] }],
                "maxResults": 50,
            }),
        ),
        (
            "filters productId, a dimension no rate is priced along",
            json!({
                "filters": [{ "name": "productId", "values": ["ABCDEF1234567890"] }],
                "maxResults": 50,
            }),
        ),
        (
            "products Lambda billed by the EC2 service code",
            json!({ "products": ["Lambda"], "serviceCodes": ["AmazonEC2"], "maxResults": 50 }),
        ),
        (
            "usageTypes a line item nothing prices",
            json!({ "usageTypes": ["us-east-1-BoxUsage:zz9.mega"], "maxResults": 50 }),
        ),
        (
            "savingsPlanOfferingIds an offering the catalog does not sell",
            json!({
                "savingsPlanOfferingIds": ["00000000-0000-4000-8000-000000000000"],
                "maxResults": 50,
            }),
        ),
    ];
    for (case, request) in empty {
        let walked = pages(&emulator, case, &request).await;
        assert!(
            walked.iter().all(|(rows, _)| rows.is_empty()),
            "case {case:?}: expected no rate to match, got {:?}",
            walked.iter().map(|(rows, _)| rows.len()).collect::<Vec<usize>>()
        );
        assert_eq!(
            walked.len(),
            2,
            "case {case:?}: the first answer carries a cursor and resuming it ends the walk: expected two pages, got {}",
            walked.len()
        );
    }
}

/// The rate search answers with a resumable cursor: its first answer always
/// carries one, each page is a distinct window of the result, resuming loses and
/// repeats nothing, and the walk ends on the resumed page with no more results
/// to return.
#[tokio::test]
async fn the_rate_search_answers_with_a_resumable_cursor() {
    let emulator = Emulator::new();
    // One offering that prices a line item per database product, and one scoped
    // to a single instance family — so the result sets are small and the page
    // sizes below straddle them.
    let database = text(
        &emulator.offering("Database", "No Upfront", ONE_YEAR).await,
        "offeringId",
    );
    let instance = text(
        &emulator
            .offering("EC2Instance", "All Upfront", THREE_YEARS)
            .await,
        "offeringId",
    );

    // The rows differ along what the rule generalises over: the page size asked
    // for (one, a size that divides the set, a size that is exactly the set, a
    // size past it) and the size of the result set itself.
    let cases: [(&str, Value, i64); 8] = [
        ("a line item per database product, one at a time", json!({ "savingsPlanOfferingIds": [database.clone()] }), 1),
        ("a line item per database product, four at a time", json!({ "savingsPlanOfferingIds": [database.clone()] }), 4),
        ("a line item per database product, at the model's largest page", json!({ "savingsPlanOfferingIds": [database.clone()] }), 1000),
        ("one instance family, one at a time", json!({ "savingsPlanOfferingIds": [instance.clone()] }), 1),
        ("one instance family, two at a time", json!({ "savingsPlanOfferingIds": [instance.clone()] }), 2),
        ("one instance family, exactly its size", json!({ "savingsPlanOfferingIds": [instance.clone()] }), 3),
        ("one product of one offering", json!({ "savingsPlanOfferingIds": [database.clone()], "products": ["DynamoDB"] }), 1),
        ("a search nothing satisfies", json!({ "savingsPlanOfferingIds": [instance.clone()], "products": ["Lambda"] }), 5),
    ];

    for (case, filters, max_results) in cases {
        let mut request = filters.clone();
        request["maxResults"] = json!(max_results);
        let walked = pages(&emulator, case, &request).await;

        let expected: Vec<String> = {
            let mut unpaged = filters.clone();
            unpaged["maxResults"] = json!(1000);
            all_rates(&emulator, case, &unpaged)
                .await
                .iter()
                .map(key)
                .collect()
        };

        for (index, (rows, next)) in walked.iter().enumerate() {
            let is_last = index + 1 == walked.len();
            assert_eq!(
                next.is_none(),
                is_last,
                "case {case:?} page {index}: only the page the walk ends on carries no token, got {next:?}"
            );
            if let Some(token) = next {
                assert!(
                    is_pagination_token(token),
                    "case {case:?} page {index}: {token:?} must satisfy PaginationToken"
                );
            }
            assert!(
                i64::try_from(rows.len()).unwrap_or(i64::MAX) <= max_results,
                "case {case:?} page {index}: a page must not exceed maxResults {max_results}, got {}",
                rows.len()
            );
        }

        let keys: Vec<String> = walked
            .iter()
            .flat_map(|(rows, _)| rows.iter().map(key))
            .collect();
        assert_eq!(
            keys, expected,
            "case {case:?}: paging must yield the result set once, in order"
        );
        assert_eq!(
            keys.iter().collect::<BTreeSet<_>>().len(),
            keys.len(),
            "case {case:?}: paging must not repeat a rate"
        );
        // Successive non-empty pages are distinct windows of the result.
        let non_empty: Vec<&Vec<Value>> = walked
            .iter()
            .map(|(rows, _)| rows)
            .filter(|rows| !rows.is_empty())
            .collect();
        for window in non_empty.windows(2) {
            let before: Vec<String> = window[0].iter().map(key).collect();
            let after: Vec<String> = window[1].iter().map(key).collect();
            assert_ne!(
                before, after,
                "case {case:?}: two pages of one walk must be distinct, got {before:?} twice"
            );
        }
        let first = walked.first().expect("a walk answers at least one page");
        assert!(
            first.1.is_some(),
            "case {case:?}: the first answer of a search must carry the cursor to resume at, got {:?}",
            first.1
        );
        assert!(
            walked.len() >= 2,
            "case {case:?}: a walk that always hands back its first cursor takes at least two calls, got {}",
            walked.len()
        );
    }
}

/// The walk is by usage line item, and within one line item by the offerings
/// that price it: successive rows quote the same usage under DIFFERENT
/// offerings, so a page always advances past the row before it.
#[tokio::test]
async fn successive_rows_quote_the_same_usage_under_different_offerings() {
    let emulator = Emulator::new();

    // One EC2 line item: every offering that would cover that usage prices it —
    // a Compute plan and the EC2 Instance plan of its family, at each term and
    // payment option — and the walk lists them one after another.
    for (case, request) in [
        (
            "one EC2 line item",
            json!({ "usageTypes": ["us-east-1-BoxUsage:m5.large"] }),
        ),
        (
            "one database line item",
            json!({ "usageTypes": ["eu-west-1-ReadCapacityUnit-Hrs"] }),
        ),
    ] {
        for page_size in [1, 2, 5] {
            let case = format!("{case} at {page_size} a page");
            let mut paged = request.clone();
            paged["maxResults"] = json!(page_size);
            let walked = pages(&emulator, &case, &paged).await;
            let rows: Vec<Value> = walked
                .iter()
                .flat_map(|(rows, _)| rows.clone())
                .collect();
            assert!(
                rows.len() > 1,
                "case {case:?}: a usage line item is priced by every offering that covers it, got {} row(s)",
                rows.len()
            );

            let usage_type = text(&rows[0], "usageType");
            let mut offerings: Vec<String> = Vec::new();
            for rate in &rows {
                assert_eq!(
                    text(rate, "usageType"),
                    usage_type,
                    "case {case:?}: every row of one line item must quote the same usage"
                );
                offerings.push(text(parent(rate), "offeringId"));
            }
            assert_eq!(
                offerings.iter().collect::<BTreeSet<_>>().len(),
                offerings.len(),
                "case {case:?}: each offering prices one line item once, got {offerings:?}"
            );

            // Each page advances past the one before it: the first row of a page
            // is priced under a different offering than the first row of the
            // page before.
            let first_rows: Vec<&Value> = walked
                .iter()
                .filter_map(|(rows, _)| rows.first())
                .collect();
            for window in first_rows.windows(2) {
                let before = text(parent(window[0]), "offeringId");
                let after = text(parent(window[1]), "offeringId");
                assert_ne!(
                    before, after,
                    "case {case:?}: a page must advance past the row before it, got offering {before:?} twice"
                );
            }
        }
    }

    // And the same holds for the unnarrowed walk a client pages through one rate
    // at a time, which is where the rule is observable with no filter at all.
    let mut seen: Vec<String> = Vec::new();
    let mut next: Option<String> = None;
    for page in 0..4 {
        let mut request = json!({ "maxResults": 1 });
        if let Some(token) = &next {
            request["nextToken"] = json!(token);
        }
        let answer = emulator.call(OPERATION, request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case the whole catalog, one at a time, page {page}: expected 200 for {request}, got {} {:?}",
            answer.status, answer.body
        );
        let rows = answer.list("searchResults");
        assert_eq!(
            rows.len(),
            1,
            "case the whole catalog, one at a time, page {page}: expected one rate, got {}",
            rows.len()
        );
        seen.push(text(parent(&rows[0]), "offeringId"));
        next = answer
            .get("nextToken")
            .as_str()
            .map(std::string::ToString::to_string);
        assert!(
            next.is_some(),
            "case the whole catalog, one at a time, page {page}: the walk has more to return, so it must carry a cursor"
        );
    }
    for (page, pair) in seen.windows(2).enumerate() {
        assert_ne!(
            pair[0], pair[1],
            "case the whole catalog, one at a time, page {page}: the next page must quote a different offering, got {:?} twice",
            pair[0]
        );
    }
}

/// A search that names no page size gets the service's own page: a truncated,
/// resumable page rather than every rate the offering prices.
#[tokio::test]
async fn a_search_that_names_no_page_size_gets_the_services_own_page() {
    let emulator = Emulator::new();
    let compute = text(
        &emulator.offering("Compute", "All Upfront", ONE_YEAR).await,
        "offeringId",
    );

    // Both spellings of "no size asked for": the member left out, and the member
    // carrying the model's own default of 0.
    for (case, request) in [
        ("maxResults left out", json!({ "savingsPlanOfferingIds": [compute.clone()] })),
        (
            "maxResults 0, the model's default",
            json!({ "savingsPlanOfferingIds": [compute.clone()], "maxResults": 0 }),
        ),
    ] {
        let answer = emulator.call(OPERATION, request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 for {request}, got {} {:?}",
            answer.status, answer.body
        );
        let rows = answer.list("searchResults");
        assert_eq!(
            rows.len(),
            state::DEFAULT_OFFERINGS_PAGE_SIZE,
            "case {case:?}: expected the service's own page of {} rates, got {}",
            state::DEFAULT_OFFERINGS_PAGE_SIZE,
            rows.len()
        );
        let token = answer
            .get("nextToken")
            .as_str()
            .map(std::string::ToString::to_string)
            .unwrap_or_else(|| {
                panic!("case {case:?}: a truncated page must carry a cursor, got {:?}", answer.body)
            });
        assert!(
            is_pagination_token(&token),
            "case {case:?}: {token:?} must satisfy PaginationToken"
        );

        // And resuming it continues where the page stopped rather than starting
        // the walk again.
        let mut resumed_request = request.clone();
        resumed_request["nextToken"] = json!(token);
        let resumed = emulator.call(OPERATION, resumed_request.clone()).await;
        assert_eq!(
            resumed.status, 200,
            "case {case:?}: expected 200 for {resumed_request}, got {} {:?}",
            resumed.status, resumed.body
        );
        let first: BTreeSet<String> = rows.iter().map(key).collect();
        let second: BTreeSet<String> = resumed.list("searchResults").iter().map(key).collect();
        assert!(
            !second.is_empty(),
            "case {case:?}: the offering prices more than one page, so resuming must answer more rates, got {:?}",
            resumed.body
        );
        assert!(
            first.is_disjoint(&second),
            "case {case:?}: a resumed page must not repeat the page before it"
        );
    }
}

/// A token this search did not issue is refused with the modelled 400, and so is
/// a filter attribute the model does not declare.
#[tokio::test]
async fn a_request_this_search_cannot_serve_is_refused() {
    let emulator = Emulator::new();
    let offerings_token = emulator
        .call(
            "DescribeSavingsPlansOfferings",
            json!({ "maxResults": 1 }),
        )
        .await
        .get("nextToken")
        .as_str()
        .map(std::string::ToString::to_string);
    let mut tokens: Vec<String> = vec![
        "zzzz".to_string(),
        "AAAA".to_string(),
        "0102030405".to_string(),
        // Hex, and decodable, but minted for another paginator.
        hex::encode("offerings:1"),
    ];
    tokens.extend(offerings_token);
    for case in tokens {
        let answer = emulator
            .call(
                OPERATION,
                json!({ "maxResults": 1, "nextToken": case.clone() }),
            )
            .await;
        assert_eq!(
            (answer.status, answer.code.as_str()),
            (400, "ValidationException"),
            "case nextToken {case:?}: expected a 400 ValidationException, got {} {:?}",
            answer.status,
            answer.body
        );
    }

    // One step outside the published value set of `SavingsPlanRateFilterAttribute`.
    for case in ["Region", "instancefamily", "sku"] {
        let answer = emulator
            .call(
                OPERATION,
                json!({
                    "filters": [{ "name": case, "values": ["us-east-1"] }],
                    "maxResults": 1,
                }),
            )
            .await;
        assert_eq!(
            (answer.status, answer.code.as_str()),
            (400, "ValidationException"),
            "case filter name {case:?}: a value outside the model's enum must be refused, got {} {:?}",
            answer.status,
            answer.body
        );
    }
}
