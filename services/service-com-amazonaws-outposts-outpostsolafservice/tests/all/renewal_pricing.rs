// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetRenewalPricing`, driven over the wire through the same
//! front door as [`crate::site_and_outpost_lifecycle`] (whose harness this suite
//! reuses).
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. `GetRenewalPricing` reports the renewal pricing RECORDED for the Outpost the
//!    identifier names — every priced subscription option of that Outpost, in the
//!    order it is held, field for field — and `PricingResult` is `PRICED` exactly
//!    when there is at least one such option and `UNABLE_TO_PRICE` when there is
//!    none. It invents nothing: a price the emulator never recorded is reported by
//!    no member.
//! 2. Both forms the modelled `OutpostIdentifier` pattern
//!    `^(arn:…:outpost/)?op-[a-f0-9]{17}$` accepts — the bare id and the full
//!    Outpost ARN — name the same Outpost, and the pricing read is a REPORT:
//!    repeating it answers the same thing, and it changes neither the recorded
//!    pricing nor the Outpost.
//! 3. An identifier that names no Outpost (including one whose Outpost was deleted)
//!    is 404 `NotFoundException` with the identifier echo of the value AS SUPPLIED;
//!    one that is no Outpost identifier at all is 400 `ValidationException`.
//!
//! Where the recorded options come from: a renewal price is a figure from AWS's own
//! price list for the hardware an Outpost's order installed, so nothing this
//! emulator computes — `CreateOrder`, `CreateRenewal` and
//! `GetOutpostBillingInformation`, the operations an Outpost's priced subscriptions
//! would arrive with, are still 501 scaffold stubs with issues of their own. Each
//! row therefore RECORDS its options on the stored Outpost through the same store
//! seam [`crate::quote_lifecycle`] ages a quote through, and then asks over the
//! wire: what the read answers about what is recorded is the whole of rule 1. The
//! figures below are the rows' own data; the emulator is never expected to produce a
//! number of its own.

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::ops::types::{
    PricingOption, SubscriptionPricingDetails, PAYMENT_OPTION_VALUES, PAYMENT_TERM_VALUES,
};
use service_com_amazonaws_outposts_outpostsolafservice::state::{
    PRICING_RESULT_PRICED, PRICING_RESULT_UNABLE_TO_PRICE,
};

use crate::site_and_outpost_lifecycle::{
    refusal, segment, settle, text, Outposts, Refusal, ABSENT_OUTPOST_ID, ABSENT_SITE_ID,
    OUTPOST_ARN_PREFIX,
};

/// The one value the modelled `QuotePricingType` enum declares, which a
/// `PricingOption$PricingType` carries (see
/// `ops::types::shared_types::QUOTE_PRICING_TYPE_VALUES`).
const SUBSCRIPTION: &str = "SUBSCRIPTION";

/// The one value the modelled `CurrencyCode` enum declares — "currently only
/// `USD` is supported", as the model documents
/// `SubscriptionPricingDetails$Currency`.
const USD: &str = "USD";

/// One priced subscription option, as a row records it on an Outpost and as the
/// wire must report it back.
#[derive(Debug, Clone)]
struct PricedOption {
    payment_option: &'static str,
    payment_term: &'static str,
    /// Every price member is `NullableFloat` and optional, so a row can record an
    /// option that carries one price, both, or neither.
    upfront: Option<f32>,
    monthly: Option<f32>,
    currency: Option<&'static str>,
}

impl PricedOption {
    /// The `PricingOption` this row records on the Outpost's own record.
    fn recorded(&self) -> PricingOption {
        PricingOption {
            pricing_type: Some(SUBSCRIPTION.to_string()),
            subscription_pricing_details: Some(SubscriptionPricingDetails {
                payment_option: Some(self.payment_option.to_string()),
                payment_term: Some(self.payment_term.to_string()),
                upfront_price: self.upfront,
                monthly_recurring_price: self.monthly,
                currency: self.currency.map(str::to_string),
            }),
        }
    }
}

/// A row's option, spelled out.
fn option(
    payment_option: &'static str,
    payment_term: &'static str,
    upfront: Option<f32>,
    monthly: Option<f32>,
    currency: Option<&'static str>,
) -> PricedOption {
    PricedOption {
        payment_option,
        payment_term,
        upfront,
        monthly,
        currency,
    }
}

/// Every `PaymentOption` × `PaymentTerm` combination the model declares: 3 × 3 = 9
/// options, which is exactly the modelled `PricingOptionList` maximum
/// (`length(0..9)`). The two value lists are the generated
/// `PAYMENT_OPTION_VALUES` / `PAYMENT_TERM_VALUES`, read off the model's enums, so
/// this row covers values no other row cites.
fn every_combination() -> Vec<PricedOption> {
    let mut options = Vec::new();
    for (index, payment_option) in PAYMENT_OPTION_VALUES.iter().enumerate() {
        for (step, payment_term) in PAYMENT_TERM_VALUES.iter().enumerate() {
            // Distinct, exactly representable figures, so an option reported out of
            // order or dropped shows up as a mismatch rather than a coincidence.
            let upfront = (index * 3 + step) as f32 * 1024.0;
            options.push(option(
                payment_option,
                payment_term,
                Some(upfront),
                Some(upfront / 32.0),
                Some(USD),
            ));
        }
    }
    options
}

/// The route `GetRenewalPricing` binds, for one identifier form.
fn route(identifier: &str) -> String {
    format!("/outpost/{}/renewal-pricing", segment(identifier))
}

/// A price member of a reported option, as a number — absent when the wire omits
/// it, which is how a client sees a price nothing recorded.
fn price(details: &Value, member: &str, case: &str) -> Option<f64> {
    details.get(member).map(|value| {
        value
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: {member} {value} is not a number"))
    })
}

/// Rule 1's option half: one reported option against the one a row recorded.
fn assert_reported(actual: &Value, expected: &PricedOption, case: &str, index: usize) {
    let case = &format!("{case} (option {index})");
    assert_eq!(
        actual.get("PricingType").and_then(Value::as_str),
        Some(SUBSCRIPTION),
        "case {case:?}: expected PricingType {SUBSCRIPTION:?}, got {actual}"
    );
    let details = actual
        .get("SubscriptionPricingDetails")
        .unwrap_or_else(|| panic!("case {case:?}: expected SubscriptionPricingDetails, got {actual}"));
    assert_eq!(
        details.get("PaymentOption").and_then(Value::as_str),
        Some(expected.payment_option),
        "case {case:?}: expected PaymentOption {:?}, got {details}",
        expected.payment_option
    );
    assert_eq!(
        details.get("PaymentTerm").and_then(Value::as_str),
        Some(expected.payment_term),
        "case {case:?}: expected PaymentTerm {:?}, got {details}",
        expected.payment_term
    );
    assert_eq!(
        details.get("Currency").and_then(Value::as_str),
        expected.currency,
        "case {case:?}: expected Currency {:?}, got {details}",
        expected.currency
    );
    for (member, recorded) in [
        ("UpfrontPrice", expected.upfront),
        ("MonthlyRecurringPrice", expected.monthly),
    ] {
        let actual = price(details, member, case);
        assert_eq!(
            actual,
            recorded.map(f64::from),
            "case {case:?}: expected {member} {:?}, got {actual:?}",
            recorded.map(f64::from)
        );
    }
}

/// An Outpost to record pricing on, as its `OutpostId`. An Outpost belongs to a
/// site, so the site comes first — the same order a real workflow takes.
async fn an_outpost(outposts: &Outposts, name: &str) -> String {
    let site = outposts
        .create_site(&json!({ "Name": format!("{name}-site") }))
        .await;
    let outpost = outposts
        .create_outpost(&json!({ "Name": name, "SiteId": text(&site, "SiteId") }))
        .await;
    text(&outpost, "OutpostId")
}

/// Record `options` as the renewal pricing of the stored Outpost. See this module's
/// header for why a row records them through the store.
fn record(outposts: &Outposts, outpost_id: &str, options: &[PricedOption], case: &str) {
    let store = outposts.store();
    let mut store = store.lock();
    let record = store
        .outposts
        .get_mut(outpost_id)
        .unwrap_or_else(|| panic!("case {case:?}: the create persisted the Outpost"));
    record.renewal_pricing = options.iter().map(PricedOption::recorded).collect();
}

/// Rules 1 + 2: the read answers every option recorded for the Outpost, with the
/// `PricingResult` that says whether it could be priced, through either identifier
/// form.
///
/// The rows differ along everything the rule generalises over: how many options are
/// recorded (none, one, two, the modelled maximum of nine), which payment option and
/// term each carries, and which of the optional price and currency members were
/// recorded at all (both prices, only the monthly one, neither, a zero price, no
/// currency).
#[tokio::test]
async fn renewal_pricing_reports_every_option_recorded_for_the_outpost() {
    let outposts = Outposts::new();
    let cases: Vec<(&str, Vec<PricedOption>)> = vec![
        ("no option recorded", Vec::new()),
        (
            "one option, both prices",
            vec![option(
                "ALL_UPFRONT",
                "THREE_YEARS",
                Some(123_456.0),
                Some(4_725.25),
                Some(USD),
            )],
        ),
        (
            "one option with no upfront payment, a zero upfront price",
            vec![option(
                "NO_UPFRONT",
                "ONE_YEAR",
                Some(0.0),
                Some(9_999.5),
                Some(USD),
            )],
        ),
        (
            "one option whose prices were not recorded",
            vec![option("PARTIAL_UPFRONT", "FIVE_YEARS", None, None, None)],
        ),
        (
            "two options on the same term, one of them without a currency",
            vec![
                option(
                    "PARTIAL_UPFRONT",
                    "FIVE_YEARS",
                    Some(250_000.0),
                    Some(1_500.0),
                    Some(USD),
                ),
                option("NO_UPFRONT", "FIVE_YEARS", None, Some(7_250.75), None),
            ],
        ),
        (
            "every payment option and term, the modelled nine-option maximum",
            every_combination(),
        ),
    ];

    for (case, options) in &cases {
        let outpost_id = an_outpost(&outposts, "renewal-pricing").await;
        record(&outposts, &outpost_id, options, case);

        let expected_result = if options.is_empty() {
            PRICING_RESULT_UNABLE_TO_PRICE
        } else {
            PRICING_RESULT_PRICED
        };
        // Rule 2's identifier half: the bare id and the full Outpost ARN name the
        // same Outpost, so both answer the same pricing.
        for identifier in [
            outpost_id.clone(),
            format!("{OUTPOST_ARN_PREFIX}{outpost_id}"),
        ] {
            let case = &format!("{case} read by {identifier}");
            let reply = outposts.ok("GET", &route(&identifier), None).await;
            assert_eq!(
                text(&reply, "PricingResult"),
                expected_result,
                "case {case:?}: {} option(s) recorded, so expected PricingResult \
                 {expected_result:?}, got {reply}",
                options.len()
            );
            let reported = reply
                .get("PricingOptions")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {case:?}: expected PricingOptions, got {reply}"));
            assert_eq!(
                reported.len(),
                options.len(),
                "case {case:?}: expected {} option(s), got {reply}",
                options.len()
            );
            for (index, (actual, expected)) in reported.iter().zip(options).enumerate() {
                assert_reported(actual, expected, case, index);
            }
        }
    }
}

/// Rule 2's report half: asking for an Outpost's renewal pricing changes nothing —
/// not the pricing it answers, not the Outpost, not another Outpost's pricing.
#[tokio::test]
async fn reading_renewal_pricing_changes_nothing_a_later_call_sees() {
    let outposts = Outposts::new();
    let priced = an_outpost(&outposts, "priced").await;
    let unpriced = an_outpost(&outposts, "unpriced").await;
    let options = every_combination();
    record(&outposts, &priced, &options, "priced");

    // Past the Outpost lifecycle transition the clock owes a just-created Outpost,
    // so the Outpost snapshots below differ only if THIS read changed one.
    settle();
    let before = outposts.get_outpost(&priced).await;

    let mut answers = Vec::new();
    for pass in ["first read", "second read", "third read"] {
        answers.push((pass, outposts.ok("GET", &route(&priced), None).await));
        // Another Outpost's pricing is its own: reading one never fills in another.
        let other = outposts.ok("GET", &route(&unpriced), None).await;
        assert_eq!(
            text(&other, "PricingResult"),
            PRICING_RESULT_UNABLE_TO_PRICE,
            "case {pass:?}: an Outpost with no recorded option cannot be priced, got {other}"
        );
        assert_eq!(
            other.get("PricingOptions"),
            Some(&json!([])),
            "case {pass:?}: an Outpost with no recorded option reports no option, got {other}"
        );
    }
    let (_, first) = &answers[0];
    for (pass, answer) in &answers {
        assert_eq!(
            answer, first,
            "case {pass:?}: every read of one Outpost's renewal pricing must answer the same"
        );
    }

    let after = outposts.get_outpost(&priced).await;
    assert_eq!(
        after, before,
        "reading renewal pricing must not change the Outpost itself"
    );
    let held = outposts.store().lock().outposts[&priced].renewal_pricing.len();
    assert_eq!(
        held,
        options.len(),
        "reading renewal pricing must leave the recorded options alone: expected {} held, got \
         {held}",
        options.len()
    );
}

/// Rule 3: an identifier that names no Outpost is 404 `NotFoundException`; one that
/// is no Outpost identifier at all is 400 `ValidationException`, from the boundary.
///
/// The rows span both identifier forms, an identifier of another resource kind, and
/// the ways an Outpost id can be malformed (wrong prefix, non-hex suffix, uppercase
/// hex, one character short, one character long).
#[tokio::test]
async fn an_absent_outpost_is_not_found_and_a_malformed_identifier_is_refused() {
    let outposts = Outposts::new();
    // An Outpost that existed, was recorded pricing, and was then deleted: its
    // pricing went with it, so the route answers as it does for one that never was.
    let deleted = an_outpost(&outposts, "deleted").await;
    record(
        &outposts,
        &deleted,
        &[option("ALL_UPFRONT", "ONE_YEAR", Some(1_024.0), None, Some(USD))],
        "deleted",
    );
    outposts
        .ok("DELETE", &format!("/outposts/{deleted}"), None)
        .await;

    let cases: Vec<Refusal> = vec![
        refusal(
            "a well-formed Outpost id that names nothing",
            "GET",
            route(ABSENT_OUTPOST_ID),
            404,
            "NotFoundException",
        ),
        refusal(
            "the ARN form of an Outpost that names nothing",
            "GET",
            route(&format!("{OUTPOST_ARN_PREFIX}{ABSENT_OUTPOST_ID}")),
            404,
            "NotFoundException",
        ),
        refusal(
            "an Outpost that was deleted",
            "GET",
            route(&deleted),
            404,
            "NotFoundException",
        ),
        refusal(
            "a label that is no identifier at all",
            "GET",
            route("not-an-outpost-id"),
            400,
            "ValidationException",
        ),
        refusal(
            "a site id",
            "GET",
            route(ABSENT_SITE_ID),
            400,
            "ValidationException",
        ),
        refusal(
            "an Outpost id whose suffix is not hex",
            "GET",
            route("op-zzzzzzzzzzzzzzzzz"),
            400,
            "ValidationException",
        ),
        refusal(
            "an Outpost id whose hex is uppercase",
            "GET",
            route("op-0000000000000000A"),
            400,
            "ValidationException",
        ),
        refusal(
            "an Outpost id one hex character short",
            "GET",
            route("op-0000000000000000"),
            400,
            "ValidationException",
        ),
        refusal(
            "an Outpost id one hex character long",
            "GET",
            route("op-000000000000000000"),
            400,
            "ValidationException",
        ),
    ];

    outposts.expect_refusals(&cases).await;
}

/// Rule 3's message half: a not-found refusal is the identifier ECHO
/// `outpostIdentifier=<value as supplied>` — the ARN form reads its ARN back, not
/// the bare id the service resolved it to.
#[tokio::test]
async fn a_not_found_outpost_echoes_the_identifier_the_request_supplied() {
    let outposts = Outposts::new();
    let absent_arn = format!("{OUTPOST_ARN_PREFIX}{ABSENT_OUTPOST_ID}");
    let cases: Vec<(&str, &str)> = vec![
        ("by bare id", ABSENT_OUTPOST_ID),
        ("by ARN", &absent_arn),
    ];

    for (case, identifier) in &cases {
        let reply = outposts.call("GET", &route(identifier), None).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (404, "NotFoundException"),
            "case {case:?}: expected 404 NotFoundException, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        let expected = format!("outpostIdentifier={identifier}");
        let actual = reply
            .body
            .get("Message")
            .or_else(|| reply.body.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the identifier echo {expected:?}, got {actual:?}"
        );
    }
}
