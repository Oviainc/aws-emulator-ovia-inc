// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the configuration-and-pricing options a quote is answered
//! with, and for the requirements an order submitted from it has to satisfy, driven
//! over the wire through the same front door as the rest of this crate's suites
//! (whose harness it reuses).
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. "A quote provides pricing and configuration options based on the requested
//!    capacity": `CreateQuote` answers the quote with the option that capacity is
//!    offered as — one `QuoteOption` carrying its own minted `QuoteOptionIdentifier`
//!    (`^oqo-[a-f0-9]{17}$`), the capacity it provides, and one `SUBSCRIPTION`
//!    pricing option per combination of the payment options and payment terms the
//!    request asked for ("if not specified, all available payment options are
//!    returned", so a request that asked for none is priced under every modelled
//!    value). The option is persisted WITH the quote, so every later read names the
//!    same identifier — the one `CreateOrder` takes.
//! 2. A quote option's price follows the EC2 capacity it provides, by the one rule
//!    this service prices a subscription by: the vCPUs the catalog publishes for the
//!    instance type each capacity names, times the instances asked for, at the
//!    published monthly rate, split into the payment option's upfront share and the
//!    rest in equal monthly instalments over the term's months. Capacity this
//!    service publishes no figure for is not priced at all, rather than priced at a
//!    figure the emulator invented.
//! 3. A quote carries the outcome of every ordering check this store holds the
//!    records to decide — that it names an Outpost, that another order can still be
//!    placed, that the Outpost has settled in its active status, that its site
//!    records each of the two addresses an order is placed against, and that the
//!    quote is for the country that site is in — each `PASS` or `FAIL` as those
//!    records stand, with the checks that need an Outpost reported only for a quote
//!    that names one.
//! 4. `UpdateQuote` re-derives what the members it replaced are derived from: an
//!    update to the configuration re-prices the quote (a fresh option, with its own
//!    identifier), one to the country code or the Outpost association re-runs the
//!    checks, and one that touches neither leaves both exactly as they stand.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::ops::types::{
    PAYMENT_OPTION_VALUES, PAYMENT_TERM_VALUES,
};
use service_com_amazonaws_outposts_outpostsolafservice::state::{
    CURRENCY_USD, QUOTE_PRICING_TYPE_SUBSCRIPTION, SUBSCRIPTION_PRICE_PER_VCPU_MONTH,
};

use crate::quote_lifecycle::{create_quote, get_quote, request};
use crate::renewal_pricing::{assert_price, months, price, upfront_share, CENT};
use crate::site_and_outpost_lifecycle::{
    address, install_hardware, is_minted_id, mark_outpost_active, segment, text, Outposts,
};

/// The two `OrderingRequirementStatus` values a check this service RAN reports.
///
/// Source: `smithy_query(query="shape", shape="OrderingRequirementStatus")` declares
/// `PASS`, `FAIL` and `EXEMPT`; an exemption is granted outside this API, so a check
/// the service ran reports one of the first two.
const RAN_STATUSES: &[&str] = &["PASS", "FAIL"];

/// The `OrderingRequirementType` of each check this suite asserts, as the model
/// spells it.
///
/// Source: `smithy_query(query="shape", shape="OrderingRequirementType")`, whose
/// documentation states what each one checks.
pub(crate) const OUTPOST_ID_MISSING: &str = "OUTPOST_ID_MISSING_ON_QUOTE_ERROR";
/// See [`OUTPOST_ID_MISSING`].
const MAXIMUM_ALLOWED_ORDERS: &str = "MAXIMUM_ALLOWED_ORDERS_CHECK_ERROR";
/// See [`OUTPOST_ID_MISSING`].
pub(crate) const OUTPOST_ACTIVE: &str = "OUTPOST_ACTIVE_CHECK_ERROR";
/// See [`OUTPOST_ID_MISSING`].
pub(crate) const OPERATING_ADDRESS: &str = "OPERATING_ADDRESS_EXISTENCE_CHECK_ERROR";
/// See [`OUTPOST_ID_MISSING`].
pub(crate) const SHIPPING_ADDRESS: &str = "SHIPPING_ADDRESS_EXISTENCE_CHECK_ERROR";
/// See [`OUTPOST_ID_MISSING`].
pub(crate) const COUNTRY_CODE_MISMATCH: &str = "COUNTRY_CODE_MISMATCH_CHECK_ERROR";

/// A `QuoteCapacity$Unit` the orderable-instance-type catalog publishes no instance
/// type for, so no vCPU figure of it exists to price. The rows that use it assert it
/// really is unpublished.
const UNPUBLISHED_UNIT: &str = "instances";

/// The sole `QuoteOption` a quote reports.
///
/// The request describes ONE configuration — the capacity it asks for — so the quote
/// is answered with one option for it, which this reads out while asserting that is
/// what the quote carries.
pub(crate) fn sole_option(quote: &Value, case: &str) -> Value {
    let options = quote
        .get("QuoteOptions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected QuoteOptions, got {quote}"));
    assert_eq!(
        options.len(),
        1,
        "case {case:?}: the requested capacity is one configuration, so the quote reports one \
         option for it, got {quote}"
    );
    options[0].clone()
}

/// The `QuoteOptionIdentifier` of that option, asserted to be of the modelled
/// `^oqo-[a-f0-9]{17}$` form — the value `CreateOrder` takes.
pub(crate) fn option_identifier(quote: &Value, case: &str) -> String {
    let option = sole_option(quote, case);
    let identifier = text(&option, "QuoteOptionIdentifier");
    assert!(
        is_minted_id(&identifier, "oqo"),
        "case {case:?}: QuoteOptionIdentifier {identifier:?} is not the modelled \
         ^oqo-[a-f0-9]{{17}}$ form"
    );
    identifier
}

/// The outcome the quote reports for one check, or `None` when it reports that check
/// at all.
pub(crate) fn requirement_status(quote: &Value, check: &str) -> Option<String> {
    quote
        .get("OrderingRequirements")
        .and_then(Value::as_array)?
        .iter()
        .find(|requirement| text(requirement, "OrderingRequirementType") == check)
        .map(|requirement| text(requirement, "Status"))
}

/// Every check a quote reports, as `check -> outcome`.
///
/// Each one has to name a modelled check once, carry an outcome a check that RAN can
/// have, and state what it found — a requirement with no message says nothing a
/// caller could act on.
fn requirement_statuses(quote: &Value, case: &str) -> BTreeMap<String, String> {
    let reported = quote
        .get("OrderingRequirements")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected OrderingRequirements, got {quote}"));
    let mut statuses: BTreeMap<String, String> = BTreeMap::new();
    for requirement in reported {
        let check = text(requirement, "OrderingRequirementType");
        let status = text(requirement, "Status");
        assert!(
            RAN_STATUSES.contains(&status.as_str()),
            "case {case:?}: {check} reports {status:?}, which is no outcome of a check that ran, \
             got {requirement}"
        );
        assert!(
            !text(requirement, "StatusMessage").is_empty(),
            "case {case:?}: {check} must state what it found, got {requirement}"
        );
        assert!(
            statuses.insert(check.clone(), status).is_none(),
            "case {case:?}: {check} is reported twice, got {quote}"
        );
    }
    statuses
}

/// The `check -> outcome` map a row expects, as a map to compare against.
fn expected_statuses(expected: &[(&str, &str)]) -> BTreeMap<String, String> {
    expected
        .iter()
        .map(|(check, status)| ((*check).to_string(), (*status).to_string()))
        .collect()
}

/// The `(PaymentOption, PaymentTerm)` combinations one option is priced under, in the
/// order it reports them.
fn priced_combinations(option: &Value, case: &str) -> Vec<(String, String)> {
    option
        .get("PricingOptions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected PricingOptions, got {option}"))
        .iter()
        .map(|pricing| {
            let details = pricing
                .get("SubscriptionPricingDetails")
                .unwrap_or_else(|| panic!("case {case:?}: expected the details, got {pricing}"));
            (
                text(details, "PaymentOption"),
                text(details, "PaymentTerm"),
            )
        })
        .collect()
}

/// Every combination of `payment_options` and `payment_terms`, in the order the two
/// lists name them — what a quote requesting them is priced under.
fn combinations(payment_options: &[&str], payment_terms: &[&str]) -> Vec<(String, String)> {
    payment_options
        .iter()
        .flat_map(|payment_option| {
            payment_terms
                .iter()
                .map(move |payment_term| ((*payment_option).to_string(), (*payment_term).to_string()))
        })
        .collect()
}

/// Every combination the model declares, which a quote that requested no payment
/// option and no term is priced under.
fn every_combination() -> Vec<(String, String)> {
    combinations(PAYMENT_OPTION_VALUES, PAYMENT_TERM_VALUES)
}

/// How many vCPUs the orderable-instance-type catalog publishes for one instance
/// type, read through the store seam [`crate::quote_lifecycle`] ages a quote through
/// — the catalog's own published figure, never one this suite picked.
pub(crate) fn published_vcpus(outposts: &Outposts, instance_type: &str) -> f64 {
    let published = outposts.store().lock().published_vcpus(instance_type);
    f64::from(
        published
            .unwrap_or_else(|| panic!("the catalog publishes a vCPU figure for {instance_type}")),
    )
}

/// Rule 2 over one option: it is priced under exactly `expected` combinations, each a
/// `SUBSCRIPTION` of `vcpus` vCPUs priced by the service's own rule — or under none at
/// all when there is no capacity here this service publishes a figure for.
fn assert_priced(option: &Value, vcpus: f64, expected: &[(String, String)], case: &str) {
    if vcpus <= 0.0 {
        assert_eq!(
            option.get("PricingOptions"),
            Some(&json!([])),
            "case {case:?}: there is no capacity here this service publishes a price for, so the \
             option carries no price, got {option}"
        );
        return;
    }
    assert_eq!(
        priced_combinations(option, case),
        *expected,
        "case {case:?}: expected one priced option per requested payment combination, got {option}"
    );
    let monthly_rate = vcpus * f64::from(SUBSCRIPTION_PRICE_PER_VCPU_MONTH);
    let reported = option
        .get("PricingOptions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected PricingOptions, got {option}"));
    for (pricing, (payment_option, payment_term)) in reported.iter().zip(expected) {
        let case = &format!("{case} ({payment_option} {payment_term})");
        assert_eq!(
            pricing.get("PricingType").and_then(Value::as_str),
            Some(QUOTE_PRICING_TYPE_SUBSCRIPTION),
            "case {case:?}: a quote prices a subscription on hardware, got {pricing}"
        );
        let details = pricing
            .get("SubscriptionPricingDetails")
            .unwrap_or_else(|| panic!("case {case:?}: expected the details, got {pricing}"));
        assert_eq!(
            details.get("Currency").and_then(Value::as_str),
            Some(CURRENCY_USD),
            "case {case:?}: expected Currency {CURRENCY_USD:?}, got {details}"
        );
        let months = months(payment_term);
        let share = upfront_share(payment_option);
        let term_cost = monthly_rate * months;
        assert_price(
            price(details, "UpfrontPrice", case),
            term_cost * share,
            "UpfrontPrice",
            case,
        );
        assert_price(
            price(details, "MonthlyRecurringPrice", case),
            monthly_rate * (1.0 - share),
            "MonthlyRecurringPrice",
            case,
        );
        // The invariant behind the split, whatever the rate is: what the subscriber
        // pays over the whole term is the term's cost, under every payment option.
        let paid = price(details, "UpfrontPrice", case).unwrap_or_default()
            + price(details, "MonthlyRecurringPrice", case).unwrap_or_default() * months;
        assert!(
            (paid - term_cost).abs() <= CENT * months,
            "case {case:?}: upfront plus {months} monthly instalments must be the term's cost \
             {term_cost}, got {paid}"
        );
    }
}

/// One row of a table below: the case, and the values it asserts.
///
/// Written as a type rather than spelled at each `let`, which is what
/// `clippy::type_complexity` asks for: the payment rows carry the members the
/// request names and the combinations the quote must then be priced under.
type PaymentCase = (&'static str, Value, Vec<(String, String)>);

/// The ordering-check rows: the country the quote is for, the Outpost it names, and
/// the outcome of every check it must report. See [`PaymentCase`].
type CheckCase<'a> = (
    &'a str,
    &'a str,
    Option<&'a str>,
    Vec<(&'static str, &'static str)>,
);

/// The update rows: the patch, whether it re-prices the quote, the capacity the
/// option must then be priced on, the combinations it must be priced under, and the
/// outcome the association check must then report. See [`PaymentCase`].
type RepriceCase<'a> = (&'a str, Value, bool, f64, Vec<(String, String)>, &'a str);

/// One requested capacity of `quantity` instances of `instance_type`.
pub(crate) fn ec2(instance_type: &str, quantity: f64) -> Value {
    json!({ "QuoteCapacityType": "EC2", "Unit": instance_type, "Quantity": quantity })
}

/// Rule 1: a created quote carries the option its requested capacity is offered as,
/// priced under exactly the payment combinations the request asked for, and reads
/// back under the same identifier.
///
/// The rows differ along everything the requested payment members generalise over:
/// neither named, each named empty (the modelled `length(min: 0)`), one of each,
/// every value of one and one of the other, two of each, and the same value named
/// twice — a value requested twice is one request for it.
#[tokio::test]
async fn a_quote_is_priced_under_exactly_the_payment_combinations_it_requests() {
    let outposts = Outposts::new();
    let capacity = ec2("c5.24xlarge", 1.0);
    let vcpus = published_vcpus(&outposts, "c5.24xlarge");

    let cases: Vec<PaymentCase> = vec![
        ("neither member named", json!({}), every_combination()),
        (
            "both named as the empty list the modelled length(min: 0) allows",
            json!({ "RequestedPaymentOptions": [], "RequestedPaymentTerms": [] }),
            every_combination(),
        ),
        (
            "one payment option, no term named",
            json!({ "RequestedPaymentOptions": ["NO_UPFRONT"] }),
            combinations(&["NO_UPFRONT"], PAYMENT_TERM_VALUES),
        ),
        (
            "one term, no payment option named",
            json!({ "RequestedPaymentTerms": ["FIVE_YEARS"] }),
            combinations(PAYMENT_OPTION_VALUES, &["FIVE_YEARS"]),
        ),
        (
            "one of each",
            json!({
                "RequestedPaymentOptions": ["ALL_UPFRONT"],
                "RequestedPaymentTerms": ["THREE_YEARS"],
            }),
            combinations(&["ALL_UPFRONT"], &["THREE_YEARS"]),
        ),
        (
            "every payment option and every term",
            json!({
                "RequestedPaymentOptions": PAYMENT_OPTION_VALUES,
                "RequestedPaymentTerms": PAYMENT_TERM_VALUES,
            }),
            combinations(PAYMENT_OPTION_VALUES, PAYMENT_TERM_VALUES),
        ),
        (
            "two of each, in the request's own order",
            json!({
                "RequestedPaymentOptions": ["PARTIAL_UPFRONT", "ALL_UPFRONT"],
                "RequestedPaymentTerms": ["FIVE_YEARS", "ONE_YEAR"],
            }),
            combinations(
                &["PARTIAL_UPFRONT", "ALL_UPFRONT"],
                &["FIVE_YEARS", "ONE_YEAR"],
            ),
        ),
        (
            "the same payment option requested twice",
            json!({
                "RequestedPaymentOptions": ["NO_UPFRONT", "NO_UPFRONT"],
                "RequestedPaymentTerms": ["ONE_YEAR"],
            }),
            combinations(&["NO_UPFRONT"], &["ONE_YEAR"]),
        ),
    ];

    let mut identifiers: BTreeSet<String> = BTreeSet::new();
    for (case, overrides, expected) in &cases {
        let body = request("US", json!([capacity.clone()]), overrides.clone());
        let created = create_quote(&outposts, &body).await;
        let option = sole_option(&created, case);
        identifiers.insert(option_identifier(&created, case));

        assert_eq!(
            option.get("Capacities"),
            Some(&json!([capacity.clone()])),
            "case {case:?}: the option provides the capacity the quote asks for, got {option}"
        );
        // The capacity that EXISTS and the rack or server a configuration is
        // deployed on are records this store does not hold for a quote, so neither
        // is reported rather than being read back off the request.
        for member in ["CapacitySummary", "Specifications"] {
            assert!(
                option.get(member).is_none(),
                "case {case:?}: {member} is not something this emulator holds a record of, so it \
                 must be absent, got {option}"
            );
        }
        assert_priced(&option, vcpus, expected, case);

        // The option is persisted with the quote: a later read names the SAME
        // option, which is what makes it an identifier an order can be submitted
        // against.
        let read = get_quote(&outposts, &text(&created, "QuoteId")).await;
        assert_eq!(
            read.get("QuoteOptions"),
            created.get("QuoteOptions"),
            "case {case:?}: GetQuote must answer the option the create reported"
        );
    }

    assert_eq!(
        identifiers.len(),
        cases.len(),
        "every quote is offered its own option, so no two share a QuoteOptionIdentifier, got \
         {identifiers:?}"
    );
}

/// Rule 2: the price of the option follows the EC2 capacity it provides, and capacity
/// this service publishes no figure for is not priced at all.
///
/// The rows differ along everything the capacity generalises over: how many
/// capacities (none, one, several), the instance type each names (several the catalog
/// publishes, from the smallest to the largest), how many instances of it (one,
/// several, a fraction, none at all), the capacity TYPE (every one the model
/// declares, so storage as well as instances), and a unit the catalog publishes no
/// instance type for.
#[tokio::test]
async fn a_quote_option_is_priced_from_the_capacity_it_provides() {
    let outposts = Outposts::new();
    assert!(
        outposts
            .store()
            .lock()
            .published_vcpus(UNPUBLISHED_UNIT)
            .is_none(),
        "the rows below need {UNPUBLISHED_UNIT:?} to be a unit the catalog publishes no instance \
         type for"
    );
    let small = published_vcpus(&outposts, "m5.xlarge");
    let middling = published_vcpus(&outposts, "r5.2xlarge");
    let large = published_vcpus(&outposts, "c5.24xlarge");

    let cases: Vec<(&str, Value, f64)> = vec![
        (
            "no capacity at all, which the modelled length(min: 0) allows",
            json!([]),
            0.0,
        ),
        ("one instance of the smallest type", json!([ec2("m5.xlarge", 1.0)]), small),
        (
            "eight instances of the same type",
            json!([ec2("m5.xlarge", 8.0)]),
            small * 8.0,
        ),
        (
            "one instance of the largest type",
            json!([ec2("c5.24xlarge", 1.0)]),
            large,
        ),
        (
            "a fractional quantity",
            json!([ec2("c5.24xlarge", 0.5)]),
            large * 0.5,
        ),
        (
            "three capacities of three instance types",
            json!([
                ec2("m5.xlarge", 2.0),
                ec2("r5.2xlarge", 3.0),
                ec2("c5.24xlarge", 1.0),
            ]),
            small * 2.0 + middling * 3.0 + large,
        ),
        (
            "no instances asked for",
            json!([ec2("c5.24xlarge", 0.0)]),
            0.0,
        ),
        (
            "a unit the catalog publishes no instance type for",
            json!([ec2(UNPUBLISHED_UNIT, 4.0)]),
            0.0,
        ),
        (
            "storage capacity only, which carries no instance vCPUs",
            json!([
                { "QuoteCapacityType": "EBS", "Unit": "TiB", "Quantity": 11.0 },
                { "QuoteCapacityType": "S3", "Unit": "TiB", "Quantity": 48.0 },
            ]),
            0.0,
        ),
        (
            "every modelled capacity type at once, priced on the instances among them",
            json!([
                ec2("r5.2xlarge", 4.0),
                { "QuoteCapacityType": "EBS", "Unit": "TiB", "Quantity": 2.0 },
                { "QuoteCapacityType": "S3", "Unit": "TiB", "Quantity": 96.0 },
            ]),
            middling * 4.0,
        ),
    ];

    for (case, capacities, vcpus) in &cases {
        let created = create_quote(&outposts, &request("US", capacities.clone(), json!({}))).await;
        let option = sole_option(&created, case);
        assert_eq!(
            option.get("Capacities"),
            Some(capacities),
            "case {case:?}: the option provides the capacity the quote asks for, got {option}"
        );
        assert_priced(&option, *vcpus, &every_combination(), case);
    }

    // Twice the capacity costs twice as much: the price follows the capacity rather
    // than being one figure the service remembers.
    let single = create_quote(
        &outposts,
        &request("US", json!([ec2("c5.24xlarge", 1.0)]), json!({})),
    )
    .await;
    let double = create_quote(
        &outposts,
        &request("US", json!([ec2("c5.24xlarge", 2.0)]), json!({})),
    )
    .await;
    for member in ["UpfrontPrice", "MonthlyRecurringPrice"] {
        let case = &format!("twice the capacity, {member}");
        for (one, two) in priced_details(&single, case)
            .iter()
            .zip(priced_details(&double, case).iter())
        {
            let one = price(one, member, case).unwrap_or_default();
            let two = price(two, member, case).unwrap_or_default();
            assert!(
                (two - one * 2.0).abs() <= CENT,
                "case {case:?}: twice the capacity must cost twice as much, got {one} and {two}"
            );
        }
    }
}

/// The `SubscriptionPricingDetails` of every option a quote is priced under, in the
/// order it reports them.
fn priced_details(quote: &Value, case: &str) -> Vec<Value> {
    sole_option(quote, case)
        .get("PricingOptions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected PricingOptions, got {quote}"))
        .iter()
        .map(|pricing| {
            pricing
                .get("SubscriptionPricingDetails")
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: expected the details, got {pricing}"))
        })
        .collect()
}

/// Rule 3: a quote reports the outcome of every ordering check this store can decide,
/// and only those.
///
/// The rows differ along everything those checks read: whether the quote names an
/// Outpost at all, whether that Outpost has had its ordered hardware installed, which
/// of its site's two addresses are recorded, and whether the country the quote is for
/// is the country of that site.
#[tokio::test]
async fn a_quote_reports_the_outcome_of_every_check_this_store_can_decide() {
    let outposts = Outposts::new();

    // One Outpost per shape of site the rows need. Each is put in its terminal ACTIVE
    // lifecycle status, which is what the active check reads, so every row but the last
    // reads an active Outpost. It is recorded rather than ordered, because a site whose
    // details are not complete takes no order at all
    // ([`crate::precondition_gates`]) and two of these sites are exactly that.
    let complete = an_outpost(
        &outposts,
        "quote-checks-complete",
        json!({
            "OperatingAddress": address("US", "WA", "Seattle"),
            "ShippingAddress": address("US", "WA", "Kirkland"),
        }),
    )
    .await;
    let operating_only = an_outpost(
        &outposts,
        "quote-checks-operating",
        json!({ "OperatingAddress": address("DE", "BE", "Berlin") }),
    )
    .await;
    let addressless = an_outpost(&outposts, "quote-checks-addressless", json!({})).await;
    for outpost_id in [&complete, &operating_only, &addressless] {
        mark_outpost_active(&outposts, outpost_id);
    }

    // Each row: the country code the quote is for, the Outpost it names, and the
    // outcome of every check it must report.
    let cases: Vec<CheckCase<'_>> = vec![
        (
            "a standalone quote, associated with no Outpost",
            "US",
            None,
            vec![
                (OUTPOST_ID_MISSING, "FAIL"),
                (MAXIMUM_ALLOWED_ORDERS, "PASS"),
            ],
        ),
        (
            "an active Outpost whose site records both addresses, in the quote's country",
            "US",
            Some(complete.as_str()),
            vec![
                (OUTPOST_ID_MISSING, "PASS"),
                (MAXIMUM_ALLOWED_ORDERS, "PASS"),
                (OUTPOST_ACTIVE, "PASS"),
                (OPERATING_ADDRESS, "PASS"),
                (SHIPPING_ADDRESS, "PASS"),
                (COUNTRY_CODE_MISMATCH, "PASS"),
            ],
        ),
        (
            "the same Outpost, for a quote in another country",
            "GB",
            Some(complete.as_str()),
            vec![
                (OUTPOST_ID_MISSING, "PASS"),
                (MAXIMUM_ALLOWED_ORDERS, "PASS"),
                (OUTPOST_ACTIVE, "PASS"),
                (OPERATING_ADDRESS, "PASS"),
                (SHIPPING_ADDRESS, "PASS"),
                (COUNTRY_CODE_MISMATCH, "FAIL"),
            ],
        ),
        (
            "a site with an operating address and no shipping address",
            "DE",
            Some(operating_only.as_str()),
            vec![
                (OUTPOST_ID_MISSING, "PASS"),
                (MAXIMUM_ALLOWED_ORDERS, "PASS"),
                (OUTPOST_ACTIVE, "PASS"),
                (OPERATING_ADDRESS, "PASS"),
                (SHIPPING_ADDRESS, "FAIL"),
                (COUNTRY_CODE_MISMATCH, "PASS"),
            ],
        ),
        (
            "a site with no address at all, so no country to compare",
            "US",
            Some(addressless.as_str()),
            vec![
                (OUTPOST_ID_MISSING, "PASS"),
                (MAXIMUM_ALLOWED_ORDERS, "PASS"),
                (OUTPOST_ACTIVE, "PASS"),
                (OPERATING_ADDRESS, "FAIL"),
                (SHIPPING_ADDRESS, "FAIL"),
            ],
        ),
    ];

    for (case, country_code, outpost_id, expected) in &cases {
        let overrides = match outpost_id {
            Some(outpost_id) => json!({ "OutpostIdentifier": outpost_id }),
            None => json!({}),
        };
        let created = create_quote(
            &outposts,
            &request(country_code, json!([ec2("m5.xlarge", 1.0)]), overrides),
        )
        .await;
        assert_eq!(
            requirement_statuses(&created, case),
            expected_statuses(expected),
            "case {case:?}: expected exactly these checks and outcomes, got {created}"
        );
        // The checks are persisted with the quote, so a read answers what the create
        // reported rather than re-deciding them.
        let read = get_quote(&outposts, &text(&created, "QuoteId")).await;
        assert_eq!(
            read.get("OrderingRequirements"),
            created.get("OrderingRequirements"),
            "case {case:?}: GetQuote must answer the checks the create reported"
        );
    }

    // An Outpost nothing has been ordered for fails the check that it is active: the
    // row quotes against an Outpost that holds no hardware, which no other row does.
    let provisioning = an_outpost(&outposts, "quote-checks-provisioning", json!({})).await;
    let created = create_quote(
        &outposts,
        &request(
            "US",
            json!([]),
            json!({ "OutpostIdentifier": provisioning }),
        ),
    )
    .await;
    assert_eq!(
        requirement_status(&created, OUTPOST_ACTIVE).as_deref(),
        Some("FAIL"),
        "an Outpost whose hardware has not been installed is not active yet, got {created}"
    );
}

/// One site carrying `site` (its addresses, if a row wants any) and one Outpost on
/// it, as that Outpost's `OutpostId`.
async fn an_outpost(outposts: &Outposts, name: &str, site: Value) -> String {
    let mut body = json!({ "Name": format!("{name}-site") });
    if let (Some(target), Some(extra)) = (body.as_object_mut(), site.as_object()) {
        for (member, value) in extra {
            target.insert(member.clone(), value.clone());
        }
    }
    let site = outposts.create_site(&body).await;
    let outpost = outposts
        .create_outpost(&json!({ "Name": name, "SiteId": text(&site, "SiteId") }))
        .await;
    text(&outpost, "OutpostId")
}

/// Rule 4: an update re-derives exactly what the members it replaced are derived
/// from.
///
/// The rows are applied in sequence to ONE quote, so a row that re-priced a quote it
/// should have left alone — or left one standing that it should have re-priced —
/// shows up immediately. They span each member the options are priced on, each member
/// the checks are read from, a member neither follows from, and both at once.
#[tokio::test]
async fn an_update_reprices_the_quote_and_re_runs_its_checks() {
    let outposts = Outposts::new();
    let outpost_id = an_outpost(
        &outposts,
        "quote-reprice",
        json!({
            "OperatingAddress": address("US", "WA", "Seattle"),
            "ShippingAddress": address("US", "WA", "Kirkland"),
        }),
    )
    .await;
    install_hardware(&outposts, &outpost_id).await;
    let vcpus = published_vcpus(&outposts, "c5.24xlarge");
    let created = create_quote(
        &outposts,
        &request(
            "US",
            json!([ec2("c5.24xlarge", 1.0)]),
            json!({
                "OutpostIdentifier": outpost_id.clone(),
                "RequestedPaymentOptions": ["ALL_UPFRONT"],
                "RequestedPaymentTerms": ["ONE_YEAR"],
                "Description": "created",
            }),
        ),
    )
    .await;
    let quote_id = text(&created, "QuoteId");

    // Each row: the patch, whether it re-prices the quote, the capacity the option
    // must then be priced on, the combinations it must be priced under, and the
    // outcome the association check must then report.
    let cases: Vec<RepriceCase<'_>> = vec![
        (
            "a description, which neither the options nor the checks follow from",
            json!({ "Description": "re-described" }),
            false,
            vcpus,
            combinations(&["ALL_UPFRONT"], &["ONE_YEAR"]),
            "PASS",
        ),
        (
            "twice the capacity",
            json!({ "RequestedCapacities": [ec2("c5.24xlarge", 2.0)] }),
            true,
            vcpus * 2.0,
            combinations(&["ALL_UPFRONT"], &["ONE_YEAR"]),
            "PASS",
        ),
        (
            "every payment option",
            json!({ "RequestedPaymentOptions": PAYMENT_OPTION_VALUES }),
            true,
            vcpus * 2.0,
            combinations(PAYMENT_OPTION_VALUES, &["ONE_YEAR"]),
            "PASS",
        ),
        (
            "the payment terms emptied, so every modelled term is quoted again",
            json!({ "RequestedPaymentTerms": [] }),
            true,
            vcpus * 2.0,
            combinations(PAYMENT_OPTION_VALUES, PAYMENT_TERM_VALUES),
            "PASS",
        ),
        (
            "a country code the site does not match",
            json!({ "CountryCode": "JP" }),
            false,
            vcpus * 2.0,
            combinations(PAYMENT_OPTION_VALUES, PAYMENT_TERM_VALUES),
            "PASS",
        ),
        (
            "the association removed",
            json!({ "OutpostIdentifier": "" }),
            false,
            vcpus * 2.0,
            combinations(PAYMENT_OPTION_VALUES, PAYMENT_TERM_VALUES),
            "FAIL",
        ),
        (
            "the capacity emptied and the Outpost named again, at once",
            json!({ "RequestedCapacities": [], "OutpostIdentifier": outpost_id.clone() }),
            true,
            0.0,
            combinations(PAYMENT_OPTION_VALUES, PAYMENT_TERM_VALUES),
            "PASS",
        ),
    ];

    let mut previous = option_identifier(&created, "the created quote");
    assert_eq!(
        requirement_status(&created, COUNTRY_CODE_MISMATCH).as_deref(),
        Some("PASS"),
        "the created quote is for the country of its Outpost's site, got {created}"
    );
    for (case, patch, repriced, capacity, expected, association) in &cases {
        let answered = outposts
            .ok(
                "PATCH",
                &format!("/quotes/{}", segment(&quote_id)),
                Some(patch),
            )
            .await["Quote"]
            .clone();
        let identifier = option_identifier(&answered, case);
        match repriced {
            true => assert_ne!(
                identifier, previous,
                "case {case:?}: the quote was re-priced, so the option it is offered as is a new \
                 one, got {answered}"
            ),
            false => assert_eq!(
                identifier, previous,
                "case {case:?}: nothing the options are priced on changed, so the quote keeps the \
                 option it had, got {answered}"
            ),
        }
        previous = identifier;
        assert_priced(&sole_option(&answered, case), *capacity, expected, case);
        assert_eq!(
            requirement_status(&answered, OUTPOST_ID_MISSING).as_deref(),
            Some(*association),
            "case {case:?}: the association check reports whether the quote names an Outpost, got \
             {answered}"
        );
        assert_eq!(
            get_quote(&outposts, &quote_id).await,
            answered,
            "case {case:?}: GetQuote must answer the quote the update reported"
        );
    }

    // The country code the last rows left the quote with is not the country of its
    // site, so the check that was re-run says so.
    let read = get_quote(&outposts, &quote_id).await;
    assert_eq!(
        requirement_status(&read, COUNTRY_CODE_MISMATCH).as_deref(),
        Some("FAIL"),
        "the quote is now for another country than its Outpost's site, got {read}"
    );
}
