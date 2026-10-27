// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for WHAT OWES an Outpost its lifecycle transition, driven over the
//! wire through the same front door as [`crate::site_and_outpost_lifecycle`] (whose
//! harness this suite reuses).
//!
//! [`crate::outpost_hardware_installation`] states what an installed Outpost holds and
//! [`crate::site_and_outpost_projection`] states that every projection reports the
//! status it holds now. This suite states the rule both of those depend on, and states
//! it once:
//!
//! 1. A resource that becomes usable only through work another operation of the API has
//!    to be asked for does not become usable on a span counted from its own creation: an
//!    Outpost `CreateOutpost` recorded holds no hardware and reports the non-terminal
//!    `LifeCycleStatus` for as long as nothing has been ordered for it, however much
//!    time passes.
//! 2. What owes the transition is the FULFILMENT of an order placed on the Outpost: when
//!    the hardware that order delivers has been installed, the order is complete and the
//!    Outpost reports the terminal status holding the configuration its form factor
//!    names.
//! 3. That install is owed at an instant and not applied by the act of reading: an
//!    Outpost whose order is still in flight reports the non-terminal status, and the
//!    first call at or after the instant the install falls due reports the terminal one
//!    — whether or not anybody read in between.
//! 4. An order nothing was delivered under installs nothing: a cancelled order leaves
//!    its Outpost exactly where it was, whatever the clock had carried it to.

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::state::{
    LIFE_CYCLE_STATUS_ACTIVE, LIFE_CYCLE_STATUS_PENDING, LINE_ITEM_STATUS_CANCELLED,
    LINE_ITEM_STATUS_INSTALLED, ORDER_PREPARATION, ORDER_STATUS_CANCELLED, ORDER_STATUS_COMPLETED,
    ORDER_STATUS_DELIVERED, ORDER_STATUS_PREPARING, OUTPOST_PROVISIONING,
};

use crate::site_and_outpost_lifecycle::{deliver_orders, ids, segment, text, Outposts};

/// The `CatalogItemId` every line item below names: a value of the modelled `SkuCode`
/// form (`^OR-[A-Z0-9]{7}$`), which is all this API says about the catalog item a line
/// item may name.
const SKU: &str = "OR-1A2B3C4";

/// The most line items one order may carry, as the modelled
/// `LineItemRequestListDefinition` (`length(min: 1, max: 20)`) allows.
const MAX_LINE_ITEMS: usize = 20;

/// One site and one Outpost on it carrying `form_factor`, as that Outpost's
/// `(OutpostId, OutpostArn)`. An Outpost belongs to a site, so the site comes first.
async fn an_outpost(
    outposts: &Outposts,
    name: &str,
    form_factor: Option<&str>,
) -> (String, String) {
    let site = outposts
        .create_site(&json!({ "Name": format!("{name}-site") }))
        .await;
    let mut request = json!({ "Name": name, "SiteId": text(&site, "SiteId") });
    if let Some(form_factor) = form_factor {
        request["SupportedHardwareType"] = json!(form_factor);
    }
    let outpost = outposts.create_outpost(&request).await;
    (text(&outpost, "OutpostId"), text(&outpost, "OutpostArn"))
}

/// `CreateOrder` for `count` line items of hardware, naming the Outpost by
/// `identifier`, and answering the `Order` it was persisted as.
async fn order_hardware(outposts: &Outposts, identifier: &str, count: usize) -> Value {
    let line_items: Vec<Value> = (0..count)
        .map(|_| json!({ "CatalogItemId": SKU, "Quantity": 1 }))
        .collect();
    outposts
        .ok(
            "POST",
            "/orders",
            Some(&json!({
                "OutpostIdentifier": identifier,
                "PaymentOption": "ALL_UPFRONT",
                "LineItems": line_items,
            })),
        )
        .await["Order"]
        .clone()
}

/// `GetOrder`, answering the `Order` it reports.
async fn get_order(outposts: &Outposts, order_id: &str) -> Value {
    outposts
        .ok("GET", &format!("/orders/{}", segment(order_id)), None)
        .await["Order"]
        .clone()
}

/// `CancelOrder`, requiring HTTP 200.
async fn cancel_order(outposts: &Outposts, order_id: &str) {
    outposts
        .ok(
            "POST",
            &format!("/orders/{}/cancel", segment(order_id)),
            None,
        )
        .await;
}

/// The `LifeCycleStatus` `GetOutpost` reports for one Outpost.
async fn status(outposts: &Outposts, outpost_id: &str) -> String {
    text(&outposts.get_outpost(outpost_id).await, "LifeCycleStatus")
}

/// The hardware `ListAssets` reports for one Outpost, in installation order.
async fn installed(outposts: &Outposts, outpost_id: &str) -> Vec<Value> {
    outposts
        .ok(
            "GET",
            &format!("/outposts/{}/assets", segment(outpost_id)),
            None,
        )
        .await["Assets"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Bring the first `steps` steps of `order_id`'s progression due NOW, leaving each step
/// past them to fall due its own span after the one before it — the spans the service
/// itself counted, now counted from this instant.
///
/// The steps an order owes fall due over spans far longer than a case may sleep for, so
/// a case that wants an order PART WAY through its progression moves those instants
/// through the same store seam the suites that age a stored resource use. What it does
/// NOT move is the span of any step it left ahead of the order: a case about when the
/// install falls due waits that one out on the service's own clock.
fn bring_forward(outposts: &Outposts, order_id: &str, steps: usize, case: &str) {
    assert!(
        steps > 0,
        "case {case:?}: bringing no step forward leaves the order where it is"
    );
    let store = outposts.store();
    let mut store = store.lock();
    let record = store
        .orders
        .get_mut(order_id)
        .unwrap_or_else(|| panic!("case {case:?}: the create persisted order {order_id}"));
    let anchor = record
        .pending
        .get(steps - 1)
        .map(|transition| (transition.due, transition.at))
        .unwrap_or_else(|| panic!("case {case:?}: the order still owes {steps} step(s)"));
    let (now, wall) = (std::time::Instant::now(), std::time::SystemTime::now());
    let offset = anchor.0.saturating_duration_since(now);
    for transition in record.pending.iter_mut() {
        transition.due -= offset;
        transition.at = wall
            + transition
                .at
                .duration_since(anchor.1)
                .unwrap_or(std::time::Duration::ZERO);
    }
}

/// Every `LineItemStatus` one order reports, in the order it reports its line items.
fn line_item_statuses(order: &Value) -> Vec<String> {
    order
        .get("LineItems")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(|item| text(item, "Status")).collect())
        .unwrap_or_default()
}

/// One row of rules 1 and 2: how the Outpost was created, how many line items the order
/// placed on it carries, and which of the two identifier forms that order names it by.
#[derive(Debug)]
struct Row {
    case: &'static str,
    form_factor: Option<&'static str>,
    line_items: usize,
    by_arn: bool,
}

/// The rows both rules are exercised over: they differ along the form factor the create
/// recorded (each modelled value and none at all), the size of the order placed (one
/// line item, a few, the modelled maximum) and the identifier form the order names the
/// Outpost by.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a rack ordered by bare id, one line item",
            form_factor: Some("RACK"),
            line_items: 1,
            by_arn: false,
        },
        Row {
            case: "a server ordered by ARN, three line items",
            form_factor: Some("SERVER"),
            line_items: 3,
            by_arn: true,
        },
        Row {
            case: "no form factor at all, ordered by ARN",
            form_factor: None,
            line_items: 1,
            by_arn: true,
        },
        Row {
            case: "a rack ordered by bare id, the modelled maximum of line items",
            form_factor: Some("RACK"),
            line_items: MAX_LINE_ITEMS,
            by_arn: false,
        },
    ]
}

/// Rule 1: an Outpost nothing has been ordered for stays in the non-terminal status,
/// holds no hardware, and is selected by the status filter that names that status —
/// however much time passes.
///
/// The rows differ along everything the create can carry besides the hardware itself
/// (the form factor, each Availability Zone identity, tags, a description), none of
/// which orders anything.
#[tokio::test]
async fn an_outpost_nothing_was_ordered_for_never_leaves_the_pending_status() {
    let outposts = Outposts::new();
    let site = outposts
        .create_site(&json!({ "Name": "unordered site" }))
        .await;
    let site_id = text(&site, "SiteId");

    let requests: Vec<(&str, Value)> = vec![
        (
            "name and site only",
            json!({ "Name": "plain", "SiteId": site_id }),
        ),
        (
            "a rack with tags and a description",
            json!({
                "Name": "tagged rack",
                "SiteId": site_id,
                "SupportedHardwareType": "RACK",
                "Description": "ordered nothing",
                "Tags": { "Env": "dev" },
            }),
        ),
        (
            "a server named by zone id",
            json!({
                "Name": "zoned server",
                "SiteId": site_id,
                "AvailabilityZoneId": "use1-az3",
                "SupportedHardwareType": "SERVER",
            }),
        ),
        (
            "a zone name and no form factor",
            json!({
                "Name": "zoned",
                "SiteId": site_id,
                "AvailabilityZone": "us-east-1b",
            }),
        ),
    ];

    let mut created: Vec<(&str, String)> = Vec::new();
    for (case, request) in &requests {
        let outpost = outposts.create_outpost(request).await;
        let reported = text(&outpost, "LifeCycleStatus");
        assert_eq!(
            reported, LIFE_CYCLE_STATUS_PENDING,
            "case {case:?}: a create that ordered no hardware answers the non-terminal \
             status, got {reported:?} in {outpost}"
        );
        created.push((case, text(&outpost, "OutpostId")));
    }

    // Nothing in this API installs hardware on request, so no span counted from the
    // create can move these Outposts: the wait is past every span one of them could
    // possibly be owed. Blocking with `std::thread::sleep` for the reason
    // [`crate::site_and_outpost_lifecycle::settle`] does.
    std::thread::sleep(
        ORDER_PREPARATION + OUTPOST_PROVISIONING + std::time::Duration::from_millis(400),
    );

    for (case, outpost_id) in &created {
        let reported = status(&outposts, outpost_id).await;
        assert_eq!(
            reported, LIFE_CYCLE_STATUS_PENDING,
            "case {case:?}: an Outpost nothing was ordered for is still waiting for \
             hardware, got {reported:?}"
        );
        // Reading it again answers the same: a read is not what moves it.
        let again = status(&outposts, outpost_id).await;
        assert_eq!(
            again, reported,
            "case {case:?}: every read answers the same status, got {again:?}"
        );
        let held = installed(&outposts, outpost_id).await;
        assert!(
            held.is_empty(),
            "case {case:?}: an Outpost nothing was ordered for holds no hardware, got \
             {held:?}"
        );
        let pending = ids(
            &outposts
                .list_outposts("?LifeCycleStatusFilter=PENDING")
                .await,
            "Outposts",
        );
        assert!(
            pending.contains(outpost_id),
            "case {case:?}: the filter naming the non-terminal status selects it, got \
             {pending:?}"
        );
        let active = ids(
            &outposts.list_outposts("?LifeCycleStatusFilter=ACTIVE").await,
            "Outposts",
        );
        assert!(
            !active.contains(outpost_id),
            "case {case:?}: the filter naming the terminal status does not select it, got \
             {active:?}"
        );
    }
}

/// Rule 2: the Outpost settles exactly when the hardware ordered for it is installed,
/// and the order that delivered it is complete, fulfilled and installed down to its
/// line items.
#[tokio::test]
async fn the_fulfilment_of_an_order_installs_the_outpost_it_was_placed_on() {
    let outposts = Outposts::new();
    let rows = rows();

    let mut placed: Vec<(&Row, String, String)> = Vec::new();
    for row in &rows {
        let (outpost_id, outpost_arn) = an_outpost(&outposts, row.case, row.form_factor).await;
        let identifier = match row.by_arn {
            true => outpost_arn,
            false => outpost_id.clone(),
        };
        let order = order_hardware(&outposts, &identifier, row.line_items).await;
        let reported = text(&order, "Status");
        assert_eq!(
            reported, ORDER_STATUS_PREPARING,
            "case {:?}: a just-submitted order is being prepared, got {reported:?} in \
             {order}",
            row.case
        );
        // Placing the order delivers nothing, so the Outpost has still had nothing
        // installed on it.
        let waiting = status(&outposts, &outpost_id).await;
        assert_eq!(
            waiting, LIFE_CYCLE_STATUS_PENDING,
            "case {:?}: an Outpost whose order is still being prepared holds no hardware \
             yet, got {waiting:?}",
            row.case
        );
        placed.push((row, outpost_id, text(&order, "OrderId")));
    }

    deliver_orders(&outposts);

    for (row, outpost_id, order_id) in &placed {
        let case = row.case;
        let reported = status(&outposts, outpost_id).await;
        assert_eq!(
            reported, LIFE_CYCLE_STATUS_ACTIVE,
            "case {case:?}: an Outpost whose ordered hardware is installed reports the \
             terminal status, got {reported:?}"
        );
        // The order the install completed reports it: complete, fulfilled, and every
        // line item installed.
        let order = get_order(&outposts, order_id).await;
        let order_status = text(&order, "Status");
        assert_eq!(
            order_status, ORDER_STATUS_COMPLETED,
            "case {case:?}: the order whose hardware is installed is complete, got \
             {order_status:?} in {order}"
        );
        assert!(
            order.get("OrderFulfilledDate").is_some(),
            "case {case:?}: a complete order reports when it was fulfilled, got {order}"
        );
        let items = line_item_statuses(&order);
        assert_eq!(
            items,
            vec![LINE_ITEM_STATUS_INSTALLED.to_string(); row.line_items],
            "case {case:?}: every line item of a complete order is installed, got {order}"
        );
        // What was installed is the configuration the Outpost's form factor names, and
        // a form factor naming none installs nothing.
        let held = installed(&outposts, outpost_id).await;
        assert_eq!(
            held.is_empty(),
            row.form_factor.is_none(),
            "case {case:?}: an Outpost holds the hardware of the configuration its form \
             factor names ({:?}), got {held:?}",
            row.form_factor
        );
        // And nothing moves again: the install happened once.
        let again = status(&outposts, outpost_id).await;
        assert_eq!(
            again, LIFE_CYCLE_STATUS_ACTIVE,
            "case {case:?}: a settled Outpost reads the same every time, got {again:?}"
        );
        assert_eq!(
            installed(&outposts, outpost_id).await,
            held,
            "case {case:?}: every read answers the hardware installed once"
        );
    }
}

/// Rule 3: the install is owed at an instant, not applied by the act of reading. An
/// Outpost whose hardware has reached the site is still waiting, and the first call at
/// or after the instant the install falls due reports it installed — on the service's
/// OWN clock, with the install step's span never moved.
///
/// The rows differ along whether anybody read the Outpost while the install was still
/// ahead of it: the answer after the span has passed is the same either way, which is
/// what "owed at an instant" means.
#[tokio::test]
async fn the_install_an_order_owes_is_due_at_an_instant_and_not_on_a_read() {
    let outposts = Outposts::new();
    let rows: &[(&str, bool)] = &[
        ("read while the install was still ahead of it", true),
        ("not read at all until the install had fallen due", false),
    ];

    let mut placed: Vec<(&str, bool, String, String)> = Vec::new();
    for (case, polled) in rows {
        let (outpost_id, _) = an_outpost(&outposts, case, Some("RACK")).await;
        let order = order_hardware(&outposts, &outpost_id, 1).await;
        let order_id = text(&order, "OrderId");
        // Everything up to the hardware standing at the site is brought forward; the
        // install itself keeps the span the service gave it. The row that reads while
        // that span is still running reads here, inside it.
        bring_forward(&outposts, &order_id, 2, case);
        if *polled {
            for pass in ["first read", "second read"] {
                let reported = status(&outposts, &outpost_id).await;
                assert_eq!(
                    reported, LIFE_CYCLE_STATUS_PENDING,
                    "case {case:?} ({pass}): an Outpost whose install is not due yet is \
                     still waiting, got {reported:?}"
                );
            }
            let read = get_order(&outposts, &order_id).await;
            let reported = text(&read, "Status");
            assert_eq!(
                reported, ORDER_STATUS_DELIVERED,
                "case {case:?}: the order's hardware is at the site and not yet \
                 installed, got {reported:?} in {read}"
            );
            assert!(
                read.get("OrderFulfilledDate").is_none(),
                "case {case:?}: an order whose hardware is not installed yet reports no \
                 fulfilment date, got {read}"
            );
        }
        placed.push((case, *polled, outpost_id, order_id));
    }

    // Waiting past the span installing hardware takes is what makes the install due.
    // Blocking with `std::thread::sleep` for the reason
    // [`crate::site_and_outpost_lifecycle::settle`] does.
    std::thread::sleep(OUTPOST_PROVISIONING + std::time::Duration::from_millis(400));

    for (case, _, outpost_id, order_id) in &placed {
        let reported = status(&outposts, outpost_id).await;
        assert_eq!(
            reported, LIFE_CYCLE_STATUS_ACTIVE,
            "case {case:?}: the first read at or after the install reports it installed, \
             got {reported:?}"
        );
        let order = get_order(&outposts, order_id).await;
        let order_status = text(&order, "Status");
        assert_eq!(
            order_status, ORDER_STATUS_COMPLETED,
            "case {case:?}: the order the install completed is complete, got \
             {order_status:?} in {order}"
        );
        assert!(
            order.get("OrderFulfilledDate").is_some(),
            "case {case:?}: a complete order reports when it was fulfilled, got {order}"
        );
        let held = installed(&outposts, outpost_id).await;
        assert!(
            !held.is_empty(),
            "case {case:?}: the install put the configuration of its form factor on the \
             Outpost, got {held:?}"
        );
    }
}

/// Rule 4: a cancelled order delivers nothing, so it installs nothing — whatever the
/// clock had carried it to when it was cancelled, and however long the store runs on
/// afterwards.
///
/// The rows differ along how far the order had got when it was cancelled: just
/// submitted, being built and shipped, and standing at the site.
#[tokio::test]
async fn a_cancelled_order_installs_nothing_on_its_outpost() {
    let outposts = Outposts::new();
    let rows: &[(&str, usize)] = &[
        ("cancelled while being prepared", 0),
        ("cancelled while being built and shipped", 1),
        ("cancelled once it had reached the site", 2),
    ];

    let mut placed: Vec<(&str, String, String)> = Vec::new();
    for (case, steps) in rows {
        let (outpost_id, _) = an_outpost(&outposts, case, Some("RACK")).await;
        let order = order_hardware(&outposts, &outpost_id, 2).await;
        let order_id = text(&order, "OrderId");
        if *steps > 0 {
            bring_forward(&outposts, &order_id, *steps, case);
            // Read the order so the steps the row brought forward are behind it before
            // the cancellation lands.
            get_order(&outposts, &order_id).await;
        }
        cancel_order(&outposts, &order_id).await;
        placed.push((case, outpost_id, order_id));
    }

    // Every step any order of this store still owes is brought due, and the span an
    // install takes waited out on top: a cancelled order has nothing left to deliver,
    // so neither changes anything about it or about its Outpost.
    deliver_orders(&outposts);
    std::thread::sleep(OUTPOST_PROVISIONING + std::time::Duration::from_millis(400));

    for (case, outpost_id, order_id) in &placed {
        let order = get_order(&outposts, order_id).await;
        let order_status = text(&order, "Status");
        assert_eq!(
            order_status, ORDER_STATUS_CANCELLED,
            "case {case:?}: a cancelled order stays cancelled, got {order_status:?} in \
             {order}"
        );
        assert_eq!(
            line_item_statuses(&order),
            vec![LINE_ITEM_STATUS_CANCELLED.to_string(); 2],
            "case {case:?}: every line item of a cancelled order is cancelled, got {order}"
        );
        assert!(
            order.get("OrderFulfilledDate").is_none(),
            "case {case:?}: a cancelled order was never fulfilled, got {order}"
        );
        let reported = status(&outposts, outpost_id).await;
        assert_eq!(
            reported, LIFE_CYCLE_STATUS_PENDING,
            "case {case:?}: an Outpost whose only order was cancelled is still waiting \
             for hardware, got {reported:?}"
        );
        let held = installed(&outposts, outpost_id).await;
        assert!(
            held.is_empty(),
            "case {case:?}: a cancelled order installs no hardware, got {held:?}"
        );
    }
}
