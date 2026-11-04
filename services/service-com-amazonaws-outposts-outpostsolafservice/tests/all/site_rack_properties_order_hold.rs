// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the limit `UpdateSiteRackPhysicalProperties` documents on an
//! order in progress, driven over the wire through the same front door as
//! [`crate::site_and_outpost_lifecycle`] (whose harness this suite reuses), placing its
//! orders through [`crate::capacity_task_lifecycle`]'s helper.
//!
//! The rule under test, stated once and then exercised over a SET of inputs: while an
//! order placed at a site is still OUTSTANDING — in any `OrderStatus` other than the
//! two in which an order is finished — a change to that site's rack physical properties
//! is refused 409 `ConflictException` naming the order in `ResourceId` /
//! `ResourceType`, and nothing is written; once no order at the site is outstanding the
//! change goes through and reads back through `GetSite`.
//!
//! The rows differ along the `OrderStatus` the order is in (every value the modelled
//! enum declares, outstanding and finished), how many rack properties the request
//! carries (none, one, two, all nine) and which values they carry, the identifier form
//! the request names the site by (bare id and full ARN), whether the site already held
//! rack properties, whether the order's Outpost is still stored, and which site the
//! order was placed at.
//!
//! What [`crate::site_address_order_hold`] pins instead is the same limit on the OTHER
//! change to a site that documents it, and [`crate::update_persistence`] pins what a
//! rack-properties change writes when no order holds it up.

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::state::{
    site_rack_properties_order_conflict, ORDER_STATUS_PREPARING, ORDER_STATUS_SETTLED,
    RESOURCE_TYPE_ORDER,
};

use crate::capacity_task_lifecycle::an_order;
use crate::site_address_order_hold::{set_order_status, ORDER_STATUSES};
use crate::site_and_outpost_lifecycle::{address, segment, text, Outposts, Reply, HOME_REGION};
use crate::update_persistence::{overlay, RACK_PROPERTY_VALUES};

/// The wire error code a conflict arrives as, from
/// `smithy_query(query="operation",operation="UpdateSiteRackPhysicalProperties")`:
/// `ConflictException`, HTTP 409.
const CONFLICT_CODE: &str = "ConflictException";

/// `PATCH /sites/{SiteId}/rackPhysicalProperties`, as the reply arrived.
async fn update_rack_properties(outposts: &Outposts, identifier: &str, patch: &Value) -> Reply {
    outposts
        .call(
            "PATCH",
            &format!("/sites/{}/rackPhysicalProperties", segment(identifier)),
            Some(patch),
        )
        .await
}

/// The `RackPhysicalProperties` `GetSite` reports for the site, absent when it holds
/// none.
async fn stored_properties(outposts: &Outposts, site_id: &str) -> Option<Value> {
    outposts
        .get_site(site_id)
        .await
        .get("RackPhysicalProperties")
        .cloned()
}

/// Every rack property at its FIRST modelled value, which is what a site of this suite
/// is created with, so a row that writes a different value overwrites a stored one.
fn properties_at_create() -> Value {
    RACK_PROPERTY_VALUES
        .iter()
        .map(|(member, values)| ((*member).to_string(), json!(values[0])))
        .collect::<serde_json::Map<String, Value>>()
        .into()
}

/// The rack-properties requests the rows are built over: none, one, two and all nine
/// properties, each at a value the site was not created with, so an accepted change is
/// visibly different from the stored one and a refused change is visibly absent.
///
/// The values come from [`RACK_PROPERTY_VALUES`], whose source is this service's model.
fn rack_patches() -> Vec<(&'static str, Value)> {
    vec![
        ("no property at all", json!({})),
        ("one property", json!({ "PowerDrawKva": "POWER_30_KVA" })),
        (
            "the uplink pair",
            json!({ "UplinkGbps": "UPLINK_40G", "UplinkCount": "UPLINK_COUNT_4" }),
        ),
        (
            "all nine properties",
            RACK_PROPERTY_VALUES
                .iter()
                .map(|(member, values)| {
                    ((*member).to_string(), json!(values[values.len() - 1]))
                })
                .collect::<serde_json::Map<String, Value>>()
                .into(),
        ),
    ]
}

/// A site with every rack property already stored, an Outpost at it and an order placed
/// on that Outpost: `(SiteId, SiteArn, OutpostId, OrderId)`.
///
/// The properties are set at creation, BEFORE the order exists, so a row that asserts
/// nothing was written has a stored value of each property to compare against.
async fn site_with_an_order(outposts: &Outposts, name: &str) -> (String, String, String, String) {
    let site = outposts
        .create_site(&json!({
            "Name": name,
            "OperatingAddress": address("US", "WA", "Seattle"),
            // An order is placed at this site, and an order can only be placed at a
            // site whose details are complete ([`crate::precondition_gates`]).
            "ShippingAddress": address("US", "WA", "Kirkland"),
            "RackPhysicalProperties": properties_at_create(),
        }))
        .await;
    let site_id = text(&site, "SiteId");
    let site_arn = text(&site, "SiteArn");
    let outpost = outposts
        .create_outpost(&json!({ "Name": name, "SiteId": site_id }))
        .await;
    let outpost_id = text(&outpost, "OutpostId");
    let order_id = an_order(outposts, &outpost_id).await;
    (site_id, site_arn, outpost_id, order_id)
}

/// The rule over the whole modelled `OrderStatus` set and every shape of
/// rack-properties request: an outstanding order at the site refuses the change naming
/// that order and writes nothing; a finished one holds nothing up.
#[tokio::test]
async fn an_order_in_progress_at_the_site_holds_a_rack_properties_change() {
    for status in ORDER_STATUSES {
        let outstanding = !ORDER_STATUS_SETTLED.contains(&status);
        for (shape, patch) in rack_patches() {
            let case = format!("{status} order, {shape}");
            // One store per row, so the order of the rows cannot matter.
            let outposts = Outposts::new();
            let (site_id, site_arn, _, order_id) =
                site_with_an_order(&outposts, "ordered site").await;
            set_order_status(&outposts, &order_id, status, &case);
            let before = stored_properties(&outposts, &site_id).await;

            if !outstanding {
                // A finished order holds nothing up — and neither does the Outpost
                // still at the site, which this operation's contract says nothing
                // about.
                let reply = update_rack_properties(&outposts, &site_id, &patch).await;
                assert_eq!(
                    reply.status, 200,
                    "case {case:?}: the change must be accepted once no order is outstanding, got {} {:?}",
                    reply.status, reply.body
                );
                let expected = overlay(before.as_ref().unwrap_or(&json!({})), &patch);
                let read = stored_properties(&outposts, &site_id).await;
                assert_eq!(
                    read.as_ref(),
                    Some(&expected),
                    "case {case:?}: the accepted change must read back, got {read:?}"
                );
                continue;
            }

            // Both identifier forms are refused the same way: the conflict is about
            // the order at the site, not about how the request spelled the site.
            for named_by in [site_id.as_str(), site_arn.as_str()] {
                let reply = update_rack_properties(&outposts, named_by, &patch).await;
                assert_eq!(
                    (reply.status, reply.error_code.as_str()),
                    (409, CONFLICT_CODE),
                    "case {case:?} named by {named_by:?}: a rack-properties change must be refused while an order is in progress, got {} {:?}",
                    reply.status,
                    reply.body
                );
                assert_eq!(
                    (
                        reply.body.get("ResourceType").cloned(),
                        reply.body.get("ResourceId").cloned(),
                        reply.body.get("Message").cloned(),
                    ),
                    (
                        Some(json!(RESOURCE_TYPE_ORDER)),
                        Some(json!(order_id)),
                        Some(json!(site_rack_properties_order_conflict(
                            &site_id,
                            &format!("order {order_id}")
                        ))),
                    ),
                    "case {case:?} named by {named_by:?}: the conflict must name the order holding the change up, got {:?}",
                    reply.body
                );
            }
            let read = stored_properties(&outposts, &site_id).await;
            assert_eq!(
                read, before,
                "case {case:?}: a refused change must write nothing, got {read:?}"
            );
        }
    }
}

/// The hold is on the order AT THE SITE: it does not depend on the Outpost the order
/// was placed on still being stored, and it is released by cancelling the order.
#[tokio::test]
async fn the_hold_follows_the_order_not_the_outpost_record() {
    let outposts = Outposts::new();
    let (site_id, _, outpost_id, order_id) =
        site_with_an_order(&outposts, "site whose Outpost goes").await;
    // The Outpost record is removed through the store seam: the delete route refuses
    // it while the order is outstanding, which is exactly the state this case is
    // about — an order still at the site with no Outpost record naming it.
    assert!(
        outposts
            .store()
            .lock()
            .remove_outpost(HOME_REGION, &outpost_id)
            .is_some(),
        "the Outpost {outpost_id} this case created is stored"
    );
    let before = stored_properties(&outposts, &site_id).await;
    for (shape, patch) in rack_patches() {
        let case = format!("{shape} with the order's Outpost gone");
        let reply = update_rack_properties(&outposts, &site_id, &patch).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (409, CONFLICT_CODE),
            "case {case:?}: the outstanding order must still hold the change up, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.body.get("ResourceId").cloned(),
            Some(json!(order_id)),
            "case {case:?}: the conflict must name the order, got {:?}",
            reply.body
        );
    }
    let read = stored_properties(&outposts, &site_id).await;
    assert_eq!(
        read, before,
        "a refused change must write nothing, got {read:?}"
    );

    // Cancelling the order — over the wire — is what releases the hold.
    let cancelled = outposts
        .call("POST", &format!("/orders/{order_id}/cancel"), None)
        .await;
    assert_eq!(
        cancelled.status, 200,
        "the order must be cancellable, got {} {:?}",
        cancelled.status, cancelled.body
    );
    let mut expected = before.unwrap_or_else(|| json!({}));
    for (shape, patch) in rack_patches() {
        let case = format!("{shape} after the order was cancelled");
        let reply = update_rack_properties(&outposts, &site_id, &patch).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: the change must be accepted once the order is cancelled, got {} {:?}",
            reply.status, reply.body
        );
        expected = overlay(&expected, &patch);
        let read = stored_properties(&outposts, &site_id).await;
        assert_eq!(
            read.as_ref(),
            Some(&expected),
            "case {case:?}: the accepted change must read back, got {read:?}"
        );
    }
}

/// The hold is scoped to the site the order was placed at: an order at another site
/// holds nothing here, and the site it IS at is still held — including a site that held
/// no rack properties at all before the call.
#[tokio::test]
async fn an_order_at_another_site_holds_nothing_at_this_one() {
    let outposts = Outposts::new();
    let (ordered_site, _, _, order_id) = site_with_an_order(&outposts, "the ordered site").await;
    let quiet = outposts
        .create_site(&json!({ "Name": "the quiet site" }))
        .await;
    let quiet_site = text(&quiet, "SiteId");
    assert!(
        stored_properties(&outposts, &quiet_site).await.is_none(),
        "the quiet site is created with no rack properties at all, got {quiet:?}"
    );

    // What the quiet site holds after each accepted row: nothing until a row carries a
    // property, since a request carrying none writes nothing at all — the structure
    // included, which a site created without one is still without.
    let mut expected: Option<Value> = None;
    for (shape, patch) in rack_patches() {
        let case = format!("{shape} at a site with no order of its own");
        let reply = update_rack_properties(&outposts, &quiet_site, &patch).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: another site's order must hold nothing up here, got {} {:?}",
            reply.status, reply.body
        );
        if patch.as_object().is_some_and(|patch| !patch.is_empty()) {
            expected = Some(overlay(expected.as_ref().unwrap_or(&json!({})), &patch));
        }
        let read = stored_properties(&outposts, &quiet_site).await;
        assert_eq!(
            read.as_ref(),
            expected.as_ref(),
            "case {case:?}: the accepted change must read back, got {read:?}"
        );

        let case = format!("{shape} at the site the order was placed at");
        let reply = update_rack_properties(&outposts, &ordered_site, &patch).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (409, CONFLICT_CODE),
            "case {case:?}: the site the order is at must still be held, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.body.get("ResourceId").cloned(),
            Some(json!(order_id)),
            "case {case:?}: the conflict must name that order, got {:?}",
            reply.body
        );
    }
}

/// A just-placed order is outstanding without the clock having to move it: the status
/// `CreateOrder` reports is one the hold already covers, so no row above depends on the
/// store seam to produce the common case.
#[tokio::test]
async fn a_just_placed_order_holds_the_change_without_any_seam() {
    let outposts = Outposts::new();
    let (site_id, _, _, order_id) = site_with_an_order(&outposts, "freshly ordered site").await;
    let order = outposts
        .ok("GET", &format!("/orders/{order_id}"), None)
        .await["Order"]
        .clone();
    assert_eq!(
        text(&order, "Status"),
        ORDER_STATUS_PREPARING,
        "a just-placed order must report the status the submit gave it, got {order:?}"
    );
    let reply = update_rack_properties(
        &outposts,
        &site_id,
        &json!({ "PowerDrawKva": "POWER_15_KVA" }),
    )
    .await;
    assert_eq!(
        (reply.status, reply.error_code.as_str()),
        (409, CONFLICT_CODE),
        "a just-placed order must hold a rack-properties change up, got {} {:?}",
        reply.status,
        reply.body
    );
}
