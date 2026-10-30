// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for WHAT a line item of an order reports as the hardware installed
//! for it, driven over the wire through the same front door as
//! [`crate::site_and_outpost_lifecycle`] (whose harness this suite reuses, as does
//! [`crate::outpost_order_driven_lifecycle`], whose order helpers it shares).
//!
//! The rule, stated once and exercised over a SET of inputs: a member whose fact this
//! service only holds once a later step has happened is absent until that step and
//! reported from the stored fact afterwards. `LineItem$AssetInformationList` is
//! "information about assets" — the hardware installed FOR the item — so:
//!
//! 1. A line item that has been installed reports the hardware the install its order's
//!    fulfilment owed put on the Outpost the order was placed on: exactly the assets
//!    `ListAssets` reports for that Outpost, each one reported by exactly one of the
//!    order's items, and an item no unit was delivered for reports nothing rather than
//!    an empty list.
//! 2. A line item nothing has been installed for reports none of it — one still being
//!    prepared, built or delivered, or one a cancellation finished — even where the
//!    Outpost it was ordered for already holds hardware of its own.
//! 3. The hardware an installed item reports is the hardware its Outpost holds: one
//!    configuration is installed on an Outpost once, so a second order fulfilled on an
//!    Outpost already holding it reports that same hardware, and the install is not
//!    performed again.

use serde_json::Value;

use service_com_amazonaws_outposts_outpostsolafservice::state::{
    LINE_ITEM_STATUS_BUILDING, LINE_ITEM_STATUS_CANCELLED, LINE_ITEM_STATUS_DELIVERED,
    LINE_ITEM_STATUS_INSTALLED, LINE_ITEM_STATUS_PREPARING, ORDER_STATUS_COMPLETED,
};

use crate::outpost_order_driven_lifecycle::{
    an_outpost, bring_forward, cancel_order, get_order, installed, line_item_statuses,
    order_hardware,
};
use crate::site_and_outpost_lifecycle::{deliver_orders, text, Outposts};

/// The most line items one order may carry, as the modelled
/// `LineItemRequestListDefinition` (`length(min: 1, max: 20)`) allows.
///
/// Source: `smithy_query(query="shape", shape="LineItemRequestListDefinition")` against
/// this service's model.
const MAX_LINE_ITEMS: usize = 20;

/// The line items one order reports, in the order it reports them.
fn line_items(order: &Value) -> Vec<Value> {
    order
        .get("LineItems")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Every `AssetId` one line item reports in its `AssetInformationList`, in the order it
/// reports them, requiring every entry to name an asset: an entry carrying no `AssetId`
/// would be information about no asset at all.
fn reported_asset_ids(item: &Value, case: &str) -> Vec<String> {
    item.get("AssetInformationList")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    let asset_id = entry.get("AssetId").and_then(Value::as_str);
                    assert!(
                        asset_id.is_some(),
                        "case {case:?}: every asset information entry names the asset it is \
                         about, got {entry}"
                    );
                    asset_id.unwrap_or_default().to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every `AssetId` one order reports across all of its line items, in reporting order.
fn asset_ids_across_items(order: &Value, case: &str) -> Vec<String> {
    line_items(order)
        .iter()
        .flat_map(|item| reported_asset_ids(item, case))
        .collect()
}

/// Require that no line item of `order` reports any asset information at all — the
/// member absent, not an empty list, which would be a report of no hardware.
fn assert_reports_no_assets(order: &Value, case: &str, when: &str) {
    for (index, item) in line_items(order).iter().enumerate() {
        assert!(
            item.get("AssetInformationList").is_none(),
            "case {case:?} ({when}): line item {index} must report no asset information, \
             got {item}"
        );
    }
}

/// `values` sorted, so two reports of the same set of assets compare equal however each
/// one was ordered.
fn sorted(values: &[String]) -> Vec<String> {
    let mut sorted = values.to_vec();
    sorted.sort();
    sorted
}

/// One row of rule 1: how the Outpost was created — which decides the configuration its
/// install delivers, from one single unit to a whole rack of them, and none at all for a
/// form factor naming no configuration — and how many line items the order placed on it
/// carries.
#[derive(Debug)]
struct Row {
    case: &'static str,
    form_factor: Option<&'static str>,
    line_items: usize,
}

/// The rows rule 1 is exercised over: they differ along the form factor the create
/// recorded (each modelled value and none at all) and the size of the order placed (one
/// line item, a few, and the modelled maximum — more items than a rack has units, so the
/// units run out before the items do).
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a rack, one line item",
            form_factor: Some("RACK"),
            line_items: 1,
        },
        Row {
            case: "a rack, three line items",
            form_factor: Some("RACK"),
            line_items: 3,
        },
        Row {
            case: "a rack, the modelled maximum of line items",
            form_factor: Some("RACK"),
            line_items: MAX_LINE_ITEMS,
        },
        Row {
            case: "a server, one line item",
            form_factor: Some("SERVER"),
            line_items: 1,
        },
        Row {
            case: "a server, three line items",
            form_factor: Some("SERVER"),
            line_items: 3,
        },
        Row {
            case: "no form factor at all, two line items",
            form_factor: None,
            line_items: 2,
        },
    ]
}

/// Rule 1: a fulfilled order's installed line items report the hardware its install
/// delivered — the assets `ListAssets` reports for the Outpost it was placed on, each
/// one reported by exactly one item — and report none of it before that install.
#[tokio::test]
async fn an_installed_line_item_reports_the_hardware_its_install_delivered() {
    let outposts = Outposts::new();
    let rows = rows();

    let mut placed: Vec<(&Row, String, String)> = Vec::new();
    for row in &rows {
        let (outpost_id, _) = an_outpost(&outposts, row.case, row.form_factor).await;
        let order = order_hardware(&outposts, &outpost_id, row.line_items).await;
        // Submitting an order installs nothing, so no item of it reports hardware yet —
        // in the answer the submit gives, or in a read of it before anything was
        // delivered.
        assert_reports_no_assets(&order, row.case, "the create response");
        let order_id = text(&order, "OrderId");
        let read = get_order(&outposts, &order_id).await;
        assert_reports_no_assets(&read, row.case, "read while the order was being prepared");
        placed.push((row, outpost_id, order_id));
    }

    deliver_orders(&outposts);

    for (row, outpost_id, order_id) in &placed {
        let case = row.case;
        let order = get_order(&outposts, order_id).await;
        let status = text(&order, "Status");
        assert_eq!(
            status, ORDER_STATUS_COMPLETED,
            "case {case:?}: the order whose hardware is installed is complete, got \
             {status:?} in {order}"
        );
        assert_eq!(
            line_item_statuses(&order),
            vec![LINE_ITEM_STATUS_INSTALLED.to_string(); row.line_items],
            "case {case:?}: every line item of a complete order is installed, got {order}"
        );
        // What the install delivered, as the Outpost's own report of its hardware.
        let held: Vec<String> = installed(&outposts, outpost_id)
            .await
            .iter()
            .map(|asset| text(asset, "AssetId"))
            .collect();
        assert_eq!(
            held.is_empty(),
            row.form_factor.is_none(),
            "case {case:?}: an Outpost holds the hardware of the configuration its form \
             factor names ({:?}), got {held:?}",
            row.form_factor
        );
        // Every asset the install delivered is reported by the order's line items, and
        // each one by exactly one of them: the two sorted lists are equal, so nothing is
        // missing, nothing is reported twice and nothing is reported that is not
        // installed.
        let reported = asset_ids_across_items(&order, case);
        assert_eq!(
            sorted(&reported),
            sorted(&held),
            "case {case:?}: the installed line items report exactly the hardware the \
             install delivered ({held:?}), got {reported:?} in {order}"
        );
        // An item no unit of that hardware was delivered for reports nothing at all
        // rather than an empty list — every item of an order on an Outpost holding no
        // hardware, and the items past the units where an order carries more items than
        // the configuration has units.
        for (index, item) in line_items(&order).iter().enumerate() {
            if reported_asset_ids(item, case).is_empty() {
                assert!(
                    item.get("AssetInformationList").is_none(),
                    "case {case:?}: line item {index} had no unit installed for it, so it \
                     reports no asset information at all, got {item}"
                );
            }
        }
        // And the report is stable: reading again answers the same hardware.
        let again = get_order(&outposts, order_id).await;
        assert_eq!(
            again, order,
            "case {case:?}: every read of a fulfilled order answers the same hardware, got \
             {again}"
        );
    }
}

/// Rule 3: a second order fulfilled on an Outpost that already holds its configuration
/// reports the hardware standing on that Outpost, exactly as the order whose fulfilment
/// installed it does — and that install happened once.
#[tokio::test]
async fn a_second_fulfilled_order_reports_the_hardware_its_outpost_holds() {
    let case = "a second order on an installed rack";
    let outposts = Outposts::new();
    let (outpost_id, _) = an_outpost(&outposts, case, Some("RACK")).await;

    let first_id = text(&order_hardware(&outposts, &outpost_id, 1).await, "OrderId");
    deliver_orders(&outposts);
    get_order(&outposts, &first_id).await;
    let held: Vec<String> = installed(&outposts, &outpost_id)
        .await
        .iter()
        .map(|asset| text(asset, "AssetId"))
        .collect();
    assert!(
        !held.is_empty(),
        "case {case:?}: the first order's fulfilment installed the rack's hardware, got \
         {held:?}"
    );

    let second_id = text(&order_hardware(&outposts, &outpost_id, 2).await, "OrderId");
    deliver_orders(&outposts);
    let second = get_order(&outposts, &second_id).await;
    assert_eq!(
        line_item_statuses(&second),
        vec![LINE_ITEM_STATUS_INSTALLED.to_string(); 2],
        "case {case:?}: every line item of the second complete order is installed, got \
         {second}"
    );
    assert_eq!(
        sorted(&asset_ids_across_items(&second, case)),
        sorted(&held),
        "case {case:?}: the second order's items report the hardware its Outpost holds \
         ({held:?}), got {second}"
    );
    // Nothing was installed a second time, and the first order reports the same hardware
    // it always did.
    let again: Vec<String> = installed(&outposts, &outpost_id)
        .await
        .iter()
        .map(|asset| text(asset, "AssetId"))
        .collect();
    assert_eq!(
        again, held,
        "case {case:?}: one configuration is installed on an Outpost once, got {again:?}"
    );
    let first = get_order(&outposts, &first_id).await;
    assert_eq!(
        sorted(&asset_ids_across_items(&first, case)),
        sorted(&held),
        "case {case:?}: the order whose fulfilment installed the hardware still reports \
         it ({held:?}), got {first}"
    );
}

/// Rule 2: an item nothing has been installed for reports no asset information, even on
/// an Outpost that already holds hardware of its own.
///
/// The rows differ along how far the order had got when it was read: just submitted,
/// being built and shipped, standing at the site, and cancelled from either of two
/// points — every item-level status other than the installed one that this slice of the
/// API can produce.
#[tokio::test]
async fn a_line_item_nothing_was_installed_for_reports_no_hardware() {
    let outposts = Outposts::new();
    // One Outpost whose hardware a first order has already delivered and installed, so
    // every row below places its order on an Outpost that DOES hold hardware.
    let (outpost_id, _) = an_outpost(&outposts, "an installed rack", Some("RACK")).await;
    let first = order_hardware(&outposts, &outpost_id, 1).await;
    let first_id = text(&first, "OrderId");
    deliver_orders(&outposts);
    let fulfilled = get_order(&outposts, &first_id).await;
    let held: Vec<String> = installed(&outposts, &outpost_id)
        .await
        .iter()
        .map(|asset| text(asset, "AssetId"))
        .collect();
    assert!(
        !held.is_empty(),
        "the first order's fulfilment installed the rack's hardware, got {held:?}"
    );
    assert_eq!(
        sorted(&asset_ids_across_items(&fulfilled, "the first order")),
        sorted(&held),
        "the fulfilled order reports the hardware its install delivered ({held:?}), got \
         {fulfilled}"
    );

    // (case, how many of the order's steps have fallen due, whether it was cancelled,
    // the item-level status its line items then hold).
    let rows: &[(&str, usize, bool, &str)] = &[
        ("still being prepared", 0, false, LINE_ITEM_STATUS_PREPARING),
        ("being built and shipped", 1, false, LINE_ITEM_STATUS_BUILDING),
        ("standing at the site", 2, false, LINE_ITEM_STATUS_DELIVERED),
        (
            "cancelled while being prepared",
            0,
            true,
            LINE_ITEM_STATUS_CANCELLED,
        ),
        (
            "cancelled once it had reached the site",
            2,
            true,
            LINE_ITEM_STATUS_CANCELLED,
        ),
    ];

    for (case, steps, cancelled, expected) in rows {
        let order = order_hardware(&outposts, &outpost_id, 2).await;
        let order_id = text(&order, "OrderId");
        if *steps > 0 {
            // Only the steps this row wants behind the order are brought forward; the
            // ones past them keep the spans the service gave them, so the order cannot
            // run on to being installed under the row.
            bring_forward(&outposts, &order_id, *steps, case);
        }
        if *cancelled {
            cancel_order(&outposts, &order_id).await;
        }
        let read = get_order(&outposts, &order_id).await;
        assert_eq!(
            line_item_statuses(&read),
            vec![expected.to_string(); 2],
            "case {case:?}: expected every line item in {expected}, got {read}"
        );
        assert_reports_no_assets(&read, case, "nothing was installed for it");
    }

    // The order whose fulfilment DID install the hardware still reports it: none of the
    // rows above took it away.
    let again = get_order(&outposts, &first_id).await;
    assert_eq!(
        sorted(&asset_ids_across_items(&again, "the first order, read again")),
        sorted(&held),
        "the fulfilled order still reports the hardware its install delivered ({held:?}), \
         got {again}"
    );
}
