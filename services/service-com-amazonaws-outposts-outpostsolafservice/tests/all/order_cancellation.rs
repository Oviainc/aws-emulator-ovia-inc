// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CancelOrder`, driven over the wire through the same front
//! door as [`crate::site_and_outpost_lifecycle`] (whose harness this suite reuses),
//! placing its orders through the route [`crate::order_lifecycle`] states what a
//! submit persists on, and its quotes through [`crate::outpost_decommission`]'s
//! helper.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. Cancelling an order settles it: the order moves to the `CANCELLED` status and
//!    every line item of it to the item-level status of the same name, whatever
//!    status the clock had brought the order to, and nothing else about the stored
//!    order changes. The transition the clock still owed the order is dropped with
//!    it, so waiting past that span leaves a cancelled order cancelled, and an order
//!    nobody cancelled is untouched by another order's cancellation. The quote an
//!    order was submitted from keeps saying so — an order WAS submitted for it.
//! 2. A cancelled order is no longer OUTSTANDING on its Outpost, so the work it was
//!    holding up goes ahead: the Outpost it was placed on can be decommissioned and
//!    deleted, whether the order was placed directly or taken from a quote.
//! 3. An order that has already finished has nothing left to cancel: 409
//!    `ConflictException`, naming the order in `ResourceId` / `ResourceType` and the
//!    status it is in, and the order is exactly as it was.
//! 4. An `OrderId` this region's inventory holds no order for is 404
//!    `NotFoundException` with the identifier echo of the value as supplied, and
//!    nothing is cancelled — including an order of ANOTHER region, which this
//!    region's endpoint does not serve.

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::state::{
    identifier_not_found, order_status_conflict, DECOMMISSION_STATUS_BLOCKED,
    DECOMMISSION_STATUS_REQUESTED, LINE_ITEM_STATUS_BUILDING, LINE_ITEM_STATUS_CANCELLED,
    LINE_ITEM_STATUS_PREPARING, ORDER_ID_MEMBER, ORDER_PREPARATION, ORDER_STATUS_CANCELLED,
    ORDER_STATUS_COMPLETED, ORDER_STATUS_IN_PROGRESS, ORDER_STATUS_PREPARING, ORDER_STATUS_SETTLED,
    QUOTE_STATUS_ORDER_SUBMITTED, RESOURCE_TYPE_ORDER,
};

use crate::outpost_decommission::{a_quote, decommission};
use crate::quote_lifecycle::get_quote;
use crate::site_and_outpost_lifecycle::{segment, text, Outposts, Reply};

/// Catalog item SKUs of the modelled `SkuCode` form (`^OR-[A-Z0-9]{7}$`), which is the
/// only thing this API says about a catalog item id a line item may name.
const SKUS: [&str; 4] = ["OR-1A2B3C4", "OR-ABCDEFG", "OR-0000001", "OR-ZZZZZZ9"];

/// An `OrderId` of the modelled form `^oo-[a-f0-9]{17}$` that this emulator never
/// mints: well formed, so the boundary accepts it, and naming nothing.
const ABSENT_ORDER_ID: &str = "oo-00000000000000000";

/// A region other than the harness's home one, used for the order that exists but is
/// not part of the inventory the call is addressed to.
const OTHER_REGION: &str = "us-west-2";

/// The wire error code a conflict arrives as, from
/// `smithy_query(query="operation",operation="CancelOrder")`: `ConflictException`,
/// HTTP 409.
const CONFLICT_CODE: &str = "ConflictException";

/// The wire error code an identifier that names nothing arrives as. See
/// [`CONFLICT_CODE`].
const NOT_FOUND_CODE: &str = "NotFoundException";

/// The cancel route of one order: `CancelOrder` binds its only member to the URI
/// (`POST /orders/{OrderId}/cancel`), so the request carries no payload of its own.
fn cancel_route(order_id: &str) -> String {
    format!("/orders/{}/cancel", segment(order_id))
}

/// `CancelOrder` on `order_id` in `region`, as the reply arrived.
async fn cancel_in(outposts: &Outposts, region: &str, order_id: &str) -> Reply {
    outposts
        .call_in(region, "POST", &cancel_route(order_id), Some(&json!({})))
        .await
}

/// `CancelOrder` on `order_id` in the home region. See [`cancel_in`].
async fn cancel(outposts: &Outposts, order_id: &str) -> Reply {
    outposts
        .call("POST", &cancel_route(order_id), Some(&json!({})))
        .await
}

/// One line item request: a catalog item SKU and a quantity.
fn line_item(sku: &str, quantity: i64) -> Value {
    json!({ "CatalogItemId": sku, "Quantity": quantity })
}

/// A `CreateOrder` body: the two members the model marks `@required`, with
/// `overrides` merged in.
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

/// `CreateOrder`, returning the `OrderId` it minted.
async fn place_order(outposts: &Outposts, body: &Value) -> String {
    let answered = outposts.ok("POST", "/orders", Some(body)).await;
    text(&answered["Order"], "OrderId")
}

/// `GetOrder`, returning the `Order` it answered with — the stored record every
/// assertion about what a cancellation left behind is made against.
async fn get_order(outposts: &Outposts, order_id: &str) -> Value {
    outposts
        .ok("GET", &format!("/orders/{}", segment(order_id)), None)
        .await["Order"]
        .clone()
}

/// Wait until an order's clock-owed `PREPARING` -> `IN_PROGRESS` transition has fallen
/// due.
///
/// The service materialises the transition at the instant it stored, so a case that
/// wants it behind the order waits past that span plus a margin. Blocking with
/// `std::thread::sleep` for the reason [`crate::site_and_outpost_lifecycle::settle`]
/// does: the workspace's `tokio` carries no timer driver.
fn settle_orders() {
    std::thread::sleep(ORDER_PREPARATION + std::time::Duration::from_millis(400));
}

/// The same order, as it must read once it has been cancelled: the order `CANCELLED`
/// and every line item of it in the item-level status of the same name, with every
/// other member exactly as it was.
fn cancelled(order: &Value) -> Value {
    let mut cancelled = order.clone();
    cancelled["Status"] = json!(ORDER_STATUS_CANCELLED);
    if let Some(items) = cancelled.get_mut("LineItems").and_then(Value::as_array_mut) {
        for item in items {
            item["Status"] = json!(LINE_ITEM_STATUS_CANCELLED);
        }
    }
    cancelled
}

/// Every line item an order reports, in the order it reports them.
fn line_items(order: &Value) -> Vec<Value> {
    order
        .get("LineItems")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Put a stored order in a status this API has no call that reaches.
///
/// `COMPLETED` is reached by hardware arriving at the site and being installed, which
/// leaves a record this emulator does not have, so a case about what a FINISHED order
/// refuses reaches it through the same store seam [`crate::outpost_decommission`] ages
/// a quote through. The clock-owed transition goes with it, because an order that has
/// finished is not owed one. Every assertion still goes over the wire.
fn finish_order(outposts: &Outposts, order_id: &str, status: &str, case: &str) {
    let store = outposts.store();
    let mut store = store.lock();
    let record = store
        .orders
        .get_mut(order_id)
        .unwrap_or_else(|| panic!("case {case:?}: the order {order_id} this case placed is stored"));
    record.order.status = Some(status.to_string());
    record.in_progress_at = None;
}

/// Rule 1: a cancellation settles the order and every line item of it as cancelled,
/// changes nothing else about the stored order, and is not undone by the clock.
///
/// The rows differ along everything the rule generalises over: how many line items the
/// order has (one, three, the modelled maximum of twenty, and none of its own),
/// whether it was taken from a quote, how the Outpost was named (bare id, ARN), which
/// payment option and term it carries, and the status the clock had brought the order
/// to when it was cancelled (still being prepared, already being built).
#[tokio::test]
async fn cancelling_an_order_settles_it_and_its_line_items_as_cancelled() {
    let outposts = Outposts::new();
    let (outpost_id, outpost_arn) = outposts.site_and_outpost("cancel-order").await;
    // One quote per row that takes one: a quote takes exactly one order.
    let quote_alone = a_quote(&outposts, Some(&outpost_id)).await;
    let quote_with_items = a_quote(&outposts, Some(&outpost_id)).await;
    let twenty: Vec<Value> = (0..20)
        .map(|index| line_item(SKUS[index % SKUS.len()], (index as i64) + 1))
        .collect();
    // An order no case here cancels: what every cancellation must leave alone.
    let keeper = place_order(
        &outposts,
        &order_body(
            &outpost_id,
            "ALL_UPFRONT",
            json!({ "LineItems": [line_item(SKUS[0], 1)] }),
        ),
    )
    .await;

    // (case, the order to place, whether the clock has moved it on before the
    //  cancellation, the quote it was taken from)
    let rows: Vec<(&str, Value, bool, Option<String>)> = vec![
        (
            "one line item, cancelled while it is still being prepared",
            order_body(
                &outpost_id,
                "ALL_UPFRONT",
                json!({ "LineItems": [line_item(SKUS[0], 1)] }),
            ),
            false,
            None,
        ),
        (
            "three line items and a payment term, the Outpost named by ARN, cancelled \
             once the clock has moved the order on",
            order_body(
                &outpost_arn,
                "NO_UPFRONT",
                json!({
                    "LineItems": [
                        line_item(SKUS[1], 1),
                        line_item(SKUS[2], 2),
                        line_item(SKUS[3], 500),
                    ],
                    "PaymentTerm": "ONE_YEAR",
                }),
            ),
            true,
            None,
        ),
        (
            "the modelled maximum of twenty line items",
            order_body(
                &outpost_id,
                "PARTIAL_UPFRONT",
                json!({ "LineItems": twenty, "PaymentTerm": "THREE_YEARS" }),
            ),
            false,
            None,
        ),
        (
            "an order taken from a quote, with no line items of its own",
            order_body(
                &outpost_id,
                "NO_UPFRONT",
                json!({ "QuoteIdentifier": quote_alone.clone() }),
            ),
            false,
            Some(quote_alone.clone()),
        ),
        (
            "an order taken from a quote, with line items of its own, cancelled once \
             the clock has moved it on",
            order_body(
                &outpost_id,
                "PARTIAL_UPFRONT",
                json!({
                    "QuoteIdentifier": quote_with_items.clone(),
                    "LineItems": [line_item(SKUS[0], 7), line_item(SKUS[1], 7)],
                }),
            ),
            true,
            Some(quote_with_items.clone()),
        ),
    ];

    let mut cancelled_orders: Vec<(&str, String, Value)> = Vec::new();
    for (case, body, moved_on, quote_id) in &rows {
        let order_id = place_order(&outposts, body).await;
        if *moved_on {
            settle_orders();
        }
        // The order as it reads at the instant it is cancelled: the status the clock
        // has brought it to is what the cancellation moves it out of.
        let before = get_order(&outposts, &order_id).await;
        let (order_status, item_status) = match *moved_on {
            true => (ORDER_STATUS_IN_PROGRESS, LINE_ITEM_STATUS_BUILDING),
            false => (ORDER_STATUS_PREPARING, LINE_ITEM_STATUS_PREPARING),
        };
        assert_eq!(
            text(&before, "Status"),
            order_status,
            "case {case:?}: expected the order to be {order_status} before it is \
             cancelled, got {before}"
        );
        for item in line_items(&before) {
            assert_eq!(
                text(&item, "Status"),
                item_status,
                "case {case:?}: expected every line item to be {item_status} before the \
                 order is cancelled, got {before}"
            );
        }

        let reply = cancel(&outposts, &order_id).await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected HTTP 200, got {} {}",
            reply.status,
            reply.body
        );
        assert!(
            reply
                .body
                .as_object()
                .is_none_or(|members| members.is_empty()),
            "case {case:?}: CancelOrder models no output members, got {}",
            reply.body
        );

        let expected = cancelled(&before);
        let after = get_order(&outposts, &order_id).await;
        assert_eq!(
            after, expected,
            "case {case:?}: expected the stored order with the order and every line item \
             {ORDER_STATUS_CANCELLED} and nothing else changed, got {after}"
        );
        if let Some(quote_id) = quote_id {
            // An order WAS submitted from the quote, which is what its status says, so
            // cancelling that order does not take the quote back.
            let quote = get_quote(&outposts, quote_id).await;
            assert_eq!(
                text(&quote, "QuoteStatus"),
                QUOTE_STATUS_ORDER_SUBMITTED,
                "case {case:?}: expected the quote to stay {QUOTE_STATUS_ORDER_SUBMITTED}, \
                 got {quote}"
            );
            assert_eq!(
                text(&quote, "SubmittedOrderId"),
                order_id,
                "case {case:?}: expected the quote to still name order {order_id:?}, got \
                 {quote}"
            );
        }
        cancelled_orders.push((case, order_id, expected));
    }

    // The clock owes a cancelled order nothing: the transition the submit started is
    // dropped with the cancellation, so waiting past that span leaves every cancelled
    // order exactly as the cancellation left it.
    settle_orders();
    for (case, order_id, expected) in &cancelled_orders {
        let read = get_order(&outposts, order_id).await;
        assert_eq!(
            &read, expected,
            "case {case:?}: a cancelled order is not moved on by the clock, got {read}"
        );
    }
    // And an order nobody cancelled is untouched by all of it: it moved on exactly as
    // it would have.
    let keeper_now = get_order(&outposts, &keeper).await;
    assert_eq!(
        text(&keeper_now, "Status"),
        ORDER_STATUS_IN_PROGRESS,
        "cancelling other orders leaves the order {keeper:?} nobody cancelled on its own \
         course, got {keeper_now}"
    );
    for item in line_items(&keeper_now) {
        assert_eq!(
            text(&item, "Status"),
            LINE_ITEM_STATUS_BUILDING,
            "cancelling other orders leaves the line items of the order nobody cancelled \
             on their own course, got {keeper_now}"
        );
    }
}

/// How the order of a row was placed: directly against the Outpost, or taken from a
/// quote of it — the two records this store keeps of an order outstanding on an
/// Outpost.
#[derive(Clone, Copy, Debug)]
enum Placed {
    Directly,
    FromQuote,
}

/// What the order outstanding on the Outpost was holding up, and which a cancellation
/// has to let through.
#[derive(Clone, Copy, Debug)]
enum HeldUp {
    Decommission,
    Delete,
}

/// Rule 2: a cancelled order is no longer outstanding on its Outpost, so what it was
/// holding up goes ahead.
///
/// The rows differ along how the order was placed (directly, or taken from a quote of
/// the Outpost) and which call the outstanding order was holding up (the Outpost's
/// decommission, its delete).
#[tokio::test]
async fn a_cancelled_order_no_longer_holds_up_its_outpost() {
    let rows: Vec<(&str, Placed, HeldUp)> = vec![
        (
            "an order placed directly blocks the decommission until it is cancelled",
            Placed::Directly,
            HeldUp::Decommission,
        ),
        (
            "an order taken from a quote blocks the decommission until it is cancelled",
            Placed::FromQuote,
            HeldUp::Decommission,
        ),
        (
            "an order placed directly blocks the delete until it is cancelled",
            Placed::Directly,
            HeldUp::Delete,
        ),
        (
            "an order taken from a quote blocks the delete until it is cancelled",
            Placed::FromQuote,
            HeldUp::Delete,
        ),
    ];

    for (case, placed, held_up) in &rows {
        // A store of its own per row: what holds a call up is what the store holds, so
        // each row starts from an empty one.
        let outposts = Outposts::new();
        let (outpost_id, _) = outposts.site_and_outpost("held-up-outpost").await;
        let contents = match placed {
            Placed::Directly => json!({ "LineItems": [line_item(SKUS[0], 1)] }),
            Placed::FromQuote => {
                json!({ "QuoteIdentifier": a_quote(&outposts, Some(&outpost_id)).await })
            }
        };
        let order_id =
            place_order(&outposts, &order_body(&outpost_id, "ALL_UPFRONT", contents)).await;

        // What the outstanding order holds up, before it is cancelled.
        match held_up {
            HeldUp::Decommission => {
                let blocked = decommission(&outposts, &outpost_id, &json!({})).await;
                assert_eq!(
                    text(&blocked, "Status"),
                    DECOMMISSION_STATUS_BLOCKED,
                    "case {case:?}: an outstanding order blocks the decommission, got \
                     {blocked}"
                );
            }
            HeldUp::Delete => {
                let refused = outposts
                    .call("DELETE", &format!("/outposts/{outpost_id}"), None)
                    .await;
                assert_eq!(
                    (refused.status, refused.error_code.as_str()),
                    (409, CONFLICT_CODE),
                    "case {case:?}: an outstanding order holds the delete up as a \
                     conflict, got {} {} {}",
                    refused.status,
                    refused.error_code,
                    refused.body
                );
            }
        }

        let reply = cancel(&outposts, &order_id).await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: expected the cancellation to be served, got {} {}",
            reply.status,
            reply.body
        );

        // And the same call once the order is cancelled.
        match held_up {
            HeldUp::Decommission => {
                let started = decommission(&outposts, &outpost_id, &json!({})).await;
                assert_eq!(
                    text(&started, "Status"),
                    DECOMMISSION_STATUS_REQUESTED,
                    "case {case:?}: a cancelled order holds nothing up, so the \
                     decommission starts, got {started}"
                );
                assert_eq!(
                    started.get("BlockingResourceTypes"),
                    None,
                    "case {case:?}: nothing blocks the decommission any more, got {started}"
                );
            }
            HeldUp::Delete => {
                let deleted = outposts
                    .call("DELETE", &format!("/outposts/{outpost_id}"), None)
                    .await;
                assert_eq!(
                    deleted.status,
                    200,
                    "case {case:?}: a cancelled order holds nothing up, so the delete \
                     goes through, got {} {}",
                    deleted.status,
                    deleted.body
                );
            }
        }
    }
}

/// Rule 3: an order that has already finished has nothing left to cancel.
///
/// One row per status in which an order is FINISHED (`state::ORDER_STATUS_SETTLED`):
/// the one this API reaches by cancelling the order, and the one an order reaches by
/// running to its end, which no call in this API carries an order to.
#[tokio::test]
async fn an_order_that_has_already_finished_cannot_be_cancelled() {
    let outposts = Outposts::new();
    let (outpost_id, _) = outposts.site_and_outpost("finished-order").await;
    // (case, the status the order is in, whether a CancelOrder call put it there)
    let rows: Vec<(&str, &str, bool)> = vec![
        (
            "an order an earlier CancelOrder call cancelled",
            ORDER_STATUS_CANCELLED,
            true,
        ),
        (
            "an order that ran to its end",
            ORDER_STATUS_COMPLETED,
            false,
        ),
    ];
    assert_eq!(
        rows.len(),
        ORDER_STATUS_SETTLED.len(),
        "every status in which an order is finished has a row of its own: \
         {ORDER_STATUS_SETTLED:?}"
    );

    for (case, status, by_cancelling) in &rows {
        let order_id = place_order(
            &outposts,
            &order_body(
                &outpost_id,
                "ALL_UPFRONT",
                json!({ "LineItems": [line_item(SKUS[1], 2)] }),
            ),
        )
        .await;
        match by_cancelling {
            true => {
                let first = cancel(&outposts, &order_id).await;
                assert_eq!(
                    first.status,
                    200,
                    "case {case:?}: the first cancellation is served, got {} {}",
                    first.status,
                    first.body
                );
            }
            false => finish_order(&outposts, &order_id, status, case),
        }
        let before = get_order(&outposts, &order_id).await;

        let reply = cancel(&outposts, &order_id).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (409, CONFLICT_CODE),
            "case {case:?}: expected 409 {CONFLICT_CODE}, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        assert_eq!(
            text(&reply.body, "ResourceId"),
            order_id,
            "case {case:?}: expected ResourceId {order_id:?}, got {}",
            reply.body
        );
        assert_eq!(
            text(&reply.body, "ResourceType"),
            RESOURCE_TYPE_ORDER,
            "case {case:?}: expected ResourceType {RESOURCE_TYPE_ORDER}, got {}",
            reply.body
        );
        assert_eq!(
            text(&reply.body, "Message"),
            order_status_conflict(&order_id, status),
            "case {case:?}: expected the refusal to name the status the order is in, got {}",
            reply.body
        );
        let after = get_order(&outposts, &order_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused cancellation leaves the order exactly as it was, \
             got {after}"
        );
    }
}

/// Rule 4: an `OrderId` this region's inventory holds no order for is not found, and
/// nothing is cancelled.
///
/// The rows differ along why the id names nothing here: an id nothing ever minted, and
/// an order that exists in ANOTHER region, which this region's endpoint does not
/// serve.
#[tokio::test]
async fn an_order_id_this_region_holds_nothing_for_is_not_found() {
    let outposts = Outposts::new();
    // An order of another region: created through that region's endpoint, on a site
    // and an Outpost of its own, because a resource belongs to the region it was
    // created in.
    let site_id = outposts.site_id_in(OTHER_REGION, "elsewhere-site").await;
    let elsewhere = outposts
        .create_outpost_in(
            OTHER_REGION,
            &json!({ "Name": "elsewhere", "SiteId": site_id }),
        )
        .await;
    let elsewhere_order = outposts
        .ok_in(
            OTHER_REGION,
            "POST",
            "/orders",
            Some(&order_body(
                &text(&elsewhere, "OutpostId"),
                "ALL_UPFRONT",
                json!({ "LineItems": [line_item(SKUS[2], 1)] }),
            )),
        )
        .await["Order"]
        .clone();
    let elsewhere_order_id = text(&elsewhere_order, "OrderId");

    let rows: Vec<(&str, String)> = vec![
        (
            "an OrderId this emulator never minted",
            ABSENT_ORDER_ID.to_string(),
        ),
        (
            "an order of another region, named through this one's endpoint",
            elsewhere_order_id.clone(),
        ),
    ];

    for (case, order_id) in &rows {
        let reply = cancel(&outposts, order_id).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (404, NOT_FOUND_CODE),
            "case {case:?}: expected 404 {NOT_FOUND_CODE}, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        assert_eq!(
            text(&reply.body, "Message"),
            identifier_not_found(ORDER_ID_MEMBER, order_id),
            "case {case:?}: expected the identifier echo of the value as supplied, got {}",
            reply.body
        );
    }

    // The order of the other region is untouched, and still cancellable through the
    // endpoint that serves it.
    let elsewhere_now = outposts
        .ok_in(
            OTHER_REGION,
            "GET",
            &format!("/orders/{}", segment(&elsewhere_order_id)),
            None,
        )
        .await["Order"]
        .clone();
    assert_eq!(
        text(&elsewhere_now, "Status"),
        ORDER_STATUS_PREPARING,
        "a call another region refused cancelled nothing, got {elsewhere_now}"
    );
    let served = cancel_in(&outposts, OTHER_REGION, &elsewhere_order_id).await;
    assert_eq!(
        served.status,
        200,
        "the region the order belongs to serves the cancellation, got {} {}",
        served.status,
        served.body
    );
}
