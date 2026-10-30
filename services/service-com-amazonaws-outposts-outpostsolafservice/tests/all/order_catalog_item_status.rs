// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for which published hardware configurations `CreateOrder` will
//! take an order for, driven over the wire through the same front door as every other
//! suite (the harness in [`crate::site_and_outpost_lifecycle`]).
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. An order may be placed for any configuration the published catalog reports with
//!    the `CatalogItemStatus` of one that can still be ordered, whatever quantity and
//!    however many line items it is ordered through, and the order reads back carrying
//!    exactly the SKUs it was placed for.
//! 2. A line item naming a configuration the catalog reports with the status of one
//!    that is no longer offered is refused as a bad request (400
//!    `ValidationException`), whichever position it holds among the line items, in any
//!    quantity, and beside any other line item — and the refusal persists nothing: no
//!    order exists afterwards and the Outpost is left without the hardware.
//! 3. The refusal names the member and the requirement and renders neither the SKU nor
//!    the status, like every other constraint this service reports.
//! 4. The catalog itself is unaffected: a read still reports a configuration that is
//!    no longer offered, which is what an Outpost already installed was ordered as.
//!
//! Both sets of rows are read OUT of the published catalog by the status each item
//! carries, so a case names hardware this service really publishes and the suite keeps
//! testing the rule if the catalog is republished.

use serde_json::{json, Value};

use crate::site_and_outpost_lifecycle::{segment, text, Outposts};

/// The modelled `CatalogItemStatus` of a configuration that can still be ordered, as
/// `smithy_query(query="shape", shape="CatalogItemStatus")` reports the value set
/// (`AVAILABLE` | `DISCONTINUED`).
const STATUS_AVAILABLE: &str = "AVAILABLE";

/// The modelled `CatalogItemStatus` of a configuration that is no longer offered, from
/// the same value set.
const STATUS_DISCONTINUED: &str = "DISCONTINUED";

/// A well-formed `SkuCode` (the modelled pattern `^OR-[A-Z0-9]{7}$`) the published
/// catalog carries no configuration under, checked against the catalog itself before
/// it is used. Such a SKU resolves to no published configuration, so it carries no
/// published status to refuse an order by.
const UNPUBLISHED_SKU: &str = "OR-0000000";

/// The refusal an order for a configuration that is no longer offered states: the
/// validation report `CreateOrder` answers, naming the member the line items arrived
/// in and the requirement they failed.
const REFUSAL: &str = "1 validation error detected: Value at 'lineItems' failed to satisfy \
                       constraint: every catalog item ordered must be one the catalog still \
                       offers";

/// The whole published catalog, in the order `ListCatalogItems` reports it — the page
/// size is the modelled `MaxResults` maximum, so one call exhausts it.
async fn catalog(outposts: &Outposts) -> Vec<Value> {
    outposts
        .ok("GET", "/catalog/items?MaxResults=1000", None)
        .await
        .get("CatalogItems")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `CatalogItemId` of every published item whose `ItemStatus` is `status`.
async fn skus_with_status(outposts: &Outposts, status: &str) -> Vec<String> {
    catalog(outposts)
        .await
        .iter()
        .filter(|item| text(item, "ItemStatus") == status)
        .map(|item| text(item, "CatalogItemId"))
        .collect()
}

/// One line item request: a catalog item SKU and a quantity.
fn line_item(sku: &str, quantity: i64) -> Value {
    json!({ "CatalogItemId": sku, "Quantity": quantity })
}

/// A `CreateOrder` body: the two members the model marks `@required`, plus the line
/// items the order is placed for.
fn order_request(outpost_identifier: &str, line_items: Vec<Value>) -> Value {
    json!({
        "OutpostIdentifier": outpost_identifier,
        "PaymentOption": "ALL_UPFRONT",
        "LineItems": line_items,
    })
}

/// The `CatalogItemId` of every line item an order reports, in the order it reports
/// them.
fn ordered_skus(order: &Value) -> Vec<String> {
    order
        .get("LineItems")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| text(item, "CatalogItemId"))
                .collect()
        })
        .unwrap_or_default()
}

/// Every `OrderId` `ListOrders` reports for the region the suite works in.
async fn listed_orders(outposts: &Outposts) -> Vec<String> {
    outposts
        .ok("GET", "/list-orders", None)
        .await
        .get("Orders")
        .and_then(Value::as_array)
        .map(|orders| orders.iter().map(|order| text(order, "OrderId")).collect())
        .unwrap_or_default()
}

/// Rule 1: every configuration the catalog still offers can be ordered, and the order
/// reads back carrying the SKUs it was placed for.
///
/// The rows differ along everything the rule generalises over: which published
/// configuration is ordered (every one the catalog still offers), the quantity bought
/// (the modelled minimum of one and larger counts), how many line items the order
/// carries (one, and one per offered configuration at once), and a SKU the catalog
/// publishes nothing under, which resolves to no published configuration at all.
#[tokio::test]
async fn an_order_is_taken_for_every_configuration_the_catalog_still_offers() {
    let outposts = Outposts::new();
    let (outpost_id, _) = outposts.site_and_outpost("offered-outpost").await;
    let offered = skus_with_status(&outposts, STATUS_AVAILABLE).await;
    assert!(
        offered.len() > 1,
        "the catalog must publish configurations that can still be ordered, got {offered:?}"
    );

    // One row per offered configuration, each at its own quantity, plus two rows that
    // vary how many line items one order carries.
    let mut rows: Vec<(String, Vec<Value>)> = Vec::new();
    for (index, sku) in offered.iter().enumerate() {
        let quantity = (index as i64) + 1;
        rows.push((
            format!("one offered configuration {sku} at quantity {quantity}"),
            vec![line_item(sku, quantity)],
        ));
    }
    rows.push((
        "every offered configuration in one order".to_string(),
        offered.iter().map(|sku| line_item(sku, 3)).collect(),
    ));
    rows.push((
        "an offered configuration beside a SKU nothing is published under".to_string(),
        vec![
            line_item(UNPUBLISHED_SKU, 1),
            line_item(&offered[0], 1_000_000),
        ],
    ));

    for (case, line_items) in &rows {
        let want: Vec<String> = line_items
            .iter()
            .map(|item| text(item, "CatalogItemId"))
            .collect();
        let reply = outposts
            .call(
                "POST",
                "/orders",
                Some(&order_request(&outpost_id, line_items.clone())),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200, got {} {}",
            reply.status, reply.body
        );
        let placed = reply.body["Order"].clone();
        let actual = ordered_skus(&placed);
        assert_eq!(
            actual, want,
            "case {case:?}: expected the order to carry {want:?}, got {actual:?}"
        );

        // Rule 1, read back through the implemented reader: what was persisted is what
        // the order was placed for.
        let order_id = text(&placed, "OrderId");
        let read = outposts
            .ok("GET", &format!("/orders/{}", segment(&order_id)), None)
            .await["Order"]
            .clone();
        let actual = ordered_skus(&read);
        assert_eq!(
            actual, want,
            "case {case:?}: expected the stored order to carry {want:?}, got {actual:?}"
        );
    }

    let listed = listed_orders(&outposts).await;
    assert_eq!(
        listed.len(),
        rows.len(),
        "every accepted order must be listed: expected {} orders, got {listed:?}",
        rows.len()
    );
}

/// Rules 2 and 3: a configuration the catalog reports as no longer offered cannot be
/// ordered, in any shape of request, and the refusal leaves nothing behind.
///
/// The rows differ along everything the rule generalises over: which position the
/// offending line item holds (only, first, last, in the middle), how many units of it
/// are asked for (the modelled minimum and larger), how many other line items the
/// order carries, whether those others are offered configurations or a SKU the catalog
/// publishes nothing under, and the same configuration ordered twice in one request.
#[tokio::test]
async fn an_order_is_refused_for_a_configuration_the_catalog_no_longer_offers() {
    let outposts = Outposts::new();
    let (outpost_id, outpost_arn) = outposts.site_and_outpost("retired-outpost").await;
    let offered = skus_with_status(&outposts, STATUS_AVAILABLE).await;
    let retired = skus_with_status(&outposts, STATUS_DISCONTINUED).await;
    assert!(
        !retired.is_empty() && offered.len() > 1,
        "the catalog must publish a configuration that is no longer offered beside ones it \
         still offers, got {retired:?} and {offered:?}"
    );
    let unpublished = UNPUBLISHED_SKU.to_string();
    assert!(
        !offered.contains(&unpublished) && !retired.contains(&unpublished),
        "the row that orders {UNPUBLISHED_SKU:?} needs a SKU this service publishes nothing \
         under, got {offered:?} and {retired:?}"
    );

    let mut rows: Vec<(String, &str, Vec<Value>)> = Vec::new();
    for sku in &retired {
        rows.push((
            format!("only the retired configuration {sku}"),
            outpost_id.as_str(),
            vec![line_item(sku, 1)],
        ));
        rows.push((
            format!("the retired configuration {sku} at a larger quantity"),
            outpost_id.as_str(),
            vec![line_item(sku, 12)],
        ));
        rows.push((
            format!("the retired configuration {sku} first, beside one still offered"),
            outpost_id.as_str(),
            vec![line_item(sku, 1), line_item(&offered[0], 2)],
        ));
        rows.push((
            format!("the retired configuration {sku} last, beside one still offered"),
            outpost_arn.as_str(),
            vec![line_item(&offered[0], 1), line_item(sku, 1)],
        ));
        rows.push((
            format!("the retired configuration {sku} in the middle of three"),
            outpost_id.as_str(),
            vec![
                line_item(&offered[0], 1),
                line_item(sku, 2),
                line_item(&offered[1], 3),
            ],
        ));
        rows.push((
            format!("the retired configuration {sku} beside an unpublished SKU"),
            outpost_id.as_str(),
            vec![line_item(UNPUBLISHED_SKU, 1), line_item(sku, 1)],
        ));
        rows.push((
            format!("the retired configuration {sku} ordered twice"),
            outpost_id.as_str(),
            vec![line_item(sku, 1), line_item(sku, 4)],
        ));
    }

    for (case, outpost, line_items) in &rows {
        let reply = outposts
            .call(
                "POST",
                "/orders",
                Some(&order_request(outpost, line_items.clone())),
            )
            .await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        assert_eq!(
            reply.errortype_header, "ValidationException",
            "case {case:?}: expected the wire error type header, got {:?}",
            reply.errortype_header
        );
        let message = text(&reply.body, "Message");
        assert_eq!(
            message, REFUSAL,
            "case {case:?}: expected $.Message {REFUSAL:?}, got {message:?}"
        );
        // Rule 3: the report renders no value — neither the SKU the request named nor
        // the status it is published in.
        for value in line_items
            .iter()
            .map(|item| text(item, "CatalogItemId"))
            .chain([STATUS_DISCONTINUED.to_string()])
        {
            assert!(
                !message.contains(&value),
                "case {case:?}: the report rendered {value:?}, got {message:?}"
            );
        }
    }

    // Rule 2: nothing was persisted by any of the refusals — no order exists, and the
    // Outpost the orders were placed on holds no hardware and has not left the
    // lifecycle status a create reports it in.
    let listed = listed_orders(&outposts).await;
    assert!(
        listed.is_empty(),
        "a refused order must persist nothing, got {listed:?}"
    );
    crate::site_and_outpost_lifecycle::deliver_orders(&outposts);
    crate::site_and_outpost_lifecycle::settle();
    let outpost = outposts.get_outpost(&outpost_id).await;
    let assets = outposts
        .ok("GET", &format!("/outposts/{outpost_id}/assets"), None)
        .await;
    assert_eq!(
        assets["Assets"].as_array().map(Vec::len).unwrap_or_default(),
        0,
        "a refused order must install no hardware, got {assets:?}"
    );
    assert_ne!(
        text(&outpost, "LifeCycleStatus"),
        "ACTIVE",
        "a refused order must not carry the Outpost to its terminal status, got {outpost:?}"
    );
}

/// Rule 4: the catalog still PUBLISHES the configuration no order can be placed for —
/// it is still what an Outpost already installed was ordered as — so both reads of the
/// catalog report it, whole and unchanged by a refused order.
#[tokio::test]
async fn a_configuration_no_longer_offered_is_still_published_by_the_catalog() {
    let outposts = Outposts::new();
    let (outpost_id, _) = outposts.site_and_outpost("catalog-outpost").await;
    let before = catalog(&outposts).await;
    let retired: Vec<Value> = before
        .iter()
        .filter(|item| text(item, "ItemStatus") == STATUS_DISCONTINUED)
        .cloned()
        .collect();
    assert!(
        !retired.is_empty(),
        "the catalog must publish a configuration that is no longer offered, got {before:?}"
    );

    for item in &retired {
        let sku = text(item, "CatalogItemId");
        let refused = outposts
            .call(
                "POST",
                "/orders",
                Some(&order_request(&outpost_id, vec![line_item(&sku, 1)])),
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {sku:?}: expected the order to be refused, got {} {}",
            refused.status, refused.body
        );
        let read = outposts
            .ok("GET", &format!("/catalog/item/{}", segment(&sku)), None)
            .await["CatalogItem"]
            .clone();
        assert_eq!(
            &read, item,
            "case {sku:?}: expected the catalog's own entry {item:?}, got {read:?}"
        );
        let status = text(&read, "ItemStatus");
        assert_eq!(
            status, STATUS_DISCONTINUED,
            "case {sku:?}: expected the published status {STATUS_DISCONTINUED:?}, got {status:?}"
        );
    }

    let after = catalog(&outposts).await;
    assert_eq!(
        after, before,
        "a refused order changes nothing the catalog publishes, got {after:?} after {before:?}"
    );
}
