// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the limit `UpdateSiteAddress` documents on an order in
//! progress, driven over the wire through the same front door as
//! [`crate::site_and_outpost_lifecycle`] (whose harness this suite reuses), placing
//! its orders through [`crate::capacity_task_lifecycle`]'s helper.
//!
//! The rule under test, stated once and then exercised over a SET of inputs: while an
//! order placed at a site is still OUTSTANDING — in any `OrderStatus` other than the
//! two in which an order is finished — a change to that site's address of EITHER type
//! is refused 409 `ConflictException` naming the order in `ResourceId` /
//! `ResourceType`, and nothing is written; once no order at the site is outstanding
//! the change goes through and reads back. The hold is on the ORDER at the site, so it
//! does not depend on the Outpost the order was placed on still being stored, and an
//! order at another site holds nothing at this one.
//!
//! The rows differ along the `AddressType` the change names (each modelled value), the
//! `OrderStatus` the order is in (every value the modelled enum declares, outstanding
//! and finished), the identifier form the request names the site by (bare id and full
//! ARN), whether the site already held an address of that type, whether the order's
//! Outpost is still stored, and which site the order was placed at.
//!
//! What [`crate::site_address`] pins instead is the SECOND limit the same operation
//! documents, on the operating address while an Outpost belongs to the site.

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::state::{
    site_address_order_conflict, ORDER_STATUS_PREPARING, ORDER_STATUS_SETTLED, RESOURCE_TYPE_ORDER,
};

use crate::capacity_task_lifecycle::an_order;
use crate::site_and_outpost_lifecycle::{address, query_value, segment, text, Outposts, HOME_REGION};

/// Every value the modelled `AddressType` enum declares, which is every address a
/// site holds and so every one the hold has to cover.
///
/// Source: `smithy_query(query="shape", shape="AddressType")` against this service's
/// model — `["SHIPPING_ADDRESS", "OPERATING_ADDRESS"]`.
const ADDRESS_TYPES: [&str; 2] = ["OPERATING_ADDRESS", "SHIPPING_ADDRESS"];

/// Every value the modelled `OrderStatus` enum declares.
///
/// Source: `smithy_query(query="shape", shape="OrderStatus")` — `RECEIVED`, `PENDING`,
/// `PROCESSING`, `INSTALLING`, `FULFILLED`, `CANCELLED`, `PREPARING`, `IN_PROGRESS`,
/// `DELIVERED`, `COMPLETED`, `ERROR`. A row is built for each, so the two in which an
/// order is finished (`ORDER_STATUS_SETTLED`) and the nine in which it is still
/// outstanding are all covered, including the ones no call of this API reaches.
pub(crate) const ORDER_STATUSES: [&str; 11] = [
    "RECEIVED",
    "PENDING",
    "PROCESSING",
    "INSTALLING",
    "FULFILLED",
    "CANCELLED",
    "PREPARING",
    "IN_PROGRESS",
    "DELIVERED",
    "COMPLETED",
    "ERROR",
];

/// The wire error code a conflict arrives as, from
/// `smithy_query(query="operation",operation="UpdateSiteAddress")`: `ConflictException`,
/// HTTP 409.
const CONFLICT_CODE: &str = "ConflictException";

/// `PUT /sites/{SiteId}/address`, as the reply arrived.
async fn update_site_address(
    outposts: &Outposts,
    identifier: &str,
    address_type: &str,
    address: &Value,
) -> crate::site_and_outpost_lifecycle::Reply {
    outposts
        .call(
            "PUT",
            &format!("/sites/{}/address", segment(identifier)),
            Some(&json!({ "AddressType": address_type, "Address": address })),
        )
        .await
}

/// The address `GET /sites/{SiteId}/address?AddressType=…` answers for `address_type`,
/// absent when the site holds none of that type.
async fn stored_address(outposts: &Outposts, site_id: &str, address_type: &str) -> Option<Value> {
    outposts
        .ok(
            "GET",
            &format!(
                "/sites/{}/address?AddressType={}",
                segment(site_id),
                query_value(address_type)
            ),
            None,
        )
        .await
        .get("Address")
        .cloned()
}

/// Put a stored order in `status`.
///
/// Most of the modelled `OrderStatus` values are reached by hardware being built,
/// shipped and installed over spans no case may sleep for, so a case about what a
/// status HOLDS UP reaches it through the same store seam
/// [`crate::order_cancellation`] finishes an order through. The transitions the clock
/// still owed go with it, so the status the row set is the one the next call reads.
/// Every assertion still goes over the wire.
pub(crate) fn set_order_status(outposts: &Outposts, order_id: &str, status: &str, case: &str) {
    let store = outposts.store();
    let mut store = store.lock();
    let record = store
        .orders
        .get_mut(order_id)
        .unwrap_or_else(|| panic!("case {case:?}: the order {order_id} this case placed is stored"));
    record.order.status = Some(status.to_string());
    record.pending.clear();
}

/// A site with a shipping address already stored, an Outpost at it and an order placed
/// on that Outpost: `(SiteId, SiteArn, OutpostId, OrderId)`.
///
/// The shipping address is written BEFORE the order exists, so a row that asserts
/// nothing was written has a stored address of either type to compare against.
async fn site_with_an_order(outposts: &Outposts, name: &str) -> (String, String, String, String) {
    let site = outposts
        .create_site(&json!({ "Name": name, "OperatingAddress": address("US", "WA", "Seattle") }))
        .await;
    let site_id = text(&site, "SiteId");
    let site_arn = text(&site, "SiteArn");
    let shipping = update_site_address(
        outposts,
        &site_id,
        "SHIPPING_ADDRESS",
        &address("US", "OR", "Portland"),
    )
    .await;
    assert_eq!(
        shipping.status, 200,
        "case {name:?}: the shipping address must be writable before any order exists, got {} {:?}",
        shipping.status, shipping.body
    );
    let outpost = outposts
        .create_outpost(&json!({ "Name": name, "SiteId": site_id }))
        .await;
    let outpost_id = text(&outpost, "OutpostId");
    let order_id = an_order(outposts, &outpost_id).await;
    (site_id, site_arn, outpost_id, order_id)
}

/// The rule over the whole modelled `OrderStatus` set and both address types: an
/// outstanding order at the site refuses the change naming that order and writes
/// nothing; a finished one holds nothing up.
#[tokio::test]
async fn an_order_in_progress_at_the_site_holds_a_change_to_either_address() {
    let attempt = address("IE", "Leinster", "Dublin");
    for status in ORDER_STATUSES {
        let outstanding = !ORDER_STATUS_SETTLED.contains(&status);
        for address_type in ADDRESS_TYPES {
            let case = format!("{status} order, {address_type}");
            // One store per row, so the order of the rows cannot matter.
            let outposts = Outposts::new();
            let (site_id, site_arn, outpost_id, order_id) =
                site_with_an_order(&outposts, "ordered site").await;
            set_order_status(&outposts, &order_id, status, &case);
            let before = stored_address(&outposts, &site_id, address_type).await;

            if !outstanding {
                // A finished order holds nothing up. The Outpost of the site is
                // removed first, because an Outpost at the site is the OTHER limit
                // the operation documents (see [`crate::site_address`]) and this row
                // is about the order.
                let deleted = outposts
                    .call("DELETE", &format!("/outposts/{outpost_id}"), None)
                    .await;
                assert_eq!(
                    deleted.status, 200,
                    "case {case:?}: a finished order must not hold the Outpost delete up, got {} {:?}",
                    deleted.status, deleted.body
                );
                let reply = update_site_address(&outposts, &site_id, address_type, &attempt).await;
                assert_eq!(
                    reply.status, 200,
                    "case {case:?}: the change must be accepted once no order is outstanding, got {} {:?}",
                    reply.status, reply.body
                );
                let read = stored_address(&outposts, &site_id, address_type).await;
                assert_eq!(
                    read.as_ref(),
                    Some(&attempt),
                    "case {case:?}: the accepted change must read back, got {read:?}"
                );
                continue;
            }

            // Both identifier forms are refused the same way: the conflict is about
            // the order at the site, not about how the request spelled the site.
            for named_by in [site_id.as_str(), site_arn.as_str()] {
                let reply = update_site_address(&outposts, named_by, address_type, &attempt).await;
                assert_eq!(
                    (reply.status, reply.error_code.as_str()),
                    (409, CONFLICT_CODE),
                    "case {case:?} named by {named_by:?}: an address change must be refused while an order is in progress, got {} {:?}",
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
                        Some(json!(site_address_order_conflict(
                            &site_id,
                            &format!("order {order_id}")
                        ))),
                    ),
                    "case {case:?} named by {named_by:?}: the conflict must name the order holding the change up, got {:?}",
                    reply.body
                );
            }
            let read = stored_address(&outposts, &site_id, address_type).await;
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
    let attempt = address("JP", "Tokyo", "Minato");
    for address_type in ADDRESS_TYPES {
        let case = format!("{address_type} with the order's Outpost gone");
        let reply = update_site_address(&outposts, &site_id, address_type, &attempt).await;
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
    // Cancelling the order — over the wire — is what releases the hold.
    let cancelled = outposts
        .call("POST", &format!("/orders/{order_id}/cancel"), None)
        .await;
    assert_eq!(
        cancelled.status, 200,
        "the order must be cancellable, got {} {:?}",
        cancelled.status, cancelled.body
    );
    for address_type in ADDRESS_TYPES {
        let case = format!("{address_type} after the order was cancelled");
        let reply = update_site_address(&outposts, &site_id, address_type, &attempt).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: the change must be accepted once the order is cancelled, got {} {:?}",
            reply.status, reply.body
        );
        let read = stored_address(&outposts, &site_id, address_type).await;
        assert_eq!(
            read.as_ref(),
            Some(&attempt),
            "case {case:?}: the accepted change must read back, got {read:?}"
        );
    }
}

/// The hold is scoped to the site the order was placed at: an order at another site
/// holds nothing here, and the site it IS at is still held.
#[tokio::test]
async fn an_order_at_another_site_holds_nothing_at_this_one() {
    let outposts = Outposts::new();
    let (ordered_site, _, _, order_id) = site_with_an_order(&outposts, "the ordered site").await;
    let quiet = outposts
        .create_site(&json!({
            "Name": "the quiet site",
            "OperatingAddress": address("US", "WA", "Seattle"),
        }))
        .await;
    let quiet_site = text(&quiet, "SiteId");
    let attempt = address("DE", "Hesse", "Frankfurt");

    for address_type in ADDRESS_TYPES {
        let case = format!("{address_type} at a site with no order of its own");
        let reply = update_site_address(&outposts, &quiet_site, address_type, &attempt).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: another site's order must hold nothing up here, got {} {:?}",
            reply.status, reply.body
        );
        let read = stored_address(&outposts, &quiet_site, address_type).await;
        assert_eq!(
            read.as_ref(),
            Some(&attempt),
            "case {case:?}: the accepted change must read back, got {read:?}"
        );

        let case = format!("{address_type} at the site the order was placed at");
        let reply = update_site_address(&outposts, &ordered_site, address_type, &attempt).await;
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
/// `CreateOrder` reports is one the hold already covers, so no row above depends on
/// the store seam to produce the common case.
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
    let reply = update_site_address(
        &outposts,
        &site_id,
        "SHIPPING_ADDRESS",
        &address("GB", "England", "London"),
    )
    .await;
    assert_eq!(
        (reply.status, reply.error_code.as_str()),
        (409, CONFLICT_CODE),
        "a just-placed order must hold a shipping-address change up, got {} {:?}",
        reply.status,
        reply.body
    );
}
