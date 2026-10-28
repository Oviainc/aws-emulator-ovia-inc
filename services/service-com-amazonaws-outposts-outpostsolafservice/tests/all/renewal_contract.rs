// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateRenewal`, driven over the wire through the same front
//! door as [`crate::site_and_outpost_lifecycle`] (whose harness this suite reuses),
//! and priced by the same rule as [`crate::renewal_pricing`] (whose order-placing and
//! price helpers it reuses, so the rule is stated once for both suites).
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. `CreateRenewal` buys a renewal of the subscription behind the Outpost the
//!    identifier names, under the payment option and payment term the request asks
//!    for, and answers that contract: the Outpost's own bare id, the option and term
//!    it covers, and the price that follows from the EC2 capacity the Outpost's own
//!    orders subscribe to — the whole-term cost split into the option's upfront share
//!    and equal monthly instalments for the rest, in the one currency the model
//!    declares. It is the SAME price `GetRenewalPricing` reports for that
//!    combination.
//! 2. An Outpost with nothing subscribed has no subscription a contract could renew,
//!    so the request is refused (400 `ValidationException`) and nothing is recorded:
//!    one nobody has ordered hardware for, one whose order was cancelled, one whose
//!    order names a catalog item this service publishes no configuration for, and one
//!    whose order carries no line item of its own.
//! 3. The contract is PERSISTED, and the modelled `@idempotencyToken` makes a repeat
//!    of the same request that one contract again: a request naming a `ClientToken`
//!    the Outpost already holds a renewal under is answered the stored contract, at
//!    the price it was bought at, even after the hardware under subscription has
//!    changed — while a request naming another token, or none at all, is a new
//!    contract priced from the hardware as it stands now.
//! 4. Both forms the modelled `OutpostIdentifier` pattern
//!    `^(arn:…:outpost/)?op-[a-f0-9]{17}$` accepts — the bare id and the full Outpost
//!    ARN — name the same Outpost, and the contract answers the bare id either way.
//! 5. An identifier that names no Outpost is 404 `NotFoundException` with the
//!    identifier echo of the value AS SUPPLIED; a request outside a modelled
//!    constraint — an identifier that is no Outpost identifier, a missing or
//!    undeclared payment option or term, a token outside its modelled length — is 400
//!    `ValidationException`.

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::ops::types::{
    PAYMENT_OPTION_VALUES, PAYMENT_TERM_VALUES,
};
use service_com_amazonaws_outposts_outpostsolafservice::state::{
    store_full, RenewalRecord, CURRENCY_USD, MAX_RESOURCES, SUBSCRIPTION_PRICE_PER_VCPU_MONTH,
};

use crate::renewal_pricing::{
    assert_price, cancelled, months, order, place_orders, price, published_catalog,
    route as pricing_route, subscribed_capacity, upfront_share, OrderSpec, CENT, UNPUBLISHED_SKU,
};
use crate::site_and_outpost_lifecycle::{
    refusal, text, Outposts, Refusal, ABSENT_OUTPOST_ID, HOME_REGION, OUTPOST_ARN_PREFIX,
};

/// The route `CreateRenewal` binds (`POST /renewals`).
const ROUTE: &str = "/renewals";

/// One `CreateRenewal` request: the Outpost to renew, the terms to renew it under,
/// and the idempotency token when the case carries one.
fn request(
    identifier: &str,
    payment_option: &str,
    payment_term: &str,
    client_token: Option<&str>,
) -> Value {
    let mut body = json!({
        "OutpostIdentifier": identifier,
        "PaymentOption": payment_option,
        "PaymentTerm": payment_term,
    });
    if let Some(token) = client_token {
        body["ClientToken"] = json!(token);
    }
    body
}

/// `CreateRenewal`, requiring HTTP 200 and returning the contract it answered with.
async fn buy(outposts: &Outposts, body: &Value) -> Value {
    outposts.ok("POST", ROUTE, Some(body)).await
}

/// Rule 1, over one answer: the contract names the Outpost and the terms it covers,
/// and its price is the one the service's own rule computes for `capacity` vCPUs of
/// subscribed hardware under those terms.
fn assert_contract(
    reply: &Value,
    outpost_id: &str,
    payment_option: &str,
    payment_term: &str,
    capacity: i64,
    case: &str,
) {
    assert_eq!(
        text(reply, "OutpostId"),
        outpost_id,
        "case {case:?}: the contract names the Outpost's own bare id, got {reply}"
    );
    assert_eq!(
        reply.get("PaymentOption").and_then(Value::as_str),
        Some(payment_option),
        "case {case:?}: expected PaymentOption {payment_option:?}, got {reply}"
    );
    assert_eq!(
        reply.get("PaymentTerm").and_then(Value::as_str),
        Some(payment_term),
        "case {case:?}: expected PaymentTerm {payment_term:?}, got {reply}"
    );
    assert_eq!(
        reply.get("Currency").and_then(Value::as_str),
        Some(CURRENCY_USD),
        "case {case:?}: expected Currency {CURRENCY_USD:?}, got {reply}"
    );
    let monthly_rate = capacity as f64 * f64::from(SUBSCRIPTION_PRICE_PER_VCPU_MONTH);
    let months = months(payment_term);
    let share = upfront_share(payment_option);
    let term_cost = monthly_rate * months;
    assert_price(
        price(reply, "UpfrontPrice", case),
        term_cost * share,
        "UpfrontPrice",
        case,
    );
    assert_price(
        price(reply, "MonthlyRecurringPrice", case),
        monthly_rate * (1.0 - share),
        "MonthlyRecurringPrice",
        case,
    );
    // The invariant behind the split, whatever the rate is: what the subscriber pays
    // over the whole term is the term's cost, under every payment option of it.
    let paid = price(reply, "UpfrontPrice", case).unwrap_or_default()
        + price(reply, "MonthlyRecurringPrice", case).unwrap_or_default() * months;
    assert!(
        (paid - term_cost).abs() <= CENT * months,
        "case {case:?}: upfront plus {months} monthly instalments must be the term's cost \
         {term_cost}, got {paid}"
    );
}

/// The price `GetRenewalPricing` reports for `(payment_option, payment_term)` on
/// `outpost_id`, as the `(UpfrontPrice, MonthlyRecurringPrice)` pair — what a renewal
/// bought under those terms has to cost, since both answer one rule.
async fn quoted(
    outposts: &Outposts,
    outpost_id: &str,
    payment_option: &str,
    payment_term: &str,
    case: &str,
) -> (Option<f64>, Option<f64>) {
    let reply = outposts
        .ok("GET", &pricing_route(outpost_id), None)
        .await;
    let options = reply
        .get("PricingOptions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected PricingOptions, got {reply}"))
        .clone();
    let details = options
        .iter()
        .filter_map(|option| option.get("SubscriptionPricingDetails"))
        .find(|details| {
            details.get("PaymentOption").and_then(Value::as_str) == Some(payment_option)
                && details.get("PaymentTerm").and_then(Value::as_str) == Some(payment_term)
        })
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: the pricing read offers {payment_option} {payment_term}, \
                 got {reply}"
            )
        })
        .clone();
    (
        price(&details, "UpfrontPrice", case),
        price(&details, "MonthlyRecurringPrice", case),
    )
}

/// Rules 1 + 4: a renewal is bought at the price the rule computes, under every
/// payment option and payment term the model declares — nine combinations — through
/// either identifier form, and at the price the pricing read quotes for it.
#[tokio::test]
async fn a_renewal_is_bought_under_every_payment_option_and_term() {
    let outposts = Outposts::new();
    let catalog = published_catalog(&outposts).await;
    let (smallest, _) = catalog.first().cloned().expect("the catalog is not empty");
    let (largest, _) = catalog.last().cloned().expect("the catalog is not empty");
    let (outpost_id, outpost_arn) = outposts.site_and_outpost("renewal-contract").await;
    // Two configurations, in different quantities: the capacity under subscription is
    // the catalog's own published figures, never a number this suite picked.
    let specs = vec![order(
        "PARTIAL_UPFRONT",
        &[(smallest.clone(), 1), (largest.clone(), 3)],
    )];
    place_orders(&outposts, &outpost_id, &specs, "every option and term").await;
    let capacity = subscribed_capacity(&specs, &catalog);
    assert!(
        capacity > 0,
        "the orders this case places subscribe hardware, got {capacity} vCPUs"
    );

    for payment_option in PAYMENT_OPTION_VALUES {
        for payment_term in PAYMENT_TERM_VALUES {
            let (upfront, monthly) = quoted(
                &outposts,
                &outpost_id,
                payment_option,
                payment_term,
                "the quote behind the contract",
            )
            .await;
            // Rule 4: either identifier form names the same Outpost, so each one buys
            // the same contract — under its own token, so neither is a repeat of the
            // other.
            for (form, identifier) in [("id", outpost_id.clone()), ("arn", outpost_arn.clone())] {
                let case = format!("{payment_option} {payment_term} bought by {identifier}");
                // A token of the modelled length (1..64) that no other row sends, so
                // no two of these calls are repeats of one another.
                let token = format!("token-{payment_option}-{payment_term}-{form}");
                let reply = buy(
                    &outposts,
                    &request(&identifier, payment_option, payment_term, Some(&token)),
                )
                .await;
                assert_contract(
                    &reply,
                    &outpost_id,
                    payment_option,
                    payment_term,
                    capacity,
                    &case,
                );
                // Rule 1's last clause: one rule prices the quote and the contract, so
                // the two cannot disagree.
                assert_price(
                    price(&reply, "UpfrontPrice", &case),
                    upfront.unwrap_or_default(),
                    "UpfrontPrice quoted by GetRenewalPricing",
                    &case,
                );
                assert_price(
                    price(&reply, "MonthlyRecurringPrice", &case),
                    monthly.unwrap_or_default(),
                    "MonthlyRecurringPrice quoted by GetRenewalPricing",
                    &case,
                );
            }
        }
    }
}

/// Rules 1 + 2: the price follows the hardware under subscription, and an Outpost
/// with nothing subscribed is refused.
///
/// The rows differ along everything the rule generalises over: which configuration was
/// ordered, how many units of it, how many line items an order carries, how many
/// orders the Outpost holds, whether an order was cancelled, whether it names a
/// catalog item the service publishes nothing for, and whether it carries line items
/// at all.
#[tokio::test]
async fn the_price_follows_the_hardware_under_subscription() {
    let outposts = Outposts::new();
    let catalog = published_catalog(&outposts).await;
    let (smallest, _) = catalog.first().cloned().expect("the catalog is not empty");
    let (largest, _) = catalog.last().cloned().expect("the catalog is not empty");

    let mut cases: Vec<(String, Vec<OrderSpec>)> = vec![
        ("no order at all".to_string(), Vec::new()),
        (
            "one unit of the smallest configuration".to_string(),
            vec![order("ALL_UPFRONT", &[(smallest.clone(), 1)])],
        ),
        (
            "four units of the largest configuration".to_string(),
            vec![order("NO_UPFRONT", &[(largest.clone(), 4)])],
        ),
        (
            "two configurations in one order".to_string(),
            vec![order(
                "PARTIAL_UPFRONT",
                &[(smallest.clone(), 2), (largest.clone(), 1)],
            )],
        ),
        (
            "two orders on the same Outpost".to_string(),
            vec![
                order("ALL_UPFRONT", &[(smallest.clone(), 1)]),
                order("NO_UPFRONT", &[(largest.clone(), 1)]),
            ],
        ),
        (
            "a cancelled order beside a live one".to_string(),
            vec![
                cancelled(order("NO_UPFRONT", &[(largest.clone(), 2)])),
                order("ALL_UPFRONT", &[(smallest.clone(), 1)]),
            ],
        ),
        (
            "the only order was cancelled".to_string(),
            vec![cancelled(order("ALL_UPFRONT", &[(largest.clone(), 2)]))],
        ),
        (
            "an order of a catalog item the service publishes nothing for".to_string(),
            vec![order("ALL_UPFRONT", &[(UNPUBLISHED_SKU.to_string(), 4)])],
        ),
        (
            "an order taken from a quote, with no line item of its own".to_string(),
            vec![order("ALL_UPFRONT", &[])],
        ),
    ];
    // Every configuration the catalog publishes, one unit of it: the rows above name
    // only the smallest and the largest, and the rule holds for each entry.
    for (sku, _) in &catalog {
        cases.push((
            format!("one unit of {sku}"),
            vec![order("PARTIAL_UPFRONT", &[(sku.clone(), 1)])],
        ));
    }

    for (index, (case, specs)) in cases.iter().enumerate() {
        let (outpost_id, _) = outposts
            .site_and_active_outpost(&format!("renewal-case-{index}"), "US")
            .await;
        place_orders(&outposts, &outpost_id, specs, case).await;
        let capacity = subscribed_capacity(specs, &catalog);
        let body = request(&outpost_id, "PARTIAL_UPFRONT", "THREE_YEARS", None);
        let reply = outposts.call("POST", ROUTE, Some(&body)).await;
        if capacity == 0 {
            // Rule 2: nothing is subscribed, so there is no subscription to renew.
            assert_eq!(
                (reply.status, reply.error_code.as_str()),
                (400, "ValidationException"),
                "case {case:?}: nothing is subscribed on this Outpost, so a renewal of it \
                 cannot be bought, got {} {} {}",
                reply.status,
                reply.error_code,
                reply.body
            );
            // And the refusal recorded nothing: once hardware IS subscribed, the same
            // request buys a contract priced from it.
            let recovery = vec![order("ALL_UPFRONT", &[(smallest.clone(), 1)])];
            place_orders(&outposts, &outpost_id, &recovery, case).await;
            let case = &format!("{case} after hardware was ordered");
            let served = buy(&outposts, &body).await;
            assert_contract(
                &served,
                &outpost_id,
                "PARTIAL_UPFRONT",
                "THREE_YEARS",
                subscribed_capacity(&recovery, &catalog),
                case,
            );
        } else {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: expected the renewal to be served, got {} {}",
                reply.status, reply.body
            );
            assert_contract(
                &reply.body,
                &outpost_id,
                "PARTIAL_UPFRONT",
                "THREE_YEARS",
                capacity,
                case,
            );
        }
    }
}

/// Rule 3: the contract is persisted, and a repeated client token is that one
/// contract again rather than a second, re-priced one.
#[tokio::test]
async fn a_repeated_client_token_answers_the_contract_that_was_bought() {
    let outposts = Outposts::new();
    let catalog = published_catalog(&outposts).await;
    let (sku, _) = catalog.last().cloned().expect("the catalog is not empty");
    let (outpost_id, _) = outposts.site_and_outpost("renewal-idempotency").await;
    let first = vec![order("ALL_UPFRONT", &[(sku.clone(), 1)])];
    place_orders(&outposts, &outpost_id, &first, "the first order").await;
    let bought_capacity = subscribed_capacity(&first, &catalog);

    const TOKEN: &str = "renewal-token-1";
    const OTHER_TOKEN: &str = "renewal-token-2";
    let body = request(&outpost_id, "ALL_UPFRONT", "ONE_YEAR", Some(TOKEN));
    let bought = buy(&outposts, &body).await;
    assert_contract(
        &bought,
        &outpost_id,
        "ALL_UPFRONT",
        "ONE_YEAR",
        bought_capacity,
        "the contract as bought",
    );

    // The hardware under subscription grows, so a renewal bought NOW costs more —
    // which is what makes the repeat below a statement about what was persisted.
    let second = vec![order("NO_UPFRONT", &[(sku.clone(), 3)])];
    place_orders(&outposts, &outpost_id, &second, "the second order").await;
    let grown_capacity = bought_capacity + subscribed_capacity(&second, &catalog);
    assert!(
        grown_capacity > bought_capacity,
        "the second order grows the subscribed capacity past {bought_capacity}, got \
         {grown_capacity}"
    );

    for repeat in 1..=2 {
        let case = format!("repeat {repeat} of the same request");
        let again = buy(&outposts, &body).await;
        assert_eq!(
            again, bought,
            "case {case:?}: the token names the contract that was bought, so the answer is \
             that contract again, got {again}"
        );
        assert_contract(
            &again,
            &outpost_id,
            "ALL_UPFRONT",
            "ONE_YEAR",
            bought_capacity,
            &case,
        );
    }

    // Another token is another request, so it buys a new contract — priced from the
    // hardware as it stands now, and alongside the first rather than instead of it.
    let newer = buy(
        &outposts,
        &request(&outpost_id, "ALL_UPFRONT", "ONE_YEAR", Some(OTHER_TOKEN)),
    )
    .await;
    assert_contract(
        &newer,
        &outpost_id,
        "ALL_UPFRONT",
        "ONE_YEAR",
        grown_capacity,
        "a second contract under another token",
    );
    assert_eq!(
        buy(&outposts, &body).await,
        bought,
        "the first contract is still held under its own token, got {newer}"
    );

    // A request that names NO token is not a repeat of anything: each one is its own
    // contract, priced from the hardware as it stands.
    for untokenized in 1..=2 {
        let case = format!("untokenized request {untokenized}");
        let reply = buy(
            &outposts,
            &request(&outpost_id, "NO_UPFRONT", "FIVE_YEARS", None),
        )
        .await;
        assert_contract(
            &reply,
            &outpost_id,
            "NO_UPFRONT",
            "FIVE_YEARS",
            grown_capacity,
            &case,
        );
    }

    // The same token on OTHER terms is not a repeat of the request that bought the
    // contract, so it is refused rather than answered a contract it did not ask for —
    // and the contract it names is left exactly as it was.
    for (case, payment_option, payment_term) in [
        ("another payment term", "ALL_UPFRONT", "FIVE_YEARS"),
        ("another payment option", "NO_UPFRONT", "ONE_YEAR"),
        ("both", "PARTIAL_UPFRONT", "THREE_YEARS"),
    ] {
        let reply = outposts
            .call(
                "POST",
                ROUTE,
                Some(&request(&outpost_id, payment_option, payment_term, Some(TOKEN))),
            )
            .await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: a token already used for a renewal of this Outpost cannot name \
             other terms, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
    }
    assert_eq!(
        buy(&outposts, &body).await,
        bought,
        "the refused requests left the contract the token names exactly as it was"
    );
}

/// Rule 5: an identifier that names no Outpost is 404 `NotFoundException`; a request
/// outside a modelled constraint is 400 `ValidationException`.
///
/// The rows span both identifier forms of an absent Outpost, the ways an Outpost id
/// can be malformed, a missing required member, a payment option and a payment term
/// the model does not declare, and a client token outside its modelled length.
#[tokio::test]
async fn absent_outposts_are_not_found_and_bad_requests_are_refused() {
    let outposts = Outposts::new();
    let absent_arn = format!("{OUTPOST_ARN_PREFIX}{ABSENT_OUTPOST_ID}");
    let valid = |identifier: &str| request(identifier, "ALL_UPFRONT", "ONE_YEAR", None);
    let not_found = |case: &'static str, identifier: &str| {
        refusal(case, "POST", ROUTE.to_string(), 404, "NotFoundException")
            .with_body(valid(identifier))
    };
    let refused = |case: &'static str, body: Value| {
        refusal(case, "POST", ROUTE.to_string(), 400, "ValidationException").with_body(body)
    };

    let cases: Vec<Refusal> = vec![
        not_found(
            "a well-formed Outpost id that names nothing",
            ABSENT_OUTPOST_ID,
        ),
        not_found("the ARN form of an Outpost that names nothing", &absent_arn),
        refused(
            "an identifier that is no identifier at all",
            valid("not-an-outpost-id"),
        ),
        refused(
            "an Outpost id whose suffix is not hex",
            valid("op-zzzzzzzzzzzzzzzzz"),
        ),
        refused(
            "an Outpost id one hex character short",
            valid("op-0000000000000000"),
        ),
        refused(
            "an Outpost id one hex character long",
            valid("op-000000000000000000"),
        ),
        refused(
            "no Outpost named at all",
            json!({ "PaymentOption": "ALL_UPFRONT", "PaymentTerm": "ONE_YEAR" }),
        ),
        refused(
            "no payment option named",
            json!({ "OutpostIdentifier": ABSENT_OUTPOST_ID, "PaymentTerm": "ONE_YEAR" }),
        ),
        refused(
            "no payment term named",
            json!({ "OutpostIdentifier": ABSENT_OUTPOST_ID, "PaymentOption": "ALL_UPFRONT" }),
        ),
        refused(
            "a payment option the model does not declare",
            request(ABSENT_OUTPOST_ID, "MONTHLY", "ONE_YEAR", None),
        ),
        refused(
            "a payment term the model does not declare",
            request(ABSENT_OUTPOST_ID, "ALL_UPFRONT", "TWO_YEARS", None),
        ),
        refused(
            "a client token shorter than the modelled minimum",
            request(ABSENT_OUTPOST_ID, "ALL_UPFRONT", "ONE_YEAR", Some("")),
        ),
        refused(
            "a client token longer than the modelled maximum",
            request(
                ABSENT_OUTPOST_ID,
                "ALL_UPFRONT",
                "ONE_YEAR",
                Some(&"t".repeat(65)),
            ),
        ),
    ];

    outposts.expect_refusals(&cases).await;
}

/// Rule 5's message half: a not-found refusal is the identifier ECHO
/// `outpostIdentifier=<value as supplied>` — the ARN form reads its ARN back, not the
/// bare id the service resolved it to.
#[tokio::test]
async fn a_not_found_outpost_echoes_the_identifier_the_request_supplied() {
    let outposts = Outposts::new();
    let absent_arn = format!("{OUTPOST_ARN_PREFIX}{ABSENT_OUTPOST_ID}");
    let cases: Vec<(&str, &str)> = vec![
        ("by bare id", ABSENT_OUTPOST_ID),
        ("by ARN", &absent_arn),
    ];

    for (case, identifier) in &cases {
        let reply = outposts
            .call(
                "POST",
                ROUTE,
                Some(&request(identifier, "ALL_UPFRONT", "ONE_YEAR", None)),
            )
            .await;
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

/// The store a renewal contract lands in is BOUNDED.
///
/// A contract is never released — only deleting its Outpost takes it away — so the
/// only thing that keeps a caller repeating `CreateRenewal` from growing the store
/// without limit is a maximum entry count, across every Outpost the store holds. Past
/// it the request is refused as the request this service will not accept, with the
/// wording that says the emulator's own store is full, and nothing is recorded.
///
/// The ceiling is `MAX_RESOURCES` entries, which no sequence of calls could fill
/// inside a test, so this case seeds the store directly and then drives the refusal
/// over the wire.
#[tokio::test]
async fn the_renewal_store_is_bounded() {
    let outposts = Outposts::new();
    let catalog = published_catalog(&outposts).await;
    let (sku, _) = catalog.first().cloned().expect("the catalog is not empty");
    let (below, _) = outposts.site_and_outpost("ceiling-below").await;
    let (at, _) = outposts.site_and_outpost("ceiling-at").await;
    for outpost_id in [&below, &at] {
        place_orders(
            &outposts,
            outpost_id,
            &[order("ALL_UPFRONT", &[(sku.clone(), 1)])],
            "the ceiling",
        )
        .await;
    }

    // One short of the ceiling, held on an Outpost of its own: the store can still
    // take a contract.
    seed_renewals(&outposts, &below, MAX_RESOURCES - 1);
    let accepted = buy(
        &outposts,
        &request(&below, "ALL_UPFRONT", "ONE_YEAR", None),
    )
    .await;
    assert_eq!(
        text(&accepted, "OutpostId"),
        below,
        "below the ceiling a renewal is bought, got {accepted}"
    );

    // And at the ceiling it cannot — on another Outpost, so what refuses the request
    // is the store's bound and not anything about the Outpost it names.
    let reply = outposts
        .call(
            "POST",
            ROUTE,
            Some(&request(&at, "ALL_UPFRONT", "ONE_YEAR", None)),
        )
        .await;
    assert_eq!(
        (reply.status, reply.error_code.as_str()),
        (400, "ValidationException"),
        "at the ceiling the renewal must be refused, got {} {} {}",
        reply.status,
        reply.error_code,
        reply.body
    );
    assert_eq!(
        text(&reply.body, "Message"),
        store_full("renewal contracts"),
        "the refusal must say the emulator's store is full, got {}",
        reply.body
    );
    let held = outposts.store().lock().renewals_held();
    assert_eq!(
        held, MAX_RESOURCES,
        "a refused renewal must record nothing: expected {MAX_RESOURCES} contracts held, \
         got {held}"
    );
}

/// Put `count` renewal contracts on the Outpost `outpost_id`, through the store seam
/// — the ceiling above is far past what a test could reach one call at a time.
///
/// Each carries no client token, so none of them is the repeat of any request the
/// cases above make, and the price the service's own rule computes for the terms it
/// names.
fn seed_renewals(outposts: &Outposts, outpost_id: &str, count: usize) {
    let store = outposts.store();
    let mut store = store.lock();
    let pricing = store
        .renewal_price(HOME_REGION, outpost_id, "ALL_UPFRONT", "ONE_YEAR")
        .expect("the seeded Outpost has a subscription to renew");
    let record = RenewalRecord {
        client_token: None,
        pricing,
    };
    let renewals = &mut store
        .outpost_mut(HOME_REGION, outpost_id)
        .expect("the seeded Outpost is held")
        .renewals;
    renewals.extend(std::iter::repeat_n(record, count));
}
