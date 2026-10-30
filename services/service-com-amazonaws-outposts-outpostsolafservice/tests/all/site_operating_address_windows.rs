// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the TWO windows in which `UpdateSiteAddress` accepts a change
//! to a site's OPERATING address, driven over the wire through the same front door as
//! [`crate::site_and_outpost_lifecycle`] (whose harness this suite reuses).
//!
//! The rule under test, stated once and then exercised over a SET of inputs: the
//! modelled sentence "You can update the operating address before you place an order at
//! the site, or after all Outposts that belong to the site have been deactivated"
//! (`smithy_query(query="doc", shape="UpdateSiteAddress")`) names two windows in which
//! the change is accepted — while no order has been placed at the site, whatever
//! Outposts belong to it, and once no Outpost belongs to the site any more — and the
//! change is refused 409 `ConflictException` naming an Outpost of the site only OUTSIDE
//! both of them: an order has been placed at that site AND an Outpost still belongs to
//! it. A refusal writes nothing. The narrower limit the same operation documents ("You
//! can't update a site address if there is an order in progress") is reported first and
//! names the order instead, which is [`crate::site_address_order_hold`]'s rule.
//!
//! The rows differ along whether an order has been placed at the site and in which
//! status it now stands (none at all, one at another site, one cancelled, one carried
//! through to completion, one still in progress), how many Outposts belong to the site
//! (none, one, three), whether those Outposts are gone again, and the identifier form
//! the request names the site by (bare id, full ARN).

use serde_json::{json, Value};

use service_com_amazonaws_outposts_outpostsolafservice::state::{
    site_address_conflict, site_address_order_conflict, RESOURCE_TYPE_ORDER, RESOURCE_TYPE_OUTPOST,
};

use crate::capacity_task_lifecycle::an_order;
use crate::site_and_outpost_lifecycle::{
    a_cancelled_order_in, address, install_hardware, query_value, segment, text, Outposts,
    HOME_REGION,
};

/// The `AddressType` naming the address a site is at — the only one either window is
/// about.
///
/// Source: `smithy_query(query="shape", shape="AddressType")` — the modelled value set
/// is `["SHIPPING_ADDRESS", "OPERATING_ADDRESS"]`, and what holds a change to the
/// SHIPPING one up is only the order in progress ([`crate::site_address`]).
const OPERATING: &str = "OPERATING_ADDRESS";

/// The wire error code a conflict arrives as, from
/// `smithy_query(query="operation",operation="UpdateSiteAddress")`: `ConflictException`,
/// HTTP 409.
const CONFLICT_CODE: &str = "ConflictException";

/// What a row has done about an order before it asks for the address change: the fact
/// that decides whether the site is still inside the first window.
#[derive(Clone, Copy, Debug)]
enum Placed {
    /// No order anywhere in the store, so no order has been placed at the site.
    Nowhere,
    /// An order placed at ANOTHER site and still in progress: an order was placed,
    /// but not at this site.
    AtAnotherSite,
    /// An order placed at this site and then cancelled over the wire.
    HereCancelled,
    /// An order placed at this site and carried through to completion.
    HereCompleted,
    /// An order placed at this site and still in progress.
    HereInProgress,
}

/// How the request the row makes must be answered.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Expect {
    /// Inside one of the two windows: the change is accepted and reads back.
    Accepted,
    /// Outside both: 409 naming an Outpost that belongs to the site.
    HeldByAnOutpost,
    /// The narrower limit: 409 naming the order that is still in progress.
    HeldByTheOrder,
}

/// Which identifier form the request names the site by — the modelled `SiteId` accepts
/// the bare id or the full site ARN, and neither is what the windows are about.
#[derive(Clone, Copy, Debug)]
enum NameBy {
    Id,
    Arn,
}

/// One row of the rule.
#[derive(Clone, Copy, Debug)]
struct Case {
    case: &'static str,
    placed: Placed,
    /// How many Outposts the row creates at the site.
    outposts: usize,
    /// Whether every one of those Outposts is deleted again before the change is
    /// asked for — this store's record of an Outpost of the site having been
    /// deactivated.
    deactivated: bool,
    name_by: NameBy,
    expect: Expect,
}

/// Every row, spanning the two windows and the state just outside both.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "no order placed, one Outpost at the site, named by id",
            placed: Placed::Nowhere,
            outposts: 1,
            deactivated: false,
            name_by: NameBy::Id,
            expect: Expect::Accepted,
        },
        Case {
            case: "no order placed, one Outpost at the site, named by ARN",
            placed: Placed::Nowhere,
            outposts: 1,
            deactivated: false,
            name_by: NameBy::Arn,
            expect: Expect::Accepted,
        },
        Case {
            case: "no order placed, three Outposts at the site",
            placed: Placed::Nowhere,
            outposts: 3,
            deactivated: false,
            name_by: NameBy::Id,
            expect: Expect::Accepted,
        },
        Case {
            case: "no order placed, no Outpost at the site",
            placed: Placed::Nowhere,
            outposts: 0,
            deactivated: false,
            name_by: NameBy::Id,
            expect: Expect::Accepted,
        },
        Case {
            case: "an order in progress at another site, one Outpost at this one",
            placed: Placed::AtAnotherSite,
            outposts: 1,
            deactivated: false,
            name_by: NameBy::Arn,
            expect: Expect::Accepted,
        },
        Case {
            case: "a cancelled order at the site, one Outpost still at it, named by id",
            placed: Placed::HereCancelled,
            outposts: 1,
            deactivated: false,
            name_by: NameBy::Id,
            expect: Expect::HeldByAnOutpost,
        },
        Case {
            case: "a cancelled order at the site, one Outpost still at it, named by ARN",
            placed: Placed::HereCancelled,
            outposts: 1,
            deactivated: false,
            name_by: NameBy::Arn,
            expect: Expect::HeldByAnOutpost,
        },
        Case {
            case: "a cancelled order at the site, three Outposts still at it",
            placed: Placed::HereCancelled,
            outposts: 3,
            deactivated: false,
            name_by: NameBy::Id,
            expect: Expect::HeldByAnOutpost,
        },
        Case {
            case: "a completed order at the site, one Outpost still at it",
            placed: Placed::HereCompleted,
            outposts: 1,
            deactivated: false,
            name_by: NameBy::Id,
            expect: Expect::HeldByAnOutpost,
        },
        Case {
            case: "a cancelled order at the site, its Outposts deactivated",
            placed: Placed::HereCancelled,
            outposts: 3,
            deactivated: true,
            name_by: NameBy::Id,
            expect: Expect::Accepted,
        },
        Case {
            case: "a completed order at the site, its Outpost deactivated",
            placed: Placed::HereCompleted,
            outposts: 1,
            deactivated: true,
            name_by: NameBy::Arn,
            expect: Expect::Accepted,
        },
        Case {
            case: "an order still in progress at the site, one Outpost at it",
            placed: Placed::HereInProgress,
            outposts: 1,
            deactivated: false,
            name_by: NameBy::Id,
            expect: Expect::HeldByTheOrder,
        },
    ]
}

/// `PUT /sites/{SiteId}/address` on the operating address, as the reply arrived.
async fn update_operating_address(
    outposts: &Outposts,
    identifier: &str,
    address: &Value,
) -> crate::site_and_outpost_lifecycle::Reply {
    outposts
        .call(
            "PUT",
            &format!("/sites/{}/address", segment(identifier)),
            Some(&json!({ "AddressType": OPERATING, "Address": address })),
        )
        .await
}

/// The operating address `GET /sites/{SiteId}/address` answers for the site.
async fn stored_operating_address(outposts: &Outposts, site_id: &str) -> Option<Value> {
    outposts
        .ok(
            "GET",
            &format!(
                "/sites/{}/address?AddressType={}",
                segment(site_id),
                query_value(OPERATING)
            ),
            None,
        )
        .await
        .get("Address")
        .cloned()
}

/// Delete every Outpost of `at_site` over the wire: this store's record of the
/// Outposts that belonged to the site having been deactivated.
async fn deactivate(outposts: &Outposts, at_site: &[String], case: &str) {
    for outpost_id in at_site {
        let deleted = outposts
            .call("DELETE", &format!("/outposts/{outpost_id}"), None)
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: the Outpost {outpost_id} must be deletable once its order is \
             finished, got {} {:?}",
            deleted.status, deleted.body
        );
    }
}

/// The rule, over every row: accepted inside either window, refused outside both.
#[tokio::test]
async fn the_operating_address_changes_inside_either_window_and_is_held_outside_both() {
    let before = address("US", "WA", "Seattle");
    let attempt = address("IE", "Leinster", "Dublin");

    for row in cases() {
        let case = row.case;
        // One store per row, so the order of the rows cannot matter.
        let outposts = Outposts::new();
        let site = outposts
            .create_site(&json!({ "Name": "windowed site", "OperatingAddress": before }))
            .await;
        let site_id = text(&site, "SiteId");
        let site_arn = text(&site, "SiteArn");

        let mut at_site: Vec<String> = Vec::new();
        for index in 0..row.outposts {
            let outpost = outposts
                .create_outpost(&json!({
                    "Name": format!("resident-{index}"),
                    "SiteId": site_id,
                }))
                .await;
            at_site.push(text(&outpost, "OutpostId"));
        }

        // The order the row places, where it places one. An order is placed at a site
        // through an Outpost of that site, so a row that places one at this site has
        // at least one.
        let order_at_site = match row.placed {
            Placed::Nowhere => None,
            Placed::AtAnotherSite => {
                let other = outposts
                    .create_site(&json!({ "Name": "another site" }))
                    .await;
                let elsewhere = outposts
                    .create_outpost(&json!({
                        "Name": "elsewhere",
                        "SiteId": text(&other, "SiteId"),
                    }))
                    .await;
                an_order(&outposts, &text(&elsewhere, "OutpostId")).await;
                None
            }
            Placed::HereInProgress => Some(an_order(&outposts, &at_site[0]).await),
            Placed::HereCancelled => {
                Some(a_cancelled_order_in(&outposts, HOME_REGION, &at_site[0]).await)
            }
            Placed::HereCompleted => Some(install_hardware(&outposts, &at_site[0]).await),
        };

        if row.deactivated {
            deactivate(&outposts, &at_site, case).await;
        }

        let named_by = match row.name_by {
            NameBy::Id => site_id.as_str(),
            NameBy::Arn => site_arn.as_str(),
        };
        let reply = update_operating_address(&outposts, named_by, &attempt).await;

        match row.expect {
            Expect::Accepted => {
                assert_eq!(
                    reply.status, 200,
                    "case {case:?}: the change is inside a window the model permits it in, \
                     so it must be accepted, got {} {:?}",
                    reply.status, reply.body
                );
                assert_eq!(
                    reply.body,
                    json!({ "AddressType": OPERATING, "Address": attempt }),
                    "case {case:?}: the reply must answer the address it stored, got {:?}",
                    reply.body
                );
                let read = stored_operating_address(&outposts, &site_id).await;
                assert_eq!(
                    read.as_ref(),
                    Some(&attempt),
                    "case {case:?}: the accepted change must read back, got {read:?}"
                );
            }
            Expect::HeldByAnOutpost => {
                assert_eq!(
                    (reply.status, reply.error_code.as_str()),
                    (409, CONFLICT_CODE),
                    "case {case:?}: outside both windows the change must be refused, got {} {:?}",
                    reply.status,
                    reply.body
                );
                let named = text(&reply.body, "ResourceId");
                assert_eq!(
                    (text(&reply.body, "ResourceType"), at_site.contains(&named)),
                    (RESOURCE_TYPE_OUTPOST.to_string(), true),
                    "case {case:?}: the conflict must name an Outpost of the site {at_site:?}, \
                     got {:?}",
                    reply.body
                );
                assert_eq!(
                    reply.body.get("Message").cloned(),
                    Some(json!(site_address_conflict(&site_id, &named))),
                    "case {case:?}: the conflict must state the limit it refuses on, got {:?}",
                    reply.body
                );
                let read = stored_operating_address(&outposts, &site_id).await;
                assert_eq!(
                    read.as_ref(),
                    Some(&before),
                    "case {case:?}: a refused change must write nothing, got {read:?}"
                );
            }
            Expect::HeldByTheOrder => {
                let order_id = order_at_site
                    .clone()
                    .unwrap_or_else(|| panic!("case {case:?}: this row places an order"));
                assert_eq!(
                    (reply.status, reply.error_code.as_str()),
                    (409, CONFLICT_CODE),
                    "case {case:?}: an order in progress must hold the change up, got {} {:?}",
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
                    "case {case:?}: the conflict must name the order in progress, got {:?}",
                    reply.body
                );
                let read = stored_operating_address(&outposts, &site_id).await;
                assert_eq!(
                    read.as_ref(),
                    Some(&before),
                    "case {case:?}: a refused change must write nothing, got {read:?}"
                );
            }
        }
    }
}

/// What closes the first window is the ORDER's site, not the Outpost record the order
/// was placed through: with that Outpost gone and another still at the site the change
/// is still refused, and it is accepted again once no Outpost belongs to the site.
#[tokio::test]
async fn the_closed_window_follows_the_order_at_the_site_not_its_outpost_record() {
    let outposts = Outposts::new();
    let before = address("US", "WA", "Seattle");
    let site = outposts
        .create_site(&json!({ "Name": "ordered site", "OperatingAddress": before }))
        .await;
    let site_id = text(&site, "SiteId");
    let ordered = text(
        &outposts
            .create_outpost(&json!({ "Name": "ordered", "SiteId": site_id }))
            .await,
        "OutpostId",
    );
    let resident = text(
        &outposts
            .create_outpost(&json!({ "Name": "resident", "SiteId": site_id }))
            .await,
        "OutpostId",
    );
    a_cancelled_order_in(&outposts, HOME_REGION, &ordered).await;
    // The Outpost the order was placed through goes; the order stays at the site.
    deactivate(&outposts, &[ordered], "the order's Outpost").await;

    let attempt = address("JP", "Tokyo", "Minato");
    let reply = update_operating_address(&outposts, &site_id, &attempt).await;
    assert_eq!(
        (reply.status, reply.error_code.as_str()),
        (409, CONFLICT_CODE),
        "case \"the order's Outpost gone, another still at the site\": the change must still \
         be refused, got {} {:?}",
        reply.status,
        reply.body
    );
    assert_eq!(
        text(&reply.body, "ResourceId"),
        resident,
        "case \"the order's Outpost gone, another still at the site\": the conflict must name \
         the Outpost that is still there, got {:?}",
        reply.body
    );
    let read = stored_operating_address(&outposts, &site_id).await;
    assert_eq!(
        read.as_ref(),
        Some(&before),
        "case \"the order's Outpost gone, another still at the site\": nothing must be \
         written, got {read:?}"
    );

    // The second window: with no Outpost left at the site the same change is accepted.
    deactivate(&outposts, &[resident], "the last Outpost at the site").await;
    let reply = update_operating_address(&outposts, &site_id, &attempt).await;
    assert_eq!(
        reply.status, 200,
        "case \"no Outpost left at the site\": the change must be accepted, got {} {:?}",
        reply.status, reply.body
    );
    let read = stored_operating_address(&outposts, &site_id).await;
    assert_eq!(
        read.as_ref(),
        Some(&attempt),
        "case \"no Outpost left at the site\": the accepted change must read back, got {read:?}"
    );
}
