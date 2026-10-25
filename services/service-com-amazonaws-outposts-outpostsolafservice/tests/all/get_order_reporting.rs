// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetOrder` as a SERVED, REPORTING operation, driven over the
//! wire.
//!
//! Every case here goes through the real front door — `MultiServiceRouter::dispatch`
//! with a restJson1 request, exactly as a signed `aws outposts` call arrives — so what
//! it asserts is the emulator's HTTP behaviour, not a handler called directly. The
//! request harness is the lifecycle suite's, reused rather than rebuilt.
//!
//! What [`crate::order_lifecycle`] states is WHICH order a submit persists and what a
//! read answers of it. This suite states the two properties that are about the read
//! itself, and pins them over a set of inputs:
//!
//! 1. The operation is SERVED: every call to it — accepted or refused — is answered
//!    from the model's own outcomes (200, or the modelled 404 / 400), never with the
//!    unimplemented-operation channel's HTTP 501 `NotImplemented`, and a served call
//!    carries the operation's own output member.
//! 2. The operation only REPORTS: the spec has it get information about the specified
//!    order, so the ONLY thing a call to it may write is the status transition the
//!    clock had already made due at that same instant. Beyond that, no call changes
//!    anything a later caller observes: not the order it answers, not any other
//!    order's record, not the quote the order was taken from, not the Outpost it was
//!    placed on. A transition not yet due is not brought forward by being read, and
//!    one already materialised is not owed a second time.
//!
//! The rows differ along the shape of the order read (one line item, several, and an
//! order with no line items of its own), where its clock-owed transition stands when it
//! is read (not yet due, due, already materialised), how many times it is read, and —
//! for rule 1 — the outcome the model declares for the request (served, 404, 400).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::state::{
    LINE_ITEM_STATUS_BUILDING, LINE_ITEM_STATUS_PREPARING, ORDER_STATUS_IN_PROGRESS,
    ORDER_STATUS_PREPARING,
};

use crate::quote_lifecycle::{create_quote, get_quote, request as quote_request};
use crate::site_and_outpost_lifecycle::{segment, settle, text, Outposts};

/// The HTTP status the unimplemented-operation channel answers with, and the wire
/// error code it carries. No row below may ever see these: an operation that is served
/// answers from the model's own outcomes.
const NOT_IMPLEMENTED_STATUS: u16 = 501;
/// See [`NOT_IMPLEMENTED_STATUS`].
const NOT_IMPLEMENTED_CODE: &str = "NotImplemented";

/// An order id of the modelled `OrderId` form (`^oo-[a-f0-9]{17}$`) that this emulator
/// never mints, for the row that needs an identifier the boundary accepts and the store
/// holds nothing for.
const ORDER_ID_NOTHING_HOLDS: &str = "oo-00000000000000000";

/// Catalog item SKUs of the modelled `SkuCode` form (`^OR-[A-Z0-9]{7}$`), which is all
/// this API says about a catalog item id a line item may name.
///
/// Source: `smithy_query(query="shape", shape="SkuCode")` against this service's model.
const SKUS: [&str; 3] = ["OR-1A2B3C4", "OR-ABCDEFG", "OR-0000001"];

/// How far out a row that wants an order's transition NOT yet due puts it: far enough
/// that no part of this suite can reach it, so "not due" is a property of the row and
/// not a race with how long the suite takes to run.
const FAR_OUT: Duration = Duration::from_secs(3600);

/// A `CreateOrder` body: the two members the model marks `@required`, with `overrides`
/// merged in.
fn order_body(outpost_identifier: &str, payment_option: &str, overrides: Value) -> Value {
    let mut body = json!({
        "OutpostIdentifier": outpost_identifier,
        "PaymentOption": payment_option,
    });
    if let (Some(target), Some(extra)) = (body.as_object_mut(), overrides.as_object()) {
        for (key, value) in extra {
            target.insert(key.clone(), value.clone());
        }
    }
    body
}

/// One line item request: a catalog item SKU and a quantity.
fn line_item(sku: &str, quantity: i64) -> Value {
    json!({ "CatalogItemId": sku, "Quantity": quantity })
}

/// `CreateOrder`, returning the `Order` it answered with.
async fn create_order(outposts: &Outposts, body: &Value) -> Value {
    outposts.ok("POST", "/orders", Some(body)).await["Order"].clone()
}

/// `GetOrder`, returning the `Order` it answered with.
async fn get_order(outposts: &Outposts, order_id: &str) -> Value {
    outposts
        .ok("GET", &format!("/orders/{}", segment(order_id)), None)
        .await["Order"]
        .clone()
}

/// The same order, as it reads once the transition the clock owed it has been
/// materialised: the order in progress and every line item being built.
fn settled(order: &Value) -> Value {
    let mut settled = order.clone();
    settled["Status"] = json!(ORDER_STATUS_IN_PROGRESS);
    if let Some(items) = settled.get_mut("LineItems").and_then(Value::as_array_mut) {
        for item in items {
            item["Status"] = json!(LINE_ITEM_STATUS_BUILDING);
        }
    }
    settled
}

/// Where a stored order's clock-owed transition stands when a row reads the order.
#[derive(Clone, Copy, Debug)]
enum Clock {
    /// The transition falls due further out than this suite runs for.
    NotYetDue,
    /// The transition was already due when the read arrived.
    AlreadyDue,
    /// The transition has already been materialised, so nothing is owed.
    Materialised,
}

impl Clock {
    /// The `Status` an order in this state reports, and the item-level status its line
    /// items report with it.
    fn expected_statuses(self) -> (&'static str, &'static str) {
        match self {
            Clock::NotYetDue => (ORDER_STATUS_PREPARING, LINE_ITEM_STATUS_PREPARING),
            Clock::AlreadyDue | Clock::Materialised => {
                (ORDER_STATUS_IN_PROGRESS, LINE_ITEM_STATUS_BUILDING)
            }
        }
    }
}

/// Put `order_id`'s clock-owed transition where `clock` says, through the store seam
/// the suites that age a stored resource use — the due instant is the service's own, so
/// a case that wants it in the past or out of reach moves the instant rather than
/// sleeping for it.
fn arrange(outposts: &Outposts, case: &str, order_id: &str, clock: Clock) {
    let store = outposts.store();
    let mut store = store.lock();
    let record = store
        .orders
        .get_mut(order_id)
        .unwrap_or_else(|| panic!("case {case:?}: the create persisted order {order_id}"));
    match clock {
        Clock::NotYetDue => record.in_progress_at = Some(Instant::now() + FAR_OUT),
        Clock::AlreadyDue => record.in_progress_at = Some(Instant::now()),
        Clock::Materialised => assert!(
            record.in_progress_at.is_none(),
            "case {case:?}: this row reads an order whose transition is already \
             materialised, got {:?}",
            record.in_progress_at
        ),
    }
}

/// Whether `order_id`'s transition is still owed in the store.
fn transition_is_owed(outposts: &Outposts, order_id: &str) -> bool {
    let store = outposts.store();
    let store = store.lock();
    store
        .orders
        .get(order_id)
        .is_some_and(|record| record.in_progress_at.is_some())
}

/// Every stored order record rendered as text, keyed by its `OrderId`: the value rule 2
/// compares before and after a read, so a read that quietly moved stored state shows up
/// here whether or not any wire response would have revealed it.
fn records(outposts: &Outposts) -> BTreeMap<String, String> {
    let store = outposts.store();
    let store = store.lock();
    store
        .orders
        .iter()
        .map(|(order_id, record)| (order_id.clone(), format!("{record:?}")))
        .collect()
}

/// Rule 1: every `GetOrder` call is answered from the model's own outcomes, so the
/// operation is no longer the 501 scaffold — over a table spanning the served rows (an
/// order of each shape this slice of the API can make) and the refused rows the model
/// declares (a well-formed order id nothing holds, and values that are no order
/// identifier at all).
#[tokio::test]
async fn every_get_order_call_is_served_from_the_modelled_outcomes() {
    let outposts = Outposts::new();
    let (outpost_id, _) = outposts.site_and_outpost("order-served").await;
    let quote_id = text(&a_quote(&outposts, &outpost_id).await, "QuoteId");

    let one_item = create_order(
        &outposts,
        &order_body(
            &outpost_id,
            "ALL_UPFRONT",
            json!({ "LineItems": [line_item(SKUS[0], 1)] }),
        ),
    )
    .await;
    let several_items = create_order(
        &outposts,
        &order_body(
            &outpost_id,
            "NO_UPFRONT",
            json!({
                "LineItems": [
                    line_item(SKUS[1], 2),
                    line_item(SKUS[2], 40),
                ],
                "PaymentTerm": "THREE_YEARS",
            }),
        ),
    )
    .await;
    let from_quote = create_order(
        &outposts,
        &order_body(
            &outpost_id,
            "PARTIAL_UPFRONT",
            json!({ "QuoteIdentifier": quote_id }),
        ),
    )
    .await;

    let mut cases: Vec<(String, String, u16)> = Vec::new();
    for (shape, order) in [
        ("one line item", &one_item),
        ("several line items", &several_items),
        ("no line items of its own", &from_quote),
    ] {
        cases.push((
            format!("an order with {shape}"),
            format!("/orders/{}", segment(&text(order, "OrderId"))),
            200,
        ));
    }
    cases.push((
        "an order id nothing holds".to_string(),
        format!("/orders/{ORDER_ID_NOTHING_HOLDS}"),
        404,
    ));
    for (shape, label) in [
        ("no identifier at all", "not-an-order-id"),
        ("another resource's id prefix", "oq-abcdef01234567890"),
        ("a suffix that is not hex", "oo-zzzzzzzzzzzzzzzzz"),
        ("a suffix one character short", "oo-0000000000000000"),
    ] {
        cases.push((
            format!("an order label that is {shape}"),
            format!("/orders/{}", segment(label)),
            400,
        ));
    }

    for (case, path, expected) in &cases {
        let reply = outposts.call("GET", path, None).await;
        assert_ne!(
            reply.status, NOT_IMPLEMENTED_STATUS,
            "case {case:?}: GetOrder must be served, not answered with the \
             unimplemented-operation status, got {} {:?}",
            reply.status, reply.body
        );
        assert_ne!(
            reply.error_code, NOT_IMPLEMENTED_CODE,
            "case {case:?}: GetOrder must be served, not answered with the \
             unimplemented-operation error code, got {} {:?}",
            reply.status, reply.body
        );
        assert_ne!(
            reply.errortype_header, NOT_IMPLEMENTED_CODE,
            "case {case:?}: GetOrder must be served, not answered with the \
             unimplemented-operation error code, got {:?}",
            reply.errortype_header
        );
        assert_eq!(
            reply.status, *expected,
            "case {case:?}: expected HTTP {expected}, got {} {:?}",
            reply.status, reply.body
        );
    }

    // A served read answers the operation's own output member — the order the id names,
    // and no other — which the 501 channel has none of.
    for (shape, order) in [
        ("one line item", &one_item),
        ("several line items", &several_items),
        ("no line items of its own", &from_quote),
    ] {
        let order_id = text(order, "OrderId");
        let read = get_order(&outposts, &order_id).await;
        assert_eq!(
            text(&read, "OrderId"),
            order_id,
            "case {shape:?}: a served read answers the order its id names, got {read}"
        );
    }
}

/// Rule 2: reading an order reports the record as the store holds it at that instant
/// and writes nothing but the transition the clock already owed.
///
/// Every order shape is taken through all three positions of its clock-owed transition,
/// read twice in each, with the whole store's order records and the other resources the
/// order is attached to compared across the reads.
#[tokio::test]
async fn reading_an_order_reports_the_stored_record_and_changes_nothing_else() {
    let outposts = Outposts::new();
    let (outpost_id, _) = outposts.site_and_outpost("order-reporting").await;
    // Past the Outpost's own clock-owed transition before any row runs, so the Outpost
    // this suite compares before and after a read is already in its terminal status and
    // cannot move under the comparison.
    settle();
    let quote_id = text(&a_quote(&outposts, &outpost_id).await, "QuoteId");

    let created: Vec<(&str, Value)> = vec![
        (
            "one line item",
            create_order(
                &outposts,
                &order_body(
                    &outpost_id,
                    "ALL_UPFRONT",
                    json!({ "LineItems": [line_item(SKUS[0], 1)] }),
                ),
            )
            .await,
        ),
        (
            "three line items",
            create_order(
                &outposts,
                &order_body(
                    &outpost_id,
                    "NO_UPFRONT",
                    json!({
                        "LineItems": [
                            line_item(SKUS[0], 1),
                            line_item(SKUS[1], 7),
                            line_item(SKUS[2], 500),
                        ],
                        "PaymentTerm": "FIVE_YEARS",
                    }),
                ),
            )
            .await,
        ),
        (
            "no line items of its own",
            create_order(
                &outposts,
                &order_body(
                    &outpost_id,
                    "PARTIAL_UPFRONT",
                    json!({ "QuoteIdentifier": quote_id.clone() }),
                ),
            )
            .await,
        ),
    ];
    // Hold every order's transition out of reach up front, so each row decides where
    // its OWN order's transition stands and no other order settles underneath it.
    for (shape, order) in &created {
        arrange(&outposts, shape, &text(order, "OrderId"), Clock::NotYetDue);
    }

    for (shape, order) in &created {
        let order_id = text(order, "OrderId");
        // The three positions in order: a transition that is not due yet, the same one
        // once it is, and the order after it has been materialised.
        for clock in [Clock::NotYetDue, Clock::AlreadyDue, Clock::Materialised] {
            let case = format!("{shape}, transition {clock:?}");
            arrange(&outposts, &case, &order_id, clock);
            let (expected_status, expected_item_status) = clock.expected_statuses();
            let expected = match clock {
                Clock::NotYetDue => order.clone(),
                Clock::AlreadyDue | Clock::Materialised => settled(order),
            };

            let records_before = records(&outposts);
            let quote_before = get_quote(&outposts, &quote_id).await;
            let outpost_before = outposts.get_outpost(&outpost_id).await;

            for pass in ["first read", "second read"] {
                let read = get_order(&outposts, &order_id).await;
                assert_eq!(
                    read, expected,
                    "case {case:?} ({pass}): GetOrder must answer the stored order"
                );
                assert_eq!(
                    text(&read, "Status"),
                    expected_status,
                    "case {case:?} ({pass}): expected Status {expected_status}, got {read}"
                );
                for (index, item) in read
                    .get("LineItems")
                    .and_then(Value::as_array)
                    .map(Vec::as_slice)
                    .unwrap_or_default()
                    .iter()
                    .enumerate()
                {
                    assert_eq!(
                        text(item, "Status"),
                        expected_item_status,
                        "case {case:?} ({pass}): expected line item {index} in \
                         {expected_item_status}, got {item}"
                    );
                }
            }

            // Every OTHER order's record is exactly as it was: a read reports one order
            // and settles nothing on anybody else's behalf.
            let records_after = records(&outposts);
            assert_eq!(
                records_after.keys().collect::<Vec<_>>(),
                records_before.keys().collect::<Vec<_>>(),
                "case {case:?}: reading an order must neither add nor remove a stored \
                 order"
            );
            for (other, before) in &records_before {
                if other == &order_id {
                    continue;
                }
                let after = records_after.get(other);
                assert_eq!(
                    after,
                    Some(before),
                    "case {case:?}: reading one order must leave order {other}'s record \
                     as it was, got {after:?}"
                );
            }

            match clock {
                // Nothing was owed, so nothing was written: the read did not bring the
                // transition forward.
                Clock::NotYetDue => {
                    let after = records_after.get(&order_id);
                    assert_eq!(
                        after,
                        records_before.get(&order_id),
                        "case {case:?}: a read before the transition falls due must \
                         leave the record untouched, got {after:?}"
                    );
                    assert!(
                        transition_is_owed(&outposts, &order_id),
                        "case {case:?}: the transition is still owed after a read that \
                         arrived before it fell due"
                    );
                }
                // What was owed was materialised, and only once: it is no longer owed,
                // and a further read writes nothing more.
                Clock::AlreadyDue | Clock::Materialised => {
                    assert!(
                        !transition_is_owed(&outposts, &order_id),
                        "case {case:?}: a materialised transition must not be owed a \
                         second time"
                    );
                    get_order(&outposts, &order_id).await;
                    let after = records(&outposts);
                    assert_eq!(
                        after, records_after,
                        "case {case:?}: a further read of a settled order must write \
                         nothing"
                    );
                }
            }

            // And nothing the order is attached to moved either.
            let quote_after = get_quote(&outposts, &quote_id).await;
            assert_eq!(
                quote_after, quote_before,
                "case {case:?}: reading an order must leave the quote it was taken from \
                 as it was, got {quote_after}"
            );
            let outpost_after = outposts.get_outpost(&outpost_id).await;
            assert_eq!(
                outpost_after, outpost_before,
                "case {case:?}: reading an order must leave the Outpost it was placed \
                 on as it was, got {outpost_after}"
            );
        }
    }
}

/// One quote of `outpost_id` an order can be submitted from.
async fn a_quote(outposts: &Outposts, outpost_id: &str) -> Value {
    create_quote(
        outposts,
        &quote_request(
            "US",
            json!([{ "QuoteCapacityType": "EC2", "Unit": "instances", "Quantity": 4.0 }]),
            json!({ "OutpostIdentifier": outpost_id }),
        ),
    )
    .await
}
