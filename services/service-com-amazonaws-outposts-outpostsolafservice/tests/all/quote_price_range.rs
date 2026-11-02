// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the RANGE a quoted price is answered within, driven over the
//! wire through the same front door as the rest of this crate's suites (whose
//! harness it reuses).
//!
//! The rule under test, stated once and then exercised over a SET of requested
//! capacities spanning the magnitudes the modelled `QuoteCapacity$Quantity` carries:
//!
//! Every price figure this service states is a FINITE figure of the type the model
//! gives a price (`SubscriptionPricingDetails$UpfrontPrice` and
//! `$MonthlyRecurringPrice` both target `NullableFloat`), rounded to the cent and
//! following the capacity it prices. A payment combination whose figures are larger
//! than that type can carry is one this service has no price for, so it is left out
//! of the option exactly as a capacity there is no published figure for is — never
//! answered as the non-finite value the arithmetic saturates to, which is no number a
//! JSON client can read. What the create answered is what was persisted, so a later
//! `GetQuote` states the same figures.

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::ops::types::{
    PAYMENT_OPTION_VALUES, PAYMENT_TERM_VALUES,
};
use service_com_amazonaws_outposts_outpostsolafservice::state::{
    CURRENCY_USD, QUOTE_PRICING_TYPE_SUBSCRIPTION, SUBSCRIPTION_PRICE_PER_VCPU_MONTH,
};

use crate::quote_lifecycle::{create_quote, get_quote, request};
use crate::quote_options::{ec2, option_identifier, published_vcpus, sole_option};
use crate::renewal_pricing::{assert_price, months, price, upfront_share};
use crate::site_and_outpost_lifecycle::{text, Outposts};

/// The largest finite figure a modelled price member can state.
///
/// Source: `smithy_query(query="shape", shape="SubscriptionPricingDetails")` targets
/// both `UpfrontPrice` and `MonthlyRecurringPrice` at `NullableFloat`, whose
/// `smithy_query(query="shape", shape="NullableFloat")` is a single-precision number
/// — so the largest price either member can carry is that type's largest finite
/// value.
fn largest_price() -> f64 {
    f64::from(f32::MAX)
}

/// Whether `amount` is a price this service can state. The rows below sit at least a
/// factor of two clear of this bound, so no row turns on which way a figure within
/// half a step of it is rounded.
fn statable(amount: f64) -> bool {
    amount.is_finite() && amount <= largest_price()
}

/// One combination a quote is priced under: the payment option and term, and the two
/// figures that combination states.
type PricedCombination = (String, String, f64, f64);

/// The combinations a quote on `vcpus` vCPUs of EC2 capacity is priced under, in the
/// order the two modelled value sets name them, and the figures each carries — by the
/// service's own rule: the term's months of the monthly rate that capacity carries
/// make up the whole-term cost, of which the payment option's share is paid up front
/// and the rest in equal monthly instalments. A combination either of whose figures is
/// past what a price member can state is not among them.
fn expected_prices(vcpus: f64) -> Vec<PricedCombination> {
    let monthly_rate = vcpus * f64::from(SUBSCRIPTION_PRICE_PER_VCPU_MONTH);
    PAYMENT_OPTION_VALUES
        .iter()
        .flat_map(|payment_option| {
            PAYMENT_TERM_VALUES
                .iter()
                .map(move |payment_term| (*payment_option, *payment_term))
        })
        .filter_map(|(payment_option, payment_term)| {
            let share = upfront_share(payment_option);
            let upfront = monthly_rate * months(payment_term) * share;
            let monthly = monthly_rate * (1.0 - share);
            (vcpus > 0.0 && statable(upfront) && statable(monthly)).then(|| {
                (
                    payment_option.to_string(),
                    payment_term.to_string(),
                    upfront,
                    monthly,
                )
            })
        })
        .collect()
}

/// The `(PaymentOption, PaymentTerm)` pairs one option reports, in reported order.
fn reported_combinations(reported: &[Value], case: &str) -> Vec<(String, String)> {
    reported
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

/// The rule over one answered option: it is priced under exactly the combinations
/// whose figures a price member can state, each at the figure the rule gives, and
/// under no other.
fn assert_prices(option: &Value, expected: &[PricedCombination], case: &str) {
    let reported = option
        .get("PricingOptions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected PricingOptions, got {option}"))
        .clone();
    let expected_combinations: Vec<(String, String)> = expected
        .iter()
        .map(|(payment_option, payment_term, _, _)| {
            (payment_option.clone(), payment_term.clone())
        })
        .collect();
    assert_eq!(
        reported_combinations(&reported, case),
        expected_combinations,
        "case {case:?}: expected the option priced under exactly the combinations whose figures a \
         price member can state, got {option}"
    );
    for (pricing, (payment_option, payment_term, upfront, monthly)) in
        reported.iter().zip(expected)
    {
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
        assert_price(
            price(details, "UpfrontPrice", case),
            *upfront,
            "UpfrontPrice",
            case,
        );
        assert_price(
            price(details, "MonthlyRecurringPrice", case),
            *monthly,
            "MonthlyRecurringPrice",
            case,
        );
        // Whatever the capacity, a figure the service DID state is a number inside
        // what the modelled member carries: never a non-finite token, and never
        // dropped from a combination the option reports.
        for member in ["UpfrontPrice", "MonthlyRecurringPrice"] {
            let figure = price(details, member, case)
                .unwrap_or_else(|| panic!("case {case:?}: expected {member}, got {details}"));
            assert!(
                statable(figure),
                "case {case:?}: {member} must be a figure a price member can state, got {figure}"
            );
        }
    }
}

/// One row of the table below: the case, the capacities the quote asks for, the vCPUs
/// they come to, and how many of the nine modelled payment combinations a price can be
/// stated for — spelled out so a row that stops exercising the regime it was written
/// for says so.
type RangeCase = (&'static str, Value, f64, usize);

/// The option provides exactly the capacity the quote asked for.
///
/// `Quantity` is compared at the precision the model gives it — a single-precision
/// number — rather than at the double a JSON reader hands back, which carries digits
/// that member never had.
fn assert_capacities_echo(option: &Value, requested: &Value, case: &str) {
    let echoed = option
        .get("Capacities")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected Capacities, got {option}"));
    let requested = requested
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the row asks for a list of capacities"));
    assert_eq!(
        echoed.len(),
        requested.len(),
        "case {case:?}: the option provides the capacities the quote asks for, got {option}"
    );
    for (echoed, requested) in echoed.iter().zip(requested) {
        for member in ["QuoteCapacityType", "Unit"] {
            assert_eq!(
                text(echoed, member),
                text(requested, member),
                "case {case:?}: expected {member} {:?}, got {echoed}",
                text(requested, member)
            );
        }
        let figure = |capacity: &Value| {
            price(capacity, "Quantity", case).map(|quantity| quantity as f32)
        };
        assert_eq!(
            figure(echoed),
            figure(requested),
            "case {case:?}: expected Quantity {:?}, got {echoed}",
            figure(requested)
        );
    }
}

/// `value` as the modelled `QuoteCapacity$Quantity` carries it: that member is a
/// single-precision number, so this is the figure the request states and the quote
/// echoes back for it.
fn quantity(value: f32) -> f64 {
    f64::from(value)
}

/// The rule: a quote states a price for exactly the payment combinations whose figures
/// a modelled price member can carry, and the figures it states follow the capacity.
///
/// The rows differ along the magnitude of the requested capacity — the one thing the
/// rule generalises over: a single instance, a quantity so small every figure rounds
/// to nothing, a large but ordinary fleet, several capacities totalled, a quantity
/// whose whole-term cost is near the top of what a price can state, one where only the
/// longer terms are past it, one where only the monthly figures are inside it, one
/// where no figure of any combination is, the largest quantity the modelled
/// single-precision `Quantity` carries, and capacity of a type with no published vCPU
/// figure at all.
#[tokio::test]
async fn a_quote_states_only_the_prices_its_modelled_price_members_can_carry() {
    let outposts = Outposts::new();
    let small = published_vcpus(&outposts, "c5.large");
    let large = published_vcpus(&outposts, "c5.24xlarge");

    let cases: Vec<RangeCase> = vec![
        (
            "one instance of a published type",
            json!([ec2("c5.large", 1.0)]),
            small,
            9,
        ),
        (
            "a quantity so small every figure rounds to nothing",
            json!([ec2("c5.large", quantity(1e-30))]),
            small * quantity(1e-30),
            9,
        ),
        (
            "a fleet of the largest published type",
            json!([ec2("c5.24xlarge", 12_345.0)]),
            large * 12_345.0,
            9,
        ),
        (
            "two capacities of two types, totalled",
            json!([ec2("c5.large", 1.0), ec2("c5.24xlarge", 2.0)]),
            small + large * 2.0,
            9,
        ),
        (
            "a whole-term cost near the top of what a price can state",
            json!([ec2("c5.large", quantity(2e34))]),
            small * quantity(2e34),
            9,
        ),
        (
            "a capacity whose longer terms cost more than a price can state",
            json!([ec2("c5.large", quantity(2.5e35))]),
            small * quantity(2.5e35),
            7,
        ),
        (
            "a capacity only some monthly and part-upfront figures are inside the range for",
            json!([ec2("c5.large", quantity(1e36))]),
            small * quantity(1e36),
            4,
        ),
        (
            "a capacity no figure of any combination can state",
            json!([ec2("c5.large", quantity(1e38))]),
            small * quantity(1e38),
            0,
        ),
        (
            "the largest quantity the modelled single-precision Quantity carries",
            json!([ec2("c5.large", quantity(f32::MAX))]),
            small * quantity(f32::MAX),
            0,
        ),
        (
            "a huge quantity of a capacity type with no published vCPU figure",
            json!([{
                "QuoteCapacityType": "EBS",
                "Unit": "TiB",
                "Quantity": quantity(1e38),
            }]),
            0.0,
            0,
        ),
    ];

    for (case, capacities, vcpus, priced) in &cases {
        let expected = expected_prices(*vcpus);
        assert_eq!(
            expected.len(),
            *priced,
            "case {case:?}: the row is written for {priced} statable combinations of the nine, \
             but the rule gives {}",
            expected.len()
        );

        let created = create_quote(&outposts, &request("US", capacities.clone(), json!({}))).await;
        let option = sole_option(&created, case);
        // The configuration is offered whatever its price comes to: a capacity the
        // service cannot state a price for is still the option the quote describes.
        option_identifier(&created, case);
        assert_capacities_echo(&option, capacities, case);
        assert_prices(&option, &expected, case);

        // The option is persisted with the quote, so a later read states the same
        // figures — and a figure the create left out is not invented on the way back.
        let read = get_quote(&outposts, &text(&created, "QuoteId")).await;
        assert_eq!(
            read.get("QuoteOptions"),
            created.get("QuoteOptions"),
            "case {case:?}: GetQuote must answer the option the create reported"
        );
    }
}
