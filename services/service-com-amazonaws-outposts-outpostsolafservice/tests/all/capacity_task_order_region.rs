// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the REGION the optional `OrderId` of a capacity task is
//! resolved in, driven over the wire through the same front door as
//! [`crate::site_and_outpost_lifecycle`] (whose harness this suite reuses) and
//! starting its tasks through [`crate::capacity_task_lifecycle`]'s helpers.
//!
//! The rule under test, stated once and then exercised over a SET of inputs: an
//! identifier a request names is resolved inside the region the request is addressed
//! to, because a resource belongs to the region it was created in and to no other —
//! so an `OrderId` on `StartCapacityTask` names an order of THIS region's inventory,
//! and one that names an order of another region is as absent as one nothing ever
//! minted: 404 `NotFoundException` worded as every refusal about an order this service
//! holds none of is,
//! and no task stored carrying it. The one-active-capacity-task-per-order rule reads
//! the same scope, so the 409 `ConflictException` it states is only ever about a task
//! of the inventory the caller's endpoint serves.
//!
//! The rows differ along the region the call is addressed to, the region the order was
//! placed in, why the id names nothing here (another region's order, an id nothing
//! minted), whether the call is a dry run, and which identifier form names the Outpost
//! (bare id, full ARN).

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::state::{
    capacity_task_conflict, order_not_found, RESOURCE_TYPE_ORDER,
};

use crate::capacity_task_lifecycle::{capacity_route, pool, task_route, ABSENT_ORDER_ID};
use crate::site_and_outpost_lifecycle::{segment, text, Outposts, Reply};

/// The region codes the rows are addressed to.
///
/// Source: the AWS region codes this emulator's callers sign for, spanning a
/// single-word direction (`us-east-1`, `eu-central-1`) and a compound one
/// (`ap-southeast-2`) — nothing about the rule depends on the form of a code, only on
/// the codes differing.
const REGIONS: [&str; 3] = ["us-east-1", "eu-central-1", "ap-southeast-2"];

/// The wire error code an identifier that names nothing in this region's inventory
/// arrives as, from `smithy_query(query="operation",operation="StartCapacityTask")`:
/// `NotFoundException`, HTTP 404.
const NOT_FOUND_CODE: &str = "NotFoundException";

/// See [`NOT_FOUND_CODE`] — the code a second task on one order arrives as, HTTP 409.
const CONFLICT_CODE: &str = "ConflictException";

/// What one region's endpoint created: two Outposts of a site of its own, and one
/// order placed there.
///
/// Two Outposts, because only one capacity task can be active per Outpost: the task a
/// case starts runs on the first, so a row about the ORDER rule has a second Outpost
/// of the same region to ask on.
struct Placed {
    region: &'static str,
    outpost_id: String,
    outpost_arn: String,
    spare_outpost_id: String,
    order_id: String,
}

impl Placed {
    /// The two identifier forms the modelled `OutpostIdentifier` pattern accepts for
    /// the Outpost a task runs on: the bare id and the full ARN.
    fn outpost_forms(&self) -> [(&'static str, &str); 2] {
        [
            ("bare id", self.outpost_id.as_str()),
            ("ARN", self.outpost_arn.as_str()),
        ]
    }
}

/// Create, through `region`'s endpoint, the inventory one region holds: a site, two
/// Outposts on it and an order placed on the first.
async fn place(outposts: &Outposts, region: &'static str) -> Placed {
    let site_id = outposts
        .site_id_in(region, &format!("{region}-capacity-order-site"))
        .await;
    let outpost = outposts
        .create_outpost_in(
            region,
            &json!({ "Name": format!("{region}-capacity-order"), "SiteId": site_id }),
        )
        .await;
    let spare = outposts
        .create_outpost_in(
            region,
            &json!({ "Name": format!("{region}-capacity-order-spare"), "SiteId": site_id }),
        )
        .await;
    let outpost_id = text(&outpost, "OutpostId");
    let order = outposts
        .ok_in(
            region,
            "POST",
            "/orders",
            Some(&json!({
                "OutpostIdentifier": outpost_id,
                "PaymentOption": "ALL_UPFRONT",
                // A catalog item SKU of the modelled `SkuCode` form
                // (`^OR-[A-Z0-9]{7}$`).
                "LineItems": [{ "CatalogItemId": "OR-1A2B3C4", "Quantity": 1 }],
            })),
        )
        .await["Order"]
        .clone();
    Placed {
        region,
        outpost_id,
        outpost_arn: text(&outpost, "OutpostArn"),
        spare_outpost_id: text(&spare, "OutpostId"),
        order_id: text(&order, "OrderId"),
    }
}

/// One region's inventory for every region of [`REGIONS`].
async fn inventory(outposts: &Outposts) -> Vec<Placed> {
    let mut placed = Vec::new();
    for region in REGIONS {
        placed.push(place(outposts, region).await);
    }
    placed
}

/// `StartCapacityTask` on `outpost_identifier`, addressed to `region`.
async fn start_in(
    outposts: &Outposts,
    region: &str,
    outpost_identifier: &str,
    request: &Value,
) -> Reply {
    outposts
        .call_in(
            region,
            "POST",
            &capacity_route(outpost_identifier),
            Some(request),
        )
        .await
}

/// A `StartCapacityTask` body naming `order_id`, as a real start or as a dry run.
fn request_naming(order_id: &str, dry_run: bool) -> Value {
    json!({
        "InstancePools": [pool("m5.4xlarge", 1)],
        "OrderId": order_id,
        "DryRun": dry_run,
    })
}

/// How many capacity tasks the store holds. A refused start must leave this where it
/// was: the dangling reference the resolution exists to prevent is one that got
/// PERSISTED.
fn tasks_held(outposts: &Outposts) -> usize {
    outposts.store().lock().capacity_tasks.len()
}

/// The rule: an `OrderId` is resolved in the region the request is addressed to, so an
/// order of another region is refused exactly as an id nothing minted is, and the
/// order of the request's own region is accepted and read back.
#[tokio::test]
async fn a_capacity_task_resolves_its_order_id_in_the_request_region() {
    let outposts = Outposts::new();
    let placed = inventory(&outposts).await;

    for request in &placed {
        // Every id that names no order of THIS region: each other region's order, and
        // an id nothing ever minted anywhere — the two reasons an id resolves to
        // nothing here, which must read the same to the caller.
        let mut absent: Vec<(String, String)> = vec![(
            "an OrderId this emulator never minted".to_string(),
            ABSENT_ORDER_ID.to_string(),
        )];
        for holder in &placed {
            if holder.region == request.region {
                continue;
            }
            absent.push((
                format!("an order placed in {}", holder.region),
                holder.order_id.clone(),
            ));
        }

        for (reason, order_id) in &absent {
            for (form, outpost_identifier) in request.outpost_forms() {
                for dry_run in [false, true] {
                    let case = format!(
                        "{reason}, named through {}'s endpoint on the Outpost's {form}, \
                         DryRun={dry_run}",
                        request.region
                    );
                    let reply = start_in(
                        &outposts,
                        request.region,
                        outpost_identifier,
                        &request_naming(order_id, dry_run),
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
                        "case {case:?}: expected the order not-found wording for the id \
                         supplied, got {}",
                        reply.body
                    );
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

    // The accepting side: in the region that placed the order, the very same id is
    // accepted, reported back as the task's own, and — the point of the scope — is an
    // id this region's endpoint still serves, so the task carries no reference the
    // caller cannot follow.
    for request in &placed {
        let started = start_in(
            &outposts,
            request.region,
            &request.outpost_id,
            &request_naming(&request.order_id, false),
        )
        .await;
        assert_eq!(
            (started.status, started.error_code.as_str()),
            (200, ""),
            "case {:?}: the region that placed the order serves the start, got {} {} {}",
            request.region,
            started.status,
            started.error_code,
            started.body
        );
        assert_eq!(
            text(&started.body, "OrderId"),
            request.order_id,
            "case {:?}: the resolved order is reported as the task's own OrderId, got {}",
            request.region,
            started.body
        );
        // The round trip: the read answers the persisted task, carrying that same id.
        let task_id = text(&started.body, "CapacityTaskId");
        let read = outposts
            .ok_in(
                request.region,
                "GET",
                &task_route(&request.outpost_id, &task_id),
                None,
            )
            .await;
        assert_eq!(
            text(&read, "OrderId"),
            request.order_id,
            "case {:?}: GetCapacityTask must answer the persisted OrderId, got {read}",
            request.region
        );
        // And the order that id names is reachable through the same endpoint, which is
        // what a cross-region reference would have broken.
        let order = outposts
            .call_in(
                request.region,
                "GET",
                &format!("/orders/{}", segment(&request.order_id)),
                None,
            )
            .await;
        assert_eq!(
            (order.status, text(&order.body["Order"], "OrderId")),
            (200, request.order_id.clone()),
            "case {:?}: the OrderId a task carries must name an order this endpoint \
             serves, got {} {}",
            request.region,
            order.status,
            order.body
        );
    }
}

/// The rule, stated for the one-active-task-per-order refusal: the task it is about is
/// a task of the region the request is addressed to, so a start naming another
/// region's order is the not-found of an absent id and never a conflict about a
/// resource this endpoint does not serve.
#[tokio::test]
async fn the_one_active_task_per_order_rule_is_about_this_regions_order() {
    let outposts = Outposts::new();
    let placed = inventory(&outposts).await;

    // One active task per region, each on that region's own order.
    let mut active = Vec::new();
    for request in &placed {
        let started = outposts
            .ok_in(
                request.region,
                "POST",
                &capacity_route(&request.outpost_id),
                Some(&request_naming(&request.order_id, false)),
            )
            .await;
        active.push(text(&started, "CapacityTaskId"));
    }

    for (request, active_task) in placed.iter().zip(&active) {
        // Inside the region, the rule holds: a second task naming the same order, on
        // another Outpost of that region, is the modelled conflict naming the order.
        let reply = start_in(
            &outposts,
            request.region,
            &request.spare_outpost_id,
            &request_naming(&request.order_id, false),
        )
        .await;
        let case = format!("a second task on {}'s own order", request.region);
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (409, CONFLICT_CODE),
            "case {case:?}: expected 409 {CONFLICT_CODE}, got {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
        assert_eq!(
            (
                text(&reply.body, "ResourceId"),
                text(&reply.body, "ResourceType"),
                text(&reply.body, "Message"),
            ),
            (
                request.order_id.clone(),
                RESOURCE_TYPE_ORDER.to_string(),
                capacity_task_conflict(RESOURCE_TYPE_ORDER, &request.order_id, active_task),
            ),
            "case {case:?}: the conflict must name this region's order and the task \
             holding it, got {}",
            reply.body
        );

        // Across regions, the same request is the not-found of an id this inventory
        // holds no order for — not a conflict, and nothing naming the other region's
        // order or task.
        for holder in &placed {
            if holder.region == request.region {
                continue;
            }
            let case = format!(
                "{}'s order, with a task active on it there, named through {}'s endpoint",
                holder.region, request.region
            );
            let reply = start_in(
                &outposts,
                request.region,
                &request.spare_outpost_id,
                &request_naming(&holder.order_id, false),
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
                order_not_found(&holder.order_id),
                "case {case:?}: expected the order not-found wording for the id supplied, \
                 got {}",
                reply.body
            );
            assert!(
                reply.body.get("ResourceId").is_none()
                    && reply.body.get("ResourceType").is_none(),
                "case {case:?}: a refusal must not name a resource this endpoint does \
                 not serve, got {}",
                reply.body
            );
        }
    }

    // Only the tasks the accepted starts created are held: every refusal above stored
    // nothing.
    let held = tasks_held(&outposts);
    assert_eq!(
        held,
        placed.len(),
        "expected {} capacity tasks held, got {held}",
        placed.len()
    );
}
