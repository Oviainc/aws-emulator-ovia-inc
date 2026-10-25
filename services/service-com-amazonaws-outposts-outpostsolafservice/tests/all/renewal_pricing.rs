// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetRenewalPricing`, driven over the wire through the same
//! front door as [`crate::site_and_outpost_lifecycle`] (whose harness this suite
//! reuses).
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. `GetRenewalPricing` prices a renewal of the subscription behind the Outpost the
//!    identifier names: one `SUBSCRIPTION` pricing option per payment option and
//!    payment term the model declares — three of each, so the nine the modelled
//!    `PricingOptionList` maximum allows — each priced from the EC2 capacity the
//!    Outpost's own orders subscribe to, with the term's whole cost split into the
//!    option's upfront share and equal monthly instalments for the rest.
//!    `PricingResult` is `PRICED` exactly when there is at least one such option.
//! 2. An Outpost with nothing subscribed has no subscription a renewal of could be
//!    priced, and is answered `UNABLE_TO_PRICE` with no option at all: one nobody has
//!    ordered hardware for, one whose order was cancelled, one whose order names a
//!    catalog item this service publishes no configuration for, and one whose order
//!    carries no line item of its own.
//! 3. Both forms the modelled `OutpostIdentifier` pattern
//!    `^(arn:…:outpost/)?op-[a-f0-9]{17}$` accepts — the bare id and the full
//!    Outpost ARN — name the same Outpost, and the pricing read is a REPORT:
//!    repeating it answers the same thing, and it changes neither the orders it
//!    prices nor the Outpost.
//! 4. An identifier that names no Outpost (including one whose Outpost was deleted)
//!    is 404 `NotFoundException` with the identifier echo of the value AS SUPPLIED;
//!    one that is no Outpost identifier at all is 400 `ValidationException`.
//!
//! Where the priced capacity comes from: the hardware under subscription on an
//! Outpost is what its orders bought — the catalog items its line items name, times
//! the quantity of each — so every row here places real orders over the wire and then
//! asks for the pricing. The per-vCPU rate, the term lengths and the upfront shares
//! the service prices by are the service's own published rule, named here through the
//! constants that state it, so what each row asserts is the arithmetic over a set of
//! subscriptions rather than one remembered figure.

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::ops::types::{
    PAYMENT_OPTION_VALUES, PAYMENT_TERM_VALUES,
};
use service_com_amazonaws_outposts_outpostsolafservice::state::{
    CURRENCY_USD, ORDER_STATUS_COMPLETED, PAYMENT_OPTION_UPFRONT_SHARE, PAYMENT_TERM_MONTHS,
    PRICING_RESULT_PRICED, PRICING_RESULT_UNABLE_TO_PRICE, QUOTE_PRICING_TYPE_SUBSCRIPTION,
    RENEWAL_PRICE_PER_VCPU_MONTH,
};

use crate::quote_lifecycle::{create_quote, request as quote_request};
use crate::site_and_outpost_lifecycle::{
    refusal, segment, settle, text, Outposts, Refusal, ABSENT_OUTPOST_ID, ABSENT_SITE_ID,
    OUTPOST_ARN_PREFIX,
};

/// A `SkuCode` of the modelled form (`^OR-[A-Z0-9]{7}$`) that this service publishes
/// no configuration under — a catalog item id a line item may name and the catalog
/// has no hardware facts for. The rows that use it assert it really is unpublished.
const UNPUBLISHED_SKU: &str = "OR-0000001";

/// How close a reported price has to be to the one the rule computes: the cent it is
/// quoted to, which is also the precision the service rounds to.
const CENT: f64 = 0.01;

/// The `OrderStatus` of a cancelled order, the one status in which an order no longer
/// subscribes the hardware it bought.
const ORDER_CANCELLED: &str = "CANCELLED";

/// The route `GetRenewalPricing` binds, for one identifier form.
fn route(identifier: &str) -> String {
    format!("/outpost/{}/renewal-pricing", segment(identifier))
}

/// One order a case places on its Outpost: the `(catalog item, quantity)` line items
/// it carries, the payment option it was bought under, and whether it was cancelled
/// after being placed.
#[derive(Debug, Clone)]
struct OrderSpec {
    items: Vec<(String, i64)>,
    payment_option: &'static str,
    cancelled: bool,
}

/// An order of `items`, live, bought under `payment_option` — which is the option
/// THAT order was bought under and so says nothing about which options a renewal of
/// it can be bought under.
fn order(payment_option: &'static str, items: &[(String, i64)]) -> OrderSpec {
    OrderSpec {
        items: items.to_vec(),
        payment_option,
        cancelled: false,
    }
}

/// The same order, cancelled after it was placed.
fn cancelled(mut spec: OrderSpec) -> OrderSpec {
    spec.cancelled = true;
    spec
}

/// The catalog of orderable hardware configurations this service publishes, as
/// `(CatalogItemId, vCPUs of EC2 capacity)` pairs.
///
/// The SKUs are the ones `ListCatalogItems` publishes over the wire. The capacity is
/// read through the store seam [`crate::quote_lifecycle`] ages a quote through,
/// because the modelled `CatalogItem` reports capacity per instance family rather
/// than one total — it is the catalog's own published figure either way, never a
/// number this suite picked.
async fn published_catalog(outposts: &Outposts) -> Vec<(String, i64)> {
    let listed = outposts
        .ok("GET", "/catalog/items?MaxResults=1000", None)
        .await;
    let items = listed["CatalogItems"]
        .as_array()
        .unwrap_or_else(|| panic!("expected CatalogItems, got {listed}"))
        .clone();
    let held = outposts.store().lock().catalog_items.clone();
    let mut catalog: Vec<(String, i64)> = items
        .iter()
        .map(|item| {
            let sku = text(item, "CatalogItemId");
            let capacity = held
                .iter()
                .find(|record| record.item.catalog_item_id.as_deref() == Some(sku.as_str()))
                .unwrap_or_else(|| panic!("the published catalog holds {sku}"))
                .capacity_vcpus;
            (sku, i64::from(capacity))
        })
        .collect();
    assert!(
        catalog.len() >= 2,
        "the published catalog has to offer more than one configuration for these rows to span \
         it, got {catalog:?}"
    );
    assert!(
        catalog.iter().all(|(sku, capacity)| *capacity > 0 && sku != UNPUBLISHED_SKU),
        "every published configuration carries capacity, and none of them is the unpublished \
         SKU {UNPUBLISHED_SKU}: got {catalog:?}"
    );
    catalog.sort_by_key(|(_, capacity)| *capacity);
    catalog
}

/// One site and one Outpost on it, as that Outpost's `OutpostId`. An Outpost belongs
/// to a site, so the site comes first — the same order a real workflow takes.
async fn an_outpost(outposts: &Outposts, name: &str) -> String {
    let site = outposts
        .create_site(&json!({ "Name": format!("{name}-site") }))
        .await;
    let outpost = outposts
        .create_outpost(&json!({ "Name": name, "SiteId": text(&site, "SiteId") }))
        .await;
    text(&outpost, "OutpostId")
}

/// Put a stored order in `CANCELLED` through the store seam
/// [`crate::quote_lifecycle`] ages a quote through: `CancelOrder` is still a 501
/// scaffold stub with an issue of its own, and a case about what a cancelled order no
/// longer subscribes must not wait on that.
fn cancel_order(outposts: &Outposts, order_id: &str, case: &str) {
    let store = outposts.store();
    let mut store = store.lock();
    let record = store
        .orders
        .get_mut(order_id)
        .unwrap_or_else(|| panic!("case {case:?}: the create persisted order {order_id}"));
    record.order.status = Some(ORDER_CANCELLED.to_string());
}

/// Let every order placed on `outpost_id` FINISH, through the same store seam as
/// [`cancel_order`]: an Outpost an order is still outstanding on cannot be deleted at
/// all (`crate::delete_conflicts`), and the clock does not carry an order to a
/// terminal status on its own. A finished order still subscribes the hardware it
/// bought, so what the row using this asserts is unchanged.
fn finish_orders_on(outposts: &Outposts, outpost_id: &str, case: &str) {
    let store = outposts.store();
    let mut store = store.lock();
    let mut finished = 0usize;
    for record in store.orders.values_mut() {
        if record.order.outpost_id.as_deref() == Some(outpost_id) {
            record.order.status = Some(ORDER_STATUS_COMPLETED.to_string());
            finished += 1;
        }
    }
    assert!(
        finished > 0,
        "case {case:?}: expected at least one order of {outpost_id} to finish, got {finished}"
    );
}

/// Place every order a case describes on `outpost_id`, over the wire.
async fn place_orders(outposts: &Outposts, outpost_id: &str, specs: &[OrderSpec], case: &str) {
    for spec in specs {
        let mut body = json!({
            "OutpostIdentifier": outpost_id,
            "PaymentOption": spec.payment_option,
        });
        if spec.items.is_empty() {
            // An order has to name either its line items or the quote to take them
            // from; one taken from a quote carries no line item of its own.
            let quote = create_quote(
                outposts,
                &quote_request(
                    "US",
                    json!([{ "QuoteCapacityType": "EC2", "Unit": "instances", "Quantity": 4.0 }]),
                    json!({ "OutpostIdentifier": outpost_id }),
                ),
            )
            .await;
            body["QuoteIdentifier"] = json!(text(&quote, "QuoteId"));
        } else {
            body["LineItems"] = Value::Array(
                spec.items
                    .iter()
                    .map(|(sku, quantity)| json!({ "CatalogItemId": sku, "Quantity": quantity }))
                    .collect(),
            );
        }
        let placed = outposts.ok("POST", "/orders", Some(&body)).await;
        let order_id = text(&placed["Order"], "OrderId");
        if spec.cancelled {
            cancel_order(outposts, &order_id, case);
        }
    }
}

/// The capacity, in vCPUs, the orders a case placed subscribe on its Outpost: every
/// live order's line items, each item's published capacity times the quantity
/// ordered. A catalog item the service publishes nothing for subscribes nothing.
fn subscribed_capacity(specs: &[OrderSpec], catalog: &[(String, i64)]) -> i64 {
    specs
        .iter()
        .filter(|spec| !spec.cancelled)
        .flat_map(|spec| spec.items.iter())
        .map(|(sku, quantity)| {
            let capacity = catalog
                .iter()
                .find(|(published, _)| published == sku)
                .map_or(0, |(_, capacity)| *capacity);
            capacity * quantity
        })
        .sum()
}

/// How many months a term runs for, from the rule the service prices by.
fn months(payment_term: &str) -> f64 {
    let (_, months) = PAYMENT_TERM_MONTHS
        .iter()
        .find(|(term, _)| *term == payment_term)
        .unwrap_or_else(|| panic!("the rule gives {payment_term} a length in months"));
    f64::from(*months)
}

/// The share of a term's cost a payment option pays up front, from the same rule.
fn upfront_share(payment_option: &str) -> f64 {
    let (_, share) = PAYMENT_OPTION_UPFRONT_SHARE
        .iter()
        .find(|(option, _)| *option == payment_option)
        .unwrap_or_else(|| panic!("the rule gives {payment_option} an upfront share"));
    f64::from(*share)
}

/// A price member of a reported option, as a number — `None` when the wire omits it.
fn price(details: &Value, member: &str, case: &str) -> Option<f64> {
    details.get(member).map(|value| {
        value
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: {member} {value} is not a number"))
    })
}

/// Require `actual` to be `expected` to the cent.
fn assert_price(actual: Option<f64>, expected: f64, member: &str, case: &str) {
    let actual =
        actual.unwrap_or_else(|| panic!("case {case:?}: expected {member} {expected}, got none"));
    assert!(
        (actual - expected).abs() <= CENT.max(expected.abs() * 1e-6),
        "case {case:?}: expected {member} {expected}, got {actual}"
    );
}

/// Rule 1, over one answer: every option the renewal of `capacity` vCPUs of
/// subscribed hardware can be bought under, priced by the service's own rule.
fn assert_priced(reply: &Value, capacity: i64, case: &str) {
    assert_eq!(
        text(reply, "PricingResult"),
        PRICING_RESULT_PRICED,
        "case {case:?}: {capacity} vCPUs are subscribed, so the renewal is priced, got {reply}"
    );
    let reported = reply
        .get("PricingOptions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: expected PricingOptions, got {reply}"));
    // Every (payment option, payment term) combination the model declares, in the
    // order the model declares the two value sets in.
    let expected: Vec<(&str, &str)> = PAYMENT_OPTION_VALUES
        .iter()
        .flat_map(|option| PAYMENT_TERM_VALUES.iter().map(move |term| (*option, *term)))
        .collect();
    assert_eq!(
        reported.len(),
        expected.len(),
        "case {case:?}: expected one option per payment option and term ({}), got {reply}",
        expected.len()
    );
    let monthly_rate = capacity as f64 * f64::from(RENEWAL_PRICE_PER_VCPU_MONTH);
    for (index, (actual, (payment_option, payment_term))) in
        reported.iter().zip(&expected).enumerate()
    {
        let case = &format!("{case} (option {index}: {payment_option} {payment_term})");
        assert_eq!(
            actual.get("PricingType").and_then(Value::as_str),
            Some(QUOTE_PRICING_TYPE_SUBSCRIPTION),
            "case {case:?}: expected PricingType {QUOTE_PRICING_TYPE_SUBSCRIPTION:?}, got {actual}"
        );
        let details = actual
            .get("SubscriptionPricingDetails")
            .unwrap_or_else(|| panic!("case {case:?}: expected the pricing details, got {actual}"));
        assert_eq!(
            details.get("PaymentOption").and_then(Value::as_str),
            Some(*payment_option),
            "case {case:?}: expected PaymentOption {payment_option:?}, got {details}"
        );
        assert_eq!(
            details.get("PaymentTerm").and_then(Value::as_str),
            Some(*payment_term),
            "case {case:?}: expected PaymentTerm {payment_term:?}, got {details}"
        );
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
        // pays over the whole term is the term's cost, under every payment option of
        // that term.
        let paid = price(details, "UpfrontPrice", case).unwrap_or_default()
            + price(details, "MonthlyRecurringPrice", case).unwrap_or_default() * months;
        assert!(
            (paid - term_cost).abs() <= CENT * months,
            "case {case:?}: upfront plus {months} monthly instalments must be the term's cost \
             {term_cost}, got {paid}"
        );
    }
}

/// Rule 2, over one answer: nothing is subscribed, so there is nothing to price.
fn assert_unable_to_price(reply: &Value, case: &str) {
    assert_eq!(
        text(reply, "PricingResult"),
        PRICING_RESULT_UNABLE_TO_PRICE,
        "case {case:?}: nothing is subscribed on this Outpost, so it cannot be priced, got {reply}"
    );
    assert_eq!(
        reply.get("PricingOptions"),
        Some(&json!([])),
        "case {case:?}: an Outpost with nothing subscribed reports no option, got {reply}"
    );
}

/// Rules 1 + 2 + 3: the pricing answers one option per payment option and term,
/// priced from the hardware the Outpost's orders subscribe to, through either
/// identifier form.
///
/// The rows differ along everything the rule generalises over: which configuration
/// was ordered (every one the catalog publishes, from the smallest to the largest),
/// how many units of it, how many line items an order carries, how many orders the
/// Outpost holds, whether an order was cancelled, whether it names a catalog item the
/// service publishes nothing for, whether it carries line items at all, and which
/// payment option it was itself bought under.
#[tokio::test]
async fn renewal_pricing_prices_the_subscription_the_outposts_orders_hold() {
    let outposts = Outposts::new();
    let catalog = published_catalog(&outposts).await;
    let (smallest, smallest_capacity) = catalog.first().cloned().expect("the catalog is not empty");
    let (largest, _) = catalog.last().cloned().expect("the catalog is not empty");

    let mut cases: Vec<(String, Vec<OrderSpec>)> = vec![
        ("no order at all".to_string(), Vec::new()),
        (
            "two units of the largest configuration".to_string(),
            vec![order("NO_UPFRONT", &[(largest.clone(), 2)])],
        ),
        (
            "two configurations in one order".to_string(),
            vec![order(
                "PARTIAL_UPFRONT",
                &[(smallest.clone(), 1), (largest.clone(), 3)],
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
            "an order of a catalog item the service publishes nothing for".to_string(),
            vec![order("ALL_UPFRONT", &[(UNPUBLISHED_SKU.to_string(), 4)])],
        ),
        (
            "a published configuration beside an unpublished SKU".to_string(),
            vec![order(
                "ALL_UPFRONT",
                &[
                    (smallest.clone(), 1),
                    (UNPUBLISHED_SKU.to_string(), 7),
                ],
            )],
        ),
        (
            "the only order was cancelled".to_string(),
            vec![cancelled(order("NO_UPFRONT", &[(largest.clone(), 2)]))],
        ),
        (
            "a cancelled order beside a live one".to_string(),
            vec![
                cancelled(order("NO_UPFRONT", &[(largest.clone(), 2)])),
                order("ALL_UPFRONT", &[(smallest.clone(), 1)]),
            ],
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

    for (case, specs) in &cases {
        let outpost_id = an_outpost(&outposts, "renewal-pricing").await;
        place_orders(&outposts, &outpost_id, specs, case).await;
        let capacity = subscribed_capacity(specs, &catalog);

        // Rule 3's identifier half: the bare id and the full Outpost ARN name the
        // same Outpost, so both answer the same pricing.
        for identifier in [
            outpost_id.clone(),
            format!("{OUTPOST_ARN_PREFIX}{outpost_id}"),
        ] {
            let case = &format!("{case} read by {identifier}");
            let reply = outposts.ok("GET", &route(&identifier), None).await;
            if capacity == 0 {
                assert_unable_to_price(&reply, case);
            } else {
                assert_priced(&reply, capacity, case);
            }
        }
    }

    // The smallest configuration is the one the rows above price one unit of, so the
    // catalog's own figure is what the pricing has to follow from.
    assert!(
        smallest_capacity > 0,
        "the smallest published configuration carries capacity, got {smallest_capacity}"
    );
}

/// Rule 1's scaling half: the price follows the subscribed capacity — twice the
/// hardware is twice the money, under every payment option and term — and a longer
/// term costs more over the term than a shorter one.
#[tokio::test]
async fn pricing_follows_the_subscribed_capacity_and_the_term() {
    let outposts = Outposts::new();
    let catalog = published_catalog(&outposts).await;
    let (sku, _) = catalog.last().cloned().expect("the catalog is not empty");

    let mut answers = Vec::new();
    for quantity in [1_i64, 2, 4] {
        let case = format!("{quantity} unit(s) of {sku}");
        let outpost_id = an_outpost(&outposts, "scaling").await;
        place_orders(
            &outposts,
            &outpost_id,
            &[order("ALL_UPFRONT", &[(sku.clone(), quantity)])],
            &case,
        )
        .await;
        let reply = outposts.ok("GET", &route(&outpost_id), None).await;
        answers.push((quantity, case, reply));
    }

    let (base_quantity, base_case, base) = &answers[0];
    for (quantity, case, reply) in &answers {
        let factor = *quantity as f64 / *base_quantity as f64;
        for member in ["UpfrontPrice", "MonthlyRecurringPrice"] {
            for index in 0..PAYMENT_OPTION_VALUES.len() * PAYMENT_TERM_VALUES.len() {
                let one = price(&base["PricingOptions"][index]["SubscriptionPricingDetails"], member, base_case)
                    .unwrap_or_default();
                let scaled =
                    price(&reply["PricingOptions"][index]["SubscriptionPricingDetails"], member, case)
                        .unwrap_or_default();
                assert!(
                    (scaled - one * factor).abs() <= CENT.max(one * factor * 1e-6),
                    "case {case:?} (option {index}): {quantity} units cost {factor} times what \
                     {base_quantity} does, so expected {member} {}, got {scaled}",
                    one * factor
                );
            }
        }
    }

    // The same subscription over a longer term costs more over that term: the
    // ordering the terms' own lengths put them in.
    let options = base["PricingOptions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {base_case:?}: expected PricingOptions, got {base}"));
    for payment_option in PAYMENT_OPTION_VALUES {
        let mut costs: Vec<(f64, f64)> = options
            .iter()
            .filter_map(|option| {
                let details = option.get("SubscriptionPricingDetails")?;
                if details.get("PaymentOption").and_then(Value::as_str) != Some(payment_option) {
                    return None;
                }
                let payment_term = details.get("PaymentTerm").and_then(Value::as_str)?;
                let months = months(payment_term);
                let paid = price(details, "UpfrontPrice", base_case).unwrap_or_default()
                    + price(details, "MonthlyRecurringPrice", base_case).unwrap_or_default()
                        * months;
                Some((months, paid))
            })
            .collect();
        costs.sort_by(|left, right| left.0.total_cmp(&right.0));
        assert_eq!(
            costs.len(),
            PAYMENT_TERM_VALUES.len(),
            "case {payment_option:?}: expected one option per payment term, got {costs:?}"
        );
        for pair in costs.windows(2) {
            assert!(
                pair[1].1 > pair[0].1,
                "case {payment_option:?}: a {} month term must cost more over the term than a {} \
                 month one, got {:?}",
                pair[1].0,
                pair[0].0,
                pair
            );
        }
    }
}

/// Rule 3's report half: asking for an Outpost's renewal pricing changes nothing —
/// not the pricing it answers, not the orders it priced, not the Outpost, not another
/// Outpost's pricing.
#[tokio::test]
async fn reading_renewal_pricing_changes_nothing_a_later_call_sees() {
    let outposts = Outposts::new();
    let catalog = published_catalog(&outposts).await;
    let (sku, _) = catalog.last().cloned().expect("the catalog is not empty");
    let priced = an_outpost(&outposts, "priced").await;
    let unpriced = an_outpost(&outposts, "unpriced").await;
    place_orders(
        &outposts,
        &priced,
        &[order("PARTIAL_UPFRONT", &[(sku.clone(), 2)])],
        "priced",
    )
    .await;

    // Past the Outpost lifecycle transition the clock owes a just-created Outpost and
    // the order transition it owes a just-placed order, so the snapshots below differ
    // only if THIS read changed one.
    settle();
    let before = outposts.get_outpost(&priced).await;
    let orders_before = outposts.store().lock().orders.clone();

    let mut answers = Vec::new();
    for pass in ["first read", "second read", "third read"] {
        answers.push((pass, outposts.ok("GET", &route(&priced), None).await));
        // Another Outpost's pricing is its own: reading one never prices another.
        let other = outposts.ok("GET", &route(&unpriced), None).await;
        assert_unable_to_price(&other, pass);
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
    let orders_after = outposts.store().lock().orders.clone();
    assert_eq!(
        format!("{orders_after:?}"),
        format!("{orders_before:?}"),
        "reading renewal pricing must leave the orders it priced alone"
    );
}

/// Rule 4: an identifier that names no Outpost is 404 `NotFoundException`; one that
/// is no Outpost identifier at all is 400 `ValidationException`, from the boundary.
///
/// The rows span both identifier forms, an identifier of another resource kind, and
/// the ways an Outpost id can be malformed (wrong prefix, non-hex suffix, uppercase
/// hex, one character short, one character long).
#[tokio::test]
async fn an_absent_outpost_is_not_found_and_a_malformed_identifier_is_refused() {
    let outposts = Outposts::new();
    // An Outpost that existed, had hardware ordered for it, and was then deleted: the
    // route answers as it does for one that never was.
    let catalog = published_catalog(&outposts).await;
    let (sku, _) = catalog.first().cloned().expect("the catalog is not empty");
    let deleted = an_outpost(&outposts, "deleted").await;
    place_orders(
        &outposts,
        &deleted,
        &[order("ALL_UPFRONT", &[(sku, 1)])],
        "deleted",
    )
    .await;
    // The hardware arrives and the order is complete, so nothing is outstanding on the
    // Outpost any more and the delete goes through; the finished order stays behind,
    // still subscribing the capacity it bought, which is what makes the row below a
    // statement about the deleted Outpost rather than about an empty store.
    finish_orders_on(&outposts, &deleted, "deleted");
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

/// Rule 4's message half: a not-found refusal is the identifier ECHO
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
