// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the OUTPOST the optional `OrderId` of a capacity task is
//! resolved against, driven over the wire through the same front door as
//! [`crate::site_and_outpost_lifecycle`] (whose harness this suite reuses) and
//! starting its tasks through [`crate::capacity_task_lifecycle`]'s helpers.
//!
//! The rule under test, stated once and then exercised over a SET of inputs: an
//! `OrderId` carried by a request addressed to ONE Outpost names an order placed on
//! THAT Outpost — `StartCapacityTask`'s is the order "of the host associated with the
//! capacity task", and an order is placed on one Outpost and delivers hardware to no
//! other — so an order of the same region placed against a different Outpost names no
//! host here: 404 `NotFoundException` worded as every refusal about an order this
//! service holds none of is, with nothing stored. Which REGION'S inventory the id is
//! looked for in is [`crate::capacity_task_order_region`]'s; the same rule read by
//! the report that scopes itself by the same member is
//! [`crate::supported_instance_types_order_scope`]'s.
//!
//! Two consequences the rows pin beside it: a task is never persisted carrying an
//! order of other hardware, so every task the service holds names an order of its own
//! Outpost; and a reference refused on one Outpost holds nothing up on the Outpost the
//! order really was placed on, which can still start its own capacity task and see it
//! in `ListCapacityTasks` filtered to itself.
//!
//! The rows differ along what the rule generalises over: which Outpost the call is
//! addressed to, whether the order was placed on another Outpost of the SAME site or
//! of another site, why the id names no order of this Outpost (another Outpost's
//! order, an id nothing minted), how many orders the Outpost in hand has of its own,
//! which identifier form names the Outpost (bare id, full ARN), whether the call is a
//! dry run, and whether an `AssetId` is named beside the order.

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::state::order_not_found;

use crate::capacity_task_lifecycle::{
    an_order, capacity_route, install, pool, task_route, ABSENT_ORDER_ID, INSTALLED,
};
use crate::site_and_outpost_lifecycle::{
    mark_outpost_active, query_value, segment, text, Outposts, Reply, HOME_REGION,
};

/// The wire error code an `OrderId` that names no order of the Outpost in hand
/// arrives as, from `smithy_query(query="operation",operation="StartCapacityTask")`:
/// `NotFoundException`, HTTP 404.
const NOT_FOUND_CODE: &str = "NotFoundException";

/// The account-level route `ListCapacityTasks` is bound to — `GET /capacity/tasks`,
/// which is where a caller looks for the task holding an order.
const TASKS_ROUTE: &str = "/capacity/tasks";

/// One Outpost of the suite's inventory: the two identifier forms every route that
/// takes an Outpost accepts it in, and the orders the account placed ON it.
struct Placed {
    name: &'static str,
    outpost_id: String,
    outpost_arn: String,
    orders: Vec<String>,
}

impl Placed {
    /// The two identifier forms the modelled `OutpostIdentifier` pattern accepts for
    /// the Outpost a task runs on: the bare id and the full ARN.
    fn forms(&self) -> [(&'static str, &str); 2] {
        [
            ("bare id", self.outpost_id.as_str()),
            ("ARN", self.outpost_arn.as_str()),
        ]
    }
}

/// One Outpost on `site_id`, with `orders` orders placed on it over the wire.
///
/// The Outpost reports the terminal `ACTIVE` lifecycle status, because a capacity task
/// configures the capacity of an Outpost's installed hardware and is refused on one
/// whose hardware is not in service ([`crate::precondition_gates`], which states that
/// refusal); what these rows are about is the `OrderId` resolution behind that gate.
async fn place(
    outposts: &Outposts,
    name: &'static str,
    site_id: &str,
    orders: usize,
) -> Placed {
    let outpost = outposts
        .create_outpost(&json!({ "Name": name, "SiteId": site_id }))
        .await;
    let outpost_id = text(&outpost, "OutpostId");
    mark_outpost_active(outposts, &outpost_id);
    let mut placed = Vec::new();
    for _ in 0..orders {
        placed.push(an_order(outposts, &outpost_id).await);
    }
    Placed {
        name,
        outpost_id,
        outpost_arn: text(&outpost, "OutpostArn"),
        orders: placed,
    }
}

/// The inventory every case works from: three Outposts of ONE region — two of them on
/// a single site, so a row's "another Outpost" is not told apart by its site — each
/// carrying orders of its own, one of them more than one.
async fn inventory(outposts: &Outposts) -> Vec<Placed> {
    let shared = outposts
        .site_id_in(HOME_REGION, "capacity-order-outpost-shared-site")
        .await;
    let own = outposts
        .site_id_in(HOME_REGION, "capacity-order-outpost-own-site")
        .await;
    vec![
        place(outposts, "the first Outpost of a shared site", &shared, 2).await,
        place(outposts, "the second Outpost of that same site", &shared, 1).await,
        place(outposts, "an Outpost of a site of its own", &own, 1).await,
    ]
}

/// A `StartCapacityTask` body naming `order_id`, as a real start or as a dry run, with
/// or without an `AssetId` beside the order.
fn request_naming(order_id: &str, dry_run: bool, asset_id: Option<&str>) -> Value {
    let mut request = json!({
        "InstancePools": [pool("m5.4xlarge", 1)],
        "OrderId": order_id,
        "DryRun": dry_run,
    });
    if let Some(asset_id) = asset_id {
        request["AssetId"] = json!(asset_id);
    }
    request
}

/// `StartCapacityTask` on `outpost_identifier`.
async fn start(outposts: &Outposts, outpost_identifier: &str, request: &Value) -> Reply {
    outposts
        .call("POST", &capacity_route(outpost_identifier), Some(request))
        .await
}

/// How many capacity tasks the store holds. A refused start must leave this where it
/// was: the reference the resolution exists to keep out is one that got PERSISTED.
fn tasks_held(outposts: &Outposts) -> usize {
    outposts.store().lock().capacity_tasks.len()
}

/// Every capacity task of the region, as `ListCapacityTasks` reports them.
async fn listed(outposts: &Outposts, query: &str) -> Vec<Value> {
    outposts
        .ok("GET", &format!("{TASKS_ROUTE}{query}"), None)
        .await
        .get("CapacityTasks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("ListCapacityTasks answers a CapacityTasks list"))
}

/// The `Order` this region's inventory holds under `order_id`.
async fn order(outposts: &Outposts, order_id: &str) -> Value {
    outposts
        .ok("GET", &format!("/orders/{}", segment(order_id)), None)
        .await["Order"]
        .clone()
}

/// The invariant behind the rule, asserted over every task the store holds: the order
/// a task carries is an order of the task's OWN Outpost, so a caller can follow the
/// reference from either end and arrive at the same hardware.
async fn every_task_names_an_order_of_its_own_outpost(outposts: &Outposts, case: &str) {
    for summary in listed(outposts, "").await {
        let Some(order_id) = summary.get("OrderId").and_then(Value::as_str) else {
            continue;
        };
        let placed_on = text(&order(outposts, order_id).await, "OutpostId");
        assert_eq!(
            placed_on,
            text(&summary, "OutpostId"),
            "case {case:?}: task {summary} carries an order placed on {placed_on:?}"
        );
    }
}

/// The rule: an `OrderId` on `StartCapacityTask` is resolved against the Outpost the
/// request is addressed to, so an order placed on another Outpost of the same region
/// is refused exactly as an id nothing minted is — and the Outpost's own orders are
/// accepted and read back.
#[tokio::test]
async fn a_capacity_task_only_takes_an_order_placed_on_the_outpost_it_runs_on() {
    let outposts = Outposts::new();
    let inventory = inventory(&outposts).await;
    // Hardware on the first Outpost, so a row can name an `AssetId` that DOES resolve
    // beside an order that does not.
    install(
        &outposts,
        &inventory[0].outpost_id,
        &INSTALLED,
        "capacity-order-outpost",
    );

    for addressed in &inventory {
        // Every id that names no order of THIS Outpost: each order the account placed
        // on another Outpost of the same region, and an id nothing ever minted — the
        // two reasons an `OrderId` resolves to nothing here, which have to read the
        // same to the caller.
        let mut absent: Vec<(String, &str)> = vec![(
            "an OrderId this emulator never minted".to_string(),
            ABSENT_ORDER_ID,
        )];
        for holder in &inventory {
            if holder.outpost_id == addressed.outpost_id {
                continue;
            }
            for order_id in &holder.orders {
                absent.push((
                    format!("an order placed on {}", holder.name),
                    order_id.as_str(),
                ));
            }
        }

        for (reason, order_id) in &absent {
            for (form, identifier) in addressed.forms() {
                for dry_run in [false, true] {
                    for asset_id in [None, Some(INSTALLED[0])] {
                        let case = format!(
                            "{reason}, named on {} through its {form}, DryRun={dry_run}, \
                             AssetId={asset_id:?}",
                            addressed.name
                        );
                        let reply = start(
                            &outposts,
                            identifier,
                            &request_naming(order_id, dry_run, asset_id),
                        )
                        .await;
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
                            order_not_found(order_id),
                            "case {case:?}: expected the order not-found wording for the \
                             id supplied, got {}",
                            reply.body
                        );
                    }
                }
            }
        }
    }

    // Not one of those calls created a task: a reference that resolves to nothing is
    // never persisted on one.
    let held = tasks_held(&outposts);
    assert_eq!(
        held, 0,
        "a refused start must create no capacity task: expected 0 held, got {held}"
    );

    // The accepting side: on the Outpost an order WAS placed on, every one of its
    // orders is accepted, through either identifier form. These rows are DRY RUNS,
    // which create no task, because only one task can be active on an Outpost at a
    // time — what each row is about is the resolution, not the task.
    for addressed in &inventory {
        for order_id in &addressed.orders {
            for (form, identifier) in addressed.forms() {
                let case = format!(
                    "{}'s own order, named through its {form}",
                    addressed.name
                );
                let reply =
                    start(&outposts, identifier, &request_naming(order_id, true, None)).await;
                assert_eq!(
                    (reply.status, reply.error_code.as_str()),
                    (200, ""),
                    "case {case:?}: the Outpost the order was placed on serves the \
                     start, got {} {} {}",
                    reply.status,
                    reply.error_code,
                    reply.body
                );
                assert_eq!(
                    text(&reply.body, "OrderId"),
                    *order_id,
                    "case {case:?}: the resolved order is reported back, got {}",
                    reply.body
                );
            }
        }
    }

    // And the same request as a real start: the task is minted carrying that order,
    // and the read answers the persisted record.
    for addressed in &inventory {
        let order_id = &addressed.orders[0];
        let case = format!("a task on {}", addressed.name);
        let started = outposts
            .ok(
                "POST",
                &capacity_route(&addressed.outpost_id),
                Some(&request_naming(order_id, false, None)),
            )
            .await;
        let task_id = text(&started, "CapacityTaskId");
        let read = outposts
            .ok("GET", &task_route(&addressed.outpost_id, &task_id), None)
            .await;
        assert_eq!(
            (text(&read, "OrderId"), text(&read, "OutpostId")),
            (order_id.clone(), addressed.outpost_id.clone()),
            "case {case:?}: GetCapacityTask must answer the persisted order and \
             Outpost, got {read}"
        );
        // The round trip from the other end: the order that id names is an order of
        // this very Outpost, which is what the resolution is for.
        assert_eq!(
            text(&order(&outposts, order_id).await, "OutpostId"),
            addressed.outpost_id,
            "case {case:?}: the OrderId a task carries names an order of its own Outpost"
        );
    }
    every_task_names_an_order_of_its_own_outpost(&outposts, "after every accepted start").await;
}

/// The rule, stated for what a reference of other hardware would have cost: a start
/// refused on the wrong Outpost holds nothing up, so the Outpost the order really was
/// placed on still starts its own capacity task and still sees it in the list filtered
/// to itself.
#[tokio::test]
async fn an_order_refused_on_another_outpost_still_serves_its_own_outposts_task() {
    let outposts = Outposts::new();
    let inventory = inventory(&outposts).await;
    let (first, second) = (&inventory[0], &inventory[1]);
    let order_id = &second.orders[0];

    // The call the rule refuses: the first Outpost's capacity route, naming an order
    // placed on the second.
    let refused = start(
        &outposts,
        &first.outpost_id,
        &request_naming(order_id, false, None),
    )
    .await;
    assert_eq!(
        (refused.status, text(&refused.body, "Message")),
        (404, order_not_found(order_id)),
        "a start naming another Outpost's order is the order not-found, got {} {}",
        refused.status,
        refused.body
    );

    // The second Outpost's own, correct request is then served — it would be the
    // conflict of a task it cannot see if the refused call had stored its reference.
    let started = outposts
        .ok(
            "POST",
            &capacity_route(&second.outpost_id),
            Some(&request_naming(order_id, false, None)),
        )
        .await;
    let task_id = text(&started, "CapacityTaskId");

    // And it is where the caller looks for it: the list filtered to that Outpost.
    let reported = listed(
        &outposts,
        &format!(
            "?OutpostIdentifierFilter={}",
            query_value(&second.outpost_id)
        ),
    )
    .await;
    let ids: Vec<String> = reported
        .iter()
        .map(|summary| text(summary, "CapacityTaskId"))
        .collect();
    assert_eq!(
        ids,
        vec![task_id.clone()],
        "the task holding the order must be listed under the Outpost it runs on, \
         got {reported:?}"
    );
    assert_eq!(
        text(&reported[0], "OrderId"),
        *order_id,
        "the listed task carries the order it was started for, got {}",
        reported[0]
    );

    // The refused Outpost is holding nothing either: its own order starts its own
    // task.
    let own = outposts
        .ok(
            "POST",
            &capacity_route(&first.outpost_id),
            Some(&request_naming(&first.orders[0], false, None)),
        )
        .await;
    assert_ne!(
        text(&own, "CapacityTaskId"),
        task_id,
        "two tasks are two resources, with two ids, got {own}"
    );
    every_task_names_an_order_of_its_own_outpost(&outposts, "both Outposts running a task").await;
}
